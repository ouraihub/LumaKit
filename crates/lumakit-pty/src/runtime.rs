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
    writer: Mutex<Box<dyn Write + Send>>,
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
            let tx = tx.clone();
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
                    drop(shared.master.lock().take());
                    let _ = tx.try_send(RuntimeEvent::Exited { exit_code: code });
                })
                .map_err(|e| RuntimeError::Pty(format!("spawn waiter: {e}")))?
        };

        let reader_handle = {
            let shared = shared.clone();
            let tx = tx.clone();
            thread::Builder::new()
                .name("lumakit-pty-reader".into())
                .spawn(move || read_loop(reader, decoder, shared, tx))
                .map_err(|e| RuntimeError::Pty(format!("spawn reader: {e}")))?
        };

        Ok(Self {
            shared,
            writer: Mutex::new(writer),
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
    /// Returns [`RuntimeError::Closed`] after shutdown; OS write errors wrap
    /// to `RuntimeError::Pty`.
    pub fn write(&self, bytes: &[u8]) -> Result<(), RuntimeError> {
        if self.shared.stopped.load(Ordering::Acquire) {
            return Err(RuntimeError::Closed);
        }
        let mut guard = self.writer.lock();
        guard
            .write_all(bytes)
            .map_err(|e| RuntimeError::Pty(format!("write: {e}")))?;
        guard
            .flush()
            .map_err(|e| RuntimeError::Pty(format!("flush: {e}")))
    }

    /// Resize the PTY.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the master is gone or the resize fails.
    pub fn resize(&self, size: TermSize) -> Result<(), RuntimeError> {
        validate_size(size)?;
        if let Some(master) = self.shared.master.lock().as_ref() {
            master
                .resize(pty_size(size))
                .map_err(|e| RuntimeError::Pty(format!("resize: {e}")))?;
        }
        Ok(())
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

    /// Shut the runtime down: close the master, signal the waiter / reader
    /// threads, kill the child if it hasn't exited within [`EXIT_TIMEOUT`].
    ///
    /// # Errors
    /// Returns [`RuntimeError`] if killing the child fails.
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        if self.shared.stopped.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        drop(self.shared.master.lock().take());
        let deadline = Instant::now() + EXIT_TIMEOUT;
        while !self.shared.exited.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                let _ = self.killer.lock().kill();
                break;
            }
            thread::sleep(POLL);
        }
        join(&self.waiter);
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

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    mut decoder: Decoder,
    shared: Arc<Shared>,
    tx: SyncSender<RuntimeEvent>,
) {
    let mut buffer = [0u8; READ_BUFFER];
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        let n = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let raw = buffer[..n].to_vec();
        shared.ring.lock().push(&raw);
        let text = decoder.feed(&raw);
        let event = RuntimeEvent::Data { raw, text };
        if !send(&tx, &shared.stopped, event) {
            break;
        }
    }
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
}
