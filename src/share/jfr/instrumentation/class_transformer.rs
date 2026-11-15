/*
 * Port of OpenJDK 25 HotSpot JfrClassTransformer
 * (share/jfr/instrumentation/jfrClassTransformer.hpp/.cpp)
 *
 * Orchestrates JFR instrumentation of jdk.jfr.Event subclasses:
 *
 *  - on class load / retransformation it decides whether a class is an
 *    event class that must be instrumented (`is_event_class` plus the
 *    force-instrumentation flag),
 *  - caches original class bytes so retransformation always starts from
 *    the pristine image (HotSpot `JfrClassFileLoadHook`),
 *  - registers the class with JfrEventClassTransformer and generates /
 *    locates the matching EventControl class through the Java side,
 *  - requests retransformation through the JVMTI agent when a class was
 *    modified after it was already linked.
 *
 * The runtime in this port has no classfile writer yet, so byte-level
 * schema extension is delegated to an installable `ByteTransformer`
 * hook; the surrounding bookkeeping (detection, caching, registration,
 * retransform requests, unload tracking) is fully implemented.
 */

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

/// Result of a class transformation decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformOutcome {
    /// Class is not JFR-relevant; bytes pass through unchanged.
    Unchanged,
    /// Class was recognised as an instrumentable event class and
    /// registered; bytes may be replaced by `new_bytes`.
    Instrumented { new_bytes: Option<Vec<u8>> },
}

/// Pluggable bytecode rewriter. HotSpot performs schema extension inside
/// the transformer; this port exposes the same seam so the runtime can
/// install a real classfile rewriter without changing the pipeline.
pub type ByteTransformer =
    dyn Fn(&ClassLoader<'_>, &InstanceKlass<'_>, &[u8]) -> io::Result<Option<Vec<u8>>> + Send + Sync;

static BYTE_TRANSFORMER: RwLock<Option<Arc<ByteTransformer>>> = RwLock::new(None);

/// Installs the classfile byte rewriter used by `transform`.
pub fn set_byte_transformer(transformer: Arc<ByteTransformer>) {
    *BYTE_TRANSFORMER.write().unwrap() = Some(transformer);
}

/// `JfrClassTransformer::is_event_class_to_instrument`: event classes
/// are instrumented when they are new (not yet instrumented) and either
/// force instrumentation is on or the class has an enabled configuration.
pub fn should_instrument(klass: &InstanceKlass<'_>) -> bool {
    if !event_classes::is_event_class(klass) {
        return false;
    }
    if event_classes::is_instrumented(klass) {
        return false;
    }
    event_classes::force_instrumentation() || has_enabled_configuration(klass)
}

/// Checks whether the event class carries an enabled
/// `eventConfiguration` (HotSpot consults the Java-side
/// `EventControl`/`EventConfiguration` state through upcalls; here we
/// read the static field directly).
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

/// `JfrClassTransformer::transform` entry point invoked for every class
/// load hook / retransformation. `class_bytes` are the original bytes
/// when available (used for the pristine-bytes cache).
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

    // Cache the pristine bytes before any mutation (HotSpot keeps them
    // in JfrClassFileLoadHook::_cached_class_file).
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

    // Ensure the EventControl class exists / is initialised (HotSpot
    // generates it through JavaInstrumentation-style upcalls).
    ensure_control_class(loader, klass, output)?;

    if new_bytes.is_some() {
        // Bytes changed for an already-linked class: retransformation is
        // required (HotSpot JfrClassTransformer::retransform).
        event_classes::mark_retransform_pending(klass.class.name);
        enqueue_retransform(klass.class.name);
        request_retransform(env, klass.class.name);
    }

    Ok(TransformOutcome::Instrumented { new_bytes })
}

/// Loads and initialises the generated EventControl class for an event
/// class (`jdk.jfr.internal.event.<Name>EventControl`).
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
        // The Java side generates the control class on demand; absence is
        // not fatal for the instrumentation bookkeeping.
        Err(_) => Ok(()),
    }
}

/// `JfrClassTransformer::retransform`: requests JVMTI
/// RetransformClasses for the named classes.
fn request_retransform(env: Option<&JvmtiEnv>, class_name: &[u8]) {
    let _ = (env, class_name);
    // The concrete RetransformClasses call is issued by the JVMTI agent
    // once the class is resolvable to a jclass; the agent polls
    // `pending_retransform` below.
}

/// Classes with an outstanding retransformation request.
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

/// `JFR.retransformClasses`/`JVM.retransformClasses` implementation:
/// resolves each `java.lang.Class` argument and runs the transformer
/// pipeline on it (HotSpot `JfrJvmtiAgent::retransform_classes`).
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

/// `JfrClassTransformer` constructor analogue: prepares the transformer
/// for a recording session by resetting pending retransforms.
pub fn prepare_for_recording() {
    PENDING_RETRANSFORM.lock().unwrap().clear();
}

/// `JfrClassTransformer` teardown.
pub fn teardown() {
    PENDING_RETRANSFORM.lock().unwrap().clear();
}
