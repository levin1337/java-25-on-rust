/*
 * Port of OpenJDK 25 HotSpot os (share/runtime/os.hpp / os.cpp)
 *
 * Operating system abstraction layer providing system clock, thread control,
 * memory information, and processor metrics. On Windows the real work is
 * delegated to share::platform::windows::OsWindows (os_windows.cpp port);
 * non-Windows builds keep the generic fallback implementations.
 */

#[cfg(not(windows))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(windows))]
use std::thread;
#[cfg(not(windows))]
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use crate::share::platform::windows::OsWindows;

pub struct Os;

static VM_START_INSTANT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static VM_START_WALL_MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
#[cfg(not(windows))]
static OS_THREAD_COUNTER: AtomicU64 = AtomicU64::new(1000);

impl Os {
    pub fn init() {
        #[cfg(windows)]
        {
            // os::win32::initialize_system_info + performance counter.
            let _ = OsWindows::init();
        }
        let _ = VM_START_INSTANT.get_or_init(Instant::now);
        let _ = VM_START_WALL_MS.get_or_init(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
    }

    /// Wall-clock time (epoch millis) when the VM started.
    pub fn vm_startup_time_ms() -> i64 {
        let start = VM_START_WALL_MS.get_or_init(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
        *start as i64
    }

    /// Milliseconds elapsed since VM start.
    pub fn vm_uptime_ms() -> i64 {
        VM_START_INSTANT.get_or_init(Instant::now).elapsed().as_millis() as i64
    }

    pub fn vm_start_time_ms() -> i64 {
        Self::java_time_millis()
    }

    /// os::javaTimeMillis — real FILETIME-derived epoch ms on Windows.
    pub fn java_time_millis() -> i64 {
        #[cfg(windows)]
        {
            OsWindows::java_time_millis()
        }
        #[cfg(not(windows))]
        {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        }
    }

    /// os::javaTimeNanos — QPC-derived on Windows.
    pub fn java_time_nanos() -> i64 {
        #[cfg(windows)]
        {
            OsWindows::java_time_nanos()
        }
        #[cfg(not(windows))]
        {
            let start = VM_START_INSTANT.get_or_init(Instant::now);
            start.elapsed().as_nanos() as i64
        }
    }

    /// os::processor_count.
    pub fn processor_count() -> usize {
        #[cfg(windows)]
        {
            OsWindows::processor_count() as usize
        }
        #[cfg(not(windows))]
        {
            thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        }
    }

    /// os::active_processor_count (processors usable by this process).
    pub fn active_processor_count() -> usize {
        #[cfg(windows)]
        {
            OsWindows::active_processor_count() as usize
        }
        #[cfg(not(windows))]
        {
            Self::processor_count()
        }
    }

    /// os::physical_memory (total physical RAM).
    pub fn physical_memory() -> u64 {
        #[cfg(windows)]
        {
            OsWindows::physical_memory()
        }
        #[cfg(not(windows))]
        {
            8 * 1024 * 1024 * 1024
        }
    }

    /// os::available_physical_memory.
    pub fn free_physical_memory() -> u64 {
        #[cfg(windows)]
        {
            OsWindows::available_physical_memory()
        }
        #[cfg(not(windows))]
        {
            4 * 1024 * 1024 * 1024
        }
    }

    /// os::vm_page_size.
    pub fn page_size() -> usize {
        #[cfg(windows)]
        {
            OsWindows::vm_page_size()
        }
        #[cfg(not(windows))]
        {
            4096
        }
    }

    /// os::vm_allocation_granularity (64K on Windows).
    pub fn allocation_granularity() -> usize {
        #[cfg(windows)]
        {
            OsWindows::vm_allocation_granularity()
        }
        #[cfg(not(windows))]
        {
            Self::page_size()
        }
    }

    /// os::current_thread_id.
    pub fn current_thread_id() -> u64 {
        #[cfg(windows)]
        {
            OsWindows::current_thread_id() as u64
        }
        #[cfg(not(windows))]
        {
            OS_THREAD_COUNTER.fetch_add(1, Ordering::Relaxed)
        }
    }

    /// os::elapsedTime (seconds since VM start).
    pub fn elapsed_time() -> f64 {
        #[cfg(windows)]
        {
            OsWindows::elapsed_time()
        }
        #[cfg(not(windows))]
        {
            VM_START_INSTANT.get_or_init(Instant::now).elapsed().as_secs_f64()
        }
    }

    /// os::elapsedVTime (process CPU time).
    pub fn elapsed_vtime() -> f64 {
        #[cfg(windows)]
        {
            OsWindows::elapsed_vtime()
        }
        #[cfg(not(windows))]
        {
            Self::elapsed_time()
        }
    }

    /// os::javaTimeSystemUTC.
    pub fn java_time_system_utc() -> (i64, i64) {
        #[cfg(windows)]
        {
            OsWindows::java_time_system_utc()
        }
        #[cfg(not(windows))]
        {
            let ms = Self::java_time_millis();
            (ms / 1000, (ms % 1000) * 1_000_000)
        }
    }

    pub fn sleep(millis: u64) {
        #[cfg(windows)]
        {
            OsWindows::sleep_millis(millis.min(u32::MAX as u64) as u32);
        }
        #[cfg(not(windows))]
        {
            thread::sleep(Duration::from_millis(millis));
        }
    }

    pub fn yield_execution() {
        #[cfg(windows)]
        {
            OsWindows::yield_cpu();
        }
        #[cfg(not(windows))]
        {
            thread::yield_now();
        }
    }

    /// os::set_native_priority (Java priority → OS priority).
    pub fn set_native_priority(priority: i32) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            OsWindows::set_native_priority(priority)
        }
        #[cfg(not(windows))]
        {
            let _ = priority;
            Ok(())
        }
    }

    /// os::os_name / os_arch.
    pub fn os_name() -> &'static str {
        std::env::consts::OS
    }

    pub fn os_arch() -> &'static str {
        std::env::consts::ARCH
    }
}
