/*
 * Port of OpenJDK 25 HotSpot JfrJavaSupport
 * (share/jfr/jni/jfrJavaSupport.hpp / jfrJavaSupport.cpp)
 *
 * Java-side support routines used across the JFR subsystem: object and
 * string construction, field access, exception raising, jdk.jfr module
 * availability checks and thread (ex)clusion bookkeeping.
 *
 * In this port "jobject"/"oop" handles are `Value`s produced by the
 * interpreter heap; there is no JNI handle table, so local/global JNI
 * handle helpers collapse to identity conversions while keeping the
 * same call shapes as the C++ original.
 */

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::{
        bytecodeInterpreter::{self, Value},
        javaThrowable::JavaThrowable,
    },
    oops::{instanceKlass::InstanceKlass, instanceOop::InstanceOop},
    runtime::javaCalls::JavaValue,
    jfr::jni::java_call::{JfrJavaArguments, JfrJavaCall},
};
use std::{
    collections::HashSet,
    io::{self, Write},
    sync::{Arc, Mutex},
};

const JDK_JFR_MODULE_NAME: &str = "jdk.jfr";
const JDK_JFR_PACKAGE_NAME: &[u8] = b"jdk/jfr";

// ---------------------------------------------------------------------------
// Thread exclusion registry
// (JfrThreadLocal exclusion flags live in the recorder core; until it lands
// we keep the same exclusion semantics in a process-wide registry keyed by
// the thread object identity used by JfrJavaSupport::jfr_thread_id.)
// ---------------------------------------------------------------------------

static EXCLUDED_THREADS: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

fn excluded_set() -> std::sync::MutexGuard<'static, Option<HashSet<u64>>> {
    EXCLUDED_THREADS.lock().unwrap()
}

/// `JfrJavaSupport::jfr_thread_id` analogue: stable trace id for a
/// `java.lang.Thread` object. Prefers the thread's `tid` field and falls
/// back to the object handle identity.
pub fn jfr_thread_id<'a>(
    loader: &ClassLoader<'a>,
    thread: Value<'a>,
) -> io::Result<i64> {
    let Value::Object(handle) = thread else {
        return Ok(0);
    };
    let objects = loader.borrow_objects();
    let Some(object) = objects.get(handle) else {
        return Ok(0);
    };
    if let Ok(slot) = object.klass.instanceField(b"tid", b"J")
        && let Some(Value::Long(tid)) = object.fields.get(slot).copied()
    {
        return Ok(tid);
    }
    Ok(handle as i64)
}

/// `JfrJavaSupport::exclude`.
pub fn exclude_thread<'a>(loader: &ClassLoader<'a>, thread: Value<'a>) -> io::Result<()> {
    let id = jfr_thread_id(loader, thread)?;
    if id != 0 {
        excluded_set().get_or_insert_with(HashSet::new).insert(id as u64);
    }
    Ok(())
}

/// `JfrJavaSupport::include`.
pub fn include_thread<'a>(loader: &ClassLoader<'a>, thread: Value<'a>) -> io::Result<()> {
    let id = jfr_thread_id(loader, thread)?;
    if id != 0 && let Some(set) = excluded_set().as_mut() {
        set.remove(&(id as u64));
    }
    Ok(())
}

/// `JfrJavaSupport::is_excluded(jobject)`.
pub fn is_thread_excluded<'a>(loader: &ClassLoader<'a>, thread: Value<'a>) -> io::Result<bool> {
    let id = jfr_thread_id(loader, thread)?;
    Ok(id != 0 && excluded_set().as_ref().is_some_and(|set| set.contains(&(id as u64))))
}

/// `JfrJavaSupport::on_thread_start`: true when the thread may be
/// instrumented (i.e. is not excluded).
pub fn on_thread_start<'a>(loader: &ClassLoader<'a>, thread: Value<'a>) -> io::Result<bool> {
    Ok(!is_thread_excluded(loader, thread)?)
}

// ---------------------------------------------------------------------------
// Object / string construction
// ---------------------------------------------------------------------------

/// `java_lang_String::create_oop_from_str` + local JNI handle. Creates a
/// real `java.lang.String` instance.
pub fn new_string<'a>(
    loader: &ClassLoader<'a>,
    text: &str,
    output: &mut dyn Write,
) -> io::Result<Value<'a>> {
    let klass = loader.load(b"java/lang/String")?;
    bytecodeInterpreter::initialize(loader, &klass, output, 0)?;

    let units: Vec<u16> = text.encode_utf16().collect();
    let latin1 = units.iter().all(|&unit| unit <= 0xff);
    let bytes: Vec<i8> = if latin1 {
        units.iter().map(|&unit| unit as i8).collect()
    } else {
        units.iter().flat_map(|unit| unit.to_ne_bytes()).map(|b| b as i8).collect()
    };
    let array = loader.alloc_type_array(crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes));

    let mut object = InstanceOop::new(klass)?;
    let value_index = object.klass.instanceField(b"value", b"[B")?;
    let coder_index = object.klass.instanceField(b"coder", b"B")?;
    object.fields[value_index] = Value::PrimitiveArray(array);
    object.fields[coder_index] = Value::Int(i32::from(!latin1));
    if let Ok(hash_index) = object.klass.instanceField(b"hash", b"I") {
        object.fields[hash_index] = Value::Int(0);
    }
    Ok(Value::Object(loader.alloc_object(object)))
}

/// `JfrJavaSupport::c_str`: decode a string-ish `Value` to owned text.
pub fn c_str<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> Option<String> {
    crate::share::prims::stringNatives::string(loader, value)
        .ok()
        .flatten()
        .map(Cow::into_owned)
}

/// `JfrJavaSupport::c_str(jstring)` accepting only real string objects.
pub fn c_str_object<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> Option<String> {
    c_str(loader, value)
}

/// `object_construction` + `create_object`: allocate an instance and run
/// its no-arg constructor.
pub fn new_object<'a>(
    loader: &ClassLoader<'a>,
    klass_name: &[u8],
    output: &mut dyn Write,
) -> io::Result<Value<'a>> {
    let klass = loader.load(klass_name)?;
    bytecodeInterpreter::initialize(loader, &klass, output, 0)?;
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        b"<init>",
        b"()V",
    );
    args.set_receiver(Value::Object(handle));
    args.call_special(loader, output)?;
    Ok(Value::Object(handle))
}

/// `JfrJavaSupport::new_java_lang_Boolean`.
pub fn new_java_lang_boolean<'a>(
    loader: &ClassLoader<'a>,
    value: bool,
    output: &mut dyn Write,
) -> io::Result<Value<'a>> {
    let klass = loader.load(b"java/lang/Boolean")?;
    bytecodeInterpreter::initialize(loader, &klass, output, 0)?;
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        b"<init>",
        b"(Z)V",
    );
    args.set_receiver(Value::Object(handle));
    args.push_int(i32::from(value));
    args.call_special(loader, output)?;
    Ok(Value::Object(handle))
}

/// `JfrJavaSupport::new_java_lang_Integer`.
pub fn new_java_lang_integer<'a>(
    loader: &ClassLoader<'a>,
    value: i32,
    output: &mut dyn Write,
) -> io::Result<Value<'a>> {
    let klass = loader.load(b"java/lang/Integer")?;
    bytecodeInterpreter::initialize(loader, &klass, output, 0)?;
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        b"<init>",
        b"(I)V",
    );
    args.set_receiver(Value::Object(handle));
    args.push_int(value);
    args.call_special(loader, output)?;
    Ok(Value::Object(handle))
}

/// `JfrJavaSupport::new_java_lang_Long`.
pub fn new_java_lang_long<'a>(
    loader: &ClassLoader<'a>,
    value: i64,
    output: &mut dyn Write,
) -> io::Result<Value<'a>> {
    let klass = loader.load(b"java/lang/Long")?;
    bytecodeInterpreter::initialize(loader, &klass, output, 0)?;
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        b"<init>",
        b"(J)V",
    );
    args.set_receiver(Value::Object(handle));
    args.push_long(value);
    args.call_special(loader, output)?;
    Ok(Value::Object(handle))
}

/// `JfrJavaSupport::set_array_element`.
pub fn set_array_element<'a>(
    loader: &ClassLoader<'a>,
    array: Value<'a>,
    element: Value<'a>,
    index: usize,
) -> io::Result<()> {
    let Value::ObjectArray(handle) = array else {
        return Err(io::Error::other("set_array_element: not an object array"));
    };
    let mut arrays = loader.obj_arrays.borrow_mut();
    let Some(array) = arrays.get_mut(handle) else {
        return Err(io::Error::other("set_array_element: invalid array"));
    };
    array.obj_at_put(index, element)
}

// ---------------------------------------------------------------------------
// Field access
// ---------------------------------------------------------------------------

/// `read_specialized_field`/`write_specialized_field` adapted to the
/// interpreter heap where fields are stored as `Value`s.
fn field_slot<'a>(
    loader: &ClassLoader<'a>,
    receiver: Option<Value<'a>>,
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<(bool, usize)> {
    if let Some(receiver) = receiver {
        let Value::Object(handle) = receiver else {
            return Err(io::Error::other("instance field access on non-object"));
        };
        let objects = loader.borrow_objects();
        let object = objects
            .get(handle)
            .ok_or_else(|| io::Error::other("Invalid object reference"))?;
        let slot = object.klass.instanceField(name, descriptor)?;
        Ok((false, slot))
    } else {
        let _ = klass;
        Ok((true, 0))
    }
}

/// `JfrJavaSupport::get_field`: read an instance or static field and
/// return it as a `JavaValue`.
pub fn get_field<'a>(
    loader: &ClassLoader<'a>,
    klass_name: &[u8],
    receiver: Option<Value<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<JavaValue<'a>> {
    let klass = loader.load(klass_name)?;
    bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0)?;
    if receiver.is_some() {
        let (.., slot) = field_slot(loader, receiver, &klass, name, descriptor)?;
        let Value::Object(handle) = receiver.unwrap() else {
            unreachable!()
        };
        let objects = loader.borrow_objects();
        let object = objects
            .get(handle)
            .ok_or_else(|| io::Error::other("Invalid object reference"))?;
        let value = object
            .fields
            .get(slot)
            .copied()
            .ok_or_else(|| io::Error::other("Field slot out of range"))?;
        Ok(JavaValue::Value(value))
    } else {
        let cell = klass.staticField(name, descriptor)?;
        Ok(JavaValue::Value(cell.get()))
    }
}

/// `JfrJavaSupport::set_field`.
pub fn set_field<'a>(
    loader: &ClassLoader<'a>,
    klass_name: &[u8],
    receiver: Option<Value<'a>>,
    name: &[u8],
    descriptor: &[u8],
    value: Value<'a>,
) -> io::Result<()> {
    let klass = loader.load(klass_name)?;
    bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0)?;
    if receiver.is_some() {
        let (_, slot) = field_slot(loader, receiver, &klass, name, descriptor)?;
        let Value::Object(handle) = receiver.unwrap() else {
            unreachable!()
        };
        let mut objects = loader.borrow_objects_mut();
        let object = objects
            .get_mut(handle)
            .ok_or_else(|| io::Error::other("Invalid object reference"))?;
        if slot >= object.fields.len() {
            return Err(io::Error::other("Field slot out of range"));
        }
        object.fields[slot] = value;
        Ok(())
    } else {
        let cell = klass.staticField(name, descriptor)?;
        cell.set(value);
        Ok(())
    }
}

/// Convenience wrappers matching `get_field`/`set_field` on a
/// `JfrJavaArguments`-style call site are provided by the callers; these
/// two helpers cover the common read path used by dcmd argument
/// descriptors.
pub fn get_string_field<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    name: &[u8],
) -> io::Result<Option<String>> {
    let Value::Object(handle) = object else {
        return Ok(None);
    };
    let objects = loader.borrow_objects();
    let Some(instance) = objects.get(handle) else {
        return Ok(None);
    };
    let klass = Arc::clone(&instance.klass);
    drop(objects);
    let value = get_field(loader, klass.class.name, Some(object), name, b"Ljava/lang/String;")?;
    let JavaValue::Value(value) = value else {
        return Ok(None);
    };
    Ok(c_str(loader, value))
}

pub fn get_boolean_field<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    name: &[u8],
) -> io::Result<bool> {
    let Value::Object(handle) = object else {
        return Ok(false);
    };
    let objects = loader.borrow_objects();
    let Some(instance) = objects.get(handle) else {
        return Ok(false);
    };
    let klass = Arc::clone(&instance.klass);
    drop(objects);
    let value = get_field(loader, klass.class.name, Some(object), name, b"Z")?;
    Ok(matches!(value, JavaValue::Value(Value::Int(v)) if v != 0))
}

// ---------------------------------------------------------------------------
// Exceptions and errors
// ---------------------------------------------------------------------------

fn throw_java<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: Option<&str>) -> io::Error {
    let result = (|| -> io::Result<usize> {
        match message {
            Some(text) => {
                let message_value = Value::HostString(Box::leak(text.to_string().into_boxed_str()));
                new_throwable(loader, class_name, b"(Ljava/lang/String;)V", &[message_value])
                    .or_else(|_| new_throwable(loader, class_name, b"()V", &[]))
            }
            None => new_throwable(loader, class_name, b"()V", &[]),
        }
    })();
    match result {
        Ok(handle) => io::Error::other(JavaThrowable(handle)),
        Err(_) => io::Error::other(String::from_utf8_lossy(class_name).replace('/', ".")),
    }
}

/// `new_throwable` adapted from the interpreter helpers: allocates the
/// throwable and invokes a matching constructor when present.
fn new_throwable<'a>(
    loader: &ClassLoader<'a>,
    class_name: &[u8],
    ctor: &[u8],
    args: &[Value<'a>],
) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0);
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(handle));
    let result = (|| -> io::Result<()> {
        let member = klass.class.methods.iter().find(|method| {
            method.name == b"<init>"
                && method.descriptor == ctor
                && method.accessFlags & 0x0008 == 0
        });
        if let Some(member) = member {
            let code = crate::share::oops::method::Code::parse(member)?;
            let mut call_args = Vec::with_capacity(args.len() + 1);
            call_args.push(Value::Object(handle));
            call_args.extend_from_slice(args);
            bytecodeInterpreter::execute(
                loader,
                &klass,
                code,
                &call_args,
                &mut io::sink(),
                0,
                member.accessFlags,
            )?;
        }
        Ok(())
    })();
    let root_position = {
        let roots = loader.gc_extra_roots.borrow();
        roots
            .iter()
            .rposition(|value| matches!(value, Value::Object(root) if *root == handle))
    };
    if let Some(position) = root_position {
        loader.gc_extra_roots.borrow_mut().remove(position);
    }
    result?;
    Ok(handle)
}

/// `JfrJavaSupport::throw_illegal_state_exception`.
pub fn throw_illegal_state_exception<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/lang/IllegalStateException", Some(message))
}

/// `JfrJavaSupport::throw_internal_error`.
pub fn throw_internal_error<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/lang/InternalError", Some(message))
}

/// `JfrJavaSupport::throw_illegal_argument_exception`.
pub fn throw_illegal_argument_exception<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/lang/IllegalArgumentException", Some(message))
}

/// `JfrJavaSupport::throw_out_of_memory_error`.
pub fn throw_out_of_memory_error<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/lang/OutOfMemoryError", Some(message))
}

/// `JfrJavaSupport::throw_class_format_error`.
pub fn throw_class_format_error<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/lang/ClassFormatError", Some(message))
}

/// `JfrJavaSupport::throw_runtime_exception`.
pub fn throw_runtime_exception<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/lang/RuntimeException", Some(message))
}

/// `JfrJavaSupport::uncaught_exception` cause classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    VmError,
    OutOfMemory,
    StackOverflow,
    RuntimeException,
    Unknown,
}

static UNCAUGHT_CAUSE: Mutex<Cause> = Mutex::new(Cause::VmError);

fn is_subclass_of<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    super_name: &[u8],
) -> bool {
    let Value::Object(handle) = object else {
        return false;
    };
    let objects = loader.borrow_objects();
    let Some(instance) = objects.get(handle) else {
        return false;
    };
    let mut current = Some(Arc::clone(&instance.klass));
    while let Some(klass) = current {
        if klass.class.name == super_name {
            return true;
        }
        current = klass.super_klass();
    }
    false
}

/// `JfrJavaSupport::uncaught_exception(jthrowable, thread)`.
pub fn uncaught_exception<'a>(loader: &ClassLoader<'a>, throwable: Value<'a>) {
    let cause = if is_subclass_of(loader, throwable, b"java/lang/OutOfMemoryError") {
        Cause::OutOfMemory
    } else if is_subclass_of(loader, throwable, b"java/lang/StackOverflowError") {
        Cause::StackOverflow
    } else if is_subclass_of(loader, throwable, b"java/lang/Error") {
        Cause::VmError
    } else if is_subclass_of(loader, throwable, b"java/lang/RuntimeException") {
        Cause::RuntimeException
    } else if is_subclass_of(loader, throwable, b"java/lang/Exception") {
        Cause::Unknown
    } else {
        Cause::VmError
    };
    *UNCAUGHT_CAUSE.lock().unwrap() = cause;
}

/// `JfrJavaSupport::cause()`.
pub fn cause() -> Cause {
    *UNCAUGHT_CAUSE.lock().unwrap()
}

/// `JfrJavaSupport::abort`.
pub fn abort(message: Option<&str>, dump_core: bool) {
    if let Some(message) = message {
        eprintln!("[jfr] {message}");
    }
    eprintln!("[jfr] An irrecoverable error in Jfr. Shutting down VM...");
    let _ = dump_core;
    std::process::exit(134);
}

// ---------------------------------------------------------------------------
// jdk.jfr module availability
// ---------------------------------------------------------------------------

/// `JfrJavaSupport::load_jdk_jfr_module`: force resolution of the module
/// by loading a representative class through the loader.
pub fn load_jdk_jfr_module<'a>(loader: &ClassLoader<'a>, output: &mut dyn Write) -> io::Result<()> {
    let _ = loader.load(b"jdk/jfr/FlightRecorder")?;
    bytecodeInterpreter::initialize(loader, &loader.load(b"jdk/jfr/internal/JVM")?, output, 0)?;
    Ok(())
}

/// `is_jdk_jfr_module_in_readability_graph` analogue: the module is
/// available when its classes resolve through the loader.
pub fn is_jdk_jfr_module_available<'a>(loader: &ClassLoader<'a>) -> bool {
    loader.load(b"jdk/jfr/internal/JVM").is_ok()
}

/// `JfrJavaSupport::is_jdk_jfr_module_available(out, THREAD)`.
pub fn is_jdk_jfr_module_available_out<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
) -> bool {
    if !is_jdk_jfr_module_available(loader) {
        let _ = writeln!(
            output,
            "Module {JDK_JFR_MODULE_NAME} not found.\nFlight Recorder can not be enabled."
        );
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// EventConfiguration field access on class mirrors
// ---------------------------------------------------------------------------

/// `get_configuration_field_descriptor` analogue: locate the
/// `eventConfiguration` static field on the event class, typed or
/// untyped (`Ljava/lang/Object;`).
fn configuration_field<'a>(
    _loader: &ClassLoader<'a>,
    klass: &Arc<InstanceKlass<'a>>,
) -> io::Result<(&'static [u8], &'static [u8])> {
    for (name, descriptor) in [
        (
            b"eventConfiguration".as_slice(),
            b"Ljdk/jfr/internal/event/EventConfiguration;".as_slice(),
        ),
        (b"eventConfiguration".as_slice(), b"Ljava/lang/Object;".as_slice()),
    ] {
        if klass
            .static_field_infos
            .iter()
            .any(|field| field.name == name && field.descriptor == descriptor)
        {
            return Ok((name, descriptor));
        }
    }
    Err(io::Error::other("Missing eventConfiguration field"))
}

/// `JfrJavaSupport::get_configuration`.
pub fn get_configuration<'a>(
    loader: &ClassLoader<'a>,
    clazz: Value<'a>,
) -> io::Result<Option<Value<'a>>> {
    let klass = resolve_mirror_klass(loader, clazz)?;
    let (name, descriptor) = match configuration_field(loader, &klass) {
        Ok(pair) => pair,
        Err(_) => return Ok(None),
    };
    let cell = klass.staticField(name, descriptor)?;
    let value = cell.get();
    Ok(if matches!(value, Value::Null) { None } else { Some(value) })
}

/// `JfrJavaSupport::set_configuration`.
pub fn set_configuration<'a>(
    loader: &ClassLoader<'a>,
    clazz: Value<'a>,
    configuration: Value<'a>,
) -> io::Result<bool> {
    let klass = resolve_mirror_klass(loader, clazz)?;
    let (name, descriptor) = match configuration_field(loader, &klass) {
        Ok(pair) => pair,
        Err(_) => return Ok(false),
    };
    let cell = klass.staticField(name, descriptor)?;
    cell.set(configuration);
    Ok(true)
}

/// `JfrJavaSupport::is_instrumented`: true when the class mirror denotes
/// an instrumented jdk.jfr.Event subclass. Delegates to the
/// instrumentation module.
pub fn is_instrumented<'a>(
    loader: &ClassLoader<'a>,
    clazz: Value<'a>,
) -> io::Result<bool> {
    let klass = resolve_mirror_klass(loader, clazz)?;
    Ok(crate::share::jfr::instrumentation::event_class_transformer::is_instrumented(&klass))
}

/// Resolves a `jclass` argument (`Value::ClassMirror` or a real
/// `java.lang.Class` object) to its `InstanceKlass`.
pub fn resolve_mirror_klass<'a>(
    loader: &ClassLoader<'a>,
    clazz: Value<'a>,
) -> io::Result<Arc<InstanceKlass<'a>>> {
    match clazz {
        Value::ClassMirror(name) => loader.load(name),
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            let object = objects
                .get(handle)
                .ok_or_else(|| io::Error::other("Invalid class object"))?;
            if object.klass.class.name != b"java/lang/Class" {
                return Err(io::Error::other("Expected java.lang.Class object"));
            }
            // In this port class objects are mirrors; recover the name
            // from the `name` field when present.
            if let Ok(slot) = object.klass.instanceField(b"name", b"Ljava/lang/String;") {
                if let Some(value) = object.fields.get(slot).copied() {
                    if let Some(text) = c_str(loader, value) {
                        let internal = text.replace('.', "/");
                        return loader.load(internal.as_bytes());
                    }
                }
            }
            Err(io::Error::other("Cannot resolve class mirror"))
        }
        _ => Err(io::Error::other("Expected class mirror")),
    }
}

/// `JfrJavaSupport::compute_field_offset`.
pub fn compute_field_offset<'a>(
    loader: &ClassLoader<'a>,
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
    is_static: bool,
) -> io::Result<usize> {
    if is_static {
        let _ = klass.staticField(name, descriptor)?;
        return Ok(0);
    }
    klass.instanceField(name, descriptor)
}

/// `JfrJavaSupport::create_long_array`.
pub fn create_long_array<'a>(
    loader: &ClassLoader<'a>,
    values: &[i64],
) -> io::Result<Value<'a>> {
    let array = crate::share::oops::typeArrayOop::TypeArrayOop::Long(values.to_vec());
    Ok(Value::PrimitiveArray(loader.alloc_type_array(array)))
}

/// `JfrJavaSupport::notify_all` analogue is intentionally absent: object
/// monitors in this port do not expose a Java-level notify path; callers
/// that need it must go through `java.lang.Object.notifyAll` via
/// `JfrJavaCall`.
