use super::rememberedSet::G1RememberedSet;
use super::heapRegion::HeapRegion;
use crate::share::gc::shared::barrierSet::BarrierSet;
use crate::share::oops::oop::Oop;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// G1 barrier set: SATB pre-write barrier (snapshot-at-the-beginning
/// for concurrent marking) plus the post-write barrier that dirties
/// cards in the remembered set for old→young reference tracking.
pub struct G1BarrierSet {
    satb_buffer: Mutex<Vec<usize>>,
    remembered: G1RememberedSet,
    region_size: usize,
    satb_mark_count: AtomicUsize,
    post_barrier_count: AtomicUsize,
}

impl G1BarrierSet {
    pub fn new() -> Self {
        Self::with_region_size(HeapRegion::DEFAULT_REGION_SIZE)
    }

    pub fn with_region_size(region_size: usize) -> Self {
        Self {
            satb_buffer: Mutex::new(Vec::new()),
            remembered: G1RememberedSet::new(9),
            region_size: region_size.max(1),
            satb_mark_count: AtomicUsize::new(0),
            post_barrier_count: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub fn remembered(&self) -> &G1RememberedSet {
        &self.remembered
    }

    #[inline]
    pub fn satb_marks(&self) -> usize {
        self.satb_mark_count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn post_barriers(&self) -> usize {
        self.post_barrier_count.load(Ordering::Relaxed)
    }

    pub fn clear_buffers(&self) {
        self.satb_buffer.lock().unwrap().clear();
        self.remembered.clear();
    }
}

impl Default for G1BarrierSet {
    fn default() -> Self {
        Self::new()
    }
}

impl BarrierSet for G1BarrierSet {
    fn write_ref_field_pre(&self, field_addr: usize) {
        self.satb_mark_count.fetch_add(1, Ordering::Relaxed);
        let mut buf = self.satb_buffer.lock().unwrap();
        if buf.len() < 1024 {
            buf.push(field_addr);
        }
    }

    fn write_ref_field_post(&self, field_addr: usize, _new_val: Oop) {
        self.post_barrier_count.fetch_add(1, Ordering::Relaxed);
        let region = self.remembered.region_index(field_addr, self.region_size);
        self.remembered.record(region, field_addr);
    }
}
