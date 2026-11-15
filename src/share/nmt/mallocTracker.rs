/*
 * Port of OpenJDK 25 HotSpot mallocTracker (share/nmt/mallocTracker.hpp/.cpp,
 * mallocTracker.inline.hpp, mallocHeader.hpp)
 *
 * Malloc-side accounting for NMT: per-tag MemoryCounter totals, arena
 * accounting, per-call-site tracking (detail level) and the MallocHeader
 * free-info record that lets callers deaccount a block.
 *
 * In HotSpot the MallocHeader is embedded in front of every tracked
 * allocation so that os::free() can find it. In this Rust port allocations
 * are tracked through the returned MallocHeader handle instead; the
 * accounting semantics (summary counters, site markers, arena counters,
 * malloc-limit checks) are identical.
 */

use super::mallocLimit::MallocLimitHandler;
use super::mallocSiteTable::MallocSiteTable;
use super::memTag::MemTag;
use super::nativeCallStack::NativeCallStack;
use super::nmtCommon::{NmtTrackingLevel, NmtUtil};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// MemoryCounter — counts allocations and total bytes, tracking peaks.
/// Mirrors the C++ class: counters updated atomically.
#[derive(Default)]
pub struct MemoryCounter {
    count: AtomicUsize,
    size: AtomicUsize,
    peak_count: AtomicUsize,
    peak_size: AtomicUsize,
}

impl MemoryCounter {
    pub fn new() -> Self {
        Self::default()
    }

    fn update_peak(&self, size: usize, cnt: usize) {
        let mut peak_sz = self.peak_size.load(Ordering::Relaxed);
        while peak_sz < size {
            match self.peak_size.compare_exchange_weak(
                peak_sz,
                size,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // I won; record the count at which the peak was reached.
                    self.peak_count.store(cnt, Ordering::Relaxed);
                    break;
                }
                Err(old) => peak_sz = old,
            }
        }
    }

    #[inline]
    pub fn set_size_and_count(&self, size: usize, count: usize) {
        self.size.store(size, Ordering::Relaxed);
        self.count.store(count, Ordering::Relaxed);
        self.update_peak(size, count);
    }

    #[inline]
    pub fn allocate(&self, sz: usize) {
        let cnt = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        if sz > 0 {
            let sum = self.size.fetch_add(sz, Ordering::Relaxed) + sz;
            self.update_peak(sum, cnt);
        }
    }

    #[inline]
    pub fn deallocate(&self, sz: usize) {
        debug_assert!(self.count.load(Ordering::Relaxed) > 0, "Nothing allocated yet");
        debug_assert!(self.size.load(Ordering::Relaxed) >= sz, "deallocation > allocated");
        self.count.fetch_sub(1, Ordering::Relaxed);
        if sz > 0 {
            self.size.fetch_sub(sz, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn resize(&self, sz: isize) {
        if sz != 0 {
            if sz >= 0 {
                let sum = self.size.fetch_add(sz as usize, Ordering::Relaxed) + sz as usize;
                self.update_peak(sum, self.count.load(Ordering::Relaxed));
            } else {
                self.size.fetch_sub((-sz) as usize, Ordering::Relaxed);
            }
        }
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn peak_count(&self) -> usize {
        self.peak_count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn peak_size(&self) -> usize {
        self.peak_size.load(Ordering::Relaxed)
    }
}

/// Malloc memory used by a particular subsystem: os::malloc acquisitions
/// plus arena backing memory.
#[derive(Default)]
pub struct MallocMemory {
    malloc: MemoryCounter,
    arena: MemoryCounter,
}

impl MallocMemory {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn record_malloc(&self, sz: usize) {
        self.malloc.allocate(sz);
    }

    #[inline]
    pub fn record_free(&self, sz: usize) {
        self.malloc.deallocate(sz);
    }

    #[inline]
    pub fn record_new_arena(&self) {
        self.arena.allocate(0);
    }

    #[inline]
    pub fn record_arena_free(&self) {
        self.arena.deallocate(0);
    }

    #[inline]
    pub fn record_arena_size_change(&self, sz: isize) {
        self.arena.resize(sz);
    }

    #[inline]
    pub fn malloc_size(&self) -> usize {
        self.malloc.size()
    }

    #[inline]
    pub fn malloc_peak_size(&self) -> usize {
        self.malloc.peak_size()
    }

    #[inline]
    pub fn malloc_count(&self) -> usize {
        self.malloc.count()
    }

    #[inline]
    pub fn arena_size(&self) -> usize {
        self.arena.size()
    }

    #[inline]
    pub fn arena_peak_size(&self) -> usize {
        self.arena.peak_size()
    }

    #[inline]
    pub fn arena_count(&self) -> usize {
        self.arena.count()
    }

    pub fn malloc_counter(&self) -> &MemoryCounter {
        &self.malloc
    }

    pub fn arena_counter(&self) -> &MemoryCounter {
        &self.arena
    }
}

/// A snapshot of malloc'd memory: per-tag usage plus the counters for all
/// mallocs (used to derive total counts and tracking overhead).
#[derive(Default)]
pub struct MallocMemorySnapshot {
    malloc: Vec<MallocMemory>,
    all_mallocs: MemoryCounter,
}

impl MallocMemorySnapshot {
    pub fn new() -> Self {
        Self {
            malloc: (0..MemTag::MtNumberOfTags as usize).map(|_| MallocMemory::new()).collect(),
            all_mallocs: MemoryCounter::new(),
        }
    }

    pub fn by_tag(&self, tag: MemTag) -> &MallocMemory {
        &self.malloc[NmtUtil::tag_to_index(tag)]
    }

    pub fn by_tag_mut(&mut self, tag: MemTag) -> &mut MallocMemory {
        &mut self.malloc[NmtUtil::tag_to_index(tag)]
    }

    /// Memory used by malloc tracking headers (18 bytes per live malloc:
    /// 16-byte header + 2-byte footer canary).
    pub fn malloc_overhead(&self) -> usize {
        self.all_mallocs.count() * MallocHeader::malloc_overhead()
    }

    /// Total malloc invocation count.
    pub fn total_count(&self) -> usize {
        self.all_mallocs.count()
    }

    /// Total malloc'd memory amount (incl. tracking overhead and arenas).
    pub fn total(&self) -> usize {
        self.all_mallocs.size() + self.malloc_overhead() + self.total_arena()
    }

    /// Total peak malloc.
    pub fn total_peak(&self) -> usize {
        self.all_mallocs.peak_size()
    }

    /// Total peak count.
    pub fn total_peak_count(&self) -> usize {
        self.all_mallocs.peak_count()
    }

    /// Total malloc'd memory used by arenas.
    pub fn total_arena(&self) -> usize {
        self.malloc.iter().map(|m| m.arena_size()).sum()
    }

    /// Make adjustment by subtracting chunks used by arenas from total
    /// chunks to get total free chunk size.
    pub fn make_adjustment(&mut self) {
        let arena_size = self.total_arena();
        if arena_size == 0 {
            return;
        }
        let chunk_idx = NmtUtil::tag_to_index(MemTag::MtChunk);
        self.malloc[chunk_idx].record_free(arena_size);
        self.all_mallocs.deallocate(arena_size);
    }
}

/// Malloc tracking header — in HotSpot this is embedded in front of the
/// user allocation; here it is the handle returned by record_malloc and
/// passed back to deaccount the block (MallocHeader::FreeInfo).
#[derive(Debug, Clone, Copy)]
pub struct MallocHeader {
    size: usize,
    mem_tag: MemTag,
    mst_marker: u32,
    live: bool,
}

/// All data needed to deaccount a block (MallocHeader::FreeInfo).
#[derive(Debug, Clone, Copy)]
pub struct FreeInfo {
    pub size: usize,
    pub mem_tag: MemTag,
    pub mst_marker: u32,
}

impl MallocHeader {
    /// Per-allocation overhead: 16-byte header + 2-byte footer canary.
    pub const MALLOC_OVERHEAD: usize = 18;

    pub fn new(size: usize, mem_tag: MemTag, mst_marker: u32) -> Self {
        Self { size, mem_tag, mst_marker, live: true }
    }

    #[inline]
    pub fn malloc_overhead() -> usize {
        Self::MALLOC_OVERHEAD
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    #[inline]
    pub fn mem_tag(&self) -> MemTag {
        self.mem_tag
    }

    #[inline]
    pub fn mst_marker(&self) -> u32 {
        self.mst_marker
    }

    /// Return the data needed to deaccount the block with NMT.
    pub fn free_info(&self) -> FreeInfo {
        FreeInfo { size: self.size, mem_tag: self.mem_tag, mst_marker: self.mst_marker }
    }

    #[inline]
    pub fn mark_block_as_dead(&mut self) {
        self.live = false;
    }

    #[inline]
    pub fn revive(&mut self) {
        self.live = true;
    }

    #[inline]
    pub fn is_dead(&self) -> bool {
        !self.live
    }

    #[inline]
    pub fn is_live(&self) -> bool {
        self.live
    }
}

/// MallocMemorySummary — collects malloc statistics at summary level.
/// Mirrors the C++ AllStatic class backed by a global snapshot.
pub struct MallocMemorySummary {
    snapshot: Mutex<MallocMemorySnapshot>,
}

static GLOBAL_SUMMARY: OnceLock<MallocMemorySummary> = OnceLock::new();

impl MallocMemorySummary {
    fn new() -> Self {
        Self { snapshot: Mutex::new(MallocMemorySnapshot::new()) }
    }

    pub fn global() -> &'static Self {
        GLOBAL_SUMMARY.get_or_init(Self::new)
    }

    pub fn initialize() {
        let _ = Self::global();
    }

    /// Copy the current snapshot; applies the arena adjustment.
    pub fn snapshot(&self) -> MallocMemorySnapshot {
        let src = self.snapshot.lock().unwrap();
        let mut s = Self::clone_snapshot(&src);
        s.make_adjustment();
        s
    }

    fn clone_snapshot(src: &MallocMemorySnapshot) -> MallocMemorySnapshot {
        let mut s = MallocMemorySnapshot::new();
        for tag in MemTag::all() {
            let sm = src.by_tag(tag);
            let dm = s.by_tag_mut(tag);
            copy_counter(sm.malloc_counter(), dm.malloc_counter());
            copy_counter(sm.arena_counter(), dm.arena_counter());
        }
        copy_counter(&src.all_mallocs_counter(), s.all_mallocs_counter_mut());
        s
    }

    pub fn record_malloc(&self, size: usize, mem_tag: MemTag) {
        let mut snap = self.snapshot.lock().unwrap();
        snap.by_tag(mem_tag).record_malloc(size);
        snap.all_mallocs_mut().allocate(size);
    }

    pub fn record_free(&self, size: usize, mem_tag: MemTag) {
        let mut snap = self.snapshot.lock().unwrap();
        snap.by_tag(mem_tag).record_free(size);
        snap.all_mallocs_mut().deallocate(size);
    }

    pub fn record_new_arena(&self, mem_tag: MemTag) {
        self.snapshot.lock().unwrap().by_tag(mem_tag).record_new_arena();
    }

    pub fn record_arena_free(&self, mem_tag: MemTag) {
        self.snapshot.lock().unwrap().by_tag(mem_tag).record_arena_free();
    }

    pub fn record_arena_size_change(&self, size: isize, mem_tag: MemTag) {
        self.snapshot.lock().unwrap().by_tag(mem_tag).record_arena_size_change(size);
    }

    /// Reset all counters (tests / shutdown).
    pub fn reset(&self) {
        let mut snap = self.snapshot.lock().unwrap();
        for tag in MemTag::all() {
            let m = snap.by_tag_mut(tag);
            m.malloc_counter().reset();
            m.arena_counter().reset();
        }
        snap.all_mallocs_mut().reset();
    }

    /// MallocLimit: returns true if allocating s bytes under mem_tag would
    /// trigger either the global or the category limit.
    pub fn check_exceeds_limit(&self, s: usize, mem_tag: MemTag) -> bool {
        if !MallocLimitHandler::have_limit() {
            return false;
        }
        let snap = self.snapshot();
        let so_far = snap.total();
        if let Some(limit) = MallocLimitHandler::global_limit() {
            if limit.sz > 0 && so_far + s > limit.sz {
                return true;
            }
        }
        if let Some(limit) = MallocLimitHandler::category_limit(mem_tag) {
            if limit.sz > 0 {
                let tag_size = snap.by_tag(mem_tag).malloc_size() + snap.by_tag(mem_tag).arena_size();
                if tag_size + s > limit.sz {
                    return true;
                }
            }
        }
        false
    }
}

fn copy_counter(src: &MemoryCounter, dst: &MemoryCounter) {
    dst.set_size_and_count(src.size(), src.count());
    // Restore peaks exactly (set_size_and_count already updates peak to at
    // least current; store the recorded peaks afterwards).
    dst.peak_size_store(src.peak_size());
    dst.peak_count_store(src.peak_count());
}

impl MallocMemorySnapshot {
    fn all_mallocs_counter(&self) -> &MemoryCounter {
        &self.all_mallocs
    }

    fn all_mallocs_mut(&mut self) -> &mut MemoryCounter {
        &mut self.all_mallocs
    }

    fn all_mallocs_counter_mut(&mut self) -> &mut MemoryCounter {
        &mut self.all_mallocs
    }
}

impl MemoryCounter {
    fn peak_size_store(&self, v: usize) {
        self.peak_size.store(v, Ordering::Relaxed);
    }

    fn peak_count_store(&self, v: usize) {
        self.peak_count.store(v, Ordering::Relaxed);
    }

    fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
        self.size.store(0, Ordering::Relaxed);
        self.peak_count.store(0, Ordering::Relaxed);
        self.peak_size.store(0, Ordering::Relaxed);
    }
}

/// Main class called from MemTracker to track malloc activities.
pub struct MallocTracker;

impl MallocTracker {
    /// Initialize malloc tracker for a specific tracking level.
    /// Returns false on allocation failure (never in this port).
    pub fn initialize(level: NmtTrackingLevel) -> bool {
        if level >= NmtTrackingLevel::Summary {
            MallocMemorySummary::initialize();
            let _ = MallocLimitHandler::initialize(std::env::var("JVMLIB_MALLOC_LIMIT").ok().as_deref());
        }
        if level == NmtTrackingLevel::Detail {
            return MallocSiteTable::initialize();
        }
        true
    }

    /// Per-malloc overhead incurred by NMT.
    #[inline]
    pub fn overhead_per_malloc() -> usize {
        MallocHeader::malloc_overhead()
    }

    /// Record a malloc. Returns the MallocHeader handle which must be
    /// passed to record_free_block/deaccount when the block is freed.
    pub fn record_malloc(size: usize, mem_tag: MemTag, stack: &NativeCallStack) -> MallocHeader {
        debug_assert!(mem_tag != MemTag::MtNumberOfTags, "precondition");
        MallocMemorySummary::global().record_malloc(size, mem_tag);
        let mut mst_marker = 0u32;
        if crate::share::nmt::memTracker::MemTracker::tracking_level() == NmtTrackingLevel::Detail {
            mst_marker = MallocSiteTable::allocation_at(stack, size, mem_tag);
        }
        MallocHeader::new(size, mem_tag, mst_marker)
    }

    /// Record malloc free and return the deaccount data.
    pub fn record_free_block(header: &MallocHeader) -> FreeInfo {
        let info = header.free_info();
        MallocTracker::deaccount(info);
        info
    }

    /// Given the free info from a block, de-account it from NMT.
    pub fn deaccount(free_info: FreeInfo) {
        MallocMemorySummary::global().record_free(free_info.size, free_info.mem_tag);
        if crate::share::nmt::memTracker::MemTracker::tracking_level() == NmtTrackingLevel::Detail {
            MallocSiteTable::deallocation_at(free_info.size, free_info.mst_marker);
        }
    }

    #[inline]
    pub fn record_new_arena(mem_tag: MemTag) {
        MallocMemorySummary::global().record_new_arena(mem_tag);
    }

    #[inline]
    pub fn record_arena_free(mem_tag: MemTag) {
        MallocMemorySummary::global().record_arena_free(mem_tag);
    }

    #[inline]
    pub fn record_arena_size_change(size: isize, mem_tag: MemTag) {
        MallocMemorySummary::global().record_arena_size_change(size, mem_tag);
    }

    /// MallocLimit: returns true if allocating s bytes for mem_tag would
    /// hit either the global or the category limit.
    #[inline]
    pub fn check_exceeds_limit(s: usize, mem_tag: MemTag) -> bool {
        MallocMemorySummary::global().check_exceeds_limit(s, mem_tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_peaks() {
        let c = MemoryCounter::new();
        c.allocate(100);
        c.allocate(200);
        assert_eq!(c.size(), 300);
        assert_eq!(c.count(), 2);
        assert_eq!(c.peak_size(), 300);
        c.deallocate(300);
        assert_eq!(c.size(), 0);
        assert_eq!(c.peak_size(), 300);
    }

    #[test]
    fn summary_recording() {
        let _g = crate::share::nmt::nmtCommon::NMT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let s = MallocMemorySummary::global();
        s.record_malloc(64, MemTag::MtTest);
        let snap = s.snapshot();
        assert!(snap.by_tag(MemTag::MtTest).malloc_size() >= 64);
        s.record_free(64, MemTag::MtTest);
    }

    #[test]
    fn header_free_info() {
        let h = MallocHeader::new(128, MemTag::MtSymbol, 0x1234);
        let fi = h.free_info();
        assert_eq!(fi.size, 128);
        assert_eq!(fi.mem_tag, MemTag::MtSymbol);
        assert_eq!(fi.mst_marker, 0x1234);
        assert_eq!(MallocHeader::malloc_overhead(), 18);
    }
}
