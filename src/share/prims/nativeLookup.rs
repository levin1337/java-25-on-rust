/*
 * Port of OpenJDK 25 HotSpot NativeLookup (share/prims/nativeLookup.hpp / nativeLookup.cpp)
 *
 * Implements standard JNI symbol name mangling and resolution of native method entry points:
 * Java_<class_name>_<method_name> and Java_<class_name>_<method_name>__<signature>
 */

use super::jni::NativeMethodFn;
use std::collections::HashMap;
use std::io;
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
pub fn invokeInstance<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[crate::share::interpreter::bytecodeInterpreter::Value<'a>],
) -> io::Result<Option<Option<crate::share::interpreter::bytecodeInterpreter::Value<'a>>>> {
    use super::jvm::Jvm;
    use crate::share::interpreter::bytecodeInterpreter::Value;

    if owner == b"jdk/internal/misc/Unsafe" {
        return super::unsafeNatives::invoke(loader, name, descriptor, arguments);
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
            let file = loader.storage.alloc_str(&format!("/{}", location.display()));
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
            let mut objects = loader.objects.borrow_mut();
            let handle = objects.len();
            objects.push(elements.next().unwrap());
            *element = Value::Object(handle);
        }
        let list_klass = loader.load(b"java/util/ArrayList")?;
        let list = crate::share::oops::instanceOop::InstanceOop::new(list_klass.clone())?;
        let data_index = list_klass.instanceField(b"elementData", b"[Ljava/lang/Object;")?;
        let size_index = list_klass.instanceField(b"size", b"I")?;
        let list_handle = {
            let mut objects = loader.objects.borrow_mut();
            let handle = objects.len();
            objects.push(list);
            handle
        };
        let array_handle = {
            let mut arrays = loader.obj_arrays.borrow_mut();
            let handle = arrays.len();
            arrays.push(array);
            handle
        };
        {
            let mut objects = loader.objects.borrow_mut();
            let list = objects
                .get_mut(list_handle)
                .ok_or_else(|| io::Error::other("Invalid ArrayList object"))?;
            list.fields[data_index] = Value::ObjectArray(array_handle);
            list.fields[size_index] = Value::Int(url_count);
        }
        let enum_klass = loader.load(b"java/util/Collections$3")?;
        let mut output = std::io::sink();
        crate::share::interpreter::bytecodeInterpreter::initialize(loader, &enum_klass, &mut output, 0)?;
        let mut enumeration = crate::share::oops::instanceOop::InstanceOop::new(enum_klass.clone())?;
        let collection_index = enum_klass.instanceField(b"val$c", b"Ljava/util/Collection;")?;
        enumeration.fields[collection_index] = Value::Object(list_handle);
        let iterator_index = enum_klass.instanceField(b"i", b"Ljava/util/Iterator;")?;
        let list_klass_for_iterator = loader.load(b"java/util/ArrayList")?;
        let iterator_klass = loader.load(b"java/util/ArrayList$Itr")?;
        let mut output = std::io::sink();
        crate::share::interpreter::bytecodeInterpreter::initialize(loader, &iterator_klass, &mut output, 0)?;
        let mut iterator = crate::share::oops::instanceOop::InstanceOop::new(iterator_klass.clone())?;
        let cursor_index = iterator_klass.instanceField(b"cursor", b"I")?;
        let last_ret_index = iterator_klass.instanceField(b"lastRet", b"I")?;
        let expected_mod_count_index = iterator_klass.instanceField(b"expectedModCount", b"I")?;
        let outer_class_index = iterator_klass.instanceField(b"this$0", b"Ljava/util/ArrayList;")?;
        iterator.fields[cursor_index] = Value::Int(0);
        iterator.fields[last_ret_index] = Value::Int(-1);
        iterator.fields[expected_mod_count_index] = Value::Int(0);
        iterator.fields[outer_class_index] = Value::Object(list_handle);
        let mut objects = loader.objects.borrow_mut();
        let iterator_handle = objects.len();
        objects.push(iterator);
        let handle = objects.len();
        objects.push(enumeration);
        drop(objects);
        {
            let mut objects = loader.objects.borrow_mut();
            let enumeration = objects
                .get_mut(handle)
                .ok_or_else(|| io::Error::other("Invalid Enumeration object"))?;
            enumeration.fields[iterator_index] = Value::Object(iterator_handle);
        }
        drop(list_klass_for_iterator);
        return Ok(Some(Some(Value::Object(handle))));
    }

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
        (b"java/lang/Object", b"hashCode", b"()I") => {
            let receiver = *arguments
                .first()
                .ok_or_else(|| io::Error::other("Missing receiver"))?;
            let Some(hash) = super::objectNatives::hashCode(loader, receiver)? else {
                return Ok(None);
            };
            Some(Value::Int(hash))
        }
        (b"java/lang/Throwable", b"fillInStackTrace", b"(I)Ljava/lang/Throwable;") => {
            Some(arguments.first().copied().unwrap_or(Value::Null))
        }
        (b"java/lang/Object", b"notify", b"()V")
        | (b"java/lang/Object", b"notifyAll", b"()V")
        | (b"java/lang/Object", b"wait0", b"(J)V") => None,
        _ => return Ok(None),
    };

    Ok(Some(result))
}

pub fn invokeStatic<'a>(
    loader: &crate::share::classfile::classLoader::ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[crate::share::interpreter::bytecodeInterpreter::Value<'a>],
) -> Option<crate::share::interpreter::bytecodeInterpreter::Value<'a>> {
    use super::jvm::Jvm;
    use super::jvmNatives::JvmNatives;
    use crate::share::interpreter::bytecodeInterpreter::Value;

    match (owner, name, descriptor) {
        (b"java/lang/StringUTF16", b"isBigEndian", b"()Z") => {
            Some(Value::Int(i32::from(cfg!(target_endian = "big"))))
        }
        (b"java/lang/System", b"currentTimeMillis", b"()J") => {
            Some(Value::Long(Jvm::current_time_millis()))
        }
        (b"java/lang/System", b"nanoTime", b"()J") => Some(Value::Long(Jvm::nano_time())),
        (b"java/lang/Thread", b"currentThread", b"()Ljava/lang/Thread;") => loader
            .currentThreadObject()
            .ok()
            .map(Value::Object),
        (b"java/lang/Thread", b"currentCarrierThread", b"()Ljava/lang/Thread;") => loader
            .currentThreadObject()
            .ok()
            .map(Value::Object),
        (b"java/lang/Thread", b"registerNatives", b"()V")
        | (b"java/lang/Thread", b"yield0", b"()V")
        | (b"java/lang/Thread", b"sleep0", b"(J)V")
        | (b"java/lang/Thread", b"clearInterruptEvent", b"()V") => {
            return Some(Value::Null);
        }
        (b"java/lang/Thread", b"holdsLock", b"(Ljava/lang/Object;)Z") => {
            Some(Value::Int(i32::from(matches!(
                arguments.first(),
                Some(Value::Null) | Some(Value::Empty)
            ))))
        }
        (b"java/lang/Thread", b"getThreads", b"()[Ljava/lang/Thread;") => {
            loader.currentThreadObject().ok().map(|handle| {
                let array_handle = loader.obj_arrays.borrow_mut().len();
                let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                    1,
                    b"java/lang/Thread",
                )
                .unwrap();
                array.elements[0] = Value::Object(handle);
                loader.obj_arrays.borrow_mut().push(array);
                Value::ObjectArray(array_handle)
            })
        }
        (b"java/lang/Thread", b"dumpThreads", b"([Ljava/lang/Thread;)[[Ljava/lang/StackTraceElement;") => {
            let component = loader.storage.alloc_slice_copy(b"[Ljava/lang/StackTraceElement;");
            let array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(0, component).unwrap();
            let mut arrays = loader.obj_arrays.borrow_mut();
            let handle = arrays.len();
            arrays.push(array);
            Some(Value::ObjectArray(handle))
        }
        (b"java/lang/System", b"identityHashCode", b"(Ljava/lang/Object;)I") => arguments
            .first()
            .map(|&value| Value::Int(super::objectNatives::identity(value))),
        (b"java/lang/Float", b"floatToRawIntBits", b"(F)I") => {
            if let Some(Value::Float(f)) = arguments.first() {
                Some(Value::Int(JvmNatives::float_to_raw_int_bits(*f)))
            } else {
                None
            }
        }
        (b"java/lang/Float", b"intBitsToFloat", b"(I)F") => {
            if let Some(Value::Int(i)) = arguments.first() {
                Some(Value::Float(JvmNatives::int_bits_to_float(*i)))
            } else {
                None
            }
        }
        (b"java/lang/Double", b"doubleToRawLongBits", b"(D)J") => {
            if let Some(Value::Double(d)) = arguments.first() {
                Some(Value::Long(JvmNatives::double_to_raw_long_bits(*d)))
            } else {
                None
            }
        }
        (b"java/lang/Double", b"longBitsToDouble", b"(J)D") => {
            if let Some(Value::Long(l)) = arguments.first() {
                Some(Value::Double(JvmNatives::long_bits_to_double(*l)))
            } else {
                None
            }
        }
        _ => {
            let class_name = String::from_utf8_lossy(owner);
            let method_name = String::from_utf8_lossy(name);
            let sig = String::from_utf8_lossy(descriptor);
            super::nativeLibraries::NativeLibraries::global().dispatch(
                &class_name,
                &method_name,
                &sig,
                arguments,
            )
        }
    }
}
