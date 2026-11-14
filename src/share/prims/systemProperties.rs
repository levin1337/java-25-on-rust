use crate::share::{
    classfile::{classFileParser::decodeString, classLoader::ClassLoader},
    interpreter::bytecodeInterpreter::Value,
};
use std::{
    borrow::Cow,
    io::{self, Error},
};

pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let previous = match (name, descriptor, args) {
        (
            b"setProperty",
            b"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
            [key, value],
        ) => {
            let key = propertyKey(*key)?.into_owned();
            let value = match value {
                Value::HostString(value) => *value,
                _ => loader.storage.alloc_str(&string(*value)?),
            };
            loader.systemProperties.borrow_mut().insert(key, value)
        }
        (b"getProperty", b"(Ljava/lang/String;)Ljava/lang/String;", [key])
        | (b"getProperty", b"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", [key, _]) => {
            loader
                .systemProperties
                .borrow()
                .get(propertyKey(*key)?.as_ref())
                .copied()
        }
        (b"clearProperty", b"(Ljava/lang/String;)Ljava/lang/String;", [key]) => loader
            .systemProperties
            .borrow_mut()
            .remove(propertyKey(*key)?.as_ref()),
        _ => return Ok(None),
    };
    let result = previous
        .map(Value::HostString)
        .unwrap_or_else(|| match (name, args) {
            (b"getProperty", [_, default]) => *default,
            _ => Value::Null,
        });
    Ok(Some(result))
}

fn propertyKey(value: Value<'_>) -> io::Result<Cow<'_, str>> {
    let key = string(value)?;
    if key.is_empty() {
        return Err(Error::other("IllegalArgumentException: empty property key"));
    }
    Ok(key)
}

fn string(value: Value<'_>) -> io::Result<Cow<'_, str>> {
    match value {
        Value::HostString(value) => Ok(Cow::Borrowed(value)),
        Value::String(bytes) => decodeString(bytes).map(Cow::Owned),
        Value::Null => Err(Error::other("NullPointerException")),
        _ => Err(Error::other("Expected java.lang.String")),
    }
}
