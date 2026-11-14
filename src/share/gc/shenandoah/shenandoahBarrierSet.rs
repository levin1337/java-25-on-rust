/*
 * Port of OpenJDK 25 HotSpot Shenandoah Barrier Set
 * (share/gc/shenandoah/shenandoahBarrierSet.hpp / shenandoahBarrierSet.cpp)
 *
 * Implements Shenandoah Load-Reference Barrier (LRB), SATB pre-write barriers,
 * and forwarding-pointer resolution for concurrent evacuation and reference updating.
 */

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub struct ShenandoahBarrierSet {
    is_evacuation_in_progress: AtomicBool,
    is_update_refs_in_progress: AtomicBool,
    satb_mark_queue: Vec<u64>,
    forwarding_table: HashMap<u64, u64>,
    cset_addresses: HashSet<u64>,
}

impl ShenandoahBarrierSet {
    pub fn new() -> Self {
        Self {
            is_evacuation_in_progress: AtomicBool::new(false),
            is_update_refs_in_progress: AtomicBool::new(false),
            satb_mark_queue: Vec::new(),
            forwarding_table: HashMap::new(),
            cset_addresses: HashSet::new(),
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

    /// Load-Reference Barrier (LRB)
    /// Fast path: if not in evacuation/update-refs or object is null or not in cset, returns raw address.
    /// Slow path: resolves forwarding pointer to to-space copy.
    #[inline]
    pub fn load_reference_barrier(&self, addr: u64) -> u64 {
        if addr == 0 {
            return 0;
        }

        // Fast check: is GC currently moving or updating objects?
        if !self.is_evacuation_in_progress() && !self.is_update_refs_in_progress() {
            return addr;
        }

        // Is this object in the collection set?
        if !self.cset_addresses.contains(&addr) {
            return addr;
        }

        // Resolve forwarded address
        if let Some(&to_addr) = self.forwarding_table.get(&addr) {
            to_addr
        } else {
            addr
        }
    }

    /// SATB (Snapshot-At-The-Beginning) pre-barrier
    /// Enqueues the prior field value before overwriting it if marking is active.
    pub fn satb_enqueue(&mut self, prev_val: u64) {
        if prev_val != 0 {
            self.satb_mark_queue.push(prev_val);
        }
    }

    pub fn drain_satb_queue(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.satb_mark_queue)
    }

    pub fn satb_queue_len(&self) -> usize {
        self.satb_mark_queue.len()
    }
}

impl Default for ShenandoahBarrierSet {
    fn default() -> Self {
        Self::new()
    }
}
