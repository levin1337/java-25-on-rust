use crate::share::classfile::classLoader::ClassLoader;
use crate::share::gc::parallel::mutableSpace::MutableSpace;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use crate::share::memory::heap::{self, HeapGcMode};
use std::io;

/// ParallelGC (Throughput collector) bound to the loader's arenas.
/// Collections run the parallel engine in `memory::heap`: a
/// work-stealing parallel marker and a range-partitioned parallel
/// sweeper across `workers` OS threads — the ParallelScavenge
/// old-generation full-GC path. Young-generation scavenges are not
/// representable on the non-moving slot arenas, so every cause performs
/// a complete parallel mark-sweep, matching what ParallelGC does for
/// full collections.
pub struct ParallelCollectedHeap<'l, 'a> {
    loader: &'l ClassLoader<'a>,
    eden: MutableSpace,
    old: MutableSpace,
    workers: usize,
    scavenge_count: usize,
    full_gc_count: usize,
}

impl<'l, 'a> ParallelCollectedHeap<'l, 'a> {
    pub fn new(
        loader: &'l ClassLoader<'a>,
        young_size: usize,
        old_size: usize,
        workers: usize,
    ) -> Self {
        Self {
            loader,
            eden: MutableSpace::new(0, young_size),
            old: MutableSpace::new(young_size, old_size),
            workers: workers.max(1),
            scavenge_count: 0,
            full_gc_count: 0,
        }
    }

    #[inline]
    pub fn loader(&self) -> &'l ClassLoader<'a> {
        self.loader
    }

    #[inline]
    pub fn workers(&self) -> usize {
        self.workers
    }

    #[inline]
    pub fn scavenge_count(&self) -> usize {
        self.scavenge_count
    }

    #[inline]
    pub fn full_gc_count(&self) -> usize {
        self.full_gc_count
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

    pub fn do_scavenge(&mut self) -> io::Result<()> {
        self.scavenge_count += 1;
        self.collect_mark_sweep()
    }

    pub fn do_full_gc(&mut self) -> io::Result<()> {
        self.full_gc_count += 1;
        self.collect_mark_sweep()
    }

    fn collect_mark_sweep(&mut self) -> io::Result<()> {
        heap::collect_with_workers(self.loader, HeapGcMode::ParallelMarkSweep, self.workers)
    }
}

impl CollectedHeap for ParallelCollectedHeap<'_, '_> {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Parallel
    }

    fn capacity(&self) -> usize {
        self.eden.capacity() + self.old.capacity()
    }

    fn used(&self) -> usize {
        self.arena_used()
    }

    fn max_capacity(&self) -> usize {
        self.capacity()
    }

    fn collect(&mut self, cause: GCCause) -> io::Result<()> {
        match cause {
            GCCause::JavaLangSystemGC
            | GCCause::FullGCAlot
            | GCCause::TenuredGenerationFull
            | GCCause::OldGenerationTooFullToScavenge => self.do_full_gc(),
            _ => self.do_scavenge(),
        }
    }
}
