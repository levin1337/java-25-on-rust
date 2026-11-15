//! Fast paths for hot pure-Java JDK helper methods.
//!
//! These are ordinary (non-native) JDK methods that show up in inner loops
//! of classfile/ZIP parsing (`java.util.zip.ZipFile$Source.initCEN`,
//! `ArraysSupport`, `Preconditions`). Interpreting each call costs a full
//! frame; the semantics here are exact, small, and side-effect free.

use std::io;

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

fn int_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i32> {
    match arguments.get(index).copied() {
        Some(Value::Int(value)) => Ok(value),
        _ => Err(invalid("Invalid int argument")),
    }
}

/// Reads `len` bytes from a primitive byte array starting at `offset`.
fn byte_slice<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    array_index: usize,
    offset_index: usize,
    len: usize,
) -> io::Result<Vec<u8>> {
    let handle = match arguments.get(array_index).copied() {
        Some(Value::PrimitiveArray(handle)) => handle,
        _ => return Err(invalid("Expected byte array")),
    };
    let offset = int_arg(arguments, offset_index)? as usize;
    let arrays = loader.arrays.borrow();
    let Some(TypeArrayOop::Byte(bytes)) = arrays.get(handle) else {
        return Err(invalid("Expected byte array"));
    };
    if offset + len > bytes.len() {
        return Err(invalid("Array index out of bounds"));
    }
    Ok(bytes[offset..offset + len].iter().map(|&b| b as u8).collect())
}

/// `ArraysSupport.unsignedHashCode(int init, byte[] a, int off, int len)`:
/// `hash = 31*hash + (a[i] & 0xff)` — the Arrays.hashCode contract.
fn unsigned_hash_code(
    loader: &ClassLoader<'_>,
    arguments: &[Value<'_>],
) -> io::Result<Value<'static>> {
    let init = int_arg(arguments, 0)?;
    let handle = match arguments.get(1).copied() {
        Some(Value::PrimitiveArray(handle)) => handle,
        _ => return Err(invalid("Expected byte array")),
    };
    let offset = int_arg(arguments, 2)? as usize;
    let len = int_arg(arguments, 3)? as usize;
    let arrays = loader.arrays.borrow();
    let Some(TypeArrayOop::Byte(bytes)) = arrays.get(handle) else {
        return Err(invalid("Expected byte array"));
    };
    if offset + len > bytes.len() {
        return Err(invalid("Array index out of bounds"));
    }
    let mut hash = init;
    for &b in &bytes[offset..offset + len] {
        hash = 31 * hash + (b as u8 as i32);
    }
    Ok(Value::Int(hash))
}

/// Handles a single static JDK helper call. Returns `Ok(None)` when the
/// (owner, name, descriptor) triple is not one of the fast-pathed methods.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let value = match (owner, name, descriptor) {
        // java.lang.Byte
        (b"java/lang/Byte", b"toUnsignedInt", b"(B)I") => {
            Value::Int(int_arg(arguments, 0)? & 0xff)
        }
        (b"java/lang/Byte", b"toUnsignedLong", b"(B)J") => {
            Value::Long((int_arg(arguments, 0)? & 0xff) as i64)
        }
        // java.lang.Integer unsigned helpers (hot in comparisons/sorts)
        (b"java/lang/Integer", b"toUnsignedLong", b"(I)J") => {
            Value::Long((int_arg(arguments, 0)? as u32) as i64)
        }
        (b"java/lang/Integer", b"compareUnsigned", b"(II)I") => {
            let a = int_arg(arguments, 0)? as u32;
            let b = int_arg(arguments, 1)? as u32;
            Value::Int(a.cmp(&b) as i32)
        }
        (b"java/lang/Integer", b"divideUnsigned", b"(II)I") => {
            let a = int_arg(arguments, 0)? as u32;
            let b = int_arg(arguments, 1)? as u32;
            if b == 0 {
                // Let the real JDK implementation throw ArithmeticException.
                return Ok(None);
            }
            Value::Int((a / b) as i32)
        }
        (b"java/lang/Integer", b"remainderUnsigned", b"(II)I") => {
            let a = int_arg(arguments, 0)? as u32;
            let b = int_arg(arguments, 1)? as u32;
            if b == 0 {
                return Ok(None);
            }
            Value::Int((a % b) as i32)
        }
        // java.util.zip.ZipUtils little-endian readers
        (b"java/util/zip/ZipUtils", b"get16", b"([BI)I") => {
            let b = byte_slice(loader, arguments, 0, 1, 2)?;
            Value::Int((b[0] as i32) | ((b[1] as i32) << 8))
        }
        (b"java/util/zip/ZipUtils", b"get32", b"([BI)J") => {
            let b = byte_slice(loader, arguments, 0, 1, 4)?;
            let v = (b[0] as u32)
                | ((b[1] as u32) << 8)
                | ((b[2] as u32) << 16)
                | ((b[3] as u32) << 24);
            Value::Long(v as i64)
        }
        // jdk.internal.util.ArraysSupport
        (b"jdk/internal/util/ArraysSupport", b"unsignedHashCode", b"(I[BII)I") => {
            return unsigned_hash_code(loader, arguments).map(Some);
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
