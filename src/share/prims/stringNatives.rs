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
            let site = || {
                crate::share::interpreter::bytecodeInterpreter::current_frame_site()
            };
            let value_index = object
                .klass
                .instanceField(b"value", b"[B")
                .map_err(|error| Error::other(format!("{error} site={}", site())))?;
            let field = object.fields[value_index];
            let Value::PrimitiveArray(handle) = field else {
                return Err(Error::other(format!(
                    "Invalid java.lang.String storage: value={field:?} site={}",
                    site()
                )));
            };
            let coder_index = object
                .klass
                .instanceField(b"coder", b"B")
                .map_err(|error| Error::other(format!("{error} site={}", site())))?;
            let coder = object.fields[coder_index].int().map_err(|error| {
                Error::other(format!(
                    "Invalid java.lang.String coder: {error} site={}",
                    site()
                ))
            })?;
            let arrays = loader.arrays.borrow();
            let Some(TypeArrayOop::Byte(bytes)) = arrays.get(handle) else {
                return Err(Error::other(format!(
                    "Invalid java.lang.String storage: array={handle} site={}",
                    site()
                )));
            };
            let text = match coder {
                0 => bytes.iter().map(|&byte| char::from(byte as u8)).collect(),
                1 if bytes.len() % 2 == 0 => {
                    // StringUTF16 stores chars in platform byte order; on
                    // little-endian hosts (x86, AArch64) that is LE.
                    let units = bytes
                        .chunks_exact(2)
                        .map(|pair| u16::from_le_bytes([pair[0] as u8, pair[1] as u8]));
                    char::decode_utf16(units)
                        .collect::<Result<String, _>>()
                        .map_err(|error| Error::new(io::ErrorKind::InvalidData, error))?
                }
                _ => {
                    let dump = object
                        .fields
                        .iter()
                        .enumerate()
                        .map(|(i, f)| format!("{i}={f:?}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    eprintln!("[bad-coder] coder={coder} coder_index={coder_index} value_index={value_index} fields=[{dump}] klass={}", std::str::from_utf8(&object.klass.class.name).unwrap_or("?"));
                    return Err(Error::other(format!(
                        "Invalid java.lang.String coder: {coder} site={}",
                        site()
                    )));
                }
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
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Option<Value<'a>>> {
    let literal = match value {
        Value::String(bytes) => Some(bytes),
        _ => None,
    };
    if let Some(bytes) = literal
        && let Some(handle) = loader.stringLiterals.borrow().get(bytes).copied()
    {
        return Ok(Some(Value::Object(handle)));
    }

    let units = match value {
        Value::String(bytes) => crate::classfile::decodeUtf16Units(bytes)?,
        Value::HostString(text) => text.encode_utf16().collect(),
        _ => return Ok(None),
    };

    let klass = loader.load(b"java/lang/String")?;
    crate::share::interpreter::bytecodeInterpreter::initialize(loader, &klass, output, depth + 1)?;

    let latin1 = units.iter().all(|&unit| unit <= 0xff);
    let bytes = if latin1 {
        units.into_iter().map(|unit| unit as i8).collect()
    } else {
        units.into_iter().flat_map(u16::to_ne_bytes).map(|byte| byte as i8).collect()
    };

    let array_handle = loader.alloc_type_array(TypeArrayOop::Byte(bytes));

    let mut object = InstanceOop::new(klass)?;
    let value_index = object.klass.instanceField(b"value", b"[B")?;
    let coder_index = object.klass.instanceField(b"coder", b"B")?;
    object.fields[value_index] = Value::PrimitiveArray(array_handle);
    object.fields[coder_index] = Value::Int(i32::from(!latin1));
    if let Ok(hash_index) = object.klass.instanceField(b"hash", b"I") {
        object.fields[hash_index] = Value::Int(0);
    }

    let candidate = loader.alloc_object(object);
    let handle = if let Some(bytes) = literal {
        *loader.stringLiterals.borrow_mut().entry(bytes).or_insert_with(|| {
            loader.gc_extra_roots.borrow_mut().push(Value::Object(candidate));
            candidate
        })
    } else {
        candidate
    };
    Ok(Some(Value::Object(handle)))
}

pub(crate) fn invokeVirtual<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    match owner {
        b"java/lang/String" | b"java/lang/CharSequence" | b"java/lang/Object" => {
            invoke(loader, name, descriptor, arguments)
        }
        _ => Ok(None),
    }
}

pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    if !matches!(
        name,
        b"startsWith" | b"endsWith" | b"getChars" | b"length" | b"isEmpty" | b"charAt"
            | b"hashCode" | b"equals" | b"toString" | b"intern"
    ) {
        return Ok(None);
    }
    let Some((&receiver, args)) = args.split_first() else {
        return Ok(None);
    };
    let text = match string(loader, receiver) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok(None),
        Err(error) => return Err(error),
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
        (b"length", b"()I", []) => {
            if std::env::var("JVM25_DEBUG_SIG").is_ok() && text.starts_with('<') && text.len() > 10 {
                eprintln!("[strlen] len={} text={:?}", text.encode_utf16().count(), &text[..text.len().min(60)]);
            }
            Value::Int(text.encode_utf16().count() as i32)
        }
        (b"isEmpty", b"()Z", []) => Value::Int(i32::from(text.is_empty())),
        (b"charAt", b"(I)C", [Value::Int(index)]) => {
            if std::env::var("JVM25_DEBUG_SIG").is_ok() && *index == 0 && text.starts_with('<') && text.len() > 10 {
                eprintln!("[charAt0] text={:?}", &text[..text.len().min(60)]);
            }
            Value::Int(
                text.encode_utf16()
                    .nth(*index as usize)
                    .map(i32::from)
                    .ok_or_else(|| Error::other("StringIndexOutOfBoundsException"))?,
            )
        }
        (b"hashCode", b"()I", []) => Value::Int(text.encode_utf16().fold(0i32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(i32::from(unit))
        })),
        (b"equals", b"(Ljava/lang/Object;)Z", [other]) => Value::Int(i32::from(
            string(loader, *other)?.as_deref() == Some(text.as_ref()),
        )),
        (b"toString", b"()Ljava/lang/String;", []) => receiver,
        // Canonicalization is approximated: lightweight strings already
        // compare by content, and materialized receivers keep their identity.
        (b"intern", b"()Ljava/lang/String;", []) => receiver,
        _ => return Ok(None),
    };
    Ok(Some(Some(result)))
}
