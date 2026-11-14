/*
 * Port of OpenJDK 25 HotSpot PSOldGen (share/gc/parallel/psOldGen.hpp)
 *
 * Parallel Scavenge Old (Tenured) Generation.
 */

use super::mutableSpace::MutableSpace;

#[derive(Debug)]
pub struct PSOldGen {
    object_space: MutableSpace,
}

impl PSOldGen {
    pub fn new(base: usize, size: usize) -> Self {
        Self {
            object_space: MutableSpace::new(base, size),
        }
    }

    #[inline]
    pub fn object_space(&self) -> &MutableSpace {
        &self.object_space
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.object_space.capacity()
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.object_space.used()
    }

    pub fn allocate(&mut self, size: usize) -> Option<usize> {
        self.object_space.allocate(size)
    }

    pub fn clear(&mut self) {
        self.object_space.clear();
    }
}
