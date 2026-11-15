//! Thread-safe `Write` sink shared by all Java threads.
//!
//! `println`/`print` from any OS thread serializes on an inner mutex so
//! output from concurrent Java threads does not interleave mid-write.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
pub struct JvmOut {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl JvmOut {
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
        }
    }

    pub fn stdout() -> Self {
        Self::new(Box::new(io::LineWriter::new(io::stdout())))
    }
}

impl Write for JvmOut {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().unwrap().write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.lock().unwrap().write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().unwrap().flush()
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.inner.lock().unwrap().write_vectored(bufs)
    }

    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> io::Result<()> {
        self.inner.lock().unwrap().write_fmt(args)
    }
}

static INSTALLED: OnceLock<JvmOut> = OnceLock::new();

/// Installs the process-wide output sink; spawned Java threads clone it.
pub fn install(out: &JvmOut) {
    let _ = INSTALLED.set(out.clone());
}

/// Returns a clone of the installed sink for a new Java thread.
pub fn shared() -> Option<JvmOut> {
    INSTALLED.get().cloned()
}
