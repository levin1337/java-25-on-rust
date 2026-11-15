/*
 * Port of OpenJDK 25 HotSpot NativeLookup (share/prims/nativeLookup.hpp / nativeLookup.cpp)
 *
 * Implements standard JNI symbol name mangling and resolution of native method entry points:
 * Java_<class_name>_<method_name> and Java_<class_name>_<method_name>__<signature>
 */

use super::jni::NativeMethodFn;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

#[allow(dead_code)]
pub struct NativeLookup {
    registered_symbols: Mutex<HashMap<String, NativeMethodFn>>,
}

#[allow(dead_code)]
static GLOBAL_NATIVE_LOOKUP: OnceLock<NativeLookup> = OnceLock::new();

#[allow(dead_code)]
impl NativeLookup {
    pub fn new() -> Self {
        Self {
            registered_symbols: Mutex::new(HashMap::new()),
        }
    }

    pub fn global() -> &'static NativeLookup {
        GLOBAL_NATIVE_LOOKUP.get_or_init(Self::new)
    }

    /// Computes the pure (short) JNI name: Java_pkg_Cls_method
    pub fn pure_jni_name(class_name: &str, method_name: &str) -> String {
        let mut result = String::from("Java_");
        Self::mangle_name(class_name, &mut result);
        result.push('_');
        Self::mangle_name(method_name, &mut result);
        result
    }

    /// Computes the long JNI name: Java_pkg_Cls_method__sig
    pub fn long_jni_name(class_name: &str, method_name: &str, signature: &str) -> String {
        let mut result = Self::pure_jni_name(class_name, method_name);
        result.push_str("__");

        // Extract parameter types inside parentheses (params)ret
        let params = if let Some(open) = signature.find('(') {
            if let Some(close) = signature.find(')') {
                &signature[open + 1..close]
            } else {
                signature
            }
        } else {
            signature
        };

        Self::mangle_name(params, &mut result);
        result
    }

    fn mangle_name(s: &str, out: &mut String) {
        use std::fmt::Write;
        for unit in s.encode_utf16() {
            match unit {
                0x2f => out.push('_'),
                0x5f => out.push_str("_1"),
                0x3b => out.push_str("_2"),
                0x5b => out.push_str("_3"),
                0x30..=0x39 | 0x41..=0x5a | 0x61..=0x7a => {
                    out.push(char::from_u32(u32::from(unit)).unwrap())
                }
                _ => write!(out, "_0{unit:04x}").unwrap(),
            }
        }
    }

    pub fn register_symbol(&self, jni_name: &str, func: NativeMethodFn) {
        let mut guard = self.registered_symbols.lock().unwrap();
        guard.insert(jni_name.to_string(), func);
    }

    pub fn lookup(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
    ) -> Option<NativeMethodFn> {
        let guard = self.registered_symbols.lock().unwrap();

        // 1. Try long name first (overloaded methods)
        let long_name = Self::long_jni_name(class_name, method_name, signature);
        if let Some(func) = guard.get(&long_name) {
            return Some(*func);
        }

        // 2. Try pure (short) name
        let pure_name = Self::pure_jni_name(class_name, method_name);
        guard.get(&pure_name).copied()
    }
}

impl Default for NativeLookup {
    fn default() -> Self {
        Self::new()
    }
}

/// Handles native instance methods which can be resolved before bytecode
/// dispatch. The outer `Option` means "handled by the VM"; the inner `Option`
/// is the Java-level return value (`None` for void methods).
/// JNI natives bind to the class that declares the method. A call site may
/// name a subclass (e.g. `PhantomReference.clear0`), while the native is
/// declared on the superclass (`Reference.clear0`). Walk the superclass
/// chain and return the declaring class name, or `None` when the owner
/// cannot be loaded or no class in the chain declares the method.
fn native_declaring_owner<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
) -> Option<Vec<u8>> {
    let key = (
        owner.as_ptr() as usize,
        owner.len(),
        name.as_ptr() as usize,
        name.len(),
        descriptor.as_ptr() as usize,
        descriptor.len(),
    );
    if let Some(cached) = loader.native_declaring_cache.borrow().get(&key) {
        return cached.clone();
    }
    let (cacheable, result) = native_declaring_owner_uncached(loader, owner, name, descriptor);
    // Only memoize resolutions that saw a successfully loaded owner class;
        // a transient load failure (e.g. circularity during clinit) must not
        // poison the cache for later calls.
    if cacheable {
        loader
            .native_declaring_cache
            .borrow_mut()
            .insert(key, result.clone());
    }
    result
}

/// Returns `(cacheable, declaring_class_name)`. `cacheable` is false when
/// the owner class could not be loaded at all.
fn native_declaring_owner_uncached<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
) -> (bool, Option<Vec<u8>>) {
    let declares = |klass: &crate::share::oops::instanceKlass::InstanceKlass<'a>| {
        klass
            .class
            .methods
            .iter()
            .any(|method| method.name == name && method.descriptor == descriptor)
    };
    let Ok(first) = loader.load(owner) else {
        return (false, None);
    };
    if declares(&first) {
        // Common case: the constant-pool owner declares the method itself.
        return (true, None);
    }
    let mut current = first.super_klass.clone();
    while let Some(klass) = current {
        if declares(&klass) {
            return (true, Some(klass.class.name.to_vec()));
        }
        current = klass.super_klass.clone();
    }
    (true, None)
}

pub static NATIVE_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

static CALL_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    /// Label of the native method currently executing on this OS thread,
    /// used by hang diagnostics (JVM25_DUMP wall-clock dumps).
    static CURRENT_NATIVE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Returns the native method label currently executing on this thread.
pub fn current_native() -> String {
    CURRENT_NATIVE.with(|cell| cell.borrow().clone())
}

struct NativeGuard {
    previous: String,
    active: bool,
}

fn native_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("JVM25_DUMP").is_some())
}

impl NativeGuard {
    fn enter(owner: &[u8], name: &[u8]) -> Self {
        if !native_debug_enabled() {
            return Self {
                previous: String::new(),
                active: false,
            };
        }
        let label = format!(
            "{}.{}",
            String::from_utf8_lossy(owner),
            String::from_utf8_lossy(name)
        );
        crate::share::runtime::threading::debug_native(label.clone());
        let previous = CURRENT_NATIVE.with(|cell| {
            let mut cell = cell.borrow_mut();
            std::mem::replace(&mut *cell, label)
        });
        Self {
            previous,
            active: true,
        }
    }
}

impl Drop for NativeGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        CURRENT_NATIVE.with(|cell| {
            *cell.borrow_mut() = std::mem::take(&mut self.previous);
        });
        crate::share::runtime::threading::debug_native(std::mem::take(&mut self.previous));
    }
}

fn trace_native_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("JVM25_TRACE_NATIVE").is_some())
}

fn maybe_dump_stack(loader: &crate::share::classfile::classLoader::ClassLoader<'_>) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("JVM25_CALLDUMP").is_some()) {
        return;
    }
    if CALL_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 2_000_000 != 0 {
        return;
    }
    let stack: Vec<String> = loader
        .call_stack
        .snapshot()
        .iter()
        .rev()
        .take(30)
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect();
    eprintln!("[calldump] {}", stack.join(" <- "));
}

pub fn invokeInstance<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[crate::share::interpreter::bytecodeInterpreter::Value<'a>],
    output: &mut dyn std::io::Write,
    depth: usize,
) -> io::Result<Option<Option<crate::share::interpreter::bytecodeInterpreter::Value<'a>>>> {
    let _rg = crate::share::recprobe::Guard::enter("invokeInstance");
    let _ng = NativeGuard::enter(owner, name);
    NATIVE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    maybe_dump_stack(loader);
    use super::jvm::Jvm;
    use crate::share::interpreter::bytecodeInterpreter::Value;

    if trace_native_enabled() {
        eprintln!(
            "[invokeInstance] {}.{}{}",
            String::from_utf8_lossy(owner),
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
        );
    }

    if owner == b"jdk/internal/misc/Unsafe" {
        return super::unsafeNatives::invoke(loader, name, descriptor, arguments, output, depth).map_err(
            |error| {
                // Preserve real Java throwables (e.g. ThreadDeath) and
                // scheduler control errors (ThreadSuspended); only annotate
                // plain errors.
                if crate::share::interpreter::bytecodeInterpreter::throwable_of(&error).is_some()
                    || crate::share::runtime::threading::is_suspended(&error)
                {
                    return error;
                }
                io::Error::other(format!(
                    "Unsafe.{}{}: {}",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor),
                    error
                ))
            },
        );
    }

    if owner == b"java/lang/Class" {
        if let Some(value) = super::reflectionNatives::class_invoke(
            loader,
            name,
            descriptor,
            arguments,
        )? {
            return Ok(Some(Some(value)));
        }
    }

    if super::vmManagementNatives::is_owner(owner) {
        if let Some(result) =
            super::vmManagementNatives::invoke(loader, name, descriptor, arguments)?
        {
            return Ok(Some(result));
        }
    }

    if super::perfNatives::is_owner(owner) {
        if let Some(result) =
            super::perfNatives::invoke(loader, name, descriptor, arguments, output)?
        {
            return Ok(Some(result));
        }
    }

    if let Some(result) =
        super::varHandleNatives::invoke(loader, name, descriptor, arguments)?
    {
        return Ok(Some(result));
    }

    if let Some(result) =
        super::methodHandleNatives::invoke_instance(loader, owner, name, descriptor, arguments)?
    {
        return Ok(Some(result));
    }

    // TEST-ONLY wiring for urlNatives (to be placed by orchestrator).
    if (owner.starts_with(b"java/net/") || owner == b"java/lang/ClassLoader" || owner.starts_with(b"jvm25/"))
        && let Some(result) =
            super::urlNatives::invoke_instance(loader, owner, name, descriptor, arguments)?
    {
        return Ok(Some(result));
    }

    if super::selectorNatives::is_owner(owner) {
        if let Some(result) =
            super::selectorNatives::invoke(loader, name, descriptor, arguments)?
        {
            return Ok(Some(result));
        }
    }

    if owner == b"sun/nio/ch/FileKey" && name == b"init" {
        return super::nioFileNatives::file_key_init(loader, arguments)
            .map(|_| Some(None));
    }

    if owner == b"java/lang/ClassLoader"
        && (name == b"findLoadedClass0" || name == b"findBootstrapClass")
        && descriptor == b"(Ljava/lang/String;)Ljava/lang/Class;"
    {
        let binary = arguments
            .get(1)
            .copied()
            .and_then(|value| super::stringNatives::string(loader, value).ok().flatten());
        let Some(binary) = binary else {
            return Ok(Some(Some(Value::Null)));
        };
        // Binary name -> internal name ("java.lang.String" -> "java/lang/String",
        // "[Ljava.lang.String;" -> "[Ljava/lang/String;").
        let internal = binary.replace('.', "/");
        let internal = loader.storage.alloc_str(&internal);
        return match loader.load(internal.as_bytes()) {
            Ok(_) => Ok(Some(Some(Value::ClassMirror(internal.as_bytes())))),
            Err(_) => Ok(Some(Some(Value::Null))),
        };
    }

    if owner == b"java/lang/ClassLoader"
        && name == b"getResources"
        && descriptor == b"(Ljava/lang/String;)Ljava/util/Enumeration;"
    {
        let resource = arguments
            .get(1)
            .copied()
            .and_then(|value| super::stringNatives::string(loader, value).ok().flatten());
        let Some(resource) = resource else {
            return Err(io::Error::other("Invalid resource name"));
        };
        let locations = loader.resource_locations(resource.as_bytes())?;
        let url_klass = loader.load(b"java/net/URL")?;
        let mut url_objects = Vec::with_capacity(locations.len());
        for location in locations {
            let mut object = crate::share::oops::instanceOop::InstanceOop::new(url_klass.clone())?;
            let file = loader
                .storage
                .alloc_str(&format!("/{}", location.display()));
            let protocol = url_klass.instanceField(b"protocol", b"Ljava/lang/String;")?;
            let host = url_klass.instanceField(b"host", b"Ljava/lang/String;")?;
            let port = url_klass.instanceField(b"port", b"I")?;
            let file_index = url_klass.instanceField(b"file", b"Ljava/lang/String;")?;
            object.fields[protocol] = Value::HostString("file");
            object.fields[host] = Value::Null;
            object.fields[port] = Value::Int(-1);
            object.fields[file_index] = Value::HostString(file);
            url_objects.push(object);
        }
        let url_count = url_objects.len() as i32;
        let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
            url_count,
            b"java/net/URL",
        )?;
        let mut elements = url_objects.into_iter();
        for element in array.elements.iter_mut() {
            *element = Value::Object(loader.alloc_object(elements.next().unwrap()));
        }
        let list_klass = loader.load(b"java/util/ArrayList")?;
        let list = crate::share::oops::instanceOop::InstanceOop::new(list_klass.clone())?;
        let data_index = list_klass.instanceField(b"elementData", b"[Ljava/lang/Object;")?;
        let size_index = list_klass.instanceField(b"size", b"I")?;
        let list_handle = loader.alloc_object(list);
        let array_handle = loader.alloc_obj_array(array);
        {
            let mut objects = loader.borrow_objects_mut();
            let list = objects
                .get_mut(list_handle)
                .ok_or_else(|| io::Error::other("Invalid ArrayList object"))?;
            list.fields[data_index] = Value::ObjectArray(array_handle);
            list.fields[size_index] = Value::Int(url_count);
        }
        let enum_klass = loader.load(b"java/util/Collections$3")?;
        let mut output = std::io::sink();
        crate::share::interpreter::bytecodeInterpreter::initialize(
            loader,
            &enum_klass,
            &mut output,
            0,
        )?;
        let mut enumeration =
            crate::share::oops::instanceOop::InstanceOop::new(enum_klass.clone())?;
        let collection_index = enum_klass.instanceField(b"val$c", b"Ljava/util/Collection;")?;
        enumeration.fields[collection_index] = Value::Object(list_handle);
        let iterator_index = enum_klass.instanceField(b"i", b"Ljava/util/Iterator;")?;
        let list_klass_for_iterator = loader.load(b"java/util/ArrayList")?;
        let iterator_klass = loader.load(b"java/util/ArrayList$Itr")?;
        let mut output = std::io::sink();
        crate::share::interpreter::bytecodeInterpreter::initialize(
            loader,
            &iterator_klass,
            &mut output,
            0,
        )?;
        let mut iterator =
            crate::share::oops::instanceOop::InstanceOop::new(iterator_klass.clone())?;
        let cursor_index = iterator_klass.instanceField(b"cursor", b"I")?;
        let last_ret_index = iterator_klass.instanceField(b"lastRet", b"I")?;
        let expected_mod_count_index = iterator_klass.instanceField(b"expectedModCount", b"I")?;
        let outer_class_index =
            iterator_klass.instanceField(b"this$0", b"Ljava/util/ArrayList;")?;
        iterator.fields[cursor_index] = Value::Int(0);
        iterator.fields[last_ret_index] = Value::Int(-1);
        iterator.fields[expected_mod_count_index] = Value::Int(0);
        iterator.fields[outer_class_index] = Value::Object(list_handle);
        let iterator_handle = loader.alloc_object(iterator);
        let handle = loader.alloc_object(enumeration);
        {
            let mut objects = loader.borrow_objects_mut();
            let enumeration = objects
                .get_mut(handle)
                .ok_or_else(|| io::Error::other("Invalid Enumeration object"))?;
            enumeration.fields[iterator_index] = Value::Object(iterator_handle);
        }
        drop(list_klass_for_iterator);
        return Ok(Some(Some(Value::Object(handle))));
    }

    // Resolve inherited natives to their declaring class before dispatch.
    let resolved_owner;
    let owner = match native_declaring_owner(loader, owner, name, descriptor) {
        Some(declaring) => {
            resolved_owner = declaring;
            resolved_owner.as_slice()
        }
        None => owner,
    };

    let result = match (owner, name, descriptor) {
        (b"java/lang/Runtime", b"availableProcessors", b"()I") => {
            let cpus = std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1);
            Some(Value::Int(cpus.min(i32::MAX as usize) as i32))
        }
        (b"java/lang/Runtime", b"freeMemory", b"()J") => {
            Some(Value::Long(Jvm::free_memory(0) as i64))
        }
        (b"java/lang/Runtime", b"totalMemory", b"()J") => {
            Some(Value::Long(Jvm::total_memory() as i64))
        }
        (b"java/lang/Runtime", b"maxMemory", b"()J") => Some(Value::Long(Jvm::max_memory() as i64)),
        (b"java/lang/Runtime", b"gc", b"()V") => None,
        (b"java/lang/Shutdown", b"beforeHalt", b"()V") => None,
        (b"java/lang/Shutdown", b"halt0", b"(I)V") => {
            let status = match arguments.first().copied() {
                Some(Value::Int(status)) => status,
                _ => 0,
            };
            let _ = std::io::Write::flush(output);
            eprintln!("[halt0i] exit status={status}");
            std::process::exit(status);
        }
        (b"java/lang/ClassLoader", b"findBootstrapClass", b"(Ljava/lang/String;)Ljava/lang/Class;") => {
            let name_value = arguments.get(1).copied().unwrap_or(Value::Null);
            let text = super::stringNatives::string(loader, name_value)?
                .map(|cow| cow.into_owned());
            match text {
                Some(text) if !text.starts_with('[') => {
                    let internal = text.replace('.', "/");
                    match loader.load(internal.as_bytes()) {
                        Ok(_) => Some(Value::ClassMirror(
                            loader.storage.alloc_str(&internal).as_bytes(),
                        )),
                        Err(_) => Some(Value::Null),
                    }
                }
                _ => Some(Value::Null),
            }
        }
        (b"java/lang/Object", b"hashCode", b"()I") => {
            let receiver = *arguments
                .first()
                .ok_or_else(|| io::Error::other("Missing receiver"))?;
            let Some(hash) = super::objectNatives::hashCode(loader, receiver)? else {
                return Ok(None);
            };
            Some(Value::Int(hash))
        }
        (b"java/lang/Object", b"clone", b"()Ljava/lang/Object;") => {
            let receiver = *arguments
                .first()
                .ok_or_else(|| io::Error::other("Missing receiver"))?;
            Some(match receiver {
                Value::Object(handle) => {
                    let (klass, fields) = {
                        let objects = loader.borrow_objects();
                        let object = objects
                            .get(handle)
                            .ok_or_else(|| io::Error::other("Invalid object reference"))?;
                        (Arc::clone(&object.klass), object.fields.clone())
                    };
                    let mut cloned =
                        crate::share::oops::instanceOop::InstanceOop::new(klass)?;
                    cloned.fields = fields;
                    Value::Object(loader.alloc_object(cloned))
                }
                Value::PrimitiveArray(handle) => {
                    let array = {
                        let arrays = loader.arrays.borrow();
                        arrays
                            .get(handle)
                            .ok_or_else(|| io::Error::other("Invalid array reference"))?
                            .clone()
                    };
                    Value::PrimitiveArray(loader.alloc_type_array(array))
                }
                Value::ObjectArray(handle) => {
                    let (component, elements) = {
                        let arrays = loader.obj_arrays.borrow();
                        let array = arrays
                            .get(handle)
                            .ok_or_else(|| io::Error::other("Invalid array reference"))?;
                        (array.component, array.elements.clone())
                    };
                    let mut array =
                        crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                            elements.len() as i32,
                            component,
                        )?;
                    array.elements = elements;
                    Value::ObjectArray(loader.alloc_obj_array(array))
                }
                Value::String(_) | Value::HostString(_) | Value::ClassMirror(_) => receiver,
                Value::Null => {
                    return Err(io::Error::other("NullPointerException"));
                }
                _ => {
                    return Err(io::Error::other(
                        "java/lang/CloneNotSupportedException",
                    ));
                }
            })
        }
        (b"java/lang/Throwable", b"fillInStackTrace", b"(I)Ljava/lang/Throwable;") => {
            fill_in_stack_trace(
                loader,
                arguments.first().copied().unwrap_or(Value::Null),
            )?;
            Some(arguments.first().copied().unwrap_or(Value::Null))
        }
        // Perf counters are optional in HotSpot (`-XX:-UsePerfData` yields
        // null); JDK callers null-check the result.
        (b"jdk/internal/perf/Perf", b"createLong", b"(Ljava/lang/String;IIJ)Ljava/nio/ByteBuffer;")
        | (b"jdk/internal/perf/Perf", b"createByteArray", b"(Ljava/lang/String;II[BI)Ljava/nio/ByteBuffer;")
        | (b"jdk/internal/perf/Perf", b"attach0", b"(I)Ljava/nio/ByteBuffer;") => {
            Some(Value::Null)
        }
        (b"jdk/internal/perf/Perf", b"highResCounter", b"()J") => {
            Some(Value::Long(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as i64)
                    .unwrap_or(0),
            ))
        }
        (b"jdk/internal/perf/Perf", b"highResFrequency", b"()J") => {
            Some(Value::Long(1_000_000_000))
        }
        (b"jdk/internal/perf/Perf", b"detach", b"(Ljava/nio/ByteBuffer;)V") => None,
        (b"java/lang/Object", b"wait0", b"(J)V") => {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            let Some(key) =
                crate::share::interpreter::bytecodeInterpreter::monitorKey(receiver)
            else {
                return Err(io::Error::other("Expected reference for wait"));
            };
            let timeout = match arguments.get(1) {
                Some(Value::Long(millis)) => *millis,
                _ => 0,
            };
            crate::share::runtime::threading::wait_object(
                loader,
                key as usize,
                timeout,
                output,
                depth,
            )?;
            None
        }
        (b"java/lang/Object", b"notify", b"()V")
        | (b"java/lang/Object", b"notifyAll", b"()V") => {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            let Some(key) =
                crate::share::interpreter::bytecodeInterpreter::monitorKey(receiver)
            else {
                return Err(io::Error::other("Expected reference for notify"));
            };
            crate::share::runtime::threading::notify_object(
                loader,
                key as usize,
                name == b"notifyAll",
            )?;
            None
        }
        (b"java/lang/NullPointerException", b"getExtendedNPEMessage", b"()Ljava/lang/String;") => {
            Some(Value::Null)
        }
        (b"java/lang/Thread", b"setPriority0", b"(I)V")
        | (b"java/lang/Thread", b"setNativeName", b"(Ljava/lang/String;)V")
        | (b"java/lang/Thread", b"setCurrentThread", b"(Ljava/lang/Thread;)V") => None,
        (b"java/lang/Thread", b"start0", b"()V") => {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            if std::env::var_os("JVM25_TRACE").is_some() {
                eprintln!("[start0] receiver={receiver:?}");
            }
            if let Value::Object(handle) = receiver {
                let tid = crate::share::runtime::threading::register(loader, handle)?;
                set_instance_field(
                    loader,
                    handle,
                    b"eetop",
                    b"J",
                    Value::Long(i64::from(tid.min(i64::MAX as u64) as i64)),
                )?;
                let holder = {
                    let objects = loader.borrow_objects();
                    objects.get(handle).and_then(|object| {
                        let slot = object
                            .klass
                            .instanceField(b"holder", b"Ljava/lang/Thread$FieldHolder;")
                            .ok()?;
                        object.fields.get(slot).copied()
                    })
                };
                if let Some(Value::Object(holder_handle)) = holder {
                    set_instance_field(
                        loader,
                        holder_handle,
                        b"threadStatus",
                        b"I",
                        Value::Int(1),
                    )?;
                }
            }
            None
        }
        (b"java/lang/Thread", b"interrupt0", b"()V") => {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            if let Value::Object(handle) = receiver {
                let _ = set_instance_field(
                    loader,
                    handle,
                    b"interrupted",
                    b"Z",
                    Value::Int(1),
                );
            }
            None
        }
        (b"java/lang/Thread", b"sleep0", b"(J)V")
        | (b"java/lang/Thread", b"sleepNanos0", b"(J)V") => {
            let nanos = match arguments.first() {
                Some(Value::Long(nanos)) => *nanos,
                _ => 0,
            };
            crate::share::runtime::threading::sleep_nanos(loader, nanos, output, depth)?;
            None
        }
        (b"java/lang/Thread", b"yield0", b"()V") => {
            crate::share::runtime::threading::yield_thread(loader, output, depth)?;
            None
        }
        (b"java/lang/Thread", b"onSpinWait0", b"(J)V") => {
            let nanos = match arguments.first() {
                Some(Value::Long(nanos)) => *nanos,
                _ => 0,
            };
            let spins = (nanos.max(0) as u64 / 64).clamp(1, 8192);
            for _ in 0..spins {
                std::hint::spin_loop();
            }
            None
        }
        (b"java/lang/Thread", b"getStackTrace0", b"()[Ljava/lang/StackTraceElement;") => {
            let component = loader
                .storage
                .alloc_slice_copy(b"java/lang/StackTraceElement");
            let array =
                crate::share::oops::objArrayOop::ObjArrayOop::withComponent(0, component)?;
            Some(Value::ObjectArray(loader.alloc_obj_array(array)))
        }
        _ if owner.starts_with(b"java/lang/ref/")
            && name == b"refersTo0"
            && descriptor == b"(Ljava/lang/Object;)Z" => {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            let target = arguments.get(1).copied().unwrap_or(Value::Null);
            let referent = match receiver {
                Value::Object(handle) => {
                    let objects = loader.borrow_objects();
                    objects.get(handle).and_then(|object| {
                        let slot = object
                            .klass
                            .instanceField(b"referent", b"Ljava/lang/Object;")
                            .ok()?;
                        object.fields.get(slot).copied()
                    })
                }
                _ => None,
            };
            Some(Value::Int(i32::from(referent == Some(target))))
        }
        _ if owner.starts_with(b"java/lang/ref/")
            && name == b"clear0"
            && descriptor == b"()V" =>
        {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            if let Value::Object(handle) = receiver {
                let _ = set_instance_field(
                    loader,
                    handle,
                    b"referent",
                    b"Ljava/lang/Object;",
                    Value::Null,
                );
            }
            None
        }
        // FinalReference inactive accessors: same `referent` slot as
        // Reference.get/clear, used by FinalizerThread after the GC has
        // already decided the referent is unreachable.
        _ if owner.starts_with(b"java/lang/ref/")
            && name == b"getFromInactiveFinalReference"
            && descriptor == b"()Ljava/lang/Object;" =>
        {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            let value = match receiver {
                Value::Object(handle) => {
                    let objects = loader.borrow_objects();
                    objects.get(handle).and_then(|object| {
                        let slot = object
                            .klass
                            .instanceField(b"referent", b"Ljava/lang/Object;")
                            .ok()?;
                        object.fields.get(slot).copied()
                    })
                }
                _ => None,
            };
            Some(value.unwrap_or(Value::Null))
        }
        _ if owner.starts_with(b"java/lang/ref/")
            && name == b"clearInactiveFinalReference"
            && descriptor == b"()V" =>
        {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            if let Value::Object(handle) = receiver {
                let _ = set_instance_field(
                    loader,
                    handle,
                    b"referent",
                    b"Ljava/lang/Object;",
                    Value::Null,
                );
            }
            None
        }
        (b"java/lang/Runtime", b"load0", b"(Ljava/lang/String;)V") => {
            // System.load(String): arguments = [receiver, filename]
            let Some(filename) = arguments
                .get(1)
                .and_then(|value| super::stringNatives::string(loader, *value).ok().flatten())
            else {
                return Err(io::Error::other("Invalid library path"));
            };
            super::nativeLibraries::NativeLibraries::global()
                .load(std::path::Path::new(filename.as_ref()))?;
            None
        }
        (b"java/lang/Runtime", b"loadLibrary0", b"(Ljava/lang/Class;Ljava/lang/String;)V") => {
            // System.loadLibrary(String): arguments = [receiver, fromClass, name]
            let Some(name) = arguments
                .get(2)
                .and_then(|value| super::stringNatives::string(loader, *value).ok().flatten())
            else {
                return Err(io::Error::other("Invalid library name"));
            };
            let java_home = loader
                .systemProperties
                .borrow()
                .get("java.home")
                .cloned();
            let file_name = if cfg!(windows) {
                format!("{name}.dll")
            } else if cfg!(target_os = "macos") {
                format!("lib{name}.dylib")
            } else {
                format!("lib{name}.so")
            };
            let builtin = java_home.as_deref().is_some_and(|home| {
                ["lib", "bin"].iter().any(|sub| {
                    std::path::Path::new(home)
                        .join(sub)
                        .join(&file_name)
                        .is_file()
                })
            });
            if std::env::var_os("JVM25_TRACE_NATIVE").is_some() {
                eprintln!("[loadLibrary0] name={name} file={file_name} home={java_home:?} builtin={builtin}");
            }
            if !builtin {
                // JDK's default java.library.path starts with the current
                // directory; honour an explicit property first.
                let mut search_path = String::from(".");
                {
                    let props = loader.systemProperties.borrow();
                    if let Some(value) = props.get("java.library.path") {
                        search_path.push_str(*value);
                    }
                    if let Some(home) = props.get("java.home") {
                        for sub in ["bin", "lib"] {
                            if !search_path.is_empty() {
                                search_path.push(';');
                            }
                            search_path.push_str(
                                &std::path::Path::new(*home)
                                    .join(sub)
                                    .to_string_lossy(),
                            );
                        }
                    }
                }
                super::nativeLibraries::NativeLibraries::global().loadLibraryWithPaths(
                    name.as_ref(),
                    Some(&search_path),
                )?;
            }
            None
        }
        _ if ((owner == b"java/io/WinNTFileSystem"
                || owner == b"java/io/UnixFileSystem"
                || owner == b"java/io/FileDescriptor"
                || owner == b"java/io/FileInputStream"
                || owner == b"java/io/FileOutputStream"
                || owner == b"java/io/RandomAccessFile")) => {
            match super::fileNatives::invoke(loader, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => None,
                Some(value) => Some(value),
                None => {
                    return match super::jniInvocation::invoke_dll_native(
                        loader, owner, name, descriptor, false, arguments,
                    )? {
                        Some(result) => Ok(Some(result)),
                        None => Ok(None),
                    };
                }
            }
        }
        _ if owner == b"java/lang/ProcessHandleImpl$Info" => {
            match super::processNatives::statik(loader, owner, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => None,
                Some(value) => Some(value),
                None => {
                    return match super::jniInvocation::invoke_dll_native(
                        loader, owner, name, descriptor, false, arguments,
                    )? {
                        Some(result) => Ok(Some(result)),
                        None => Ok(None),
                    };
                }
            }
        }
        _ if owner == b"java/util/zip/Inflater" || owner == b"java/util/zip/Deflater" => {
            match super::zipNatives::instance(loader, owner, name, descriptor, arguments)? {
                Some(value) => Some(value),
                None => {
                    return match super::jniInvocation::invoke_dll_native(
                        loader, owner, name, descriptor, false, arguments,
                    )? {
                        Some(result) => Ok(Some(result)),
                        None => Ok(None),
                    };
                }
            }
        }
        _ => {
            if std::env::var_os("JVM25_TRACE").is_some() {
                let args_dbg = if name == b"<init>" {
                    let parts: Vec<String> = arguments
                        .iter()
                        .map(|value| match value {
                            Value::String(bytes) => {
                                format!("\"{}\"", String::from_utf8_lossy(bytes))
                            }
                            Value::HostString(text) => format!("\"{text}\""),
                            Value::Null => "null".to_owned(),
                            other => format!("{other:?}"),
                        })
                        .collect();
                    format!(" args=[{}]", parts.join(", "))
                } else {
                    String::new()
                };
                eprintln!(
                    "[nativeInstance] {}.{}{}{}",
                    String::from_utf8_lossy(owner),
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor),
                    args_dbg
                );
            }
            return match super::jniInvocation::invoke_dll_native(
                loader, owner, name, descriptor, false, arguments,
            )? {
                Some(result) => Ok(Some(result)),
                None => Ok(None),
            };
        }
    };

    Ok(Some(result))
}

/// Allocates a raw instance of `class_name` without running any constructor.
fn raw_instance<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    class_name: &[u8],
) -> io::Result<usize> {
    use crate::share::oops::instanceOop::InstanceOop;

    let klass = loader.load(class_name)?;
    let object = InstanceOop::new(klass)?;
    Ok(loader.alloc_object(object))
}

/// Writes an instance field by name/descriptor on an already-allocated object.
pub(crate) fn set_instance_field<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    handle: usize,
    name: &[u8],
    descriptor: &[u8],
    value: crate::share::interpreter::bytecodeInterpreter::Value<'a>,
) -> io::Result<()> {
    use crate::share::interpreter::bytecodeInterpreter::Value;

    let slot = {
        let objects = loader.borrow_objects();
        let object = objects
            .get(handle)
            .ok_or_else(|| io::Error::other("Invalid object reference"))?;
        object.klass.instanceField(name, descriptor)?
    };
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(handle)
        .ok_or_else(|| io::Error::other("Invalid object reference"))?;
    if slot < object.fields.len() {
        object.fields[slot] = value;
    }
    let _ = Value::Null;
    Ok(())
}

/// Implements `Throwable.fillInStackTrace(int)`: snapshots the current Java
/// call stack (innermost first, skipping `java.lang.Throwable` frames) into a
/// `StackTraceElement[]` stored in the throwable's `stackTrace` field.
fn fill_in_stack_trace<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    receiver: crate::share::interpreter::bytecodeInterpreter::Value<'a>,
) -> io::Result<()> {
    use crate::share::interpreter::bytecodeInterpreter::Value;
    use crate::share::oops::objArrayOop::ObjArrayOop;

    let Value::Object(handle) = receiver else {
        return Err(io::Error::other("Invalid Throwable receiver"));
    };
    let frames = super::stackwalk::StackWalk::currentFrames(loader);
    if std::env::var_os("JVM25_TRACE_NATIVE").is_some() {
        eprintln!("[fillInStackTrace] frames={}", frames.len());
    }
    let component = loader
        .storage
        .alloc_slice_copy(b"java/lang/StackTraceElement");
    let mut array = ObjArrayOop::withComponent(frames.len() as i32, component)?;
    let mut count = 0usize;
    for frame in frames.iter() {
        if std::env::var_os("JVM25_TRACE_NATIVE").is_some() {
            eprintln!(
                "[fillInStackTrace] frame {}.{}{}",
                frame.class_name.as_str(),
                frame.method_name.as_str(),
                frame.signature.as_str()
            );
        }
        // HotSpot drops the `fillInStackTrace` machinery and every
        // `Throwable` subclass constructor frame: the trace starts at the
        // code that created the throwable.
        let class_name = frame.class_name.as_str();
        if class_name == "java/lang/Throwable" {
            continue;
        }
        let loaded = if !class_name.starts_with('[') && !class_name.starts_with(':') {
            loader.load(class_name.as_bytes()).ok()
        } else {
            None
        };
        if frame.method_name.as_str() == "<init>"
            && let Some(klass) = loaded.as_ref()
            && crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(
                klass,
                b"java/lang/Throwable",
            )
        {
            continue;
        }
        let dotted = class_name.replace('/', ".");
        let method_name = frame.method_name.as_str().to_owned();
        let file_name = loaded
            .as_ref()
            .and_then(|klass| klass.class.source_file())
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let element = match raw_instance(loader, b"java/lang/StackTraceElement") {
            Ok(handle) => handle,
            Err(error) => {
                if std::env::var_os("JVM25_TRACE_NATIVE").is_some() {
                    eprintln!("[fillInStackTrace] raw_instance failed: {error}");
                }
                break;
            }
        };
        set_instance_field(
            loader,
            element,
            b"declaringClass",
            b"Ljava/lang/String;",
            Value::HostString(loader.storage.alloc_str(&dotted)),
        )?;
        set_instance_field(
            loader,
            element,
            b"methodName",
            b"Ljava/lang/String;",
            Value::HostString(loader.storage.alloc_str(&method_name)),
        )?;
        set_instance_field(
            loader,
            element,
            b"fileName",
            b"Ljava/lang/String;",
            match file_name.as_deref() {
                Some(name) => Value::HostString(loader.storage.alloc_str(name)),
                None => Value::Null,
            },
        )?;
        set_instance_field(
            loader,
            element,
            b"lineNumber",
            b"I",
            Value::Int(frame.line_number.max(-1)),
        )?;
        // `computeFormat` resolves the declaring class lazily through this
        // field; seed it with a class mirror so `getClassLoader0`/`getModule`
        // work without a materialized `java.lang.Class` instance.
        let internal = loader.storage.alloc_str(frame.class_name.as_str());
        set_instance_field(
            loader,
            element,
            b"declaringClassObject",
            b"Ljava/lang/Class;",
            Value::ClassMirror(internal.as_bytes()),
        )?;
        array.elements[count] = Value::Object(element);
        count += 1;
    }
    array.elements.truncate(count);
    // HotSpot protocol: the native stores the raw backtrace object plus a
    // depth; the Java wrapper marks `stackTrace` as UNASSIGNED_STACK and
    // `getOurStackTrace` materializes elements lazily through
    // `StackTraceElement.of(backtrace, depth)` -> `initStackTraceElements`.
    // We store a ready `StackTraceElement[]` as the backtrace payload.
    let array_handle = loader.alloc_obj_array(array);
    set_instance_field(
        loader,
        handle,
        b"backtrace",
        b"Ljava/lang/Object;",
        Value::ObjectArray(array_handle),
    )?;
    set_instance_field(loader, handle, b"depth", b"I", Value::Int(count as i32))
}

/// Implements `StackTraceElement.initStackTraceElements(elements, backtrace,
/// depth)`: copies the pre-materialized `StackTraceElement` objects stored as
/// the throwable's `backtrace` payload into the freshly allocated target
/// array produced by `StackTraceElement.of`.
fn init_stack_trace_elements<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    arguments: &[crate::share::interpreter::bytecodeInterpreter::Value<'a>],
) -> io::Result<()> {
    use crate::share::interpreter::bytecodeInterpreter::Value;

    let (Some(&Value::ObjectArray(target)), Some(&Value::ObjectArray(source)), Some(&Value::Int(depth))) = (
        arguments.first(),
        arguments.get(1),
        arguments.get(2),
    ) else {
        return Ok(());
    };
    let count = depth.max(0) as usize;
    let source_elements = {
        let sources = loader.obj_arrays.borrow();
        let Some(source_array) = sources.get(source) else {
            return Ok(());
        };
        source_array.elements.clone()
    };
    let mut targets = loader.obj_arrays.borrow_mut();
    let Some(target_array) = targets.get_mut(target) else {
        return Ok(());
    };
    for index in 0..count.min(source_elements.len()).min(target_array.elements.len()) {
        target_array.elements[index] = source_elements[index];
    }
    Ok(())
}

/// Creates a minimal `AccessControlContext` with an empty protection-domain
/// array, used for the no-security-manager bootstrap path.
fn access_control_context<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
) -> io::Result<crate::share::interpreter::bytecodeInterpreter::Value<'a>> {
    use crate::share::interpreter::bytecodeInterpreter::Value;
    use crate::share::oops::objArrayOop::ObjArrayOop;

    let component = loader
        .storage
        .alloc_slice_copy(b"java/security/ProtectionDomain");
    let domains = ObjArrayOop::withComponent(0, component)?;
    let context_array = loader.alloc_obj_array(domains);
    let handle = raw_instance(loader, b"java/security/AccessControlContext")?;
    set_instance_field(
        loader,
        handle,
        b"context",
        b"[Ljava/security/ProtectionDomain;",
        Value::ObjectArray(context_array),
    )?;
    Ok(Value::Object(handle))
}

/// Returns the current thread object. The lazily-created synthetic instance is
/// initialized directly (FieldHolder + ThreadGroups + fields) because running
/// `Thread.<init>` would recursively call `Thread.currentThread()`.
fn current_thread<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
) -> io::Result<crate::share::interpreter::bytecodeInterpreter::Value<'a>> {
    use crate::share::interpreter::bytecodeInterpreter::Value;

    if let Some(handle) = crate::share::runtime::threading::current_thread_handle(loader) {
        return Ok(Value::Object(handle));
    }
    let handle = loader.currentThreadObject()?;
    let uninitialized = {
        let objects = loader.borrow_objects();
        match objects.get(handle) {
            Some(object) => match object
                .klass
                .instanceField(b"holder", b"Ljava/lang/Thread$FieldHolder;")
            {
                Ok(slot) => object
                    .fields
                    .get(slot)
                    .map_or(true, |value| matches!(value, Value::Null)),
                Err(_) => false,
            },
            None => false,
        }
    };
    if uninitialized {
        // Root "system" thread group (parent = null).
        let system_group = raw_instance(loader, b"java/lang/ThreadGroup")?;
        set_instance_field(
            loader,
            system_group,
            b"name",
            b"Ljava/lang/String;",
            Value::HostString("system"),
        )?;
        set_instance_field(loader, system_group, b"parent", b"Ljava/lang/ThreadGroup;", Value::Null)?;
        set_instance_field(loader, system_group, b"maxPriority", b"I", Value::Int(10))?;
        set_instance_field(loader, system_group, b"daemon", b"Z", Value::Int(0))?;
        set_instance_field(loader, system_group, b"ngroups", b"I", Value::Int(0))?;
        set_instance_field(loader, system_group, b"groups", b"[Ljava/lang/ThreadGroup;", Value::Null)?;
        set_instance_field(loader, system_group, b"nweaks", b"I", Value::Int(0))?;
        set_instance_field(
            loader,
            system_group,
            b"weaks",
            b"[Ljava/lang/ref/WeakReference;",
            Value::Null,
        )?;

        // "main" thread group, child of "system".
        let main_group = raw_instance(loader, b"java/lang/ThreadGroup")?;
        set_instance_field(
            loader,
            main_group,
            b"name",
            b"Ljava/lang/String;",
            Value::HostString("main"),
        )?;
        set_instance_field(
            loader,
            main_group,
            b"parent",
            b"Ljava/lang/ThreadGroup;",
            Value::Object(system_group),
        )?;
        set_instance_field(loader, main_group, b"maxPriority", b"I", Value::Int(10))?;
        set_instance_field(loader, main_group, b"daemon", b"Z", Value::Int(0))?;
        set_instance_field(loader, main_group, b"ngroups", b"I", Value::Int(0))?;
        set_instance_field(loader, main_group, b"groups", b"[Ljava/lang/ThreadGroup;", Value::Null)?;
        set_instance_field(loader, main_group, b"nweaks", b"I", Value::Int(0))?;
        set_instance_field(
            loader,
            main_group,
            b"weaks",
            b"[Ljava/lang/ref/WeakReference;",
            Value::Null,
        )?;

        // Thread$FieldHolder for the main thread: group=main, priority=NORM(5),
        // daemon=false, threadStatus=RUNNABLE(4), task=null, stackSize=0.
        let holder = raw_instance(loader, b"java/lang/Thread$FieldHolder")?;
        set_instance_field(
            loader,
            holder,
            b"group",
            b"Ljava/lang/ThreadGroup;",
            Value::Object(main_group),
        )?;
        set_instance_field(loader, holder, b"task", b"Ljava/lang/Runnable;", Value::Null)?;
        set_instance_field(loader, holder, b"stackSize", b"J", Value::Long(0))?;
        set_instance_field(loader, holder, b"priority", b"I", Value::Int(5))?;
        set_instance_field(loader, holder, b"daemon", b"Z", Value::Int(0))?;
        set_instance_field(loader, holder, b"threadStatus", b"I", Value::Int(4))?;

        // interruptLock object.
        let interrupt_lock = raw_instance(loader, b"java/lang/Object")?;

        // Thread instance fields.
        set_instance_field(
            loader,
            handle,
            b"holder",
            b"Ljava/lang/Thread$FieldHolder;",
            Value::Object(holder),
        )?;
        set_instance_field(
            loader,
            handle,
            b"name",
            b"Ljava/lang/String;",
            Value::HostString("main"),
        )?;
        set_instance_field(loader, handle, b"tid", b"J", Value::Long(1))?;
        set_instance_field(
            loader,
            handle,
            b"interruptLock",
            b"Ljava/lang/Object;",
            Value::Object(interrupt_lock),
        )?;
        set_instance_field(
            loader,
            handle,
            b"inheritedAccessControlContext",
            b"Ljava/security/AccessControlContext;",
            Value::Null,
        )?;
        if let Ok(loader_object) = loader.systemClassLoaderObject() {
            set_instance_field(
                loader,
                handle,
                b"contextClassLoader",
                b"Ljava/lang/ClassLoader;",
                Value::Object(loader_object),
            )?;
        }
    }
    Ok(Value::Object(handle))
}

pub fn invokeStatic<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[crate::share::interpreter::bytecodeInterpreter::Value<'a>],
    output: &mut dyn std::io::Write,
    depth: usize,
) -> io::Result<Option<Option<crate::share::interpreter::bytecodeInterpreter::Value<'a>>>> {
    let _rg = crate::share::recprobe::Guard::enter("invokeStatic");
    let _ng = NativeGuard::enter(owner, name);
    NATIVE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    maybe_dump_stack(loader);
    use super::jvm::Jvm;
    use super::jvmNatives::JvmNatives;
    use crate::share::interpreter::bytecodeInterpreter::Value;

    if trace_native_enabled() {
        eprintln!(
            "[invokeStatic] {}.{}{}",
            String::from_utf8_lossy(owner),
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
        );
    }

    if (owner == b"java/lang/Class"
        || owner.starts_with(b"java/lang/reflect/")
        || owner.starts_with(b"jdk/internal/reflect/"))
        && let Some(result) =
            super::reflectionNatives::invoke_static(loader, owner, name, descriptor, arguments)?
    {
        return Ok(Some(result));
    }

    if let Some(result) =
        super::methodHandleNatives::invoke_static(loader, owner, name, descriptor, arguments)?
    {
        return Ok(Some(result));
    }

    if super::vmManagementNatives::is_owner(owner) {
        if let Some(result) =
            super::vmManagementNatives::invoke_static(loader, name, descriptor, arguments)?
        {
            return Ok(Some(result));
        }
    }

    if super::perfNatives::is_owner(owner) {
        if let Some(result) =
            super::perfNatives::invoke_static(loader, name, descriptor, arguments, output)?
        {
            return Ok(Some(result));
        }
    }

    if (owner.starts_with(b"java/net/")
        || owner.starts_with(b"sun/nio/ch/")
        || owner.starts_with(b"jdk/net/"))
        && let Some(result) =
            super::netNatives::invoke_static(loader, owner, name, descriptor, arguments)?
    {
        return Ok(Some(result));
    }

    if super::nioBitsNatives::is_owner(owner) {
        if let Some(result) = super::nioBitsNatives::invoke_static(
            loader,
            name,
            descriptor,
            arguments,
            output,
        )? {
            return Ok(Some(result));
        }
    }

    if super::selectorNatives::is_owner(owner) {
        if let Some(result) = super::selectorNatives::invoke_static(
            loader,
            name,
            descriptor,
            arguments,
            output,
        )? {
            return Ok(Some(result));
        }
    }

    if owner == b"jdk/internal/jimage/NativeImageBuffer" {
        if let Some(result) = super::jimageNatives::invoke_static(
            loader,
            name,
            descriptor,
            arguments,
            output,
        )? {
            return Ok(Some(result));
        }
    }

    // Resolve inherited natives to their declaring class before dispatch.
    let resolved_owner;
    let owner = match native_declaring_owner(loader, owner, name, descriptor) {
        Some(declaring) => {
            resolved_owner = declaring;
            resolved_owner.as_slice()
        }
        None => owner,
    };

    let result = match (owner, name, descriptor) {
        (b"java/lang/ProcessEnvironment", b"environmentBlock", b"()Ljava/lang/String;") if cfg!(windows) => {
            let mut variables: Vec<_> = std::env::vars_os()
                .map(|(key, value)| {
                    let mut entry = key.to_string_lossy().into_owned();
                    entry.push('=');
                    entry.push_str(&value.to_string_lossy());
                    entry
                })
                .collect();
            variables.sort_by_key(|entry| entry.to_uppercase());
            let mut block = variables.join("\0");
            block.push('\0');
            block.push('\0');
            Some(Some(Value::HostString(loader.storage.alloc_str(&block))))
        }
        (b"java/lang/StackStreamFactory", b"checkStackWalkModes", b"()Z") => {
            use super::stackwalk::{
                JVM_STACKWALK_FILL_CLASS_REFS_ONLY, JVM_STACKWALK_FILL_LIVE_STACK_FRAMES,
                JVM_STACKWALK_GET_CALLER_CLASS, JVM_STACKWALK_SHOW_HIDDEN_FRAMES,
            };
            let klass = loader.load(owner)?;
            let modes: [(&[u8], i32); 5] = [
                (b"DEFAULT_MODE", 0),
                (b"FILL_CLASS_REFS_ONLY", JVM_STACKWALK_FILL_CLASS_REFS_ONLY),
                (b"GET_CALLER_CLASS", JVM_STACKWALK_GET_CALLER_CLASS),
                (b"SHOW_HIDDEN_FRAMES", JVM_STACKWALK_SHOW_HIDDEN_FRAMES),
                (b"FILL_LIVE_STACK_FRAMES", JVM_STACKWALK_FILL_LIVE_STACK_FRAMES),
            ];
            let mut compatible = true;
            for (field, expected) in modes {
                compatible &= klass.staticField(field, b"I")?.get() == Value::Int(expected);
            }
            Some(Some(Value::Int(i32::from(compatible))))
        }
        (b"java/lang/StringUTF16", b"isBigEndian", b"()Z") => {
            Some(Some(Value::Int(i32::from(cfg!(target_endian = "big")))))
        }
        (_, b"registerNatives", b"()V") | (_, b"initIDs", b"()V") => Some(None),
        (
            b"java/lang/StackTraceElement",
            b"initStackTraceElements",
            b"([Ljava/lang/StackTraceElement;Ljava/lang/Object;I)V",
        ) => {
            init_stack_trace_elements(loader, arguments)?;
            Some(None)
        }
        (b"java/lang/Shutdown", b"beforeHalt", b"()V") => Some(None),
        (b"java/lang/Shutdown", b"halt0", b"(I)V") => {
            let status = match arguments.first().copied() {
                Some(Value::Int(status)) => status,
                _ => 0,
            };
            let _ = std::io::Write::flush(output);
            let mut frames = Vec::new();
            crate::share::interpreter::bytecodeInterpreter::for_each_active_frame(
                &mut |f| frames.push(f.site()),
            );
            eprintln!("[halt0] exit status={status} stack={frames:?}");
            std::process::exit(status)
        }
        (b"java/io/ObjectStreamClass", b"initNative", b"()V") => Some(None),
        (b"java/io/ObjectStreamClass", b"hasStaticInitializer", b"(Ljava/lang/Class;)Z") => {
            let has = match arguments.first().copied().unwrap_or(Value::Null) {
                Value::ClassMirror(name) => loader.load(name).map(|klass| {
                    klass.class.methods.iter().any(|method| {
                        method.name == b"<clinit>" && method.descriptor == b"()V"
                    })
                }),
                _ => Ok(false),
            };
            match has {
                Ok(flag) => Some(Some(Value::Int(i32::from(flag)))),
                Err(error) => return Err(error),
            }
        }
        (b"java/io/FileDescriptor", b"getHandle", b"(I)J")
        | (b"java/io/FileDescriptor", b"getLong", b"(I)J") => Some(Some(Value::Long(0))),
        (b"java/io/FileDescriptor", b"getInt", b"(I)I")
        | (b"java/io/FileDescriptor", b"getBoolean", b"(I)Z")
        | (b"java/io/FileDescriptor", b"getAppend", b"(I)Z") => Some(Some(Value::Int(0))),
        (b"java/io/FileDescriptor", b"setInt", b"(II)V")
        | (b"java/io/FileDescriptor", b"setLong", b"(IJ)V")
        | (b"java/io/FileDescriptor", b"setHandle", b"(IJ)V") => Some(None),
        (b"jdk/internal/misc/Signal", b"findSignal0", b"(Ljava/lang/String;)I") => {
            // Standard signal numbers (Linux/POSIX numbering, same table
            // HotSpot uses on Windows for the Signal API).
            let name = arguments
                .first()
                .and_then(|value| super::stringNatives::string(loader, *value).ok().flatten());
            let number = name.as_deref().and_then(|name| match name {
                "HUP" => Some(1),
                "INT" => Some(2),
                "QUIT" => Some(3),
                "ILL" => Some(4),
                "TRAP" => Some(5),
                "ABRT" | "IOT" => Some(6),
                "BUS" => Some(7),
                "FPE" => Some(8),
                "KILL" => Some(9),
                "USR1" => Some(10),
                "SEGV" => Some(11),
                "USR2" => Some(12),
                "PIPE" => Some(13),
                "ALRM" => Some(14),
                "TERM" => Some(15),
                "STKFLT" => Some(16),
                "CHLD" => Some(17),
                "CONT" => Some(18),
                "STOP" => Some(19),
                "TSTP" => Some(20),
                "TTIN" => Some(21),
                "TTOU" => Some(22),
                "URG" => Some(23),
                "XCPU" => Some(24),
                "XFSZ" => Some(25),
                "VTALRM" => Some(26),
                "PROF" => Some(27),
                "WINCH" => Some(28),
                "IO" => Some(29),
                "PWR" => Some(30),
                "SYS" => Some(31),
                _ => None,
            });
            Some(Some(Value::Int(number.unwrap_or(-1))))
        }
        (b"jdk/internal/misc/Signal", b"handle0", b"(IJ)J") => {
            Some(Some(Value::Long(0)))
        }
        (b"jdk/internal/misc/Signal", b"raise0", b"(I)V") => Some(None),
        (b"java/util/TimeZone", b"getSystemTimeZoneID", b"(Ljava/lang/String;)Ljava/lang/String;") => {
            let user_tz = arguments
                .first()
                .and_then(|value| super::stringNatives::string(loader, *value).ok().flatten())
                .map(|text| text.into_owned())
                .filter(|text| !text.is_empty());
            let tz = user_tz
                .or_else(|| std::env::var("TZ").ok().filter(|v| !v.is_empty()))
                .unwrap_or_else(|| "GMT".to_string());
            Some(Some(Value::HostString(loader.storage.alloc_str(&tz))))
        }
        (b"java/util/TimeZone", b"getSystemGMTOffsetID", b"()Ljava/lang/String;") => {
            Some(Some(Value::HostString(loader.storage.alloc_str("GMT"))))
        }
        (b"java/lang/Thread", b"getNextThreadIdOffset", b"()J") => Some(Some(Value::Long(
            crate::share::prims::unsafeNatives::THREAD_TID_SENTINEL_OFFSET,
        ))),
        (b"java/lang/Thread", b"findScopedValueBindings", b"()Ljava/lang/Object;")
        | (b"java/security/AccessController", b"ensureMaterializedForStackWalk", b"(Ljava/lang/Object;)V")
        | (b"java/lang/Thread", b"ensureMaterializedForStackWalk", b"(Ljava/lang/Object;)V") => {
            Some(if name == b"findScopedValueBindings" { Some(Value::Null) } else { None })
        }
        (b"java/lang/Thread", b"scopedValueCache", b"()[Ljava/lang/Object;") => {
            let component = loader.storage.alloc_slice_copy(b"java/lang/Object");
            let array =
                crate::share::oops::objArrayOop::ObjArrayOop::withComponent(0, component)?;
            Some(Some(Value::ObjectArray(loader.alloc_obj_array(array))))
        }
        (b"java/lang/Thread", b"setScopedValueCache", b"([Ljava/lang/Object;)V") => Some(None),
        (b"jdk/internal/misc/Unsafe", b"getUnsafe", b"()Ljdk/internal/misc/Unsafe;") => {
            match loader.load(b"jdk/internal/misc/Unsafe") {
                Ok(klass) => match crate::share::oops::instanceOop::InstanceOop::new(klass) {
                    Ok(object) => Some(Some(Value::Object(loader.alloc_object(object)))),
                    Err(_) => Some(Some(Value::Null)),
                },
                Err(_) => Some(Some(Value::Null)),
            }
        }
        (b"java/lang/System", b"setIn0", b"(Ljava/io/InputStream;)V")
        | (b"java/lang/System", b"setOut0", b"(Ljava/io/PrintStream;)V")
        | (b"java/lang/System", b"setErr0", b"(Ljava/io/PrintStream;)V") => {
            let field = match name {
                b"setIn0" => (b"in".as_slice(), b"Ljava/io/InputStream;".as_slice()),
                b"setOut0" => (b"out".as_slice(), b"Ljava/io/PrintStream;".as_slice()),
                _ => (b"err".as_slice(), b"Ljava/io/PrintStream;".as_slice()),
            };
            if let Ok(system) = loader.load(b"java/lang/System")
                && let Ok(cell) = system.staticField(field.0, field.1)
            {
                cell.set(arguments.first().copied().unwrap_or(Value::Null));
            }
            Some(None)
        }
        (b"java/lang/System", b"mapLibraryName", b"(Ljava/lang/String;)Ljava/lang/String;") => {
            let mapped = arguments
                .first()
                .and_then(|value| super::stringNatives::string(loader, *value).ok().flatten())
                .map(|name| {
                    let name = name.as_ref();
                    if cfg!(windows) {
                        if name.ends_with(".dll") {
                            name.to_string()
                        } else {
                            format!("{name}.dll")
                        }
                    } else if cfg!(target_os = "macos") {
                        if name.starts_with("lib") && name.ends_with(".dylib") {
                            name.to_string()
                        } else {
                            format!("lib{name}.dylib")
                        }
                    } else if name.starts_with("lib") && name.ends_with(".so") {
                        name.to_string()
                    } else {
                        format!("lib{name}.so")
                    }
                });
            if std::env::var_os("JVM25_TRACE_NATIVE").is_some() {
                eprintln!("[mapLibraryName] -> {mapped:?}");
            }
            match mapped {
                Some(text) => Some(Some(Value::HostString(loader.storage.alloc_str(&text)))),
                None => Some(Some(Value::Null)),
            }
        }
        (b"jdk/internal/loader/NativeLibraries", b"findBuiltinLib", b"(Ljava/lang/String;)Ljava/lang/String;") => {
            let found = arguments
                .first()
                .and_then(|value| super::stringNatives::string(loader, *value).ok().flatten())
                .and_then(|file| {
                    let home = loader
                        .systemProperties
                        .borrow()
                        .get("java.home")
                        .copied()?;
                    for sub in ["lib", "bin"] {
                        let path = std::path::Path::new(home).join(sub).join(file.as_ref());
                        if path.is_file() {
                            return Some(path.to_string_lossy().into_owned());
                        }
                    }
                    None
                });
            if std::env::var_os("JVM25_TRACE_NATIVE").is_some() {
                eprintln!("[findBuiltinLib] -> {found:?}");
            }
            match found {
                Some(path) => Some(Some(Value::HostString(loader.storage.alloc_str(&path)))),
                None => Some(Some(Value::Null)),
            }
        }
        (b"jdk/internal/loader/NativeLibraries", b"load", b"(Ljdk/internal/loader/NativeLibraries$NativeLibraryImpl;Ljava/lang/String;ZZ)Z") => {
            let impl_value = arguments.first().copied().unwrap_or(Value::Null);
            let path_value = arguments.get(1).copied().unwrap_or(Value::Null);
            let is_builtin = matches!(arguments.get(2), Some(Value::Int(i)) if *i != 0);
            let throw_on_fail = matches!(arguments.get(3), Some(Value::Int(i)) if *i != 0);
            let path = super::stringNatives::string(loader, path_value)?
                .map(|cow| cow.into_owned())
                .unwrap_or_default();
            if std::env::var_os("JVM25_TRACE_NATIVE").is_some() {
                eprintln!("[NativeLibraries.load] path={path} builtin={is_builtin} throw={throw_on_fail}");
            }
            let Value::Object(impl_handle) = impl_value else {
                return Err(io::Error::other("Invalid NativeLibraryImpl"));
            };
            let (handle, ok) = if is_builtin {
                (-1i64, true)
            } else {
                match super::nativeLibraries::NativeLibraries::global()
                    .load_handle(std::path::Path::new(&path))
                {
                    Ok(value) => (value as i64, true),
                    Err(error) => {
                        if throw_on_fail {
                            return Err(error);
                        }
                        (0, false)
                    }
                }
            };
            if ok {
                set_instance_field(
                    loader,
                    impl_handle,
                    b"handle",
                    b"J",
                    Value::Long(handle),
                )?;
                set_instance_field(
                    loader,
                    impl_handle,
                    b"jniVersion",
                    b"I",
                    Value::Int(0x0001_0008),
                )?;
            }
            Some(Some(Value::Int(i32::from(ok))))
        }
        (b"jdk/internal/loader/NativeLibraries", b"unload", b"(Ljava/lang/String;ZJ)V") => {
            let is_builtin = matches!(arguments.get(1), Some(Value::Int(i)) if *i != 0);
            let handle = match arguments.get(2) {
                Some(Value::Long(value)) => *value as usize,
                _ => 0,
            };
            if !is_builtin && handle != usize::MAX {
                let _ = super::nativeLibraries::NativeLibraries::global().unload_handle(handle);
            }
            Some(None)
        }
        (b"jdk/internal/loader/NativeLibrary", b"findEntry0", b"(JLjava/lang/String;)J") => {
            let handle = match arguments.first() {
                Some(Value::Long(value)) => *value as usize,
                _ => 0,
            };
            let symbol = arguments
                .get(1)
                .and_then(|value| super::stringNatives::string(loader, *value).ok().flatten());
            let address = if handle == usize::MAX {
                None
            } else {
                symbol.and_then(|name| {
                    super::nativeLibraries::NativeLibraries::global().find_entry(name.as_ref())
                })
            };
            Some(Some(Value::Long(address.unwrap_or(0) as i64)))
        }
        (b"jdk/internal/loader/BootLoader", b"setBootLoaderUnnamedModule0", b"(Ljava/lang/Module;)V") => {
            Some(None)
        }
        // java.lang.Module VM-graph updates. Java-side state (reads,
        // exportedPackages/openPackages, ReflectionData) is maintained by
        // java.lang.Module itself, so these are no-ops for the interpreter.
        (b"java/lang/Module", b"defineModule0", b"(Ljava/lang/Module;ZLjava/lang/String;Ljava/lang/String;[Ljava/lang/Object;)V") => {
            // JVM_DefineModule: record the module object so Class.getModule
            // can resolve it. Java-side state is maintained by java.lang.Module.
            let module = arguments.first().copied().unwrap_or(Value::Null);
            if let Value::Object(handle) = module {
                let name_value = {
                    let objects = loader.borrow_objects();
                    objects.get(handle).and_then(|object| {
                        let slot = object
                            .klass
                            .instanceField(b"name", b"Ljava/lang/String;")
                            .ok()?;
                        object.fields.get(slot).copied()
                    })
                };
                if let Some(name_value) = name_value
                    && let Ok(Some(name)) = super::stringNatives::string(loader, name_value)
                {
                    loader.register_module_object(&name, handle);
                }
            }
            Some(None)
        }
        (b"java/lang/Module", b"defineModule0", _)
        | (b"java/lang/Module", b"addReads0", _)
        | (b"java/lang/Module", b"addExports0", _)
        | (b"java/lang/Module", b"addExportsToAll0", _)
        | (b"java/lang/Module", b"addExportsToAllUnnamed0", _) => Some(None),
        // Lambda proxy class archive is a HotSpot hidden-class cache; this
        // interpreter generates lambda classes directly, so archive lookups
        // always miss and inserts are ignored.
        (b"java/lang/ClassLoader", b"retrieveDirectives", b"()Ljava/lang/AssertionStatusDirectives;") => {
            // No -ea/-da/-esa options are parsed by this launcher, so the
            // VM reports HotSpot's default: empty directives, assertions off.
            let klass = loader.load(b"java/lang/AssertionStatusDirectives")?;
            let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
                loader, &klass, &mut io::sink(), 0,
            );
            let object = crate::share::oops::instanceOop::InstanceOop::new(klass.clone())?;
            let handle = loader.alloc_object(object);
            let strings = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                0,
                b"java/lang/String",
            )?;
            let string_array = Value::ObjectArray(loader.alloc_obj_array(strings));
            let bools = crate::share::oops::typeArrayOop::TypeArrayOop::Boolean(Vec::new());
            let bool_array = Value::PrimitiveArray(loader.alloc_type_array(bools));
            let fields: [(&[u8], &[u8], Value<'_>); 5] = [
                (b"classes", b"[Ljava/lang/String;", string_array),
                (b"classEnabled", b"[Z", bool_array),
                (b"packages", b"[Ljava/lang/String;", string_array),
                (b"packageEnabled", b"[Z", bool_array),
                (b"deflt", b"Z", Value::Int(0)),
            ];
            for (field, descriptor, value) in fields {
                if let Ok(slot) = klass.instanceField(field, descriptor) {
                    let mut objects = loader.borrow_objects_mut();
                    if let Some(object) = objects.get_mut(handle)
                        && slot < object.fields.len()
                    {
                        object.fields[slot] = value;
                    }
                }
            }
            Some(Some(Value::Object(handle)))
        }
        (b"java/lang/invoke/LambdaProxyClassArchive", b"addToArchive", _) => Some(None),
        (b"java/lang/invoke/LambdaProxyClassArchive", b"findFromArchive", _) => {
            Some(Some(Value::Null))
        }
        (b"jdk/internal/loader/RawNativeLibraries", b"load0", _) => {
            Some(Some(Value::Int(0)))
        }
        (b"jdk/internal/loader/RawNativeLibraries", b"unload0", _) => Some(None),
        (b"java/lang/ref/Finalizer", b"isFinalizationEnabled", b"()Z") => {
            Some(Some(Value::Int(1)))
        }
        (b"java/lang/ref/Finalizer", b"reportComplete", b"(Ljava/lang/Object;)V") => {
            Some(None)
        }
        (b"java/lang/ref/Reference", b"getAndClearReferencePendingList", b"()Ljava/lang/ref/Reference;") => {
            let head = loader.reference_pending.borrow_mut().take();
            Some(Some(match head {
                Some(handle) => Value::Object(handle),
                None => Value::Null,
            }))
        }
        (b"java/lang/ref/Reference", b"hasReferencePendingList", b"()Z") => {
            Some(Some(Value::Int(
                i32::from(loader.reference_pending.borrow().is_some()),
            )))
        }
        (b"java/lang/ref/Reference", b"waitForReferencePendingList", b"()V") => {
            let pending_empty = { loader.reference_pending.borrow().is_none() };
            if pending_empty {
                crate::share::runtime::threading::park(loader, false, 0, output, depth)?;
            }
            Some(None)
        }
        (b"jdk/internal/misc/VM", b"initialize", b"()V") => {
            // HotSpot's VM_Initialize processes VM options and ends up calling
            // VM.saveProperties, whose Java body derives `directMemory` from
            // the `jdk.nio.MaxDirectMemorySize` property (defaulting to
            // Runtime.maxMemory()). Without this, Bits.reserveMemory sees the
            // 64MB clinit default and rejects the ~170MB lib/modules map.
            if let Ok(vm) = loader.load(b"jdk/internal/misc/VM")
                && let Ok(cell) = vm.staticField(b"directMemory", b"J")
            {
                cell.set(Value::Long(Jvm::max_memory() as i64));
            }
            Some(None)
        }
        (b"jdk/internal/misc/VM", b"saveAndRemoveProperties", b"()V")
        | (b"jdk/internal/misc/VM", b"initializeOSEnvironment", b"()V") => Some(None),
        (b"jdk/internal/misc/VM", b"saveProperties", b"(Ljava/util/Map;)V") => {
            if let Ok(vm) = loader.load(b"jdk/internal/misc/VM")
                && let Ok(cell) = vm.staticField(b"savedProps", b"Ljava/util/Map;")
            {
                cell.set(arguments.first().copied().unwrap_or(Value::Null));
            }
            Some(None)
        }
        (b"jdk/internal/misc/VM", b"initLevel", b"(I)V") => {
            if let Ok(vm) = loader.load(b"jdk/internal/misc/VM")
                && let Ok(cell) = vm.staticField(b"initLevel", b"I")
            {
                cell.set(arguments.first().copied().unwrap_or(Value::Int(0)));
            }
            Some(None)
        }
        (b"jdk/internal/vm/ContinuationSupport", b"isSupported0", b"()Z") => {
            // This VM has no continuation/virtual-thread support; report
            // the capability honestly so JDK code takes the platform-thread
            // fallback paths instead of crashing on an unlinked native.
            Some(Some(Value::Int(0)))
        }
        (b"java/util/concurrent/atomic/AtomicLong", b"VMSupportsCS8", b"()Z") => Some(Some(Value::Int(1))),
        (b"jdk/internal/misc/VM", b"isBooted", b"()Z") => Some(Some(Value::Int(1))),
        (b"jdk/internal/misc/VM", b"getNanoTimeAdjustment", b"(J)J") => Some(Some(Value::Long(0))),
        (b"jdk/internal/misc/VM", b"latestUserDefinedLoader", b"()Ljava/lang/ClassLoader;") => {
            Some(Some(Value::Null))
        }
        (b"java/lang/ClassLoader", b"registerAsParallelCapable", b"()Z") => {
            Some(Some(Value::Int(1)))
        }
        (b"java/security/AccessController", b"getStackAccessControlContext", b"()Ljava/security/AccessControlContext;")
        | (b"java/security/AccessController", b"getInheritedAccessControlContext", b"()Ljava/security/AccessControlContext;") => {
            match access_control_context(loader) {
                Ok(value) => Some(Some(value)),
                Err(error) => return Err(error),
            }
        }
        (b"java/security/AccessController", b"getProtectionDomain", b"(Ljava/lang/Class;)Ljava/security/ProtectionDomain;") => {
            match raw_instance(loader, b"java/security/ProtectionDomain") {
                Ok(handle) => Some(Some(Value::Object(handle))),
                Err(error) => return Err(error),
            }
        }
        (b"java/lang/ClassLoader", b"findBootstrapClass", b"(Ljava/lang/String;)Ljava/lang/Class;") => {
            let name_value = arguments.first().copied().unwrap_or(Value::Null);
            let text = super::stringNatives::string(loader, name_value)?
                .map(|cow| cow.into_owned());
            match text {
                Some(text) if !text.starts_with('[') => {
                    let internal = text.replace('.', "/");
                    match loader.load(internal.as_bytes()) {
                        Ok(_) => Some(Some(Value::ClassMirror(
                            loader.storage.alloc_str(&internal).as_bytes(),
                        ))),
                        Err(_) => Some(Some(Value::Null)),
                    }
                }
                _ => Some(Some(Value::Null)),
            }
        }
        (b"java/lang/ClassLoader", b"defineClass0", b"(Ljava/lang/ClassLoader;Ljava/lang/Class;Ljava/lang/String;[BIILjava/security/ProtectionDomain;ZILjava/lang/Object;)Ljava/lang/Class;")
        | (b"java/lang/ClassLoader", b"defineClass1", b"(Ljava/lang/ClassLoader;Ljava/lang/String;[BIILjava/security/ProtectionDomain;Ljava/lang/String;)Ljava/lang/Class;") => {
            let result = if name == b"defineClass0" {
                super::defineClassNatives::define_class0(loader, arguments)
            } else {
                super::defineClassNatives::define_class1(loader, arguments)
            };
            match result {
                Ok(value) => value.map(Some),
                Err(error) => return Err(error),
            }
        }
        (b"java/lang/ClassLoader", b"defineClass2", b"(Ljava/lang/ClassLoader;Ljava/lang/String;Ljava/nio/ByteBuffer;IILjava/security/ProtectionDomain;Ljava/lang/String;)Ljava/lang/Class;") => {
            match super::defineClassNatives::define_class2(loader, arguments) {
                Ok(value) => value.map(Some),
                Err(error) => return Err(error),
            }
        }
        (b"java/lang/System", b"currentTimeMillis", b"()J") => {
            Some(Some(Value::Long(Jvm::current_time_millis())))
        }
        (b"java/lang/System", b"nanoTime", b"()J") => Some(Some(Value::Long(Jvm::nano_time()))),
        (b"java/lang/Thread", b"currentThread", b"()Ljava/lang/Thread;")
        | (b"java/lang/Thread", b"currentCarrierThread", b"()Ljava/lang/Thread;") => {
            match current_thread(loader) {
                Ok(value) => Some(Some(value)),
                Err(error) => return Err(error),
            }
        }
        (b"java/lang/Thread", b"yield0", b"()V") => {
            crate::share::runtime::threading::yield_thread(loader, output, depth)?;
            Some(None)
        }
        (b"java/lang/Thread", b"onSpinWait0", b"(J)V") => {
            let nanos = match arguments.first() {
                Some(Value::Long(nanos)) => *nanos,
                _ => 0,
            };
            let spins = (nanos.max(0) as u64 / 64).clamp(1, 8192);
            for _ in 0..spins {
                std::hint::spin_loop();
            }
            Some(None)
        }
        (b"java/lang/Thread", b"sleep0", b"(J)V")
        | (b"java/lang/Thread", b"sleepNanos0", b"(J)V") => {
            // sleep0/sleepNanos0 take nanoseconds (Thread.sleep converts
            // millis via TimeUnit.MILLISECONDS.toNanos before calling it).
            let nanos = match arguments.first() {
                Some(Value::Long(nanos)) => *nanos,
                _ => 0,
            };
            crate::share::runtime::threading::sleep_nanos(loader, nanos, output, depth)?;
            Some(None)
        }
        (b"java/lang/Thread", b"clearInterruptEvent", b"()V") => Some(None),
        (b"java/lang/Thread", b"holdsLock", b"(Ljava/lang/Object;)Z") => {
            Some(Some(Value::Int(i32::from(matches!(
                arguments.first(),
                Some(Value::Null) | Some(Value::Empty)
            )))))
        }
        (b"java/lang/Thread", b"getThreads", b"()[Ljava/lang/Thread;") => {
            // All live Java threads: registered spawned threads plus the
            // primordial thread (keyed by its object handle).
            let mut handles: Vec<usize> = loader
                .java_threads
                .borrow()
                .iter()
                .filter(|(_, info)| info.state != crate::share::runtime::threading::ThreadState::Finished)
                .map(|(&handle, _)| handle)
                .collect();
            if let Ok(current) = current_thread(loader) {
                if let Value::Object(handle) = current
                    && !handles.contains(&handle)
                {
                    handles.push(handle);
                }
            }
            let mut array =
                crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                    handles.len() as i32,
                    b"java/lang/Thread",
                )
                .unwrap();
            for (slot, handle) in array.elements.iter_mut().zip(handles) {
                *slot = Value::Object(handle);
            }
            Some(Some(Value::ObjectArray(loader.alloc_obj_array(array))))
        }
        (
            b"java/lang/Thread",
            b"dumpThreads",
            b"([Ljava/lang/Thread;)[[Ljava/lang/StackTraceElement;",
        ) => {
            let component = loader
                .storage
                .alloc_slice_copy(b"[Ljava/lang/StackTraceElement;");
            let array =
                crate::share::oops::objArrayOop::ObjArrayOop::withComponent(0, component).unwrap();
            Some(Some(Value::ObjectArray(loader.alloc_obj_array(array))))
        }
        (b"java/lang/System", b"identityHashCode", b"(Ljava/lang/Object;)I") => arguments
            .first()
            .map(|&value| Some(Value::Int(super::objectNatives::identity(value)))),
        (b"java/lang/Float", b"floatToRawIntBits", b"(F)I") => {
            if let Some(Value::Float(f)) = arguments.first() {
                Some(Some(Value::Int(JvmNatives::float_to_raw_int_bits(*f))))
            } else {
                None
            }
        }
        (b"java/lang/Float", b"intBitsToFloat", b"(I)F") => {
            if let Some(Value::Int(i)) = arguments.first() {
                Some(Some(Value::Float(JvmNatives::int_bits_to_float(*i))))
            } else {
                None
            }
        }
        (b"java/lang/Double", b"doubleToRawLongBits", b"(D)J") => {
            if let Some(Value::Double(d)) = arguments.first() {
                Some(Some(Value::Long(JvmNatives::double_to_raw_long_bits(*d))))
            } else {
                None
            }
        }
        (b"java/lang/Double", b"longBitsToDouble", b"(J)D") => {
            if let Some(Value::Long(l)) = arguments.first() {
                Some(Some(Value::Double(JvmNatives::long_bits_to_double(*l))))
            } else {
                None
            }
        }
        (b"jdk/internal/misc/CDS", b"getRandomSeedForDumping", b"()J") => {
            Some(Some(Value::Long(Jvm::nano_time())))
        }
        (b"jdk/internal/misc/CDS", b"getCDSConfigStatus", b"()I") => {
            Some(Some(Value::Int(0)))
        }
        (b"jdk/internal/misc/CDS", b"isDumpingClassList0", b"()Z") => {
            Some(Some(Value::Int(0)))
        }
        (b"jdk/internal/misc/CDS", b"isDumpingArchive0", b"()Z") => {
            Some(Some(Value::Int(0)))
        }
        (b"jdk/internal/misc/CDS", b"isSharingEnabled0", b"()Z") => {
            // No CDS archive: behave as if sharing is unavailable.
            Some(Some(Value::Int(0)))
        }
        (b"jdk/internal/misc/CDS", b"logLambdaFormInvoker", b"(Ljava/lang/String;)V") => Some(None),
        (b"jdk/internal/misc/CDS", b"initializeFromArchive", b"(Ljava/lang/Class;)V") => Some(None),
        (b"jdk/internal/misc/CDS", b"defineArchivedModules", b"(Ljava/lang/ClassLoader;Ljava/lang/ClassLoader;)V") => Some(None),
        (b"jdk/internal/misc/CDS", b"dumpClassList", b"(Ljava/lang/String;)V") => Some(None),
        (b"jdk/internal/misc/CDS", b"dumpDynamicArchive", b"(Ljava/lang/String;)V") => Some(None),
        (b"jdk/internal/util/SystemProps$Raw", b"platformProperties", b"()[Ljava/lang/String;") => {
            // Positional array indexed by SystemProps.Raw NDX constants.
            let slots = super::systemProperties::platform_property_slots(loader);
            let component = loader.storage.alloc_slice_copy(b"java/lang/String");
            let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                slots.len() as i32,
                component,
            )?;
            let mut leaked = Vec::with_capacity(slots.len());
            for (index, slot) in slots.iter().enumerate() {
                if let Some(value) = slot {
                    let value: &'static str = Box::leak(value.clone().into_boxed_str());
                    leaked.push(value);
                    array.elements[index] =
                        Value::HostString(loader.storage.alloc_str(value));
                }
            }
            Some(Some(Value::ObjectArray(loader.alloc_obj_array(array))))
        }
        (b"jdk/internal/util/SystemProps$Raw", b"vmProperties", b"()[Ljava/lang/String;") => {
            let mut pairs: Vec<(String, String)> =
                super::systemProperties::platform_properties(loader);
            {
                let props = loader.systemProperties.borrow();
                for (key, value) in props.iter() {
                    pairs.push((key.clone(), (*value).to_owned()));
                }
            }
            let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                pairs.len() as i32 * 2,
                b"java/lang/String",
            )
            .unwrap();
            let mut leaked = Vec::with_capacity(pairs.len() * 2);
            for (index, (key, value)) in pairs.iter().enumerate() {
                let key_ptr: &'static str = Box::leak(key.clone().into_boxed_str());
                let value_ptr: &'static str = Box::leak(value.clone().into_boxed_str());
                leaked.push(key_ptr);
                leaked.push(value_ptr);
                array.elements[index * 2] = Value::HostString(loader.storage.alloc_str(key_ptr));
                array.elements[index * 2 + 1] =
                    Value::HostString(loader.storage.alloc_str(value_ptr));
            }
            Some(Some(Value::ObjectArray(loader.alloc_obj_array(array))))
        }
        (b"jdk/internal/reflect/Reflection", b"getCallerClass", b"()Ljava/lang/Class;") => {
            let len = loader.call_stack.len();
            let caller = len
                .checked_sub(2)
                .and_then(|i| loader.call_stack.get(i))
                .or_else(|| loader.call_stack.last());
            Some(Some(caller.map(Value::ClassMirror).unwrap_or(Value::Null)))
        }
        (
            b"jdk/internal/reflect/Reflection",
            b"getClassAccessFlags",
            b"(Ljava/lang/Class;)I",
        ) => {
            let flags = match arguments.first() {
                Some(Value::ClassMirror(name)) => {
                    let mut name = *name;
                    if name.starts_with(b":") {
                        Some(0x0411i32)
                    } else {
                        while name.starts_with(b"[") {
                            name = &name[1..];
                            if name.starts_with(b"L") {
                                name = &name[1..name.len() - 1];
                                break;
                            }
                            if name.len() == 1 {
                                name = b":V";
                                break;
                            }
                        }
                        if name.starts_with(b":") {
                            Some(0x0411)
                        } else {
                            loader
                                .load(name)
                                .ok()
                                .map(|klass| i32::from(klass.class.accessFlags))
                        }
                    }
                }
                _ => None,
            };
            flags.map(|f| Some(Value::Int(f)))
        }
        (b"java/lang/reflect/Array", b"newArray", b"(Ljava/lang/Class;I)Ljava/lang/Object;") => {
            let length = arguments
                .get(1)
                .and_then(|value| match value {
                    Value::Int(i) => Some(*i),
                    _ => None,
                })
                .unwrap_or(0);
            match arguments.first() {
                Some(Value::ClassMirror(component)) => {
                    let descriptor: Vec<u8> = if component.starts_with(b":") {
                        let tag = match component.get(1..).unwrap_or(b"") {
                            b"Z" => b'Z',
                            b"B" => b'B',
                            b"C" => b'C',
                            b"S" => b'S',
                            b"I" => b'I',
                            b"J" => b'J',
                            b"F" => b'F',
                            b"D" => b'D',
                            _ => return Ok(Some(Some(Value::Null))),
                        };
                        vec![b'[', tag]
                    } else if component.starts_with(b"[") {
                        let mut desc = Vec::with_capacity(component.len() + 1);
                        desc.push(b'[');
                        desc.extend_from_slice(component);
                        desc
                    } else {
                        let mut desc = Vec::with_capacity(component.len() + 3);
                        desc.extend_from_slice(b"[L");
                        desc.extend_from_slice(component);
                        desc.push(b';');
                        desc
                    };
                    let descriptor = loader.storage.alloc_slice_copy(&descriptor);
                    return crate::share::oops::arrayKlass::ArrayKlass::multi(
                        loader, descriptor, &[length],
                    )
                    .map(|value| Some(Some(value)));
                }
                _ => return Ok(Some(Some(Value::Null))),
            }
        }
        (b"jdk/internal/reflect/Reflection", b"areNestMates", b"(Ljava/lang/Class;Ljava/lang/Class;)Z") => {
            let mirror_name = |value: &Value<'a>| -> Option<&'a [u8]> {
                match value {
                    Value::ClassMirror(name) => Some(*name),
                    Value::Object(handle) => loader
                        .class_object_names
                        .borrow()
                        .get(handle)
                        .copied(),
                    _ => None,
                }
            };
            let host_of = |value: &Value<'a>| -> Option<Vec<u8>> {
                let name = mirror_name(value)?;
                let name = name
                    .strip_prefix(b"[")
                    .and_then(|rest| rest.strip_prefix(b"L"))
                    .and_then(|rest| rest.strip_suffix(b";"))
                    .unwrap_or(name);
                if name.starts_with(b":") {
                    return None;
                }
                loader
                    .load(name)
                    .ok()
                    .map(|klass| {
                        klass
                            .class
                            .nest_host()
                            .map(|host| host.to_vec())
                            .unwrap_or_else(|| klass.class.name.to_vec())
                    })
            };
            let first = arguments.first().and_then(host_of);
            let second = arguments.get(1).and_then(host_of);
            Some(Some(Value::Int(i32::from(
                first.is_some() && first == second,
            ))))
        }
        (b"jdk/internal/reflect/Reflection", b"getCallerClass", b"(I)Ljava/lang/Class;") => {
            let depth = arguments
                .first()
                .and_then(|value| match value {
                    Value::Int(i) => Some(*i as usize),
                    _ => None,
                })
                .unwrap_or(0);
            let caller = loader
                .call_stack
                .len()
                .checked_sub(depth + 2)
                .and_then(|i| loader.call_stack.get(i));
            Some(Some(caller.map(Value::ClassMirror).unwrap_or(Value::Null)))
        }
        (b"jdk/internal/jimage/NativeImageBuffer", b"getNativeMap", b"(Ljava/lang/String;)Ljava/nio/ByteBuffer;") => {
            Some(Some(jimage_native_map(loader, arguments, output)?))
        }
        _ if owner == b"java/util/zip/Inflater"
            || owner == b"java/util/zip/Deflater"
            || owner == b"java/util/zip/CRC32" =>
        {
            match super::zipNatives::statik(loader, owner, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => return Ok(None),
            }
        }
        _ if owner == b"java/lang/ProcessImpl"
            || owner == b"java/lang/ProcessHandleImpl"
            || owner == b"java/lang/ProcessHandleImpl$Info" =>
        {
            match super::processNatives::statik(loader, owner, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => return Ok(None),
            }
        }
        _ if owner == b"sun/nio/fs/WindowsNativeDispatcher" => {
            match super::nioFileNatives::dispatcher(loader, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => return Ok(None),
            }
        }
        _ if owner == b"java/nio/MappedMemoryUtils" => {
            match super::nioFileNatives::mapped_memory_utils(loader, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => return Ok(None),
            }
        }
        _ if owner == b"sun/nio/ch/FileDispatcherImpl" || owner == b"sun/nio/ch/WindowsFileDispatcherImpl" => {
            match super::nioFileNatives::dispatcher_impl(loader, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => return Ok(None),
            }
        }
        _ if owner == b"sun/nio/ch/IOUtil" => {
            match super::nioFileNatives::io_util(loader, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => return Ok(None),
            }
        }
        (b"sun/security/provider/NativeSeedGenerator", b"nativeGenerateSeed", b"([B)Z") => {
            let filled = match arguments.first().copied() {
                Some(Value::PrimitiveArray(handle)) => {
                    let mut arrays = loader.arrays.borrow_mut();
                    match arrays.get_mut(handle) {
                        Some(crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes)) => {
                            let mut buf = vec![0u8; bytes.len()];
                            let ok = os_random(&mut buf);
                            for (slot, &byte) in bytes.iter_mut().zip(buf.iter()) {
                                *slot = byte as i8;
                            }
                            ok
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            Some(Some(Value::Int(i32::from(filled))))
        }
        _ if ((owner == b"java/io/WinNTFileSystem"
                || owner == b"java/io/UnixFileSystem"
                || owner == b"java/io/FileDescriptor"
                || owner == b"java/io/FileInputStream"
                || owner == b"java/io/FileOutputStream"
                || owner == b"java/io/RandomAccessFile")) => {
            match super::fileNatives::invoke_static(loader, name, descriptor, arguments)? {
                Some(_value) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => return Ok(None),
            }
        }
        _ => {
            let class_name = String::from_utf8_lossy(owner);
            let method_name = String::from_utf8_lossy(name);
            let sig = String::from_utf8_lossy(descriptor);
            let result = super::nativeLibraries::NativeLibraries::global().dispatch(
                &class_name,
                &method_name,
                &sig,
                arguments,
            );
            match result {
                Some(_) if descriptor.ends_with(b")V") => Some(None),
                Some(value) => Some(Some(value)),
                None => {
                    return super::jniInvocation::invoke_dll_native(
                        loader, owner, name, descriptor, true, arguments,
                    );
                }
            }
        }
    };
    Ok(result)
}


/// `jdk.internal.jimage.NativeImageBuffer.getNativeMap`: memory-maps the
/// runtime image for the `jrt:` filesystem. We satisfy it with a heap
/// `ByteBuffer` wrapping the file contents (equivalent for read-only use).
fn jimage_native_map<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    arguments: &[crate::share::interpreter::bytecodeInterpreter::Value<'a>],
    output: &mut dyn std::io::Write,
) -> io::Result<crate::share::interpreter::bytecodeInterpreter::Value<'a>> {
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
    use crate::share::interpreter::bytecodeInterpreter::Value;
    let path = match arguments.first().copied() {
        Some(value) => match super::stringNatives::string(loader, value)? {
            Some(path) => path.into_owned(),
            None => return Ok(Value::Null),
        },
        None => return Ok(Value::Null),
    };
    let bytes = match std::fs::read(path.as_str()) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(Value::Null),
    };
    let array = TypeArrayOop::Byte(bytes.into_iter().map(|byte| byte as i8).collect());
    let handle = loader.alloc_type_array(array);
    let klass = loader.load(b"java/nio/ByteBuffer")?;
    let mut args = JavaCallArguments::new();
    args.push_oop(Value::PrimitiveArray(handle));
    let buffer = JavaCalls::call_static(
        loader,
        &klass,
        b"wrap",
        b"([B)Ljava/nio/ByteBuffer;",
        &args,
        output,
    )?;
    Ok(buffer.unwrap_or(Value::Null))
}

/// Fills `bytes` with operating-system entropy.
#[cfg(windows)]
fn os_random(bytes: &mut [u8]) -> bool {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut core::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    if bytes.is_empty() {
        return true;
    }
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk = (bytes.len() - offset).min(u32::MAX as usize) as u32;
        let status = unsafe {
            BCryptGenRandom(
                core::ptr::null_mut(),
                bytes[offset..].as_mut_ptr(),
                chunk,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return false;
        }
        offset += chunk as usize;
    }
    true
}

/// Fills `bytes` with a splitmix64 generator seeded from the OS clock
/// and process state (non-Windows fallback).
#[cfg(not(windows))]
fn os_random(bytes: &mut [u8]) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ ((std::process::id() as u64) << 32);
    let mut state = seed | 1;
    let mut buffer = [0u8; 8];
    for chunk in bytes.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        buffer[..8].copy_from_slice(&z.to_le_bytes());
        chunk.copy_from_slice(&buffer[..chunk.len()]);
    }
    true
}
