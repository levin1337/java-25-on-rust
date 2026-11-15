//! Native methods for `jdk.internal.jimage.NativeImageBuffer`.
//!
//! HotSpot returns a direct `ByteBuffer` over the mmap'ed jimage file.
//! There is no mmap here, so the file bytes are read into memory and copied
//! straight into the off-heap block backing a real direct `ByteBuffer`
//! (`ByteBuffer.allocateDirect`), located through the buffer's `address`
//! field — avoiding the heavyweight ScopedMemoryAccess `put` path.

use std::io;

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};

use super::offHeapMemory::OffHeapMemory;

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
    // Stream the image into the off-heap block in chunks: holding the
    // whole file as a Vec on top of the direct buffer doubles the
    // transient footprint (~200 MB for java.base modules) and was a
    // real OOM source during Fabric startup.
    let mut file = std::fs::File::open(&path).map_err(|error| {
        io::Error::other(format!(
            "java/io/FileNotFoundException: {} ({error})",
            path
        ))
    })?;
    let capacity = file.metadata().map(|m| m.len() as usize).unwrap_or(0);
    eprintln!("[jimage] getNativeMap path={path} capacity={capacity}");
    let buffer_klass = loader.load(b"java/nio/ByteBuffer")?;

    // ByteBuffer.allocateDirect(capacity) -> DirectByteBuffer over off-heap
    // memory allocated by jdk.internal.misc.Unsafe.allocateMemory0.
    let mut alloc_args = JavaCallArguments::new();
    alloc_args.push_int(capacity as i32);
    let buffer = JavaCalls::call_static(
        loader,
        &buffer_klass,
        b"allocateDirect",
        b"(I)Ljava/nio/ByteBuffer;",
        &alloc_args,
        output,
    )?;
    eprintln!("[jimage] allocateDirect done");

    let Some(Value::Object(handle)) = buffer else {
        return Ok(buffer.map_or(Some(None), |value| Some(Some(value))));
    };

    // Locate the off-heap address stored in the buffer's `address` field
    // (declared on DirectByteBuffer/MappedByteBuffer depending on version)
    // and write the image bytes there directly.
    let (klass, address) = {
        let objects = loader.borrow_objects();
        let Some(object) = objects.get(handle) else {
            return Ok(Some(Some(Value::Object(handle))));
        };
        let klass = object.klass.clone();
        let slot = klass.instanceField(b"address", b"J").ok();
        let address = slot.and_then(|slot| match object.fields.get(slot) {
            Some(Value::Long(value)) => Some(*value as u64),
            _ => None,
        });
        (klass, address)
    };
    let Some(address) = address else {
        eprintln!("[jimage] could not resolve buffer address field");
        return Ok(Some(Some(Value::Object(handle))));
    };
    let memory = OffHeapMemory::global();
    let mut offset = 0usize;
    let mut written = 0usize;
    let mut chunk = vec![0u8; 8 * 1024 * 1024];
    use std::io::Read;
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        memory.write_bytes(address + offset as u64, &chunk[..n])?;
        offset += n;
        written += n;
    }
    eprintln!("[jimage] wrote {written} bytes at 0x{address:016X}");
    let _ = klass;
    Ok(Some(Some(Value::Object(handle))))
}
