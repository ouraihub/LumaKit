//! `PtyRuntime`: one live PTY-backed shell, with waiter and reader threads.
//! Ported from autorail `crates/terminal/src/runtime.rs`; the persistent
//! history is replaced by a host-sized ring buffer, and the service wiring
//! is factored out so other hosts (ThinkRail, AutoTerm) can reuse this layer
//! without dragging in session-id and persistence concerns.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;
use tracing::warn;

use crate::decode::Decoder;
use crate::ring::RingBuffer;

const READ_BUFFER: usize = 8192;
const QUEUE_CHUNKS: usize = 64;
const POLL: Duration = Duration::from_millis(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_RING: usize = 128 * 1024;

/// Terminal grid dimensions in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Column count (>=1).
    pub cols: u16,
    /// Row count (>=1).
    pub rows: u16,
}

/// Options for spawning a PTY-backed shell.
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    /// Program to execute (absolute path recommended).
    pub program: PathBuf,
    /// Arguments passed verbatim.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Grid size.
    pub size: TermSize,
    /// Encoding label (e.g. `"utf-8"`, `"gbk"`).
    pub encoding: String,
    /// Environment overrides. Base environment is inherited.
    pub env: Vec<(String, String)>,
    /// Ring buffer capacity, in bytes. 0 falls back to [`DEFAULT_RING`].
    pub ring_bytes: usize,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            program: PathBuf::new(),
            args: Vec::new(),
            cwd: PathBuf::from("."),
            size: TermSize { cols: 80, rows: 24 },
            encoding: "utf-8".to_owned(),
            env: Vec::new(),
            ring_bytes: DEFAULT_RING,
        }
    }
}

/// Events the runtime emits.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    /// Fresh bytes arrived from the PTY. Already passed through the decoder;
    /// the raw copy is also in the ring buffer.
    Data {
        /// Original bytes from the PTY.
        raw: Vec<u8>,
        /// Decoded UTF-8 text (may be empty when a chunk ended mid-sequence).
        text: String,
    },
    /// Child exited.
    Exited {
        /// Exit code reported by the OS, if any.
        exit_code: Option<i32>,
    },
}

/// Runtime errors.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Portable PTY surface failed.
    #[error("pty error: {0}")]
    Pty(String),
    /// Decoder construction failed.
    #[error(transparent)]
    Decode(#[from] crate::decode::DecodeError),
    /// Session already stopped.
    #[error("session closed")]
    Closed,
    /// Grid size out of range.
    #[error("invalid grid size: {0}x{1}")]
    InvalidSize(u16, u16),
    /// Process did not exit within the shutdown timeout.
    #[error("process did not exit within {0:?}")]
    ExitTimeout(Duration),
}

struct Shared {
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    ring: Mutex<RingBuffer>,
    exit_code: Mutex<Option<i32>>,
    exited: AtomicBool,
    stopped: AtomicBool,
}

/// A live PTY runtime handle.
pub struct PtyRuntime {
    shared: Arc<Shared>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    waiter: Mutex<Option<JoinHandle<()>>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    events: Mutex<Option<Receiver<RuntimeEvent>>>,
}

impl std::fmt::Debug for PtyRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyRuntime").finish_non_exhaustive()
    }
}

impl PtyRuntime {
    /// Spawn a PTY-backed shell. On success returns the runtime; on error no
    /// child remains alive.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the PTY cannot be opened, the shell
    /// fails to launch, or the decoder label is unknown.
    pub fn spawn(options: SpawnOptions) -> Result<Self, RuntimeError> {
        validate_size(options.size)?;
        let ring_capacity = if options.ring_bytes == 0 {
            DEFAULT_RING
        } else {
            options.ring_bytes
        };

        let pty = native_pty_system();
        let pair = pty
            .openpty(pty_size(options.size))
            .map_err(|e| RuntimeError::Pty(format!("openpty: {e}")))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| RuntimeError::Pty(format!("clone reader: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| RuntimeError::Pty(format!("take writer: {e}")))?;

        let mut cmd = CommandBuilder::new(&options.program);
        for arg in &options.args {
            cmd.arg(arg);
        }
        cmd.cwd(options.cwd.clone());
        for (k, v) in &options.env {
            cmd.env(k, v);
        }
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| RuntimeError::Pty(format!("spawn: {e}")))?;
        drop(pair.slave);
        let killer = child.clone_killer();

        let decoder = Decoder::new(&options.encoding)?;
        let shared = Arc::new(Shared {
            master: Mutex::new(Some(pair.master)),
            ring: Mutex::new(RingBuffer::new(ring_capacity)),
            exit_code: Mutex::new(None),
            exited: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });

        let (tx, rx) = mpsc::sync_channel::<RuntimeEvent>(QUEUE_CHUNKS);
        let waiter = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("lumakit-pty-waiter".into())
                .spawn(move || {
                    let status = child.wait();
                    let code = status
                        .as_ref()
                        .ok()
                        .map(portable_pty::ExitStatus::exit_code)
                        .map(|c| c as i32);
                    *shared.exit_code.lock() = code;
                    shared.exited.store(true, Ordering::Release);
                    // Closing the master is what ends the reader on Windows ConPTY.
                    drop(shared.master.lock().take());
                    if status.is_err() {
                        warn!("lumakit-pty: wait for child failed");
                    }
                })
                .map_err(|e| RuntimeError::Pty(format!("spawn waiter: {e}")))?
        };

        let reader_handle = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("lumakit-pty-reader".into())
                .spawn(move || read_loop(reader, decoder, shared, tx))
                .map_err(|e| RuntimeError::Pty(format!("spawn reader: {e}")))?
        };

        Ok(Self {
            shared,
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(killer),
            waiter: Mutex::new(Some(waiter)),
            reader: Mutex::new(Some(reader_handle)),
            events: Mutex::new(Some(rx)),
        })
    }

    /// Take the event receiver. Callable once; subsequent calls return
    /// `None`.
    pub fn take_events(&self) -> Option<Receiver<RuntimeEvent>> {
        self.events.lock().take()
    }

    /// Write `bytes` to the PTY.
    ///
    /// # Errors
    /// Returns [`RuntimeError::Closed`] after shutdown or once the child has
    /// exited; OS write errors wrap to `RuntimeError::Pty`.
    pub fn write(&self, bytes: &[u8]) -> Result<(), RuntimeError> {
        let mut guard = self.writer.lock();
        let writer = guard
            .as_mut()
            .filter(|_| !self.is_exited())
            .ok_or(RuntimeError::Closed)?;
        writer
            .write_all(bytes)
            .map_err(|e| RuntimeError::Pty(format!("write: {e}")))?;
        writer
            .flush()
            .map_err(|e| RuntimeError::Pty(format!("flush: {e}")))
    }

    /// Resize the PTY.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the master is gone or the resize fails.
    pub fn resize(&self, size: TermSize) -> Result<(), RuntimeError> {
        validate_size(size)?;
        self.shared
            .master
            .lock()
            .as_ref()
            .ok_or(RuntimeError::Closed)?
            .resize(pty_size(size))
            .map_err(|e| RuntimeError::Pty(format!("resize: {e}")))
    }

    /// Copy the ring-buffered raw bytes (oldest first).
    pub fn ring_snapshot(&self) -> Vec<u8> {
        self.shared.ring.lock().snapshot()
    }

    /// `true` when the child exited.
    pub fn is_exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }

    /// Recorded exit code, when the child has exited.
    pub fn exit_code(&self) -> Option<i32> {
        *self.shared.exit_code.lock()
    }

    /// End the shell and join its threads. The child is killed unless it
    /// already exited; success means the exit was observed within
    /// [`EXIT_TIMEOUT`]. portable-pty 0.9.0's Windows `kill` can report an
    /// error even when it worked, so its result alone is not proof.
    ///
    /// # Errors
    /// [`RuntimeError::ExitTimeout`] when the child is still alive after the
    /// timeout (or [`RuntimeError::Pty`] carrying the kill error).
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        self.shared.stopped.store(true, Ordering::Release);
        let kill_error = if self.is_exited() {
            None
        } else {
            self.killer.lock().kill().err()
        };
        self.writer.lock().take();
        let deadline = Instant::now() + EXIT_TIMEOUT;
        while !self.is_exited() && Instant::now() < deadline {
            thread::sleep(POLL);
        }
        if !self.is_exited() {
            return Err(match kill_error {
                Some(error) => RuntimeError::Pty(format!("kill: {error}")),
                None => RuntimeError::ExitTimeout(EXIT_TIMEOUT),
            });
        }
        join(&self.waiter);
        drop(self.shared.master.lock().take());
        join(&self.reader);
        Ok(())
    }
}

impl Drop for PtyRuntime {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            warn!(%error, "lumakit-pty: shutdown failed on drop");
        }
    }
}

fn validate_size(size: TermSize) -> Result<(), RuntimeError> {
    if size.cols == 0 || size.rows == 0 {
        return Err(RuntimeError::InvalidSize(size.cols, size.rows));
    }
    Ok(())
}

fn pty_size(size: TermSize) -> PtySize {
    PtySize {
        cols: size.cols,
        rows: size.rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn join(handle: &Mutex<Option<JoinHandle<()>>>) {
    if let Some(handle) = handle.lock().take() {
        let _ = handle.join();
    }
}

/// Reads until end of stream, then waits (bounded) for the waiter to record
/// the exit code and sends [`RuntimeEvent::Exited`] after the last data, so
/// consumers never see output arrive after the exit. After shutdown the PTY
/// is still drained, without emitting, until it closes.
fn read_loop(
    mut reader: Box<dyn Read + Send>,
    mut decoder: Decoder,
    shared: Arc<Shared>,
    tx: SyncSender<RuntimeEvent>,
) {
    let mut buffer = [0u8; READ_BUFFER];
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if !shared.exited.load(Ordering::Acquire) && !shared.stopped.load(Ordering::Acquire)
                {
                    warn!(io_kind = ?error.kind(), "lumakit-pty: read failed");
                }
                break;
            }
        };
        if shared.stopped.load(Ordering::Acquire) {
            continue;
        }
        let raw = buffer[..n].to_vec();
        shared.ring.lock().push(&raw);
        let text = decoder.feed(&raw);
        if !send(&tx, &shared.stopped, RuntimeEvent::Data { raw, text }) {
            return;
        }
    }
    if shared.stopped.load(Ordering::Acquire) {
        return;
    }
    let tail = decoder.finish();
    if !tail.is_empty()
        && !send(
            &tx,
            &shared.stopped,
            RuntimeEvent::Data {
                raw: Vec::new(),
                text: tail,
            },
        )
    {
        return;
    }
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while !shared.exited.load(Ordering::Acquire)
        && !shared.stopped.load(Ordering::Acquire)
        && Instant::now() < deadline
    {
        thread::sleep(POLL);
    }
    if !shared.exited.load(Ordering::Acquire) && !shared.stopped.load(Ordering::Acquire) {
        warn!(
            limit_ms = EXIT_TIMEOUT.as_millis() as u64,
            "lumakit-pty: exit unconfirmed"
        );
    }
    let exit_code = *shared.exit_code.lock();
    send(&tx, &shared.stopped, RuntimeEvent::Exited { exit_code });
}

fn send(tx: &SyncSender<RuntimeEvent>, stopped: &AtomicBool, mut event: RuntimeEvent) -> bool {
    loop {
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Full(e)) => {
                if stopped.load(Ordering::Acquire) {
                    return false;
                }
                event = e;
                thread::sleep(POLL);
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn invalid_size_rejected() {
        let options = SpawnOptions {
            size: TermSize { cols: 0, rows: 0 },
            program: PathBuf::from("/does/not/matter"),
            ..SpawnOptions::default()
        };
        assert!(matches!(
            PtyRuntime::spawn(options),
            Err(RuntimeError::InvalidSize(_, _))
        ));
    }

    /// Real ConPTY: output precedes the exit event, the exit code is
    /// reported, and writes after the exit are refused.
    #[cfg(windows)]
    #[test]
    #[ignore = "Starts a disposable real Windows ConPTY cmd.exe process"]
    fn exit_follows_output_and_closes_writes() {
        let runtime = PtyRuntime::spawn(SpawnOptions {
            program: PathBuf::from("cmd.exe"),
            args: vec![
                "/D".into(),
                "/C".into(),
                "echo LUMAKIT_PTY_MARK & exit 7".into(),
            ],
            cwd: std::env::temp_dir(),
            ..SpawnOptions::default()
        })
        .unwrap();
        let events = runtime.take_events().unwrap();
        let mut output = String::new();
        let exit = loop {
            let event = events
                .recv_timeout(Duration::from_secs(15))
                .unwrap_or_else(|_| panic!("no exit event; output so far: {output:?}"));
            match event {
                RuntimeEvent::Data { text, .. } => {
                    // ConPTY waits for a cursor-position report before it
                    // runs the child; a terminal emulator would answer.
                    if text.contains("\u{1b}[6n") {
                        runtime.write(b"\x1b[1;1R").unwrap();
                    }
                    output.push_str(&text);
                }
                RuntimeEvent::Exited { exit_code } => break exit_code,
            }
        };
        assert!(
            output.contains("LUMAKIT_PTY_MARK"),
            "no output before exit: {output:?}"
        );
        assert_eq!(exit, Some(7));
        assert!(matches!(runtime.write(b"x"), Err(RuntimeError::Closed)));
        runtime.shutdown().unwrap();
    }
}
