

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    jfr::instrumentation::event_class_transformer as event_classes,
    oops::instanceKlass::InstanceKlass,
    prims::jvmti::JvmtiEnv,
};
use std::{
    io,
    sync::{Arc, Mutex, RwLock},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformOutcome {

    Unchanged,

    Instrumented { new_bytes: Option<Vec<u8>> },
}

pub type ByteTransformer =
    dyn Fn(&ClassLoader<'_>, &InstanceKlass<'_>, &[u8]) -> io::Result<Option<Vec<u8>>> + Send + Sync;

static BYTE_TRANSFORMER: RwLock<Option<Arc<ByteTransformer>>> = RwLock::new(None);

pub fn set_byte_transformer(transformer: Arc<ByteTransformer>) {
    *BYTE_TRANSFORMER.write().unwrap() = Some(transformer);
}

pub fn should_instrument(klass: &InstanceKlass<'_>) -> bool {
    if !event_classes::is_event_class(klass) {
        return false;
    }
    if event_classes::is_instrumented(klass) {
        return false;
    }
    event_classes::force_instrumentation() || has_enabled_configuration(klass)
}

fn has_enabled_configuration(klass: &InstanceKlass<'_>) -> bool {
    let Ok(cell) = klass.staticField(
        b"eventConfiguration",
        b"Ljdk/jfr/internal/event/EventConfiguration;",
    ) else {
        return false;
    };
    let value = cell.get();
    !matches!(value, Value::Null)
}

pub fn transform<'a>(
    loader: &ClassLoader<'a>,
    klass: &Arc<InstanceKlass<'a>>,
    class_bytes: Option<&[u8]>,
    env: Option<&JvmtiEnv>,
    output: &mut dyn std::io::Write,
) -> io::Result<TransformOutcome> {
    if !should_instrument(klass) {
        return Ok(TransformOutcome::Unchanged);
    }

    if let Some(bytes) = class_bytes {
        event_classes::store_original_bytes(klass.class.name, bytes.to_vec());
    }

    let new_bytes = {
        let transformer = BYTE_TRANSFORMER.read().unwrap().clone();
        match transformer {
            Some(rewriter) => rewriter(loader, klass, class_bytes.unwrap_or(&[]))?,
            None => None,
        }
    };

    event_classes::register_event_class(klass, class_bytes.map(|b| b.to_vec()));

    ensure_control_class(loader, klass, output)?;

    if new_bytes.is_some() {

        event_classes::mark_retransform_pending(klass.class.name);
        enqueue_retransform(klass.class.name);
        request_retransform(env, klass.class.name);
    }

    Ok(TransformOutcome::Instrumented { new_bytes })
}

fn ensure_control_class<'a>(
    loader: &ClassLoader<'a>,
    event_class: &InstanceKlass<'a>,
    output: &mut dyn std::io::Write,
) -> io::Result<()> {
    let control_name = event_classes::control_class_name(event_class.class.name);
    match loader.load(&control_name) {
        Ok(control) => {
            let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
                loader, &control, output, 0,
            );
            Ok(())
        }

        Err(_) => Ok(()),
    }
}

fn request_retransform(env: Option<&JvmtiEnv>, class_name: &[u8]) {
    let _ = (env, class_name);

}

static PENDING_RETRANSFORM: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

pub fn enqueue_retransform(class_name: &[u8]) {
    PENDING_RETRANSFORM
        .lock()
        .unwrap()
        .push(class_name.to_vec());
}

pub fn take_pending_retransform() -> Vec<Vec<u8>> {
    std::mem::take(&mut *PENDING_RETRANSFORM.lock().unwrap())
}

pub fn retransform_classes<'a>(
    loader: &ClassLoader<'a>,
    classes: &[Value<'a>],
    env: Option<&JvmtiEnv>,
    output: &mut dyn std::io::Write,
) -> io::Result<()> {
    for value in classes {
        let klass = crate::share::jfr::jni::java_support::resolve_mirror_klass(loader, *value)?;
        let bytes = event_classes::original_bytes(klass.class.name);
        transform(loader, &klass, bytes.as_deref(), env, output)?;
        event_classes::clear_retransform_pending(klass.class.name);
        let _ = event_classes::notify_retransformed(loader, &klass, output);
    }
    Ok(())
}

pub fn prepare_for_recording() {
    PENDING_RETRANSFORM.lock().unwrap().clear();
}

pub fn teardown() {
    PENDING_RETRANSFORM.lock().unwrap().clear();
}
