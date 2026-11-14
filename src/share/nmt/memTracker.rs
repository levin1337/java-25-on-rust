/*
 * Port of OpenJDK 25 HotSpot MemTracker (share/nmt/memTracker.hpp)
 *
 * Implements Native Memory Tracking (NMT) for tracking internal JVM heap,
 * thread stack, GC, code cache, and metadata allocations.
 */

use super::memTag::MemoryType;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmtLevel {
    Off,
    Summary,
    Detail,
}

pub struct MemTracker {
    level: NmtLevel,
    allocations: Mutex<HashMap<MemoryType, usize>>,
}

static GLOBAL_MEM_TRACKER: OnceLock<MemTracker> = OnceLock::new();

impl MemTracker {
    pub fn new(level: NmtLevel) -> Self {
        Self {
            level,
            allocations: Mutex::new(HashMap::new()),
        }
    }

    pub fn global() -> &'static MemTracker {
        GLOBAL_MEM_TRACKER.get_or_init(|| Self::new(NmtLevel::Summary))
    }

    #[inline]
    pub fn level(&self) -> NmtLevel {
        self.level
    }

    pub fn record_malloc(&self, size: usize, mem_type: MemoryType) {
        if self.level == NmtLevel::Off {
            return;
        }
        let mut guard = self.allocations.lock().unwrap();
        *guard.entry(mem_type).or_insert(0) += size;
    }

    pub fn record_free(&self, size: usize, mem_type: MemoryType) {
        if self.level == NmtLevel::Off {
            return;
        }
        let mut guard = self.allocations.lock().unwrap();
        if let Some(entry) = guard.get_mut(&mem_type) {
            *entry = entry.saturating_sub(size);
        }
    }

    pub fn total_allocated(&self) -> usize {
        let guard = self.allocations.lock().unwrap();
        guard.values().sum()
    }

    pub fn allocated_for_type(&self, mem_type: MemoryType) -> usize {
        let guard = self.allocations.lock().unwrap();
        guard.get(&mem_type).copied().unwrap_or(0)
    }

    pub fn summary_report(&self) -> String {
        let mut out = String::from("Native Memory Tracking:\n\nTotal: ");
        let total = self.total_allocated();
        out.push_str(&format!("{} KB\n", total / 1024));

        let guard = self.allocations.lock().unwrap();
        for (tag, bytes) in guard.iter() {
            out.push_str(&format!("- {:<14} : {} KB\n", tag.name(), bytes / 1024));
        }
        out
    }

    pub fn reset(&self) {
        let mut guard = self.allocations.lock().unwrap();
        guard.clear();
    }
}

impl Default for MemTracker {
    fn default() -> Self {
        Self::new(NmtLevel::Summary)
    }
}
