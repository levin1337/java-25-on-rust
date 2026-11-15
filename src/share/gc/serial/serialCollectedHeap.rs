use crate::share::classfile::classLoader::ClassLoader;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use crate::share::memory::heap::{self, HeapGcMode};
use std::io;
use std::sync::Arc;

/// SerialGC as a `CollectedHeap`: a single-threaded full-heap
/// mark-sweep over the loader's object arenas. HotSpot SerialGC is
/// non-generational, so every collection cause runs the same complete
/// cycle; `DefNewGeneration`/`TenuredGeneration` below only track the
/// configured capacity split for ergonomics reporting.
///
/// The collector owns an `Arc` to the JVM `ClassLoader` so it is
/// `'static` and can be stored as `Box<dyn CollectedHeap + Send>` in
/// `Universe`.
pub struct SerialCollectedHeap {
    loader: Arc<ClassLoader<'static>>,
    young: super::defNewGeneration::DefNewGeneration,
    old: super::tenuredGeneration::TenuredGeneration,
    young_gc_count: usize,
    full_gc_count: usize,
}

impl SerialCollectedHeap {
    pub fn new(loader: Arc<ClassLoader<'static>>, young_size: usize, old_size: usize) -> Self {
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
    pub fn loader(&self) -> &ClassLoader<'static> {
        &self.loader
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
        heap::collect_with_mode(self.loader.as_ref(), HeapGcMode::SerialMarkSweep)
    }

    pub fn do_young_gc(&mut self) -> io::Result<()> {
        self.young_gc_count += 1;
        heap::collect_with_mode(self.loader.as_ref(), HeapGcMode::SerialMarkSweep)
    }
}

impl CollectedHeap for SerialCollectedHeap {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::classfile::{ClassFile, Member};
    use crate::share::interpreter::bytecodeInterpreter::Value;
    use crate::share::oops::instanceKlass::InstanceKlass;
    use crate::share::oops::instanceOop::InstanceOop;
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    use crate::share::runtime::syncBump::SyncBump;
    use std::path::Path;

    fn test_loader() -> Arc<ClassLoader<'static>> {
        let arena: &'static SyncBump = Box::leak(Box::new(SyncBump::new()));
        Arc::new(ClassLoader::open(Path::new("."), arena).unwrap())
    }

    fn test_heap(loader: &Arc<ClassLoader<'static>>) -> SerialCollectedHeap {
        SerialCollectedHeap::new(Arc::clone(loader), 8 << 20, 32 << 20)
    }

    /// Runs `heap.collect(cause)` until `done` observes the post-GC
    /// state. A collection cycle can be skipped when a sibling test
    /// briefly holds an arena borrow (`try_borrow` fails), so tests
    /// retry instead of asserting after a single attempt.
    fn collect_until(
        heap: &mut SerialCollectedHeap,
        cause: GCCause,
        mut done: impl FnMut(&ClassLoader<'static>) -> bool,
    ) {
        for _ in 0..100 {
            heap.collect(cause).unwrap();
            if done(heap.loader()) {
                return;
            }
        }
        panic!("collection did not reach the expected state");
    }

    /// Allocation-triggered path parity: a full GC through the
    /// collector sweeps unreachable arena slots, retains GC roots and
    /// returns freed slots to the allocation free lists.
    #[test]
    fn fullGcSweepsDeadSlotsRetainsRootsAndReusesSlots() {
        let _serial = crate::share::memory::heap::GC_TEST_SERIAL.lock();
        let loader = test_loader();
        let mut heap = test_heap(&loader);

        let dead = loader.alloc_type_array(TypeArrayOop::Byte(vec![1; 4096]));
        let kept = loader.alloc_type_array(TypeArrayOop::Byte(vec![2; 4096]));
        loader
            .gc_extra_roots
            .borrow_mut()
            .push(Value::PrimitiveArray(kept));

        let arena_len_before = loader.arrays.borrow().len();
        collect_until(&mut heap, GCCause::JavaLangSystemGC, |loader| {
            loader.arrays.borrow()[dead].len() == 0
        });
        assert!(heap.full_gc_count() >= 1);
        assert_eq!(heap.young_gc_count(), 0);

        // Dead slot swept, rooted slot retained.
        assert_eq!(loader.arrays.borrow()[dead].len(), 0);
        assert_eq!(loader.arrays.borrow()[kept].len(), 4096);

        // Sweep reuse: the next allocation pops a freed slot instead of
        // growing the arena.
        let reused = loader.alloc_type_array(TypeArrayOop::Int(vec![7]));
        assert!(reused < arena_len_before);
        assert_eq!(loader.arrays.borrow()[reused].len(), 1);
    }

    /// HotSpot SerialGC is non-generational: a young-generation cause
    /// runs the same complete mark-sweep cycle.
    #[test]
    fn youngGcRunsFullMarkSweep() {
        let _serial = crate::share::memory::heap::GC_TEST_SERIAL.lock();
        let loader = test_loader();
        let mut heap = test_heap(&loader);

        let dead = loader.alloc_type_array(TypeArrayOop::Byte(vec![3; 2048]));
        collect_until(&mut heap, GCCause::ScavengeAlot, |loader| {
            loader.arrays.borrow()[dead].len() == 0
        });
        assert!(heap.young_gc_count() >= 1);
        assert_eq!(heap.full_gc_count(), 0);
        assert_eq!(loader.arrays.borrow()[dead].len(), 0);
    }

    /// ReferenceProcessor semantics: a live `java.lang.ref.Reference`
    /// whose referent did not survive is linked onto the VM pending
    /// list through `discovered` and its `referent` field is cleared.
    #[test]
    fn deadReferentIsClearedAndPending() {
        let _serial = crate::share::memory::heap::GC_TEST_SERIAL.lock();
        let loader = test_loader();
        let mut heap = test_heap(&loader);

        let reference_klass = Arc::new(
            InstanceKlass::new(
                ClassFile {
                    majorVersion: 65,
                    minorVersion: 0,
                    accessFlags: 0x0001,
                    name: b"java/lang/ref/Reference",
                    superName: Some(b"java/lang/Object"),
                    interfaces: Vec::new(),
                    constants: Vec::new(),
                    fields: vec![
                        Member {
                            accessFlags: 0,
                            name: b"referent",
                            descriptor: b"Ljava/lang/Object;",
                            attributes: Vec::new(),
                        },
                        Member {
                            accessFlags: 0,
                            name: b"discovered",
                            descriptor: b"Ljava/lang/ref/Reference;",
                            attributes: Vec::new(),
                        },
                    ],
                    methods: Vec::new(),
                    attributes: Vec::new(),
                },
                None,
                Vec::new(),
            )
            .unwrap(),
        );

        let referent = loader.alloc_type_array(TypeArrayOop::Byte(vec![9; 2048]));
        let reference = loader.alloc_object(InstanceOop {
            klass: reference_klass,
            fields: vec![Value::PrimitiveArray(referent), Value::Null],
        });
        loader
            .gc_extra_roots
            .borrow_mut()
            .push(Value::Object(reference));

        collect_until(&mut heap, GCCause::JavaLangSystemGC, |loader| {
            loader.arrays.borrow()[referent].len() == 0
                && loader.borrow_objects()[reference].fields[0] == Value::Null
                && *loader.reference_pending.borrow() == Some(reference)
        });

        // The reference object itself survived (it is a GC root); its
        // referent did not, so `referent` was cleared, the referent's
        // arena slot was swept and the reference heads the pending list.
        let objects = loader.borrow_objects();
        assert_eq!(objects[reference].fields[0], Value::Null);
        drop(objects);
        assert_eq!(*loader.reference_pending.borrow(), Some(reference));
        assert_eq!(loader.arrays.borrow()[referent].len(), 0);
    }
}
