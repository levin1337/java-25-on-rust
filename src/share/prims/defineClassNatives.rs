//! Runtime class definition support for `java.lang.ClassLoader.defineClass0`
//! and related entry points. This is what backs
//! `MethodHandles.Lookup.defineHiddenClass` / `LambdaForm.compileToBytecode`
//! invoker spinning: ASM-generated class bytes are parsed and registered with
//! the loader exactly like classpath classes.

use crate::classfile::ClassFile;
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use std::io;

fn linkage_error(error: crate::classfile::ClassError) -> io::Error {
    crate::share::classfile::linkageError::LinkageError::new(
        crate::share::classfile::linkageError::LinkageErrorKind::ClassFormat,
        error.to_string(),
    )
    .io()
}

fn define_from_window<'a>(
    loader: &ClassLoader<'a>,
    bytes_value: Value<'a>,
    offset: usize,
    length: usize,
    protection_domain: Value<'a>,
) -> io::Result<Value<'a>> {
    let Value::PrimitiveArray(handle) = bytes_value else {
        return Err(io::Error::other("defineClass: expected byte[]"));
    };
    // Copy the requested window into loader storage so the parsed ClassFile
    // borrows for 'a.
    let window: Vec<u8> = {
        let arrays = loader.arrays.borrow();
        let Some(TypeArrayOop::Byte(bytes)) = arrays.get(handle) else {
            return Err(io::Error::other("defineClass: expected byte[]"));
        };
        if offset > bytes.len() || length > bytes.len() - offset {
            return Err(io::Error::other("defineClass: offset/length out of bounds"));
        }
        bytes[offset..offset + length]
            .iter()
            .map(|&b| b as u8)
            .collect()
    };
    let owned = loader.storage.alloc_slice_copy(&window);
    let class = ClassFile::parse(owned, false).map_err(linkage_error)?;
    let name = class.name;
    loader.define_class(class)?;
    if let Value::Object(handle) = protection_domain {
        if std::env::var_os("JVM25_TRACE_PD").is_some() {
            crate::share::prims::classNatives::trace_pd_url(loader, handle, name);
        }
        loader.record_class_protection_domain(name, handle);
    }
    Ok(Value::ClassMirror(name))
}

/// `ClassLoader.defineClass0(ClassLoader, Class, String, byte[], int, int,
/// ProtectionDomain, boolean, int, Object)`.
///
/// Defines a class (including hidden classes produced by
/// `InvokerBytecodeGenerator`) from raw classfile bytes and returns its
/// `java.lang.Class` mirror.
pub(crate) fn define_class0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    // (loader, pdClass, name, bytes, offset, length, protectionDomain,
    //  initialize, mode, classData)
    if let Value::Object(handle) = arguments.first().copied().unwrap_or(Value::Null) {
        loader.register_delegate_loader(handle);
    }
    let bytes_value = arguments.get(3).copied().unwrap_or(Value::Null);
    let offset = arguments
        .get(4)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    let length = arguments
        .get(5)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    let protection_domain = arguments.get(6).copied().unwrap_or(Value::Null);
    define_from_window(loader, bytes_value, offset, length, protection_domain).map(Some)
}

/// Reads an instance field of a buffer object, walking the hierarchy.
fn buffer_field<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    name: &[u8],
    descriptor: &[u8],
) -> Option<Value<'a>> {
    let objects = loader.borrow_objects();
    let object = objects.get(handle)?;
    let slot = object.klass.instanceField(name, descriptor).ok()?;
    object.fields.get(slot).copied()
}

/// `ClassLoader.defineClass2(ClassLoader, String, ByteBuffer, int, int,
/// ProtectionDomain, String)` — same as `defineClass1` but the class bytes
/// come from a `java.nio.ByteBuffer` (heap or direct) instead of `byte[]`.
pub(crate) fn define_class2<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    // (loader, name, buffer, offset, length, protectionDomain, source)
    if let Value::Object(handle) = arguments.first().copied().unwrap_or(Value::Null) {
        loader.register_delegate_loader(handle);
    }
    let Value::Object(handle) = arguments.get(2).copied().unwrap_or(Value::Null) else {
        return Err(io::Error::other("defineClass2: expected ByteBuffer"));
    };
    let offset = arguments
        .get(3)
        .and_then(|value| value.int().ok())
        .unwrap_or(0)
        .max(0) as usize;
    let length = arguments
        .get(4)
        .and_then(|value| value.int().ok())
        .unwrap_or(0)
        .max(0) as usize;
    // Heap buffers carry `hb` + `offset`; direct buffers carry `address`.
    let window: Vec<u8> = if let Some(Value::PrimitiveArray(array)) =
        buffer_field(loader, handle, b"hb", b"[B")
    {
        let base = match buffer_field(loader, handle, b"offset", b"I") {
            Some(Value::Int(v)) => v.max(0) as usize,
            _ => 0,
        };
        let arrays = loader.arrays.borrow();
        let Some(TypeArrayOop::Byte(bytes)) = arrays.get(array) else {
            return Err(io::Error::other("defineClass2: expected byte[] backing"));
        };
        let start = base + offset;
        if start > bytes.len() || length > bytes.len() - start {
            return Err(io::Error::other("defineClass2: offset/length out of bounds"));
        }
        bytes[start..start + length].iter().map(|&b| b as u8).collect()
    } else {
        let address = match buffer_field(loader, handle, b"address", b"J") {
            Some(Value::Long(v)) if v != 0 => v as u64,
            _ => return Err(io::Error::other("defineClass2: not a buffer")),
        };
        super::offHeapMemory::OffHeapMemory::global()
            .read_bytes(address + offset as u64, length)?
    };
    let owned = loader.storage.alloc_slice_copy(&window);
    let class = ClassFile::parse(owned, false).map_err(linkage_error)?;
    let name = class.name;
    loader.define_class(class)?;
    let protection_domain = arguments.get(5).copied().unwrap_or(Value::Null);
    if let Value::Object(handle) = protection_domain {
        if std::env::var_os("JVM25_TRACE_PD").is_some() {
            crate::share::prims::classNatives::trace_pd_url(loader, handle, name);
        }
        loader.record_class_protection_domain(name, handle);
    }
    Ok(Some(Value::ClassMirror(name)))
}

/// `ClassLoader.defineClass1(ClassLoader, String, byte[], int, int,
/// ProtectionDomain, String)` — the JDK 9+ entry point used by
/// `ClassLoader.defineClass` for ordinary (non-hidden) classes.
pub(crate) fn define_class1<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    // (loader, name, bytes, offset, length, protectionDomain, source)
    if let Value::Object(handle) = arguments.first().copied().unwrap_or(Value::Null) {
        loader.register_delegate_loader(handle);
    }
    let bytes_value = arguments.get(2).copied().unwrap_or(Value::Null);
    let offset = arguments
        .get(3)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    let length = arguments
        .get(4)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    let protection_domain = arguments.get(5).copied().unwrap_or(Value::Null);
    define_from_window(loader, bytes_value, offset, length, protection_domain).map(Some)
}
