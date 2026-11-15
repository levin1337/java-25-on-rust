/*
 * Port of OpenJDK 25 HotSpot VirtualMemoryTracker
 * (share/nmt/virtualMemoryTracker.hpp/.cpp)
 *
 * Tracks reserved and committed virtual memory per MemTag as a set of
 * [base, size) regions. In HotSpot this is fed by os::pd_reserve_memory /
 * pd_commit_memory / pd_uncommit_memory / pd_release_memory hooks; here the
 * platform layer (share::platform::windows) is the source of truth and this
 * tracker records the same events when callers go through the NMT-aware
 * wrappers in `MemTracker`.
 */

use super::memTag::MemTag;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// One tracked VM region.
#[derive(Debug, Clone, Copy, Default)]
pub struct VmRegion {
    pub base: usize,
    pub size: usize,
    pub committed: usize,
    pub tag: MemTag,
    pub is_arena: bool,
}

impl VmRegion {
    pub fn end(&self) -> usize {
        self.base.saturating_add(self.size)
    }
    pub fn committed_end(&self) -> usize {
        self.base.saturating_add(self.committed)
    }
}

/// Per-tag totals.
#[derive(Debug, Clone, Copy, Default)]
pub struct TagVmTotals {
    pub reserved: usize,
    pub committed: usize,
    pub regions: usize,
}

pub struct VirtualMemoryTracker {
    /// addr range key → region (sorted for range lookups).
    regions: Mutex<BTreeMap<usize, VmRegion>>,
    /// Per-tag accumulated totals.
    totals: Mutex<[TagVmTotals; MemTag::MtNumberOfTags as usize]>,
}

static GLOBAL_VMT: std::sync::OnceLock<VirtualMemoryTracker> = std::sync::OnceLock::new();

impl VirtualMemoryTracker {
    pub fn global() -> &'static Self {
        GLOBAL_VMT.get_or_init(Self::new)
    }

    fn new() -> Self {
        Self {
            regions: Mutex::new(BTreeMap::new()),
            totals: Mutex::new([TagVmTotals::default(); MemTag::MtNumberOfTags as usize]),
        }
    }

    /// Record a MEM_RESERVE (or fresh MEM_COMMIT|MEM_RESERVE) region.
    pub fn record_reserve(&self, base: usize, size: usize, tag: MemTag) {
        if size == 0 {
            return;
        }
        let mut regions = self.regions.lock().unwrap();
        regions.insert(
            base,
            VmRegion {
                base,
                size,
                committed: 0,
                tag,
                is_arena: false,
            },
        );
        drop(regions);
        self.add_totals(tag, size, 0, 1);
    }

    /// Record a MEM_COMMIT within a reserved region (or standalone commit).
    pub fn record_commit(&self, addr: usize, size: usize, tag: MemTag) {
        if size == 0 {
            return;
        }
        let mut regions = self.regions.lock().unwrap();
        // Find region containing addr.
        let key = regions
            .range(..=addr)
            .next_back()
            .map(|(k, _)| *k);
        if let Some(k) = key {
            if let Some(r) = regions.get_mut(&k) {
                if addr >= r.base && addr < r.end() {
                    let old_committed = r.committed;
                    r.committed = r.committed.max(addr + size - r.base);
                    let delta = r.committed - old_committed;
                    r.tag = tag;
                    drop(regions);
                    self.add_totals(tag, 0, delta, 0);
                    return;
                }
            }
        }
        drop(regions);
        // Standalone committed region (allocate_memory path).
        let mut regions = self.regions.lock().unwrap();
        regions.insert(
            addr,
            VmRegion {
                base: addr,
                size,
                committed: size,
                tag,
                is_arena: false,
            },
        );
        drop(regions);
        self.add_totals(tag, size, size, 1);
    }

    /// Record MEM_DECOMMIT.
    pub fn record_uncommit(&self, addr: usize, size: usize) {
        if size == 0 {
            return;
        }
        let mut regions = self.regions.lock().unwrap();
        let key = regions.range(..=addr).next_back().map(|(k, _)| *k);
        let mut freed_tag = MemTag::MtInternal;
        let mut freed_committed = 0usize;
        if let Some(k) = key {
            if let Some(r) = regions.get_mut(&k) {
                if addr >= r.base && addr < r.end() {
                    let new_committed = (addr + size - r.base).min(r.committed);
                    freed_committed = r.committed.saturating_sub(new_committed);
                    r.committed = new_committed;
                    freed_tag = r.tag;
                }
            }
        }
        drop(regions);
        self.sub_committed(freed_tag, freed_committed);
    }

    /// Record MEM_RELEASE of a whole reservation.
    pub fn record_release(&self, base: usize, _size: usize) {
        let mut regions = self.regions.lock().unwrap();
        // Release may cover multiple mappings; drop all regions overlapping.
        let keys: Vec<usize> = regions
            .range(..)
            .filter(|(_, r)| r.base == base || (r.base <= base && base < r.end()))
            .map(|(k, _)| *k)
            .collect();
        let mut freed_reserved = 0usize;
        let mut freed_committed = 0usize;
        let mut tag = MemTag::MtInternal;
        for k in keys {
            if let Some(r) = regions.remove(&k) {
                freed_reserved += r.size;
                freed_committed += r.committed.min(r.size);
                tag = r.tag;
            }
        }
        drop(regions);
        if freed_reserved > 0 {
            self.sub_reserved(tag, freed_reserved);
            self.sub_committed(tag, freed_committed);
        }
    }

    fn add_totals(&self, tag: MemTag, reserved: usize, committed: usize, regions: usize) {
        let mut totals = self.totals.lock().unwrap();
        let t = &mut totals[tag as usize];
        t.reserved = t.reserved.saturating_add(reserved);
        t.committed = t.committed.saturating_add(committed);
        t.regions = t.regions.saturating_add(regions);
    }

    fn sub_reserved(&self, tag: MemTag, v: usize) {
        let mut totals = self.totals.lock().unwrap();
        totals[tag as usize].reserved = totals[tag as usize].reserved.saturating_sub(v);
    }

    fn sub_committed(&self, tag: MemTag, v: usize) {
        let mut totals = self.totals.lock().unwrap();
        totals[tag as usize].committed = totals[tag as usize].committed.saturating_sub(v);
    }

    pub fn totals_for(&self, tag: MemTag) -> TagVmTotals {
        self.totals.lock().unwrap()[tag as usize]
    }

    pub fn all_totals(&self) -> Vec<(MemTag, TagVmTotals)> {
        let totals = self.totals.lock().unwrap();
        MemTag::all()
            .map(|t| (t, totals[t as usize]))
            .collect()
    }

    pub fn total_reserved(&self) -> usize {
        self.totals.lock().unwrap().iter().map(|t| t.reserved).sum()
    }

    pub fn total_committed(&self) -> usize {
        self.totals.lock().unwrap().iter().map(|t| t.committed).sum()
    }

    /// Snapshot of all tracked regions (for reports / SA).
    pub fn regions_snapshot(&self) -> Vec<VmRegion> {
        self.regions.lock().unwrap().values().copied().collect()
    }

    pub fn reset(&self) {
        self.regions.lock().unwrap().clear();
        *self.totals.lock().unwrap() = [TagVmTotals::default(); MemTag::MtNumberOfTags as usize];
    }
}
