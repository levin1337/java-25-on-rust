/*
 * Port of OpenJDK 25 HotSpot LogConfiguration (share/logging/logConfiguration.hpp)
 *
 * LogConfiguration configures log output destinations, tag selectors, and log levels (-Xlog).
 */

use super::logLevel::LogLevel;
use super::logTag::LogTag;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub struct LogConfiguration {
    tag_levels: Mutex<HashMap<LogTag, LogLevel>>,
    default_level: LogLevel,
}

static GLOBAL_LOG_CONFIG: OnceLock<LogConfiguration> = OnceLock::new();

impl LogConfiguration {
    pub fn new() -> Self {
        Self {
            tag_levels: Mutex::new(HashMap::new()),
            default_level: LogLevel::Info,
        }
    }

    pub fn global() -> &'static LogConfiguration {
        GLOBAL_LOG_CONFIG.get_or_init(Self::new)
    }

    pub fn parse_log_argument(&self, arg: &str) -> Result<(), String> {
        // Formats: "gc=info", "class+load=debug", "all=warning", "gc" (defaults to info)
        for part in arg.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            let (tag_str, level_str) = if let Some(eq_pos) = part.find('=') {
                (&part[..eq_pos], &part[eq_pos + 1..])
            } else {
                (part, "info")
            };

            let level = LogLevel::from_str(level_str)
                .ok_or_else(|| format!("Invalid log level '{}'", level_str))?;

            for t_str in tag_str.split('+') {
                let tag = LogTag::from_str(t_str)
                    .ok_or_else(|| format!("Invalid log tag '{}'", t_str))?;
                self.set_level(tag, level);
            }
        }
        Ok(())
    }

    pub fn set_level(&self, tag: LogTag, level: LogLevel) {
        let mut guard = self.tag_levels.lock().unwrap();
        guard.insert(tag, level);
    }

    pub fn is_level(&self, tag: LogTag, level: LogLevel) -> bool {
        let guard = self.tag_levels.lock().unwrap();
        let target_level = guard
            .get(&tag)
            .or_else(|| guard.get(&LogTag::All))
            .copied()
            .unwrap_or(self.default_level);

        level <= target_level && target_level != LogLevel::Off
    }

    pub fn reset(&self) {
        let mut guard = self.tag_levels.lock().unwrap();
        guard.clear();
    }
}

impl Default for LogConfiguration {
    fn default() -> Self {
        Self::new()
    }
}
