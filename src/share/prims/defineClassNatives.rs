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
    let bytes_value = arguments.get(3).copied().unwrap_or(Value::Null);
    let offset = arguments
        .get(4)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    let length = arguments
        .get(5)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    define_from_window(loader, bytes_value, offset, length).map(Some)
}

/// `ClassLoader.defineClass1(ClassLoader, String, byte[], int, int,
/// ProtectionDomain, String)` — the JDK 9+ entry point used by
/// `ClassLoader.defineClass` for ordinary (non-hidden) classes.
pub(crate) fn define_class1<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    // (loader, name, bytes, offset, length, protectionDomain, source)
    let bytes_value = arguments.get(2).copied().unwrap_or(Value::Null);
    let offset = arguments
        .get(3)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    let length = arguments
        .get(4)
        .and_then(|value| value.int().ok())
        .unwrap_or(0) as usize;
    define_from_window(loader, bytes_value, offset, length).map(Some)
}
