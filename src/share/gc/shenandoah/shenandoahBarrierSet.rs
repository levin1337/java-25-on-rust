/*
 * Port of OpenJDK 25 HotSpot Shenandoah Barrier Set
 * (share/gc/shenandoah/shenandoahBarrierSet.hpp / shenandoahBarrierSet.cpp /
 *  shenandoahBarrierSet.inline.hpp)
 *
 * Implements the SATB pre-barrier (enqueue overwritten/loaded values
 * into per-thread SATB mark queues while concurrent marking is active),
 * the Load-Reference Barrier resolving forwarded objects during
 * evacuation and reference updating, and mark-word forwarding helpers.
 */

use super::shenandoahForwarding::{MarkWord, ShenandoahForwarding};
use super::shenandoahSATBMarkQueueSet::SatbMarkQueueSet;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct ShenandoahBarrierSet {
    is_evacuation_in_progress: AtomicBool,
    is_update_refs_in_progress: AtomicBool,
    is_concurrent_mark_in_progress: AtomicBool,
    forwarding_table: HashMap<u64, u64>,
    cset_addresses: HashSet<u64>,
    satb_mark_queue_set: SatbMarkQueueSet,
}

impl ShenandoahBarrierSet {
    pub fn new() -> Self {
        Self {
            is_evacuation_in_progress: AtomicBool::new(false),
            is_update_refs_in_progress: AtomicBool::new(false),
            is_concurrent_mark_in_progress: AtomicBool::new(false),
            forwarding_table: HashMap::new(),
            cset_addresses: HashSet::new(),
            satb_mark_queue_set: SatbMarkQueueSet::new(super::shenandoahSATBMarkQueueSet::SATB_BUFFER_SIZE),
        }
    }

    #[inline]
    pub fn is_evacuation_in_progress(&self) -> bool {
        self.is_evacuation_in_progress.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_evacuation_in_progress(&self, val: bool) {
        self.is_evacuation_in_progress.store(val, Ordering::Release);
    }

    #[inline]
    pub fn is_update_refs_in_progress(&self) -> bool {
        self.is_update_refs_in_progress.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_update_refs_in_progress(&self, val: bool) {
        self.is_update_refs_in_progress
            .store(val, Ordering::Release);
    }

    #[inline]
    pub fn is_concurrent_mark_in_progress(&self) -> bool {
        self.is_concurrent_mark_in_progress.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_concurrent_mark_in_progress(&self, val: bool) {
        self.is_concurrent_mark_in_progress
            .store(val, Ordering::Release);
        if val {
            self.satb_mark_queue_set.set_active_all(true);
        } else {
            self.satb_mark_queue_set.set_active_all(false);
        }
    }

    pub fn satb_mark_queue_set(&self) -> &SatbMarkQueueSet {
        &self.satb_mark_queue_set
    }

    pub fn set_cset_addresses(&mut self, cset: HashSet<u64>) {
        self.cset_addresses = cset;
    }

    pub fn clear_cset_addresses(&mut self) {
        self.cset_addresses.clear();
    }

    pub fn record_forwarding(&mut self, from_addr: u64, to_addr: u64) {
        self.forwarding_table.insert(from_addr, to_addr);
    }

    pub fn clear_forwarding(&mut self) {
        self.forwarding_table.clear();
    }

    pub fn forwarding_table(&self) -> &HashMap<u64, u64> {
        &self.forwarding_table
    }

    /// Load-Reference Barrier (LRB)
    /// Fast path: if not in evacuation/update-refs or object is null or not in cset, returns raw address.
    /// Slow path: resolves forwarding pointer to to-space copy.
    #[inline]
    pub fn load_reference_barrier(&self, addr: u64) -> u64 {
        if addr == 0 {
            return 0;
        }

        if !self.is_evacuation_in_progress() && !self.is_update_refs_in_progress() {
            return addr;
        }

        if !self.cset_addresses.contains(&addr) {
            return addr;
        }

        if let Some(&to_addr) = self.forwarding_table.get(&addr) {
            to_addr
        } else {
            addr
        }
    }

    /// Load Value Barrier (LVB): keeps the loaded value alive in the
    /// SATB snapshot when the load happens during concurrent marking.
    #[inline]
    pub fn load_value_barrier(&self, thread: usize, value: u64) {
        if value != 0 && self.is_concurrent_mark_in_progress() {
            self.satb_mark_queue_set.enqueue_known_active(thread, value);
        }
    }

    /// SATB pre-barrier for a strong reference field about to be
    /// overwritten: the previous value must stay alive for marking.
    pub fn write_ref_field_pre(&self, thread: usize, prev_val: u64) {
        self.satb_enqueue(thread, prev_val);
    }

    /// SATB enqueue of a raw oop value (the decoded previous field
    /// value or a value kept alive by a weak/phantom load).
    pub fn satb_enqueue(&self, thread: usize, value: u64) {
        if value != 0 && self.is_concurrent_mark_in_progress() {
            self.satb_mark_queue_set.enqueue_known_active(thread, value);
        }
    }

    pub fn drain_satb_queue(&self, thread: usize) {
        self.satb_mark_queue_set.flush_queue(thread);
    }

    pub fn drain_all_satb_queues(&self) {
        self.satb_mark_queue_set.flush_all();
    }

    pub fn satb_all_entries<F: FnMut(u64)>(&self, f: F) {
        self.satb_mark_queue_set.all_entries(f);
    }

    pub fn satb_completed_buffers(&self) -> usize {
        self.satb_mark_queue_set.completed_buffers_num()
    }

    pub fn register_satb_thread(&self) -> usize {
        self.satb_mark_queue_set.register_queue()
    }

    pub fn resolve_forwarded(&self, mark: MarkWord, self_addr: u64) -> u64 {
        ShenandoahForwarding::get_forwardee(mark, self_addr)
    }

    pub fn try_update_forwardee(header: &AtomicU64, self_addr: u64, update: u64) -> u64 {
        ShenandoahForwarding::try_update_forwardee(header, self_addr, update)
    }

    pub fn is_forwarded(mark: MarkWord) -> bool {
        ShenandoahForwarding::is_forwarded(mark)
    }
}

impl Default for ShenandoahBarrierSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn satb_enqueue_requires_marking() {
        let mut bs = ShenandoahBarrierSet::new();
        let t = bs.register_satb_thread();
        bs.satb_enqueue(t, 0x1000);
        assert_eq!(bs.satb_completed_buffers(), 0);
        let mut seen = Vec::new();
        bs.satb_all_entries(|e| seen.push(e));
        assert!(seen.is_empty());

        bs.set_concurrent_mark_in_progress(true);
        bs.satb_enqueue(t, 0x2000);
        bs.satb_enqueue(t, 0x3000);
        bs.satb_all_entries(|e| seen.push(e));
        assert_eq!(seen, vec![0x2000, 0x3000]);
        bs.set_concurrent_mark_in_progress(false);
    }

    #[test]
    fn lrb_resolution() {
        let mut bs = ShenandoahBarrierSet::new();
        bs.set_evacuation_in_progress(true);
        bs.set_cset_addresses([0x1000u64].into_iter().collect());
        bs.record_forwarding(0x1000, 0x8000);
        assert_eq!(bs.load_reference_barrier(0x1000), 0x8000);
        assert_eq!(bs.load_reference_barrier(0x2000), 0x2000);
        bs.set_evacuation_in_progress(false);
        assert_eq!(bs.load_reference_barrier(0x1000), 0x1000);
    }

    #[test]
    fn mark_word_forwarding_roundtrip() {
        let header = AtomicU64::new(0);
        let obj = 0x7000_0000_0000u64;
        let fwd = 0x7000_0000_8000u64;
        assert_eq!(ShenandoahBarrierSet::try_update_forwardee(&header, obj, fwd), fwd);
        let mark = MarkWord::new(header.load(Ordering::Relaxed));
        let mut bs = ShenandoahBarrierSet::new();
        assert_eq!(bs.resolve_forwarded(mark, obj), fwd);
    }
}
