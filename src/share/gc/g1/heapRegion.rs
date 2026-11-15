/*
 * Port of OpenJDK 25 HotSpot G1HeapRegionType + G1HeapRegionAttr
 * (share/gc/g1/g1HeapRegionType.hpp, g1HeapRegionAttr.hpp)
 * and HeapRegion (heapRegion.hpp).
 *
 * Regions are fixed-size partitions of the Java heap. The region type
 * is tag-encoded exactly like HotSpot: the high bits identify the
 * generation (young / humongous / old masks) and the low bit selects
 * the minor type (eden/survivor, starts/continues humongous).
 *
 * G1HeapRegionAttr is the per-GC attribute table entry: for each region
 * it records whether the region is in the current collection set
 * (CSet), whether it is a humongous candidate, new survivor, optional,
 * or pinned, and whether its remembered set is tracked. Values mirror
 * the HotSpot constants so selection logic ports directly.
 */

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

/// Tag values identical to `G1HeapRegionType::Tag` in
/// g1HeapRegionType.hpp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum G1HeapRegionType {
    Free = 0,
    Eden = 2,
    Survivor = 3,
    StartsHumongous = 4,
    ContinuesHumongous = 5,
    Old = 8,
}

impl G1HeapRegionType {
    pub const YOUNG_MASK: i32 = 2;
    pub const HUMONGOUS_MASK: i32 = 4;
    pub const OLD_MASK: i32 = 8;

    pub const fn from_tag(tag: i32) -> Self {
        match tag {
            0 => Self::Free,
            2 => Self::Eden,
            3 => Self::Survivor,
            4 => Self::StartsHumongous,
            5 => Self::ContinuesHumongous,
            8 => Self::Old,
            _ => Self::Free,
        }
    }

    #[inline]
    pub const fn tag(self) -> i32 {
        self as i32
    }

    #[inline]
    pub const fn is_free(self) -> bool {
        matches!(self, Self::Free)
    }

    #[inline]
    pub const fn is_young(self) -> bool {
        matches!(self, Self::Eden | Self::Survivor)
    }

    #[inline]
    pub const fn is_eden(self) -> bool {
        matches!(self, Self::Eden)
    }

    #[inline]
    pub const fn is_survivor(self) -> bool {
        matches!(self, Self::Survivor)
    }

    #[inline]
    pub const fn is_humongous(self) -> bool {
        matches!(self, Self::StartsHumongous | Self::ContinuesHumongous)
    }

    #[inline]
    pub const fn is_starts_humongous(self) -> bool {
        matches!(self, Self::StartsHumongous)
    }

    #[inline]
    pub const fn is_continues_humongous(self) -> bool {
        matches!(self, Self::ContinuesHumongous)
    }

    #[inline]
    pub const fn is_old(self) -> bool {
        matches!(self, Self::Old)
    }

    #[inline]
    pub const fn is_old_or_humongous(self) -> bool {
        matches!(self, Self::Old | Self::StartsHumongous | Self::ContinuesHumongous)
    }

    /// `same_essentials_as` in g1HeapRegionType.hpp: regions of the same
    /// generation and minor kind can be merged when iterating.
    #[inline]
    pub const fn same_essentials_as(self, other: Self) -> bool {
        self as i32 == other as i32
    }
}

/// Per-GC region attributes, mirroring `G1HeapRegionAttr`. The `type`
/// field uses the same value encoding as HotSpot so the same branches
/// (in collection set? humongous candidate? new survivor?) apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum G1HeapRegionAttrType {
    Optional = -4,
    HumongousCandidate = -3,
    NewSurvivor = -2,
    NotInCSet = -1,
    Young = 0,
    Old = 1,
}

impl G1HeapRegionAttrType {
    pub const NUM: i8 = 2;

    #[inline]
    pub const fn is_in_cset_or_humongous_candidate(self) -> bool {
        (self as i8) >= G1HeapRegionAttrType::Young as i8
            || matches!(self, Self::HumongousCandidate)
    }

    #[inline]
    pub const fn is_in_cset(self) -> bool {
        (self as i8) >= G1HeapRegionAttrType::Young as i8
    }

    #[inline]
    pub const fn is_humongous_candidate(self) -> bool {
        matches!(self, Self::HumongousCandidate)
    }

    #[inline]
    pub const fn is_new_survivor(self) -> bool {
        matches!(self, Self::NewSurvivor)
    }

    #[inline]
    pub const fn is_young(self) -> bool {
        matches!(self, Self::Young)
    }

    #[inline]
    pub const fn is_old(self) -> bool {
        matches!(self, Self::Old)
    }

    #[inline]
    pub const fn is_optional(self) -> bool {
        matches!(self, Self::Optional)
    }

    #[inline]
    pub const fn is_default(self) -> bool {
        matches!(self, Self::NotInCSet)
    }

    #[inline]
    pub const fn is_valid(self) -> bool {
        (self as i8) >= G1HeapRegionAttrType::Optional as i8 && (self as i8) < Self::NUM
    }
}

#[derive(Debug, Clone, Copy)]
pub struct G1HeapRegionAttr {
    remset_is_tracked: bool,
    region_type: G1HeapRegionAttrType,
    is_pinned: bool,
}

impl Default for G1HeapRegionAttr {
    fn default() -> Self {
        Self::new()
    }
}

impl G1HeapRegionAttr {
    #[inline]
    pub const fn new() -> Self {
        Self {
            remset_is_tracked: false,
            region_type: G1HeapRegionAttrType::NotInCSet,
            is_pinned: false,
        }
    }

    pub const fn with(region_type: G1HeapRegionAttrType, remset_is_tracked: bool, is_pinned: bool) -> Self {
        Self { remset_is_tracked, region_type, is_pinned }
    }

    #[inline]
    pub const fn region_type(&self) -> G1HeapRegionAttrType {
        self.region_type
    }

    #[inline]
    pub const fn remset_is_tracked(&self) -> bool {
        self.remset_is_tracked
    }

    #[inline]
    pub const fn is_pinned(&self) -> bool {
        self.is_pinned
    }

    #[inline]
    pub fn is_in_cset(&self) -> bool {
        self.region_type.is_in_cset()
    }

    #[inline]
    pub fn is_in_cset_or_humongous_candidate(&self) -> bool {
        self.region_type.is_in_cset_or_humongous_candidate()
    }

    #[inline]
    pub fn is_humongous_candidate(&self) -> bool {
        self.region_type.is_humongous_candidate()
    }

    #[inline]
    pub fn is_new_survivor(&self) -> bool {
        self.region_type.is_new_survivor()
    }

    #[inline]
    pub fn is_young(&self) -> bool {
        self.region_type.is_young()
    }

    #[inline]
    pub fn is_old(&self) -> bool {
        self.region_type.is_old()
    }

    #[inline]
    pub fn is_optional(&self) -> bool {
        self.region_type.is_optional()
    }

    #[inline]
    pub fn is_default(&self) -> bool {
        self.region_type.is_default()
    }

    pub fn set_type(&mut self, region_type: G1HeapRegionAttrType) {
        debug_assert!(region_type.is_valid());
        self.region_type = region_type;
    }

    pub fn set_new_survivor(&mut self) {
        self.region_type = G1HeapRegionAttrType::NewSurvivor;
    }

    pub fn set_old(&mut self) {
        self.region_type = G1HeapRegionAttrType::Old;
    }

    pub fn set_young(&mut self) {
        self.region_type = G1HeapRegionAttrType::Young;
    }

    pub fn set_not_in_cset(&mut self) {
        self.region_type = G1HeapRegionAttrType::NotInCSet;
    }

    pub fn clear_humongous_candidate(&mut self) {
        assert!(self.is_humongous_candidate() || !self.is_in_cset());
        self.region_type = G1HeapRegionAttrType::NotInCSet;
    }

    pub fn set_optional(&mut self, remset_is_tracked: bool) {
        debug_assert!(self.is_default());
        self.region_type = G1HeapRegionAttrType::Optional;
        self.remset_is_tracked = remset_is_tracked;
    }

    pub fn set_humongous_candidate(&mut self) {
        debug_assert!(self.is_default());
        self.region_type = G1HeapRegionAttrType::HumongousCandidate;
        self.remset_is_tracked = true;
        self.is_pinned = false;
    }

    pub fn set_remset_is_tracked(&mut self, value: bool) {
        self.remset_is_tracked = value;
    }

    pub fn set_is_pinned(&mut self, value: bool) {
        self.is_pinned = value;
    }

    pub fn type_str(&self) -> &'static str {
        match self.region_type {
            G1HeapRegionAttrType::Optional => "Optional",
            G1HeapRegionAttrType::HumongousCandidate => "HumongousCandidate",
            G1HeapRegionAttrType::NewSurvivor => "NewSurvivor",
            G1HeapRegionAttrType::NotInCSet => "NotInCSet",
            G1HeapRegionAttrType::Young => "Young",
            G1HeapRegionAttrType::Old => "Old",
        }
    }
}

/// Region GC state, mirroring `G1HeapRegion::GCState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum G1GCState {
 /// The region has not been linked into the heap.
 Unlinked,
 /// The region is tracked by the remembered set.
 Tracked,
 /// The region is in the collection set.
 InCollectionSet,
 /// The region has been retired (e.g. an optional region that was
 /// not used during the collection).
 Retired,
}

/// Region-level counters for SATB/marking, kept per region so
/// statistics and dirty-card tracking can be queried cheaply.
#[derive(Debug, Default)]
pub struct HeapRegionMarking {
    /// Bytes marked at TAMS (top-at-mark-start) boundaries.
    pub live_bytes_at_tams: usize,
    /// Bytes marked between bottom and TAMS.
    pub live_bytes_at_bottom: usize,
    /// Estimated incoming references into this region.
    pub incoming_refs: usize,
}

/// HeapRegion: fixed-size region of the Java heap.
///
/// Fields mirror `G1HeapRegion`: `_bottom`, `_top`, `_end`,
/// `_parsable_bottom`, `_humongous_start_region` (as region index),
/// `_live_bytes`, `_type`, `_gc_state`, `_remset`, `_attr`.
///
/// Object addresses are byte offsets inside the region's
/// `[bottom, end)` window, consistent with how `memory/heap.rs`
/// treats `usize` handles.
#[derive(Debug)]
pub struct HeapRegion {
    index: usize,
    bottom: usize,
    top: AtomicUsize,
    end: usize,
    region_type: G1HeapRegionType,
    attr: G1HeapRegionAttr,
    gc_state: G1GCState,
    live_bytes: AtomicUsize,
    mark_words: AtomicUsize,
    incoming_refs: usize,
    /// For humongous continuation regions: index of the region where
    /// the object starts; for starts-humongous: index of the last
    /// region the object occupies. `usize::MAX` = not humongous.
    humongous_start_index: usize,
    humongous_end_index: usize,
    /// Number of bytes of the humongous object starting in this
    /// region (starts-humongous only).
    humongous_size: usize,
    /// TAMS (top-at-mark-start) boundary as a byte offset from bottom.
    tams: usize,
    /// Parsable bottom: bottom-most address from which objects can
    /// be iterated after compaction.
    parsable_bottom: usize,
    /// True while the region participates in incremental evacuation.
    evacuation_candidate: bool,
    /// Free-list / reclaim tracking for optional regions.
    retired: bool,
}

impl HeapRegion {
    /// HotSpot default region size for 4GB+ heaps (power of two,
    /// between 1MB and 32MB).
    pub const DEFAULT_REGION_SIZE: usize = 1024 * 1024;

    pub fn new(index: usize, bottom: usize, size: usize) -> Self {
        Self {
            index,
            bottom,
            top: AtomicUsize::new(bottom),
            end: bottom + size,
            region_type: G1HeapRegionType::Free,
            attr: G1HeapRegionAttr::new(),
            gc_state: G1GCState::Unlinked,
            live_bytes: AtomicUsize::new(0),
            mark_words: AtomicUsize::new(0),
            incoming_refs: 0,
            humongous_start_index: usize::MAX,
            humongous_end_index: usize::MAX,
            humongous_size: 0,
            tams: 0,
            parsable_bottom: 0,
            evacuation_candidate: false,
            retired: false,
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
        self.top.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_top(&self, value: usize) {
        self.top.store(value, Ordering::Release);
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
        self.top().saturating_sub(self.bottom)
    }

    #[inline]
    pub fn free_space(&self) -> usize {
        self.end.saturating_sub(self.top())
    }

    #[inline]
    pub fn remaining_in_bytes(&self) -> usize {
        self.end - self.top()
    }

    #[inline]
    pub fn is_in_reserved(&self, p: usize) -> bool {
        self.bottom <= p && p < self.end
    }

    #[inline]
    pub fn contains(&self, p: usize) -> bool {
        self.is_in_reserved(p)
    }

    /// `young_index_in_cset`-style queries: whether an address falls
    /// within the young part of the collection set.
    #[inline]
    pub fn region_type(&self) -> G1HeapRegionType {
        self.region_type
    }

    #[inline]
    pub fn set_region_type(&mut self, t: G1HeapRegionType) {
        self.region_type = t;
    }

    #[inline]
    pub fn is_free(&self) -> bool {
        self.region_type.is_free()
    }

    #[inline]
    pub fn is_eden(&self) -> bool {
        self.region_type.is_eden()
    }

    #[inline]
    pub fn is_survivor(&self) -> bool {
        self.region_type.is_survivor()
    }

    #[inline]
    pub fn is_young(&self) -> bool {
        self.region_type.is_young()
    }

    #[inline]
    pub fn is_old(&self) -> bool {
        self.region_type.is_old()
    }

    #[inline]
    pub fn is_old_or_humongous(&self) -> bool {
        self.region_type.is_old_or_humongous()
    }

    #[inline]
    pub fn is_humongous(&self) -> bool {
        self.region_type.is_humongous()
    }

    #[inline]
    pub fn is_starts_humongous(&self) -> bool {
        self.region_type.is_starts_humongous()
    }

    #[inline]
    pub fn is_continues_humongous(&self) -> bool {
        self.region_type.is_continues_humongous()
    }

    #[inline]
    pub fn attr(&self) -> G1HeapRegionAttr {
        self.attr
    }

    #[inline]
    pub fn attr_mut(&mut self) -> &mut G1HeapRegionAttr {
        &mut self.attr
    }

    #[inline]
    pub fn gc_state(&self) -> G1GCState {
        self.gc_state
    }

    #[inline]
    pub fn set_gc_state(&mut self, state: G1GCState) {
        self.gc_state = state;
    }

    #[inline]
    pub fn is_unlinked(&self) -> bool {
        self.gc_state == G1GCState::Unlinked
    }

    #[inline]
    pub fn is_tracked(&self) -> bool {
        self.gc_state == G1GCState::Tracked
    }

    #[inline]
    pub fn is_in_collection_set(&self) -> bool {
        self.gc_state == G1GCState::InCollectionSet
    }

    #[inline]
    pub fn is_retired(&self) -> bool {
        self.retired
    }

    // -- humongous ----------------------------------------------------

    /// `set_starts_humongous`: region holds the start of a humongous
    /// object of `humongous_size` bytes spanning `end_index`.
    pub fn set_starts_humongous(&mut self, end_index: usize, humongous_size: usize) {
        self.region_type = G1HeapRegionType::StartsHumongous;
        self.humongous_start_index = self.index;
        self.humongous_end_index = end_index;
        self.humongous_size = humongous_size;
    }

    pub fn set_continues_humongous(&mut self, start_index: usize) {
        self.region_type = G1HeapRegionType::ContinuesHumongous;
        self.humongous_start_index = start_index;
    }

    pub fn unset_humongous(&mut self) {
        self.region_type = G1HeapRegionType::Free;
        self.humongous_start_index = usize::MAX;
        self.humongous_end_index = usize::MAX;
        self.humongous_size = 0;
    }

    #[inline]
    pub fn humongous_start_region_index(&self) -> usize {
        self.humongous_start_index
    }

    #[inline]
    pub fn humongous_end_region_index(&self) -> usize {
        self.humongous_end_index
    }

    #[inline]
    pub fn humongous_size(&self) -> usize {
        self.humongous_size
    }

    // -- live bytes / marking -----------------------------------------

    #[inline]
    pub fn live_bytes(&self) -> usize {
        self.live_bytes.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_live_bytes(&self, value: usize) {
        self.live_bytes.store(value.min(self.used()), Ordering::Release);
    }

    #[inline]
    pub fn add_live_bytes(&self, delta: usize) {
        let _ = self.live_bytes.fetch_add(delta, Ordering::AcqRel);
    }

    #[inline]
    pub fn garbage_bytes(&self) -> usize {
        self.used().saturating_sub(self.live_bytes())
    }

    #[inline]
    pub fn capacity_free(&self) -> usize {
        self.capacity().saturating_sub(self.live_bytes())
    }

    /// `collection_efficiency`: reclaimed bytes per allocated byte.
    /// Used by collection-set candidate ranking.
    #[inline]
    pub fn collection_efficiency(&self) -> f64 {
        let used = self.used();
        if used == 0 {
            return 0.0;
        }
        self.garbage_bytes() as f64 / used as f64
    }

    /// `note_start_of_marking`/`note_end_of_marking` pair.
    pub fn note_start_of_marking(&mut self, top_at_mark_start: usize) {
        debug_assert!(top_at_mark_start <= self.top());
        self.tams = top_at_mark_start - self.bottom;
    }

    pub fn note_end_of_marking(&mut self, live_bytes: usize, incoming_refs: usize) {
        self.set_live_bytes(live_bytes);
        self.incoming_refs = incoming_refs;
        self.mark_words.store(self.live_bytes() / 8, Ordering::Release);
    }

    #[inline]
    pub fn tams(&self) -> usize {
        self.bottom + self.tams
    }

    #[inline]
    pub fn top_at_mark_start(&self) -> usize {
        self.tams()
    }

    #[inline]
    pub fn parsable_bottom(&self) -> usize {
        self.bottom + self.parsable_bottom
    }

    pub fn set_parsable_bottom(&mut self, bottom: usize) {
        debug_assert!(self.is_in_reserved(bottom) && bottom <= self.top());
        self.parsable_bottom = bottom - self.bottom;
    }

    pub fn reset_parsable_bottom(&mut self) {
        self.parsable_bottom = 0;
    }

    #[inline]
    pub fn incoming_refs(&self) -> usize {
        self.incoming_refs
    }

    #[inline]
    pub fn mark_words(&self) -> usize {
        self.mark_words.load(Ordering::Acquire)
    }

    #[inline]
    pub fn is_evacuation_candidate(&self) -> bool {
        self.evacuation_candidate
    }

    pub fn set_evacuation_candidate(&mut self, value: bool) {
        self.evacuation_candidate = value;
    }

    pub fn set_retired(&mut self, value: bool) {
        self.retired = value;
    }

    /// `reset_after_full_gc` / `reset_after_young_gc`-style reset:
    /// empties the region and returns it to the free state.
    pub fn reset(&mut self) {
        self.set_top(self.bottom);
        self.region_type = G1HeapRegionType::Free;
        self.attr = G1HeapRegionAttr::new();
        self.gc_state = G1GCState::Unlinked;
        self.live_bytes.store(0, Ordering::Release);
        self.mark_words.store(0, Ordering::Release);
        self.incoming_refs = 0;
        self.unset_humongous();
        self.tams = 0;
        self.reset_parsable_bottom();
        self.evacuation_candidate = false;
        self.retired = false;
    }

    /// `reset_after_compaction` used by full GC: keeps type but
    /// re-establishes top and clears live bytes.
    pub fn reset_compacted_after_full_gc(&mut self, new_top: usize) {
        debug_assert!(self.bottom <= new_top && new_top <= self.end);
        self.set_top(new_top);
        self.live_bytes.store(0, Ordering::Release);
        self.parsable_bottom = 0;
        self.tams = 0;
        self.incoming_refs = 0;
    }

    /// `allocate` bump-pointer allocation within the region.
    pub fn allocate(&self, size: usize) -> Option<usize> {
        let top = self.top();
        if top + size <= self.end {
            self.set_top(top + size);
            Some(top)
        } else {
            None
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self.region_type {
            G1HeapRegionType::Free => "Free",
            G1HeapRegionType::Eden => "Eden",
            G1HeapRegionType::Survivor => "Survivor",
            G1HeapRegionType::StartsHumongous => "StartsHumongous",
            G1HeapRegionType::ContinuesHumongous => "ContinuesHumongous",
            G1HeapRegionType::Old => "Old",
        }
    }
}

/// Lookup interface used by remembered-set scanning and parallel
/// marking to map an address to its region. `memory/heap.rs` can
/// implement this over its region array later.
pub trait HeapRegionProvider {
    fn region_size(&self) -> usize;
    fn num_regions(&self) -> usize;
    fn region_index_of(&self, addr: usize) -> usize {
        addr / self.region_size().max(1)
    }
    fn region(&self, index: usize) -> Option<&HeapRegion>;
    fn region_mut(&mut self, index: usize) -> Option<&mut HeapRegion>;
}

impl HeapRegionProvider for Vec<HeapRegion> {
    fn region_size(&self) -> usize {
        self.first().map(|r| r.size()).unwrap_or(0)
    }

    fn num_regions(&self) -> usize {
        self.len()
    }

    fn region(&self, index: usize) -> Option<&HeapRegion> {
        self.get(index)
    }

    fn region_mut(&mut self, index: usize) -> Option<&mut HeapRegion> {
        self.get_mut(index)
    }
}
