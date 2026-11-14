use super::jvm::Jvm;
use crate::share::{classfile::classLoader::ClassLoader, interpreter::bytecodeInterpreter::Value};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Error},
};

pub(crate) fn identity(value: Value<'_>) -> i32 {
    let mut hash = DefaultHasher::new();
    match value {
        Value::Null => return 0,
        Value::Object(handle) => return Jvm::identity_hash_code(handle.wrapping_add(1)),
        Value::ClassMirror(name) => name.hash(&mut hash),
        Value::PrimitiveArray(handle) | Value::ObjectArray(handle) => handle.hash(&mut hash),
        Value::String(bytes) => bytes.hash(&mut hash),
        Value::HostString(text) => text.as_ptr().hash(&mut hash),
        Value::Arguments(values) => values.as_ptr().hash(&mut hash),
        Value::Output => return 1,
        _ => return 0,
    }
    hash.finish() as i32
}

pub(crate) fn hashCode(loader: &ClassLoader<'_>, receiver: Value<'_>) -> io::Result<Option<i32>> {
    match receiver {
        Value::Object(handle) => {
            let objects = loader.objects.borrow();
            let object = objects
                .get(handle)
                .ok_or_else(|| Error::other("Invalid hashCode receiver"))?;
            let mut klass = Some(object.klass.as_ref());
            while let Some(current) = klass {
                if current.class.name == b"java/lang/Object" {
                    break;
                }
                if current.class.methods.iter().any(|method| {
                    method.name == b"hashCode"
                        && method.descriptor == b"()I"
                        && method.accessFlags & 10 == 0
                }) {
                    return Ok(None);
                }
                klass = current.super_klass.as_deref();
            }
        }
        Value::Null => return Err(Error::other("NullPointerException")),
        Value::ClassMirror(_)
        | Value::PrimitiveArray(_)
        | Value::ObjectArray(_)
        | Value::Arguments(_)
        | Value::Output => {}
        _ => return Ok(None),
    }
    Ok(Some(identity(receiver)))
}
