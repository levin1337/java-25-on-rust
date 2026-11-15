/*
 * Port of OpenJDK 25 HotSpot ShenandoahCollectorPolicy
 * (share/gc/shenandoah/shenandoahCollectorPolicy.hpp / shenandoahCollectorPolicy.cpp)
 *
 * GC outcome accounting: concurrent/degenerated/full success and
 * abbreviation counters, degenerated-point histogram, collection-cause
 * histogram, and the generational degenerated-to-full upgrade rule.
 */

use crate::share::gc::shared::gcCause::GCCause;

pub const GENERATIONAL_CONSECUTIVE_BAD_DEGEN_PROGRESS_THRESHOLD: u32 = 2;
pub const DEGENERATED_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShenandoahDegenPoint {
    Evac = 0,
    UpdateRefs = 1,
    FullGc = 2,
}

impl ShenandoahDegenPoint {
    pub fn degen_point_to_string(self) -> &'static str {
        match self {
            Self::Evac => "evacuation",
            Self::UpdateRefs => "update references",
            Self::FullGc => "full GC",
        }
    }
}

pub struct ShenandoahCollectorPolicy {
    success_concurrent_gcs: usize,
    abbreviated_concurrent_gcs: usize,
    success_degenerated_gcs: usize,
    abbreviated_degenerated_gcs: usize,
    success_full_gcs: usize,
    consecutive_degenerated_gcs: u32,
    consecutive_degenerated_gcs_without_progress: u32,
    consecutive_young_gcs: usize,
    mixed_gcs: usize,
    success_old_gcs: usize,
    interrupted_old_gcs: usize,
    alloc_failure_degenerated: usize,
    alloc_failure_degenerated_upgrade_to_full: usize,
    alloc_failure_full: usize,
    collection_cause_counts: [usize; 64],
    degen_point_counts: [usize; DEGENERATED_LIMIT],
    in_shutdown: bool,
}

impl ShenandoahCollectorPolicy {
    pub fn new() -> Self {
        Self {
            success_concurrent_gcs: 0,
            abbreviated_concurrent_gcs: 0,
            success_degenerated_gcs: 0,
            abbreviated_degenerated_gcs: 0,
            success_full_gcs: 0,
            consecutive_degenerated_gcs: 0,
            consecutive_degenerated_gcs_without_progress: 0,
            consecutive_young_gcs: 0,
            mixed_gcs: 0,
            success_old_gcs: 0,
            interrupted_old_gcs: 0,
            alloc_failure_degenerated: 0,
            alloc_failure_degenerated_upgrade_to_full: 0,
            alloc_failure_full: 0,
            collection_cause_counts: [0; 64],
            degen_point_counts: [0; DEGENERATED_LIMIT],
            in_shutdown: false,
        }
    }

    fn reset_consecutive_degenerated_gcs(&mut self) {
        self.consecutive_degenerated_gcs = 0;
        self.consecutive_degenerated_gcs_without_progress = 0;
    }

    pub fn record_mixed_cycle(&mut self) {
        self.mixed_gcs += 1;
    }

    pub fn record_success_old(&mut self) {
        self.consecutive_young_gcs = 0;
        self.success_old_gcs += 1;
    }

    pub fn record_interrupted_old(&mut self) {
        self.consecutive_young_gcs = 0;
        self.interrupted_old_gcs += 1;
    }

    pub fn record_success_concurrent(&mut self, is_young: bool, is_abbreviated: bool) {
        self.update_young(is_young);
        self.reset_consecutive_degenerated_gcs();
        self.success_concurrent_gcs += 1;
        if is_abbreviated {
            self.abbreviated_concurrent_gcs += 1;
        }
    }

    pub fn record_degenerated(&mut self, is_young: bool, is_abbreviated: bool, progress: bool) {
        self.update_young(is_young);
        self.success_degenerated_gcs += 1;
        self.consecutive_degenerated_gcs += 1;
        if progress {
            self.consecutive_degenerated_gcs_without_progress = 0;
        } else {
            self.consecutive_degenerated_gcs_without_progress += 1;
        }
        if is_abbreviated {
            self.abbreviated_degenerated_gcs += 1;
        }
    }

    fn update_young(&mut self, is_young: bool) {
        if is_young {
            self.consecutive_young_gcs += 1;
        } else {
            self.consecutive_young_gcs = 0;
        }
    }

    pub fn record_success_full(&mut self) {
        self.reset_consecutive_degenerated_gcs();
        self.consecutive_young_gcs = 0;
        self.success_full_gcs += 1;
    }

    pub fn record_alloc_failure_to_degenerated(&mut self, point: ShenandoahDegenPoint) {
        self.alloc_failure_degenerated += 1;
        self.degen_point_counts[point as usize] += 1;
    }

    pub fn record_alloc_failure_to_full(&mut self) {
        self.alloc_failure_full += 1;
    }

    pub fn record_degenerated_upgrade_to_full(&mut self) {
        self.reset_consecutive_degenerated_gcs();
        self.alloc_failure_degenerated_upgrade_to_full += 1;
    }

    pub fn record_collection_cause(&mut self, cause: GCCause) {
        let idx = cause as usize;
        if idx < self.collection_cause_counts.len() {
            self.collection_cause_counts[idx] += 1;
        }
    }

    pub fn record_shutdown(&mut self) {
        self.in_shutdown = true;
    }

    pub fn is_at_shutdown(&self) -> bool {
        self.in_shutdown
    }

    pub fn full_gc_count(&self) -> usize {
        self.success_full_gcs + self.alloc_failure_degenerated_upgrade_to_full
    }

    pub fn consecutive_degenerated_gc_count(&self) -> u32 {
        self.consecutive_degenerated_gcs
    }

    pub fn consecutive_young_gc_count(&self) -> usize {
        self.consecutive_young_gcs
    }

    pub fn generational_should_upgrade_degenerated_gc(&self) -> bool {
        self.consecutive_degenerated_gcs_without_progress
            >= GENERATIONAL_CONSECUTIVE_BAD_DEGEN_PROGRESS_THRESHOLD
    }

    pub fn success_concurrent_gcs(&self) -> usize {
        self.success_concurrent_gcs
    }

    pub fn abbreviated_concurrent_gcs(&self) -> usize {
        self.abbreviated_concurrent_gcs
    }

    pub fn success_degenerated_gcs(&self) -> usize {
        self.success_degenerated_gcs
    }

    pub fn success_full_gcs(&self) -> usize {
        self.success_full_gcs
    }

    pub fn mixed_gcs(&self) -> usize {
        self.mixed_gcs
    }

    pub fn success_old_gcs(&self) -> usize {
        self.success_old_gcs
    }

    pub fn alloc_failure_degenerated(&self) -> usize {
        self.alloc_failure_degenerated
    }

    pub fn degen_point_count(&self, point: ShenandoahDegenPoint) -> usize {
        self.degen_point_counts[point as usize]
    }

    pub fn collection_cause_count(&self, cause: GCCause) -> usize {
        let idx = cause as usize;
        if idx < self.collection_cause_counts.len() {
            self.collection_cause_counts[idx]
        } else {
            0
        }
    }

    pub fn completed_gcs(&self) -> usize {
        self.success_full_gcs
            + self.success_degenerated_gcs
            + self.success_concurrent_gcs
            + self.success_old_gcs
    }

    pub fn print_gc_stats(&self) -> String {
        let mut out = String::new();
        let completed = self.completed_gcs();
        out.push_str(&format!("{:5} Completed GCs\n", completed));
        out.push_str(&format!(
            "{:5} Successful Concurrent GCs ({:.2}%)\n",
            self.success_concurrent_gcs,
            percent_of(self.success_concurrent_gcs, completed)
        ));
        out.push_str(&format!(
            "{:5} abbreviated ({:.2}%)\n",
            self.abbreviated_concurrent_gcs,
            percent_of(self.abbreviated_concurrent_gcs, self.success_concurrent_gcs)
        ));
        let degenerated = self.alloc_failure_degenerated_upgrade_to_full + self.success_degenerated_gcs;
        out.push_str(&format!(
            "{:5} Degenerated GCs ({:.2}%)\n",
            degenerated,
            percent_of(degenerated, completed)
        ));
        out.push_str(&format!(
            "{:5} Full GCs ({:.2}%)\n",
            self.success_full_gcs,
            percent_of(self.success_full_gcs, completed)
        ));
        for (i, &c) in self.degen_point_counts.iter().enumerate() {
            if c > 0 {
                let point = match i {
                    0 => ShenandoahDegenPoint::Evac,
                    1 => ShenandoahDegenPoint::UpdateRefs,
                    _ => ShenandoahDegenPoint::FullGc,
                };
                out.push_str(&format!(
                    "    {:5} happened at {}\n",
                    c,
                    point.degen_point_to_string()
                ));
            }
        }
        out
    }
}

impl Default for ShenandoahCollectorPolicy {
    fn default() -> Self {
        Self::new()
    }
}

fn percent_of(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degen_progress_tracking() {
        let mut p = ShenandoahCollectorPolicy::new();
        p.record_degenerated(true, false, false);
        p.record_degenerated(true, false, false);
        assert_eq!(p.consecutive_degenerated_gc_count(), 2);
        assert!(p.generational_should_upgrade_degenerated_gc());
        p.record_degenerated_upgrade_to_full();
        assert_eq!(p.consecutive_degenerated_gc_count(), 0);
        assert!(!p.generational_should_upgrade_degenerated_gc());
    }

    #[test]
    fn success_resets_consecutive() {
        let mut p = ShenandoahCollectorPolicy::new();
        p.record_degenerated(false, true, true);
        p.record_success_concurrent(false, true);
        assert_eq!(p.consecutive_degenerated_gc_count(), 0);
        assert_eq!(p.success_concurrent_gcs(), 1);
        assert_eq!(p.abbreviated_concurrent_gcs(), 1);
        assert_eq!(p.consecutive_young_gc_count(), 0);
    }

    #[test]
    fn young_gc_streak() {
        let mut p = ShenandoahCollectorPolicy::new();
        p.record_success_concurrent(true, false);
        p.record_success_concurrent(true, false);
        assert_eq!(p.consecutive_young_gc_count(), 2);
        p.record_success_old();
        assert_eq!(p.consecutive_young_gc_count(), 0);
        assert_eq!(p.success_old_gcs(), 1);
    }
}
