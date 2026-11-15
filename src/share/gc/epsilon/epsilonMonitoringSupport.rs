/*
 * Port of OpenJDK 25 HotSpot Epsilon monitoring support
 * (share/gc/epsilon/epsilonMonitoringSupport.hpp/.cpp,
 *  epsilonMemoryPool.hpp, share/services/memoryUsage.hpp)
 *
 * Heap/space occupancy counters and memory-pool usage reporting.
 * Epsilon never collects, so counters only grow with allocation.
 */

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Default)]
pub struct EpsilonGenerationCounters {
    pub heap_used: u64,
    pub heap_committed: u64,
    pub heap_max_capacity: u64,
}

#[derive(Clone, Debug, Default)]
pub struct EpsilonSpaceCounters {
    pub used: u64,
    pub committed: u64,
    pub max_capacity: u64,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryUsage {
    pub init: u64,
    pub used: u64,
    pub committed: u64,
    pub max: u64,
}

pub struct EpsilonMonitoringSupport {
    heap_counters: EpsilonGenerationCounters,
    space_counters: EpsilonSpaceCounters,
    gc_count: AtomicU64,
}

impl EpsilonMonitoringSupport {
    pub fn new() -> Self {
        Self {
            heap_counters: EpsilonGenerationCounters::default(),
            space_counters: EpsilonSpaceCounters::default(),
            gc_count: AtomicU64::new(0),
        }
    }

    pub fn update_counters(&mut self, used: usize, committed: usize, max_capacity: usize) {
        self.heap_counters.heap_used = used as u64;
        self.heap_counters.heap_committed = committed as u64;
        self.heap_counters.heap_max_capacity = max_capacity as u64;
        self.space_counters.used = used as u64;
        self.space_counters.committed = committed as u64;
        self.space_counters.max_capacity = max_capacity as u64;
    }

    pub fn note_gc(&self) {
        self.gc_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn gc_count(&self) -> u64 {
        self.gc_count.load(Ordering::Relaxed)
    }

    pub fn heap_counters(&self) -> &EpsilonGenerationCounters {
        &self.heap_counters
    }

    pub fn space_counters(&self) -> &EpsilonSpaceCounters {
        &self.space_counters
    }

    pub fn get_memory_usage(&self, init: usize) -> MemoryUsage {
        MemoryUsage {
            init: init as u64,
            used: self.space_counters.used,
            committed: self.space_counters.committed,
            max: self.space_counters.max_capacity,
        }
    }
}

impl Default for EpsilonMonitoringSupport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_track_usage() {
        let mut mon = EpsilonMonitoringSupport::new();
        mon.update_counters(100, 200, 1024);
        assert_eq!(mon.heap_counters().heap_used, 100);
        assert_eq!(mon.space_counters().committed, 200);
        let usage = mon.get_memory_usage(50);
        assert_eq!(usage.used, 100);
        assert_eq!(usage.committed, 200);
        assert_eq!(usage.max, 1024);
        mon.note_gc();
        assert_eq!(mon.gc_count(), 1);
    }
}
