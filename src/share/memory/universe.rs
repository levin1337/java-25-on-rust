/*
 * Port of OpenJDK 25 HotSpot Universe (share/memory/universe.hpp / universe.cpp)
 *
 * Universe is the global namespace holding well-known classes, preallocated
 * error objects, and the JVM object heap instance (CollectedHeap).
 */

use crate::share::gc::epsilon::epsilonHeap::EpsilonHeap;
use crate::share::gc::g1::g1CollectedHeap::G1CollectedHeap;
use crate::share::gc::parallel::parallelScavengeHeap::ParallelScavengeHeap;
use crate::share::gc::serial::serialHeap::SerialHeap;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shenandoah::shenandoahHeap::ShenandoahHeap;
use crate::share::gc::z::zCollectedHeap::ZCollectedHeap;
use crate::share::oops::symbol::Symbol;
use crate::share::runtime::arguments::{Arguments, GCSelected};
use std::sync::{Mutex, OnceLock, RwLock};

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

    /// Initializes the JVM CollectedHeap according to command-line configuration
    pub fn initialize_heap(&self, args: &Arguments) -> Result<(), &'static str> {
        let mut heap_guard = self.heap.lock().unwrap();
        if heap_guard.is_some() {
            return Err("Heap already initialized");
        }

        let young_size = (args.initial_heap_size / 3).max(1024 * 1024);
        let old_size = args.initial_heap_size.saturating_sub(young_size);

        let heap_instance: Box<dyn CollectedHeap + Send> = match args.gc {
            GCSelected::G1 => Box::new(G1CollectedHeap::new(young_size + old_size, 1024 * 1024)),
            GCSelected::Parallel => Box::new(ParallelScavengeHeap::new(young_size, old_size)),
            GCSelected::Serial => Box::new(SerialHeap::new(young_size, old_size)),
            GCSelected::Epsilon => Box::new(EpsilonHeap::new(args.max_heap_size)),
            GCSelected::Z => Box::new(ZCollectedHeap::new(
                args.initial_heap_size,
                args.max_heap_size,
            )),
            GCSelected::Shenandoah => Box::new(ShenandoahHeap::new(
                args.initial_heap_size,
                args.max_heap_size,
            )),
        };

        *heap_guard = Some(heap_instance);
        Ok(())
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
