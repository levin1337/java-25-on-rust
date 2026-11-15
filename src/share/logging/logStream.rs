/*
 * Port of OpenJDK 25 HotSpot logStream (share/logging/logStream.hpp)
 *
 * logStream is a buffered, formatted output stream used by the unified
 * logging framework. Lines are assembled with print/print_cr/vprint-style
 * helpers and flushed through a LogOutput sink.
 */

use super::logDecorations::Decorations;
use super::logLevel::LogLevel;
use super::logOutput::LogOutput;
use super::logTag::LogTag;
use std::fmt::Write as _;

/// Buffered log stream for one log record.
pub struct LogStream {
    buffer: String,
    tag: LogTag,
    level: LogLevel,
}

impl LogStream {
    pub fn new(tag: LogTag, level: LogLevel) -> Self {
        Self {
            buffer: String::with_capacity(256),
            tag,
            level,
        }
    }

    /// logStream::print
    pub fn print(&mut self, s: &str) -> &mut Self {
        self.buffer.push_str(s);
        self
    }

    /// logStream::print_cr — newline.
    pub fn print_cr(&mut self) -> &mut Self {
        self.buffer.push('\n');
        self
    }

    /// logStream::vprint-style formatted write.
    pub fn print_fmt(&mut self, args: std::fmt::Arguments) -> &mut Self {
        let _ = write!(self.buffer, "{}", args);
        self
    }

    /// Borrow the accumulated buffer.
    pub fn as_str(&self) -> &str {
        &self.buffer
    }

    /// Consume the stream and return the buffered text.
    pub fn into_string(self) -> String {
        self.buffer
    }

    /// Convenience: formatted print with args (like `*stream << args`).
    pub fn print_args(&mut self, args: std::fmt::Arguments) -> &mut Self {
        self.print_fmt(args)
    }

    /// Commit the buffered record through `output` with decorations applied.
    /// Multi-line messages get each line decorated (logDecorators behavior).
    pub fn commit(self, output: &dyn LogOutput, deco: &Decorations) {
        emit_lines(
            self.tag,
            self.level,
            &self.buffer,
            output,
            deco,
        );
    }

    /// Convenience: build prefix + message in one call without a stream.
    pub fn emit(
        tag: LogTag,
        level: LogLevel,
        message: &str,
        output: &dyn LogOutput,
        deco: &Decorations,
    ) {
        emit_lines(tag, level, message, output, deco);
    }
}

impl Default for LogStream {
    fn default() -> Self {
        Self::new(LogTag::All, LogLevel::Info)
    }
}

fn emit_lines(
    tag: LogTag,
    level: LogLevel,
    message: &str,
    output: &dyn LogOutput,
    deco: &Decorations,
) {
    let uptime = crate::share::runtime::os::Os::elapsed_time();
    #[cfg(windows)]
    let pid = crate::share::platform::windows::OsWindows::current_process_id();
    #[cfg(not(windows))]
    let pid = std::process::id();
    #[cfg(windows)]
    let tid = crate::share::platform::windows::OsWindows::current_thread_id() as u64;
    #[cfg(not(windows))]
    let tid = 0u64;

    let mut wrote_any = false;
    for line in message.lines() {
        let prefix = deco.decorate(level.name(), tag, uptime, pid, tid);
        let _ = output.write(&format!("{}{}", prefix, line));
        wrote_any = true;
    }
    if !wrote_any {
        let prefix = deco.decorate(level.name(), tag, uptime, pid, tid);
        let _ = output.write(&prefix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::logging::logOutput::NullOutput;

    #[test]
    fn stream_buffers() {
        let mut s = LogStream::new(LogTag::Gc, LogLevel::Info);
        s.print("hello ").print("world");
        assert_eq!(s.buffer, "hello world");
        s.print_cr();
        assert!(s.buffer.ends_with('\n'));
    }

    #[test]
    fn emit_to_null() {
        LogStream::emit(
            LogTag::Init,
            LogLevel::Info,
            "boot complete",
            &NullOutput,
            &Decorations::none(),
        );
    }
}
