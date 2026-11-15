//! Native methods for `jdk.internal.perf.Perf` (hsperfdata counters).
//!
//! HotSpot backs these with the shared `PerfDataBuffer` mmap that the VM
//! updates in place. We have no such mmap, so each counter gets a real heap
//! `ByteBuffer` (via `ByteBuffer.wrap`) that the JDK can hold and read; the
//! values simply stay at their initial contents because nothing writes them.
//! `highResCounter`/`highResFrequency` report real clock values.

use std::io;

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};

const ENTRY_SIZE: usize = 512;

fn heap_buffer<'a>(
    loader: &ClassLoader<'a>,
    size: usize,
    output: &mut dyn std::io::Write,
) -> io::Result<Value<'a>> {
    let array = loader.alloc_type_array(TypeArrayOop::Byte(vec![0i8; size]));
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
    Ok(buffer.unwrap_or(Value::Null))
}

fn high_res_counter() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Instance natives of `jdk.internal.perf.Perf`.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    _arguments: &[Value<'a>],
    output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    let result = match (name, descriptor) {
        (b"highResCounter", b"()J") => Value::Long(high_res_counter()),
        (b"highResFrequency", b"()J") => Value::Long(1_000_000_000),
        (b"attach0", b"(I)Ljava/nio/ByteBuffer;") => {
            return heap_buffer(loader, ENTRY_SIZE, output).map(|v| Some(Some(v)));
        }
        (b"detach", b"(Ljava/nio/ByteBuffer;)V") => return Ok(Some(None)),
        (b"createLong", b"(Ljava/lang/String;IIJ)Ljava/nio/ByteBuffer;")
        | (b"createByteArray", b"(Ljava/lang/String;II[B)Ljava/nio/ByteBuffer;") => {
            return heap_buffer(loader, ENTRY_SIZE, output).map(|v| Some(Some(v)));
        }
        _ => return Ok(None),
    };
    Ok(Some(Some(result)))
}

/// Static natives of `jdk.internal.perf.Perf`.
pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    _arguments: &[Value<'a>],
    _output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    match (name, descriptor) {
        (b"registerNatives", b"()V") => Ok(Some(None)),
        _ => {
            let _ = loader;
            Ok(None)
        }
    }
}

pub(crate) fn is_owner(owner: &[u8]) -> bool {
    owner == b"jdk/internal/perf/Perf"
}
