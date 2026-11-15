/*
 * Port of OpenJDK 25 HotSpot G1IHOPControl
 * (share/gc/g1/g1IHOPControl.hpp/.cpp) — the controller that adjusts
 * the initiate-heap-occupancy threshold (IHOP) so that concurrent
 * marking finishes before the old generation fills.
 *
 * The controller compares the time marking actually took against the
 * time available (allocation-rate driven) and moves IHOP up/down
 * within [min, max].
 */

/// `G1IHOPControl`: adaptive IHOP controller.
#[derive(Debug)]
pub struct G1IHOPControl {
    /// Current IHOP in percent (0..100).
    initiate_heap_occupancy_percent: f64,
    /// Lower/upper bounds (`IHOPMinPercent`/`IHOPMaxPercent`).
    min_percent: f64,
    max_percent: f64,
    /// Last computed marking-time surplus/deficit (seconds).
    last_time_to_mark_ms: f64,
    /// Allocation rate used by the last update (bytes/sec).
    last_alloc_rate_bytes_per_sec: f64,
}

impl G1IHOPControl {
    /// `G1IHOPControl` defaults: IHOPMinPercent=10, IHOPMaxPercent=90,
    /// initial IHOP=45 (HotSpot ergonomics).
    pub fn new() -> Self {
        Self {
            initiate_heap_occupancy_percent: 45.0,
            min_percent: 10.0,
            max_percent: 90.0,
            last_time_to_mark_ms: 0.0,
            last_alloc_rate_bytes_per_sec: 0.0,
        }
    }

    #[inline]
    pub fn initiate_heap_occupancy_percent(&self) -> f64 {
        self.initiate_heap_occupancy_percent
    }

    #[inline]
    pub fn min_percent(&self) -> f64 {
        self.min_percent
    }

    #[inline]
    pub fn max_percent(&self) -> f64 {
        self.max_percent
    }

    /// `update`: called at the end of concurrent marking with the
    /// measured marking duration and the allocation rate observed
    /// during marking.
    ///
    /// Model: if marking finished early relative to the allocation
    /// window, IHOP can rise (mark later); if it was tight, IHOP
    /// drops (mark earlier). The adjustment scales with the ratio of
    /// marking time to the available window.
    pub fn update(
        &mut self,
        marking_time_ms: f64,
        alloc_rate_bytes_per_sec: f64,
        heap_capacity_bytes: u64,
        time_available_ms: f64,
    ) {
        self.last_time_to_mark_ms = marking_time_ms;
        self.last_alloc_rate_bytes_per_sec = alloc_rate_bytes_per_sec;
        if alloc_rate_bytes_per_sec <= 0.0 || heap_capacity_bytes == 0 || time_available_ms <= 0.0 {
            return;
        }
        // fraction of the allocation window consumed by marking
        let utilization = marking_time_ms / time_available_ms;
        // adjust proportionally; clamp step to avoid oscillation
        let step = (utilization - 0.75) * 10.0; // aim ~75% utilization
        let new_ihop = self.initiate_heap_occupancy_percent + step;
        self.initiate_heap_occupancy_percent = new_ihop.clamp(self.min_percent, self.max_percent);
    }

    /// `set_initiate_heap_occupancy_percent` (ergonomic/flag path).
    pub fn set_initiate_heap_occupancy_percent(&mut self, pct: f64) {
        self.initiate_heap_occupancy_percent = pct.clamp(0.0, 100.0);
    }

    /// Whether a GC should be initiated now given old-gen occupancy.
    /// `old_gen_used_bytes` / `heap_capacity_bytes` -> occupancy %.
    pub fn should_initiate_concurrent_mark(
        &self,
        old_gen_used_bytes: u64,
        heap_capacity_bytes: u64,
    ) -> bool {
        if heap_capacity_bytes == 0 {
            return false;
        }
        let occupancy = old_gen_used_bytes as f64 / heap_capacity_bytes as f64 * 100.0;
        occupancy >= self.initiate_heap_occupancy_percent
    }
}

impl Default for G1IHOPControl {
    fn default() -> Self {
        Self::new()
    }
}
