use crate::classfile::decodeString;
use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    oops::{instanceOop::InstanceOop, typeArrayOop::TypeArrayOop},
};
use std::{
    borrow::Cow,
    io::{self, Error, Write},
};

pub(crate) fn string<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
) -> io::Result<Option<Cow<'a, str>>> {
    match value {
        Value::String(bytes) => decodeString(bytes).map(|value| Some(Cow::Owned(value))),
        Value::HostString(value) => Ok(Some(Cow::Borrowed(value))),
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            let object = objects
                .get(handle)
                .ok_or_else(|| Error::other("Invalid object reference"))?;
            if object.klass.class.name != b"java/lang/String" {
                return Ok(None);
            }
            let invalid = || Error::other("Invalid java.lang.String storage");
            let Value::PrimitiveArray(handle) =
                object.fields[object.klass.instanceField(b"value", b"[B")?]
            else {
                return Err(invalid());
            };
            let coder = object.fields[object.klass.instanceField(b"coder", b"B")?].int()?;
            let arrays = loader.arrays.borrow();
            let Some(TypeArrayOop::Byte(bytes)) = arrays.get(handle) else {
                return Err(invalid());
            };
            let text = match coder {
                0 => bytes.iter().map(|&byte| char::from(byte as u8)).collect(),
                1 if bytes.len() % 2 == 0 => {
                    let units = bytes
                        .chunks_exact(2)
                        .map(|pair| u16::from_be_bytes([pair[0] as u8, pair[1] as u8]));
                    char::decode_utf16(units)
                        .collect::<Result<String, _>>()
                        .map_err(Error::other)?
                }
                _ => return Err(invalid()),
            };
            Ok(Some(Cow::Owned(text)))
        }
        _ => Ok(None),
    }
}

/// Materializes a lightweight string value (`Value::String` / `Value::HostString`)
/// into a real `java.lang.String` heap object so that JDK bytecode can access
/// its fields (`value`, `coder`, `hash`) and invoke its methods.
pub(crate) fn materialize<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    output: &mut impl Write,
    depth: usize,
) -> io::Result<Option<Value<'a>>> {
    if matches!(
        value,
        Value::Object(_)
            | Value::ObjectArray(_)
            | Value::PrimitiveArray(_)
            | Value::Arguments(_)
            | Value::ClassMirror(_)
            | Value::Null
            | Value::Empty
            | Value::Output
            | Value::ReturnAddress(_)
            | Value::Int(_)
            | Value::Long(_)
            | Value::Float(_)
            | Value::Double(_)
    ) {
        return Ok(None);
    }
    let Some(text) = string(loader, value)? else {
        return Ok(None);
    };
    let text = text.into_owned();

    let klass = loader.load(b"java/lang/String")?;
    crate::share::interpreter::bytecodeInterpreter::initialize(loader, &klass, output, depth + 1)?;

    let latin1 = text.chars().all(|c| u32::from(c) <= 0xff);
    let bytes: Vec<u8> = if latin1 {
        text.chars().map(|c| c as u32 as u8).collect()
    } else {
        let mut bytes = Vec::with_capacity(text.encode_utf16().count() * 2);
        for unit in text.encode_utf16() {
            bytes.push((unit >> 8) as u8);
            bytes.push(unit as u8);
        }
        bytes
    };

    let array_handle = loader.alloc_type_array(TypeArrayOop::Byte(
        bytes.into_iter().map(|b| b as i8).collect(),
    ));

    let mut object = InstanceOop::new(klass)?;
    let value_index = object.klass.instanceField(b"value", b"[B")?;
    let coder_index = object.klass.instanceField(b"coder", b"B")?;
    object.fields[value_index] = Value::PrimitiveArray(array_handle);
    object.fields[coder_index] = Value::Int(i32::from(!latin1));
    if let Ok(hash_index) = object.klass.instanceField(b"hash", b"I") {
        object.fields[hash_index] = Value::Int(0);
    }

    Ok(Some(Value::Object(loader.alloc_object(object))))
}

pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    let Some((&receiver, args)) = args.split_first() else {
        return Ok(None);
    };
    let Some(text) = string(loader, receiver)? else {
        return Ok(None);
    };
    let result = match (name, descriptor, args) {
        (b"startsWith" | b"endsWith", b"(Ljava/lang/String;)Z", [needle]) => {
            let needle = string(loader, *needle)?.ok_or_else(|| {
                Error::other(if matches!(needle, Value::Null) {
                    "NullPointerException"
                } else {
                    "Expected string argument"
                })
            })?;
            Value::Int(i32::from(if name == b"startsWith" {
                text.starts_with(needle.as_ref())
            } else {
                text.ends_with(needle.as_ref())
            }))
        }
        (
            b"getChars",
            b"(II[CI)V",
            [
                Value::Int(start),
                Value::Int(end),
                array,
                Value::Int(offset),
            ],
        ) => {
            let bounds = || Error::other("StringIndexOutOfBoundsException");
            let start = usize::try_from(*start).map_err(|_| bounds())?;
            let end = usize::try_from(*end).map_err(|_| bounds())?;
            let length = end
                .checked_sub(start)
                .filter(|_| end <= text.encode_utf16().count())
                .ok_or_else(bounds)?;
            let Value::PrimitiveArray(handle) = array else {
                return Err(Error::other(if matches!(array, Value::Null) {
                    "NullPointerException"
                } else {
                    "Expected char array"
                }));
            };
            let mut arrays = loader.arrays.borrow_mut();
            let Some(TypeArrayOop::Char(destination)) = arrays.get_mut(*handle) else {
                return Err(Error::other("Expected char array"));
            };
            let offset = usize::try_from(*offset).map_err(|_| bounds())?;
            let limit = offset.checked_add(length).ok_or_else(bounds)?;
            let destination = destination.get_mut(offset..limit).ok_or_else(bounds)?;
            for (slot, code) in destination.iter_mut().zip(text.encode_utf16().skip(start)) {
                *slot = code;
            }
            return Ok(Some(None));
        }
        (b"length", b"()I", []) => Value::Int(text.encode_utf16().count() as i32),
        (b"isEmpty", b"()Z", []) => Value::Int(i32::from(text.is_empty())),
        (b"charAt", b"(I)C", [Value::Int(index)]) => Value::Int(
            text.encode_utf16()
                .nth(*index as usize)
                .map(i32::from)
                .ok_or_else(|| Error::other("StringIndexOutOfBoundsException"))?,
        ),
        (b"hashCode", b"()I", []) => Value::Int(text.encode_utf16().fold(0i32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(i32::from(unit))
        })),
        (b"equals", b"(Ljava/lang/Object;)Z", [other]) => Value::Int(i32::from(
            string(loader, *other)?.as_deref() == Some(text.as_ref()),
        )),
        (b"toString", b"()Ljava/lang/String;", []) => receiver,
        _ => return Ok(None),
    };
    Ok(Some(Some(result)))
}
