/*
 * Port of OpenJDK 25 HotSpot Log (share/logging/log.hpp)
 *
 * Implements Unified JVM Logging dispatch according to active LogConfiguration:
 * each record is level/tag filtered, decorated, and written to every
 * configured LogOutput (default: stderr). A bounded in-memory ring keeps
 * recent messages for diagnostics (VM.log dcmd equivalent).
 */

use super::logConfiguration::LogConfiguration;
use super::logLevel::LogLevel;
use super::logOutput::LogOutput;
use super::logStream::LogStream;
use super::logTag::LogTag;
use std::collections::VecDeque;
use std::sync::Mutex;

const MAX_RETAINED_MESSAGES: usize = 4096;

pub struct LogMessage {
    pub tag: LogTag,
    pub level: LogLevel,
    pub text: String,
}

struct LogState {
    messages: VecDeque<LogMessage>,
    /// Lazily materialized outputs (File outputs opened once).
    outputs: Option<Vec<Box<dyn LogOutput>>>,
}

pub struct Log {
    state: Mutex<LogState>,
}

static GLOBAL_LOG: Log = Log {
    state: Mutex::new(LogState {
        messages: VecDeque::new(),
        outputs: None,
    }),
};

impl Log {
    /// Core write path: filter → decorate → emit to outputs → retain.
    pub fn write(tag: LogTag, level: LogLevel, msg: &str) {
        if !LogConfiguration::global().is_level(tag, level) {
            return;
        }

        let mut state = GLOBAL_LOG.state.lock().unwrap();

        // Retain for diagnostics.
        if state.messages.len() >= MAX_RETAINED_MESSAGES {
            state.messages.pop_front();
        }
        state.messages.push_back(LogMessage {
            tag,
            level,
            text: msg.to_string(),
        });

        // Materialize outputs once.
        if state.outputs.is_none() {
            state.outputs = Some(LogConfiguration::global().outputs());
        }

        for (i, out) in state.outputs.as_ref().unwrap().iter().enumerate() {
            let deco = LogConfiguration::global().decorations_for(i);
            LogStream::emit(tag, level, msg, out.as_ref(), &deco);
        }
    }

    pub fn info(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Info, msg);
    }

    pub fn debug(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Debug, msg);
    }

    pub fn warning(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Warning, msg);
    }

    pub fn error(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Error, msg);
    }

    pub fn trace(tag: LogTag, msg: &str) {
        Self::write(tag, LogLevel::Trace, msg);
    }

    /// logStream-style structured write used by subsystems that build
    /// multi-part messages.
    pub fn write_stream(tag: LogTag, level: LogLevel, build: impl FnOnce(&mut LogStream)) {
        if !LogConfiguration::global().is_level(tag, level) {
            return;
        }
        let mut stream = LogStream::new(tag, level);
        build(&mut stream);
        let text = stream.into_string();
        Self::write(tag, level, &text);
    }

    pub fn recent_messages() -> Vec<String> {
        let state = GLOBAL_LOG.state.lock().unwrap();
        state
            .messages
            .iter()
            .map(|m| format!("[{:?}][{:?}] {}", m.level, m.tag, m.text))
            .collect()
    }

    pub fn recent_messages_raw() -> Vec<LogMessage> {
        let state = GLOBAL_LOG.state.lock().unwrap();
        state
            .messages
            .iter()
            .map(|m| LogMessage {
                tag: m.tag,
                level: m.level,
                text: m.text.clone(),
            })
            .collect()
    }

    /// Invalidate cached outputs (call after reconfiguring -Xlog).
    pub fn reset_outputs() {
        GLOBAL_LOG.state.lock().unwrap().outputs = None;
    }

    pub fn clear() {
        GLOBAL_LOG.state.lock().unwrap().messages.clear();
    }
}
