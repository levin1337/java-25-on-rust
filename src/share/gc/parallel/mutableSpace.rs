/*
 * Port of OpenJDK 25 HotSpot MutableSpace (share/gc/parallel/mutableSpace.hpp)
 *
 * Contiguous heap memory space used for young and old spaces in Parallel GC.
 */

#[derive(Debug, Clone)]
pub struct MutableSpace {
    bottom: usize,
    top: usize,
    end: usize,
}

impl MutableSpace {
    pub fn new(bottom: usize, size: usize) -> Self {
        Self {
            bottom,
            top: bottom,
            end: bottom + size,
        }
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
    pub fn capacity(&self) -> usize {
        self.end - self.bottom
    }

    #[inline]
    pub fn used(&self) -> usize {
        self.top - self.bottom
    }

    #[inline]
    pub fn free(&self) -> usize {
        self.end - self.top
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

    pub fn clear(&mut self) {
        self.top = self.bottom;
    }
}
