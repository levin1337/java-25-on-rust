/*
 * Port of OpenJDK 25 HotSpot platform layer (src/hotspot/os/windows)
 *
 * Platform abstraction: operating-system specific services are selected at
 * compile time. On Windows the implementation lives in `windows`.
 */

#[cfg(windows)]
pub mod windows;

#[cfg(windows)]
pub use windows::OsWindows as OsPlatform;

/// Operating-system neutral helpers shared by all platform backends.
pub mod neutral {
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Windows FILETIME epoch offset: 100-ns intervals between 1601-01-01
    /// and 1970-01-01 (mirrors os::offset() in os_windows.cpp).
    pub const FILETIME_UNIX_EPOCH_OFFSET_100NS: i64 = 116_444_736_000_000_000;

    /// Convert a Windows FILETIME (100-ns ticks since 1601) to Unix epoch ms.
    #[inline]
    pub fn filetime_to_unix_ms(low: u32, high: u32) -> i64 {
        let ticks = (((high as i64) << 32) | low as i64) - FILETIME_UNIX_EPOCH_OFFSET_100NS;
        ticks / 10_000
    }

    /// Unix epoch ms → Windows FILETIME ticks.
    #[inline]
    pub fn unix_ms_to_filetime(ms: i64) -> i64 {
        ms.saturating_mul(10_000) + FILETIME_UNIX_EPOCH_OFFSET_100NS
    }

    pub fn unix_time_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}
