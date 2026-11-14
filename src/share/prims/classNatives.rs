use crate::share::{
    classfile::{classFileParser::decodeString, classLoader::ClassLoader},
    interpreter::bytecodeInterpreter::Value,
};
use std::io;

fn primitiveName(tag: &[u8]) -> Option<&'static str> {
    match tag {
        b"Z" => Some("boolean"),
        b"B" => Some("byte"),
        b"C" => Some("char"),
        b"S" => Some("short"),
        b"I" => Some("int"),
        b"J" => Some("long"),
        b"F" => Some("float"),
        b"D" => Some("double"),
        b"V" => Some("void"),
        _ => None,
    }
}

pub(crate) fn primitiveClass(value: Value<'_>) -> io::Result<Value<'_>> {
    let name = match value {
        Value::HostString(name) => std::borrow::Cow::Borrowed(name),
        Value::String(bytes) => std::borrow::Cow::Owned(decodeString(bytes)?),
        _ => return Err(io::Error::other("Expected primitive name")),
    };
    let tag: &'static [u8] = match name.as_ref() {
        "boolean" => b"Z",
        "byte" => b"B",
        "char" => b"C",
        "short" => b"S",
        "int" => b"I",
        "long" => b"J",
        "float" => b"F",
        "double" => b"D",
        "void" => b"V",
        _ => return Err(io::Error::other(format!("Unknown primitive type {name}"))),
    };
    Ok(Value::ClassMirror(match tag {
        b"Z" => b":Z",
        b"B" => b":B",
        b"C" => b":C",
        b"S" => b":S",
        b"I" => b":I",
        b"J" => b":J",
        b"F" => b":F",
        b"D" => b":D",
        _ => b":V",
    }))
}

pub(crate) fn classOf<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Value<'a>> {
    let invalid = || io::Error::other("Invalid getClass receiver");
    let name: &'a [u8] = match value {
        Value::Object(handle) => {
            loader
                .objects
                .borrow()
                .get(handle)
                .ok_or_else(invalid)?
                .klass
                .class
                .name
        }
        Value::String(_) | Value::HostString(_) => b"java/lang/String",
        Value::Arguments(_) => b"[Ljava/lang/String;",
        Value::ClassMirror(_) => b"java/lang/Class",
        Value::Output => b"java/io/PrintStream",
        Value::PrimitiveArray(handle) => loader
            .arrays
            .borrow()
            .get(handle)
            .ok_or_else(invalid)?
            .descriptor(),
        Value::ObjectArray(handle) => {
            let component = loader
                .obj_arrays
                .borrow()
                .get(handle)
                .ok_or_else(invalid)?
                .component;
            let mut descriptor = Vec::with_capacity(component.len() + 3);
            descriptor.push(b'[');
            if component.starts_with(b"[") {
                descriptor.extend_from_slice(component);
            } else {
                descriptor.push(b'L');
                descriptor.extend_from_slice(component);
                descriptor.push(b';');
            }
            loader.storage.alloc_slice_copy(&descriptor)
        }
        Value::Null => return Err(io::Error::other("NullPointerException")),
        _ => return Err(invalid()),
    };
    Ok(Value::ClassMirror(name))
}

pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let Some((receiver, arguments)) = arguments.split_first() else {
        return Ok(None);
    };
    let Value::ClassMirror(class) = receiver else {
        return Ok(None);
    };
    if let [Value::ClassMirror(target), Value::ClassMirror(source)] = arguments
        && name == b"isAssignableFrom"
        && descriptor == b"(Ljava/lang/Class;)Z"
    {
        let assignable = if *target == *source {
            true
        } else if target.starts_with(b":") || source.starts_with(b":") {
            false
        } else if source.starts_with(b"[") {
            loader.classAssignable(source, target)?
        } else {
            let source = loader.load(source)?;
            crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(
                &source, target,
            ) || crate::share::classfile::systemDictionary::SystemDictionary::implements_interface(
                &source, target,
            )
        };
        return Ok(Some(Value::Int(i32::from(assignable))));
    }
    let value = match (name, descriptor) {
        (b"desiredAssertionStatus", b"()Z") => Value::Int(0),
        (b"isArray", b"()Z") => Value::Int(i32::from(class.starts_with(b"["))),
        (b"isPrimitive", b"()Z") => Value::Int(i32::from(class.starts_with(b":"))),
        (b"getName", b"()Ljava/lang/String;") => {
            if let Some(name) = class.strip_prefix(b":").and_then(primitiveName) {
                return Ok(Some(Value::HostString(name)));
            }
            let name = decodeString(class)?.replace('/', ".");
            Value::HostString(loader.storage.alloc_str(&name))
        }
        (b"isInterface", b"()Z") => Value::Int(i32::from(
            !class.starts_with(b"[")
                && !class.starts_with(b":")
                && loader.load(class)?.class.accessFlags & 0x0200 != 0,
        )),
        (b"getClassLoader", b"()Ljava/lang/ClassLoader;") => {
            if class.starts_with(b"[") || class.starts_with(b":") {
                Value::Null
            } else if loader.bootImage.is_some() {
                if loader.hasClass(class)? {
                    Value::Null
                } else {
                    Value::Object(loader.systemClassLoaderObject()?)
                }
            } else {
                Value::Object(loader.systemClassLoaderObject()?)
            }
        }
        (b"getModifiers", b"()I") => {
            let flags = if class.starts_with(b":") || class.starts_with(b"[") {
                0x0411
            } else {
                i32::from(loader.load(class)?.class.accessFlags & 0x7611)
            };
            Value::Int(flags)
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
