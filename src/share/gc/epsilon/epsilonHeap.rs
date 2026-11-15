use crate::share::classfile::classLoader::ClassLoader;
use crate::share::gc::epsilon::epsilonArguments::EpsilonHeapConfig;
use crate::share::gc::epsilon::epsilonMonitoringSupport::EpsilonMonitoringSupport;
use crate::share::gc::epsilon::epsilonThreadLocalData::EpsilonThreadLocalData;
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

/*
 * Structural port of OpenJDK 25 HotSpot EpsilonHeap
 * (share/gc/epsilon/epsilonHeap.hpp / epsilonHeap.cpp)
 *
 * Allocate-only heap: a contiguous byte arena that grows (commits)
 * toward the reserved -Xmx size on demand. Allocation is a bump
 * pointer; there is no reclamation, so exhaustion of the reserved
 * space surfaces as allocation failure (OutOfMemoryError upstream).
 */

pub struct EpsilonHeap {
    config: EpsilonHeapConfig,
    reserved_size: usize,
    committed_size: usize,
    top: usize,
    bottom: usize,
    last_counter_update: usize,
    last_heap_print: usize,
    monitoring: EpsilonMonitoringSupport,
    alloc_count: usize,
    oom_count: usize,
}

impl EpsilonHeap {
    pub fn new(config: EpsilonHeapConfig) -> Self {
        let reserved = config.max_heap_size.max(config.min_heap_size);
        let committed = config.min_heap_size.min(reserved);
        Self {
            config,
            reserved_size: reserved,
            committed_size: committed,
            top: 0,
            bottom: 0,
            last_counter_update: 0,
            last_heap_print: 0,
            monitoring: EpsilonMonitoringSupport::new(),
            alloc_count: 0,
            oom_count: 0,
        }
    }

    #[inline]
    pub fn is_object_aligned(size: usize) -> bool {
        size % 8 == 0
    }

    pub fn allocate_work(&mut self, size: usize, verbose: bool) -> Option<usize> {
        debug_assert!(Self::is_object_aligned(size));

        loop {
            if self.top + size <= self.committed_size {
                break;
            }
            let size_in_bytes = size;
            let uncommitted_space = self.reserved_size - self.committed_size;
            let used = self.used();
            let unused_space = self.reserved_size - used;
            let want_space = size_in_bytes.max(self.config.min_heap_expand);
            debug_assert!(unused_space >= uncommitted_space);

            let expand_by = if want_space < uncommitted_space {
                want_space
            } else if size_in_bytes < unused_space {
                uncommitted_space
            } else {
                self.oom_count += 1;
                return None;
            };
            self.committed_size += expand_by;
        }

        let res = self.top;
        self.top += size;
        self.alloc_count += 1;
        let used = self.used();

        if verbose {
            if used - self.last_counter_update >= self.config.update_counters_step {
                self.last_counter_update = used;
                self.monitoring
                    .update_counters(used, self.committed_size, self.reserved_size);
            }
            if used - self.last_heap_print >= self.config.print_heap_steps * 1024 * 1024 {
                self.last_heap_print = used;
            }
        }

        Some(res)
    }

    pub fn allocate_new_tlab(
        &mut self,
        min_size: usize,
        requested_size: usize,
        actual_size: &mut usize,
        thread: &mut EpsilonThreadLocalData,
        now_nanos: i64,
    ) -> Option<usize> {
        let mut fits = true;
        let mut size = requested_size;
        let mut ergo_tlab = requested_size;
        let mut time = 0i64;

        if self.config.elastic_tlab {
            ergo_tlab = thread.ergo_tlab_size();

            if self.config.elastic_tlab_decay {
                let last_time = thread.last_tlab_time();
                time = now_nanos;
                debug_assert!(last_time <= time);
                if last_time != 0
                    && time - last_time
                        > (self.config.tlab_decay_time_ms as i64) * 1_000_000
                {
                    ergo_tlab = 0;
                    thread.set_ergo_tlab_size(0);
                }
            }

            fits = requested_size <= ergo_tlab;
            if !fits {
                size = (ergo_tlab as f64 * self.config.tlab_elasticity) as usize;
            }
        }

        size = size.clamp(min_size, self.config.max_tlab_size);
        size = size.div_ceil(8) * 8;

        debug_assert!(Self::is_object_aligned(size));
        debug_assert!(min_size <= size);
        debug_assert!(size <= self.config.max_tlab_size);

        let res = self.allocate_work(size, true);

        if res.is_some() {
            *actual_size = size;
            if self.config.elastic_tlab_decay {
                thread.set_last_tlab_time(time);
            }
            if self.config.elastic_tlab && !fits {
                thread.set_ergo_tlab_size(size);
            }
        } else if self.config.elastic_tlab {
            thread.set_ergo_tlab_size(0);
        }

        res
    }

    pub fn mem_allocate(&mut self, size: usize) -> Option<usize> {
        self.allocate_work(size, true)
    }

    pub fn allocate_loaded_archive_space(&mut self, size: usize) -> Option<usize> {
        self.allocate_work(size, false)
    }

    pub fn tlab_capacity(&self) -> usize {
        self.committed_size
    }

    pub fn tlab_used(&self) -> usize {
        self.used()
    }

    pub fn max_tlab_size(&self) -> usize {
        self.config.max_tlab_size
    }

    pub fn unsafe_max_tlab_alloc(&self) -> usize {
        self.config.max_tlab_size
    }

    pub fn collect(&mut self, _cause: GCCause) {
        self.monitoring.note_gc();
    }

    pub fn is_in(&self, addr: usize) -> bool {
        addr >= self.bottom && addr < self.top
    }

    pub fn is_in_reserved(&self, addr: usize) -> bool {
        addr >= self.bottom && addr < self.reserved_size
    }

    pub fn reserved_region(&self) -> (usize, usize) {
        (self.bottom, self.reserved_size)
    }

    pub fn block_start(&self, _addr: usize) -> Option<usize> {
        None
    }

    pub fn block_is_obj(&self, _addr: usize) -> bool {
        false
    }

    pub fn capacity(&self) -> usize {
        self.committed_size
    }

    pub fn max_capacity(&self) -> usize {
        self.reserved_size
    }

    pub fn used(&self) -> usize {
        self.top - self.bottom
    }

    pub fn monitoring(&self) -> &EpsilonMonitoringSupport {
        &self.monitoring
    }

    pub fn alloc_count(&self) -> usize {
        self.alloc_count
    }

    pub fn oom_count(&self) -> usize {
        self.oom_count
    }
}

#[cfg(test)]
mod epsilon_heap_tests {
    use super::*;
    use crate::share::gc::shared::gcCause::GCCause;

    fn small_heap() -> EpsilonHeap {
        let mut config = EpsilonHeapConfig::default();
        config.max_heap_size = 1 << 20;
        config.min_heap_size = 0;
        config.min_heap_expand = 64 * 1024;
        config.max_tlab_size = 256 * 1024;
        config.update_counters_step = 1024;
        config.print_heap_steps = 0;
        EpsilonHeap::new(config)
    }

    #[test]
    fn bump_allocation_and_expansion() {
        let mut heap = small_heap();
        let a = heap.allocate_work(4096, true).unwrap();
        let b = heap.allocate_work(8192, true).unwrap();
        assert_eq!(b - a, 4096);
        assert!(heap.is_in(a));
        assert!(heap.capacity() > 0);
        assert!(heap.capacity() <= heap.max_capacity());
    }

    #[test]
    fn exhaustion_is_oom() {
        let mut heap = small_heap();
        let chunk = 256 * 1024usize;
        let mut allocated = 0usize;
        loop {
            match heap.allocate_work(chunk, false) {
                Some(_) => allocated += chunk,
                None => break,
            }
        }
        assert!(allocated > 0);
        assert!(allocated <= heap.max_capacity());
        assert_eq!(heap.used(), allocated);
        assert!(heap.oom_count() > 0);
        assert!(heap.allocate_work(chunk, false).is_none());
        heap.collect(GCCause::AllocationFailure);
        assert!(heap.allocate_work(chunk, false).is_none());
    }
    #[test]
    fn elastic_tlab_growth() {
        let mut heap = small_heap();
        let mut tld = EpsilonThreadLocalData::new();
        let mut actual = 0usize;
        heap.allocate_new_tlab(1024, 4096, &mut actual, &mut tld, 1_000_000);
        assert_eq!(actual, 1024);
        assert_eq!(tld.ergo_tlab_size(), 1024);

        heap.allocate_new_tlab(1024, 1 << 20, &mut actual, &mut tld, 2_000_000);
        assert!(actual > 1024);
        assert!(actual <= heap.max_tlab_size());
        assert_eq!(tld.ergo_tlab_size(), actual);
    }

    #[test]
    fn tlab_decay_retracts_ergo_size() {
        let mut heap = small_heap();
        let mut tld = EpsilonThreadLocalData::new();
        tld.set_ergo_tlab_size(1 << 20);
        tld.set_last_tlab_time(1_000_000_000);
        let mut actual = 0usize;
        heap.allocate_new_tlab(1024, 1 << 18, &mut actual, &mut tld, 2_500_000_000);
        assert_eq!(actual, 1024);
        assert_eq!(tld.ergo_tlab_size(), 1024);
    }
}
