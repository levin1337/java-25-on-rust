/*
 * Port of OpenJDK 25 HotSpot Management (share/services/management.hpp)
 *
 * Provides JVM management beans (MemoryMXBean, RuntimeMXBean,
 * ThreadMXBean, ClassLoadingMXBean) mirroring java.lang.management.
 */

use crate::share::runtime::os::Os;
use crate::share::runtime::threads::Threads;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsage {
    pub init: u64,
    pub used: u64,
    pub committed: u64,
    pub max: u64,
}

impl MemoryUsage {
    pub fn new(init: u64, used: u64, committed: u64, max: u64) -> Self {
        Self {
            init,
            used,
            committed,
            max,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryPool {
    pub name: String,
    pub is_heap: bool,
    pub usage: MemoryUsage,
    pub peak_usage: MemoryUsage,
}

pub struct ClassLoadingMXBean {
    loaded_class_count: AtomicU64,
    total_loaded_class_count: AtomicU64,
    unloaded_class_count: AtomicU64,
}

impl ClassLoadingMXBean {
    pub fn new() -> Self {
        Self {
            loaded_class_count: AtomicU64::new(0),
            total_loaded_class_count: AtomicU64::new(0),
            unloaded_class_count: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn record_class_load(&self) {
        self.loaded_class_count.fetch_add(1, Ordering::Relaxed);
        self.total_loaded_class_count
            .fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_class_unload(&self) {
        self.loaded_class_count.fetch_sub(1, Ordering::Relaxed);
        self.unloaded_class_count.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn loaded_class_count(&self) -> u64 {
        self.loaded_class_count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn total_loaded_class_count(&self) -> u64 {
        self.total_loaded_class_count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn unloaded_class_count(&self) -> u64 {
        self.unloaded_class_count.load(Ordering::Relaxed)
    }
}

impl Default for ClassLoadingMXBean {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RuntimeMXBean {
    name: String,
    vm_name: String,
    vm_vendor: String,
    vm_version: String,
    start_time_ms: i64,
    input_arguments: Vec<String>,
    system_properties: HashMap<String, String>,
}

impl RuntimeMXBean {
    pub fn new() -> Self {
        let mut system_properties = HashMap::new();
        system_properties.insert("java.version".to_string(), "25-ea".to_string());
        system_properties.insert(
            "java.vm.name".to_string(),
            "OpenJDK 64-Bit HotSpot(TM) VM (Rust)".to_string(),
        );
        system_properties.insert(
            "java.vm.vendor".to_string(),
            "Oracle Corporation".to_string(),
        );
        system_properties.insert("os.name".to_string(), Os::os_name().to_string());
        system_properties.insert("os.arch".to_string(), Os::os_arch().to_string());

        Self {
            name: format!("{}@localhost", std::process::id()),
            vm_name: "OpenJDK 64-Bit HotSpot(TM) VM (Rust)".to_string(),
            vm_vendor: "Oracle Corporation".to_string(),
            vm_version: "25-ea".to_string(),
            start_time_ms: Os::java_time_millis(),
            input_arguments: std::env::args().collect(),
            system_properties,
        }
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn vm_name(&self) -> &str {
        &self.vm_name
    }

    #[inline]
    pub fn vm_vendor(&self) -> &str {
        &self.vm_vendor
    }

    #[inline]
    pub fn vm_version(&self) -> &str {
        &self.vm_version
    }

    #[inline]
    pub fn start_time_ms(&self) -> i64 {
        self.start_time_ms
    }

    #[inline]
    pub fn uptime_ms(&self) -> i64 {
        Os::java_time_millis().saturating_sub(self.start_time_ms)
    }

    #[inline]
    pub fn input_arguments(&self) -> &[String] {
        &self.input_arguments
    }

    #[inline]
    pub fn system_properties(&self) -> &HashMap<String, String> {
        &self.system_properties
    }
}

impl Default for RuntimeMXBean {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Management {
    runtime_mxbean: RuntimeMXBean,
    class_loading_mxbean: ClassLoadingMXBean,
    memory_pools: Mutex<Vec<MemoryPool>>,
}

static GLOBAL_MANAGEMENT: OnceLock<Management> = OnceLock::new();

impl Management {
    pub fn new() -> Self {
        let initial_pools = vec![
            MemoryPool {
                name: "Eden Space".to_string(),
                is_heap: true,
                usage: MemoryUsage::new(64 * 1024 * 1024, 0, 64 * 1024 * 1024, 256 * 1024 * 1024),
                peak_usage: MemoryUsage::new(
                    64 * 1024 * 1024,
                    0,
                    64 * 1024 * 1024,
                    256 * 1024 * 1024,
                ),
            },
            MemoryPool {
                name: "Survivor Space".to_string(),
                is_heap: true,
                usage: MemoryUsage::new(8 * 1024 * 1024, 0, 8 * 1024 * 1024, 32 * 1024 * 1024),
                peak_usage: MemoryUsage::new(8 * 1024 * 1024, 0, 8 * 1024 * 1024, 32 * 1024 * 1024),
            },
            MemoryPool {
                name: "Tenured Gen".to_string(),
                is_heap: true,
                usage: MemoryUsage::new(128 * 1024 * 1024, 0, 128 * 1024 * 1024, 512 * 1024 * 1024),
                peak_usage: MemoryUsage::new(
                    128 * 1024 * 1024,
                    0,
                    128 * 1024 * 1024,
                    512 * 1024 * 1024,
                ),
            },
            MemoryPool {
                name: "Metaspace".to_string(),
                is_heap: false,
                usage: MemoryUsage::new(
                    16 * 1024 * 1024,
                    4 * 1024 * 1024,
                    16 * 1024 * 1024,
                    1024 * 1024 * 1024,
                ),
                peak_usage: MemoryUsage::new(
                    16 * 1024 * 1024,
                    4 * 1024 * 1024,
                    16 * 1024 * 1024,
                    1024 * 1024 * 1024,
                ),
            },
        ];

        Self {
            runtime_mxbean: RuntimeMXBean::new(),
            class_loading_mxbean: ClassLoadingMXBean::new(),
            memory_pools: Mutex::new(initial_pools),
        }
    }

    pub fn global() -> &'static Management {
        GLOBAL_MANAGEMENT.get_or_init(Self::new)
    }

    #[inline]
    pub fn runtime_mxbean(&self) -> &RuntimeMXBean {
        &self.runtime_mxbean
    }

    #[inline]
    pub fn class_loading_mxbean(&self) -> &ClassLoadingMXBean {
        &self.class_loading_mxbean
    }

    pub fn total_heap_memory_usage(&self) -> MemoryUsage {
        let pools = self.memory_pools.lock().unwrap();
        let mut init = 0;
        let mut used = 0;
        let mut committed = 0;
        let mut max = 0;
        for p in pools.iter().filter(|p| p.is_heap) {
            init += p.usage.init;
            used += p.usage.used;
            committed += p.usage.committed;
            max += p.usage.max;
        }
        MemoryUsage::new(init, used, committed, max)
    }

    pub fn live_threads_count(&self) -> usize {
        Threads::global().number_of_threads()
    }

    pub fn daemon_threads_count(&self) -> usize {
        Threads::global().number_of_daemon_threads()
    }
}

impl Default for Management {
    fn default() -> Self {
        Self::new()
    }
}
