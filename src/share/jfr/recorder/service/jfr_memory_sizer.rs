/*
 * OpenJDK 25 HotSpot jfr/recorder/service/jfrMemorySizer.hpp + .cpp port
 *
 * Estimates the amount of memory a recording will need based on the
 * number of live threads / loaded classes. Simplified but functional.
 */

use std::sync::atomic::{AtomicU64, Ordering};

static MEMORY_SIZE: AtomicU64 = AtomicU64::new(0);

pub struct JfrMemorySizer;

impl JfrMemorySizer {
    // Approximate per-thread buffer cost (bytes) used when recording.
    pub const ESTIMATED_THREAD_LOCAL_BUFFER_SIZE: u64 = 8 * 1024;

    pub fn on_start_recording() {
        // Conservative estimate: threads * per-thread buffer + global pool.
        let threads = crate::share::jfr::utilities::jfr_types::thread_count_hint();
        let est = threads as u64 * Self::ESTIMATED_THREAD_LOCAL_BUFFER_SIZE + 16 * 1024 * 1024;
        MEMORY_SIZE.store(est, Ordering::Relaxed);
    }

    pub fn memory_size() -> u64 {
        MEMORY_SIZE.load(Ordering::Relaxed)
    }
}
