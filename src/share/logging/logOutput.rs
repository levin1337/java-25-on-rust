/*
 * Port of OpenJDK 25 HotSpot LogOutput hierarchy
 * (share/logging/logOutput.hpp, logFileOutput.cpp, logStdoutOutput is tty)
 *
 * LogOutput is the sink for a single -Xlog output configuration
 * (e.g. "file=hotspot.log" or default stderr).
 */

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

/// A single configured log output destination.
pub trait LogOutput: Send + Sync {
    /// Write one fully decorated log line (no trailing newline added).
    fn write(&self, line: &str) -> io::Result<()>;
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
    fn name(&self) -> &str;
}

/// Default output: JVM stderr (tty equivalent).
pub struct StderrOutput;

impl LogOutput for StderrOutput {
    fn write(&self, line: &str) -> io::Result<()> {
        let mut err = io::stderr().lock();
        err.write_all(line.as_bytes())?;
        err.write_all(b"\n")?;
        err.flush()
    }

    fn name(&self) -> &str {
        "stderr"
    }
}

/// stdout output (for -Xlog:...:stdout style configs).
pub struct StdoutOutput;

impl LogOutput for StdoutOutput {
    fn write(&self, line: &str) -> io::Result<()> {
        let mut out = io::stdout().lock();
        out.write_all(line.as_bytes())?;
        out.write_all(b"\n")?;
        out.flush()
    }

    fn name(&self) -> &str {
        "stdout"
    }
}

/// file=path output (logFileOutput.cpp). Supports "file=path" and
/// append semantics; HotSpot rotates on restart — we open in append mode.
pub struct FileOutput {
    path: PathBuf,
    file: Mutex<File>,
}

impl FileOutput {
    pub fn create(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }
}

impl LogOutput for FileOutput {
    fn write(&self, line: &str) -> io::Result<()> {
        let mut f = self.file.lock().unwrap();
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()
    }

    fn flush(&self) -> io::Result<()> {
        self.file.lock().unwrap().flush()
    }

    fn name(&self) -> &str {
        self.path.to_str().unwrap_or("<file>")
    }
}

/// Output that discards everything (log disabled sink).
pub struct NullOutput;

impl LogOutput for NullOutput {
    fn write(&self, _line: &str) -> io::Result<()> {
        Ok(())
    }
    fn name(&self) -> &str {
        "null"
    }
}
