//! Native methods for `jdk.internal.jimage.NativeImageBuffer`.
//!
//! HotSpot returns a direct `ByteBuffer` over the mmap'ed jimage file.
//! There is no mmap here, so the file bytes are read into a Java `byte[]`
//! and wrapped with the genuine `java.nio.ByteBuffer.wrap(byte[])` — a real
//! heap buffer whose accessors run through the normal `Unsafe` natives.

use std::io;

use crate::share::classfile::{classFileParser::decodeString, classLoader::ClassLoader};
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};

pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
    output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    if !(name == b"getNativeMap" && descriptor == b"(Ljava/lang/String;)Ljava/nio/ByteBuffer;")
    {
        return Ok(None);
    }
    let path = super::stringNatives::string(
        loader,
        arguments.first().copied().unwrap_or(Value::Null),
    )?
    .map(|value| value.into_owned())
    .ok_or_else(|| {
        eprintln!(
            "[jimage] getNativeMap arg is not a string: {:?}",
            arguments.first()
        );
        io::Error::other("Expected String path")
    })?;
    let bytes: Vec<i8> = std::fs::read(&path)
        .map_err(|error| {
            io::Error::other(format!(
                "java/io/FileNotFoundException: {} ({error})",
                path
            ))
        })?
        .into_iter()
        .map(|byte| byte as i8)
        .collect();
    let array = loader.alloc_type_array(TypeArrayOop::Byte(bytes));
    let klass = loader.load(b"java/nio/ByteBuffer")?;
    let mut args = JavaCallArguments::new();
    args.push_oop(Value::PrimitiveArray(array));
    let buffer = JavaCalls::call_static(
        loader,
        &klass,
        b"wrap",
        b"([B)Ljava/nio/ByteBuffer;",
        &args,
        output,
    )?;
    Ok(buffer.map_or(Some(None), |value| Some(Some(value))))
}
