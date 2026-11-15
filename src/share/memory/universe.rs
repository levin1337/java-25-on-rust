/*
 * Port of OpenJDK 25 HotSpot Universe (share/memory/universe.hpp / universe.cpp)
 *
 * Universe is the global namespace holding well-known classes, preallocated
 * error objects, and the JVM object heap instance (CollectedHeap).
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::gc::epsilon::epsilonHeap::EpsilonCollectedHeap;
use crate::share::gc::parallel::parallelCollectedHeap::ParallelCollectedHeap;
use crate::share::gc::serial::serialCollectedHeap::SerialCollectedHeap;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use crate::share::memory::heap::HeapGcMode;
use crate::share::oops::symbol::Symbol;
use crate::share::runtime::arguments::Arguments;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

pub struct Universe {
    // Known class symbols
    pub object_class_name: Symbol,
    pub string_class_name: Symbol,
    pub class_class_name: Symbol,
    pub cloneable_class_name: Symbol,
    pub serializable_class_name: Symbol,
    pub system_class_name: Symbol,
    pub thread_class_name: Symbol,
    pub thread_group_class_name: Symbol,
    pub throwable_class_name: Symbol,
    pub error_class_name: Symbol,
    pub out_of_memory_error_class_name: Symbol,
    pub stack_overflow_error_class_name: Symbol,
    pub null_pointer_exception_class_name: Symbol,
    pub arithmetic_exception_class_name: Symbol,

    // Runtime state flags
    bootstrapping: RwLock<bool>,
    fully_initialized: RwLock<bool>,

    // Active CollectedHeap
    heap: Mutex<Option<Box<dyn CollectedHeap + Send>>>,
}

static UNIVERSE: OnceLock<Universe> = OnceLock::new();

impl Universe {
    pub fn new() -> Self {
        Self {
            object_class_name: Symbol::from("java/lang/Object"),
            string_class_name: Symbol::from("java/lang/String"),
            class_class_name: Symbol::from("java/lang/Class"),
            cloneable_class_name: Symbol::from("java/lang/Cloneable"),
            serializable_class_name: Symbol::from("java/io/Serializable"),
            system_class_name: Symbol::from("java/lang/System"),
            thread_class_name: Symbol::from("java/lang/Thread"),
            thread_group_class_name: Symbol::from("java/lang/ThreadGroup"),
            throwable_class_name: Symbol::from("java/lang/Throwable"),
            error_class_name: Symbol::from("java/lang/Error"),
            out_of_memory_error_class_name: Symbol::from("java/lang/OutOfMemoryError"),
            stack_overflow_error_class_name: Symbol::from("java/lang/StackOverflowError"),
            null_pointer_exception_class_name: Symbol::from("java/lang/NullPointerException"),
            arithmetic_exception_class_name: Symbol::from("java/lang/ArithmeticException"),

            bootstrapping: RwLock::new(true),
            fully_initialized: RwLock::new(false),
            heap: Mutex::new(None),
        }
    }

    pub fn genesis() -> &'static Self {
        UNIVERSE.get_or_init(Self::new)
    }

    pub fn is_bootstrapping(&self) -> bool {
        *self.bootstrapping.read().unwrap()
    }

    pub fn is_fully_initialized(&self) -> bool {
        *self.fully_initialized.read().unwrap()
    }

    pub fn set_fully_initialized(&self) {
        *self.bootstrapping.write().unwrap() = false;
        *self.fully_initialized.write().unwrap() = true;
    }

    /// Initializes the JVM CollectedHeap according to command-line configuration.
    ///
    /// The collector is bound to the JVM-wide `ClassLoader` installed by
    /// `threading::install_loader`; fails if bootstrap has not run yet.
    pub fn initialize_heap(&self, args: &Arguments) -> Result<(), &'static str> {
        let Some(loader) = crate::share::runtime::threading::jvm_loader() else {
            return Err("Universe::initialize_heap requires the JVM ClassLoader \
                        (threading::install_loader must run first)");
        };
        self.initialize_heap_with_loader(&loader, args)
    }

    /// Initializes the JVM CollectedHeap against an explicit loader.
    ///
    /// Collector selection is driven by `gc::shared::gcEngine::
    /// heap_gc_mode()` — the same engine the allocation-triggered
    /// `memory::heap::collect` uses — so the `Universe` collector and
    /// the interpreter's GC path always agree and a collection can never
    /// run twice for the same safepoint:
    ///
    /// - `-XX:+UseSerialGC`            -> `SerialCollectedHeap`
    /// - `-XX:+UseParallelGC`          -> `ParallelCollectedHeap`
    ///   (work-stealing mark + parallel sweep)
    /// - `-XX:+UseEpsilonGC` / `JVM25_GC=epsilon`
    ///                                -> `EpsilonCollectedHeap`
    ///   (allocation only, `collect` is a no-op)
    /// - `-XX:+UseG1GC` / `UseZGC` / `UseShenandoahGC`
    ///                                -> `SerialCollectedHeap` fallback:
    ///   their region machinery, remembered sets and concurrent barriers
    ///   live in `share::gc` but are not wired into the slot-arena heap,
    ///   so `gcEngine` maps them to `SerialMarkSweep` and the real
    ///   mark-sweep collector executes their collections.
    pub fn initialize_heap_with_loader(
        &self,
        loader: &Arc<ClassLoader<'static>>,
        args: &Arguments,
    ) -> Result<(), &'static str> {
        let mut heap_guard = self.heap.lock().unwrap();
        if heap_guard.is_some() {
            return Err("Heap already initialized");
        }

        *heap_guard = Some(Self::select_heap(loader, args));
        Ok(())
    }

    /// Builds the real `CollectedHeap` for the active GC engine.
    /// `args` supplies the heap sizing knobs; the collector kind comes
    /// from `heap_gc_mode()` so `Universe` and `memory::heap` can never
    /// disagree about which collector is running.
    fn select_heap(
        loader: &Arc<ClassLoader<'static>>,
        args: &Arguments,
    ) -> Box<dyn CollectedHeap + Send> {
        Self::heap_for_mode(loader, crate::share::gc::shared::gcEngine::heap_gc_mode(), args)
    }

    /// Maps a `HeapGcMode` onto the concrete `CollectedHeap`:
    /// `SerialMarkSweep` covers `-XX:+UseSerialGC` and the documented
    /// G1/ZGC/Shenandoah fallback, `ParallelMarkSweep` is
    /// `-XX:+UseParallelGC`, `NoCollect` is `-XX:+UseEpsilonGC`.
    fn heap_for_mode(
        loader: &Arc<ClassLoader<'static>>,
        mode: HeapGcMode,
        args: &Arguments,
    ) -> Box<dyn CollectedHeap + Send> {
        let young_size = (args.initial_heap_size / 3).max(1024 * 1024);
        let old_size = args.initial_heap_size.saturating_sub(young_size);

        match mode {
            HeapGcMode::ParallelMarkSweep => {
                let workers = crate::share::gc::shared::gcEngine::parallel_gc_workers();
                Box::new(ParallelCollectedHeap::new(
                    Arc::clone(loader),
                    young_size,
                    old_size,
                    workers,
                ))
            }
            HeapGcMode::NoCollect => Box::new(EpsilonCollectedHeap::new(
                Arc::clone(loader),
                args.max_heap_size,
            )),
            HeapGcMode::SerialMarkSweep => Box::new(SerialCollectedHeap::new(
                Arc::clone(loader),
                young_size,
                old_size,
            )),
        }
    }

    /// Runs a collection through the active `CollectedHeap`, mirroring
    /// HotSpot `Universe::heap()->collect()`.
    pub fn collect_heap(&self, cause: GCCause) -> Result<(), &'static str> {
        let mut guard = self.heap.lock().unwrap();
        match guard.as_mut() {
            Some(heap) => heap.collect(cause).map_err(|_| "GC failed"),
            None => Err("Heap not initialized"),
        }
    }

    /// Returns the kind of the currently active CollectedHeap, if initialized
    pub fn heap_kind(&self) -> Option<CollectedHeapKind> {
        let guard = self.heap.lock().unwrap();
        guard.as_ref().map(|h| h.kind())
    }

    /// Returns current heap capacity
    pub fn heap_capacity(&self) -> usize {
        let guard = self.heap.lock().unwrap();
        guard.as_ref().map(|h| h.capacity()).unwrap_or(0)
    }

    /// Returns current heap used bytes
    pub fn heap_used(&self) -> usize {
        let guard = self.heap.lock().unwrap();
        guard.as_ref().map(|h| h.used()).unwrap_or(0)
    }
}

impl Default for Universe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::classfile::classLoader::ClassLoader;
    use crate::share::gc::shared::gcEngine;
    use crate::share::runtime::syncBump::SyncBump;
    use std::path::Path;

    fn test_loader() -> Arc<ClassLoader<'static>> {
        let arena: &'static SyncBump = Box::leak(Box::new(SyncBump::new()));
        Arc::new(ClassLoader::open(Path::new("."), arena).unwrap())
    }

    /// `heap_for_mode` maps each GC engine to the real collector bound
    /// to the loader's arenas — never the placeholder counters.
    #[test]
    fn heapForModeSelectsRealCollectors() {
        let loader = test_loader();
        let args = Arguments::default();

        let serial = Universe::heap_for_mode(&loader, HeapGcMode::SerialMarkSweep, &args);
        assert_eq!(serial.kind(), CollectedHeapKind::Serial);

        let parallel = Universe::heap_for_mode(&loader, HeapGcMode::ParallelMarkSweep, &args);
        assert_eq!(parallel.kind(), CollectedHeapKind::Parallel);

        let epsilon = Universe::heap_for_mode(&loader, HeapGcMode::NoCollect, &args);
        assert_eq!(epsilon.kind(), CollectedHeapKind::Epsilon);
    }

    /// `initialize_heap_with_loader` installs the collector matching
    /// `gcEngine::heap_gc_mode()` — the same engine
    /// `memory::heap::collect` uses — and refuses a second install.
    #[test]
    fn initializeHeapInstallsEngineCollectorOnce() {
        let universe = Universe::new();
        let loader = test_loader();
        let args = Arguments::default();

        universe
            .initialize_heap_with_loader(&loader, &args)
            .unwrap();
        let expected = match gcEngine::heap_gc_mode() {
            HeapGcMode::ParallelMarkSweep => CollectedHeapKind::Parallel,
            HeapGcMode::NoCollect => CollectedHeapKind::Epsilon,
            HeapGcMode::SerialMarkSweep => CollectedHeapKind::Serial,
        };
        assert_eq!(universe.heap_kind(), Some(expected));
        assert!(universe.heap_capacity() > 0);

        // Second initialization is rejected: the collector (and its
        // loader binding) is fixed for the process lifetime.
        assert!(universe.initialize_heap_with_loader(&loader, &args).is_err());
        assert_eq!(universe.heap_kind(), Some(expected));
    }

    /// The installed collector and the allocation-triggered GC path
    /// share `memory::heap::collect_cycle`: a `Universe`-driven
    /// collection sweeps the same arenas `heap::collect` manages, so
    /// there is no double GC — the pending guard admits one cycle at
    /// a time and both paths update the same sweep watermark.
    #[test]
    fn universeCollectSharesHeapEngine() {
        let _serial = crate::share::memory::heap::GC_TEST_SERIAL.lock();
        let universe = Universe::new();
        let loader = test_loader();
        let args = Arguments::default();
        universe
            .initialize_heap_with_loader(&loader, &args)
            .unwrap();

        let dead = loader.alloc_type_array(
            crate::share::oops::typeArrayOop::TypeArrayOop::Byte(vec![4; 4096]),
        );
        if gcEngine::heap_gc_mode() == HeapGcMode::NoCollect {
            // Epsilon: collection is a no-op by definition.
            universe
                .collect_heap(crate::share::gc::shared::gcCause::GCCause::JavaLangSystemGC)
                .unwrap();
            assert_eq!(loader.arrays.borrow()[dead].len(), 4096);
            return;
        }
        for _ in 0..100 {
            universe
                .collect_heap(crate::share::gc::shared::gcCause::GCCause::JavaLangSystemGC)
                .unwrap();
            if loader.arrays.borrow()[dead].len() == 0 {
                break;
            }
        }
        assert_eq!(loader.arrays.borrow()[dead].len(), 0);
        // `used()` reports live arena occupancy through the loader.
        assert!(universe.heap_used() > 0);
    }
}
