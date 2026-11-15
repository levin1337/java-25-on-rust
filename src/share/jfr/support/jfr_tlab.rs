/*
 * OpenJDK 25 HotSpot jfr/support/jfrTlab.hpp + .cpp port
 *
 * TLAB (thread-local allocation buffer) fast-path support for JFR event
 * allocation. In HotSpot this hooks into the TLAB slow-path; in Rust
 * it provides the allocation-size accounting helpers.
 */

use std::sync::atomic::{AtomicU64, Ordering};

static EVENT_ALLOCATION_SLOW_PATHS: AtomicU64 = AtomicU64::new(0);
static EVENT_ALLOCATION_FAST_PATHS: AtomicU64 = AtomicU64::new(0);

pub struct JfrTlab;

impl JfrTlab {
    pub fn on_fast_path_alloc() {
        EVENT_ALLOCATION_FAST_PATHS.fetch_add(1, Ordering::Relaxed);
    }

    pub fn on_slow_path_alloc() {
        EVENT_ALLOCATION_SLOW_PATHS.fetch_add(1, Ordering::Relaxed);
    }

    pub fn fast_path_count() -> u64 {
        EVENT_ALLOCATION_FAST_PATHS.load(Ordering::Relaxed)
    }

    pub fn slow_path_count() -> u64 {
        EVENT_ALLOCATION_SLOW_PATHS.load(Ordering::Relaxed)
    }
}
