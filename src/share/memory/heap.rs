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
    let id = std::thread::current().id();
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
    let id = std::thread::current().id();
    let roots = snapshot_handles();
    let entry = entry_mut(&mut state, id);
    entry.roots = roots;
    entry.state = ThreadGcState::Blocked;
    GC_CV.notify_all();
}

/// Called when a previously blocked thread resumes execution.
pub fn unquiesce() {
    let mut state = GC_STATE.lock();
    let id = std::thread::current().id();
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
    let id = std::thread::current().id();
    state.threads.retain(|entry| entry.id != id);
    GC_CV.notify_all();
}

/// Runs a full mark-sweep collection over the loader's object arenas.
///
/// The calling thread becomes the collector: it marks the safepoint
/// pending, parks itself like every other Java thread, waits until all
/// registered threads are quiesced or blocked, sweeps, then releases
/// everyone.
pub fn collect<'a>(loader: &ClassLoader<'a>) -> io::Result<()> {
    let total = {
        let objects = loader.objects.try_borrow();
        let arrays = loader.arrays.try_borrow();
        let obj_arrays = loader.obj_arrays.try_borrow();
        match (objects, arrays, obj_arrays) {
            (Some(objects), Some(arrays), Some(obj_arrays)) => {
                objects.len() + arrays.len() + obj_arrays.len()
            }
            _ => return Ok(()),
        }
    };
    let last = LAST_SWEEP_TOTAL.load(Ordering::Relaxed);
    if total.saturating_sub(last) < SWEEP_THRESHOLD {
        return Ok(());
    }
    let id = std::thread::current().id();
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
        let _ = collect_mark_sweep(loader, &frame_roots);
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
/// Every arena access uses `try_borrow*`; if a quiesced thread still
/// holds a borrow across a safepoint the collection is skipped for
/// this cycle rather than panicking (the arenas simply keep growing).
fn collect_mark_sweep<'a>(loader: &ClassLoader<'a>, frame_roots: &[Handle]) -> io::Result<()> {
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

    let mut marks = Marks::default();
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

    drain(&objects, &obj_arrays, &mut marks, &mut worklist);
    drop((
        objects,
        obj_arrays,
        classes,
        lambda_classes,
        extra_roots,
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
    *loader.reference_pending.borrow_mut() = pending.first().copied();
    crate::share::runtime::threading::wake_all(loader);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::oops::typeArrayOop::TypeArrayOop;

    #[test]
    fn releasesUnreachableArrayStorageAndReusesSlot() {
        let arena = crate::share::runtime::syncBump::SyncBump::new();
        let loader = ClassLoader::open(std::path::Path::new("."), &arena).unwrap();
        let handle = loader.alloc_type_array(TypeArrayOop::Byte(vec![1; 1_000_000]));
        // Force a collection directly (bypasses the growth threshold
        // used by the interpreter-driven `collect`).
        let _ = collect_mark_sweep(&loader, &[]);
        assert_eq!(loader.arrays.borrow()[handle].len(), 0);
        let reused = loader.alloc_type_array(TypeArrayOop::Int(vec![42]));
        assert_eq!(reused, handle);
        assert_eq!(loader.arrays.borrow()[reused].len(), 1);
    }
}
