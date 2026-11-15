

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    oops::instanceKlass::InstanceKlass,
    runtime::javaCalls::{JavaCallArguments, JavaCalls},
};
use std::{
    collections::HashMap,
    io::{self, Write},
    sync::{Arc, Mutex},
};

pub const EVENT_CONTROL_CLASS: &[u8] = b"jdk/jfr/internal/event/EventControl";

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

#[derive(Debug, Clone)]
pub struct InstrumentedEventClass {

    pub name: Vec<u8>,

    pub control_class_name: Vec<u8>,

    pub retransform_pending: bool,

    pub transform_count: u64,

    pub registration_order: u64,
}

struct EventClassRegistry {
    classes: HashMap<Vec<u8>, InstrumentedEventClass>,

    unloaded: Vec<Vec<u8>>,

    original_bytes: HashMap<Vec<u8>, Vec<u8>>,
}

static EVENT_CLASSES: std::sync::LazyLock<Mutex<EventClassRegistry>> =
    std::sync::LazyLock::new(|| {
        Mutex::new(EventClassRegistry {
            classes: HashMap::new(),
            unloaded: Vec::new(),
            original_bytes: HashMap::new(),
        })
    });
static FORCE_INSTRUMENTATION: Mutex<bool> = Mutex::new(false);
static NEXT_REGISTRATION: Mutex<u64> = Mutex::new(1);

fn registry() -> std::sync::MutexGuard<'static, EventClassRegistry> {
    EVENT_CLASSES.lock().unwrap()
}

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

pub fn is_instrumented(klass: &InstanceKlass<'_>) -> bool {
    registry().classes.contains_key(klass.class.name)
}

pub fn is_instrumented_mirror<'a>(
    loader: &ClassLoader<'a>,
    clazz: Value<'a>,
) -> io::Result<bool> {
    let klass = crate::share::jfr::jni::java_support::resolve_mirror_klass(loader, clazz)?;
    Ok(is_instrumented(&klass))
}

pub fn register_event_class(
    klass: &InstanceKlass<'_>,
    original_bytes: Option<Vec<u8>>,
) -> Option<u64> {
    let name = klass.class.name.to_vec();
    let mut reg = registry();
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

pub fn mark_retransform_pending(class_name: &[u8]) {
    let mut reg = registry();
    if let Some(entry) = reg.classes.get_mut(class_name) {
        entry.retransform_pending = true;
    }
}

pub fn clear_retransform_pending(class_name: &[u8]) {
    let mut reg = registry();
    if let Some(entry) = reg.classes.get_mut(class_name) {
        entry.retransform_pending = false;
        entry.transform_count += 1;
    }
}

pub fn control_klass<'a>(
    loader: &ClassLoader<'a>,
    event_class: &InstanceKlass<'a>,
) -> io::Result<Option<Arc<InstanceKlass<'a>>>> {
    let control_name = {
        let reg = registry();
        let Some(entry) = reg.classes.get(event_class.class.name) else {
            return Ok(None);
        };
        entry.control_class_name.clone()
    };
    match loader.load(&control_name) {
        Ok(klass) => Ok(Some(klass)),
        Err(_) => Ok(None),
    }
}

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

pub fn instrumented_classes() -> Vec<InstrumentedEventClass> {
    let reg = registry();
    let mut classes: Vec<InstrumentedEventClass> =
        reg.classes.values().cloned().collect();
    classes.sort_by_key(|entry| entry.registration_order);
    classes
}

pub fn unloaded_event_class_count() -> u64 {
    registry().unloaded.len() as u64
}

pub fn on_class_unload(class_name: &[u8]) {
    let mut reg = registry();
    if let Some(entry) = reg.classes.remove(class_name) {
        reg.unloaded.push(entry.name);
    }
}

pub fn original_bytes(class_name: &[u8]) -> Option<Vec<u8>> {
    registry().original_bytes.get(class_name).cloned()
}

pub fn store_original_bytes(class_name: &[u8], bytes: Vec<u8>) {
    registry()
        .original_bytes
        .insert(class_name.to_vec(), bytes);
}

pub fn set_force_instrumentation(enabled: bool) {
    *FORCE_INSTRUMENTATION.lock().unwrap() = enabled;
}

pub fn force_instrumentation() -> bool {
    *FORCE_INSTRUMENTATION.lock().unwrap()
}

pub fn name_may_be_event_class(name: &[u8]) -> bool {

    !name.is_empty() && name != b"jdk/jfr/Event"
}

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
