use crate::share::classfile::classLoader::ClassLoader;
use crate::share::gc::parallel::mutableSpace::MutableSpace;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use crate::share::memory::heap::{self, HeapGcMode};
use std::io;
use std::sync::Arc;

/// ParallelGC (Throughput collector) bound to the loader's arenas.
/// Collections run the parallel engine in `memory::heap`: a
/// work-stealing parallel marker and a range-partitioned parallel
/// sweeper across `workers` OS threads — the ParallelScavenge
/// old-generation full-GC path. Young-generation scavenges are not
/// representable on the non-moving slot arenas, so every cause performs
/// a complete parallel mark-sweep, matching what ParallelGC does for
/// full collections.
///
/// The collector owns an `Arc` to the JVM `ClassLoader` so it is
/// `'static` and can be stored as `Box<dyn CollectedHeap + Send>` in
/// `Universe`.
pub struct ParallelCollectedHeap {
    loader: Arc<ClassLoader<'static>>,
    eden: MutableSpace,
    old: MutableSpace,
    workers: usize,
    scavenge_count: usize,
    full_gc_count: usize,
}

impl ParallelCollectedHeap {
    pub fn new(
        loader: Arc<ClassLoader<'static>>,
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
    pub fn loader(&self) -> &ClassLoader<'static> {
        &self.loader
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
        heap::collect_with_workers(
            self.loader.as_ref(),
            HeapGcMode::ParallelMarkSweep,
            self.workers,
        )
    }
}

impl CollectedHeap for ParallelCollectedHeap {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::interpreter::bytecodeInterpreter::Value;
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    use crate::share::runtime::syncBump::SyncBump;
    use std::path::Path;

    fn test_loader() -> Arc<ClassLoader<'static>> {
        let arena: &'static SyncBump = Box::leak(Box::new(SyncBump::new()));
        Arc::new(ClassLoader::open(Path::new("."), arena).unwrap())
    }

    /// ParallelGC workers: a scavenge cause drives the work-stealing
    /// parallel marker and the range-partitioned parallel sweeper
    /// across the configured worker count; unreachable slots are freed
    /// and GC roots retained exactly like the serial engine.
    #[test]
    fn parallelScavengeSweepsWithWorkers() {
        let _serial = crate::share::memory::heap::GC_TEST_SERIAL.lock();
        let loader = test_loader();
        let mut heap = ParallelCollectedHeap::new(
            Arc::clone(&loader),
            8 << 20,
            32 << 20,
            4,
        );
        assert_eq!(heap.workers(), 4);

        // Enough live objects for the parallel marker to share work.
        let mut dead = Vec::new();
        for i in 0..64i8 {
            dead.push(loader.alloc_type_array(TypeArrayOop::Byte(vec![i; 1024])));
        }
        let kept = loader.alloc_type_array(TypeArrayOop::Byte(vec![-1i8; 4096]));
        loader
            .gc_extra_roots
            .borrow_mut()
            .push(Value::PrimitiveArray(kept));

        for _ in 0..100 {
            heap.collect(GCCause::ScavengeAlot).unwrap();
            if loader.arrays.borrow()[dead[0]].len() == 0 {
                break;
            }
        }
        assert!(heap.scavenge_count() >= 1);
        assert_eq!(heap.full_gc_count(), 0);
        for &handle in &dead {
            assert_eq!(loader.arrays.borrow()[handle].len(), 0);
        }
        assert_eq!(loader.arrays.borrow()[kept].len(), 4096);

        // Parallel sweep feeds the same free lists: after a full GC the
        // next allocation reuses a swept slot instead of growing the
        // arena.
        let arena_len_before = loader.arrays.borrow().len();
        heap.collect(GCCause::JavaLangSystemGC).unwrap();
        let reused = loader.alloc_type_array(TypeArrayOop::Int(vec![1, 2, 3]));
        assert!(reused < arena_len_before);
        assert_eq!(loader.arrays.borrow()[reused].len(), 3);
        assert!(heap.full_gc_count() >= 1);
    }
}
