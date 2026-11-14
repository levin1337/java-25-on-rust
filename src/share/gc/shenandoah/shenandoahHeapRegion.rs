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
    HumongousStart,
    HumongousContinuation,
    Pinned,
    Trash,
}

#[derive(Clone, Debug)]
pub struct ShenandoahHeapRegion {
    index: usize,
    bottom: usize,
    top: usize,
    end: usize,
    state: ShenandoahRegionState,
    live_data: usize,
    in_cset: bool,
    seqnum_first_alloc: u64,
    seqnum_last_alloc: u64,
}

impl ShenandoahHeapRegion {
    pub const DEFAULT_REGION_SIZE: usize = 1024 * 1024; // 1 MB

    pub fn new(index: usize, bottom: usize, size: usize) -> Self {
        Self {
            index,
            bottom,
            top: bottom,
            end: bottom + size,
            state: ShenandoahRegionState::EmptyCommitted,
            live_data: 0,
            in_cset: false,
            seqnum_first_alloc: 0,
            seqnum_last_alloc: 0,
        }
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
    pub fn is_empty(&self) -> bool {
        self.state == ShenandoahRegionState::EmptyCommitted
            || self.state == ShenandoahRegionState::EmptyUncommitted
    }

    #[inline]
    pub fn is_regular(&self) -> bool {
        self.state == ShenandoahRegionState::Regular
    }

    #[inline]
    pub fn is_humongous(&self) -> bool {
        self.state == ShenandoahRegionState::HumongousStart
            || self.state == ShenandoahRegionState::HumongousContinuation
    }

    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.state == ShenandoahRegionState::Pinned
    }

    #[inline]
    pub fn is_trash(&self) -> bool {
        self.state == ShenandoahRegionState::Trash
    }

    #[inline]
    pub fn is_in_cset(&self) -> bool {
        self.in_cset
    }

    #[inline]
    pub fn set_in_cset(&mut self, in_cset: bool) {
        self.in_cset = in_cset;
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
    pub fn garbage(&self) -> usize {
        self.used().saturating_sub(self.live_data)
    }

    pub fn allocate(&mut self, bytes: usize, seq: u64) -> Option<usize> {
        let align = 8;
        let aligned_bytes = (bytes + (align - 1)) & !(align - 1);
        if self.top + aligned_bytes <= self.end {
            let addr = self.top;
            self.top += aligned_bytes;
            if self.state == ShenandoahRegionState::EmptyCommitted {
                self.state = ShenandoahRegionState::Regular;
                self.seqnum_first_alloc = seq;
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

    pub fn make_trash(&mut self) {
        self.state = ShenandoahRegionState::Trash;
        self.in_cset = false;
    }

    pub fn recycle(&mut self) {
        self.top = self.bottom;
        self.live_data = 0;
        self.in_cset = false;
        self.state = ShenandoahRegionState::EmptyCommitted;
        self.seqnum_first_alloc = 0;
        self.seqnum_last_alloc = 0;
    }
}
