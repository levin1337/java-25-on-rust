use crate::share::classfile::classLoader::ClassLoader;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io;
use std::sync::Arc;

/// EpsilonGC bound to the loader's arenas: the only legal operation is
/// allocation; `collect` is a no-op by definition (HotSpot Epsilon
/// throws OutOfMemoryError when the -Xmx budget is exhausted instead
/// of collecting — the slot arenas have no byte budget, so exhaustion
/// surfaces as arena growth).
///
/// The collector owns an `Arc` to the JVM `ClassLoader` so it is
/// `'static` and can be stored as `Box<dyn CollectedHeap + Send>` in
/// `Universe`.
pub struct EpsilonCollectedHeap {
    loader: Arc<ClassLoader<'static>>,
    max_capacity: usize,
    alloc_count: usize,
}

impl EpsilonCollectedHeap {
    pub fn new(loader: Arc<ClassLoader<'static>>, max_capacity: usize) -> Self {
        Self {
            loader,
            max_capacity,
            alloc_count: 0,
        }
    }

    #[inline]
    pub fn loader(&self) -> &ClassLoader<'static> {
        &self.loader
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

impl CollectedHeap for EpsilonCollectedHeap {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::gc::shared::gcCause::GCCause;
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    use crate::share::runtime::syncBump::SyncBump;
    use std::path::Path;

    fn test_loader() -> Arc<ClassLoader<'static>> {
        let arena: &'static SyncBump = Box::leak(Box::new(SyncBump::new()));
        Arc::new(ClassLoader::open(Path::new("."), arena).unwrap())
    }

    /// EpsilonGC never collects: `collect` is a no-op for every cause
    /// and unreachable arena slots are left untouched, matching
    /// HotSpot Epsilon which only allocates until -Xmx exhaustion.
    #[test]
    fn collectNeverSweeps() {
        let _serial = crate::share::memory::heap::GC_TEST_SERIAL.lock();
        let loader = test_loader();
        let mut heap = EpsilonCollectedHeap::new(Arc::clone(&loader), 1 << 30);

        let garbage = loader.alloc_type_array(TypeArrayOop::Byte(vec![5; 8192]));
        let before = heap.arena_used();
        heap.note_allocation();
        assert_eq!(heap.alloc_count(), 1);

        for cause in [
            GCCause::JavaLangSystemGC,
            GCCause::ScavengeAlot,
            GCCause::AllocationFailure,
        ] {
            heap.collect(cause).unwrap();
        }
        // No collection ran: arena contents and occupancy are unchanged.
        assert_eq!(heap.arena_used(), before);
        assert_eq!(loader.arrays.borrow()[garbage].len(), 8192);
    }
}
