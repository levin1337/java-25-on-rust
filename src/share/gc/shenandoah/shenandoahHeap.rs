/*
 * Port of OpenJDK 25 HotSpot Shenandoah Heap
 * (share/gc/shenandoah/shenandoahHeap.hpp / shenandoahHeap.cpp)
 *
 * Implements Shenandoah ultra-low-latency collector:
 * region management, concurrent marking, concurrent evacuation,
 * reference updating, and the 7-phase Shenandoah collection cycle.
 */

use super::shenandoahBarrierSet::ShenandoahBarrierSet;
use super::shenandoahHeapRegion::ShenandoahHeapRegion;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::collections::HashSet;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShenandoahPhase {
    Idle,
    InitMark,
    ConcurrentMark,
    FinalMark,
    ConcurrentEvac,
    InitUpdateRefs,
    ConcurrentUpdateRefs,
    FinalUpdateRefs,
}

#[derive(Debug)]
pub struct ShenandoahHeap {
    regions: Vec<ShenandoahHeapRegion>,
    region_size: usize,
    capacity: usize,
    max_capacity: usize,
    barrier_set: ShenandoahBarrierSet,
    collection_set: HashSet<usize>,
    current_alloc_region: usize,
    alloc_seqnum: u64,
    gc_cycle_count: u64,
    bytes_reclaimed: usize,
    current_phase: ShenandoahPhase,
}

impl ShenandoahHeap {
    pub fn new(initial_size: usize, max_size: usize) -> Self {
        let region_size = ShenandoahHeapRegion::DEFAULT_REGION_SIZE;
        let num_regions = (max_size + region_size - 1) / region_size;
        let mut regions = Vec::with_capacity(num_regions);

        let mut bottom = 0x1000_0000;
        for i in 0..num_regions {
            regions.push(ShenandoahHeapRegion::new(i, bottom, region_size));
            bottom += region_size;
        }

        Self {
            regions,
            region_size,
            capacity: initial_size,
            max_capacity: max_size,
            barrier_set: ShenandoahBarrierSet::new(),
            collection_set: HashSet::new(),
            current_alloc_region: 0,
            alloc_seqnum: 1,
            gc_cycle_count: 0,
            bytes_reclaimed: 0,
            current_phase: ShenandoahPhase::Idle,
        }
    }

    #[inline]
    pub fn region_size(&self) -> usize {
        self.region_size
    }

    #[inline]
    pub fn num_regions(&self) -> usize {
        self.regions.len()
    }

    #[inline]
    pub fn region(&self, idx: usize) -> Option<&ShenandoahHeapRegion> {
        self.regions.get(idx)
    }

    #[inline]
    pub fn region_mut(&mut self, idx: usize) -> Option<&mut ShenandoahHeapRegion> {
        self.regions.get_mut(idx)
    }

    #[inline]
    pub fn barrier_set(&self) -> &ShenandoahBarrierSet {
        &self.barrier_set
    }

    #[inline]
    pub fn barrier_set_mut(&mut self) -> &mut ShenandoahBarrierSet {
        &mut self.barrier_set
    }

    #[inline]
    pub fn current_phase(&self) -> ShenandoahPhase {
        self.current_phase
    }

    #[inline]
    pub fn gc_cycle_count(&self) -> u64 {
        self.gc_cycle_count
    }

    #[inline]
    pub fn total_bytes_reclaimed(&self) -> usize {
        self.bytes_reclaimed
    }

    pub fn allocate(&mut self, bytes: usize) -> Option<usize> {
        if bytes > self.region_size / 2 {
            return self.allocate_humongous(bytes);
        }

        let seq = self.alloc_seqnum;
        self.alloc_seqnum += 1;

        // Try allocate in current allocation region
        if self.current_alloc_region < self.regions.len() {
            if let Some(addr) = self.regions[self.current_alloc_region].allocate(bytes, seq) {
                return Some(addr);
            }
        }

        // Find next free or regular region with enough room
        for i in 0..self.regions.len() {
            if self.regions[i].is_empty() || self.regions[i].is_regular() {
                if let Some(addr) = self.regions[i].allocate(bytes, seq) {
                    self.current_alloc_region = i;
                    return Some(addr);
                }
            }
        }

        None
    }

    fn allocate_humongous(&mut self, bytes: usize) -> Option<usize> {
        let regions_needed = (bytes + self.region_size - 1) / self.region_size;
        let mut start_idx = None;
        let mut count = 0;

        for (i, r) in self.regions.iter().enumerate() {
            if r.is_empty() {
                if count == 0 {
                    start_idx = Some(i);
                }
                count += 1;
                if count == regions_needed {
                    break;
                }
            } else {
                start_idx = None;
                count = 0;
            }
        }

        if count == regions_needed {
            let start = start_idx.unwrap();
            let seq = self.alloc_seqnum;
            self.alloc_seqnum += 1;

            let mut remaining = bytes;
            for i in 0..regions_needed {
                let chunk = remaining.min(self.region_size);
                if i == 0 {
                    self.regions[start + i].make_humongous_start(chunk, seq);
                } else {
                    self.regions[start + i].make_humongous_cont(chunk, seq);
                }
                remaining -= chunk;
            }

            Some(self.regions[start].bottom())
        } else {
            None
        }
    }

    /// Complete Shenandoah concurrent collection cycle
    pub fn run_gc_cycle(&mut self, _cause: GCCause) -> io::Result<()> {
        self.gc_cycle_count += 1;

        // 1. Init Mark (Pause)
        self.current_phase = ShenandoahPhase::InitMark;
        self.barrier_set.drain_satb_queue();

        // 2. Concurrent Mark
        self.current_phase = ShenandoahPhase::ConcurrentMark;
        // In real HotSpot, mark live objects; here simulate live ratio:
        // Regions with used memory keep ~40% live data, remainder is garbage
        for r in self.regions.iter_mut() {
            if r.is_regular() && r.used() > 0 {
                let live = (r.used() * 4) / 10;
                r.set_live_data(live);
            }
        }

        // 3. Final Mark (Pause) - build Collection Set (CSet)
        self.current_phase = ShenandoahPhase::FinalMark;
        self.collection_set.clear();
        let mut cset_addrs = HashSet::new();

        // Any region with >50% garbage is candidate for collection
        for (idx, r) in self.regions.iter_mut().enumerate() {
            if r.is_regular() && !r.is_pinned() && r.garbage() > (r.used() / 2) {
                r.set_in_cset(true);
                self.collection_set.insert(idx);
                cset_addrs.insert(r.bottom() as u64);
            }
        }
        self.barrier_set.set_cset_addresses(cset_addrs);

        // 4. Concurrent Evacuation
        self.current_phase = ShenandoahPhase::ConcurrentEvac;
        self.barrier_set.set_evacuation_in_progress(true);
        self.barrier_set.clear_forwarding();

        // Evacuate live data from CSet regions to non-CSet regions
        let cset_indices: Vec<usize> = self.collection_set.iter().copied().collect();
        for &cset_idx in &cset_indices {
            let live_bytes = self.regions[cset_idx].live_data();
            let from_addr = self.regions[cset_idx].bottom() as u64;

            if live_bytes > 0 {
                // Find target region
                let seq = self.alloc_seqnum;
                self.alloc_seqnum += 1;
                for target_idx in 0..self.regions.len() {
                    if !self.collection_set.contains(&target_idx)
                        && (self.regions[target_idx].is_empty()
                            || self.regions[target_idx].is_regular())
                    {
                        if let Some(to_addr) = self.regions[target_idx].allocate(live_bytes, seq) {
                            self.barrier_set
                                .record_forwarding(from_addr, to_addr as u64);
                            break;
                        }
                    }
                }
            }
        }
        self.barrier_set.set_evacuation_in_progress(false);

        // 5. Init Update Refs (Pause)
        self.current_phase = ShenandoahPhase::InitUpdateRefs;
        self.barrier_set.set_update_refs_in_progress(true);

        // 6. Concurrent Update Refs
        self.current_phase = ShenandoahPhase::ConcurrentUpdateRefs;
        // In real execution, updates pointers using LRB / forwarding table
        self.barrier_set.set_update_refs_in_progress(false);

        // 7. Final Update Refs (Pause) - Recycle CSet
        self.current_phase = ShenandoahPhase::FinalUpdateRefs;
        let mut reclaimed = 0;
        for &idx in &cset_indices {
            let freed = self.regions[idx].used();
            self.regions[idx].make_trash();
            self.regions[idx].recycle();
            reclaimed += freed;
        }
        self.bytes_reclaimed += reclaimed;
        self.collection_set.clear();
        self.barrier_set.clear_cset_addresses();
        self.barrier_set.clear_forwarding();

        self.current_phase = ShenandoahPhase::Idle;
        Ok(())
    }
}

impl CollectedHeap for ShenandoahHeap {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Shenandoah
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn used(&self) -> usize {
        self.regions.iter().map(|r| r.used()).sum()
    }

    fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    fn collect(&mut self, cause: GCCause) -> io::Result<()> {
        self.run_gc_cycle(cause)
    }
}
