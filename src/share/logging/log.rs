/*
 * Port of OpenJDK 25 HotSpot Log / LogImpl / LogTargetImpl
 * (share/logging/log.hpp, logTagSet.cpp)
 *
 * Log dispatches a record through the active LogConfiguration: the tag
 * set's level is resolved per configured output, the message is decorated
 * (or enqueued to the async writer) and written to each output. A bounded
 * in-memory ring keeps recent messages for diagnostics (VM.log "print").
 */

use super::logAsyncWriter;
use super::logConfiguration::LogConfiguration;
use super::logDecorations::LogDecorations;
use super::logLevel::LogLevel;
use super::logTag::LogTag;
use super::logTagSet::LogTagSet;
use std::collections::VecDeque;
use std::sync::Mutex;

const MAX_RETAINED_MESSAGES: usize = 4096;

/// One retained log record (LogMessage equivalent).
#[derive(Debug, Clone)]
pub struct LogMessage {
    pub tagset: LogTagSet,
    pub level: LogLevel,
    pub text: String,
}

struct LogState {
    messages: VecDeque<LogMessage>,
}

static LOG_STATE: Mutex<LogState> = Mutex::new(LogState {
    messages: VecDeque::new(),
});

/// Namespace of static log functions (LogImpl / log_* macros equivalent).
pub struct Log;

impl Log {
    fn tagset_for(tags: &[LogTag]) -> LogTagSet {
        LogTagSet::try_new(tags).unwrap_or_else(|| LogTagSet::for_tag(LogTag::Logging))
    }

    /// log_is_enabled(level, tags...) — true if any output accepts the
    /// record at the given level for this tag set.
    pub fn is_level_enabled(tagset: &LogTagSet, level: LogLevel) -> bool {
        LogConfiguration::global().is_level_enabled(tagset, level)
    }

    pub fn is_level_enabled_tag(tag: LogTag, level: LogLevel) -> bool {
        Self::is_level_enabled(&LogTagSet::for_tag(tag), level)
    }

    pub fn is_level_enabled_tags(tags: &[LogTag], level: LogLevel) -> bool {
        Self::is_level_enabled(&Self::tagset_for(tags), level)
    }

    /// LogTagSet::log(level, msg) — core write path:
    /// filter per output → decorate → emit (sync or async) → retain.
    pub fn write_tagset(tagset: &LogTagSet, level: LogLevel, msg: &str) {
        let cfg = LogConfiguration::global();
        if level == LogLevel::Off || level == LogLevel::NotMentioned {
            return;
        }
        let outputs = cfg.outputs_snapshot();
        if outputs.is_empty() {
            return;
        }

        // Fast path: no output accepts this record.
        let mut any = false;
        let mut targets: Vec<(usize, _)> = Vec::new();
        for (idx, (output, decorators)) in outputs.iter().enumerate() {
            let lvl = cfg.level_for(tagset, idx);
            if lvl != LogLevel::Off && level <= lvl {
                any = true;
                targets.push((idx, (output.clone(), *decorators)));
            }
        }
        if !any {
            return;
        }

        // Retain for diagnostics.
        {
            let mut state = LOG_STATE.lock().unwrap();
            if state.messages.len() >= MAX_RETAINED_MESSAGES {
                state.messages.pop_front();
            }
            state.messages.push_back(LogMessage {
                tagset: tagset.clone(),
                level,
                text: msg.to_string(),
            });
        }

        let async_mode = cfg.async_mode();
        let use_async = async_mode != logAsyncWriter::AsyncMode::Off;

        for (idx, (output, decorators)) in targets {
            let _ = idx;
            if use_async {
                // Values are resolved at the log site (LogDecorations
                // constructor), then handed to the writer thread.
                let deco = LogDecorations::new(level, tagset, &decorators);
                logAsyncWriter::enqueue_record(
                    output,
                    deco,
                    decorators,
                    msg.to_string(),
                    async_mode,
                );
            } else {
                let deco = LogDecorations::new(level, tagset, &decorators);
                let _ = output.write_record(&deco, &decorators, msg);
            }
        }
    }

    /// Single-tag convenience (log_debug(gc)("...") equivalent).
    pub fn write(tag: LogTag, level: LogLevel, msg: &str) {
        Self::write_tagset(&LogTagSet::for_tag(tag), level, msg);
    }

    /// Multi-tag tag set (log_debug(gc, heap)("...") equivalent).
    pub fn write_tags(tags: &[LogTag], level: LogLevel, msg: &str) {
        Self::write_tagset(&Self::tagset_for(tags), level, msg);
    }

    pub fn error(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Error, msg);
    }
    pub fn warning(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Warning, msg);
    }
    pub fn info(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Info, msg);
    }
    pub fn debug(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Debug, msg);
    }
    pub fn trace(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Trace, msg);
    }

    /// Tag-set variants of the level helpers.
    pub fn error_tagset(tagset: &LogTagSet, msg: &str) {
        Self::write_tagset(tagset, LogLevel::Error, msg);
    }
    pub fn warning_tagset(tagset: &LogTagSet, msg: &str) {
        Self::write_tagset(tagset, LogLevel::Warning, msg);
    }
    pub fn info_tagset(tagset: &LogTagSet, msg: &str) {
        Self::write_tagset(tagset, LogLevel::Info, msg);
    }
    pub fn debug_tagset(tagset: &LogTagSet, msg: &str) {
        Self::write_tagset(tagset, LogLevel::Debug, msg);
    }
    pub fn trace_tagset(tagset: &LogTagSet, msg: &str) {
        Self::write_tagset(tagset, LogLevel::Trace, msg);
    }

    /// Multi-tag level helpers.
    pub fn error_tags(tags: &[LogTag], msg: &str) {
        Self::write_tags(tags, LogLevel::Error, msg);
    }
    pub fn warning_tags(tags: &[LogTag], msg: &str) {
        Self::write_tags(tags, LogLevel::Warning, msg);
    }
    pub fn info_tags(tags: &[LogTag], msg: &str) {
        Self::write_tags(tags, LogLevel::Info, msg);
    }
    pub fn debug_tags(tags: &[LogTag], msg: &str) {
        Self::write_tags(tags, LogLevel::Debug, msg);
    }
    pub fn trace_tags(tags: &[LogTag], msg: &str) {
        Self::write_tags(tags, LogLevel::Trace, msg);
    }

    /// Formatted write with explicit level (vwrite equivalent).
    pub fn write_fmt(tagset: &LogTagSet, level: LogLevel, args: std::fmt::Arguments) {
        let mut s = String::new();
        use std::fmt::Write as _;
        let _ = write!(s, "{}", args);
        Self::write_tagset(tagset, level, &s);
    }

    /// RAII stream bound to (tag, level) — LogStreamHandle equivalent.
    pub fn stream(tag: LogTag, level: LogLevel) -> super::logStream::LogStream {
        super::logStream::LogStream::for_tag(tag, level)
    }

    pub fn stream_tagset(tagset: &LogTagSet, level: LogLevel) -> super::logStream::LogStream {
        super::logStream::LogStream::new(tagset.clone(), level)
    }

    /// Structured multi-part message builder (logStream-style).
    pub fn write_stream(tag: LogTag, level: LogLevel, build: impl FnOnce(&mut String)) {
        if !Self::is_level_enabled_tag(tag, level) {
            return;
        }
        let mut s = String::new();
        build(&mut s);
        Self::write(tag, level, &s);
    }

    /// Recent retained messages rendered for diagnostics.
    pub fn recent_messages() -> Vec<String> {
        let state = LOG_STATE.lock().unwrap();
        state
            .messages
            .iter()
            .map(|m| format!("[{}][{}] {}", m.level.name(), m.tagset.label(), m.text))
            .collect()
    }

    pub fn recent_messages_raw() -> Vec<LogMessage> {
        let state = LOG_STATE.lock().unwrap();
        state.messages.iter().cloned().collect()
    }

    pub fn clear() {
        LOG_STATE.lock().unwrap().messages.clear();
    }

    /// Flush pending async records (arguments.rs calls this after
    /// reconfiguration; also useful before shutdown).
    pub fn reset_outputs() {
        logAsyncWriter::flush();
        Self::clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::logging::logConfiguration::LogConfiguration;

    #[test]
    fn write_levels_and_tags() {
        let cfg = LogConfiguration::global();
        cfg.reset();
        cfg.parse_xlog("gc=debug").unwrap();
        Log::clear();

        Log::debug(LogTag::Gc, "gc debug msg");
        Log::info(LogTag::Gc, "gc info msg");
        Log::trace(LogTag::Gc, "gc trace msg");
        Log::info(LogTag::Os, "os info msg");
        Log::warning(LogTag::Os, "os warning msg");

        let msgs = Log::recent_messages_raw();
        let texts: Vec<&str> = msgs.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"gc debug msg"));
        assert!(texts.contains(&"gc info msg"));
        assert!(!texts.contains(&"gc trace msg"));
        assert!(!texts.contains(&"os info msg"));
        assert!(texts.contains(&"os warning msg"));

        let gc_msg = msgs.iter().find(|m| m.text == "gc info msg").unwrap();
        assert_eq!(gc_msg.tagset.label(), "gc");
        assert_eq!(gc_msg.level, LogLevel::Info);
    }

    #[test]
    fn multi_tag_exact_matching() {
        let cfg = LogConfiguration::global();
        cfg.reset();
        cfg.parse_xlog("gc+heap=debug").unwrap();
        Log::clear();

        Log::debug_tags(&[LogTag::Gc, LogTag::Heap], "gc+heap msg");
        Log::debug_tags(&[LogTag::Gc], "gc only msg");
        Log::debug_tags(&[LogTag::Gc, LogTag::Heap, LogTag::Ref], "gc+heap+ref msg");

        let texts: Vec<String> = Log::recent_messages()
            .into_iter()
            .map(|s| s.clone())
            .collect();
        assert!(texts.iter().any(|t| t.contains("gc+heap msg")));
        assert!(!texts.iter().any(|t| t.contains("gc only msg")));
        assert!(!texts.iter().any(|t| t.contains("gc+heap+ref msg")));
    }

    #[test]
    fn is_level_enabled_checks() {
        let cfg = LogConfiguration::global();
        cfg.reset();
        cfg.parse_xlog("gc=info,os=off").unwrap();
        assert!(Log::is_level_enabled_tag(LogTag::Gc, LogLevel::Warning));
        assert!(!Log::is_level_enabled_tag(LogTag::Gc, LogLevel::Debug));
        assert!(!Log::is_level_enabled_tag(LogTag::Os, LogLevel::Error));
        assert!(Log::is_level_enabled_tag(LogTag::Init, LogLevel::Warning));
    }
}
