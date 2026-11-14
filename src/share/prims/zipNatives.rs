//! Native methods of `java.util.zip.Inflater`, `java.util.zip.Deflater` and
//! `java.util.zip.CRC32`, backed by `flate2` (zlib-rs).
//!
//! Stream handles returned by `init` are indexes into a global table. The
//! `inflate*`/`deflate*` methods follow the JDK return packing:
//! `written | consumed << 31 | finished << 62 | flag << 63`, and store
//! `inputConsumed`/`outputConsumed` on the Java receiver.

use super::offHeapMemory::OffHeapMemory;
use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
};
use std::{
    collections::HashMap,
    io,
    sync::{Mutex, OnceLock},
};

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

enum Stream {
    Inflate(flate2::Decompress),
    Deflate(flate2::Compress),
}

struct Table {
    streams: HashMap<u64, Stream>,
    next: u64,
}

fn table() -> &'static Mutex<Table> {
    static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(Table {
            streams: HashMap::new(),
            next: 1,
        })
    })
}

fn alloc(stream: Stream) -> u64 {
    let mut table = table().lock().unwrap();
    let handle = table.next;
    table.next += 1;
    table.streams.insert(handle, stream);
    handle
}

fn get_stream(handle: u64) -> io::Result<()> {
    let table = table().lock().unwrap();
    if table.streams.contains_key(&handle) {
        Ok(())
    } else {
        Err(invalid("Invalid zip stream"))
    }
}

/// Collects input bytes from either a heap `byte[]` slice or an off-heap
/// address range.

fn collect_input<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    offset: usize,
    length: usize,
) -> io::Result<Vec<u8>> {
    match value {
        Value::PrimitiveArray(handle) => {
            let arrays = loader.arrays.borrow();
            let array = arrays
                .get(handle)
                .ok_or_else(|| invalid("Invalid input array"))?;
            if let crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) = array {
                let start = offset.min(bytes.len());
                let end = (offset + length).min(bytes.len());
                Ok(bytes[start..end].iter().map(|&b| b as u8).collect())
            } else {
                Err(invalid("Expected byte array"))
            }
        }
        Value::Long(address) => {
            let base = u64::try_from(address).map_err(|_| invalid("Invalid buffer address"))?;
            let memory = OffHeapMemory::global();
            let mut data = vec![0u8; length];
            for (index, byte) in data.iter_mut().enumerate() {
                *byte = memory.get_byte(base + offset as u64 + index as u64)? as u8;
            }
            Ok(data)
        }
        _ => Err(invalid("Unsupported input")),
    }
}

fn store_output<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    offset: usize,
    data: &[u8],
) -> io::Result<()> {
    match value {
        Value::PrimitiveArray(handle) => {
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid output array"))?;
            if let crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) = array {
                for (index, &byte) in data.iter().enumerate() {
                    let slot = offset + index;
                    if slot < bytes.len() {
                        bytes[slot] = byte as i8;
                    }
                }
                Ok(())
            } else {
                Err(invalid("Expected byte array"))
            }
        }
        Value::Long(address) => {
            let base = u64::try_from(address).map_err(|_| invalid("Invalid buffer address"))?;
            let memory = OffHeapMemory::global();
            for (index, &byte) in data.iter().enumerate() {
                memory.put_byte(base + offset as u64 + index as u64, byte as i8)?;
            }
            Ok(())
        }
        _ => Err(invalid("Unsupported output")),
    }
}

fn arg_i64(arguments: &[Value<'_>], index: usize) -> i64 {
    match arguments.get(index).copied().unwrap_or(Value::Null) {
        Value::Long(value) => value,
        Value::Int(value) => i64::from(value),
        _ => 0,
    }
}

fn arg_usize(arguments: &[Value<'_>], index: usize) -> usize {
    arg_i64(arguments, index).max(0) as usize
}

fn arg_value<'a>(arguments: &[Value<'a>], index: usize) -> Value<'a> {
    arguments.get(index).copied().unwrap_or(Value::Null)
}

/// Sets `inputConsumed`/`outputConsumed` fields on the Java receiver.
fn set_counters<'a>(
    loader: &ClassLoader<'a>,
    receiver: usize,
    consumed: usize,
    produced: usize,
) -> io::Result<()> {
    // JDK 21 tracks progress through inputPos/inputLim and the packed return
    // value; newer JDKs expose inputConsumed/outputConsumed fields instead.
    let (input_slot, output_slot) = {
        let objects = loader.borrow_objects();
        let Some(object) = objects.get(receiver) else {
            return Err(invalid("Invalid receiver"));
        };
        let input_slot = object
            .klass
            .instanceField(b"inputConsumed", b"I")
            .ok();
        let output_slot = object
            .klass
            .instanceField(b"outputConsumed", b"I")
            .ok();
        (input_slot, output_slot)
    };
    let (Some(input_slot), Some(output_slot)) = (input_slot, output_slot) else {
        return Ok(());
    };
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(receiver)
        .ok_or_else(|| invalid("Invalid receiver"))?;
    if input_slot < object.fields.len() {
        object.fields[input_slot] = Value::Int(consumed as i32);
    }
    if output_slot < object.fields.len() {
        object.fields[output_slot] = Value::Int(produced as i32);
    }
    Ok(())
}

fn run_inflate<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    layout: usize,
) -> io::Result<u64> {
    // layout: index pairs (input_value, input_off, input_len, output_value, output_off, output_len)
    // args[0] = receiver; args[1] = stream handle.
    let receiver = match arg_value(arguments, 0) {
        Value::Object(handle) => handle,
        _ => return Err(invalid("Expected Inflater receiver")),
    };
    let stream_handle = arg_i64(arguments, 1) as u64;
    get_stream(stream_handle)?;
    let input = collect_input(
        loader,
        arg_value(arguments, layout),
        arg_usize(arguments, layout + 1),
        arg_usize(arguments, layout + 2),
    )?;
    let output_len = arg_usize(arguments, layout + 5);
    let mut output = vec![0u8; output_len];
    let (written, consumed, finished) = {
        let mut table = table().lock().unwrap();
        let stream = table
            .streams
            .get_mut(&stream_handle)
            .ok_or_else(|| invalid("Invalid zip stream"))?;
        match stream {
            Stream::Inflate(decompress) => {
                let before_in = decompress.total_in();
                let before_out = decompress.total_out();
                let status = decompress
                    .decompress(&input, &mut output, flate2::FlushDecompress::None)
                    .map_err(|_error| invalid("DataFormatException"))?;
                let consumed = (decompress.total_in() - before_in) as usize;
                let written = (decompress.total_out() - before_out) as usize;
                (written.min(output_len), consumed.min(input.len()), matches!(status, flate2::Status::StreamEnd))
            }
            _ => return Err(invalid("Not an inflater")),
        }
    };
    store_output(
        loader,
        arg_value(arguments, layout + 3),
        arg_usize(arguments, layout + 4),
        &output[..written],
    )?;
    set_counters(loader, receiver, consumed, written)?;
    Ok((written as u64) | ((consumed as u64) << 31) | ((finished as u64) << 62))
}

fn run_deflate<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    layout: usize,
    flush_at: usize,
) -> io::Result<u64> {
    let receiver = match arg_value(arguments, 0) {
        Value::Object(handle) => handle,
        _ => return Err(invalid("Expected Deflater receiver")),
    };
    let stream_handle = arg_i64(arguments, 1) as u64;
    get_stream(stream_handle)?;
    let input = collect_input(
        loader,
        arg_value(arguments, layout),
        arg_usize(arguments, layout + 1),
        arg_usize(arguments, layout + 2),
    )?;
    let output_len = arg_usize(arguments, layout + 5);
    let flush = arg_usize(arguments, flush_at);
    let mut output = vec![0u8; output_len];
    let (written, consumed, finished) = {
        let mut table = table().lock().unwrap();
        let stream = table
            .streams
            .get_mut(&stream_handle)
            .ok_or_else(|| invalid("Invalid zip stream"))?;
        match stream {
            Stream::Deflate(compress) => {
                let before_in = compress.total_in();
                let before_out = compress.total_out();
                // zlib flush constants passed straight through by the JDK.
                let flush = match flush {
                    1 => flate2::FlushCompress::Partial,
                    2 => flate2::FlushCompress::Sync,
                    3 => flate2::FlushCompress::Full,
                    4 => flate2::FlushCompress::Finish,
                    _ => flate2::FlushCompress::None,
                };
                let status = compress
                    .compress(&input, &mut output, flush)
                    .map_err(|_| invalid("Deflate error"))?;
                let consumed = (compress.total_in() - before_in) as usize;
                let written = (compress.total_out() - before_out) as usize;
                (
                    written.min(output_len),
                    consumed.min(input.len()),
                    matches!(status, flate2::Status::StreamEnd),
                )
            }
            _ => return Err(invalid("Not a deflater")),
        }
    };
    store_output(
        loader,
        arg_value(arguments, layout + 3),
        arg_usize(arguments, layout + 4),
        &output[..written],
    )?;
    set_counters(loader, receiver, consumed, written)?;
    Ok((written as u64) | ((consumed as u64) << 31) | ((finished as u64) << 62))
}

/// Instance natives for Inflater/Deflater.
pub(crate) fn instance<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let is_inflater = owner == b"java/util/zip/Inflater";
    let is_deflater = owner == b"java/util/zip/Deflater";
    if !is_inflater && !is_deflater {
        return Ok(None);
    }
    let result = match (name, descriptor) {
        (b"inflateBytesBytes", b"(J[BII[BII)J") => run_inflate(loader, arguments, 2)?,
        (b"inflateBytesBytes", b"(J[BI[BI[BI)J") => run_inflate(loader, arguments, 2)?,
        (b"inflateBytesBuffer", b"(J[BIIJI)J") => run_inflate(loader, arguments, 2)?,
        (b"inflateBufferBytes", b"(JJ[BII[BI)J") => run_inflate(loader, arguments, 3)?,
        (b"inflateBufferBuffer", b"(JJIJI)J") => run_inflate(loader, arguments, 3)?,
        (b"deflateBytesBytes", b"(J[BII[BIIII)J") => run_deflate(loader, arguments, 2, 8)?,
        (b"deflateBytesBytes", b"(J[BI[I[BIIII)J") => run_deflate(loader, arguments, 2, 10)?,
        (b"deflateBytesBuffer", b"(J[BIIJIIII)J") => run_deflate(loader, arguments, 2, 10)?,
        (b"deflateBufferBytes", b"(JJ[BII[BIIII)J") => run_deflate(loader, arguments, 3, 11)?,
        (b"deflateBufferBuffer", b"(JJIJIIII)J") => run_deflate(loader, arguments, 3, 11)?,
        _ => return Ok(None),
    };
    Ok(Some(Value::Long(result as i64)))
}

/// Static natives for Inflater/Deflater/CRC32.
pub(crate) fn statik<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let result = match (owner, name, descriptor) {
        (b"java/util/zip/Inflater", b"init", b"(Z)J") => {
            let nowrap = arg_i64(arguments, 0) != 0;
            Value::Long(alloc(Stream::Inflate(flate2::Decompress::new(!nowrap))) as i64)
        }
        (b"java/util/zip/Deflater", b"init", b"(IIZ)J") => {
            let level = arg_i64(arguments, 0);
            let nowrap = arg_i64(arguments, 2) != 0;
            let level = if level < 0 {
                flate2::Compression::default()
            } else {
                flate2::Compression::new(level.min(9) as u32)
            };
            Value::Long(alloc(Stream::Deflate(flate2::Compress::new(level, !nowrap))) as i64)
        }
        (b"java/util/zip/Inflater", b"reset", b"(J)V")
        | (b"java/util/zip/Deflater", b"reset", b"(J)V") => {
            let handle = arg_i64(arguments, 0) as u64;
            let mut table = table().lock().unwrap();
            if let Some(stream) = table.streams.get_mut(&handle) {
                match stream {
                    Stream::Inflate(decompress) => *decompress = flate2::Decompress::new(true),
                    Stream::Deflate(compress) => {
                        *compress = flate2::Compress::new(flate2::Compression::default(), true)
                    }
                }
            }
            return Ok(Some(Value::Null));
        }
        (b"java/util/zip/Inflater", b"end", b"(J)V")
        | (b"java/util/zip/Deflater", b"end", b"(J)V") => {
            let handle = arg_i64(arguments, 0) as u64;
            table().lock().unwrap().streams.remove(&handle);
            return Ok(Some(Value::Null));
        }
        (b"java/util/zip/Inflater", b"getAdler", b"(J)I")
        | (b"java/util/zip/Deflater", b"getAdler", b"(J)I") => Value::Int(1),
        (b"java/util/zip/CRC32", b"update", b"(II)I") => {
            let crc = arg_i64(arguments, 0) as u32;
            let byte = arg_i64(arguments, 1) as u8;
            Value::Int(crc32_finalize(crc32_update(crc ^ 0xFFFF_FFFF, byte)) as i32)
        }
        (b"java/util/zip/CRC32", b"updateBytes0", b"(I[BII)I") => {
            let crc = arg_i64(arguments, 0) as u32;
            let data = collect_input(loader, arg_value(arguments, 1), arg_usize(arguments, 2), arg_usize(arguments, 3))?;
            let mut value = crc ^ 0xFFFF_FFFF;
            for &byte in &data {
                value = crc32_update(value, byte);
            }
            Value::Int(crc32_finalize(value) as i32)
        }
        (b"java/util/zip/CRC32", b"updateByteBuffer0", b"(IJII)I") => {
            let crc = arg_i64(arguments, 0) as u32;
            let address = arg_i64(arguments, 1) as u64;
            let offset = arg_usize(arguments, 2);
            let length = arg_usize(arguments, 3);
            let memory = OffHeapMemory::global();
            let mut value = crc ^ 0xFFFF_FFFF;
            for index in 0..length {
                let byte = memory.get_byte(address + (offset + index) as u64)? as u8;
                value = crc32_update(value, byte);
            }
            Value::Int(crc32_finalize(value) as i32)
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

fn crc32_finalize(value: u32) -> u32 {
    !value
}

fn crc32_update(crc: u32, byte: u8) -> u32 {
    let mut crc = crc;
    crc ^= u32::from(byte);
    for _ in 0..8 {
        crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
    }
    crc
}
