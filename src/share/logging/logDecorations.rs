/*
 * Port of OpenJDK 25 HotSpot logDecorations / logDecorators
 * (share/logging/logDecorations.hpp/.cpp, logDecorators.cpp)
 *
 * Decorates each log line with the -Xlog decorations selector:
 *   time     — [2024-01-01T00:00:00.000+0000]
 *   uptime   — [0.123s]  (seconds since VM start)
 *   level    — [info]
 *   tags     — [gc,init]
 *   pid      — [pid: 1234]
 *   thread   — [thread: 0x00007f...]
 */

use super::logTag::LogTag;

/// Decoration bit set parsed from "time,uptime,level,tags" style lists.
#[derive(Debug, Clone, Copy, Default)]
pub struct Decorations {
    pub time: bool,
    pub uptime: bool,
    pub level: bool,
    pub tags: bool,
    pub pid: bool,
    pub thread: bool,
}

impl Decorations {
    pub fn none() -> Self {
        Self::default()
    }

    /// Parse a comma-separated decoration list; unknown names are ignored
    /// (HotSpot warns; we silently drop to keep parsing total).
    pub fn parse(list: &str) -> Self {
        let mut d = Self::default();
        for part in list.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "time" => d.time = true,
                "uptime" => d.uptime = true,
                "level" => d.level = true,
                "tags" => d.tags = true,
                "pid" => d.pid = true,
                "thread" => d.thread = true,
                _ => {}
            }
        }
        d
    }

    /// Default decorations used by HotSpot when none are specified:
    /// level,tags,time,uptime for file outputs is "time,uptime,level,tags".
    pub fn default_file() -> Self {
        Self {
            time: true,
            uptime: true,
            level: true,
            tags: true,
            pid: false,
            thread: false,
        }
    }

    pub fn default_stderr() -> Self {
        Self {
            time: false,
            uptime: true,
            level: true,
            tags: true,
            pid: false,
            thread: false,
        }
    }

    /// Render the decoration prefix for one log record.
    /// `uptime_secs` is supplied by the caller (os::elapsedTime).
    pub fn decorate(
        &self,
        level_name: &str,
        tag: LogTag,
        uptime_secs: f64,
        pid: u32,
        thread_id: u64,
    ) -> String {
        let mut out = String::new();
        if self.time {
            out.push_str(&format_time_now());
        }
        if self.uptime {
            out.push_str(&format!("[{:.3}s] ", uptime_secs));
        }
        if self.level {
            out.push_str(&format!("[{}] ", level_name));
        }
        if self.tags {
            out.push_str(&format!("[{}] ", tag.name()));
        }
        if self.pid {
            out.push_str(&format!("[pid: {}] ", pid));
        }
        if self.thread {
            out.push_str(&format!("[thread: 0x{:x}] ", thread_id));
        }
        out
    }
}

fn format_time_now() -> String {
    // ISO-8601 with milliseconds and UTC offset, like logDecorators.cpp.
    let ms = crate::share::runtime::os::Os::java_time_millis();
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    // civil-from-days algorithm (Howard Hinnant).
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
    let secs_of_day = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (secs_of_day / 3600, secs_of_day % 3600 / 60, secs_of_day % 60);
    format!(
        "[{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}+0000] ",
        y, m, d, hh, mm, ss, millis
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decorations() {
        let d = Decorations::parse("time,uptime,level,tags");
        assert!(d.time && d.uptime && d.level && d.tags);
        assert!(!d.pid);
        let d2 = Decorations::parse("uptime");
        assert!(d2.uptime && !d2.level);
    }

    #[test]
    fn decorate_output() {
        let d = Decorations::parse("level,tags,uptime");
        let s = d.decorate("info", LogTag::Gc, 1.5, 42, 7);
        assert!(s.contains("[1.500s]"));
        assert!(s.contains("[info]"));
        assert!(s.contains("[gc]"));
    }
}
