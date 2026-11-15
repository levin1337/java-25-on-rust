/*
 * OpenJDK 25 HotSpot jfr/utilities/jfrTime.hpp + jfrTime.cpp port
 */

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Instant;

// JfrTicks — fast counter. On HotSpot this is an unordered elapsed counter
// (TSC when available). The Rust port uses a monotonic Instant-based
// nanosecond counter; frequency() reports NANOS_PER_SEC so that
// counter_to_nanos is the identity, matching the common os::elapsed_counter
// configuration where the counter is already nanosecond based.
static FT_ENABLED: AtomicBool = AtomicBool::new(false);
static TICKS_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn jfr_ticks_epoch() -> &'static Instant {
    TICKS_EPOCH.get_or_init(Instant::now)
}

// JfrTicks counter value (i64).
pub fn ticks_now() -> i64 {
    jfr_ticks_epoch().elapsed().as_nanos() as i64
}

pub struct JfrTicks(pub i64);

impl JfrTicks {
    pub fn now() -> JfrTicks {
        JfrTicks(ticks_now())
    }
    pub fn value(self) -> i64 {
        self.0
    }
}

pub struct JfrTickspan(pub i64);

impl JfrTickspan {
    pub fn now() -> JfrTickspan {
        JfrTickspan(ticks_now())
    }
    pub fn value(self) -> i64 {
        self.0
    }
}

pub struct JfrTime;

impl JfrTime {
    pub fn initialize() -> bool {
        // rdtsc equivalent is not used; initialize once.
        FT_ENABLED.store(true, Ordering::Relaxed);
        true
    }

    pub fn is_ft_enabled() -> bool {
        FT_ENABLED.load(Ordering::Relaxed)
    }

    pub fn is_ft_supported() -> bool {
        false
    }

    // counter frequency in ticks per second
    pub fn frequency() -> i64 {
        NANOS_PER_SEC as i64
    }
}

pub const NANOS_PER_SEC: f64 = 1_000_000_000.0;
pub const NANOS_PER_MILLISEC: f64 = 1_000_000.0;
pub const NANOS_PER_MICROSEC: f64 = 1_000.0;

static LAST_NANOS: AtomicI64 = AtomicI64::new(0);

// JfrChunk::nanos_now — same clock source as Instant.now(), strictly monotone.
pub fn nanos_now() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    loop {
        let last = LAST_NANOS.load(Ordering::Relaxed);
        if now <= last {
            return last;
        }
        if LAST_NANOS
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return now;
        }
    }
}
