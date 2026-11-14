/*
 * Port of OpenJDK 25 HotSpot Log (share/logging/log.hpp)
 *
 * Implements Unified JVM Logging dispatch according to active LogConfiguration.
 */

use super::logConfiguration::LogConfiguration;
use super::logLevel::LogLevel;
use super::logTag::LogTag;
use std::sync::Mutex;

pub struct LogMessage {
    pub tag: LogTag,
    pub level: LogLevel,
    pub text: String,
}

pub struct Log {
    messages: Mutex<Vec<LogMessage>>,
}

static GLOBAL_LOG: Log = Log {
    messages: Mutex::new(Vec::new()),
};

impl Log {
    pub fn write(tag: LogTag, level: LogLevel, msg: &str) {
        if LogConfiguration::global().is_level(tag, level) {
            let log_msg = LogMessage {
                tag,
                level,
                text: msg.to_string(),
            };
            GLOBAL_LOG.messages.lock().unwrap().push(log_msg);
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

    pub fn recent_messages() -> Vec<String> {
        let guard = GLOBAL_LOG.messages.lock().unwrap();
        guard
            .iter()
            .map(|m| format!("[{:?}][{:?}] {}", m.level, m.tag, m.text))
            .collect()
    }

    pub fn clear() {
        GLOBAL_LOG.messages.lock().unwrap().clear();
    }
}
