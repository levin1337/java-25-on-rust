use crate::share::classfile::classLoader::ClassLoader;
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

/// EpsilonGC bound to the loader's arenas: the only legal operation is
/// allocation; `collect` is a no-op by definition (HotSpot Epsilon
/// throws OutOfMemoryError when the -Xmx budget is exhausted instead
/// of collecting — the slot arenas have no byte budget, so exhaustion
/// surfaces as arena growth).
pub struct EpsilonCollectedHeap<'l, 'a> {
    loader: &'l ClassLoader<'a>,
    max_capacity: usize,
    alloc_count: usize,
}

impl<'l, 'a> EpsilonCollectedHeap<'l, 'a> {
    pub fn new(loader: &'l ClassLoader<'a>, max_capacity: usize) -> Self {
        Self {
            loader,
            max_capacity,
            alloc_count: 0,
        }
    }

    #[inline]
    pub fn loader(&self) -> &'l ClassLoader<'a> {
        self.loader
    }

    #[inline]
    pub fn alloc_count(&self) -> usize {
        self.alloc_count
    }

    pub fn note_allocation(&mut self) {
        self.alloc_count += 1;
    }

    pub fn arena_used(&self) -> usize {
        let objects = self.loader.objects.try_borrow();
        let arrays = self.loader.arrays.try_borrow();
        let obj_arrays = self.loader.obj_arrays.try_borrow();
        match (objects, arrays, obj_arrays) {
            (Some(objects), Some(arrays), Some(obj_arrays)) => {
                objects.len() + arrays.len() + obj_arrays.len()
            }
            _ => 0,
        }
    }
}

impl CollectedHeap for EpsilonCollectedHeap<'_, '_> {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Epsilon
    }

    fn capacity(&self) -> usize {
        self.max_capacity
    }

    fn used(&self) -> usize {
        self.arena_used()
    }

    fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    fn collect(&mut self, _cause: GCCause) -> io::Result<()> {
        Ok(())
    }
}
