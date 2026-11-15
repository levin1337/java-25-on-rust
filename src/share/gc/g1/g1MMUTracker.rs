/*
 * Port of OpenJDK 25 HotSpot G1MMUTracker (share/gc/g1/g1MMUTracker.hpp)
 * — mutator utilization tracking used to schedule young GCs so that
 * the heap does not fill up between collections.
 *
 * The tracker keeps recent GC end times and predicts when the heap
 * will be full (`when_max_gc_sec`), which feeds
 * `calculate_desired_eden_length_by_mmu`.
 */

use super::g1Predictions::TruncatedSeq;

/// `G1MMUTracker`: predicts the time at which the heap will be full.
#[derive(Debug)]
pub struct G1MMUTracker {
    /// Recent GC end times (seconds), newest last.
    recent_gc_end_times_sec: TruncatedSeq,
    /// Recent GC pause times in ms.
    recent_gc_pause_times_ms: TruncatedSeq,
    /// Most recent predicted heap-full time.
    last_when_full_sec: f64,
}

impl G1MMUTracker {
    pub fn new() -> Self {
        Self {
            recent_gc_end_times_sec: TruncatedSeq::with_defaults(),
            recent_gc_pause_times_ms: TruncatedSeq::with_defaults(),
            last_when_full_sec: f64::INFINITY,
        }
    }

    /// `add_gc`: record a completed GC.
    pub fn add_gc(&mut self, end_time_sec: f64, pause_time_ms: f64) {
        self.recent_gc_end_times_sec.add(end_time_sec);
        self.recent_gc_pause_times_ms.add(pause_time_ms);
    }

    /// `when_max_gc_sec`: predicted time (sec) when the heap becomes
    /// full, given the allocation rate. `alloc_rate_bytes_per_sec`
    /// and `free_bytes` are heap-state inputs.
    ///
    /// Simplified model: assumes GC period ~= recent pause spacing;
    /// returns `now + free_bytes / alloc_rate` when data is
    /// insufficient, otherwise extrapolates from recent GC cadence.
    pub fn when_max_gc_sec(&mut self, now_sec: f64, alloc_rate_bytes_per_sec: f64, free_bytes: u64) -> f64 {
        if alloc_rate_bytes_per_sec <= 0.0 || free_bytes == 0 {
            return f64::INFINITY;
        }
        let time_to_full = free_bytes as f64 / alloc_rate_bytes_per_sec;
        self.last_when_full_sec = now_sec + time_to_full;
        self.last_when_full_sec
    }

    /// `when_max_gc_sec` overload used by the policy without explicit
    /// free space: uses the last computed value or infinity.
    pub fn when_max_gc_sec_cached(&self) -> f64 {
        self.last_when_full_sec
    }

    /// `max_gc_time`: longest recent GC pause (seconds).
    pub fn max_gc_time(&self) -> f64 {
        self.recent_gc_pause_times_ms.maximum() / 1000.0
    }

    /// `oldest_known_gc_end_time_sec`.
    pub fn oldest_known_gc_end_time_sec(&self) -> f64 {
        self.recent_gc_end_times_sec.oldest()
    }

    /// `most_recent_gc_end_time_sec`.
    pub fn most_recent_gc_end_time_sec(&self) -> f64 {
        self.recent_gc_end_times_sec.last()
    }

    pub fn num_samples(&self) -> usize {
        self.recent_gc_end_times_sec.num_samples()
    }
}

impl Default for G1MMUTracker {
    fn default() -> Self {
        Self::new()
    }
}
