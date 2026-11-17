/*
 * Port of OpenJDK 25 HotSpot LogLevel (share/logging/logLevel.hpp/.cpp)
 *
 * Defines Unified JVM Logging log levels. Ordering follows the Rust port
 * convention: Off < Error < Warning < Info < Debug < Trace (increasing
 * verbosity); a configured level L enables all messages with level <= L.
 * LogLevel::NotMentioned is a sentinel used by LogSelectionList::level_for
 * when no selection covers a tag set.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum LogLevel {
    Off = 0,
    Error = 1,
    Warning = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
    NotMentioned = 6,
}

impl LogLevel {
    /// LogLevel::Default — level assigned to tag sets that were never
    /// explicitly configured (warning and above go to stdout).
    pub const DEFAULT: LogLevel = LogLevel::Warning;
    /// LogLevel::Unspecified — level assumed when a selection has no '=' part.
    pub const UNSPECIFIED: LogLevel = LogLevel::Info;

    /// LogLevel::from_string — case-insensitive exact match on level names.
    pub fn from_string(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    /// LogLevel::fuzzy_match — Dice bigram similarity, threshold 0.4.
    pub fn fuzzy_match(s: &str) -> Option<Self> {
        let mut best: Option<Self> = None;
        let mut best_score = 0.4f64;
        for level in [
            Self::Trace,
            Self::Debug,
            Self::Info,
            Self::Warning,
            Self::Error,
        ] {
            let score = string_utils_similarity(s, level.name());
            if score >= best_score {
                best = Some(level);
                best_score = score;
            }
        }
        best
    }

    /// LogLevel::name — printable name for real levels.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
            Self::NotMentioned => "not_mentioned",
        }
    }

    pub fn is_real(self) -> bool {
        self != Self::NotMentioned
    }
}

/// StringUtils::similarity — Dice coefficient over adjacent character pairs.
pub(crate) fn string_utils_similarity(a: &str, b: &str) -> f64 {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut hit = 0usize;
    if a.len() >= 2 {
        'outer: for i in 0..a.len() - 1 {
            if b.len() >= 2 {
                for j in 0..b.len() - 1 {
                    if a[i] == b[j] && a[i + 1] == b[j + 1] {
                        hit += 1;
                        continue 'outer;
                    }
                }
            }
        }
    }
    2.0 * hit as f64 / (a.len() + b.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_names_roundtrip() {
        for l in [
            LogLevel::Off,
            LogLevel::Error,
            LogLevel::Warning,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            assert_eq!(LogLevel::from_string(l.name()), Some(l));
            assert_eq!(LogLevel::from_string(&l.name().to_uppercase()), Some(l));
        }
        assert_eq!(LogLevel::from_string("warn"), None);
        assert_eq!(LogLevel::from_string("verbose"), None);
    }

    #[test]
    fn level_ordering() {
        assert!(LogLevel::Error < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Trace);
        assert!(LogLevel::Off < LogLevel::Error);
    }

    #[test]
    fn fuzzy_suggests_level() {
        assert_eq!(LogLevel::fuzzy_match("warng"), Some(LogLevel::Warning));
        assert_eq!(LogLevel::fuzzy_match("debu"), Some(LogLevel::Debug));
    }
}
