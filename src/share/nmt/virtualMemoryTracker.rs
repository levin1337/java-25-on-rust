/*
 * Port of OpenJDK 25 HotSpot VirtualMemoryTracker
 * (share/nmt/virtualMemoryTracker.hpp/.cpp)
 *
 * Tracks reserved and committed virtual memory per MemTag. The C++ code
 * keeps a sorted linked list of ReservedMemoryRegion, each holding a sorted
 * list of CommittedMemoryRegion sub-regions; here the same model is kept
 * with BTreeMap (ordered by base address — regions never overlap, so base
 * order equals end order).
 *
 * All public entry points mirror MemTracker::record_virtual_memory_*:
 *   add_reserved_region / add_committed_region /
 *   remove_uncommitted_region / remove_released_region /
 *   set_reserved_region_type / split_reserved_region /
 *   walk_virtual_memory / print_containing_region /
 *   snapshot_thread_stacks
 */

use super::memTag::MemTag;
use super::nativeCallStack::NativeCallStack;
use super::nmtCommon::{NmtTrackingLevel, NmtUtil};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// VirtualMemory — reserved/committed counters with peak tracking
// ---------------------------------------------------------------------------

pub struct VirtualMemory {
    reserved: AtomicUsize,
    committed: AtomicUsize,
    peak_size: AtomicUsize,
}

impl VirtualMemory {
    pub fn new() -> Self {
        Self {
            reserved: AtomicUsize::new(0),
            committed: AtomicUsize::new(0),
            peak_size: AtomicUsize::new(0),
        }
    }

    fn update_peak(&self, size: usize) {
        let mut peak = self.peak_size.load(Ordering::Relaxed);
        while peak < size {
            match self.peak_size.compare_exchange_weak(
                peak,
                size,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(old) => peak = old,
            }
        }
    }

    #[inline]
    pub fn reserve_memory(&self, sz: usize) {
        self.reserved.fetch_add(sz, Ordering::Relaxed);
    }

    #[inline]
    pub fn commit_memory(&self, sz: usize) {
        let c = self.committed.fetch_add(sz, Ordering::Relaxed) + sz;
        debug_assert!(c <= self.reserved.load(Ordering::Relaxed), "Sanity check");
        self.update_peak(c);
    }

    #[inline]
    pub fn release_memory(&self, sz: usize) {
        debug_assert!(self.reserved.load(Ordering::Relaxed) >= sz, "Negative amount");
        self.reserved.fetch_sub(sz, Ordering::Relaxed);
    }

    #[inline]
    pub fn uncommit_memory(&self, sz: usize) {
        debug_assert!(self.committed.load(Ordering::Relaxed) >= sz, "Negative amount");
        self.committed.fetch_sub(sz, Ordering::Relaxed);
    }

    #[inline]
    pub fn reserved(&self) -> usize {
        self.reserved.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn committed(&self) -> usize {
        self.committed.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn peak_size(&self) -> usize {
        self.peak_size.load(Ordering::Relaxed)
    }

    fn set_values(&self, reserved: usize, committed: usize, peak: usize) {
        self.reserved.store(reserved, Ordering::Relaxed);
        self.committed.store(committed, Ordering::Relaxed);
        self.peak_size.store(peak, Ordering::Relaxed);
    }

    fn reset(&self) {
        self.set_values(0, 0, 0);
    }
}

impl Default for VirtualMemory {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// VirtualMemoryAllocationSite — VM usage attributed to a call site
// ---------------------------------------------------------------------------

pub struct VirtualMemoryAllocationSite {
    stack: NativeCallStack,
    mem_tag: MemTag,
    counter: VirtualMemory,
}

impl VirtualMemoryAllocationSite {
    pub fn new(stack: NativeCallStack, mem_tag: MemTag) -> Self {
        Self { stack, mem_tag, counter: VirtualMemory::new() }
    }

    #[inline]
    pub fn reserve_memory(&self, sz: usize) {
        self.counter.reserve_memory(sz);
    }

    #[inline]
    pub fn commit_memory(&self, sz: usize) {
        self.counter.commit_memory(sz);
    }

    #[inline]
    pub fn reserved(&self) -> usize {
        self.counter.reserved()
    }

    #[inline]
    pub fn committed(&self) -> usize {
        self.counter.committed()
    }

    #[inline]
    pub fn peak_size(&self) -> usize {
        self.counter.peak_size()
    }

    pub fn call_stack(&self) -> &NativeCallStack {
        &self.stack
    }

    pub fn mem_tag(&self) -> MemTag {
        self.mem_tag
    }
}

// ---------------------------------------------------------------------------
// VirtualMemorySnapshot / VirtualMemorySummary
// ---------------------------------------------------------------------------

/// A snapshot of virtual memory at a given time, per MemTag.
pub struct VirtualMemorySnapshot {
    vm: Vec<VirtualMemory>,
}

impl VirtualMemorySnapshot {
    pub fn new() -> Self {
        Self { vm: (0..MemTag::MtNumberOfTags as usize).map(|_| VirtualMemory::new()).collect() }
    }

    pub fn by_tag(&self, tag: MemTag) -> &VirtualMemory {
        &self.vm[NmtUtil::tag_to_index(tag)]
    }

    pub fn by_tag_mut(&mut self, tag: MemTag) -> &mut VirtualMemory {
        &mut self.vm[NmtUtil::tag_to_index(tag)]
    }

    pub fn total_reserved(&self) -> usize {
        self.vm.iter().map(|v| v.reserved()).sum()
    }

    pub fn total_committed(&self) -> usize {
        self.vm.iter().map(|v| v.committed()).sum()
    }

    pub fn copy_to(&self, s: &mut VirtualMemorySnapshot) {
        for (dst, src) in s.vm.iter().zip(self.vm.iter()) {
            dst.set_values(src.reserved(), src.committed(), src.peak_size());
        }
    }
}

impl Default for VirtualMemorySnapshot {
    fn default() -> Self {
        Self::new()
    }
}

/// VirtualMemorySummary — the live per-tag totals, updated inline by the
/// tracker (C++ uses a static snapshot with atomic counters).
pub struct VirtualMemorySummary {
    snapshot: VirtualMemorySnapshot,
}

static GLOBAL_VM_SUMMARY: OnceLock<VirtualMemorySummary> = OnceLock::new();

impl VirtualMemorySummary {
    pub fn global() -> &'static Self {
        GLOBAL_VM_SUMMARY.get_or_init(|| Self { snapshot: VirtualMemorySnapshot::new() })
    }

    #[inline]
    pub fn record_reserved_memory(&self, size: usize, tag: MemTag) {
        self.snapshot.by_tag(tag).reserve_memory(size);
    }

    #[inline]
    pub fn record_committed_memory(&self, size: usize, tag: MemTag) {
        self.snapshot.by_tag(tag).commit_memory(size);
    }

    #[inline]
    pub fn record_uncommitted_memory(&self, size: usize, tag: MemTag) {
        self.snapshot.by_tag(tag).uncommit_memory(size);
    }

    #[inline]
    pub fn record_released_memory(&self, size: usize, tag: MemTag) {
        self.snapshot.by_tag(tag).release_memory(size);
    }

    /// Move virtual memory from one memory tag to another (used when a
    /// region reserved as mtNone is later tagged).
    #[inline]
    pub fn move_reserved_memory(&self, from: MemTag, to: MemTag, size: usize) {
        self.snapshot.by_tag(from).release_memory(size);
        self.snapshot.by_tag(to).reserve_memory(size);
    }

    #[inline]
    pub fn move_committed_memory(&self, from: MemTag, to: MemTag, size: usize) {
        self.snapshot.by_tag(from).uncommit_memory(size);
        self.snapshot.by_tag(to).commit_memory(size);
    }

    pub fn snapshot(&self, s: &mut VirtualMemorySnapshot) {
        self.snapshot.copy_to(s);
    }

    pub fn as_snapshot(&self) -> &VirtualMemorySnapshot {
        &self.snapshot
    }

    /// Reset all per-tag counters (tests / shutdown).
    pub fn reset(&self) {
        for tag in MemTag::all() {
            self.snapshot.by_tag(tag).reset();
        }
    }
}

// ---------------------------------------------------------------------------
// Region model
// ---------------------------------------------------------------------------

/// A virtual memory region [base, base+size).
#[derive(Clone, Copy, Debug)]
pub struct VirtualMemoryRegion {
    base: usize,
    size: usize,
}

impl VirtualMemoryRegion {
    pub fn new(base: usize, size: usize) -> Self {
        debug_assert!(base != 0, "Invalid address");
        debug_assert!(size > 0, "Invalid size");
        Self { base, size }
    }

    #[inline]
    pub fn base(&self) -> usize {
        self.base
    }

    #[inline]
    pub fn end(&self) -> usize {
        self.base.saturating_add(self.size)
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    #[inline]
    pub fn contain_address(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.end()
    }

    #[inline]
    pub fn contain_region(&self, addr: usize, size: usize) -> bool {
        self.contain_address(addr) && self.contain_address(addr + size - 1)
    }

    #[inline]
    pub fn same_region(&self, addr: usize, sz: usize) -> bool {
        addr == self.base && sz == self.size
    }

    #[inline]
    pub fn overlap_region(&self, addr: usize, sz: usize) -> bool {
        debug_assert!(sz > 0 && self.size > 0, "Invalid size");
        addr.max(self.base) < (addr + sz).min(self.end())
    }

    #[inline]
    pub fn adjacent_to(&self, addr: usize, sz: usize) -> bool {
        addr == self.end() || (addr + sz) == self.base
    }

    /// Remove [addr, addr+sz) from either edge of this region.
    fn exclude_region(&mut self, addr: usize, sz: usize) {
        debug_assert!(self.contain_region(addr, sz), "Not containment");
        debug_assert!(addr == self.base || addr + sz == self.end(), "Can not exclude from middle");
        let new_size = self.size - sz;
        if addr == self.base {
            self.base = addr + sz;
        }
        self.size = new_size;
    }

    /// Expand this region to include the adjacent [addr, addr+sz).
    fn expand_region(&mut self, addr: usize, sz: usize) {
        debug_assert!(self.adjacent_to(addr, sz), "Not adjacent regions");
        if self.base == addr + sz {
            self.base = addr;
        }
        self.size += sz;
    }
}

/// A committed sub-region inside a ReservedMemoryRegion.
#[derive(Clone, Debug)]
pub struct CommittedMemoryRegion {
    region: VirtualMemoryRegion,
    stack: NativeCallStack,
}

impl CommittedMemoryRegion {
    pub fn new(addr: usize, size: usize, stack: NativeCallStack) -> Self {
        Self { region: VirtualMemoryRegion::new(addr, size), stack }
    }

    #[inline]
    pub fn base(&self) -> usize {
        self.region.base()
    }

    #[inline]
    pub fn end(&self) -> usize {
        self.region.end()
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.region.size()
    }

    #[inline]
    pub fn call_stack(&self) -> &NativeCallStack {
        &self.stack
    }

    #[inline]
    fn set_call_stack(&mut self, stack: NativeCallStack) {
        self.stack = stack;
    }

    #[inline]
    fn contain_region(&self, addr: usize, size: usize) -> bool {
        self.region.contain_region(addr, size)
    }

    #[inline]
    fn contain_address(&self, addr: usize) -> bool {
        self.region.contain_address(addr)
    }

    #[inline]
    fn same_region(&self, addr: usize, size: usize) -> bool {
        self.region.same_region(addr, size)
    }

    #[inline]
    fn overlap_region(&self, addr: usize, size: usize) -> bool {
        self.region.overlap_region(addr, size)
    }

    #[inline]
    fn adjacent_to(&self, addr: usize, size: usize) -> bool {
        self.region.adjacent_to(addr, size)
    }

    fn exclude_region(&mut self, addr: usize, sz: usize) {
        self.region.exclude_region(addr, sz);
    }

    fn expand_region(&mut self, addr: usize, sz: usize) {
        self.region.expand_region(addr, sz);
    }
}

/// A reserved memory region with its committed sub-regions.
#[derive(Clone, Debug)]
pub struct ReservedMemoryRegion {
    region: VirtualMemoryRegion,
    stack: NativeCallStack,
    mem_tag: MemTag,
    /// Committed sub-regions keyed by base address (sorted, non-overlapping).
    committed: BTreeMap<usize, CommittedMemoryRegion>,
}

impl ReservedMemoryRegion {
    pub fn new(base: usize, size: usize, stack: NativeCallStack, mem_tag: MemTag) -> Self {
        Self {
            region: VirtualMemoryRegion::new(base, size),
            stack,
            mem_tag,
            committed: BTreeMap::new(),
        }
    }

    /// Region used for lookups without stack/tag (C++ second ctor).
    pub fn lookup_region(base: usize, size: usize) -> Self {
        Self {
            region: VirtualMemoryRegion::new(base, size),
            stack: NativeCallStack::empty(),
            mem_tag: MemTag::MtNone,
            committed: BTreeMap::new(),
        }
    }

    #[inline]
    pub fn base(&self) -> usize {
        self.region.base()
    }

    #[inline]
    pub fn end(&self) -> usize {
        self.region.end()
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.region.size()
    }

    #[inline]
    pub fn call_stack(&self) -> &NativeCallStack {
        &self.stack
    }

    #[inline]
    pub fn set_call_stack(&mut self, stack: NativeCallStack) {
        self.stack = stack;
    }

    #[inline]
    pub fn mem_tag(&self) -> MemTag {
        self.mem_tag
    }

    #[inline]
    pub fn mem_tag_name(&self) -> &'static str {
        NmtUtil::tag_to_name(self.mem_tag)
    }

    #[inline]
    pub fn contain_address(&self, addr: usize) -> bool {
        self.region.contain_address(addr)
    }

    #[inline]
    pub fn contain_region(&self, addr: usize, size: usize) -> bool {
        self.region.contain_region(addr, size)
    }

    #[inline]
    pub fn same_region(&self, addr: usize, size: usize) -> bool {
        self.region.same_region(addr, size)
    }

    #[inline]
    pub fn overlap_region(&self, addr: usize, size: usize) -> bool {
        self.region.overlap_region(addr, size)
    }

    /// Sum of committed sub-region sizes.
    pub fn committed_size(&self) -> usize {
        self.committed.values().map(|r| r.size()).sum()
    }

    /// Iterate committed sub-regions in address order.
    pub fn iterate_committed_regions(&self) -> impl Iterator<Item = &CommittedMemoryRegion> {
        self.committed.values()
    }

    /// Uncommitted thread stack bottom, above guard pages if any.
    pub fn thread_stack_uncommitted_bottom(&self) -> usize {
        debug_assert!(self.mem_tag == MemTag::MtThreadStack, "Only for thread stack");
        let top = self.end();
        let mut bottom = self.base();
        for r in self.committed.values() {
            let committed_top = r.end();
            if committed_top < top {
                bottom = committed_top;
            } else {
                debug_assert!(top == committed_top, "Sanity");
                break;
            }
        }
        bottom
    }

    fn exclude_region(&mut self, addr: usize, size: usize) {
        self.region.exclude_region(addr, size);
    }

    /// Change the memory tag, moving summary accounting between tags.
    pub fn set_mem_tag(&mut self, new_mem_tag: MemTag) {
        debug_assert!(
            self.mem_tag == MemTag::MtNone || self.mem_tag == new_mem_tag,
            "Overwrite memory tag for region"
        );
        if self.mem_tag != new_mem_tag {
            let summary = VirtualMemorySummary::global();
            summary.move_reserved_memory(self.mem_tag, new_mem_tag, self.size());
            summary.move_committed_memory(self.mem_tag, new_mem_tag, self.committed_size());
            self.mem_tag = new_mem_tag;
        }
    }

    // -- committed sub-region operations -------------------------------------

    /// Last committed region with end() <= addr (find_preceding_node_from).
    fn preceding_key(&self, addr: usize) -> Option<usize> {
        let mut prev = None;
        for (&k, r) in self.committed.iter() {
            if r.end() > addr {
                break;
            }
            prev = Some(k);
        }
        prev
    }

    /// First committed region after `prev_key` in base order.
    fn next_key_after(&self, prev_key: Option<usize>) -> Option<usize> {
        match prev_key {
            Some(k) => self.committed.range((std::ops::Bound::Excluded(k), std::ops::Bound::Unbounded)).next().map(|(k2, _)| *k2),
            None => self.committed.keys().next().copied(),
        }
    }

    fn is_mergeable_with(rgn: &CommittedMemoryRegion, addr: usize, size: usize, stack: &NativeCallStack) -> bool {
        rgn.adjacent_to(addr, size) && rgn.call_stack().equals(stack)
    }

    fn is_same_as(rgn: &CommittedMemoryRegion, addr: usize, size: usize, stack: &NativeCallStack) -> bool {
        rgn.same_region(addr, size) && rgn.call_stack().equals(stack)
    }

    /// Try to merge the new [addr,size,stack) region into `key`'s region.
    /// Returns true if merged.
    fn try_merge_with(&mut self, key: usize, addr: usize, size: usize, stack: &NativeCallStack) -> bool {
        if let Some(rgn) = self.committed.get_mut(&key) {
            if Self::is_mergeable_with(rgn, addr, size, stack) {
                rgn.expand_region(addr, size);
                return true;
            }
        }
        false
    }

    /// Node-level remove: [addr,sz) is strictly inside rgn (not identical,
    /// contained). Shrinks an edge or splits the region in two.
    /// Returns the key of the (possibly new) high region.
    fn remove_uncommitted_within(&mut self, key: usize, addr: usize, sz: usize) -> Option<usize> {
        let rgn = self.committed.get(&key)?;
        debug_assert!(rgn.contain_region(addr, sz), "Has to be contained");
        debug_assert!(!rgn.same_region(addr, sz), "Can not be the same region");
        if rgn.base() == addr || rgn.end() == addr + sz {
            self.committed.get_mut(&key).unwrap().exclude_region(addr, sz);
            None
        } else {
            // Split: keep lower part [base, addr), create high part
            // [addr+sz, top) with the same call stack.
            let stack = *self.committed.get(&key).unwrap().call_stack();
            let top = self.committed.get(&key).unwrap().end();
            self.committed.get_mut(&key).unwrap().exclude_region(addr, top - addr);
            let high_base = addr + sz;
            let high_size = top - high_base;
            let high = CommittedMemoryRegion::new(high_base, high_size, stack);
            self.committed.insert(high_base, high);
            Some(high_base)
        }
    }

    /// Add a committed region [addr, addr+size) inside this reserved
    /// region. Merges with adjacent regions sharing the call stack.
    pub fn add_committed_region(&mut self, addr: usize, size: usize, stack: NativeCallStack) -> bool {
        debug_assert!(addr != 0, "Invalid address");
        debug_assert!(size > 0, "Invalid size");
        debug_assert!(self.contain_region(addr, size), "Not contain this region");

        let mut prev = self.preceding_key(addr);
        let mut next = self.next_key_after(prev);

        if let Some(nk) = next {
            let rgn = &self.committed[&nk];
            // Ignore request if region already exists.
            if Self::is_same_as(rgn, addr, size, &stack) {
                return true;
            }
            // Overlapping next region: clear all overlapping committed
            // regions, then recompute prev/next.
            if rgn.overlap_region(addr, size) {
                self.remove_uncommitted_region(addr, size);
                prev = self.preceding_key(addr);
                next = self.next_key_after(prev);
            }
        }

        VirtualMemorySummary::global().record_committed_memory(size, self.mem_tag);

        // Try to merge with prev, possibly absorbing next as well.
        if let Some(pk) = prev {
            if self.try_merge_with(pk, addr, size, &stack) {
                // prev was expanded; try to merge next into it.
                if let Some(nk) = next {
                    if nk != pk {
                        // Check adjacency of next to the (expanded) prev.
                        let (prev_end, prev_stack) = {
                            let p = &self.committed[&pk];
                            (p.end(), *p.call_stack())
                        };
                        let n_base = self.committed[&nk].base();
                        let n_size = self.committed[&nk].size();
                        if n_base == prev_end && self.committed[&nk].call_stack().equals(&prev_stack) {
                            self.committed.get_mut(&pk).unwrap().expand_region(n_base, n_size);
                            self.committed.remove(&nk);
                        }
                    }
                }
                return true;
            }
        }

        // Didn't merge with prev; try with next.
        if let Some(nk) = next {
            if self.try_merge_with(nk, addr, size, &stack) {
                return true;
            }
        }

        // Couldn't merge — create a new region.
        self.committed.insert(addr, CommittedMemoryRegion::new(addr, size, stack));
        true
    }

    /// Remove committed memory [addr, addr+size) from this region.
    pub fn remove_uncommitted_region(&mut self, addr: usize, sz: usize) -> bool {
        debug_assert!(addr != 0, "Invalid address");
        debug_assert!(sz > 0, "Invalid size");
        let end = addr + sz;
        let summary = VirtualMemorySummary::global();

        loop {
            // Find first committed region overlapping [addr, end).
            let mut hit: Option<usize> = None;
            for (&k, r) in self.committed.iter() {
                if r.end() > addr && r.base() < end {
                    hit = Some(k);
                    break;
                }
                if r.base() >= end {
                    break;
                }
            }
            let Some(key) = hit else { break };
            let rgn = self.committed.get(&key).unwrap().clone();

            if rgn.same_region(addr, sz) {
                summary.record_uncommitted_memory(rgn.size(), self.mem_tag);
                self.committed.remove(&key);
                continue;
            }

            // del_rgn contains crgn entirely.
            if addr <= rgn.base() && rgn.end() <= end {
                summary.record_uncommitted_memory(rgn.size(), self.mem_tag);
                self.committed.remove(&key);
                continue;
            }

            if rgn.contain_address(addr) {
                if rgn.contain_address(end - 1) {
                    // del_rgn is contained in crgn.
                    summary.record_uncommitted_memory(sz, self.mem_tag);
                    self.remove_uncommitted_within(key, addr, sz);
                    continue;
                } else {
                    // Found addr but not del_rgn's end: exclude the tail.
                    let size = rgn.end() - addr;
                    self.committed.get_mut(&key).unwrap().exclude_region(addr, size);
                    summary.record_uncommitted_memory(size, self.mem_tag);
                    continue;
                }
            } else if rgn.contain_address(end - 1) {
                // Found del_rgn's end, but not its base: exclude the head.
                let size = end - rgn.base();
                self.committed.get_mut(&key).unwrap().exclude_region(rgn.base(), size);
                summary.record_uncommitted_memory(size, self.mem_tag);
                continue;
            } else {
                // Region does not actually overlap the del range in a
                // handled way — skip past it to avoid an infinite loop.
                // (Non-overlapping interior regions are removed by the
                // "del contains crgn" case above.)
                break;
            }
        }
        true
    }

    /// Move committed regions with base >= addr to `rgn`.
    pub fn move_committed_regions(&mut self, addr: usize, rgn: &mut ReservedMemoryRegion) {
        debug_assert!(addr != 0, "Invalid address");
        let tail: BTreeMap<usize, CommittedMemoryRegion> =
            self.committed.split_off(&addr);
        for (k, v) in tail {
            rgn.committed.insert(k, v);
        }
    }
}

// ---------------------------------------------------------------------------
// VirtualMemoryTracker
// ---------------------------------------------------------------------------

pub struct VirtualMemoryTracker {
    /// Reserved regions keyed by base address (sorted, non-overlapping).
    reserved_regions: Mutex<BTreeMap<usize, ReservedMemoryRegion>>,
}

static GLOBAL_VMT: OnceLock<VirtualMemoryTracker> = OnceLock::new();
static VMT_INITIALIZED: AtomicUsize = AtomicUsize::new(0);

impl VirtualMemoryTracker {
    pub fn global() -> &'static Self {
        GLOBAL_VMT.get_or_init(|| Self { reserved_regions: Mutex::new(BTreeMap::new()) })
    }

    pub fn initialize(level: NmtTrackingLevel) -> bool {
        if level >= NmtTrackingLevel::Summary {
            let _ = Self::global();
            VMT_INITIALIZED.store(1, Ordering::Relaxed);
        }
        true
    }

    #[inline]
    pub fn is_initialized(&self) -> bool {
        VMT_INITIALIZED.load(Ordering::Relaxed) != 0
    }

    // -- public operations ----------------------------------------------------

    /// Add a reserved region. Handles recursive reservation, thread stack
    /// overwrite and the special mtClassShared/mtJavaHeap/mtCode absorption
    /// rules from the C++ implementation.
    pub fn add_reserved_region(
        &self,
        base_addr: usize,
        size: usize,
        stack: NativeCallStack,
        mem_tag: MemTag,
    ) -> bool {
        debug_assert!(base_addr != 0, "Invalid address");
        debug_assert!(size > 0, "Invalid size");

        let mut regions = self.reserved_regions.lock().unwrap();
        let rgn = ReservedMemoryRegion::new(base_addr, size, stack, mem_tag);

        if let Some(key) = Self::find_overlapping_in(&regions, base_addr, size) {
            let reserved_rgn = regions.get(&key).unwrap();
            // Deal with recursive reservation
            // (os::reserve_memory -> pd_reserve_memory -> os::reserve_memory).
            if reserved_rgn.same_region(base_addr, size)
                && (reserved_rgn.mem_tag() == mem_tag || reserved_rgn.mem_tag() == MemTag::MtNone)
            {
                let r = regions.get_mut(&key).unwrap();
                r.set_call_stack(stack);
                r.set_mem_tag(mem_tag);
                return true;
            }

            debug_assert!(reserved_rgn.overlap_region(base_addr, size), "Must be");

            // Overlapped reservation: thread stacks of exiting JNI threads.
            if reserved_rgn.mem_tag() == MemTag::MtThreadStack {
                let committed = reserved_rgn.committed_size();
                let old_size = reserved_rgn.size();
                let old_tag = reserved_rgn.mem_tag();
                let summary = VirtualMemorySummary::global();
                summary.record_uncommitted_memory(committed, old_tag);
                summary.record_released_memory(old_size, old_tag);
                summary.record_reserved_memory(rgn.size(), mem_tag);
                regions.insert(key, rgn);
                return true;
            }

            // CDS mapping region: NMT reports CDS as a whole.
            if reserved_rgn.mem_tag() == MemTag::MtClassShared
                && reserved_rgn.contain_region(base_addr, size)
            {
                return true;
            }

            // Mapped CDS string region: part of the java heap.
            if reserved_rgn.mem_tag() == MemTag::MtJavaHeap
                && reserved_rgn.contain_region(base_addr, size)
            {
                return true;
            }

            if reserved_rgn.mem_tag() == MemTag::MtCode
                && reserved_rgn.contain_region(base_addr, size)
            {
                return true;
            }

            // Overlapped reservation we cannot merge.
            return false;
        }

        VirtualMemorySummary::global().record_reserved_memory(size, mem_tag);
        regions.insert(base_addr, rgn);
        true
    }

    /// Find the reserved region overlapping [addr, addr+size) in an
    /// already-locked map. Mirrors SortedLinkedList::find with compare()==0.
    fn find_overlapping_in(
        regions: &BTreeMap<usize, ReservedMemoryRegion>,
        addr: usize,
        size: usize,
    ) -> Option<usize> {
        if let Some((&k, r)) = regions.range(..=addr).next_back() {
            if r.overlap_region(addr, size) {
                return Some(k);
            }
        }
        if let Some((&k, r)) = regions
            .range((std::ops::Bound::Excluded(addr), std::ops::Bound::Unbounded))
            .next()
        {
            if r.overlap_region(addr, size) {
                return Some(k);
            }
        }
        None
    }

    /// Set the memory tag of the reserved region containing `addr`.
    /// The region must currently be tagged mtNone.
    pub fn set_reserved_region_type(&self, addr: usize, _size: usize, mem_tag: MemTag) -> bool {
        debug_assert!(addr != 0, "Invalid address");
        let mut regions = self.reserved_regions.lock().unwrap();
        // C++ searches with a 1-byte probe region at addr.
        if let Some(key) = Self::find_overlapping_in(&regions, addr, 1) {
            let r = regions.get_mut(&key).unwrap();
            debug_assert!(r.contain_address(addr), "Containment");
            if r.mem_tag() != mem_tag {
                debug_assert!(
                    r.mem_tag() == MemTag::MtNone,
                    "Overwrite memory tag (should be mtNone)"
                );
                r.set_mem_tag(mem_tag);
            }
            return true;
        }
        false
    }

    /// Add a committed region [addr, addr+size). The region must be fully
    /// contained in a reserved region.
    pub fn add_committed_region(&self, addr: usize, size: usize, stack: NativeCallStack) -> bool {
        debug_assert!(addr != 0, "Invalid address");
        debug_assert!(size > 0, "Invalid size");
        let mut regions = self.reserved_regions.lock().unwrap();
        let Some(key) = Self::find_overlapping_in(&regions, addr, size) else {
            return false;
        };
        let rgn = regions.get_mut(&key).unwrap();
        if !rgn.contain_region(addr, size) {
            return false;
        }
        rgn.add_committed_region(addr, size, stack)
    }

    /// Remove committed memory [addr, addr+size).
    pub fn remove_uncommitted_region(&self, addr: usize, size: usize) -> bool {
        debug_assert!(addr != 0, "Invalid address");
        debug_assert!(size > 0, "Invalid size");
        let mut regions = self.reserved_regions.lock().unwrap();
        let Some(key) = Self::find_overlapping_in(&regions, addr, size) else {
            return false;
        };
        let rgn = regions.get_mut(&key).unwrap();
        if !rgn.contain_region(addr, size) {
            return false;
        }
        rgn.remove_uncommitted_region(addr, size)
    }

    /// Release a whole reserved region found by key: uncommit everything
    /// inside, record the release, remove from the map
    /// (C++ remove_released_region(ReservedMemoryRegion*)). Caller holds
    /// the lock.
    fn release_whole_region(regions: &mut BTreeMap<usize, ReservedMemoryRegion>, key: usize) -> bool {
        let Some(rgn) = regions.get_mut(&key) else { return false };
        let base = rgn.base();
        let size = rgn.size();
        if !rgn.remove_uncommitted_region(base, size) {
            return false;
        }
        let tag = rgn.mem_tag();
        VirtualMemorySummary::global().record_released_memory(size, tag);
        regions.remove(&key);
        true
    }

    /// Remove (release) a reserved region [addr, addr+size). Handles both
    /// whole-region release and partial release (region split), plus the
    /// mtClassShared archive+class space combined release.
    pub fn remove_released_region(&self, addr: usize, size: usize) -> bool {
        debug_assert!(addr != 0, "Invalid address");
        debug_assert!(size > 0, "Invalid size");
        let mut regions = self.reserved_regions.lock().unwrap();
        let Some(key) = Self::find_overlapping_in(&regions, addr, size) else {
            return false;
        };

        if regions[&key].same_region(addr, size) {
            return Self::release_whole_region(&mut regions, key);
        }

        // Uncommit regions within the released region.
        {
            let rgn = regions.get_mut(&key).unwrap();
            if !rgn.remove_uncommitted_region(addr, size) {
                return false;
            }
        }

        let tag = regions[&key].mem_tag();
        if tag == MemTag::MtClassShared {
            let r = &regions[&key];
            if r.contain_region(addr, size) {
                // Unmapped CDS region inside the reserved shared region.
                return true;
            }
            if size > r.size() {
                // Release spanning archive space and class space: release
                // both regions altogether.
                let shared_base = r.base();
                let shared_size = r.size();
                let class_addr = addr + shared_size;
                let class_size = size - shared_size;
                let class_key = Self::find_overlapping_in(&regions, class_addr, class_size);
                if let Some(ck) = class_key {
                    if regions[&ck].mem_tag() == MemTag::MtClass {
                        let ok1 = Self::release_whole_region(&mut regions, key);
                        let ok2 = Self::release_whole_region(&mut regions, ck);
                        return ok1 && ok2;
                    }
                }
                let _ = shared_base;
                return false;
            }
        }

        VirtualMemorySummary::global().record_released_memory(size, tag);

        let r = regions.get_mut(&key).unwrap();
        debug_assert!(r.contain_region(addr, size), "Not completely contained");
        if r.base() == addr || r.end() == addr + size {
            r.exclude_region(addr, size);
            return true;
        }

        // Split the reserved region: lower part keeps [base, addr), the
        // high part [addr+size, end) inherits stack/tag and committed
        // regions at or above addr.
        let top = r.end();
        let high_base = addr + size;
        let high_size = top - high_base;
        let high_stack = *r.call_stack();
        let high_tag = r.mem_tag();
        r.exclude_region(addr, top - addr);
        let mut high = ReservedMemoryRegion::new(high_base, high_size, high_stack, high_tag);
        r.move_committed_regions(addr, &mut high);
        regions.insert(high_base, high);
        true
    }

    /// Split an existing reserved region into two at `split` bytes.
    /// Both parts keep stack/tag of the original; `split_tag` overrides
    /// the tag of the second part.
    pub fn split_reserved_region(
        &self,
        addr: usize,
        size: usize,
        split: usize,
        mem_tag: MemTag,
        split_tag: MemTag,
    ) -> bool {
        let mut regions = self.reserved_regions.lock().unwrap();
        let Some(key) = Self::find_overlapping_in(&regions, addr, size) else {
            return false;
        };
        let rgn = regions.get(&key).unwrap();
        if !rgn.same_region(addr, size) || rgn.committed_size() != 0 {
            return false;
        }
        let original_stack = *rgn.call_stack();
        let original_tag = rgn.mem_tag();
        // C++ goes through remove_released_region(rgn): uncommit (empty by
        // precondition), record release, remove from the map.
        VirtualMemorySummary::global().record_released_memory(size, original_tag);
        regions.remove(&key);
        drop(regions);

        let ok1 = self.add_reserved_region(addr, split, original_stack, mem_tag);
        let ok2 = self.add_reserved_region(addr + split, size - split, original_stack, split_tag);
        ok1 && ok2
    }

    /// Walk all reserved regions in base order.
    pub fn walk<F: FnMut(&ReservedMemoryRegion) -> bool>(&self, mut f: F) -> bool {
        let regions = self.reserved_regions.lock().unwrap();
        for rgn in regions.values() {
            if !f(rgn) {
                return false;
            }
        }
        true
    }

    /// Snapshot all reserved regions (for baselines).
    pub fn reserved_regions_snapshot(&self) -> Vec<ReservedMemoryRegion> {
        self.reserved_regions.lock().unwrap().values().cloned().collect()
    }

    /// If p is contained within a known memory region, return a
    /// description; None otherwise (C++ print_containing_region).
    pub fn print_containing_region(&self, p: usize) -> Option<String> {
        let regions = self.reserved_regions.lock().unwrap();
        for rgn in regions.values() {
            if rgn.contain_address(p) {
                let mut out = format!(
                    "0x{:x} in mmap'd memory region [0x{:x} - 0x{:x}], tag {}",
                    p,
                    rgn.base(),
                    rgn.end(),
                    NmtUtil::tag_to_enum_name(rgn.mem_tag())
                );
                out.push('\n');
                if crate::share::nmt::memTracker::MemTracker::tracking_level()
                    == NmtTrackingLevel::Detail
                {
                    out.push_str(&rgn.call_stack().print_on());
                }
                return Some(out);
            }
        }
        None
    }

    /// Snapshot committed ranges of all thread stack regions using the OS
    /// page state (C++ SnapshotThreadStackWalker). Only meaningful on
    /// platforms with VirtualQuery-style page state queries.
    pub fn snapshot_thread_stacks(&self) {
        #[cfg(windows)]
        {
            let mut regions = self.reserved_regions.lock().unwrap();
            let keys: Vec<usize> = regions
                .iter()
                .filter(|(_, r)| r.mem_tag() == MemTag::MtThreadStack)
                .map(|(&k, _)| k)
                .collect();
            for key in keys {
                let (stack_bottom, stack_size) = {
                    let r = &regions[&key];
                    let bottom = r.thread_stack_uncommitted_bottom();
                    (bottom, r.end() - bottom)
                };
                let page = crate::share::platform::windows::OsWindows::vm_page_size();
                let aligned_stack_size = stack_size.div_ceil(page) * page;
                let mut committed_ranges = Vec::new();
                committed_in_range(stack_bottom, aligned_stack_size, &mut committed_ranges);
                let empty = NativeCallStack::empty();
                if let Some(region) = regions.get_mut(&key) {
                    for (cs, csz) in committed_ranges {
                        let mut csz = csz;
                        // Unaligned stack_size case: correct the region to
                        // fit the actual stack size.
                        if stack_bottom + stack_size < cs + csz {
                            csz = stack_bottom + stack_size - cs;
                        }
                        region.add_committed_region(cs, csz, empty);
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            // No OS committed-page query on this platform; thread stack
            // committed detail is unavailable (regions stay reserved-only).
        }
    }

    /// Render the detail "virtual memory allocation site" section used
    /// by reports: regions aggregated by (call stack, tag), address order.
    pub fn site_section(&self) -> String {
        let mut out = String::new();
        let mut sites: Vec<(NativeCallStack, MemTag, usize, usize)> = Vec::new();
        let regions = self.reserved_regions.lock().unwrap();
        for rgn in regions.values() {
            if rgn.size() == 0 {
                continue;
            }
            let stack = *rgn.call_stack();
            let tag = rgn.mem_tag();
            if let Some(e) = sites
                .iter_mut()
                .find(|(s, t, _, _)| *s == stack && *t == tag)
            {
                e.2 += rgn.size();
                e.3 += rgn.committed_size();
            } else {
                sites.push((stack, tag, rgn.size(), rgn.committed_size()));
            }
        }
        sites.sort_by(|a, b| a.0.compare(&b.0));
        if sites.is_empty() {
            out.push_str("  (no virtual memory allocations recorded)\n");
            return out;
        }
        for (stack, tag, reserved, committed) in sites {
            out.push_str(&format!(
                "  {}: reserved={} committed={}\n",
                tag.name(),
                reserved,
                committed
            ));
            out.push_str(&stack.print_on());
        }
        out
    }

    /// Release all tracked state (tests / shutdown).
    pub fn reset(&self) {
        self.reserved_regions.lock().unwrap().clear();
        VirtualMemorySummary::global().reset();
    }

    /// Per-tag VM totals for reports.
    pub fn totals_for(&self, tag: MemTag) -> TagVmTotals {
        let v = VirtualMemorySummary::global().as_snapshot().by_tag(tag);
        TagVmTotals { reserved: v.reserved(), committed: v.committed(), regions: 0 }
    }

    pub fn total_reserved(&self) -> usize {
        VirtualMemorySummary::global().as_snapshot().total_reserved()
    }

    pub fn total_committed(&self) -> usize {
        VirtualMemorySummary::global().as_snapshot().total_committed()
    }
}

/// Backwards-compatible per-tag totals view.
#[derive(Debug, Clone, Copy, Default)]
pub struct TagVmTotals {
    pub reserved: usize,
    pub committed: usize,
    pub regions: usize,
}

/// Find committed (MEM_COMMIT) subranges within [start, start+size) using
/// OS page queries. Windows implementation via VirtualQuery.
#[cfg(windows)]
pub fn committed_in_range(start: usize, size: usize, out: &mut Vec<(usize, usize)>) -> bool {
    use crate::share::platform::windows::{OsWindows, MEM_COMMIT_STATE};
    let end = start + size;
    let page = OsWindows::vm_page_size();
    let mut cur = start;
    while cur < end {
        match OsWindows::virtual_query(cur) {
            Some(mi) => {
                let rstart = mi.base.max(start);
                let rend = (mi.base + mi.region_size).min(end);
                if mi.state == MEM_COMMIT_STATE && rend > rstart {
                    // merge with previous if adjacent
                    if let Some(last) = out.last_mut() {
                        if last.0 + last.1 == rstart {
                            last.1 += rend - rstart;
                            cur = rend;
                            continue;
                        }
                    }
                    out.push((rstart, rend - rstart));
                }
                cur = rend.max(cur + page);
                if mi.region_size == 0 {
                    break;
                }
            }
            None => return false,
        }
    }
    true
}

#[cfg(not(windows))]
pub fn committed_in_range(_start: usize, _size: usize, _out: &mut Vec<(usize, usize)>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> NativeCallStack {
        NativeCallStack::from_frames(&[0x1000, 0x2000])
    }

    #[test]
    fn reserve_commit_uncommit_release_totals() {
        let _g = crate::share::nmt::nmtCommon::NMT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = VirtualMemoryTracker::global();
        t.reset();
        let s = stack();
        assert!(t.add_reserved_region(0x10000, 0x4000, s, MemTag::MtMetaspace));
        let totals = t.totals_for(MemTag::MtMetaspace);
        assert_eq!(totals.reserved, 0x4000);
        assert_eq!(totals.committed, 0);

        assert!(t.add_committed_region(0x10000, 0x2000, s));
        assert!(t.add_committed_region(0x12000, 0x1000, s));
        let totals = t.totals_for(MemTag::MtMetaspace);
        assert_eq!(totals.committed, 0x3000);

        assert!(t.remove_uncommitted_region(0x10000, 0x2000));
        assert_eq!(t.totals_for(MemTag::MtMetaspace).committed, 0x1000);

        assert!(t.remove_released_region(0x10000, 0x4000));
        let totals = t.totals_for(MemTag::MtMetaspace);
        assert_eq!(totals.reserved, 0);
        assert_eq!(totals.committed, 0);
        assert_eq!(t.total_reserved(), 0);
        assert_eq!(t.total_committed(), 0);
    }

    #[test]
    fn partial_release_splits_region() {
        let _g = crate::share::nmt::nmtCommon::NMT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = VirtualMemoryTracker::global();
        t.reset();
        let s = stack();
        assert!(t.add_reserved_region(0x20000, 0x8000, s, MemTag::MtCode));
        assert!(t.add_committed_region(0x20000, 0x8000, s));

        // Release the middle third: [0x22000, 0x24000).
        assert!(t.remove_released_region(0x22000, 0x2000));
        let totals = t.totals_for(MemTag::MtCode);
        assert_eq!(totals.reserved, 0x6000);
        assert_eq!(totals.committed, 0x6000);

        let mut count = 0;
        t.walk(|r| {
            count += 1;
            true
        });
        assert_eq!(count, 2, "interior release must split into two regions");

        // Head release shrinks the lower region.
        assert!(t.remove_released_region(0x20000, 0x2000));
        assert_eq!(t.totals_for(MemTag::MtCode).reserved, 0x4000);
        t.reset();
    }

    #[test]
    fn retag_region() {
        let _g = crate::share::nmt::nmtCommon::NMT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = VirtualMemoryTracker::global();
        t.reset();
        let s = stack();
        assert!(t.add_reserved_region(0x30000, 0x1000, s, MemTag::MtNone));
        assert!(t.add_committed_region(0x30000, 0x1000, s));
        assert!(t.set_reserved_region_type(0x30000, 0x1000, MemTag::MtClassShared));
        assert_eq!(t.totals_for(MemTag::MtNone).reserved, 0);
        assert_eq!(t.totals_for(MemTag::MtClassShared).reserved, 0x1000);
        assert_eq!(t.totals_for(MemTag::MtClassShared).committed, 0x1000);
        t.reset();
    }

    #[test]
    fn snapshot_and_reset() {
        let _g = crate::share::nmt::nmtCommon::NMT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = VirtualMemoryTracker::global();
        t.reset();
        let s = stack();
        assert!(t.add_reserved_region(0x40000, 0x2000, s, MemTag::MtGC));
        assert!(t.add_committed_region(0x40000, 0x1000, s));

        let mut snap = VirtualMemorySnapshot::new();
        VirtualMemorySummary::global().snapshot(&mut snap);
        assert_eq!(snap.by_tag(MemTag::MtGC).reserved(), 0x2000);
        assert_eq!(snap.by_tag(MemTag::MtGC).committed(), 0x1000);

        t.reset();
        assert_eq!(t.total_reserved(), 0);
        assert_eq!(t.total_committed(), 0);
    }
}
