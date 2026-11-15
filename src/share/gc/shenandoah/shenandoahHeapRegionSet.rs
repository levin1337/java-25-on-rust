/*
 * Port of OpenJDK 25 HotSpot ShenandoahHeapRegionSet
 * (share/gc/shenandoah/shenandoahHeapRegionSet.hpp / shenandoahHeapRegionSet.cpp /
 *  shenandoahHeapRegionSet.inline.hpp)
 *
 * Byte-map membership set over region indices plus its iterator.
 */

pub struct ShenandoahHeapRegionSet {
    map_size: usize,
    set_map: Vec<u8>,
    region_count: usize,
}

impl ShenandoahHeapRegionSet {
    pub fn new(map_size: usize) -> Self {
        Self {
            map_size,
            set_map: vec![0; map_size],
            region_count: 0,
        }
    }

    pub fn add_region(&mut self, region_idx: usize) {
        debug_assert!(!self.is_in(region_idx), "already in region set");
        self.set_map[region_idx] = 1;
        self.region_count += 1;
    }

    pub fn remove_region(&mut self, region_idx: usize) {
        debug_assert!(self.is_in(region_idx), "not in region set");
        self.set_map[region_idx] = 0;
        self.region_count -= 1;
    }

    pub fn clear(&mut self) {
        self.set_map.fill(0);
        self.region_count = 0;
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.region_count
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.region_count == 0
    }

    #[inline]
    pub fn is_in(&self, region_idx: usize) -> bool {
        region_idx < self.map_size && self.set_map[region_idx] == 1
    }
}

pub struct ShenandoahHeapRegionSetIterator<'a> {
    set: &'a ShenandoahHeapRegionSet,
    num_regions: usize,
    current_index: usize,
}

impl<'a> ShenandoahHeapRegionSetIterator<'a> {
    pub fn new(set: &'a ShenandoahHeapRegionSet, num_regions: usize) -> Self {
        Self {
            set,
            num_regions,
            current_index: 0,
        }
    }

    pub fn next(&mut self) -> Option<usize> {
        while self.current_index < self.num_regions {
            let index = self.current_index;
            self.current_index += 1;
            if self.set.is_in(index) {
                return Some(index);
            }
        }
        None
    }
}

impl<'a> Iterator for ShenandoahHeapRegionSetIterator<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        ShenandoahHeapRegionSetIterator::next(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_set_membership() {
        let mut set = ShenandoahHeapRegionSet::new(16);
        assert!(set.is_empty());
        set.add_region(3);
        set.add_region(7);
        assert_eq!(set.count(), 2);
        assert!(set.is_in(3));
        assert!(set.is_in(7));
        assert!(!set.is_in(4));
        set.remove_region(3);
        assert_eq!(set.count(), 1);
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn iterator_walks_set() {
        let mut set = ShenandoahHeapRegionSet::new(16);
        set.add_region(1);
        set.add_region(5);
        set.add_region(9);
        let it = ShenandoahHeapRegionSetIterator::new(&set, 16);
        let collected: Vec<usize> = it.collect();
        assert_eq!(collected, vec![1, 5, 9]);
    }
}
