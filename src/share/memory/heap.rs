/*
 * Port of OpenJDK 25 HotSpot heap reachability analysis
 * (share/memory + share/gc/shared)
 *
 * Mark-sweep collector over the ClassLoader object arenas
 * (objects / primitive arrays / object arrays). Handles are slot
 * indices, so collection is non-moving: dead slots go on a free
 * list and are reused by alloc_* helpers.
 *
 * Roots: interpreter frames of all active executions, static fields
 * of loaded classes, the system class loader and current thread
 * objects, extra GC roots registered by the VM, thread objects,
 * module/class mirror objects and JNI references.
 *
 * Stop-the-world: Java threads run on dedicated OS threads, so a
 * collection must quiesce all of them. Every interpreter safepoint
 * (and every blocking wait/park/monitor/sleep entry) reports the
 * thread's frame handles into a global registry; the thread that
 * triggers a collection waits until every registered Java thread is
 * either parked at a safepoint or blocked in a Java wait, then marks
 * and sweeps while all mutators are stopped.
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::objArrayOop::ObjArrayOop;
use crate::share::oops::oop::Oop;
use crate::share::runtime::shareCell::ShareRef;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use parking_lot::{Condvar, Mutex};
use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::ThreadId;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Handle {
    Object(usize),
    TypeArray(usize),
    ObjArray(usize),
}

impl Handle {
    fn of(value: &Value<'_>) -> Option<Self> {
        match *value {
            Value::Object(handle) => Some(Self::Object(handle)),
            Value::PrimitiveArray(handle) => Some(Self::TypeArray(handle)),
            Value::ObjectArray(handle) => Some(Self::ObjArray(handle)),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Marks {
    objects: HashSet<usize>,
    type_arrays: HashSet<usize>,
    obj_arrays: HashSet<usize>,
}

impl Marks {
    fn insert(&mut self, handle: Handle) -> bool {
        match handle {
            Handle::Object(index) => self.objects.insert(index),
            Handle::TypeArray(index) => self.type_arrays.insert(index),
            Handle::ObjArray(index) => self.obj_arrays.insert(index),
        }
    }
}

fn mark_value(value: &Value<'_>, worklist: &mut Vec<Handle>) {
    if let Some(handle) = Handle::of(value) {
        worklist.push(handle);
    }
}

type Objects<'a, 'b> = ShareRef<'b, Vec<InstanceOop<'a>>>;
type ObjArrays<'a, 'b> = ShareRef<'b, Vec<ObjArrayOop<'a>>>;

fn drain<'a>(
    objects: &Objects<'a, '_>,
    obj_arrays: &ObjArrays<'a, '_>,
    marks: &mut Marks,
    worklist: &mut Vec<Handle>,
) {
    while let Some(handle) = worklist.pop() {
        if !marks.insert(handle) {
            continue;
        }
        match handle {
            Handle::Object(index) => {
                if let Some(object) = objects.get(index) {
                    // ReferenceProcessor semantics: the `referent` of ANY
                    // `java.lang.ref.Reference` subclass is not a strong
                    // root — skipping it here is what lets the referent
                    // die and the reference be discovered. Finalizer
                    // referents are resurrected for one more cycle by
                    // `process_references` so `finalize()` can still run.
                    let referent_slot = reference_referent_slot(object);
                    for (field_index, value) in object.fields.iter().enumerate() {
                        if Some(field_index) == referent_slot {
                            continue;
                        }
                        mark_value(value, worklist);
                    }
                }
            }
            Handle::ObjArray(index) => {
                if let Some(array) = obj_arrays.get(index) {
                    for value in &array.elements {
                        mark_value(value, worklist);
                    }
                }
            }
            Handle::TypeArray(_) => {}
        }
    }
}

/// ParallelGC marking: `workers` OS threads share one worklist
/// (LIFO stack under a mutex, HotSpot `ParallelTaskTerminator`-style
/// termination protocol). A shared claimed-set deduplicates handles so
/// each object is scanned exactly once; `busy` counts workers holding a
/// popped handle whose children are not yet pushed, which is what lets
/// idle workers distinguish "empty for now" from "done".
fn drain_parallel<'a>(
    objects: &[InstanceOop<'a>],
    obj_arrays: &[ObjArrayOop<'a>],
    seeds: Vec<Handle>,
    workers: usize,
) -> Marks {
    let worklist = Mutex::new(seeds);
    let claimed = Mutex::new(HashSet::new());
    let busy = AtomicUsize::new(0);
    let cv = Condvar::new();
    let workers = workers.max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let handle = {
                    let mut list = worklist.lock();
                    loop {
                        if let Some(handle) = list.pop() {
                            busy.fetch_add(1, Ordering::SeqCst);
                            break Some(handle);
                        }
                        if busy.load(Ordering::SeqCst) == 0 {
                            break None;
                        }
                        cv.wait(&mut list);
                    }
                };
                let Some(handle) = handle else {
                    return;
                };
                let is_new = claimed.lock().insert(handle);
                if is_new {
                    let mut children = Vec::new();
                    match handle {
                        Handle::Object(index) => {
                            if let Some(object) = objects.get(index) {
                                let referent_slot = reference_referent_slot(object);
                                for (field_index, value) in
                                    object.fields.iter().enumerate()
                                {
                                    if Some(field_index) == referent_slot {
                                        continue;
                                    }
                                    if let Some(child) = Handle::of(value) {
                                        children.push(child);
                                    }
                                }
                            }
                        }
                        Handle::ObjArray(index) => {
                            if let Some(array) = obj_arrays.get(index) {
                                for value in &array.elements {
                                    if let Some(child) = Handle::of(value) {
                                        children.push(child);
                                    }
                                }
                            }
                        }
                        Handle::TypeArray(_) => {}
                    }
                    worklist.lock().extend(children);
                }
                busy.fetch_sub(1, Ordering::SeqCst);
                cv.notify_all();
            });
        }
    });
    let mut marks = Marks::default();
    for handle in claimed.into_inner() {
        marks.insert(handle);
    }
    marks
}

/// Sweeps one contiguous range-partition of an arena in parallel.
/// Each worker owns a disjoint `&mut` chunk of `items`/`live`, clears
/// dead slots through `clear` and returns the freed indices; results
/// are concatenated in chunk order.
fn sweep_space<T: Send>(
    items: &mut [T],
    live: &mut [bool],
    marked: &HashSet<usize>,
    workers: usize,
    clear: fn(&mut T),
) -> Vec<usize> {
    let len = items.len();
    if len == 0 {
        return Vec::new();
    }
    let parts = workers.clamp(1, len);
    let chunk = len.div_ceil(parts);
    let mut chunks: Vec<(usize, &mut [T], &mut [bool])> = Vec::new();
    let mut base = 0;
    {
        let mut items_rest = items;
        let mut live_rest = live;
        while !items_rest.is_empty() {
            let take = chunk.min(items_rest.len());
            let (item_head, item_tail) = items_rest.split_at_mut(take);
            let (live_head, live_tail) = live_rest.split_at_mut(take);
            chunks.push((base, item_head, live_head));
            items_rest = item_tail;
            live_rest = live_tail;
            base += take;
        }
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|(base, items, live)| {
                scope.spawn(move || {
                    let mut freed = Vec::new();
                    for (offset, (item, was_live)) in
                        items.iter_mut().zip(live.iter_mut()).enumerate()
                    {
                        if *was_live && !marked.contains(&(base + offset)) {
                            *was_live = false;
                            clear(item);
                            freed.push(base + offset);
                        }
                    }
                    freed
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

/// ParallelGC sweep phase: each of the three arenas is range-partitioned
/// across `workers` scoped threads. Arenas that cannot be exclusively
/// borrowed are skipped for this cycle, same as the serial path.
fn sweep_parallel(loader: &ClassLoader<'_>, marks: &Marks, workers: usize) {
    if let (Some(mut objects), Some(mut live)) = (
        loader.objects.try_borrow_mut(),
        loader.objects_live.try_borrow_mut(),
    ) {
        live.resize(objects.len(), true);
        if let Some(mut free) = loader.free_objects_handle().try_borrow_mut() {
            let freed = sweep_space(
                objects.as_mut_slice(),
                live.as_mut_slice(),
                &marks.objects,
                workers,
                |object: &mut InstanceOop<'_>| object.fields = Vec::new(),
            );
            free.extend(freed);
        }
    }
    if let (Some(mut arrays), Some(mut live)) = (
        loader.arrays.try_borrow_mut(),
        loader.arrays_live.try_borrow_mut(),
    ) {
        live.resize(arrays.len(), true);
        if let Some(mut free) = loader.free_type_arrays_handle().try_borrow_mut() {
            let freed = sweep_space(
                arrays.as_mut_slice(),
                live.as_mut_slice(),
                &marks.type_arrays,
                workers,
                |array: &mut TypeArrayOop| *array = TypeArrayOop::Byte(Vec::new()),
            );
            free.extend(freed);
        }
    }
    if let (Some(mut arrays), Some(mut live)) = (
        loader.obj_arrays.try_borrow_mut(),
        loader.obj_arrays_live.try_borrow_mut(),
    ) {
        live.resize(arrays.len(), true);
        if let Some(mut free) = loader.free_obj_arrays_handle().try_borrow_mut() {
            let freed = sweep_space(
                arrays.as_mut_slice(),
                live.as_mut_slice(),
                &marks.obj_arrays,
                workers,
                |array: &mut ObjArrayOop<'_>| array.elements = Vec::new(),
            );
            free.extend(freed);
        }
    }
}

/// Returns the field slot of `referent` when `object` is a
/// `java.lang.ref.Reference` subclass (weak/soft/phantom/final), i.e. the
/// field the marker must skip so referent liveness is decided by
/// reachability from other roots only.
fn reference_referent_slot(object: &InstanceOop<'_>) -> Option<usize> {
    let mut klass = Some(std::sync::Arc::clone(&object.klass));
    let mut is_reference = false;
    while let Some(k) = klass {
        let name = k.class.name;
        // Soft references are only cleared under real memory pressure;
        // with no heap limit modelled here they behave as strong refs.
        if name == b"java/lang/ref/SoftReference" {
            return None;
        }
        if name == b"java/lang/ref/Reference" {
            is_reference = true;
            break;
        }
        klass = k.super_klass();
    }
    if !is_reference {
        return None;
    }
    object
        .klass
        .instanceField(b"referent", b"Ljava/lang/Object;")
        .ok()
}

/// `true` when `object` is a `java.lang.ref.Finalizer` (or subclass).
fn is_finalizer_reference(object: &InstanceOop<'_>) -> bool {
    let mut klass = Some(std::sync::Arc::clone(&object.klass));
    while let Some(k) = klass {
        if k.class.name == b"java/lang/ref/Finalizer" {
            return true;
        }
        klass = k.super_klass();
    }
    false
}

fn jni_roots() -> Vec<Oop> {
    let Some(vm) = crate::share::prims::jniInvocation::JavaVm::global_if_exists() else {
        return Vec::new();
    };
    unsafe { &*vm }.references().referenced_oops()
}

// ---------------------------------------------------------------------------
// Stop-the-world registry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ThreadGcState {
    /// Executing bytecode; must report at the next safepoint.
    Running,
    /// Parked inside `safepoint()` for the pending collection.
    Quiesced,
    /// Blocked in a Java wait/park/monitor/sleep; roots already captured.
    Blocked,
}

struct ThreadEntry {
    id: ThreadId,
    roots: Vec<Handle>,
    state: ThreadGcState,
}

struct GcShared {
    pending: bool,
    collector: Option<ThreadId>,
    threads: Vec<ThreadEntry>,
}

static GC_STATE: Mutex<GcShared> = Mutex::new(GcShared {
    pending: false,
    collector: None,
    threads: Vec::new(),
});
static GC_CV: Condvar = Condvar::new();
/// Serializes `#[cfg(test)]` code paths that drive `collect_cycle`:
/// the pending-GC guard would otherwise let a concurrent test's
/// collection return early without sweeping, making assertions flaky.
#[cfg(test)]
pub(crate) static GC_TEST_SERIAL: Mutex<()> = Mutex::new(());
/// Arena slot total after the last completed sweep; collections only run
/// once the arenas have grown by `SWEEP_THRESHOLD` slots.
static LAST_SWEEP_TOTAL: AtomicUsize = AtomicUsize::new(0);
const SWEEP_THRESHOLD: usize = 1 << 16;

fn entry_mut<'a>(state: &'a mut GcShared, id: ThreadId) -> &'a mut ThreadEntry {
    if let Some(position) = state.threads.iter().position(|entry| entry.id == id) {
        &mut state.threads[position]
    } else {
        state.threads.push(ThreadEntry {
            id,
            roots: Vec::new(),
            state: ThreadGcState::Running,
        });
        state.threads.last_mut().unwrap()
    }
}

/// Captures the handles reachable from this thread's interpreter frames.
fn snapshot_handles() -> Vec<Handle> {
    let mut out = Vec::new();
    crate::share::interpreter::bytecodeInterpreter::for_each_active_frame(&mut |frame| {
        for value in frame.locals().iter().chain(frame.stack()) {
            if let Some(handle) = Handle::of(value) {
                out.push(handle);
            }
        }
    });
    out
}

/// Interpreter safepoint (every ~1024 bytecodes). Cheap when no
/// collection is pending; otherwise captures this thread's frame roots
/// and sleeps until the collector finishes. Re-registers after every
/// wakeup so a new cycle started while this thread was still parked
/// cannot strand it.
pub fn safepoint() {
    let id = crate::share::prims::jniReferences::current_thread_id();
    loop {
        let mut state = GC_STATE.lock();
        if !state.pending {
            let entry = entry_mut(&mut state, id);
            if entry.state != ThreadGcState::Blocked {
                entry.state = ThreadGcState::Running;
                entry.roots = Vec::new();
            }
            return;
        }
        let roots = snapshot_handles();
        {
            let entry = entry_mut(&mut state, id);
            entry.roots = roots;
            entry.state = ThreadGcState::Quiesced;
        }
        if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
            eprintln!("[block] safepoint-park tid={id:?}");
        }
        GC_CV.notify_all();
        GC_CV.wait(&mut state);
    }
}

/// Marks the calling thread as blocked in a Java wait. The frame roots
/// are captured now; while blocked the frames cannot change, so the
/// snapshot stays valid until `unquiesce`.
pub fn quiesce() {
    let mut state = GC_STATE.lock();
    let id = crate::share::prims::jniReferences::current_thread_id();
    let roots = snapshot_handles();
    let entry = entry_mut(&mut state, id);
    entry.roots = roots;
    entry.state = ThreadGcState::Blocked;
    GC_CV.notify_all();
}

/// Called when a previously blocked thread resumes execution.
pub fn unquiesce() {
    let mut state = GC_STATE.lock();
    let id = crate::share::prims::jniReferences::current_thread_id();
    if let Some(entry) = state.threads.iter_mut().find(|entry| entry.id == id)
        && entry.state == ThreadGcState::Blocked
    {
        entry.state = ThreadGcState::Running;
        entry.roots = Vec::new();
    }
}

/// Removes the calling thread from the GC registry (thread exit).
pub fn thread_finished() {
    let mut state = GC_STATE.lock();
    let id = crate::share::prims::jniReferences::current_thread_id();
    state.threads.retain(|entry| entry.id != id);
    GC_CV.notify_all();
}

/// GC execution engine backing the safepoint mark-sweep cycle.
/// Selected once per process by `gc::shared::gcEngine` from the
/// `-XX:+Use*GC` flags (or `JVM25_GC` for tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeapGcMode {
    /// Single-threaded mark + sweep. This is also the fallback engine
    /// for the region/concurrent collectors (G1, ZGC, Shenandoah) whose
    /// remembered sets and barriers live in `share::gc` but are not yet
    /// wired into the arena heap.
    SerialMarkSweep,
    /// Work-stealing parallel mark + range-partitioned parallel sweep
    /// (ParallelGC semantics).
    ParallelMarkSweep,
    /// EpsilonGC semantics: allocation only, `collect` is a no-op.
    NoCollect,
}

/// Registers `value` as an extra GC root. Used by JIT-compiled frames
/// and runtime stubs that must keep handles alive across GC-unsafe
/// operations (allocation slow paths, safepoint polls). Returns the
/// slot index for diagnostics; removal should go through
/// `unregister_extra_root`, which is atomic with respect to concurrent
/// pushes — the find-and-remove happens under a single mutable borrow,
/// unlike the previous borrow-then-remove-position pattern that could
/// observe a shifted index when another thread registered in between.
///
/// The borrow is held only for the `push` itself, so stubs must scope
/// it tightly: never hold a `gc_extra_roots` borrow across a safepoint,
/// or the collecting thread's `try_borrow` fails and the whole cycle is
/// skipped until the borrow is released.
pub fn register_extra_root<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> usize {
    let mut roots = loader.gc_extra_roots.borrow_mut();
    roots.push(value);
    roots.len() - 1
}

/// Removes one occurrence of `value` from the extra-root list (the last
/// one, matching LIFO stub push/pop order). Returns `true` when an
/// entry was removed. Atomic under a single mutable borrow — safe for
/// the push/pop pairs JIT allocation stubs perform around GC-unsafe
/// sections.
pub fn unregister_extra_root<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> bool {
    let mut roots = loader.gc_extra_roots.borrow_mut();
    let Some(position) = roots.iter().rposition(|entry| *entry == value) else {
        return false;
    };
    roots.remove(position);
    true
}

fn heap_gc_mode() -> HeapGcMode {
    crate::share::gc::shared::gcEngine::heap_gc_mode()
}

fn gc_workers(mode: HeapGcMode) -> usize {
    match mode {
        HeapGcMode::ParallelMarkSweep => {
            crate::share::gc::shared::gcEngine::parallel_gc_workers()
        }
        _ => 1,
    }
}

fn arena_total<'a>(loader: &ClassLoader<'a>) -> usize {
    let objects = loader.objects.try_borrow();
    let arrays = loader.arrays.try_borrow();
    let obj_arrays = loader.obj_arrays.try_borrow();
    match (objects, arrays, obj_arrays) {
        (Some(objects), Some(arrays), Some(obj_arrays)) => {
            objects.len() + arrays.len() + obj_arrays.len()
        }
        _ => 0,
    }
}

/// Runs a full mark-sweep collection over the loader's object arenas.
///
/// The calling thread becomes the collector: it marks the safepoint
/// pending, parks itself like every other Java thread, waits until all
/// registered threads are quiesced or blocked, sweeps, then releases
/// everyone. Collection only runs once the arenas have grown by
/// `SWEEP_THRESHOLD` slots since the last completed sweep.
pub fn collect<'a>(loader: &ClassLoader<'a>) -> io::Result<()> {
    let mode = heap_gc_mode();
    if mode == HeapGcMode::NoCollect {
        return Ok(());
    }
    let total = arena_total(loader);
    if total == 0 {
        return Ok(());
    }
    let last = LAST_SWEEP_TOTAL.load(Ordering::Relaxed);
    if total.saturating_sub(last) < SWEEP_THRESHOLD {
        return Ok(());
    }
    collect_cycle(loader, mode, total, gc_workers(mode))
}

/// Forced collection bypassing the growth threshold (System.gc,
/// WhiteBox, explicit `CollectedHeap::collect` calls).
pub fn collect_now<'a>(loader: &ClassLoader<'a>) -> io::Result<()> {
    let mode = heap_gc_mode();
    if mode == HeapGcMode::NoCollect {
        return Ok(());
    }
    collect_cycle(loader, mode, arena_total(loader), gc_workers(mode))
}

/// Forced collection with an explicit engine, used by the collector
/// objects in `share::gc` (SerialGC / ParallelGC wrappers).
pub fn collect_with_mode<'a>(loader: &ClassLoader<'a>, mode: HeapGcMode) -> io::Result<()> {
    if mode == HeapGcMode::NoCollect {
        return Ok(());
    }
    collect_cycle(loader, mode, arena_total(loader), gc_workers(mode))
}

/// Forced collection with an explicit engine and worker count, used by
/// `ParallelCollectedHeap` which sizes its thread team itself.
pub fn collect_with_workers<'a>(
    loader: &ClassLoader<'a>,
    mode: HeapGcMode,
    workers: usize,
) -> io::Result<()> {
    if mode == HeapGcMode::NoCollect {
        return Ok(());
    }
    collect_cycle(loader, mode, arena_total(loader), workers.max(1))
}

fn collect_cycle<'a>(
    loader: &ClassLoader<'a>,
    _mode: HeapGcMode,
    total: usize,
    workers: usize,
) -> io::Result<()> {
    let id = crate::share::prims::jniReferences::current_thread_id();
    {
        let mut state = GC_STATE.lock();
        if state.pending {
            // Another thread is already collecting; this thread will
            // park at its next safepoint like everyone else.
            return Ok(());
        }
        state.pending = true;
        state.collector = Some(id);
        // Threads parked for a *previous* cycle are stale: they will
        // re-report at their next safepoint. Blocked threads keep
        // their (still valid) roots and already count as quiesced.
        for entry in &mut state.threads {
            if entry.state == ThreadGcState::Quiesced {
                entry.state = ThreadGcState::Running;
                entry.roots = Vec::new();
            }
        }
        GC_CV.notify_all();
    }

    loop {
        let frame_roots = {
            let mut state = GC_STATE.lock();
            let roots = snapshot_handles();
            let entry = entry_mut(&mut state, id);
            entry.roots = roots;
            entry.state = ThreadGcState::Quiesced;
            GC_CV.notify_all();
            let all_quiet = state
                .threads
                .iter()
                .all(|entry| entry.state != ThreadGcState::Running);
            if !all_quiet {
                if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
                    eprintln!("[block] gc-wait collector={id:?} threads={:?}",
                        state.threads.iter().map(|e| (e.id, e.state)).collect::<Vec<_>>());
                }
                GC_CV.wait(&mut state);
                continue;
            }
            let mut roots = Vec::new();
            for entry in &state.threads {
                roots.extend_from_slice(&entry.roots);
            }
            roots
        };
        let _ = collect_mark_sweep(loader, &frame_roots, workers);
        let mut state = GC_STATE.lock();
        state.pending = false;
        state.collector = None;
        for entry in &mut state.threads {
            if entry.state == ThreadGcState::Quiesced {
                entry.state = ThreadGcState::Running;
                entry.roots = Vec::new();
            }
        }
        LAST_SWEEP_TOTAL.store(total, Ordering::Relaxed);
        GC_CV.notify_all();
        return Ok(());
    }
}

/// Mark phase over all known roots, then sweep dead arena slots.
/// `workers > 1` selects the ParallelGC engine: work-stealing parallel
/// marking and range-partitioned parallel sweeping; `workers == 1` is
/// the SerialGC single-threaded path.
/// Every arena access uses `try_borrow*`; if a quiesced thread still
/// holds a borrow across a safepoint the collection is skipped for
/// this cycle rather than panicking (the arenas simply keep growing).
fn collect_mark_sweep<'a>(
    loader: &ClassLoader<'a>,
    frame_roots: &[Handle],
    workers: usize,
) -> io::Result<()> {
    let objects = match loader.objects.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let obj_arrays = match loader.obj_arrays.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let classes = match loader.classes.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let lambda_classes = match loader.lambda_cache.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let extra_roots = match loader.gc_extra_roots.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let reference_pending = match loader.reference_pending.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let loader_object = match loader.class_loader_object_handle().try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let current_thread = match loader.current_thread_object_handle().try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let unnamed_module = match loader.unnamed_module_object_handle().try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let module_objects = match loader.module_objects_handle().try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let class_objects = match loader.class_objects.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let thread_ready = match loader.thread_ready.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let java_threads = match loader.java_threads.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let current_java_thread = match loader.current_java_thread.try_borrow() {
        Some(guard) => guard,
        None => return Ok(()),
    };

    let mut worklist = Vec::new();

    worklist.extend_from_slice(frame_roots);

    for klass in classes
        .values()
        .map(|klass| &**klass)
        .chain(lambda_classes.values().map(|klass| &**klass))
    {
        for cell in &klass.static_fields {
            if let Some(value) = cell.try_borrow().map(|guard| *guard) {
                mark_value(&value, &mut worklist);
            }
        }
    }

    for handle in [
        *loader_object,
        *current_thread,
        *unnamed_module,
        *current_java_thread,
    ]
    .into_iter()
    .flatten()
    {
        worklist.push(Handle::Object(handle));
    }
    for &handle in module_objects.values().chain(class_objects.values()) {
        worklist.push(Handle::Object(handle));
    }
    for &handle in thread_ready.iter().chain(java_threads.keys()) {
        worklist.push(Handle::Object(handle));
    }
    for value in extra_roots.iter() {
        mark_value(value, &mut worklist);
    }
    for oop in jni_roots() {
        match oop {
            Oop::Instance(handle) => worklist.push(Handle::Object(handle)),
            Oop::TypeArray(handle) => worklist.push(Handle::TypeArray(handle)),
            Oop::ObjArray(handle) => worklist.push(Handle::ObjArray(handle)),
            Oop::Null => {}
        }
    }

    // The VM pending-reference list is a GC root: its entries have not
    // been enqueued into their `ReferenceQueue` yet (the JDK
    // ReferenceHandler consumes `reference_pending` asynchronously via
    // `getAndClearReferencePendingList`), so without this they could be
    // swept before `discovered` processing and lose their notification —
    // including `Cleaner.clean()` runs. The chain is linked through the
    // `discovered` field; cycle-guarded because the list is built by GC
    // but walked here on every cycle.
    let mut pending_cursor = *reference_pending;
    let mut pending_seen = HashSet::new();
    while let Some(index) = pending_cursor {
        if !pending_seen.insert(index) {
            break;
        }
        worklist.push(Handle::Object(index));
        pending_cursor = objects.get(index).and_then(|object| {
            let slot = object
                .klass
                .instanceField(b"discovered", b"Ljava/lang/ref/Reference;")
                .ok()?;
            match object.fields.get(slot) {
                Some(Value::Object(next)) => Some(*next),
                _ => None,
            }
        });
    }

    let mut marks = if workers > 1 {
        drain_parallel(objects.as_slice(), obj_arrays.as_slice(), worklist, workers)
    } else {
        let mut marks = Marks::default();
        drain(&objects, &obj_arrays, &mut marks, &mut worklist);
        marks
    };
    drop((
        objects,
        obj_arrays,
        classes,
        lambda_classes,
        extra_roots,
        reference_pending,
        loader_object,
        current_thread,
        unnamed_module,
        module_objects,
        class_objects,
        thread_ready,
        java_threads,
        current_java_thread,
    ));

    process_references(loader, &mut marks);

    // Sweep: free-list slots are reused by the alloc_* helpers. If any
    // arena is still borrowed (same-thread borrow across the safepoint)
    // that arena is skipped for this cycle.
    if workers > 1 {
        sweep_parallel(loader, &marks, workers);
        return Ok(());
    }
    if let (Some(mut objects), Some(mut live)) = (
        loader.objects.try_borrow_mut(),
        loader.objects_live.try_borrow_mut(),
    ) {
        live.resize(objects.len(), true);
        let mut free = loader.free_objects_handle().try_borrow_mut();
        for index in 0..objects.len() {
            if live[index] && !marks.objects.contains(&index) {
                live[index] = false;
                objects[index].fields = Vec::new();
                if let Some(free) = free.as_deref_mut() {
                    free.push(index);
                }
            }
        }
    }
    if let (Some(mut arrays), Some(mut live)) = (
        loader.arrays.try_borrow_mut(),
        loader.arrays_live.try_borrow_mut(),
    ) {
        live.resize(arrays.len(), true);
        let mut free = loader.free_type_arrays_handle().try_borrow_mut();
        for index in 0..arrays.len() {
            if live[index] && !marks.type_arrays.contains(&index) {
                live[index] = false;
                arrays[index] = crate::share::oops::typeArrayOop::TypeArrayOop::Byte(Vec::new());
                if let Some(free) = free.as_deref_mut() {
                    free.push(index);
                }
            }
        }
    }
    if let (Some(mut arrays), Some(mut live)) = (
        loader.obj_arrays.try_borrow_mut(),
        loader.obj_arrays_live.try_borrow_mut(),
    ) {
        live.resize(arrays.len(), true);
        let mut free = loader.free_obj_arrays_handle().try_borrow_mut();
        for index in 0..arrays.len() {
            if live[index] && !marks.obj_arrays.contains(&index) {
                live[index] = false;
                arrays[index].elements = Vec::new();
                if let Some(free) = free.as_deref_mut() {
                    free.push(index);
                }
            }
        }
    }
    Ok(())
}

/// HotSpot `ReferenceProcessor` discovery: after marking, every live
/// `java.lang.ref.Reference` subclass instance whose referent did not
/// survive is linked onto the VM pending list through `discovered`.
/// Non-finalizer references also have `referent` cleared. Finalizer
/// referents are kept alive for one more cycle (resurrected below) so
/// `FinalizerThread` can still invoke `finalize()` on them; the JDK-side
/// `clear()` then drops the referent and the next GC reclaims it.
fn process_references(loader: &ClassLoader<'_>, marks: &mut Marks) {
    let Some(mut objects) = loader.objects.try_borrow_mut() else {
        return;
    };
    let mut pending: Vec<usize> = Vec::new();
    let mut resurrect: Vec<Handle> = Vec::new();
    for (index, object) in objects.iter_mut().enumerate() {
        if !marks.objects.contains(&index) {
            continue;
        }
        let Some(referent_slot) = reference_referent_slot(object) else {
            continue;
        };
        if object.fields.get(referent_slot).is_none() {
            continue;
        }
        let referent_dead = match object.fields[referent_slot] {
            Value::Null => false,
            Value::Object(h) => !marks.objects.contains(&h),
            Value::PrimitiveArray(h) => !marks.type_arrays.contains(&h),
            Value::ObjectArray(h) => !marks.obj_arrays.contains(&h),
            _ => false,
        };
        if !referent_dead {
            continue;
        }
        if is_finalizer_reference(object) {
            if let Some(handle) = Handle::of(&object.fields[referent_slot]) {
                resurrect.push(handle);
            }
        } else {
            object.fields[referent_slot] = Value::Null;
        }
        pending.push(index);
    }
    drop(objects);

    // Resurrect finalizer referents: drain from them so the finalizee and
    // everything it reaches survives this collection cycle.
    if !resurrect.is_empty()
        && let (Some(objects), Some(obj_arrays)) = (
            loader.objects.try_borrow(),
            loader.obj_arrays.try_borrow(),
        )
    {
        drain(&objects, &obj_arrays, marks, &mut resurrect);
    }

    if pending.is_empty() {
        return;
    }
    // Link discovered fields: pending[i].discovered = pending[i+1],
    // last points to null (JDK ReferenceProcessor pending-list order).
    if let Some(mut objects) = loader.objects.try_borrow_mut() {
        for (i, &handle) in pending.iter().enumerate() {
            let next = pending.get(i + 1).copied();
            if let Some(object) = objects.get_mut(handle) {
                if let Ok(slot) = object
                    .klass
                    .instanceField(b"discovered", b"Ljava/lang/ref/Reference;")
                {
                    object.fields[slot] = match next {
                        Some(h) => Value::Object(h),
                        None => Value::Null,
                    };
                }
            }
        }
    }

    // Publish the pending list. `getAndClearReferencePendingList` takes
    // the whole head atomically, so a non-empty slot means the JDK
    // ReferenceHandler has not started consuming the previous list yet:
    // append the newly discovered references to its tail instead of
    // dropping it (HotSpot keeps a single pending list across cycles).
    let mut head_slot = loader.reference_pending.borrow_mut();
    match (*head_slot, pending.first().copied()) {
        (None, new_head) => *head_slot = new_head,
        (Some(head), Some(new_head)) => {
            if let Some(mut objects) = loader.objects.try_borrow_mut() {
                // Walk to the tail of the existing chain; it ends with
                // `discovered == null`. Bounded by the arena size so a
                // corrupted chain cannot loop forever.
                let mut tail = head;
                for _ in 0..objects.len() {
                    let Some(object) = objects.get(tail) else {
                        break;
                    };
                    let Ok(slot) = object
                        .klass
                        .instanceField(b"discovered", b"Ljava/lang/ref/Reference;")
                    else {
                        break;
                    };
                    match object.fields.get(slot) {
                        Some(Value::Object(next)) => tail = *next,
                        _ => break,
                    }
                }
                if let Some(object) = objects.get_mut(tail)
                    && let Ok(slot) = object
                        .klass
                        .instanceField(b"discovered", b"Ljava/lang/ref/Reference;")
                {
                    object.fields[slot] = Value::Object(new_head);
                }
            }
        }
        (Some(_), None) => {}
    }
    drop(head_slot);
    crate::share::runtime::threading::wake_all(loader);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::classfile::{ClassFile, Member};
    use crate::share::oops::instanceKlass::InstanceKlass;
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    use std::sync::Arc;

    fn reference_klass<'a>() -> Arc<InstanceKlass<'a>> {
        Arc::new(
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
        )
    }

    #[test]
    fn extraRootHelpersPinAndRelease() {
        let _serial = GC_TEST_SERIAL.lock();
        let arena = crate::share::runtime::syncBump::SyncBump::new();
        let loader = ClassLoader::open(std::path::Path::new("."), &arena).unwrap();
        let handle = loader.alloc_type_array(TypeArrayOop::Byte(vec![5; 4096]));
        let slot = register_extra_root(&loader, Value::PrimitiveArray(handle));
        assert_eq!(loader.gc_extra_roots.borrow()[slot], Value::PrimitiveArray(handle));
        for _ in 0..100 {
            let _ = collect_mark_sweep(&loader, &[], 1);
            if loader.arrays.borrow()[handle].len() == 4096 {
                break;
            }
        }
        assert_eq!(loader.arrays.borrow()[handle].len(), 4096);
        assert!(unregister_extra_root(&loader, Value::PrimitiveArray(handle)));
        assert!(!unregister_extra_root(&loader, Value::PrimitiveArray(handle)));
        for _ in 0..100 {
            let _ = collect_mark_sweep(&loader, &[], 1);
            if loader.arrays.borrow()[handle].len() == 0 {
                break;
            }
        }
        assert_eq!(loader.arrays.borrow()[handle].len(), 0);
    }

    /// The VM pending-reference list is a GC root: a discovered
    /// reference whose `gc_extra_roots` pin was already released must
    /// survive subsequent collections until the JDK ReferenceHandler
    /// consumes it via `getAndClearReferencePendingList`.
    #[test]
    fn pendingReferencesSurviveUntilConsumed() {
        let _serial = GC_TEST_SERIAL.lock();
        let arena = crate::share::runtime::syncBump::SyncBump::new();
        let loader = ClassLoader::open(std::path::Path::new("."), &arena).unwrap();

        let referent = loader.alloc_type_array(TypeArrayOop::Byte(vec![9; 2048]));
        let reference = loader.alloc_object(InstanceOop {
            klass: reference_klass(),
            fields: vec![Value::PrimitiveArray(referent), Value::Null],
        });
        register_extra_root(&loader, Value::Object(reference));

        for _ in 0..100 {
            let _ = collect_mark_sweep(&loader, &[], 1);
            if *loader.reference_pending.borrow() == Some(reference) {
                break;
            }
        }
        assert_eq!(*loader.reference_pending.borrow(), Some(reference));
        assert_eq!(loader.borrow_objects()[reference].fields[0], Value::Null);

        // Drop the explicit pin: the reference is now reachable only
        // through the VM pending list. Run several cycles — if the
        // pending chain were not a GC root the first successful sweep
        // would clear the object's fields.
        assert!(unregister_extra_root(&loader, Value::Object(reference)));
        for _ in 0..100 {
            let _ = collect_mark_sweep(&loader, &[], 1);
            if loader.borrow_objects()[reference].fields.len() == 0 {
                break;
            }
        }
        // Referent stays swept; the reference object itself must NOT be
        // collected — its fields (referent slot + discovered link) have
        // to remain intact for the pending-list walk.
        assert_eq!(loader.arrays.borrow()[referent].len(), 0);
        assert_eq!(loader.borrow_objects()[reference].fields.len(), 2);
        assert_eq!(*loader.reference_pending.borrow(), Some(reference));
    }

    #[test]
    fn releasesUnreachableArrayStorageAndReusesSlot() {
        let _serial = GC_TEST_SERIAL.lock();
        let arena = crate::share::runtime::syncBump::SyncBump::new();
        let loader = ClassLoader::open(std::path::Path::new("."), &arena).unwrap();
        let handle = loader.alloc_type_array(TypeArrayOop::Byte(vec![1; 1_000_000]));
        // Force a collection directly (bypasses the growth threshold
        // used by the interpreter-driven `collect`).
        let _ = collect_mark_sweep(&loader, &[], 1);
        assert_eq!(loader.arrays.borrow()[handle].len(), 0);
        let reused = loader.alloc_type_array(TypeArrayOop::Int(vec![42]));
        assert_eq!(reused, handle);
        assert_eq!(loader.arrays.borrow()[reused].len(), 1);
    }

    #[test]
    fn parallelSweepReclaimsUnreachableArrayStorage() {
        let _serial = GC_TEST_SERIAL.lock();
        let arena = crate::share::runtime::syncBump::SyncBump::new();
        let loader = ClassLoader::open(std::path::Path::new("."), &arena).unwrap();
        let mut handles = Vec::new();
        for _ in 0..64 {
            handles.push(loader.alloc_type_array(TypeArrayOop::Byte(vec![7; 4096])));
        }
        let _ = collect_mark_sweep(&loader, &[], 8);
        for &handle in &handles {
            assert_eq!(loader.arrays.borrow()[handle].len(), 0);
        }
        let reused = loader.alloc_type_array(TypeArrayOop::Int(vec![9]));
        assert!(handles.contains(&reused));
    }
}
