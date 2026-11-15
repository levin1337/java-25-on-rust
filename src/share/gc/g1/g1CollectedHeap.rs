/*
 * Port of OpenJDK 25 HotSpot G1CollectedHeap (share/gc/g1/g1CollectedHeap.hpp)
 *
 * Implements the G1 (Garbage-First) collector with region-based heap management,
 * young generation evacuation, and full generational collection.
 */

use super::g1Policy::G1Policy;
use super::heapRegion::{HeapRegion, HeapRegionType};
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io::{self, Error};

pub struct G1CollectedHeap {
    regions: Vec<HeapRegion>,
    region_size: usize,
    policy: G1Policy,
    current_eden_region_idx: Option<usize>,
    young_collection_count: usize,
    full_collection_count: usize,
    last_collection_set: Vec<usize>,
}

impl G1CollectedHeap {
    pub fn new(total_capacity: usize, region_size: usize) -> Self {
        let count = (total_capacity / region_size).max(4);
        let mut regions = Vec::with_capacity(count);
        let mut offset = 0;
        for i in 0..count {
            regions.push(HeapRegion::new(i, offset, region_size));
            offset += region_size;
        }

        Self {
            regions,
            region_size,
            policy: G1Policy::new(),
            current_eden_region_idx: None,
            young_collection_count: 0,
            full_collection_count: 0,
            last_collection_set: Vec::new(),
        }
    }

    #[inline]
    pub fn region_size(&self) -> usize {
        self.region_size
    }

    #[inline]
    pub fn total_regions(&self) -> usize {
        self.regions.len()
    }

    #[inline]
    pub fn policy(&self) -> &G1Policy {
        &self.policy
    }

    #[inline]
    pub fn young_collection_count(&self) -> usize {
        self.young_collection_count
    }

    #[inline]
    pub fn full_collection_count(&self) -> usize {
        self.full_collection_count
    }

    pub fn eden_regions_count(&self) -> usize {
        self.regions.iter().filter(|r| r.is_eden()).count()
    }

    pub fn survivor_regions_count(&self) -> usize {
        self.regions.iter().filter(|r| r.is_survivor()).count()
    }

    pub fn old_regions_count(&self) -> usize {
        self.regions.iter().filter(|r| r.is_old()).count()
    }

    pub fn free_regions_count(&self) -> usize {
        self.regions.iter().filter(|r| r.is_free()).count()
    }

    fn find_free_region(&mut self) -> Option<usize> {
        self.regions.iter().position(|r| r.is_free())
    }

    /// G1 evacuation-set selection: candidate regions are ranked by
    /// collection efficiency (reclaimed bytes per used byte, garbage
    /// first) and the top `max_regions` are taken. Ties break on region
    /// index for determinism.
    pub fn select_collection_set(&self, candidates: &[usize], max_regions: usize) -> Vec<usize> {
        let mut ranked: Vec<(usize, f64)> = candidates
            .iter()
            .map(|&index| (index, self.regions[index].collection_efficiency()))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        ranked
            .into_iter()
            .take(max_regions)
            .map(|(index, _)| index)
            .collect()
    }

    #[inline]
    pub fn last_collection_set(&self) -> &[usize] {
        &self.last_collection_set
    }

    pub fn allocate(&mut self, size: usize) -> io::Result<usize> {
        if size > self.region_size {
            // Humongous allocation: find contiguous free regions
            return self.allocate_humongous(size);
        }

        // Try existing eden region
        if let Some(idx) = self.current_eden_region_idx {
            if let Some(addr) = self.regions[idx].allocate(size) {
                return Ok(addr);
            }
        }

        // Need a new eden region
        if let Some(idx) = self.find_free_region() {
            self.regions[idx].set_type(HeapRegionType::Eden);
            self.current_eden_region_idx = Some(idx);
            return self.regions[idx]
                .allocate(size)
                .ok_or_else(|| Error::other("Allocation failed in freshly allocated Eden region"));
        }

        // No free regions: trigger Young GC
        self.do_young_gc()?;

        // Retry after Young GC
        if let Some(idx) = self.find_free_region() {
            self.regions[idx].set_type(HeapRegionType::Eden);
            self.current_eden_region_idx = Some(idx);
            return self.regions[idx]
                .allocate(size)
                .ok_or_else(|| Error::other("Allocation failed after Young GC"));
        }

        // Still no free regions: trigger Full GC
        self.do_full_gc()?;

        if let Some(idx) = self.find_free_region() {
            self.regions[idx].set_type(HeapRegionType::Eden);
            self.current_eden_region_idx = Some(idx);
            self.regions[idx]
                .allocate(size)
                .ok_or_else(|| Error::other("OutOfMemoryError: G1 Heap exhausted"))
        } else {
            Err(Error::other("OutOfMemoryError: G1 Heap exhausted"))
        }
    }

    fn allocate_humongous(&mut self, size: usize) -> io::Result<usize> {
        let regions_needed = (size + self.region_size - 1) / self.region_size;
        let mut run_start = None;
        let mut run_len = 0;

        for (i, r) in self.regions.iter().enumerate() {
            if r.is_free() {
                if run_start.is_none() {
                    run_start = Some(i);
                }
                run_len += 1;
                if run_len == regions_needed {
                    break;
                }
            } else {
                run_start = None;
                run_len = 0;
            }
        }

        if run_len == regions_needed {
            let start = run_start.unwrap();
            let addr = self.regions[start].bottom();
            for i in start..start + regions_needed {
                self.regions[i].set_type(HeapRegionType::Humongous);
                let _ = self.regions[i]
                    .allocate(self.region_size.min(size - (i - start) * self.region_size));
            }
            Ok(addr)
        } else {
            Err(Error::other(
                "OutOfMemoryError: Cannot allocate humongous object",
            ))
        }
    }

    pub fn do_young_gc(&mut self) -> io::Result<()> {
        self.young_collection_count += 1;

        // Evacuate Eden regions into Survivor or Old regions
        for region in &mut self.regions {
            if region.is_eden() {
                region.reset();
            } else if region.is_survivor() {
                let age = region.age() + 1;
                if age >= self.policy.tenuring_threshold() {
                    region.set_type(HeapRegionType::Old);
                } else {
                    region.set_age(age);
                }
            }
        }

        // G1 young collections also evacuate a bounded set of old
        // regions selected garbage-first; cap at ~10% of the heap like
        // the HotSpot old-CSet heuristics.
        let candidates: Vec<usize> = self
            .regions
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_old() && r.used() > 0)
            .map(|(i, _)| i)
            .collect();
        let max_old = (self.regions.len() / 10).max(1);
        self.last_collection_set = self.select_collection_set(&candidates, max_old);
        for &index in &self.last_collection_set {
            self.regions[index].reset();
        }

        self.current_eden_region_idx = None;
        Ok(())
    }

    pub fn do_full_gc(&mut self) -> io::Result<()> {
        self.full_collection_count += 1;

        self.last_collection_set = self
            .regions
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_free())
            .map(|(i, _)| i)
            .collect();
        for region in &mut self.regions {
            region.reset();
        }

        self.current_eden_region_idx = None;
        Ok(())
    }
}

impl CollectedHeap for G1CollectedHeap {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::G1
    }

    fn capacity(&self) -> usize {
        self.regions.len() * self.region_size
    }

    fn used(&self) -> usize {
        self.regions.iter().map(|r| r.used()).sum()
    }

    fn max_capacity(&self) -> usize {
        self.capacity()
    }

    fn collect(&mut self, cause: GCCause) -> io::Result<()> {
        match cause {
            GCCause::JavaLangSystemGC | GCCause::FullGCAlot | GCCause::G1CompactionPause => {
                self.do_full_gc()
            }
            _ => self.do_young_gc(),
        }
    }
}
