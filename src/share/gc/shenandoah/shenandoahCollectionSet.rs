/*
 * Port of OpenJDK 25 HotSpot ShenandoahCollectionSet
 * (share/gc/shenandoah/shenandoahCollectionSet.hpp / shenandoahCollectionSet.cpp /
 *  shenandoahCollectionSet.inline.hpp)
 *
 * Region-indexed membership map biased so that an arbitrary heap
 * address can be tested with a single indexed load, plus evacuation
 * accounting (garbage/live/used, young/old split).
 */

use super::shenandoahHeapRegion::{ShenandoahAffiliation, ShenandoahHeapRegion};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct ShenandoahCollectionSet {
    region_size_bytes_shift: usize,
    heap_base_shift: usize,
    cset_map: Vec<u8>,
    has_old_regions: bool,
    garbage: usize,
    used: usize,
    live: usize,
    region_count: usize,
    young_bytes_to_evacuate: usize,
    young_bytes_to_promote: usize,
    old_bytes_to_evacuate: usize,
    old_garbage: usize,
    young_available_bytes_collected: usize,
    current_index: AtomicUsize,
}

impl ShenandoahCollectionSet {
    pub fn new(heap_base: usize, heap_end: usize, num_regions: usize) -> Self {
        let shift = ShenandoahHeapRegion::REGION_SIZE_BYTES_SHIFT;
        let map_len = (heap_end >> shift) + 1;
        Self {
            region_size_bytes_shift: shift,
            heap_base_shift: heap_base >> shift,
            cset_map: vec![0; map_len],
            has_old_regions: false,
            garbage: 0,
            used: 0,
            live: 0,
            region_count: 0,
            young_bytes_to_evacuate: 0,
            young_bytes_to_promote: 0,
            old_bytes_to_evacuate: 0,
            old_garbage: 0,
            young_available_bytes_collected: 0,
            current_index: AtomicUsize::new(0),
        }
    }

    pub fn add_region(&mut self, region: &ShenandoahHeapRegion) {
        let idx = region.index();
        debug_assert!(!self.is_in(idx), "already in collection set");
        debug_assert!(!region.is_humongous(), "only regular regions in cset");
        let map_idx = self.heap_base_shift + idx;
        self.cset_map[map_idx] = 1;

        let live = region.live_data_bytes();
        let garbage = region.garbage();
        let free = region.free();
        match region.affiliation() {
            ShenandoahAffiliation::Young => {
                self.young_bytes_to_evacuate += live;
                self.young_available_bytes_collected += free;
                if region.age() >= SHENANDOAH_TENURE_AGE_THRESHOLD {
                    self.young_bytes_to_promote += live;
                }
            }
            ShenandoahAffiliation::Old => {
                self.old_bytes_to_evacuate += live;
                self.old_garbage += garbage;
            }
            ShenandoahAffiliation::NonGen => {}
        }

        self.region_count += 1;
        self.has_old_regions |= region.is_old();
        self.garbage += garbage;
        self.used += region.used();
        self.live += live;
    }

    pub fn clear(&mut self) {
        for b in self.cset_map.iter_mut() {
            *b = 0;
        }
        self.garbage = 0;
        self.old_garbage = 0;
        self.used = 0;
        self.live = 0;
        self.region_count = 0;
        self.current_index.store(0, Ordering::Relaxed);
        self.young_bytes_to_evacuate = 0;
        self.young_bytes_to_promote = 0;
        self.old_bytes_to_evacuate = 0;
        self.young_available_bytes_collected = 0;
        self.has_old_regions = false;
    }

    pub fn clear_current_index(&self) {
        self.current_index.store(0, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_in(&self, region_idx: usize) -> bool {
        let map_idx = self.heap_base_shift + region_idx;
        map_idx < self.cset_map.len() && self.cset_map[map_idx] == 1
    }

    #[inline]
    pub fn is_in_addr(&self, addr: usize) -> bool {
        let index = addr >> self.region_size_bytes_shift;
        index < self.cset_map.len() && self.cset_map[index] == 1
    }

    #[inline]
    pub fn is_in_loc(&self, loc: usize) -> bool {
        self.is_in_addr(loc)
    }

    pub fn claim_next(&self, num_regions: usize, region_at: impl Fn(usize) -> Option<usize>) -> Option<usize> {
        let max = num_regions;
        let mut old = self.current_index.load(Ordering::Relaxed);
        loop {
            if old >= max {
                return None;
            }
            let mut index = old;
            let mut found = None;
            while index < max {
                if self.is_in(index) {
                    found = Some(index);
                    break;
                }
                index += 1;
            }
            let Some(hit) = found else {
                return None;
            };
            match self.current_index.compare_exchange_weak(
                old,
                hit + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return region_at(hit),
                Err(cur) => {
                    debug_assert!(cur >= old);
                    old = cur;
                }
            }
        }
    }

    pub fn next(&self, num_regions: usize) -> Option<usize> {
        let mut index = self.current_index.load(Ordering::Relaxed);
        while index < num_regions {
            if self.is_in(index) {
                self.current_index.store(index + 1, Ordering::Relaxed);
                return Some(index);
            }
            index += 1;
        }
        None
    }

    pub fn count(&self) -> usize {
        self.region_count
    }

    pub fn is_empty(&self) -> bool {
        self.region_count == 0
    }

    pub fn garbage(&self) -> usize {
        self.garbage
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn live(&self) -> usize {
        self.live
    }

    pub fn has_old_regions(&self) -> bool {
        self.has_old_regions
    }

    pub fn get_live_bytes_in_old_regions(&self) -> usize {
        self.old_bytes_to_evacuate
    }

    pub fn get_live_bytes_in_tenurable_regions(&self) -> usize {
        self.young_bytes_to_promote
    }

    pub fn get_live_bytes_in_untenurable_regions(&self) -> usize {
        self.young_bytes_to_evacuate.saturating_sub(self.young_bytes_to_promote)
    }

    pub fn get_old_garbage(&self) -> usize {
        self.old_garbage
    }

    pub fn young_available_bytes_collected(&self) -> usize {
        self.young_available_bytes_collected
    }
}

const SHENANDOAH_TENURE_AGE_THRESHOLD: u8 = 15;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_set_membership_and_accounting() {
        let heap_base = 0x1_0000_0000usize;
        let region_size = ShenandoahHeapRegion::DEFAULT_REGION_SIZE;
        let mut set = ShenandoahCollectionSet::new(heap_base, heap_base + 8 * region_size, 8);
        let mut regions: Vec<ShenandoahHeapRegion> = (0..8)
            .map(|i| {
                ShenandoahHeapRegion::new_affiliated(
                    i,
                    heap_base + i * region_size,
                    region_size,
                    ShenandoahAffiliation::NonGen,
                    true,
                )
            })
            .collect();
        for r in regions.iter_mut().take(3) {
            r.allocate(1024, 1);
            r.set_live_data(512);
        }
        assert!(set.is_empty());
        set.add_region(&regions[1]);
        set.add_region(&regions[2]);
        assert_eq!(set.count(), 2);
        assert!(set.is_in(1));
        assert!(set.is_in(2));
        assert!(!set.is_in(0));
        assert!(set.is_in_addr(heap_base + region_size + 16));
        assert!(!set.is_in_addr(heap_base + 4 * region_size));
        assert_eq!(set.garbage(), 2 * (1024 - 512));
        assert_eq!(set.live(), 2 * 512);
        assert_eq!(set.used(), 2 * 1024);

        assert_eq!(set.next(8), Some(1));
        assert_eq!(set.next(8), Some(2));
        assert_eq!(set.next(8), None);
        set.clear_current_index();
        set.clear();
        assert!(set.is_empty());
        assert!(!set.is_in(1));
    }

    #[test]
    fn claim_next_walks_regions() {
        let heap_base = 0usize;
        let region_size = ShenandoahHeapRegion::DEFAULT_REGION_SIZE;
        let set = ShenandoahCollectionSet::new(heap_base, 8 * region_size, 8);
        let mut set = set;
        let mut regions: Vec<ShenandoahHeapRegion> = (0..8)
            .map(|i| ShenandoahHeapRegion::new(i, i * region_size, region_size))
            .collect();
        for r in regions.iter_mut() {
            r.allocate(64, 1);
        }
        set.add_region(&regions[2]);
        set.add_region(&regions[5]);
        set.add_region(&regions[7]);
        let claimed: Vec<usize> = {
            let mut v = Vec::new();
            loop {
                match set.claim_next(8, |i| Some(i)) {
                    Some(i) => v.push(i),
                    None => break,
                }
            }
            v
        };
        assert_eq!(claimed, vec![2, 5, 7]);
    }
}
