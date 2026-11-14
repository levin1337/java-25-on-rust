/*
 * Port of OpenJDK 25 HotSpot os (share/runtime/os.hpp / os.cpp)
 *
 * Operating system abstraction layer providing system clock, thread control,
 * memory information, and processor metrics.
 */

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Os;

static VM_START_INSTANT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static OS_THREAD_COUNTER: AtomicU64 = AtomicU64::new(1000);

impl Os {
    pub fn init() {
        let _ = VM_START_INSTANT.get_or_init(Instant::now);
    }

    pub fn vm_start_time_ms() -> i64 {
        Self::java_time_millis()
    }

    pub fn java_time_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    pub fn java_time_nanos() -> i64 {
        let start = VM_START_INSTANT.get_or_init(Instant::now);
        start.elapsed().as_nanos() as i64
    }

    pub fn processor_count() -> usize {
        thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }

    pub fn physical_memory() -> u64 {
        // Default physical memory estimate: 8GB
        8 * 1024 * 1024 * 1024
    }

    pub fn free_physical_memory() -> u64 {
        // Default free physical memory estimate: 4GB
        4 * 1024 * 1024 * 1024
    }

    pub fn page_size() -> usize {
        4096
    }

    pub fn current_thread_id() -> u64 {
        // Fallback or unique ID per OS thread
        OS_THREAD_COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    pub fn sleep(millis: u64) {
        thread::sleep(Duration::from_millis(millis));
    }

    pub fn yield_execution() {
        thread::yield_now();
    }

    pub fn os_name() -> &'static str {
        std::env::consts::OS
    }

    pub fn os_arch() -> &'static str {
        std::env::consts::ARCH
    }
}
