/*
 * Port of OpenJDK 25 HotSpot JfrEventClassTransformer
 * (share/jfr/instrumentation/jfrEventClassTransformer.hpp/.cpp)
 *
 * Tracks jdk.jfr.Event subclasses that have been instrumented by the
 * JFR class transformer. Keeps the mapping between an event class and
 * its generated EventControl class, records whether retransformation
 * is pending, and counts classes that were unloaded while instrumented.
 */

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    oops::instanceKlass::InstanceKlass,
    runtime::javaCalls::JavaCallArguments,
};
use std::{
    collections::HashMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};

/// Name of the generated event-control base class (HotSpot:
/// `jdk/jfr/internal/event/EventControl`).
pub const EVENT_CONTROL_CLASS: &[u8] = b"jdk/jfr/internal/event/EventControl";

/// `JfrEventClassTransformer::is_event_class` analogue: an event class
/// is any `jdk.jfr.Event` subclass (excluding `jdk.jfr.Event` itself).
pub fn is_event_class(klass: &InstanceKlass<'_>) -> bool {
    if klass.class.name == b"jdk/jfr/Event" {
        return false;
    }
    let mut current = klass.super_klass();
    while let Some(super_klass) = current {
        if super_klass.class.name == b"jdk/jfr/Event" {
            return true;
        }
        current = super_klass.super_klass();
    }
    false
}

/// Per-class instrumentation record (HotSpot `JfrEventClassTransformer::
/// JfrEventClass` fields flattened).
#[derive(Debug, Clone)]
pub struct InstrumentedEventClass {
    /// Internal (slash-separated) class name.
    pub name: Vec<u8>,
    /// Name of the matching `jdk.jfr.internal.event.EventControl`
    /// subclass (`jdk.jfr.internal.event.<EventName>EventControl`).
    pub control_class_name: Vec<u8>,
    /// HotSpot `_retransform` flag: the class was modified and a
    /// retransformation request is outstanding.
    pub retransform_pending: bool,
    /// Number of times the class was (re)transformed.
    pub transform_count: u64,
    /// Sequence number assigned when first registered.
    pub registration_order: u64,
}

static EVENT_CLASSES: Mutex<Option<EventClassRegistry>> = Mutex::new(None);
static FORCE_INSTRUMENTATION: Mutex<bool> = Mutex::new(false);
static NEXT_REGISTRATION: Mutex<u64> = Mutex::new(1);

struct EventClassRegistry {
    classes: HashMap<Vec<u8>, InstrumentedEventClass>,
    /// Classes that were instrumented and later unloaded (HotSpot keeps
    /// them in `_unloaded_event_classes` for `getUnloadedEventClassCount`).
    unloaded: Vec<Vec<u8>>,
    /// Original class bytes cache keyed by class name, used to restore
    /// bytes before retransformation (HotSpot `JfrClassFileLoadHook`).
    original_bytes: HashMap<Vec<u8>, Vec<u8>>,
}

fn registry() -> std::sync::MutexGuard<'static, Option<EventClassRegistry>> {
    EVENT_CLASSES.lock().unwrap()
}

fn registry_mut() -> &'static mut EventClassRegistry {
    registry().get_or_insert_with(|| EventClassRegistry {
        classes: HashMap::new(),
        unloaded: Vec::new(),
        original_bytes: HashMap::new(),
    })
}

/// Derives the EventControl class name for an event class, mirroring
/// HotSpot `JfrEventClassTransformer::control_class_name`:
/// `com.foo.MyEvent` -> `jdk/jfr/internal/event/MyEventEventControl`.
pub fn control_class_name(event_class_name: &[u8]) -> Vec<u8> {
    let short = event_class_name
        .rsplit(|&b| b == b'/')
        .next()
        .unwrap_or(event_class_name);
    let mut name = b"jdk/jfr/internal/event/".to_vec();
    name.extend_from_slice(short);
    name.extend_from_slice(b"EventControl");
    name
}

/// `JfrEventClassTransformer::is_instrumented(InstanceKlass*)`.
pub fn is_instrumented(klass: &InstanceKlass<'_>) -> bool {
    registry()
        .as_ref()
        .is_some_and(|reg| reg.classes.contains_key(klass.class.name))
}

/// `JfrEventClassTransformer::is_instrumented(jclass)` form used by the
/// JNI layer: resolves the mirror then checks the registry.
pub fn is_instrumented_mirror<'a>(
    loader: &ClassLoader<'a>,
    clazz: Value<'a>,
) -> io::Result<bool> {
    let klass = crate::share::jfr::jni::java_support::resolve_mirror_klass(loader, clazz)?;
    Ok(is_instrumented(&klass))
}

/// Registers an event class after successful transformation.
/// Returns the registration order, or `None` when the class was already
/// registered (HotSpot deduplicates by klass pointer).
pub fn register_event_class(
    klass: &InstanceKlass<'_>,
    original_bytes: Option<Vec<u8>>,
) -> Option<u64> {
    let name = klass.class.name.to_vec();
    let reg = registry_mut();
    if reg.classes.contains_key(&name) {
        return None;
    }
    let order = {
        let mut next = NEXT_REGISTRATION.lock().unwrap();
        let order = *next;
        *next += 1;
        order
    };
    let control = control_class_name(&name);
    if let Some(bytes) = original_bytes {
        reg.original_bytes.insert(name.clone(), bytes);
    }
    reg.classes.insert(
        name.clone(),
        InstrumentedEventClass {
            name,
            control_class_name: control,
            retransform_pending: false,
            transform_count: 1,
            registration_order: order,
        },
    );
    Some(order)
}

/// Marks the class as having an outstanding retransformation request.
pub fn mark_retransform_pending(class_name: &[u8]) {
    if let Some(reg) = registry().as_mut()
        && let Some(entry) = reg.classes.get_mut(class_name)
    {
        entry.retransform_pending = true;
    }
}

/// Clears the pending flag once retransformation completed (called from
/// the retransformation callback).
pub fn clear_retransform_pending(class_name: &[u8]) {
    if let Some(reg) = registry().as_mut()
        && let Some(entry) = reg.classes.get_mut(class_name)
    {
        entry.retransform_pending = false;
        entry.transform_count += 1;
    }
}

/// `JfrEventClassTransformer::get_control` analogue: resolves the
/// EventControl object for an event class through the Java side
/// (`jdk.jfr.internal.event.EventControl.getControlFor` style lookup).
/// Returns the control class `InstanceKlass` when it can be loaded.
pub fn control_klass<'a>(
    loader: &ClassLoader<'a>,
    event_class: &InstanceKlass<'a>,
) -> io::Result<Option<Arc<InstanceKlass<'a>>>> {
    let reg = registry();
    let Some(entry) = reg.as_ref().and_then(|r| r.classes.get(event_class.class.name)) else {
        return Ok(None);
    };
    let control_name = entry.control_class_name.clone();
    drop(reg);
    match loader.load(&control_name) {
        Ok(klass) => Ok(Some(klass)),
        Err(_) => Ok(None),
    }
}

/// Invokes `EventControl.<event>.commit()`-style notification: calls the
/// static `onRetransform`/`retransform` hook on the control class when
/// present. Mirrors `JfrEventClassTransformer::notify`.
pub fn notify_retransformed<'a>(
    loader: &ClassLoader<'a>,
    event_class: &InstanceKlass<'a>,
    output: &mut dyn Write,
) -> io::Result<()> {
    let Some(control) = control_klass(loader, event_class)? else {
        return Ok(());
    };
    let has_hook = control.class.methods.iter().any(|method| {
        method.name == b"onRetransform" && method.descriptor == b"()V"
    });
    if has_hook {
        let klass = Arc::clone(&control);
        let args = JavaCallArguments::new();
        let _ = JavaCalls::call_static(
            loader,
            &klass,
            b"onRetransform",
            b"()V",
            &args,
            output,
        );
    }
    Ok(())
}

/// Snapshot of all registered instrumented event classes, ordered by
/// registration order (HotSpot iterates a linked list).
pub fn instrumented_classes() -> Vec<InstrumentedEventClass> {
    let reg = registry();
    let Some(reg) = reg.as_ref() else {
        return Vec::new();
    };
    let mut classes: Vec<InstrumentedEventClass> =
        reg.classes.values().cloned().collect();
    classes.sort_by_key(|entry| entry.registration_order);
    classes
}

/// `JfrEventClassTransformer::get_unloaded_event_classes_count`.
pub fn unloaded_event_class_count() -> u64 {
    registry()
        .as_ref()
        .map(|reg| reg.unloaded.len() as u64)
        .unwrap_or(0)
}

/// Called from the class-unload JVMTI callback: moves the class record
/// to the unloaded list (HotSpot keeps the name for diagnostics).
pub fn on_class_unload(class_name: &[u8]) {
    let mut guard = registry();
    let Some(reg) = guard.as_mut() else {
        return;
    };
    if let Some(entry) = reg.classes.remove(class_name) {
        reg.unloaded.push(entry.name);
    }
}

/// Cached original (pre-transformation) bytes for retransformation.
pub fn original_bytes(class_name: &[u8]) -> Option<Vec<u8>> {
    registry()
        .as_ref()
        .and_then(|reg| reg.original_bytes.get(class_name).cloned())
}

pub fn store_original_bytes(class_name: &[u8], bytes: Vec<u8>) {
    registry_mut()
        .original_bytes
        .insert(class_name.to_vec(), bytes);
}

/// `JfrEventClassTransformer::set_force_instrumentation` /
/// `is_force_instrumentation`: when set, every `jdk.jfr.Event`
/// subclass is instrumented even without explicit enablement.
pub fn set_force_instrumentation(enabled: bool) {
    *FORCE_INSTRUMENTATION.lock().unwrap() = enabled;
}

pub fn force_instrumentation() -> bool {
    *FORCE_INSTRUMENTATION.lock().unwrap()
}

/// Whether a class name denotes an instrumentable event class without
/// needing a resolved `InstanceKlass` (name-prefix fast path used by the
/// class-load hook before the klass exists).
pub fn name_may_be_event_class(name: &[u8]) -> bool {
    // Conservative check: JDK event classes live under jdk/jfr or user
    // packages; the superclass walk in `is_event_class` is authoritative.
    // This fast path only rejects obvious non-candidates.
    !name.is_empty() && name != b"jdk/jfr/Event"
}

/// Collects the set of currently loaded event classes by scanning the
/// loader's class table (used by `getAllEventClasses` when the Java
/// registry is unavailable).
pub fn loaded_event_classes<'a>(
    loader: &ClassLoader<'a>,
) -> Vec<Arc<InstanceKlass<'a>>> {
    let mut result = Vec::new();
    let names: Vec<&'a [u8]> = {
        let classes = loader.classes.borrow();
        classes.keys().copied().collect()
    };
    for name in names {
        if let Ok(klass) = loader.load(name)
            && is_event_class(&klass)
        {
            result.push(klass);
        }
    }
    result
}

/// Helper producing a Java `Value` list of class mirrors for an array
/// of `InstanceKlass` (used by `getAllEventClasses`).
pub fn class_mirror_values<'a>(
    loader: &ClassLoader<'a>,
    klasses: &[Arc<InstanceKlass<'a>>],
) -> io::Result<Value<'a>> {
    let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
        klasses.len() as i32,
        b"java/lang/Class",
    )?;
    let mut mirrors = Vec::with_capacity(klasses.len());
    for klass in klasses {
        mirrors.push(Value::ClassMirror(klass.class.name));
    }
    for (index, mirror) in mirrors.into_iter().enumerate() {
        array.obj_at_put(index, mirror)?;
    }
    Ok(Value::ObjectArray(loader.alloc_obj_array(array)))
}

/// Builds a `java.util.ArrayList` containing the given values — used by
/// JNI natives returning `Ljava/util/List;`.
pub fn new_array_list<'a>(
    loader: &ClassLoader<'a>,
    values: &[Value<'a>],
    output: &mut dyn Write,
) -> io::Result<Value<'a>> {
    use crate::share::jfr::jni::java_support;
    let list = java_support::new_object(loader, b"java/util/ArrayList", output)?;
    let klass =
        java_support::resolve_mirror_klass(loader, Value::ClassMirror(b"java/util/ArrayList"))?;
    for value in values {
        let mut call = JavaCallArguments::with_receiver(list);
        call.push_oop(*value);
        let _ = JavaCalls::call_virtual(
            loader,
            &klass,
            b"add",
            b"(Ljava/lang/Object;)Z",
            &call,
            output,
        );
    }
    Ok(list)
}
