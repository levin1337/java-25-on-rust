/*
 * Port of OpenJDK 25 HotSpot MemBaseline (share/nmt/memBaseline.hpp/.cpp)
 *
 * A memory baseline is a snapshot of NMT state at a point in time:
 * summary counters always, plus (at detail level) malloc call sites,
 * virtual memory allocations and per-site VM aggregates used by the
 * detail report and baseline diff reports.
 *
 * Metaspace/class-loader statistics are VM-internal and live outside
 * this module; the baseline stores zeros for them (the reporters omit
 * metaspace sections when the values are unavailable).
 */

use super::mallocSiteTable::{MallocSiteSnapshot, MallocSiteTable};
use super::mallocTracker::{MallocMemory, MallocMemorySnapshot, MallocMemorySummary};
use super::memTag::MemTag;
use super::nativeCallStack::NativeCallStack;
use super::nmtCommon::{NmtTrackingLevel, NmtUtil};
use super::virtualMemoryTracker::{
    ReservedMemoryRegion, VirtualMemory, VirtualMemorySnapshot,
    VirtualMemorySummary, VirtualMemoryTracker,
};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BaselineType {
    NotBaselined = 0,
    SummaryBaselined = 1,
    DetailBaselined = 2,
}

/// SortingOrder for site lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortingOrder {
    ByAddress,
    BySize,
    BySite,
    BySiteAndTag,
}

fn cmp_size_desc(a: usize, b: usize) -> Ordering {
    b.cmp(&a)
}

fn stack_cmp(a: &NativeCallStack, b: &NativeCallStack) -> Ordering {
    a.compare(b)
}

/// Metaspace/class statistics placeholders — populated by the VM when
/// available; NMT itself does not own them.
#[derive(Debug, Clone, Copy, Default)]
pub struct MetaspaceCombinedStats {
    pub non_class: MetaspaceStats,
    pub class_type: MetaspaceStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MetaspaceStats {
    pub reserved: usize,
    pub committed: usize,
    pub used: usize,
}

/// MemBaseline — snapshot used by reporters and diff reporters.
pub struct MemBaseline {
    malloc_memory_snapshot: MallocMemorySnapshot,
    virtual_memory_snapshot: VirtualMemorySnapshot,
    metaspace_stats: MetaspaceCombinedStats,
    instance_class_count: usize,
    array_class_count: usize,
    thread_count: usize,

    malloc_sites: Vec<MallocSiteSnapshot>,
    virtual_memory_allocations: Vec<ReservedMemoryRegion>,
    virtual_memory_sites: Vec<VirtualMemoryAllocationSiteSnapshot>,

    malloc_sites_order: SortingOrder,
    virtual_memory_sites_order: SortingOrder,
    baseline_type: BaselineType,
}

/// Immutable view of a VirtualMemoryAllocationSite.
#[derive(Clone)]
pub struct VirtualMemoryAllocationSiteSnapshot {
    pub stack: NativeCallStack,
    pub mem_tag: MemTag,
    pub reserved: usize,
    pub committed: usize,
    pub peak_size: usize,
}

impl MemBaseline {
    pub fn new() -> Self {
        Self {
            malloc_memory_snapshot: MallocMemorySnapshot::new(),
            virtual_memory_snapshot: VirtualMemorySnapshot::new(),
            metaspace_stats: MetaspaceCombinedStats::default(),
            instance_class_count: 0,
            array_class_count: 0,
            thread_count: 0,
            malloc_sites: Vec::new(),
            virtual_memory_allocations: Vec::new(),
            virtual_memory_sites: Vec::new(),
            malloc_sites_order: SortingOrder::BySize,
            virtual_memory_sites_order: SortingOrder::ByAddress,
            baseline_type: BaselineType::NotBaselined,
        }
    }

    /// Take a baseline. summary_only=true skips allocation-site detail
    /// (used for summary reports and summary-level tracking).
    pub fn baseline(&mut self, summary_only: bool) {
        self.reset();
        self.thread_count = super::threadStackTracker::ThreadStackTracker::thread_count();
        self.baseline_summary();
        self.baseline_type = BaselineType::SummaryBaselined;

        if !summary_only
            && super::memTracker::MemTracker::tracking_level() == NmtTrackingLevel::Detail
        {
            self.baseline_allocation_sites();
            self.baseline_type = BaselineType::DetailBaselined;
        }
    }

    fn baseline_summary(&mut self) {
        self.malloc_memory_snapshot = MallocMemorySummary::global().snapshot();
        VirtualMemorySummary::global().snapshot(&mut self.virtual_memory_snapshot);
        // MemoryFileTracker file allocations are folded into the VM
        // snapshot (their committed memory counts under their tags).
        super::memoryFileTracker::MemoryFileTrackerInstance::global().summary_snapshot(
            &mut self.virtual_memory_snapshot,
        );
    }

    fn baseline_allocation_sites(&mut self) {
        // Malloc allocation sites, collected in size order.
        self.malloc_sites = MallocSiteTable::global().sites_by_size();
        self.malloc_sites_order = SortingOrder::BySize;

        // Virtual memory allocations in address order.
        self.virtual_memory_allocations =
            VirtualMemoryTracker::global().reserved_regions_snapshot();

        if !self.aggregate_virtual_memory_allocation_sites() {
            self.virtual_memory_sites.clear();
        }
        self.virtual_memory_sites_order = SortingOrder::ByAddress;
    }

    /// Aggregate VM allocations by call site (stack + tag).
    fn aggregate_virtual_memory_allocation_sites(&mut self) -> bool {
        let mut sites: Vec<VirtualMemoryAllocationSiteSnapshot> = Vec::new();
        for rgn in &self.virtual_memory_allocations {
            if rgn.size() == 0 {
                continue;
            }
            let stack = *rgn.call_stack();
            let tag = rgn.mem_tag();
            if let Some(site) = sites
                .iter_mut()
                .find(|s| s.mem_tag == tag && s.stack.equals(&stack))
            {
                site.reserved += rgn.size();
                site.committed += rgn.committed_size();
            } else {
                sites.push(VirtualMemoryAllocationSiteSnapshot {
                    stack,
                    mem_tag: tag,
                    reserved: rgn.size(),
                    committed: rgn.committed_size(),
                    peak_size: rgn.size(),
                });
            }
        }
        // Regions arrive in base-address order; keep it (ByAddress).
        self.virtual_memory_sites = sites;
        true
    }

    // -- accessors -------------------------------------------------------------

    pub fn baseline_type(&self) -> BaselineType {
        self.baseline_type
    }

    pub fn malloc_memory_snapshot(&self) -> &MallocMemorySnapshot {
        &self.malloc_memory_snapshot
    }

    pub fn virtual_memory_snapshot(&self) -> &VirtualMemorySnapshot {
        &self.virtual_memory_snapshot
    }

    pub fn metaspace_stats(&self) -> &MetaspaceCombinedStats {
        &self.metaspace_stats
    }

    /// Malloc sites in the requested order (only detail baselines).
    /// Read-only view of the recorded malloc sites (no re-sort).
    pub fn malloc_sites_ref(&self) -> &[MallocSiteSnapshot] {
        &self.malloc_sites
    }

    pub fn malloc_sites(&mut self, order: SortingOrder) -> &[MallocSiteSnapshot] {
        self.sort_malloc_sites(order);
        &self.malloc_sites
    }

    /// VM allocation sites in the requested order (detail baselines).
    pub fn virtual_memory_sites(
        &mut self,
        order: SortingOrder,
    ) -> &[VirtualMemoryAllocationSiteSnapshot] {
        self.sort_vm_sites(order);
        &self.virtual_memory_sites
    }

    /// Virtual memory allocations, always in base address order.
    pub fn virtual_memory_allocations(&self) -> &[ReservedMemoryRegion] {
        &self.virtual_memory_allocations
    }

    /// Malloc sites in capture order (size-descending).
    pub fn malloc_sites_ref(&self) -> &[MallocSiteSnapshot] {
        &self.malloc_sites
    }

    /// Malloc sites for reports (size-descending).
    pub fn malloc_sites_for_report(&self) -> &[MallocSiteSnapshot] {
        &self.malloc_sites
    }

    /// VM allocation sites for reports (address order).
    pub fn virtual_memory_sites_for_report(&self) -> &[VirtualMemoryAllocationSiteSnapshot] {
        &self.virtual_memory_sites
    }

    /// Total reserved memory = malloc'd + reserved virtual.
    pub fn total_reserved_memory(&self) -> usize {
        self.malloc_memory_snapshot.total() + self.virtual_memory_snapshot.total_reserved()
    }

    /// Total committed memory = malloc'd + committed virtual.
    pub fn total_committed_memory(&self) -> usize {
        self.malloc_memory_snapshot.total() + self.virtual_memory_snapshot.total_committed()
    }

    pub fn total_arena_memory(&self) -> usize {
        self.malloc_memory_snapshot.total_arena()
    }

    pub fn malloc_tracking_overhead(&self) -> usize {
        self.malloc_memory_snapshot.malloc_overhead()
    }

    pub fn malloc_memory(&self, tag: MemTag) -> &MallocMemory {
        self.malloc_memory_snapshot.by_tag(tag)
    }

    pub fn virtual_memory(&self, tag: MemTag) -> &VirtualMemory {
        self.virtual_memory_snapshot.by_tag(tag)
    }

    pub fn class_count(&self) -> usize {
        self.instance_class_count + self.array_class_count
    }

    pub fn instance_class_count(&self) -> usize {
        self.instance_class_count
    }

    pub fn array_class_count(&self) -> usize {
        self.array_class_count
    }

    pub fn thread_count(&self) -> usize {
        self.thread_count
    }

    /// Reset for reuse.
    pub fn reset(&mut self) {
        self.baseline_type = BaselineType::NotBaselined;
        self.instance_class_count = 0;
        self.array_class_count = 0;
        self.thread_count = 0;
        self.malloc_sites.clear();
        self.virtual_memory_sites.clear();
        self.virtual_memory_allocations.clear();
    }

    fn sort_malloc_sites(&mut self, order: SortingOrder) {
        match order {
            SortingOrder::BySize => {
                if self.malloc_sites_order != SortingOrder::BySize {
                    self.malloc_sites
                        .sort_by(|a, b| cmp_size_desc(a.size, b.size));
                    self.malloc_sites_order = SortingOrder::BySize;
                }
            }
            SortingOrder::BySite | SortingOrder::BySiteAndTag => {
                if self.malloc_sites_order != order {
                    self.malloc_sites.sort_by(|a, b| {
                        let c = stack_cmp(&a.stack, &b.stack);
                        if c == Ordering::Equal && order == SortingOrder::BySiteAndTag {
                            NmtUtil::tag_to_index(a.mem_tag).cmp(&NmtUtil::tag_to_index(b.mem_tag))
                        } else {
                            c
                        }
                    });
                    self.malloc_sites_order = order;
                }
            }
            SortingOrder::ByAddress => {}
        }
    }

    fn sort_vm_sites(&mut self, order: SortingOrder) {
        match order {
            SortingOrder::BySize => {
                if self.virtual_memory_sites_order != SortingOrder::BySize {
                    self.virtual_memory_sites
                        .sort_by(|a, b| cmp_size_desc(a.reserved, b.reserved));
                    self.virtual_memory_sites_order = SortingOrder::BySize;
                }
            }
            SortingOrder::BySite | SortingOrder::BySiteAndTag | SortingOrder::ByAddress => {
                if self.virtual_memory_sites_order != SortingOrder::BySite {
                    self.virtual_memory_sites
                        .sort_by(|a, b| stack_cmp(&a.stack, &b.stack));
                    self.virtual_memory_sites_order = SortingOrder::BySite;
                }
            }
        }
    }
}

impl Default for MemBaseline {
    fn default() -> Self {
        Self::new()
    }
}
