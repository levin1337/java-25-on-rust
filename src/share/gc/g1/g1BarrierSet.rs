use super::g1Satb::G1SATBMarkQueueSet;
use super::rememberedSet::{G1HotCardCache, G1RememberedSet};
use super::heapRegion::HeapRegion;
use crate::share::gc::shared::barrierSet::BarrierSet;
use crate::share::oops::oop::Oop;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// G1 barrier set: SATB pre-write barrier (snapshot-at-the-beginning
/// for concurrent marking) plus the post-write barrier that dirties
/// cards in the remembered set for old→young reference tracking.
///
/// Mirrors `G1BarrierSet` (g1BarrierSet.hpp): the SATB side is a
/// `G1SATBMarkQueueSet` of per-thread buffers; the post-write side
/// goes through `G1RememberedSet` (from-card cache + card set).
pub struct G1BarrierSet {
    satb_queue_set: Mutex<G1SATBMarkQueueSet>,
    remembered: G1RememberedSet,
    hot_card_cache: G1HotCardCache,
    region_size: usize,
    heap_base: usize,
    num_regions: usize,
    card_shift: u32,
    satb_mark_count: AtomicU64,
    post_barrier_count: AtomicU64,
}

impl G1BarrierSet {
    pub fn new() -> Self {
        Self::with_region_size(HeapRegion::DEFAULT_REGION_SIZE)
    }

    pub fn with_region_size(region_size: usize) -> Self {
        Self::with_geometry(region_size, 0, 4, 9)
    }

    /// Full constructor.
    ///
    /// - `region_size`: bytes per heap region
    /// - `heap_base`: base address for card index computation
    /// - `num_regions`: number of regions in the remembered set
    /// - `card_shift`: log2 of card size in bytes (9 = 512 B cards)
    pub fn with_geometry(region_size: usize, heap_base: usize, num_regions: usize, card_shift: u32) -> Self {
        Self {
            satb_queue_set: Mutex::new(G1SATBMarkQueueSet::new(8)),
            remembered: G1RememberedSet::with_geometry(card_shift, heap_base, region_size, num_regions),
            hot_card_cache: G1HotCardCache::new(),
            region_size: region_size.max(1),
            heap_base,
            num_regions,
            card_shift,
            satb_mark_count: AtomicU64::new(0),
            post_barrier_count: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn remembered(&self) -> &G1RememberedSet {
        &self.remembered
    }

    /// Locked access to the SATB queue set (marker/refinement side).
    pub fn satb_queues(&self) -> std::sync::MutexGuard<'_, G1SATBMarkQueueSet> {
        self.satb_queue_set.lock().unwrap()
    }

    #[inline]
    pub fn hot_card_cache(&self) -> &G1HotCardCache {
        &self.hot_card_cache
    }

    #[inline]
    pub fn region_size(&self) -> usize {
        self.region_size
    }

    #[inline]
    pub fn heap_base(&self) -> usize {
        self.heap_base
    }

    #[inline]
    pub fn num_regions(&self) -> usize {
        self.num_regions
    }

    #[inline]
    pub fn card_shift(&self) -> u32 {
        self.card_shift
    }

    #[inline]
    pub fn satb_marks(&self) -> u64 {
        self.satb_mark_count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn post_barriers(&self) -> u64 {
        self.post_barrier_count.load(Ordering::Relaxed)
    }

    /// `card_index_for` helper for the barrier fast path.
    #[inline]
    pub fn card_index(&self, addr: usize) -> usize {
        addr >> self.card_shift
    }

    /// `region_index_for`.
    #[inline]
    pub fn region_index(&self, addr: usize) -> usize {
        addr / self.region_size
    }

    /// Records a reference-field write's *pre-value* into the SATB
    /// queue of `thread` (0 = mutator fallback slot).
    pub fn satb_enqueue(&self, thread: usize, pre_val: usize) -> bool {
        self.satb_mark_count.fetch_add(1, Ordering::Relaxed);
        self.satb_queue_set.lock().unwrap().enqueue(thread, pre_val)
    }

    /// Post-write barrier: dirty the card containing `field_addr` in
    /// the remembered set. `thread` is used for the hot-card cache.
    pub fn post_write_barrier(&self, thread: usize, field_addr: usize, _new_val: Oop) {
        self.post_barrier_count.fetch_add(1, Ordering::Relaxed);
        let region = self.region_index(field_addr);
        let card_key = self.card_index(field_addr) as u64;
        if self.hot_card_cache.check_and_update(thread, card_key) {
            let _ = self.remembered.record_addr(region, field_addr);
        }
    }

    pub fn clear_buffers(&self) {
        self.satb_queue_set.lock().unwrap().reset();
        self.remembered.clear();
        self.hot_card_cache.clear();
    }
}

impl Default for G1BarrierSet {
    fn default() -> Self {
        Self::new()
    }
}

impl BarrierSet for G1BarrierSet {
    fn write_ref_field_pre(&self, field_addr: usize) {
        // SATB pre-barrier: the previous value would be loaded by the
        // caller; we record the field address as the queue entry —
        // heap integration resolves it to the pre-value oop later.
        let _ = self.satb_enqueue(0, field_addr);
    }

    fn write_ref_field_post(&self, field_addr: usize, new_val: Oop) {
        self.post_write_barrier(0, field_addr, new_val);
    }
}
