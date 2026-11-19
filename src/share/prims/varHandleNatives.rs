//! Signature-polymorphic access modes of `java.lang.invoke.VarHandle`
//! subclasses, implemented natively for the array-view handles used by
//! `jdk.internal.util.ByteArrayLittleEndian` and friends.
//!
//! Only the `VarHandle*ArrayAs*` family is handled: the receiver class name
//! encodes the viewed type and the `be` field encodes endianness. Heap
//! `byte[]` views (`[B`) are supported; other array kinds fall through.

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::{JavaThrowable, Value},
    oops::instanceOop::InstanceOop,
};
use std::io;

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

/// (size, is_float, is_double) for the viewed element type.
fn view_kind(class_name: &[u8]) -> Option<(usize, bool, bool)> {
    let name = class_name;
    if name.windows(7).any(|w| w == b"AsBytes") {
        Some((1, false, false))
    } else if name.windows(8).any(|w| w == b"AsShorts") || name.windows(8).any(|w| w == b"AsChars") {
        Some((2, false, false))
    } else if name.windows(7).any(|w| w == b"AsInts") {
        Some((4, false, false))
    } else if name.windows(9).any(|w| w == b"AsFloats") {
        Some((4, true, false))
    } else if name.windows(8).any(|w| w == b"AsLongs") {
        Some((8, false, false))
    } else if name.windows(9).any(|w| w == b"AsDoubles") {
        Some((8, false, true))
    } else {
        None
    }
}

/// Parses `(params)return` of a signature-polymorphic descriptor into the
/// parameter type list and return type char.
fn parse_descriptor(descriptor: &[u8]) -> Option<(Vec<Vec<u8>>, u8)> {
    let open = descriptor.iter().position(|&b| b == b'(')?;
    let close = descriptor.iter().rposition(|&b| b == b')')?;
    if close <= open + 1 {
        return None;
    }
    let params_raw = &descriptor[open + 1..close];
    let ret = *descriptor.get(close + 1)?;
    let mut params = Vec::new();
    let mut i = 0;
    while i < params_raw.len() {
        let b = params_raw[i];
        if b == b'[' || b == b'L' {
            let dims = params_raw[i..].iter().take_while(|&&c| c == b'[').count();
            let elem_index = i + dims;
            let elem = *params_raw.get(elem_index)?;
            let end = if elem == b'L' {
                params_raw[elem_index..]
                    .iter()
                    .position(|&c| c == b';')
                    .map(|pos| elem_index + pos + 1)?
            } else {
                elem_index + 1
            };
            params.push(params_raw[i..end].to_vec());
            i = end;
        } else {
            params.push(vec![b]);
            i += 1;
        }
    }
    Some((params, ret))
}

fn throwable<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: &str) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
        loader,
        &klass,
        &mut io::sink(),
        0,
    );
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    if let Ok(slot) = klass.instanceField(b"detailMessage", b"Ljava/lang/String;") {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle) {
            object.fields[slot] = Value::HostString(loader.storage.alloc_str(message));
        }
    }
    Ok(handle)
}

fn index_out_of_bounds<'a>(loader: &ClassLoader<'a>) -> io::Error {
    match throwable(loader, b"java/lang/IndexOutOfBoundsException", "Index out of bounds") {
        Ok(handle) => io::Error::other(JavaThrowable(handle)),
        Err(_) => io::Error::other("IndexOutOfBoundsException"),
    }
}

fn byte_array<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<(usize, usize)> {
    let Value::PrimitiveArray(handle) = value else {
        return Err(invalid("Expected byte array"));
    };
    let arrays = loader.arrays.borrow();
    let array = arrays
        .get(handle)
        .ok_or_else(|| invalid("Invalid array"))?;
    if let crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) = array {
        Ok((handle, bytes.len()))
    } else {
        Err(invalid("Expected byte array"))
    }
}

fn read_view(loader: &ClassLoader<'_>, array: usize, offset: usize, size: usize, be: bool, is_float: bool, is_double: bool) -> io::Result<Value<'static>> {
    let arrays = loader.arrays.borrow();
    let array = arrays.get(array).ok_or_else(|| invalid("Invalid array"))?;
    let crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) = array else {
        return Err(invalid("Expected byte array"));
    };
    let mut raw = [0u8; 8];
    raw[..size].copy_from_slice(&bytes[offset..offset + size].iter().map(|&b| b as u8).collect::<Vec<_>>());
    if be {
        raw[..size].reverse();
    }
    Ok(match (size, is_float, is_double) {
        (1, _, _) => Value::Int(i32::from(raw[0])),
        (2, _, _) => Value::Int(i16::from_le_bytes([raw[0], raw[1]]) as i32),
        (4, true, _) => Value::Float(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
        (4, _, _) => Value::Int(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
        (8, _, true) => Value::Double(f64::from_le_bytes(raw)),
        (8, _, _) => Value::Long(i64::from_le_bytes(raw)),
        _ => return Err(invalid("Unsupported view size")),
    })
}

fn write_view(loader: &ClassLoader<'_>, array: usize, offset: usize, size: usize, be: bool, value: Value<'_>) -> io::Result<()> {
    let mut raw = [0u8; 8];
    match value {
        Value::Int(v) if size == 1 => raw[0] = v as u8,
        Value::Int(v) if size == 2 => raw[..2].copy_from_slice(&(v as i16).to_le_bytes()),
        Value::Int(v) if size == 4 => raw[..4].copy_from_slice(&v.to_le_bytes()),
        Value::Long(v) if size == 8 => raw.copy_from_slice(&v.to_le_bytes()),
        Value::Float(v) => raw[..4].copy_from_slice(&v.to_le_bytes()),
        Value::Double(v) => raw.copy_from_slice(&v.to_le_bytes()),
        _ => return Err(invalid("Mismatched view value")),
    }
    if be {
        raw[..size].reverse();
    }
    let mut arrays = loader.arrays.borrow_mut();
    let array = arrays.get_mut(array).ok_or_else(|| invalid("Invalid array"))?;
    let crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) = array else {
        return Err(invalid("Expected byte array"));
    };
    for (i, &b) in raw[..size].iter().enumerate() {
        bytes[offset + i] = b as i8;
    }
    Ok(())
}

/// Instance-native entry point used by `nativeLookup`: resolves the receiver
/// VarHandle's class from `arguments[0]` and delegates to `polymorphic`.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'static>>>> {
    let Value::Object(handle) = arguments.first().copied().unwrap_or(Value::Null) else {
        return Ok(None);
    };
    let klass_name = {
        let objects = loader.borrow_objects();
        let Some(object) = objects.get(handle) else {
            return Ok(None);
        };
        object.klass.class.name
    };
    match polymorphic(loader, klass_name, name, descriptor, arguments)? {
        Some(value) => {
            // Void access modes (set*) are handled but push no value.
            if descriptor
                .iter()
                .rposition(|&b| b == b')')
                .and_then(|i| descriptor.get(i + 1))
                == Some(&b'V')
            {
                Ok(Some(None))
            } else {
                Ok(Some(Some(value)))
            }
        }
        None => Ok(None),
    }
}

/// Attempts to service a signature-polymorphic VarHandle access on an
/// array-view receiver. Returns `Ok(None)` for unsupported shapes.
pub(crate) fn polymorphic<'a>(
    loader: &ClassLoader<'a>,
    receiver_klass_name: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'static>>> {
    if !receiver_klass_name.starts_with(b"java/lang/invoke/VarHandle") {
        return Ok(None);
    }
    if std::env::var_os("JVM25_TRACE_VH").is_some() {
        eprintln!(
            "[varhandle] {}.{}{} args={}",
            String::from_utf8_lossy(receiver_klass_name),
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor),
            arguments.len()
        );
    }
    let Some((size, is_float, is_double)) = view_kind(receiver_klass_name) else {
        return Ok(None);
    };
    let Some((params, ret)) = parse_descriptor(descriptor) else {
        return Ok(None);
    };
    // args[0] = VarHandle receiver; args[1] = byte[]; args[2] = index (int).
    let Some(&Value::PrimitiveArray(array)) = arguments.get(1) else {
        return Ok(None);
    };
    let (_, array_len) = byte_array(loader, Value::PrimitiveArray(array))?;
    let index = match arguments.get(2).copied().unwrap_or(Value::Null) {
        Value::Int(i) => i,
        _ => return Ok(None),
    };
    // For byte[]-view VarHandles the index coordinate is a byte offset
    // (aligned to the element size by the access-mode contract).
    if index < 0 || index as usize + size > array_len {
        return Err(index_out_of_bounds(loader));
    }
    let offset = index as usize;

    // Endianness: `be` field on ByteArrayViewVarHandle.
    let be = {
        let receiver = match arguments.first().copied().unwrap_or(Value::Null) {
            Value::Object(handle) => handle,
            _ => return Ok(None),
        };
        let objects = loader.borrow_objects();
        let object = objects.get(receiver).ok_or_else(|| invalid("Invalid receiver"))?;
        let slot = object
            .klass
            .instanceField(b"be", b"Z")
            .map_err(|_| invalid("Missing be field"))?;
        matches!(object.fields.get(slot), Some(Value::Int(v)) if *v != 0)
    };

    let is_get = matches!(name, b"get" | b"getVolatile" | b"getAcquire" | b"getOpaque");
    let is_set = matches!(name, b"set" | b"setVolatile" | b"setRelease" | b"setOpaque");
    if is_get && params.len() == 2 {
        let value = read_view(loader, array, offset, size, be, is_float, is_double)?;
        return Ok(Some(value));
    }
    if is_set && params.len() == 3 {
        let value = arguments.get(3).copied().unwrap_or(Value::Null);
        write_view(loader, array, offset, size, be, value)?;
        return Ok(Some(Value::Null));
    }
    if name == b"compareAndSet" && params.len() == 4 && ret == b'Z' {
        let expected = arguments.get(3).copied().unwrap_or(Value::Null);
        let new_value = arguments.get(4).copied().unwrap_or(Value::Null);
        // Read current, compare loosely by raw bits for numeric types.
        let current = read_view(loader, array, offset, size, be, is_float, is_double)?;
        let same = match (current, expected) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Long(a), Value::Long(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
            (Value::Double(a), Value::Double(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        };
        if same {
            write_view(loader, array, offset, size, be, new_value)?;
        }
        return Ok(Some(Value::Int(i32::from(same))));
    }
    Ok(None)
}
