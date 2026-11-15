/*
 * Port of OpenJDK 25 HotSpot Shenandoah Heap Region
 * (share/gc/shenandoah/shenandoahHeapRegion.hpp / shenandoahHeapRegion.cpp)
 *
 * Implements Shenandoah region state machine, garbage tracking,
 * allocation watermarks, and collection-set membership.
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShenandoahRegionState {
    EmptyUncommitted,
    EmptyCommitted,
    Regular,
    Pinned,
    HumongousStart,
    HumongousContinuation,
    Trash,
}

impl ShenandoahRegionState {
    pub fn is_empty(self) -> bool {
        matches!(self, Self::EmptyUncommitted | Self::EmptyCommitted)
    }

    pub fn is_committed(self) -> bool {
        !matches!(self, Self::EmptyUncommitted)
    }

    pub fn is_humongous(self) -> bool {
        matches!(self, Self::HumongousStart | Self::HumongousContinuation)
    }

    pub fn is_cset_candidate(self) -> bool {
        matches!(
            self,
            Self::Regular | Self::Pinned | Self::HumongousStart | Self::HumongousContinuation
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShenandoahAffiliation {
    NonGen,
    Old,
    Young,
}

impl ShenandoahAffiliation {
    pub fn name(self) -> &'static str {
        match self {
            Self::NonGen => "Non-generational",
            Self::Old => "Old",
            Self::Young => "Young",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShenandoahHeapRegion {
    index: usize,
    bottom: usize,
    top: usize,
    end: usize,
    state: ShenandoahRegionState,
    committed: bool,
    live_data: usize,
    in_cset: bool,
    seqnum_first_alloc: u64,
    seqnum_last_alloc: u64,
    tlab_allocs: usize,
    gclab_allocs: usize,
    plab_allocs: usize,
    critical_pins: usize,
    mixed_candidate_garbage_words: usize,
    update_watermark: usize,
    age: u8,
    affiliation: ShenandoahAffiliation,
    needs_bitmap_reset: bool,
    recycling: bool,
}

impl ShenandoahHeapRegion {
    pub const DEFAULT_REGION_SIZE: usize = 4 * 1024 * 1024;
    pub const REGION_SIZE_BYTES_SHIFT: usize = 22;
    pub const REGION_SIZE_BYTES_MASK: usize = Self::DEFAULT_REGION_SIZE - 1;
    pub const REGION_SIZE_WORDS_SHIFT: usize = Self::REGION_SIZE_BYTES_SHIFT - 3;
    pub const REGION_SIZE_WORDS: usize = Self::DEFAULT_REGION_SIZE / 8;
    pub const REGION_SIZE_WORDS_MASK: usize = Self::REGION_SIZE_WORDS - 1;
    pub const MIN_NUM_REGIONS: usize = 10;

    pub fn new(index: usize, bottom: usize, size: usize) -> Self {
        Self::new_affiliated(index, bottom, size, ShenandoahAffiliation::NonGen, true)
    }

    pub fn new_affiliated(
        index: usize,
        bottom: usize,
        size: usize,
        affiliation: ShenandoahAffiliation,
        committed: bool,
    ) -> Self {
        Self {
            index,
            bottom,
            top: bottom,
            end: bottom + size,
            state: if committed {
                ShenandoahRegionState::EmptyCommitted
            } else {
                ShenandoahRegionState::EmptyUncommitted
            },
            committed,
            live_data: 0,
            in_cset: false,
            seqnum_first_alloc: 0,
            seqnum_last_alloc: 0,
            tlab_allocs: 0,
            gclab_allocs: 0,
            plab_allocs: 0,
            critical_pins: 0,
            mixed_candidate_garbage_words: 0,
            update_watermark: bottom,
            age: 0,
            affiliation,
            needs_bitmap_reset: false,
            recycling: false,
        }
    }

    pub fn setup_sizes(max_heap_size: usize) -> usize {
        let max_regions = 32768;
        let min_regions = Self::MIN_NUM_REGIONS;
        let max = (max_heap_size + Self::DEFAULT_REGION_SIZE - 1) / Self::DEFAULT_REGION_SIZE;
        let num = max.clamp(min_regions, max_regions);
        num * Self::DEFAULT_REGION_SIZE
    }

    #[inline]
    pub fn index(&self) -> usize {
        self.index
    }

    #[inline]
    pub fn bottom(&self) -> usize {
        self.bottom
    }

    #[inline]
    pub fn top(&self) -> usize {
        self.top
    }

    #[inline]
    pub fn end(&self) -> usize {
        self.end
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.end - self.bottom
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.size()
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.top - self.bottom
    }

    #[inline]
    pub fn free(&self) -> usize {
        self.end - self.top
    }

    #[inline]
    pub fn state(&self) -> ShenandoahRegionState {
        self.state
    }

    #[inline]
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }

    #[inline]
    pub fn is_regular(&self) -> bool {
        self.state == ShenandoahRegionState::Regular
    }

    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.state == ShenandoahRegionState::Pinned
    }

    #[inline]
    pub fn is_humongous(&self) -> bool {
        self.state.is_humongous()
    }

    #[inline]
    pub fn is_humongous_start(&self) -> bool {
        self.state == ShenandoahRegionState::HumongousStart
    }

    #[inline]
    pub fn is_humongous_continuation(&self) -> bool {
        self.state == ShenandoahRegionState::HumongousContinuation
    }

    #[inline]
    pub fn is_cset(&self) -> bool {
        self.state == ShenandoahRegionState::Regular && self.in_cset
            || self.is_humongous()
    }

    #[inline]
    pub fn is_cset_candidate(&self) -> bool {
        self.state.is_cset_candidate() && self.in_cset
    }

    #[inline]
    pub fn in_cset(&self) -> bool {
        self.in_cset
    }

    #[inline]
    pub fn set_in_cset(&mut self, in_cset: bool) {
        self.in_cset = in_cset;
    }

    #[inline]
    pub fn is_trash(&self) -> bool {
        self.state == ShenandoahRegionState::Trash
    }

    #[inline]
    pub fn is_affiliated(&self) -> bool {
        self.affiliation != ShenandoahAffiliation::NonGen
    }

    #[inline]
    pub fn affiliation(&self) -> ShenandoahAffiliation {
        self.affiliation
    }

    #[inline]
    pub fn set_affiliation(&mut self, affiliation: ShenandoahAffiliation) {
        self.affiliation = affiliation;
    }

    #[inline]
    pub fn is_young(&self) -> bool {
        self.affiliation == ShenandoahAffiliation::Young
    }

    #[inline]
    pub fn is_old(&self) -> bool {
        self.affiliation == ShenandoahAffiliation::Old
    }

    #[inline]
    pub fn live_data(&self) -> usize {
        self.live_data
    }

    #[inline]
    pub fn set_live_data(&mut self, bytes: usize) {
        self.live_data = bytes.min(self.used());
    }

    #[inline]
    pub fn live_data_bytes(&self) -> usize {
        self.live_data
    }

    #[inline]
    pub fn garbage(&self) -> usize {
        self.used().saturating_sub(self.live_data)
    }

    #[inline]
    pub fn tlab_allocs(&self) -> usize {
        self.tlab_allocs
    }

    #[inline]
    pub fn gclab_allocs(&self) -> usize {
        self.gclab_allocs
    }

    #[inline]
    pub fn plab_allocs(&self) -> usize {
        self.plab_allocs
    }

    #[inline]
    pub fn critical_pins(&self) -> usize {
        self.critical_pins
    }

    #[inline]
    pub fn increment_critical_pins(&mut self) {
        self.critical_pins += 1;
    }

    #[inline]
    pub fn decrement_critical_pins(&mut self) {
        self.critical_pins = self.critical_pins.saturating_sub(1);
    }

    #[inline]
    pub fn has_critical_pins(&self) -> bool {
        self.critical_pins > 0
    }

    #[inline]
    pub fn mixed_candidate_garbage_words(&self) -> usize {
        self.mixed_candidate_garbage_words
    }

    #[inline]
    pub fn set_mixed_candidate_garbage_words(&mut self, words: usize) {
        self.mixed_candidate_garbage_words = words;
    }

    #[inline]
    pub fn update_watermark(&self) -> usize {
        self.update_watermark
    }

    #[inline]
    pub fn set_update_watermark(&mut self, watermark: usize) {
        self.update_watermark = watermark.clamp(self.bottom, self.end);
    }

    #[inline]
    pub fn age(&self) -> u8 {
        self.age
    }

    #[inline]
    pub fn set_age(&mut self, age: u8) {
        self.age = age.min(15);
    }

    #[inline]
    pub fn needs_bitmap_reset(&self) -> bool {
        self.needs_bitmap_reset
    }

    #[inline]
    pub fn set_needs_bitmap_reset(&mut self, needs: bool) {
        self.needs_bitmap_reset = needs;
    }

    #[inline]
    pub fn is_recycling(&self) -> bool {
        self.recycling
    }

    #[inline]
    pub fn try_start_recycling(&mut self) -> bool {
        if self.recycling {
            return false;
        }
        self.recycling = true;
        true
    }

    #[inline]
    pub fn finish_recycling(&mut self) {
        self.recycling = false;
    }

    #[inline]
    pub fn increment_tlab_allocs(&mut self) {
        self.tlab_allocs += 1;
    }

    #[inline]
    pub fn increment_gclab_allocs(&mut self) {
        self.gclab_allocs += 1;
    }

    #[inline]
    pub fn increment_plab_allocs(&mut self) {
        self.plab_allocs += 1;
    }

    #[inline]
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.bottom && addr < self.end
    }

    pub fn is_free_or_regular(&self) -> bool {
        self.is_empty() || self.is_regular()
    }

    pub fn can_allocate(&self, bytes: usize) -> bool {
        self.committed && bytes <= self.free()
    }

    pub fn allocate(&mut self, bytes: usize, seq: u64) -> Option<usize> {
        self.allocate_with_watermark(bytes, seq, false)
    }

    pub fn allocate_plab(&mut self, bytes: usize, seq: u64) -> Option<usize> {
        if let Some(addr) = self.allocate_with_watermark(bytes, seq, true) {
            self.plab_allocs += 1;
            return Some(addr);
        }
        None
    }

    fn allocate_with_watermark(
        &mut self,
        bytes: usize,
        seq: u64,
        use_watermark: bool,
    ) -> Option<usize> {
        let align = 8;
        let aligned_bytes = bytes.div_ceil(align) * align;
        let limit = if use_watermark {
            self.update_watermark
        } else {
            self.end
        };
        let alloc_end = self.top + aligned_bytes;
        if self.committed && alloc_end <= limit {
            let addr = self.top;
            self.top = alloc_end;
            match self.state {
                ShenandoahRegionState::EmptyCommitted => {
                    self.state = ShenandoahRegionState::Regular;
                    self.seqnum_first_alloc = seq;
                }
                ShenandoahRegionState::EmptyUncommitted => {}
                _ => {}
            }
            self.seqnum_last_alloc = seq;
            Some(addr)
        } else {
            None
        }
    }

    pub fn make_humongous_start(&mut self, size: usize, seq: u64) {
        self.state = ShenandoahRegionState::HumongousStart;
        self.top = self.bottom + size.min(self.end - self.bottom);
        self.seqnum_first_alloc = seq;
        self.seqnum_last_alloc = seq;
        self.live_data = self.used();
    }

    pub fn make_humongous_cont(&mut self, size: usize, seq: u64) {
        self.state = ShenandoahRegionState::HumongousContinuation;
        self.top = self.bottom + size.min(self.end - self.bottom);
        self.seqnum_first_alloc = seq;
        self.seqnum_last_alloc = seq;
        self.live_data = self.used();
    }

    pub fn make_humongous_start_end(&mut self, size: usize, seq: u64) {
        self.make_humongous_start(size, seq);
    }

    pub fn pin(&mut self) {
        if self.state == ShenandoahRegionState::Regular {
            self.state = ShenandoahRegionState::Pinned;
        }
    }

    pub fn unpin(&mut self) {
        if self.state == ShenandoahRegionState::Pinned {
            self.state = ShenandoahRegionState::Regular;
        }
    }

    pub fn pin_with_count(&mut self) {
        self.pin();
        self.increment_critical_pins();
    }

    pub fn make_cset(&mut self) {
        if !self.is_humongous() {
            assert_regular_for_cset();
        }
        self.in_cset = true;
        self.needs_bitmap_reset = true;
    }

    pub fn make_trash(&mut self) {
        self.state = ShenandoahRegionState::Trash;
        self.in_cset = false;
    }

    pub fn make_regular(&mut self) {
        self.state = ShenandoahRegionState::Regular;
        self.in_cset = false;
    }

    pub fn make_empty(&mut self) {
        self.recycle();
    }

    pub fn commit(&mut self) {
        self.committed = true;
        if self.state == ShenandoahRegionState::EmptyUncommitted {
            self.state = ShenandoahRegionState::EmptyCommitted;
        }
    }

    pub fn uncommit(&mut self) {
        self.committed = false;
        self.recycle();
        self.state = ShenandoahRegionState::EmptyUncommitted;
    }

    pub fn recycle(&mut self) {
        self.top = self.bottom;
        self.live_data = 0;
        self.in_cset = false;
        self.state = if self.committed {
            ShenandoahRegionState::EmptyCommitted
        } else {
            ShenandoahRegionState::EmptyUncommitted
        };
        self.seqnum_first_alloc = 0;
        self.seqnum_last_alloc = 0;
        self.update_watermark = self.bottom;
        self.needs_bitmap_reset = false;
        self.recycling = false;
    }

    pub fn reset_live_data(&mut self) {
        self.live_data = 0;
    }

    pub fn reset_tlab_counters(&mut self) {
        self.tlab_allocs = 0;
        self.gclab_allocs = 0;
        self.plab_allocs = 0;
    }

    #[inline]
    pub fn required_regions(bytes: usize) -> usize {
        bytes.div_ceil(Self::DEFAULT_REGION_SIZE)
    }

    #[inline]
    pub fn requires_humongous(words: usize) -> bool {
        words > Self::REGION_SIZE_WORDS
    }

    #[inline]
    pub fn region_size_bytes() -> usize {
        Self::DEFAULT_REGION_SIZE
    }

    #[inline]
    pub fn region_size_bytes_shift() -> usize {
        Self::REGION_SIZE_BYTES_SHIFT
    }

    #[inline]
    pub fn region_size_words() -> usize {
        Self::REGION_SIZE_WORDS
    }

    #[inline]
    pub fn region_size_words_shift() -> usize {
        Self::REGION_SIZE_WORDS_SHIFT
    }

    #[inline]
    pub fn region_size_bytes_mask() -> usize {
        Self::REGION_SIZE_BYTES_MASK
    }

    #[inline]
    pub fn region_size_words_mask() -> usize {
        Self::REGION_SIZE_WORDS_MASK
    }

    #[inline]
    pub fn region_index(addr: usize) -> usize {
        addr >> Self::REGION_SIZE_BYTES_SHIFT
    }
}

fn assert_regular_for_cset() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_state_transitions() {
        let mut r = ShenandoahHeapRegion::new(0, 0x1000, ShenandoahHeapRegion::DEFAULT_REGION_SIZE);
        assert!(r.is_empty());
        let addr = r.allocate(64, 1).unwrap();
        assert_eq!(addr, r.bottom());
        assert!(r.is_regular());
        r.pin();
        assert!(r.is_pinned());
        r.unpin();
        assert!(r.is_regular());
        r.set_live_data(32);
        assert_eq!(r.live_data(), 32);
        assert_eq!(r.garbage(), 32);
        r.make_trash();
        assert!(r.is_trash());
        r.recycle();
        assert!(r.is_empty());
        assert_eq!(r.used(), 0);
    }

    #[test]
    fn humongous_region_spans() {
        let mut start = ShenandoahHeapRegion::new(0, 0, ShenandoahHeapRegion::DEFAULT_REGION_SIZE);
        let mut cont = ShenandoahHeapRegion::new(1, ShenandoahHeapRegion::DEFAULT_REGION_SIZE, ShenandoahHeapRegion::DEFAULT_REGION_SIZE);
        start.make_humongous_start(ShenandoahHeapRegion::DEFAULT_REGION_SIZE, 7);
        cont.make_humongous_cont(ShenandoahHeapRegion::DEFAULT_REGION_SIZE, 7);
        assert!(start.is_humongous_start());
        assert!(cont.is_humongous_continuation());
        assert!(start.is_humongous());
        assert_eq!(start.size(), ShenandoahHeapRegion::DEFAULT_REGION_SIZE);
    }

    #[test]
    fn region_size_math() {
        assert_eq!(
            ShenandoahHeapRegion::required_regions(3 * ShenandoahHeapRegion::DEFAULT_REGION_SIZE + 1),
            4
        );
        assert!(ShenandoahHeapRegion::requires_humongous(ShenandoahHeapRegion::REGION_SIZE_WORDS + 1));
        assert!(!ShenandoahHeapRegion::requires_humongous(ShenandoahHeapRegion::REGION_SIZE_WORDS));
        assert_eq!(ShenandoahHeapRegion::region_index(0x1234_5678), 0x1234_5678 >> 22);
    }
}
