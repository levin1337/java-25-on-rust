use crate::share::classfile::classLoader::ClassLoader;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use crate::share::memory::heap::{self, HeapGcMode};
use std::io;

/// SerialGC as a `CollectedHeap`: a single-threaded full-heap
/// mark-sweep over the loader's object arenas. HotSpot SerialGC is
/// non-generational, so every collection cause runs the same complete
/// cycle; `DefNewGeneration`/`TenuredGeneration` below only track the
/// configured capacity split for ergonomics reporting.
pub struct SerialCollectedHeap<'l, 'a> {
    loader: &'l ClassLoader<'a>,
    young: super::defNewGeneration::DefNewGeneration,
    old: super::tenuredGeneration::TenuredGeneration,
    young_gc_count: usize,
    full_gc_count: usize,
}

impl<'l, 'a> SerialCollectedHeap<'l, 'a> {
    pub fn new(loader: &'l ClassLoader<'a>, young_size: usize, old_size: usize) -> Self {
        Self {
            loader,
            young: super::defNewGeneration::DefNewGeneration::new(
                young_size * 8 / 10,
                young_size / 10,
            ),
            old: super::tenuredGeneration::TenuredGeneration::new(old_size),
            young_gc_count: 0,
            full_gc_count: 0,
        }
    }

    #[inline]
    pub fn loader(&self) -> &'l ClassLoader<'a> {
        self.loader
    }

    #[inline]
    pub fn young_gc_count(&self) -> usize {
        self.young_gc_count
    }

    #[inline]
    pub fn full_gc_count(&self) -> usize {
        self.full_gc_count
    }

    /// Arena occupancy in object slots (objects + primitive arrays +
    /// object arrays), the real heap usage of this collector.
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

    pub fn do_full_gc(&mut self) -> io::Result<()> {
        self.full_gc_count += 1;
        heap::collect_with_mode(self.loader, HeapGcMode::SerialMarkSweep)
    }

    pub fn do_young_gc(&mut self) -> io::Result<()> {
        self.young_gc_count += 1;
        heap::collect_with_mode(self.loader, HeapGcMode::SerialMarkSweep)
    }
}

impl CollectedHeap for SerialCollectedHeap<'_, '_> {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Serial
    }

    fn capacity(&self) -> usize {
        self.young.capacity() + self.old.capacity()
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
            _ => self.do_young_gc(),
        }
    }
}
