use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io::{self, Error};

pub struct EpsilonHeap {
    capacity: usize,
    used: usize,
    alloc_count: usize,
}

impl EpsilonHeap {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            used: 0,
            alloc_count: 0,
        }
    }

    pub fn allocate(&mut self, size: usize) -> io::Result<usize> {
        if self.used + size > self.capacity {
            return Err(Error::other("OutOfMemoryError: Epsilon heap exhausted"));
        }
        let addr = self.used;
        self.used += size;
        self.alloc_count += 1;
        Ok(addr)
    }

    pub fn alloc_count(&self) -> usize {
        self.alloc_count
    }
}

impl CollectedHeap for EpsilonHeap {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Epsilon
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn used(&self) -> usize {
        self.used
    }

    fn max_capacity(&self) -> usize {
        self.capacity
    }

    fn collect(&mut self, _cause: GCCause) -> io::Result<()> {
        Ok(())
    }
}
