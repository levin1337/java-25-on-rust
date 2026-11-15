/*
 * Port of OpenJDK 25 HotSpot logStream (share/logging/logStream.hpp/.cpp)
 *
 * LogStream is a line-buffered RAII output stream bound to a (level,
 * tagset) pair. Text ending in '\n' is flushed immediately as one log
 * record; any partial line is flushed on drop. is_enabled() mirrors
 * log_is_enabled so callers can skip expensive formatting.
 */

use super::logLevel::LogLevel;
use super::logTag::LogTag;
use super::logTagSet::LogTagSet;
use super::logConfiguration::LogConfiguration;
use std::fmt::Write as _;

/// RAII log stream for one (level, tag set) pair.
pub struct LogStream {
    tagset: LogTagSet,
    level: LogLevel,
    line: String,
    enabled: bool,
}

impl LogStream {
    /// LogStream(level, tagset) constructor.
    pub fn new(tagset: LogTagSet, level: LogLevel) -> Self {
        let enabled = LogConfiguration::global().is_level_enabled(&tagset, level);
        Self {
            tagset,
            level,
            line: String::with_capacity(256),
            enabled,
        }
    }

    /// Convenience constructor for a single-tag set.
    pub fn for_tag(tag: LogTag, level: LogLevel) -> Self {
        Self::new(LogTagSet::for_tag(tag), level)
    }

    /// logStream::is_enabled / LogTargetHandle::is_enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// logStream::print — append text; embedded '\n' flushes complete
    /// lines as separate log records.
    pub fn print(&mut self, s: &str) -> &mut Self {
        if !self.enabled {
            return self;
        }
        let mut rest = s;
        while let Some(pos) = rest.find('\n') {
            self.line.push_str(&rest[..pos]);
            self.flush_line();
            rest = &rest[pos + 1..];
        }
        self.line.push_str(rest);
        self
    }

    /// outputStream::print_cr — append text followed by a newline.
    pub fn print_cr(&mut self, s: &str) -> &mut Self {
        self.print(s);
        self.print("\n");
        self
    }

    /// print-style formatted write (like `*stream << args`).
    pub fn print_fmt(&mut self, args: std::fmt::Arguments) -> &mut Self {
        let mut s = String::new();
        let _ = write!(s, "{}", args);
        self.print(&s)
    }

    /// Borrow the pending (unterminated) line.
    pub fn pending(&self) -> &str {
        &self.line
    }

    /// Flush the pending partial line as a record (if any).
    pub fn flush_line(&mut self) {
        if !self.line.is_empty() {
            let msg = std::mem::take(&mut self.line);
            super::log::Log::write_tagset(&self.tagset, self.level, &msg);
        }
    }
}

impl Drop for LogStream {
    /// ~LogStreamImpl — writes any unfinished output left in the buffer.
    fn drop(&mut self) {
        self.flush_line();
    }
}

impl std::fmt::Write for LogStream {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.print(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_line_splitting() {
        super::super::logConfiguration::LogConfiguration::global().reset();
        // Warning is enabled by the default stdout configuration.
        let mut s = LogStream::new(LogTagSet::new(&[LogTag::Logging]), LogLevel::Warning);
        assert!(s.is_enabled());
        s.print("hello\nwor");
        assert_eq!(s.pending(), "wor");
        s.print("ld\n");
        assert_eq!(s.pending(), "");
        drop(s);
    }

    #[test]
    fn stream_disabled_skips_buffering() {
        super::super::logConfiguration::LogConfiguration::global().reset();
        super::super::logConfiguration::LogConfiguration::global()
            .parse_xlog("all=off")
            .unwrap();
        let mut s = LogStream::new(LogTagSet::new(&[LogTag::Gc]), LogLevel::Warning);
        assert!(!s.is_enabled());
        s.print("x\n");
        assert_eq!(s.pending(), "");
        super::super::logConfiguration::LogConfiguration::global().reset();
    }
}
