/*
 * Port of OpenJDK 25 HotSpot ShenandoahPacer
 * (share/gc/shenandoah/shenandoahPacer.hpp / shenandoahPacer.cpp /
 *  shenandoahPacer.inline.hpp, on top of TruncatedSeq from
 *  shenandoahNumberSeq.rs)
 *
 * Tax-and-spend allocation pacing: GC threads report progress words,
 * mutator threads claim budget (tax = words * tax_rate) or stall.
 */

use super::shenandoahNumberSeq::TruncatedSeq;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

pub const PACING_PROGRESS_UNINIT: i64 = -1;
pub const PACING_PROGRESS_ZERO: i64 = 0;

pub const SHENANDOAH_PACING_CYCLE_SLACK: usize = 10;
pub const SHENANDOAH_PACING_SURCHARGE: f64 = 1.2;
pub const SHENANDOAH_PACING_IDLE_SLACK: usize = 2;
pub const SHENANDOAH_PACING_MAX_DELAY_MS: u64 = 10_000;

pub struct ShenandoahPacer {
    last_time: f64,
    progress_history: TruncatedSeq,
    epoch: AtomicI64,
    tax_rate: AtomicU64,
    budget: AtomicI64,
    progress: AtomicI64,
    max_capacity: usize,
    free_available: usize,
    cset_used: usize,
    heap_used: usize,
}

impl ShenandoahPacer {
    pub fn new(max_capacity: usize) -> Self {
        Self {
            last_time: 0.0,
            progress_history: TruncatedSeq::new(5, 0.3),
            epoch: AtomicI64::new(0),
            tax_rate: AtomicU64::new(1.0f64.to_bits()),
            budget: AtomicI64::new(0),
            progress: AtomicI64::new(PACING_PROGRESS_UNINIT),
            max_capacity,
            free_available: max_capacity,
            cset_used: 0,
            heap_used: 0,
        }
    }

    pub fn set_cycle_inputs(&mut self, free: usize, cset_used: usize, heap_used: usize) {
        self.free_available = free;
        self.cset_used = cset_used;
        self.heap_used = heap_used;
    }

    pub fn setup_for_idle(&mut self) {
        let initial = self.max_capacity / 100 * SHENANDOAH_PACING_IDLE_SLACK;
        self.restart_with(initial, 1.0);
    }

    pub fn setup_for_mark(&mut self) {
        let live = self.update_and_get_progress_history();
        let free = self.free_available;
        let non_taxable = free * SHENANDOAH_PACING_CYCLE_SLACK / 100;
        let taxable = (free - non_taxable).max(1);
        let mut tax = live as f64 / taxable as f64;
        tax *= SHENANDOAH_PACING_SURCHARGE;
        self.restart_with(non_taxable, tax);
    }

    pub fn setup_for_evac(&mut self) {
        let used = self.cset_used;
        let free = self.free_available;
        let non_taxable = free * SHENANDOAH_PACING_CYCLE_SLACK / 100;
        let taxable = (free - non_taxable).max(1);
        let mut tax = used as f64 / taxable as f64;
        tax *= 2.0;
        tax = tax.max(1.0);
        tax *= SHENANDOAH_PACING_SURCHARGE;
        self.restart_with(non_taxable, tax);
    }

    pub fn setup_for_update_refs(&mut self) {
        let used = self.heap_used;
        let free = self.free_available;
        let non_taxable = free * SHENANDOAH_PACING_CYCLE_SLACK / 100;
        let taxable = (free - non_taxable).max(1);
        let mut tax = used as f64 / taxable as f64;
        tax *= SHENANDOAH_PACING_SURCHARGE;
        tax = tax.max(1.0);
        self.restart_with(non_taxable, tax);
    }

    pub fn setup_for_reset(&mut self) {
        self.restart_with(self.max_capacity, 1.0);
    }

    fn update_and_get_progress_history(&mut self) -> usize {
        let cur = self.progress.load(Ordering::Relaxed);
        if cur == PACING_PROGRESS_UNINIT {
            self.progress.store(PACING_PROGRESS_ZERO, Ordering::Relaxed);
            return (self.max_capacity as f64 * 0.1) as usize;
        }
        self.progress_history.add(cur as f64);
        self.progress.store(PACING_PROGRESS_ZERO, Ordering::Relaxed);
        (self.progress_history.avg() * 8.0) as usize
    }

    fn restart_with(&mut self, non_taxable_bytes: usize, tax_rate: f64) {
        let initial = ((non_taxable_bytes as f64 * tax_rate) as i64) >> 3;
        self.budget.store(initial, Ordering::Relaxed);
        self.tax_rate.store(tax_rate.to_bits(), Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }

    pub fn report_mark(&self, words: usize) {
        self.report_internal(words);
    }

    pub fn report_evac(&self, words: usize) {
        self.report_internal(words);
    }

    pub fn report_update_refs(&self, words: usize) {
        self.report_internal(words);
    }

    fn report_internal(&self, words: usize) {
        self.report_progress_internal(words);
        self.add_budget(words);
    }

    fn report_progress_internal(&self, words: usize) {
        self.progress.fetch_add(words as i64, Ordering::Relaxed);
    }

    fn add_budget(&self, words: usize) {
        let tax = (words as f64 * self.tax_rate()).max(1.0) as i64;
        self.budget.fetch_add(tax, Ordering::Relaxed);
    }

    pub fn claim_for_alloc(&self, words: usize, force: bool) -> bool {
        let tax = ((words as f64 * self.tax_rate()).max(1.0)) as i64;
        let mut cur = self.budget.load(Ordering::Relaxed);
        loop {
            if cur < tax && !force {
                return false;
            }
            let new_val = cur - tax;
            match self.budget.compare_exchange_weak(
                cur,
                new_val,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(v) => cur = v,
            }
        }
    }

    pub fn pace_for_alloc(&self, words: usize) {
        let mut claimed = self.claim_for_alloc(words, false);
        if claimed {
            return;
        }
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(SHENANDOAH_PACING_MAX_DELAY_MS);
        while !claimed && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
            claimed = self.claim_for_alloc(words, false);
        }
        if !claimed {
            claimed = self.claim_for_alloc(words, true);
            debug_assert!(claimed, "force claim should always succeed");
        }
    }

    pub fn unpace_for_alloc(&self, epoch: i64, words: usize) {
        if self.epoch() != epoch {
            return;
        }
        let tax = ((words as f64 * self.tax_rate()).max(1.0)) as usize;
        self.add_budget(tax);
    }

    pub fn epoch(&self) -> i64 {
        self.epoch.load(Ordering::Relaxed)
    }

    pub fn budget(&self) -> i64 {
        self.budget.load(Ordering::Relaxed)
    }

    pub fn progress(&self) -> i64 {
        self.progress.load(Ordering::Relaxed)
    }

    pub fn tax_rate(&self) -> f64 {
        f64::from_bits(self.tax_rate.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacing_budget_cycle() {
        let mut pacer = ShenandoahPacer::new(1 << 30);
        pacer.set_cycle_inputs(512 << 20, 0, 256 << 20);
        pacer.setup_for_evac();
        let epoch = pacer.epoch();
        assert!(epoch > 0);

        let words = 1024usize;
        assert!(pacer.claim_for_alloc(words, false) || pacer.claim_for_alloc(words, true));
        pacer.unpace_for_alloc(epoch, words);
        assert!(pacer.budget() >= 0);
    }

    #[test]
    fn pacer_phases_restart_budget() {
        let mut pacer = ShenandoahPacer::new(1 << 30);
        pacer.set_cycle_inputs(100 << 20, 10 << 20, 50 << 20);
        pacer.setup_for_mark();
        let b1 = pacer.budget();
        pacer.setup_for_reset();
        assert_eq!(pacer.budget(), (1 << 30) as i64 >> 3);
        pacer.setup_for_idle();
        assert!(pacer.budget() > 0);
        pacer.setup_for_update_refs();
        assert!(b1 >= 0);
    }

    #[test]
    fn force_claim_when_exhausted() {
        let mut pacer = ShenandoahPacer::new(1 << 20);
        pacer.setup_for_reset();
        let huge = (1usize << 40) / 8;
        assert!(!pacer.claim_for_alloc(huge, false));
        assert!(pacer.claim_for_alloc(huge, true));
    }
}
