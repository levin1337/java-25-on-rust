/*
 * Runtime implementation of jdk.internal.misc.Unsafe native methods.
 *
 * Java fields are stored in typed slots and primitive arrays in typed
 * vectors. Unsafe byte offsets are mapped back to FieldInfo metadata for
 * objects and to native-endian element storage for arrays.
 */

use super::{offHeapMemory::OffHeapMemory, stringNatives, r#unsafe::Unsafe};
use crate::classfile::ClassFile;
use crate::share::{
    classfile::{
        classLoader::ClassLoader,
        linkageError::{LinkageError, LinkageErrorKind},
    },
    interpreter::bytecodeInterpreter::Value,
    oops::{instanceKlass::InitState, instanceOop::InstanceOop, typeArrayOop::TypeArrayOop},
};
use std::io::{self, Error};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// Sentinel offset returned by `Thread.getNextThreadIdOffset`. Unsafe accesses
/// with a null object and this offset operate on a VM-side thread-id counter,
/// mirroring HotSpot's static-field-offset fast path for `Thread.threadSeqNumber`.
pub(crate) const THREAD_TID_SENTINEL_OFFSET: i64 = i64::MAX;

static THREAD_TID_COUNTER: AtomicI64 = AtomicI64::new(0);

type NativeResult<'a> = Option<Option<Value<'a>>>;

pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
    output: &mut dyn std::io::Write,
    depth: usize,
) -> io::Result<NativeResult<'a>> {
    if name == b"registerNatives" {
        return Ok(Some(None));
    }

    let result = match (name, descriptor) {
        (b"getBoolean", b"(Ljava/lang/Object;J)Z")
        | (b"getBooleanVolatile", b"(Ljava/lang/Object;J)Z") => Some(Value::Int(i32::from(
            read_raw(loader, arguments, 1, 1)?.boolean()?,
        ))),
        (b"putBoolean", b"(Ljava/lang/Object;JZ)V")
        | (b"putBooleanVolatile", b"(Ljava/lang/Object;JZ)V") => {
            write_raw(
                loader,
                arguments,
                1,
                1,
                Value::Int(i32::from(bool_arg(arguments, 3)?)),
            )?;
            None
        }
        (b"getByte", b"(Ljava/lang/Object;J)B")
        | (b"getByteVolatile", b"(Ljava/lang/Object;J)B") => Some(Value::Int(i32::from(
            read_raw(loader, arguments, 1, 1)?.byte()?,
        ))),
        (b"putByte", b"(Ljava/lang/Object;JB)V")
        | (b"putByteVolatile", b"(Ljava/lang/Object;JB)V") => {
            write_raw(
                loader,
                arguments,
                1,
                1,
                Value::Int(i32::from(byte_arg(arguments, 3)?)),
            )?;
            None
        }
        (b"getShort", b"(Ljava/lang/Object;J)S")
        | (b"getShortVolatile", b"(Ljava/lang/Object;J)S") => Some(Value::Int(i32::from(
            read_raw(loader, arguments, 1, 2)?.short()?,
        ))),
        (b"putShort", b"(Ljava/lang/Object;JS)V")
        | (b"putShortVolatile", b"(Ljava/lang/Object;JS)V") => {
            write_raw(
                loader,
                arguments,
                1,
                2,
                Value::Int(i32::from(short_arg(arguments, 3)?)),
            )?;
            None
        }
        (b"getChar", b"(Ljava/lang/Object;J)C")
        | (b"getCharVolatile", b"(Ljava/lang/Object;J)C") => Some(Value::Int(i32::from(
            read_raw(loader, arguments, 1, 2)?.char()?,
        ))),
        (b"putChar", b"(Ljava/lang/Object;JC)V")
        | (b"putCharVolatile", b"(Ljava/lang/Object;JC)V") => {
            write_raw(
                loader,
                arguments,
                1,
                2,
                Value::Int(i32::from(char_arg(arguments, 3)?)),
            )?;
            None
        }
        (b"getInt", b"(Ljava/lang/Object;J)I") | (b"getIntVolatile", b"(Ljava/lang/Object;J)I") => {
            Some(Value::Int(read_raw(loader, arguments, 1, 4)?.int()?))
        }
        (b"putInt", b"(Ljava/lang/Object;JI)V")
        | (b"putIntVolatile", b"(Ljava/lang/Object;JI)V") => {
            write_raw(loader, arguments, 1, 4, Value::Int(int_arg(arguments, 3)?))?;
            None
        }
        (b"getLong", b"(Ljava/lang/Object;J)J")
        | (b"getLongVolatile", b"(Ljava/lang/Object;J)J") => {
            Some(Value::Long(read_raw(loader, arguments, 1, 8)?.long()?))
        }
        (b"putLong", b"(Ljava/lang/Object;JJ)V")
        | (b"putLongVolatile", b"(Ljava/lang/Object;JJ)V") => {
            write_raw(
                loader,
                arguments,
                1,
                8,
                Value::Long(long_arg(arguments, 3)?),
            )?;
            None
        }
        (b"getFloat", b"(Ljava/lang/Object;J)F")
        | (b"getFloatVolatile", b"(Ljava/lang/Object;J)F") => {
            Some(Value::Float(read_raw(loader, arguments, 1, 4)?.float()?))
        }
        (b"putFloat", b"(Ljava/lang/Object;JF)V")
        | (b"putFloatVolatile", b"(Ljava/lang/Object;JF)V") => {
            write_raw(
                loader,
                arguments,
                1,
                4,
                Value::Float(float_arg(arguments, 3)?),
            )?;
            None
        }
        (b"getDouble", b"(Ljava/lang/Object;J)D")
        | (b"getDoubleVolatile", b"(Ljava/lang/Object;J)D") => {
            Some(Value::Double(read_raw(loader, arguments, 1, 8)?.double()?))
        }
        (b"putDouble", b"(Ljava/lang/Object;JD)V")
        | (b"putDoubleVolatile", b"(Ljava/lang/Object;JD)V") => {
            write_raw(
                loader,
                arguments,
                1,
                8,
                Value::Double(double_arg(arguments, 3)?),
            )?;
            None
        }
        (b"getReference", b"(Ljava/lang/Object;J)Ljava/lang/Object;")
        | (b"getReferenceVolatile", b"(Ljava/lang/Object;J)Ljava/lang/Object;") => {
            Some(read_reference(loader, arguments, 1)?)
        }
        (b"putReference", b"(Ljava/lang/Object;JLjava/lang/Object;)V")
        | (b"putReferenceVolatile", b"(Ljava/lang/Object;JLjava/lang/Object;)V") => {
            write_reference(
                loader,
                arguments,
                1,
                arguments.get(3).copied().unwrap_or(Value::Null),
            )?;
            None
        }
        (
            b"compareAndSetReference",
            b"(Ljava/lang/Object;JLjava/lang/Object;Ljava/lang/Object;)Z",
        ) => {
            let expected = arguments.get(3).copied().unwrap_or(Value::Null);
            let value = arguments.get(4).copied().unwrap_or(Value::Null);
            Some(Value::Int(i32::from(compare_exchange_reference(
                loader, arguments, 1, expected, value, false,
            )?)))
        }
        (
            b"compareAndExchangeReference",
            b"(Ljava/lang/Object;JLjava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
        ) => {
            let expected = arguments.get(3).copied().unwrap_or(Value::Null);
            let value = arguments.get(4).copied().unwrap_or(Value::Null);
            Some(compare_exchange_reference_value(
                loader, arguments, 1, expected, value,
            )?)
        }
        (b"compareAndSetInt", b"(Ljava/lang/Object;JII)Z") => {
            let expected = int_arg(arguments, 3)?;
            let value = int_arg(arguments, 4)?;
            Some(Value::Int(i32::from(compare_exchange_int(
                loader, arguments, 1, expected, value, false,
            )?)))
        }
        (b"compareAndExchangeInt", b"(Ljava/lang/Object;JII)I") => {
            let expected = int_arg(arguments, 3)?;
            let value = int_arg(arguments, 4)?;
            Some(Value::Int(compare_exchange_int_value(
                loader, arguments, 1, expected, value,
            )?))
        }
        (b"compareAndSetLong", b"(Ljava/lang/Object;JJJ)Z") => {
            let expected = long_arg(arguments, 3)?;
            let value = long_arg(arguments, 4)?;
            Some(Value::Int(i32::from(compare_exchange_long(
                loader, arguments, 1, expected, value, false,
            )?)))
        }
        (b"compareAndExchangeLong", b"(Ljava/lang/Object;JJJ)J") => {
            let expected = long_arg(arguments, 3)?;
            let value = long_arg(arguments, 4)?;
            Some(Value::Long(compare_exchange_long_value(
                loader, arguments, 1, expected, value,
            )?))
        }
        (b"getAndAddInt", b"(Ljava/lang/Object;JI)I") => {
            let delta = int_arg(arguments, 3)?;
            Some(Value::Int(
                get_and_add(loader, arguments, 1, 4, delta as u32 as u64, false)? as u32 as i32,
            ))
        }
        (b"getAndAddLong", b"(Ljava/lang/Object;JJ)J") => {
            let delta = long_arg(arguments, 3)?;
            Some(Value::Long(
                get_and_add(loader, arguments, 1, 8, delta as u64, true)? as i64,
            ))
        }
        (b"getAndSetInt", b"(Ljava/lang/Object;JI)I") => {
            let value = int_arg(arguments, 3)?;
            let old = read_raw(loader, arguments, 1, 4)?.int()?;
            write_raw(loader, arguments, 1, 4, Value::Int(value))?;
            Some(Value::Int(old))
        }
        (b"getAndSetLong", b"(Ljava/lang/Object;JJ)J") => {
            let value = long_arg(arguments, 3)?;
            let old = read_raw(loader, arguments, 1, 8)?.long()?;
            write_raw(loader, arguments, 1, 8, Value::Long(value))?;
            Some(Value::Long(old))
        }
        (b"getAndSetReference", b"(Ljava/lang/Object;JLjava/lang/Object;)Ljava/lang/Object;") => {
            let new = arguments.get(3).copied().unwrap_or(Value::Null);
            let old = read_reference(loader, arguments, 1)?;
            write_reference(loader, arguments, 1, new)?;
            Some(old)
        }
        (b"allocateMemory0", b"(J)J") => {
            let size = usize::try_from(long_arg(arguments, 1)?)
                .map_err(|_| Error::other("NegativeArraySizeException"))?;
            Some(Value::Long(
                OffHeapMemory::global().allocate_memory(size) as i64
            ))
        }
        (b"reallocateMemory0", b"(JJ)J") => {
            let address = u64::try_from(long_arg(arguments, 1)?)
                .map_err(|_| Error::other("IllegalArgumentException"))?;
            let size = usize::try_from(long_arg(arguments, 2)?)
                .map_err(|_| Error::other("NegativeArraySizeException"))?;
            Some(Value::Long(
                OffHeapMemory::global().reallocate_memory(address, size)? as i64,
            ))
        }
        (b"freeMemory0", b"(J)V") => {
            let address = u64::try_from(long_arg(arguments, 1)?)
                .map_err(|_| Error::other("IllegalArgumentException"))?;
            OffHeapMemory::global().free_memory(address);
            None
        }
        (b"setMemory0", b"(Ljava/lang/Object;JJB)V") => {
            let object = arguments.get(1).copied().unwrap_or(Value::Null);
            let offset = long_arg(arguments, 2)?;
            let bytes = usize::try_from(long_arg(arguments, 3)?)
                .map_err(|_| Error::other("IllegalArgumentException"))?;
            let value = byte_arg(arguments, 4)? as u8;
            write_bytes(loader, object, offset, bytes, value)?;
            None
        }
        (b"copyMemory0", b"(Ljava/lang/Object;JLjava/lang/Object;JJ)V") => {
            let src_object = arguments.get(1).copied().unwrap_or(Value::Null);
            let src_offset = long_arg(arguments, 2)?;
            let dst_object = arguments.get(3).copied().unwrap_or(Value::Null);
            let dst_offset = long_arg(arguments, 4)?;
            let bytes = usize::try_from(long_arg(arguments, 5)?)
                .map_err(|_| Error::other("IllegalArgumentException"))?;
            copy_bytes(
                loader, src_object, src_offset, dst_object, dst_offset, bytes,
            )?;
            None
        }
        (b"copySwapMemory0", b"(Ljava/lang/Object;JLjava/lang/Object;JJJ)V") => {
            // copySwapMemory is used for overlapping swap operations; the
            // interpreter implements it as a safe temporary copy.
            let src_object = arguments.get(1).copied().unwrap_or(Value::Null);
            let src_offset = long_arg(arguments, 2)?;
            let dst_object = arguments.get(3).copied().unwrap_or(Value::Null);
            let dst_offset = long_arg(arguments, 4)?;
            let bytes = usize::try_from(long_arg(arguments, 5)?)
                .map_err(|_| Error::other("IllegalArgumentException"))?;
            let swap = long_arg(arguments, 6)?;
            if swap == 0 {
                copy_bytes(
                    loader, src_object, src_offset, dst_object, dst_offset, bytes,
                )?;
            } else {
                copy_swap_bytes(
                    loader,
                    src_object,
                    src_offset,
                    dst_object,
                    dst_offset,
                    bytes,
                    usize::try_from(swap)
                        .map_err(|_| invalid("Invalid Unsafe swap size"))?,
                )?;
            }
            None
        }
        (b"objectFieldOffset0", b"(Ljava/lang/reflect/Field;)J") => {
            Some(Value::Long(field_offset_from_field(loader, arguments, 1)?))
        }
        (b"objectFieldOffset1", b"(Ljava/lang/Class;Ljava/lang/String;)J") => {
            Some(Value::Long(field_offset_by_name(loader, arguments, 1, 2)?))
        }
        (b"staticFieldOffset0", b"(Ljava/lang/reflect/Field;)J") => Some(Value::Long(
            static_field_offset_from_field(loader, arguments, 1)?,
        )),
        (b"staticFieldBase0", b"(Ljava/lang/reflect/Field;)Ljava/lang/Object;") => {
            Some(static_field_base_from_field(loader, arguments, 1)?)
        }
        (b"shouldBeInitialized0", b"(Ljava/lang/Class;)Z") => Some(Value::Int(i32::from(
            class_initialized(loader, class_arg(arguments, 1)?)?,
        ))),
        (b"ensureClassInitialized0", b"(Ljava/lang/Class;)V") => {
            ensure_class_initialized(loader, class_arg(arguments, 1)?)?;
            None
        }
        (b"arrayBaseOffset0", b"(Ljava/lang/Class;)I") => {
            let class = class_arg(arguments, 1)?;
            Unsafe::arrayLayout(class)?;
            Some(Value::Int(0))
        }
        (b"arrayIndexScale0", b"(Ljava/lang/Class;)I") => {
            let class = class_arg(arguments, 1)?;
            Some(Value::Int(Unsafe::arrayLayout(class)?))
        }
        (b"getLoadAverage0", b"([DI)I") => Some(Value::Int(-1)),
        (b"allocateInstance", b"(Ljava/lang/Class;)Ljava/lang/Object;") => {
            Some(allocate_instance(loader, class_arg(arguments, 1)?)?)
        }
        (b"getUncompressedObject", b"(J)Ljava/lang/Object;") => Some(Value::Null),
        (
            b"defineClass0",
            b"(Ljava/lang/String;[BIILjava/lang/ClassLoader;Ljava/security/ProtectionDomain;)Ljava/lang/Class;",
        ) => Some(define_class0(loader, arguments)?),
        (b"unpark", b"(Ljava/lang/Object;)V") => {
            crate::share::runtime::threading::unpark(
                loader,
                arguments.get(1).copied().unwrap_or(Value::Null),
            )?;
            None
        }
        (b"park", b"(ZJ)V") => {
            let is_absolute = matches!(arguments.get(1), Some(Value::Int(v)) if *v != 0);
            let time = match arguments.get(2) {
                Some(Value::Long(v)) => *v,
                _ => 0,
            };
            crate::share::runtime::threading::park(loader, is_absolute, time, output, depth)?;
            None
        }
        (b"loadFence", b"()V") | (b"storeFence", b"()V") | (b"fullFence", b"()V") => {
            std::sync::atomic::fence(Ordering::SeqCst);
            None
        }
        (b"writeback0", b"(J)V")
        | (b"writebackPreSync0", b"()V")
        | (b"writebackPostSync0", b"()V") => None,
        (b"throwException", b"(Ljava/lang/Throwable;)V") => {
            return Err(Error::other(
                "Java exception thrown by Unsafe.throwException",
            ));
        }
        _ => {
            if let Some((kind, op)) = bitwise_access(name, descriptor) {
                Some(get_and_bitwise_typed(loader, arguments, kind, op)?)
            } else {
                return Ok(None);
            }
        }
    };

    Ok(Some(result))
}

#[derive(Clone, Copy)]
struct Raw {
    value: u64,
}

impl Raw {
    fn boolean(self) -> io::Result<bool> {
        Ok(self.value & 1 != 0)
    }
    fn byte(self) -> io::Result<i8> {
        Ok(self.value as u8 as i8)
    }
    fn short(self) -> io::Result<i16> {
        Ok(self.value as u16 as i16)
    }
    fn char(self) -> io::Result<u16> {
        Ok(self.value as u16)
    }
    fn int(self) -> io::Result<i32> {
        Ok(self.value as u32 as i32)
    }
    fn long(self) -> io::Result<i64> {
        Ok(self.value as i64)
    }
    fn float(self) -> io::Result<f32> {
        Ok(f32::from_bits(self.value as u32))
    }
    fn double(self) -> io::Result<f64> {
        Ok(f64::from_bits(self.value))
    }
}

fn invalid(message: &'static str) -> Error {
    Error::other(message)
}

fn argument<'a>(arguments: &[Value<'a>], index: usize) -> io::Result<Value<'a>> {
    arguments
        .get(index)
        .copied()
        .ok_or_else(|| invalid("Missing Unsafe argument"))
}

fn long_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i64> {
    match argument(arguments, index)? {
        Value::Long(value) => Ok(value),
        Value::Int(value) => Ok(i64::from(value)),
        _ => Err(invalid("Expected long argument")),
    }
}

fn int_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i32> {
    match argument(arguments, index)? {
        Value::Int(value) => Ok(value),
        _ => Err(invalid("Expected int argument")),
    }
}

fn byte_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i8> {
    Ok(int_arg(arguments, index)? as i8)
}

fn short_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i16> {
    Ok(int_arg(arguments, index)? as i16)
}

fn char_arg(arguments: &[Value<'_>], index: usize) -> io::Result<u16> {
    Ok(int_arg(arguments, index)? as u16)
}

fn bool_arg(arguments: &[Value<'_>], index: usize) -> io::Result<bool> {
    Ok(int_arg(arguments, index)? != 0)
}

fn float_arg(arguments: &[Value<'_>], index: usize) -> io::Result<f32> {
    match argument(arguments, index)? {
        Value::Float(value) => Ok(value),
        _ => Err(invalid("Expected float argument")),
    }
}

fn double_arg(arguments: &[Value<'_>], index: usize) -> io::Result<f64> {
    match argument(arguments, index)? {
        Value::Double(value) => Ok(value),
        _ => Err(invalid("Expected double argument")),
    }
}

fn string_arg<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<String> {
    let Some(text) = stringNatives::string(loader, value)? else {
        if std::env::var_os("JVM25_TRACE_STR").is_some() { eprintln!("[str-fail-unsafe] {:?}", std::mem::discriminant(&value)); }
        return Err(invalid(if matches!(value, Value::Null) {
            "NullPointerException"
        } else {
            "Expected java.lang.String"
        }));
    };
    Ok(text.into_owned())
}

fn class_arg<'a>(arguments: &[Value<'a>], index: usize) -> io::Result<&'a [u8]> {
    match argument(arguments, index)? {
        Value::ClassMirror(name) => Ok(name),
        _ => Err(invalid("Expected java.lang.Class")),
    }
}

fn object_arg<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>], index: usize) -> io::Result<Value<'a>> {
    let value = argument(arguments, index)?;
    Ok(stringNatives::materialize(loader, value, &mut io::sink(), 0)?.unwrap_or(value))
}

fn offset_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i64> {
    long_arg(arguments, index)
}

fn find_field(object: &InstanceOop<'_>, offset: i64) -> io::Result<(usize, u8)> {
    let mut current = Some(object.klass.as_ref());
    while let Some(klass) = current {
        for field in &klass.instance_field_infos {
            if i64::from(field.offset) == offset {
                let kind = field
                    .descriptor
                    .first()
                    .copied()
                    .ok_or_else(|| invalid("Invalid field descriptor"))?;
                return Ok((field.slot, kind));
            }
        }
        current = klass.super_klass.as_deref();
    }
    Err(invalid("Invalid Unsafe object field offset"))
}

fn value_to_raw(value: Value<'_>, size: usize) -> io::Result<u64> {
    if size == 0 || size > 8 {
        return Err(invalid("Invalid Unsafe access size"));
    }
    let raw = match value {
        Value::Int(value) => (value as u32) as u64,
        Value::Long(value) => value as u64,
        Value::Float(value) => u64::from(value.to_bits()),
        Value::Double(value) => value.to_bits(),
        _ => return Err(invalid("Unsafe field is not primitive")),
    };
    Ok(match size {
        8 => raw,
        size => raw & ((1u64 << (size * 8)) - 1),
    })
}

fn raw_to_value(kind: u8, raw: u64) -> io::Result<Value<'static>> {
    Ok(match kind {
        b'Z' => Value::Int(i32::from(raw & 1 != 0)),
        b'B' => Value::Int(i32::from(raw as u8 as i8)),
        b'C' => Value::Int(i32::from(raw as u16)),
        b'S' => Value::Int(i32::from(raw as u16 as i16)),
        b'I' => Value::Int(raw as u32 as i32),
        b'J' => Value::Long(raw as i64),
        b'F' => Value::Float(f32::from_bits(raw as u32)),
        b'D' => Value::Double(f64::from_bits(raw)),
        _ => return Err(invalid("Unsafe field is not primitive")),
    })
}

fn element_u64(array: &TypeArrayOop, index: usize) -> io::Result<u64> {
    match array {
        TypeArrayOop::Boolean(values) => values
            .get(index)
            .map(|&value| u64::from(value))
            .ok_or_else(|| invalid("Array index out of bounds")),
        TypeArrayOop::Byte(values) => values
            .get(index)
            .map(|&value| u64::from(value as u8))
            .ok_or_else(|| invalid("Array index out of bounds")),
        TypeArrayOop::Char(values) => values
            .get(index)
            .map(|&value| u64::from(value))
            .ok_or_else(|| invalid("Array index out of bounds")),
        TypeArrayOop::Short(values) => values
            .get(index)
            .map(|&value| u64::from(value as u16))
            .ok_or_else(|| invalid("Array index out of bounds")),
        TypeArrayOop::Int(values) => values
            .get(index)
            .map(|&value| u64::from(value as u32))
            .ok_or_else(|| invalid("Array index out of bounds")),
        TypeArrayOop::Long(values) => values
            .get(index)
            .map(|&value| value as u64)
            .ok_or_else(|| invalid("Array index out of bounds")),
        TypeArrayOop::Float(values) => values
            .get(index)
            .map(|&value| u64::from(value.to_bits()))
            .ok_or_else(|| invalid("Array index out of bounds")),
        TypeArrayOop::Double(values) => values
            .get(index)
            .map(|&value| value.to_bits())
            .ok_or_else(|| invalid("Array index out of bounds")),
    }
}

fn put_element_u64(array: &mut TypeArrayOop, index: usize, raw: u64) -> io::Result<()> {
    match array {
        TypeArrayOop::Boolean(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = (raw & 1) as u8;
        }
        TypeArrayOop::Byte(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = raw as u8 as i8;
        }
        TypeArrayOop::Char(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = raw as u16;
        }
        TypeArrayOop::Short(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = raw as u16 as i16;
        }
        TypeArrayOop::Int(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = raw as u32 as i32;
        }
        TypeArrayOop::Long(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = raw as i64;
        }
        TypeArrayOop::Float(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = f32::from_bits(raw as u32);
        }
        TypeArrayOop::Double(values) => {
            *values
                .get_mut(index)
                .ok_or_else(|| invalid("Array index out of bounds"))? = f64::from_bits(raw);
        }
    }
    Ok(())
}

fn array_byte(array: &TypeArrayOop, offset: usize) -> io::Result<u8> {
    let element_size = array.element_size();
    let index = offset / element_size;
    let shift = (offset % element_size) * 8;
    Ok(((element_u64(array, index)? >> shift) & 0xff) as u8)
}

fn put_array_byte(array: &mut TypeArrayOop, offset: usize, value: u8) -> io::Result<()> {
    let element_size = array.element_size();
    let index = offset / element_size;
    let shift = (offset % element_size) * 8;
    let mut element = element_u64(array, index)?;
    let mask = 0xffu64 << shift;
    element = (element & !mask) | (u64::from(value) << shift);
    put_element_u64(array, index, element)
}

fn read_array_raw(array: &TypeArrayOop, offset: i64, size: usize) -> io::Result<u64> {
    let mut result = 0u64;
    for index in 0..size {
        let address = offset
            .checked_add(index as i64)
            .ok_or_else(|| invalid("Unsafe address overflow"))?;
        let byte = usize::try_from(address).map_err(|_| invalid("Invalid Unsafe array offset"))?;
        result |= u64::from(array_byte(array, byte)?) << (index * 8);
    }
    Ok(result)
}

fn write_array_raw(array: &mut TypeArrayOop, offset: i64, size: usize, raw: u64) -> io::Result<()> {
    for index in 0..size {
        let address = offset
            .checked_add(index as i64)
            .ok_or_else(|| invalid("Unsafe address overflow"))?;
        let byte = usize::try_from(address).map_err(|_| invalid("Invalid Unsafe array offset"))?;
        put_array_byte(array, byte, (raw >> (index * 8)) as u8)?;
    }
    Ok(())
}

fn read_offheap_raw(address: i64, size: usize) -> io::Result<u64> {
    let base = u64::try_from(address).map_err(|_| invalid("Invalid Unsafe address"))?;
    let bytes = OffHeapMemory::global().read_bytes(base, size)?;
    let mut raw = [0; 8];
    raw[..size].copy_from_slice(&bytes);
    Ok(u64::from_le_bytes(raw))
}

fn write_offheap_raw(address: i64, size: usize, raw: u64) -> io::Result<()> {
    let base = u64::try_from(address).map_err(|_| invalid("Invalid Unsafe address"))?;
    OffHeapMemory::global().write_bytes(base, &raw.to_le_bytes()[..size])
}

fn find_static_field<'a>(
    loader: &ClassLoader<'a>,
    class: &[u8],
    offset: i64,
) -> io::Result<(Arc<crate::share::oops::instanceKlass::InstanceKlass<'a>>, usize, u8)> {
    let klass = loader.load(class)?;
    for (index, info) in klass.static_field_infos.iter().enumerate() {
        if i64::from(info.offset) == offset {
            let kind = info
                .descriptor
                .first()
                .copied()
                .ok_or_else(|| invalid("Invalid field descriptor"))?;
            return Ok((klass, index, kind));
        }
    }
    Err(invalid("Invalid Unsafe static field offset"))
}

fn read_raw<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    size: usize,
) -> io::Result<Raw> {
    let object = object_arg(loader, arguments, object_index)?;
    let offset = offset_arg(arguments, object_index + 1)?;
    if std::env::var_os("JVM25_TRACE").is_some() && matches!(object, Value::Null) {
        eprintln!("[unsafe-null] offset={offset} args={arguments:?}");
    }
    if matches!(object, Value::Null) && offset == THREAD_TID_SENTINEL_OFFSET {
        return Ok(Raw {
            value: THREAD_TID_COUNTER.load(Ordering::Relaxed) as u64,
        });
    }
    let raw = match object {
        Value::Null => read_offheap_raw(offset, size)?,
        Value::ClassMirror(class) => {
            let (klass, index, _) = find_static_field(loader, class, offset)?;
            let value = klass
                .static_fields
                .get(index)
                .map(|cell| cell.get())
                .ok_or_else(|| invalid("Invalid static field slot"))?;
            value_to_raw(value, size)?
        }
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            let object = objects
                .get(handle)
                .ok_or_else(|| invalid("Invalid object reference"))?;
            let (slot, _) = find_field(object, offset)?;
            let value = object
                .fields
                .get(slot)
                .copied()
                .ok_or_else(|| invalid("Invalid field slot"))?;
            value_to_raw(value, size)?
        }
        Value::PrimitiveArray(handle) => {
            let arrays = loader.arrays.borrow();
            let array = arrays
                .get(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            read_array_raw(array, offset, size)?
        }
        _ => return Err(invalid("Unsupported Unsafe object")),
    };
    Ok(Raw { value: raw })
}

fn write_raw<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    size: usize,
    value: Value<'static>,
) -> io::Result<()> {
    let object = object_arg(loader, arguments, object_index)?;
    let offset = offset_arg(arguments, object_index + 1)?;
    if matches!(object, Value::Null) && offset == THREAD_TID_SENTINEL_OFFSET {
        let raw = value_to_raw(value, size)?;
        THREAD_TID_COUNTER.store(raw as i64, Ordering::Relaxed);
        return Ok(());
    }
    let value_raw = value_to_raw(value, size)?;
    match object {
        Value::Null => write_offheap_raw(offset, size, value_raw),
        Value::ClassMirror(class) => {
            let (klass, index, kind) = find_static_field(loader, class, offset)?;
            let cell = klass
                .static_fields
                .get(index)
                .ok_or_else(|| invalid("Invalid static field slot"))?;
            let old = cell.get();
            let mut bytes = [0u8; 8];
            bytes[..8].copy_from_slice(&value_to_raw(old, 8)?.to_le_bytes());
            for index in 0..size {
                bytes[index] = (value_raw >> (index * 8)) as u8;
            }
            let merged = u64::from_le_bytes(bytes);
            cell.set(raw_to_value(kind, merged)?);
            Ok(())
        }
        Value::Object(handle) => {
            let mut objects = loader.borrow_objects_mut();
            let object = objects
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid object reference"))?;
            let (slot, kind) = find_field(object, offset)?;
            let old = object
                .fields
                .get(slot)
                .copied()
                .ok_or_else(|| invalid("Invalid field slot"))?;
            let mut bytes = [0u8; 8];
            bytes[..8].copy_from_slice(&value_to_raw(old, 8)?.to_le_bytes());
            for index in 0..size {
                bytes[index] = (value_raw >> (index * 8)) as u8;
            }
            let merged = u64::from_le_bytes(bytes);
            object.fields[slot] = raw_to_value(kind, merged)?;
            Ok(())
        }
        Value::PrimitiveArray(handle) => {
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            write_array_raw(array, offset, size, value_raw)
        }
        _ => Err(invalid("Unsupported Unsafe object")),
    }
}

fn read_reference<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
) -> io::Result<Value<'a>> {
    let object = object_arg(loader, arguments, object_index)?;
    let offset = offset_arg(arguments, object_index + 1)?;
    match object {
        Value::ClassMirror(class) => {
            let (klass, index, kind) = find_static_field(loader, class, offset)?;
            if !matches!(kind, b'L' | b'[') {
                return Err(invalid("Unsafe field is not a reference"));
            }
            klass
                .static_fields
                .get(index)
                .map(|cell| cell.get())
                .ok_or_else(|| invalid("Invalid static field slot"))
        }
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            let object = objects
                .get(handle)
                .ok_or_else(|| invalid("Invalid object reference"))?;
            let (slot, kind) = find_field(object, offset)?;
            if !matches!(kind, b'L' | b'[') {
                return Err(invalid("Unsafe field is not a reference"));
            }
            object
                .fields
                .get(slot)
                .copied()
                .ok_or_else(|| invalid("Invalid field slot"))
        }
        Value::ObjectArray(handle) => {
            let arrays = loader.obj_arrays.borrow();
            let array = arrays
                .get(handle)
                .ok_or_else(|| invalid("Invalid object array reference"))?;
            let index =
                usize::try_from(offset / 8).map_err(|_| invalid("Invalid Unsafe array offset"))?;
            array.get(index)
        }
        _ => Err(invalid("Unsupported Unsafe reference object")),
    }
}

fn write_reference<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    value: Value<'a>,
) -> io::Result<()> {
    let object = object_arg(loader, arguments, object_index)?;
    let offset = offset_arg(arguments, object_index + 1)?;
    match object {
        Value::ClassMirror(class) => {
            let (klass, index, kind) = find_static_field(loader, class, offset)?;
            if !matches!(kind, b'L' | b'[') {
                return Err(invalid("Unsafe field is not a reference"));
            }
            let cell = klass
                .static_fields
                .get(index)
                .ok_or_else(|| invalid("Invalid static field slot"))?;
            cell.set(value);
            Ok(())
        }
        Value::Object(handle) => {
            let mut objects = loader.borrow_objects_mut();
            let object = objects
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid object reference"))?;
            let (slot, kind) = find_field(object, offset)?;
            if !matches!(kind, b'L' | b'[') {
                return Err(invalid("Unsafe field is not a reference"));
            }
            if slot >= object.fields.len() {
                return Err(invalid("Invalid field slot"));
            }
            object.fields[slot] = value;
            Ok(())
        }
        Value::ObjectArray(handle) => {
            let mut arrays = loader.obj_arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid object array reference"))?;
            let index =
                usize::try_from(offset / 8).map_err(|_| invalid("Invalid Unsafe array offset"))?;
            array.set(index, value)
        }
        _ => Err(invalid("Unsupported Unsafe reference object")),
    }
}

fn compare_exchange_reference<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    expected: Value<'a>,
    value: Value<'a>,
    _acquire: bool,
) -> io::Result<bool> {
    let old = read_reference(loader, arguments, object_index)?;
    if old != expected {
        return Ok(false);
    }
    write_reference_with_value(loader, arguments, object_index, value)?;
    Ok(true)
}

fn compare_exchange_reference_value<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    expected: Value<'a>,
    value: Value<'a>,
) -> io::Result<Value<'a>> {
    let old = read_reference(loader, arguments, object_index)?;
    if old == expected {
        write_reference_with_value(loader, arguments, object_index, value)?;
    }
    Ok(match old {
        Value::Int(value) => Value::Int(value),
        Value::Long(value) => Value::Long(value),
        Value::Float(value) => Value::Float(value),
        Value::Double(value) => Value::Double(value),
        Value::String(value) => Value::String(value),
        Value::ClassMirror(value) => Value::ClassMirror(value),
        Value::HostString(value) => Value::HostString(value),
        Value::Arguments(value) => Value::Arguments(value),
        Value::PrimitiveArray(value) => Value::PrimitiveArray(value),
        Value::Object(value) => Value::Object(value),
        Value::ObjectArray(value) => Value::ObjectArray(value),
        Value::ReturnAddress(value) => Value::ReturnAddress(value),
        Value::Null => Value::Null,
        Value::Output => Value::Output,
        Value::Empty => Value::Empty,
    })
}

fn write_reference_with_value<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    value: Value<'a>,
) -> io::Result<()> {
    let object = object_arg(loader, arguments, object_index)?;
    let offset = offset_arg(arguments, object_index + 1)?;
    match object {
        Value::ClassMirror(class) => {
            let (klass, index, kind) = find_static_field(loader, class, offset)?;
            if !matches!(kind, b'L' | b'[') {
                return Err(invalid("Unsafe field is not a reference"));
            }
            let cell = klass
                .static_fields
                .get(index)
                .ok_or_else(|| invalid("Invalid static field slot"))?;
            cell.set(value);
            Ok(())
        }
        Value::Object(handle) => {
            let mut objects = loader.borrow_objects_mut();
            let object = objects
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid object reference"))?;
            let (slot, kind) = find_field(object, offset)?;
            if !matches!(kind, b'L' | b'[') {
                return Err(invalid("Unsafe field is not a reference"));
            }
            object.fields[slot] = value;
            Ok(())
        }
        Value::ObjectArray(handle) => {
            let mut arrays = loader.obj_arrays.borrow_mut();
            let index =
                usize::try_from(offset / 8).map_err(|_| invalid("Invalid Unsafe array offset"))?;
            arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid object array reference"))?
                .set(index, value)
        }
        _ => Err(invalid("Unsupported Unsafe reference object")),
    }
}

fn compare_exchange_int<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    expected: i32,
    value: i32,
    _acquire: bool,
) -> io::Result<bool> {
    let old = read_raw(loader, arguments, object_index, 4)?.int()?;
    if old != expected {
        return Ok(false);
    }
    write_raw(loader, arguments, object_index, 4, Value::Int(value))?;
    Ok(true)
}

fn compare_exchange_int_value<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    expected: i32,
    value: i32,
) -> io::Result<i32> {
    let old = read_raw(loader, arguments, object_index, 4)?.int()?;
    if old == expected {
        write_raw(loader, arguments, object_index, 4, Value::Int(value))?;
    }
    Ok(old)
}

fn compare_exchange_long<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    expected: i64,
    value: i64,
    _acquire: bool,
) -> io::Result<bool> {
    let old = read_raw(loader, arguments, object_index, 8)?.long()?;
    if old != expected {
        return Ok(false);
    }
    write_raw(loader, arguments, object_index, 8, Value::Long(value))?;
    Ok(true)
}

fn get_and_add<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    size: usize,
    delta: u64,
    wide: bool,
) -> io::Result<u64> {
    let mask = if size == 8 {
        u64::MAX
    } else {
        (1u64 << (size * 8)) - 1
    };
    let old = read_raw(loader, arguments, object_index, size)?.value & mask;
    let new = old.wrapping_add(delta) & mask;
    let value = if wide {
        Value::Long(new as i64)
    } else {
        Value::Int(new as u32 as i32)
    };
    write_raw(loader, arguments, object_index, size, value)?;
    Ok(old)
}

#[derive(Clone, Copy)]
enum BitwiseKind {
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
}

#[derive(Clone, Copy)]
enum BitwiseOp {
    And,
    Or,
    Xor,
}

fn bitwise_access(name: &[u8], descriptor: &[u8]) -> Option<(BitwiseKind, BitwiseOp)> {
    let rest = name.strip_prefix(b"getAndBitwise")?;
    let (op, rest) = if let Some(rest) = rest.strip_prefix(b"And") {
        (BitwiseOp::And, rest)
    } else if let Some(rest) = rest.strip_prefix(b"Or") {
        (BitwiseOp::Or, rest)
    } else {
        (BitwiseOp::Xor, rest.strip_prefix(b"Xor")?)
    };
    let kind_name = rest
        .strip_suffix(b"Release")
        .or_else(|| rest.strip_suffix(b"Acquire"))
        .unwrap_or(rest);
    let kind = match (kind_name, descriptor) {
        (b"Boolean", b"(Ljava/lang/Object;JZ)Z") => BitwiseKind::Boolean,
        (b"Byte", b"(Ljava/lang/Object;JB)B") => BitwiseKind::Byte,
        (b"Char", b"(Ljava/lang/Object;JC)C") => BitwiseKind::Char,
        (b"Short", b"(Ljava/lang/Object;JS)S") => BitwiseKind::Short,
        (b"Int", b"(Ljava/lang/Object;JI)I") => BitwiseKind::Int,
        (b"Long", b"(Ljava/lang/Object;JJ)J") => BitwiseKind::Long,
        (b"Float", b"(Ljava/lang/Object;JF)F") => BitwiseKind::Float,
        (b"Double", b"(Ljava/lang/Object;JD)D") => BitwiseKind::Double,
        _ => return None,
    };
    Some((kind, op))
}

fn get_and_bitwise<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    size: usize,
    mask: u64,
    wide: bool,
    op: BitwiseOp,
) -> io::Result<u64> {
    let bits = if size == 8 {
        u64::MAX
    } else {
        (1u64 << (size * 8)) - 1
    };
    let old = read_raw(loader, arguments, object_index, size)?.value & bits;
    let new = match op {
        BitwiseOp::And => old & mask,
        BitwiseOp::Or => old | mask,
        BitwiseOp::Xor => old ^ mask,
    } & bits;
    let value = if wide {
        Value::Long(new as i64)
    } else {
        Value::Int(new as u32 as i32)
    };
    write_raw(loader, arguments, object_index, size, value)?;
    Ok(old)
}

fn get_and_bitwise_typed<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    kind: BitwiseKind,
    op: BitwiseOp,
) -> io::Result<Value<'static>> {
    let (size, wide, mask) = match kind {
        BitwiseKind::Boolean => (1usize, false, u64::from(bool_arg(arguments, 3)?)),
        BitwiseKind::Byte => (1, false, u64::from(byte_arg(arguments, 3)? as u8)),
        BitwiseKind::Char => (2, false, u64::from(char_arg(arguments, 3)?)),
        BitwiseKind::Short => (2, false, u64::from(short_arg(arguments, 3)? as u16)),
        BitwiseKind::Int => (4, false, u64::from(int_arg(arguments, 3)? as u32)),
        BitwiseKind::Long => (8, true, long_arg(arguments, 3)? as u64),
        BitwiseKind::Float => (4, false, u64::from(float_arg(arguments, 3)?.to_bits())),
        BitwiseKind::Double => (8, true, double_arg(arguments, 3)?.to_bits()),
    };
    let old = get_and_bitwise(loader, arguments, 1, size, mask, wide, op)?;
    Ok(match kind {
        BitwiseKind::Boolean => Value::Int(i32::from(old & 1 != 0)),
        BitwiseKind::Byte => Value::Int(i32::from(old as u8 as i8)),
        BitwiseKind::Char => Value::Int(i32::from(old as u16)),
        BitwiseKind::Short => Value::Int(i32::from(old as u16 as i16)),
        BitwiseKind::Int => Value::Int(old as u32 as i32),
        BitwiseKind::Long => Value::Long(old as i64),
        BitwiseKind::Float => Value::Float(f32::from_bits(old as u32)),
        BitwiseKind::Double => Value::Double(f64::from_bits(old)),
    })
}

fn compare_exchange_long_value<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    object_index: usize,
    expected: i64,
    value: i64,
) -> io::Result<i64> {
    let old = read_raw(loader, arguments, object_index, 8)?.long()?;
    if old == expected {
        write_raw(loader, arguments, object_index, 8, Value::Long(value))?;
    }
    Ok(old)
}

fn field_offset_by_name<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    class_index: usize,
    name_index: usize,
) -> io::Result<i64> {
    let class = class_arg(arguments, class_index)?;
    let name = string_arg(loader, argument(arguments, name_index)?)?;
    let klass = loader.load(class)?;
    let name = name.as_bytes();
    let mut current = Some(klass.as_ref());
    while let Some(owner) = current {
        for field in &owner.instance_field_infos {
            if field.name == name {
                return Ok(i64::from(field.offset));
            }
        }
        current = owner.super_klass.as_deref();
    }
    Err(invalid("NoSuchFieldException"))
}

fn field_member<'a>(
    loader: &ClassLoader<'a>,
    field: Value<'a>,
) -> io::Result<(Option<&'a [u8]>, Option<String>, Option<i32>)> {
    let Value::Object(handle) = field else {
        return Err(invalid("Expected java.lang.reflect.Field"));
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid field object"))?;
    if object.klass.class.name != b"java/lang/reflect/Field" {
        return Err(invalid("Expected java.lang.reflect.Field"));
    }
    let clazz_index = object
        .klass
        .instanceField(b"clazz", b"Ljava/lang/Class;")
        .map_err(|_| invalid("Invalid Field.clazz layout"))?;
    let name_index = object
        .klass
        .instanceField(b"name", b"Ljava/lang/String;")
        .map_err(|_| invalid("Invalid Field.name layout"))?;
    let slot_index = object
        .klass
        .instanceField(b"slot", b"I")
        .map_err(|_| invalid("Invalid Field.slot layout"))?;
    let clazz = object.fields.get(clazz_index).copied();
    let name = object.fields.get(name_index).copied();
    let slot = object.fields.get(slot_index).copied();
    drop(objects);

    let class = match clazz {
        Some(Value::ClassMirror(name)) => Some(name),
        _ => None,
    };
    let field_name = match name {
        Some(value) => Some(string_arg(loader, value)?),
        None => None,
    };
    let slot = match slot {
        Some(Value::Int(value)) => Some(value),
        _ => None,
    };
    Ok((class, field_name, slot))
}

fn field_offset_from_field<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    field_index: usize,
) -> io::Result<i64> {
    let (class, name, slot) = field_member(loader, argument(arguments, field_index)?)?;
    if let (Some(class), Some(name)) = (class, name) {
        let klass = loader.load(class)?;
        let bytes = name.as_bytes();
        let mut current = Some(klass.as_ref());
        while let Some(owner) = current {
            for field in &owner.instance_field_infos {
                if field.name == bytes {
                    return Ok(i64::from(field.offset));
                }
            }
            current = owner.super_klass.as_deref();
        }
    }
    slot.map(i64::from)
        .ok_or_else(|| invalid("Invalid java.lang.reflect.Field"))
}

fn static_field_offset_from_field<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    field_index: usize,
) -> io::Result<i64> {
    let (class, name, slot) = field_member(loader, argument(arguments, field_index)?)?;
    if let (Some(class), Some(name)) = (class, name) {
        let klass = loader.load(class)?;
        let bytes = name.as_bytes();
        let mut current = Some(klass.as_ref());
        while let Some(owner) = current {
            for field in &owner.static_field_infos {
                if field.name == bytes {
                    return Ok(i64::from(field.offset));
                }
            }
            current = owner.super_klass.as_deref();
        }
    }
    slot.map(i64::from)
        .ok_or_else(|| invalid("Invalid java.lang.reflect.Field"))
}

fn static_field_base_from_field<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    field_index: usize,
) -> io::Result<Value<'a>> {
    let (class, _, _) = field_member(loader, argument(arguments, field_index)?)?;
    class
        .map(|name| Value::ClassMirror(name))
        .ok_or_else(|| invalid("Invalid java.lang.reflect.Field"))
}

fn class_initialized(loader: &ClassLoader<'_>, class: &[u8]) -> io::Result<bool> {
    let klass = loader.load(class)?;
    Ok(matches!(
        klass.state.get(),
        InitState::BeingInitialized | InitState::FullyInitialized
    ))
}

fn ensure_class_initialized(loader: &ClassLoader<'_>, class: &[u8]) -> io::Result<()> {
    let klass = loader.load(class)?;
    crate::share::interpreter::bytecodeInterpreter::initialize(
        loader,
        klass.as_ref(),
        &mut io::sink(),
        0,
    )
}

fn allocate_instance<'a>(loader: &ClassLoader<'a>, class: &[u8]) -> io::Result<Value<'a>> {
    let klass = loader.load(class)?;
    if klass.is_interface() || klass.is_abstract() {
        return Err(invalid("InstantiationException"));
    }
    let object = InstanceOop::new(klass)?;
    Ok(Value::Object(loader.alloc_object(object)))
}

fn write_bytes<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    offset: i64,
    bytes: usize,
    value: u8,
) -> io::Result<()> {
    match object {
        Value::Null => {
            let address = u64::try_from(offset).map_err(|_| invalid("Invalid Unsafe address"))?;
            OffHeapMemory::global().set_memory(address, bytes, value)
        }
        Value::PrimitiveArray(handle) => {
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            for index in 0..bytes {
                let address = offset
                    .checked_add(index as i64)
                    .ok_or_else(|| invalid("Unsafe address overflow"))?;
                put_array_byte(
                    array,
                    usize::try_from(address).map_err(|_| invalid("Invalid Unsafe array offset"))?,
                    value,
                )?;
            }
            Ok(())
        }
        _ => Err(invalid("Unsupported Unsafe setMemory object")),
    }
}

fn copy_bytes<'a>(
    loader: &ClassLoader<'a>,
    src_object: Value<'a>,
    src_offset: i64,
    dst_object: Value<'a>,
    dst_offset: i64,
    bytes: usize,
) -> io::Result<()> {
    if bytes == 0 {
        return Ok(());
    }
    let buffer = if src_object == Value::Null {
        OffHeapMemory::global().read_bytes(u64::try_from(src_offset).map_err(io::Error::other)?, bytes)?
    } else {
        let mut buffer = vec![0; bytes];
        for (index, byte) in buffer.iter_mut().enumerate() {
            let address = src_offset.checked_add(index as i64).ok_or_else(|| invalid("Unsafe address overflow"))?;
            *byte = read_byte(loader, src_object, address)?;
        }
        buffer
    };
    if dst_object == Value::Null {
        return OffHeapMemory::global().write_bytes(u64::try_from(dst_offset).map_err(io::Error::other)?, &buffer);
    }
    for (index, byte) in buffer.into_iter().enumerate() {
        let address = dst_offset
            .checked_add(index as i64)
            .ok_or_else(|| invalid("Unsafe address overflow"))?;
        write_byte(loader, dst_object, address, byte)?;
    }
    Ok(())
}

/// `Unsafe.copySwapMemory0` with a nonzero element size: copies `bytes`
/// while reversing each `elem_size`-byte block (HotSpot `copy_swap_memory`).
fn copy_swap_bytes<'a>(
    loader: &ClassLoader<'a>,
    src_object: Value<'a>,
    src_offset: i64,
    dst_object: Value<'a>,
    dst_offset: i64,
    bytes: usize,
    elem_size: usize,
) -> io::Result<()> {
    if elem_size == 0 || bytes % elem_size != 0 {
        return Err(invalid("Invalid Unsafe swap size"));
    }
    if bytes == 0 {
        return Ok(());
    }
    let mut buffer = vec![0u8; bytes];
    for index in 0..bytes {
        let address = src_offset
            .checked_add(index as i64)
            .ok_or_else(|| invalid("Unsafe address overflow"))?;
        buffer[index] = read_byte(loader, src_object, address)?;
    }
    for chunk in buffer.chunks_exact_mut(elem_size) {
        chunk.reverse();
    }
    for (index, byte) in buffer.into_iter().enumerate() {
        let address = dst_offset
            .checked_add(index as i64)
            .ok_or_else(|| invalid("Unsafe address overflow"))?;
        write_byte(loader, dst_object, address, byte)?;
    }
    Ok(())
}

fn read_byte(loader: &ClassLoader<'_>, object: Value<'_>, offset: i64) -> io::Result<u8> {
    match object {
        Value::Null => {
            let address = u64::try_from(offset).map_err(|_| invalid("Invalid Unsafe address"))?;
            Ok(OffHeapMemory::global().get_byte(address)? as u8)
        }
        Value::PrimitiveArray(handle) => {
            let arrays = loader.arrays.borrow();
            let array = arrays
                .get(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            array_byte(
                array,
                usize::try_from(offset).map_err(|_| invalid("Invalid Unsafe array offset"))?,
            )
        }
        _ => Err(invalid("Unsupported Unsafe copy object")),
    }
}

fn define_class0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let expected_name = match arguments.get(0).copied().unwrap_or(Value::Null) {
        Value::Null => None,
        value => Some(string_arg(loader, value)?.replace('.', "/")),
    };
    let bytes_value = arguments.get(1).copied().unwrap_or(Value::Null);
    let Value::PrimitiveArray(handle) = bytes_value else {
        return Err(invalid("defineClass0: expected byte[]"));
    };
    let offset = usize::try_from(i64::from(int_arg(arguments, 2)?))
        .map_err(|_| invalid("defineClass0: negative offset"))?;
    let length = usize::try_from(i64::from(int_arg(arguments, 3)?))
        .map_err(|_| invalid("defineClass0: negative length"))?;
    let window: Vec<u8> = {
        let arrays = loader.arrays.borrow();
        let Some(TypeArrayOop::Byte(bytes)) = arrays.get(handle) else {
            return Err(invalid("defineClass0: expected byte[]"));
        };
        if offset > bytes.len() || length > bytes.len() - offset {
            return Err(invalid("defineClass0: offset/length out of bounds"));
        }
        bytes[offset..offset + length]
            .iter()
            .map(|&b| b as u8)
            .collect()
    };
    let owned = loader.storage.alloc_slice_copy(&window);
    let class = ClassFile::parse(owned, false).map_err(|error| {
        LinkageError::new(LinkageErrorKind::ClassFormat, error.to_string()).io()
    })?;
    let name = class.name;
    if let Some(expected) = expected_name
        && expected.as_bytes() != name
    {
        return Err(LinkageError::new(
            LinkageErrorKind::NoClassDefFound,
            format!(
                "{} (wrong name: {})",
                String::from_utf8_lossy(name),
                expected
            ),
        )
        .io());
    }
    loader.define_class(class)?;
    Ok(Value::ClassMirror(name))
}

fn write_byte(
    loader: &ClassLoader<'_>,
    object: Value<'_>,
    offset: i64,
    value: u8,
) -> io::Result<()> {
    match object {
        Value::Null => {
            let address = u64::try_from(offset).map_err(|_| invalid("Invalid Unsafe address"))?;
            OffHeapMemory::global().put_byte(address, value as i8)
        }
        Value::PrimitiveArray(handle) => {
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            put_array_byte(
                array,
                usize::try_from(offset).map_err(|_| invalid("Invalid Unsafe array offset"))?,
                value,
            )
        }
        _ => Err(invalid("Unsupported Unsafe copy object")),
    }
}
