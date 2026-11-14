/*
 * Port of OpenJDK 25 HotSpot PSYoungGen (share/gc/parallel/psYoungGen.hpp)
 *
 * Parallel Scavenge Young Generation (Eden + From + To survivor spaces).
 */

use super::mutableSpace::MutableSpace;

#[derive(Debug)]
pub struct PSYoungGen {
    eden_space: MutableSpace,
    from_space: MutableSpace,
    to_space: MutableSpace,
}

impl PSYoungGen {
    pub fn new(base: usize, total_young: usize) -> Self {
        // Standard HotSpot young ratio: 8:1:1
        let survivor_size = total_young / 10;
        let eden_size = total_young - 2 * survivor_size;

        let eden = MutableSpace::new(base, eden_size);
        let from = MutableSpace::new(base + eden_size, survivor_size);
        let to = MutableSpace::new(base + eden_size + survivor_size, survivor_size);

        Self {
            eden_space: eden,
            from_space: from,
            to_space: to,
        }
    }

    #[inline]
    pub fn eden_space(&self) -> &MutableSpace {
        &self.eden_space
    }

    #[inline]
    pub fn from_space(&self) -> &MutableSpace {
        &self.from_space
    }

    #[inline]
    pub fn to_space(&self) -> &MutableSpace {
        &self.to_space
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.eden_space.capacity() + self.from_space.capacity() + self.to_space.capacity()
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.eden_space.used() + self.from_space.used() + self.to_space.used()
    }

    pub fn allocate(&mut self, size: usize) -> Option<usize> {
        self.eden_space.allocate(size)
    }

    pub fn reset_young(&mut self) {
        self.eden_space.clear();
        self.from_space.clear();
        self.to_space.clear();
    }
}
