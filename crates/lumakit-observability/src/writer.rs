use std::{
    cell::Cell,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

static REPORTED_FAILURE: AtomicBool = AtomicBool::new(false);
thread_local! {
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

struct Writing;

impl Drop for Writing {
    fn drop(&mut self) {
        WRITING.set(false);
    }
}

// Never send writer failures back through tracing, or print an OS error containing a path.
fn report_failure() {
    if !REPORTED_FAILURE.swap(true, Ordering::Relaxed) {
        let _ = io::stderr().write_all(b"logging.writer.failed\n");
    }
}

#[derive(Clone, Copy)]
struct Failure {
    kind: io::ErrorKind,
    os_code: Option<i32>,
}

impl Failure {
    fn capture(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            os_code: error.raw_os_error(),
        }
    }

    fn error(self) -> io::Error {
        self.os_code.map_or_else(
            || io::Error::new(self.kind, "log writer failed"),
            io::Error::from_raw_os_error,
        )
    }
}

struct FileWriter {
    file: File,
    path: PathBuf,
    max_bytes: u64,
    failure: Option<Failure>,
}

impl FileWriter {
    fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file,
            path,
            max_bytes,
            failure: None,
        })
    }

    fn rotate(&mut self, incoming: usize) -> io::Result<()> {
        if self.file.metadata()?.len().saturating_add(incoming as u64) <= self.max_bytes {
            return Ok(());
        }
        self.file.flush()?;
        let old = self.path.with_extension("log.1");
        match fs::remove_file(&old) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(&self.path, &old)?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        Ok(())
    }

    fn remember<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| Failure::capture(error));
            report_failure();
        }
        result
    }
}

impl Write for FileWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(failure) = self.failure {
            return Err(failure.error());
        }
        let result = self
            .rotate(bytes.len())
            .and_then(|()| self.file.write(bytes));
        self.remember(result)
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if let Some(failure) = self.failure {
            return Err(failure.error());
        }
        let result = self
            .rotate(bytes.len())
            .and_then(|()| self.file.write_all(bytes));
        self.remember(result)
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.file.flush();
        self.remember(result)?;
        match self.failure {
            Some(failure) => Err(failure.error()),
            None => Ok(()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SharedWriter(Arc<Mutex<FileWriter>>);

impl SharedWriter {
    pub(crate) fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        Ok(Self(Arc::new(Mutex::new(FileWriter::open(
            path, max_bytes,
        )?))))
    }

    fn with_writer<T>(
        &self,
        write: impl FnOnce(&mut FileWriter) -> io::Result<T>,
    ) -> io::Result<T> {
        // A panic originating inside this writer cannot lock its own mutex. Other threads
        // still wait for the normal synchronous writer rather than dropping a panic record.
        let result = if WRITING.get() {
            Err(io::Error::other("recursive log writer access"))
        } else {
            self.0
                .lock()
                .map_err(|_| io::Error::other("log writer poisoned"))
                .and_then(|mut writer| {
                    WRITING.set(true);
                    let _writing = Writing;
                    write(&mut writer)
                })
        };
        if result.is_err() {
            report_failure();
        }
        result
    }
}

impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.with_writer(|writer| writer.write(bytes))
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.with_writer(|writer| writer.write_all(bytes))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.with_writer(Write::flush)
    }
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
