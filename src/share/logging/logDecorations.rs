/*
 * Port of OpenJDK 25 HotSpot LogDecorators / LogDecorations
 * (share/logging/logDecorators.hpp/.cpp, logDecorations.hpp/.cpp)
 *
 * LogDecorators is the bitmask of decorations selected via -Xlog
 * (time, utctime, uptime, timemillis, uptimemillis, timenanos,
 * uptimenanos, hostname, pid, tid, level, tags). LogDecorations holds the
 * values resolved at the log site; rendering happens per output with
 * per-output column padding, mirroring write_decorations() in
 * logFileStreamOutput.cpp.
 */

use super::logLevel::LogLevel;
use super::logTagSet::LogTagSet;
use crate::share::runtime::os::Os;
use std::sync::OnceLock;

/// DECORATOR_LIST order — decorations are always emitted in this order.
pub const DECORATOR_NAMES: &[(&str, &str)] = &[
    ("time", "t"),
    ("utctime", "utc"),
    ("uptime", "u"),
    ("timemillis", "tm"),
    ("uptimemillis", "um"),
    ("timenanos", "tn"),
    ("uptimenanos", "un"),
    ("hostname", "hn"),
    ("pid", "p"),
    ("tid", "ti"),
    ("level", "l"),
    ("tags", "tg"),
];

pub const N_DECORATORS: usize = DECORATOR_NAMES.len();

pub const TIME: u32 = 1 << 0;
pub const UTCTIME: u32 = 1 << 1;
pub const UPTIME: u32 = 1 << 2;
pub const TIMEMILLIS: u32 = 1 << 3;
pub const UPTIMEMILLIS: u32 = 1 << 4;
pub const TIMENANOS: u32 = 1 << 5;
pub const UPTIMENANOS: u32 = 1 << 6;
pub const HOSTNAME: u32 = 1 << 7;
pub const PID: u32 = 1 << 8;
pub const TID: u32 = 1 << 9;
pub const LEVEL: u32 = 1 << 10;
pub const TAGS: u32 = 1 << 11;

const ALL_MASK: u32 = (1 << N_DECORATORS) - 1;

/// LogDecorators — bitmask of enabled decorations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Decorations(pub u32);

impl Decorations {
    /// LogDecorators::None
    pub fn none() -> Self {
        Self(0)
    }

    /// LogDecorators::All
    pub fn all() -> Self {
        Self(ALL_MASK)
    }

    /// LogDecorators default constructor — uptime,level,tags
    /// (defaultsMask in logDecorators.hpp).
    pub fn default() -> Self {
        Self(UPTIME | LEVEL | TAGS)
    }

    /// LogDecorators::is_empty
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// LogDecorators::is_decorator
    pub fn is_decorator(&self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    /// LogDecorators::combine_with
    pub fn combine_with(&mut self, other: &Decorations) {
        self.0 |= other.0;
    }

    /// LogDecorators::from_string — name or abbreviation, case-insensitive.
    pub fn decorator_from_string(s: &str) -> Option<u32> {
        for (i, (name, abbr)) in DECORATOR_NAMES.iter().enumerate() {
            if s.eq_ignore_ascii_case(name) || s.eq_ignore_ascii_case(abbr) {
                return Some(1u32 << i);
            }
        }
        None
    }

    /// LogDecorators::parse — strict parse; "none" clears all decorators,
    /// empty input keeps the current set (returns Ok without change when
    /// `keep_on_empty` semantics apply — callers pass the default first).
    /// On error returns the offending token description.
    pub fn parse(list: &str) -> Result<Self, String> {
        if list.eq_ignore_ascii_case("none") {
            return Ok(Self::none());
        }
        let mut mask = 0u32;
        for token in list.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match Self::decorator_from_string(token) {
                Some(bit) => mask |= bit,
                None => return Err(format!("Invalid decorator '{}'.", token)),
            }
        }
        Ok(Self(mask))
    }

    /// Iterate enabled decorator bits in DECORATOR_LIST order.
    pub fn enabled(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        (0..N_DECORATORS).filter_map(move |i| {
            let bit = 1u32 << i;
            if self.0 & bit != 0 {
                Some((i, bit))
            } else {
                None
            }
        })
    }

    /// Comma-separated decorator names for describe() output.
    pub fn describe(&self) -> String {
        let names: Vec<&str> = self
            .enabled()
            .map(|(i, _)| DECORATOR_NAMES[i].0)
            .collect();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(",")
        }
    }
}

/// Backwards-compatible field accessors used by older call sites/tests.
impl Decorations {
    pub fn has_time(&self) -> bool {
        self.is_decorator(TIME)
    }
    pub fn has_uptime(&self) -> bool {
        self.is_decorator(UPTIME)
    }
    pub fn has_level(&self) -> bool {
        self.is_decorator(LEVEL)
    }
    pub fn has_tags(&self) -> bool {
        self.is_decorator(TAGS)
    }
    pub fn has_pid(&self) -> bool {
        self.is_decorator(PID)
    }
    pub fn has_tid(&self) -> bool {
        self.is_decorator(TID)
    }
}

/// LogDecorations — values resolved at the log site for one record.
/// Only fields needed by the enabled decorator mask are populated.
#[derive(Debug, Clone)]
pub struct LogDecorations {
    pub millis: i64,
    pub nanos: i64,
    pub elapsed_seconds: f64,
    pub tid: u64,
    pub pid: u32,
    pub level: LogLevel,
    pub tagset: LogTagSet,
    enabled: u32,
}

impl LogDecorations {
    /// LogDecorations constructor — resolves decorator values eagerly.
    pub fn new(level: LogLevel, tagset: &LogTagSet, decorators: &Decorations) -> Self {
        let needs_millis = decorators.is_decorator(TIME)
            || decorators.is_decorator(UTCTIME)
            || decorators.is_decorator(TIMEMILLIS);
        let needs_nanos = decorators.is_decorator(TIMENANOS);
        let needs_uptime = decorators.is_decorator(UPTIME)
            || decorators.is_decorator(UPTIMEMILLIS)
            || decorators.is_decorator(UPTIMENANOS);
        let needs_tid = decorators.is_decorator(TID);
        Self {
            millis: if needs_millis {
                Os::java_time_millis()
            } else {
                0
            },
            nanos: if needs_nanos {
                Os::java_time_nanos()
            } else {
                0
            },
            elapsed_seconds: if needs_uptime {
                Os::elapsed_time()
            } else {
                0.0
            },
            tid: if needs_tid {
                Os::current_thread_id()
            } else {
                0
            },
            pid: if decorators.is_decorator(PID) {
                current_pid()
            } else {
                0
            },
            level,
            tagset: tagset.clone(),
            enabled: decorators.0,
        }
    }

    /// LogDecorations::decoration — rendered value of one decorator
    /// (without brackets), e.g. "0.123s", "info", "gc+heap".
    pub fn decoration(&self, index: usize) -> String {
        match index {
            0 => iso8601_time(self.millis, false),
            1 => iso8601_time(self.millis, true),
            2 => format!("{:.3}s", self.elapsed_seconds),
            3 => format!("{}ms", self.millis),
            4 => format!("{}ms", (self.elapsed_seconds * 1000.0) as i64),
            5 => format!("{}ns", self.nanos),
            6 => format!("{}ns", (self.elapsed_seconds * 1_000_000_000.0) as i64),
            7 => host_name(),
            8 => format!("{}", self.pid),
            9 => format!("{}", self.tid),
            10 => self.level.name().to_string(),
            11 => self.tagset.label_with(","),
            _ => String::new(),
        }
    }

    /// write_decorations equivalent: renders "[v][v] " block applying
    /// per-output column padding (updated in place like _decorator_padding).
    /// Returns the rendered prefix including the trailing separator space.
    pub fn render_prefix(&self, padding: &mut [usize; N_DECORATORS]) -> String {
        let mut out = String::new();
        for (i, _) in Decorations(self.enabled).enabled() {
            let value = self.decoration(i);
            let width = padding[i].max(value.len());
            padding[i] = width;
            out.push('[');
            out.push_str(&value);
            for _ in value.len()..width {
                out.push(' ');
            }
            out.push(']');
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out
    }

    /// Continuation-line prefix for multi-line messages: a bracketed blank
    /// field covering the decoration block, matching "[%*c] " in
    /// logFileStreamOutput.cpp::write_internal.
    pub fn continuation_prefix(deco_block_len: usize) -> String {
        if deco_block_len < 2 {
            return String::new();
        }
        let mut s = String::with_capacity(deco_block_len + 2);
        s.push('[');
        for _ in 0..deco_block_len.saturating_sub(2) {
            s.push(' ');
        }
        s.push_str("] ");
        s
    }
}

fn current_pid() -> u32 {
    #[cfg(windows)]
    {
        crate::share::platform::windows::OsWindows::current_process_id()
    }
    #[cfg(not(windows))]
    {
        std::process::id()
    }
}

fn host_name() -> String {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
        #[cfg(windows)]
        {
            crate::share::platform::windows::OsWindows::get_host_name()
                .unwrap_or_else(|| "unknown-host".to_string())
        }
        #[cfg(not(windows))]
        {
            std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_else(|_| "unknown-host".to_string())
        }
    })
    .clone()
}

/// os::iso8601_time — "YYYY-MM-DDTHH:MM:SS.mmm+zzzz".
/// Local timezone offset requires OS calls outside the logging module's
/// scope; UTC (+0000) is used, matching HotSpot behavior on UTC hosts.
fn iso8601_time(millis: i64, utc: bool) -> String {
    let _ = utc;
    let secs = millis.div_euclid(1000);
    let ms = millis.rem_euclid(1000);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let sod = secs.rem_euclid(86_400);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}+0000",
        y,
        m,
        d,
        sod / 3600,
        sod % 3600 / 60,
        sod % 60,
        ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::logTag::LogTag;

    #[test]
    fn parse_decorations() {
        let d = Decorations::parse("time,uptime,level,tags").unwrap();
        assert!(d.has_time() && d.has_uptime() && d.has_level() && d.has_tags());
        assert!(!d.has_pid() && !d.has_tid());

        let d2 = Decorations::parse("uptime").unwrap();
        assert!(d2.has_uptime() && !d2.has_level());

        assert!(Decorations::parse("none").unwrap().is_empty());
        assert!(Decorations::parse("bogus").is_err());
        assert!(Decorations::parse("t,u,l,tg").unwrap().0 == (TIME | UPTIME | LEVEL | TAGS));
    }

    #[test]
    fn default_mask_is_uptime_level_tags() {
        let d = Decorations::default();
        assert_eq!(d.0, UPTIME | LEVEL | TAGS);
    }

    #[test]
    fn render_prefix_format() {
        let ts = LogTagSet::new(&[LogTag::Gc]);
        let deco = LogDecorations::new(LogLevel::Info, &ts, &Decorations::parse("uptime,level,tags").unwrap());
        let mut padding = [0usize; N_DECORATORS];
        let s = deco.render_prefix(&mut padding);
        assert!(s.contains("s]["));
        assert!(s.contains("[info]"));
        assert!(s.contains("[gc]"));
        assert!(s.ends_with(' '));

        let s2 = deco.render_prefix(&mut padding);
        assert_eq!(s, s2);
    }

    #[test]
    fn decoration_values() {
        let ts = LogTagSet::new(&[LogTag::Gc, LogTag::Heap]);
        let d = Decorations::all();
        let deco = LogDecorations::new(LogLevel::Debug, &ts, &d);
        assert!(deco.decoration(2).ends_with('s'));
        assert!(deco.decoration(3).ends_with("ms"));
        assert_eq!(deco.decoration(10), "debug");
        assert_eq!(deco.decoration(11), "gc,heap");
        assert!(deco.decoration(0).contains('T'));
        assert!(deco.decoration(7).len() > 0);
    }

    #[test]
    fn iso8601_format() {
        // 2024-01-02T03:04:05.678 UTC
        let s = iso8601_time(1_704_164_645_678, true);
        assert_eq!(s, "2024-01-02T03:04:05.678+0000");
    }
}
