/*
 * Port of OpenJDK 25 HotSpot HeapRegion (share/gc/g1/heapRegion.hpp)
 *
 * HeapRegion represents a fixed-size partition of the Java heap managed by G1.
 * Regions can be typed as Free, Eden, Survivor, Old, or Humongous.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapRegionType {
    Free,
    Eden,
    Survivor,
    Old,
    Humongous,
}

#[derive(Debug, Clone)]
pub struct HeapRegion {
    index: usize,
    bottom: usize,
    top: usize,
    end: usize,
    region_type: HeapRegionType,
    age: u32,
}

impl HeapRegion {
    pub const DEFAULT_REGION_SIZE: usize = 1024 * 1024; // 1MB

    pub fn new(index: usize, bottom: usize, size: usize) -> Self {
        Self {
            index,
            bottom,
            top: bottom,
            end: bottom + size,
            region_type: HeapRegionType::Free,
            age: 0,
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
    pub fn used(&self) -> usize {
        self.top - self.bottom
    }

    #[inline]
    pub fn free_space(&self) -> usize {
        self.end - self.top
    }

    #[inline]
    pub fn is_free(&self) -> bool {
        self.region_type == HeapRegionType::Free
    }

    #[inline]
    pub fn is_eden(&self) -> bool {
        self.region_type == HeapRegionType::Eden
    }

    #[inline]
    pub fn is_survivor(&self) -> bool {
        self.region_type == HeapRegionType::Survivor
    }

    #[inline]
    pub fn is_young(&self) -> bool {
        matches!(
            self.region_type,
            HeapRegionType::Eden | HeapRegionType::Survivor
        )
    }

    #[inline]
    pub fn is_old(&self) -> bool {
        self.region_type == HeapRegionType::Old
    }

    #[inline]
    pub fn is_humongous(&self) -> bool {
        self.region_type == HeapRegionType::Humongous
    }

    #[inline]
    pub fn region_type(&self) -> HeapRegionType {
        self.region_type
    }

    #[inline]
    pub fn set_type(&mut self, region_type: HeapRegionType) {
        self.region_type = region_type;
    }

    #[inline]
    pub fn age(&self) -> u32 {
        self.age
    }

    #[inline]
    pub fn set_age(&mut self, age: u32) {
        self.age = age;
    }

    pub fn allocate(&mut self, size: usize) -> Option<usize> {
        if self.top + size <= self.end {
            let addr = self.top;
            self.top += size;
            Some(addr)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.top = self.bottom;
        self.region_type = HeapRegionType::Free;
        self.age = 0;
    }
}
