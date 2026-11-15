//! Native methods of `java.util.zip.Inflater`, `java.util.zip.Deflater` and
//! `java.util.zip.CRC32`, backed by `flate2` (zlib-rs).
//!
//! Stream handles returned by `init` are indexes into a global table. The
//! `inflate*`/`deflate*` methods follow the JDK return packing:
//! `consumed | written << 31 | finished << 62 | flag << 63`, and store
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
    // bool = zlib wrapper enabled, u32 = adler32 state
    Inflate(flate2::Decompress, bool, u32),
    // bool = zlib wrapper enabled, u32 = compression level, u32 = adler32 state
    Deflate(flate2::Compress, bool, u32, u32),
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
            OffHeapMemory::global().read_bytes(base + offset as u64, length)
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
            OffHeapMemory::global().write_bytes(base + offset as u64, data)
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
            Stream::Inflate(decompress, zlib, adler) => {
                let before_in = decompress.total_in();
                let before_out = decompress.total_out();
                let status = decompress
                    .decompress(&input, &mut output, flate2::FlushDecompress::None)
                    .map_err(|_error| invalid("DataFormatException"))?;
                let consumed = (decompress.total_in() - before_in) as usize;
                let written = (decompress.total_out() - before_out) as usize;
                if *zlib {
                    *adler = adler32(*adler, &output[..written.min(output.len())]);
                }
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
    Ok((consumed as u64) | ((written as u64) << 31) | ((finished as u64) << 62))
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
            Stream::Deflate(compress, zlib, _, adler) => {
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
                if *zlib {
                    *adler = adler32(*adler, &input[..consumed.min(input.len())]);
                }
                (
                    written.min(output_len),
                    consumed.min(input.len()),
                    matches!(status, flate2::Status::StreamEnd),
                )
            }
            _ => return Err(invalid("Not a deflater")),
        }
    };
    if std::env::var_os("JVM25_TRACE_ZIP").is_some() {
        eprintln!(
            "[deflate] in_len={} out_len={} flush={} -> written={} consumed={} finished={}",
            input.len(), output_len, flush, written, consumed, finished
        );
    }
    store_output(
        loader,
        arg_value(arguments, layout + 3),
        arg_usize(arguments, layout + 4),
        &output[..written],
    )?;
    set_counters(loader, receiver, consumed, written)?;
    Ok((consumed as u64) | ((written as u64) << 31) | ((finished as u64) << 62))
}

/// Builds the canonical argument vector expected by `run_inflate`/
/// `run_deflate`: `[receiver, addr, inVal, inOff, inLen, outVal, outOff,
/// outLen, (flush, params)]`. `spec` entries are `(value_index, offset_index,
/// length_index)`; a `None` offset means the address already includes it.
fn canonical_args<'a>(
    arguments: &[Value<'a>],
    input: (usize, Option<usize>, usize),
    output: (usize, Option<usize>, usize),
    flush: Option<usize>,
) -> Vec<Value<'a>> {
    let pick = |index: Option<usize>| -> Value<'a> {
        index
            .and_then(|i| arguments.get(i).copied())
            .unwrap_or(Value::Int(0))
    };
    let mut out = Vec::with_capacity(10);
    out.push(arg_value(arguments, 0));
    out.push(arg_value(arguments, 1));
    out.push(arg_value(arguments, input.0));
    out.push(pick(input.1));
    out.push(arg_value(arguments, input.2));
    out.push(arg_value(arguments, output.0));
    out.push(pick(output.1));
    out.push(arg_value(arguments, output.2));
    if let Some(flush_index) = flush {
        out.push(arg_value(arguments, flush_index));
        out.push(arg_value(arguments, flush_index + 1));
    }
    out
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
    // JDK 21 signatures: buffer addresses carry their own offset, so the
    // canonical (value, offset, length) triples use offset 0 for `long`
    // address operands.
    let normalized: Option<(Vec<Value<'a>>, usize)> = match (name, descriptor) {
        // inflateBytesBuffer(addr, inArr, inOff, inLen, outAddr, outLen)
        (b"inflateBytesBuffer", b"(J[BIIJI)J") => Some((
            canonical_args(arguments, (2, Some(3), 4), (5, None, 6), None),
            2,
        )),
        // inflateBufferBytes(addr, inAddr, inLen, outArr, outOff, outLen)
        (b"inflateBufferBytes", b"(JJI[BII)J") => Some((
            canonical_args(arguments, (2, None, 3), (4, Some(5), 6), None),
            2,
        )),
        // inflateBufferBuffer(addr, inAddr, inLen, outAddr, outLen)
        (b"inflateBufferBuffer", b"(JJIJI)J") => Some((
            canonical_args(arguments, (2, None, 3), (4, None, 5), None),
            2,
        )),
        // deflateBytesBuffer(addr, inArr, inOff, inLen, outAddr, outLen, flush, params)
        (b"deflateBytesBuffer", b"(J[BIIJIII)J") => Some((
            canonical_args(arguments, (2, Some(3), 4), (5, None, 6), Some(7)),
            2,
        )),
        // deflateBufferBytes(addr, inAddr, inLen, outArr, outOff, outLen, flush, params)
        (b"deflateBufferBytes", b"(JJI[BIIII)J") => Some((
            canonical_args(arguments, (2, None, 3), (4, Some(5), 6), Some(7)),
            2,
        )),
        // deflateBufferBuffer(addr, inAddr, inLen, outAddr, outLen, flush, params)
        (b"deflateBufferBuffer", b"(JJIJIII)J") => Some((
            canonical_args(arguments, (2, None, 3), (4, None, 5), Some(6)),
            2,
        )),
        _ => None,
    };
    let result = if let Some((args, layout)) = normalized {
        if is_inflater {
            run_inflate(loader, &args, layout)?
        } else {
            run_deflate(loader, &args, layout, 8)?
        }
    } else {
        match (name, descriptor) {
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
        }
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
            Value::Long(alloc(Stream::Inflate(flate2::Decompress::new(!nowrap), !nowrap, 1)) as i64)
        }
        (b"java/util/zip/Deflater", b"init", b"(IIZ)J") => {
            let level = arg_i64(arguments, 0);
            let nowrap = arg_i64(arguments, 2) != 0;
            let level_inner = if level < 0 { 6 } else { level.min(9) as u32 };
            let level = flate2::Compression::new(level_inner);
            Value::Long(alloc(Stream::Deflate(flate2::Compress::new(level, !nowrap), !nowrap, level_inner, 1)) as i64)
        }
        (b"java/util/zip/Inflater", b"reset", b"(J)V")
        | (b"java/util/zip/Deflater", b"reset", b"(J)V") => {
            let handle = arg_i64(arguments, 0) as u64;
            let mut table = table().lock().unwrap();
            if let Some(stream) = table.streams.get_mut(&handle) {
                match stream {
                    Stream::Inflate(decompress, zlib, adler) => {
                        *decompress = flate2::Decompress::new(*zlib);
                        *adler = 1;
                    }
                    Stream::Deflate(compress, zlib, level, adler) => {
                        *compress =
                            flate2::Compress::new(flate2::Compression::new(*level), *zlib);
                        *adler = 1;
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
        // Preset dictionaries are not supported by flate2; accepting them
        // keeps `Inflater.setDictionary`/`Deflater.setDictionary` callers
        // working for streams that never actually use one.
        (b"java/util/zip/Inflater", b"setDictionary", b"(J[BII)V")
        | (b"java/util/zip/Deflater", b"setDictionary", b"(J[BII)V")
        | (b"java/util/zip/Inflater", b"setDictionaryBuffer", b"(JJI)V")
        | (b"java/util/zip/Deflater", b"setDictionaryBuffer", b"(JJI)V") => {
            return Ok(Some(Value::Null));
        }
        (b"java/util/zip/Inflater", b"getAdler", b"(J)I")
        | (b"java/util/zip/Deflater", b"getAdler", b"(J)I") => {
            let handle = arg_i64(arguments, 0) as u64;
            let table = table().lock().unwrap();
            let adler = match table.streams.get(&handle) {
                Some(Stream::Inflate(_, _, adler))
                | Some(Stream::Deflate(_, _, _, adler)) => *adler,
                _ => 1,
            };
            Value::Int(adler as i32)
        }
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
            let data = memory.read_bytes(address + offset as u64, length)?;
            let mut value = crc ^ 0xFFFF_FFFF;
            for byte in data {
                value = crc32_update(value, byte);
            }
            Value::Int(crc32_finalize(value) as i32)
        }
        (b"java/util/zip/CRC32C", b"update", b"(II)I") => {
            let crc = arg_i64(arguments, 0) as u32;
            let byte = arg_i64(arguments, 1) as u8;
            Value::Int(crc32c_finalize(crc32c_update(crc ^ 0xFFFF_FFFF, byte)) as i32)
        }
        (b"java/util/zip/CRC32C", b"updateBytes0", b"(I[BII)I") => {
            let crc = arg_i64(arguments, 0) as u32;
            let data = collect_input(loader, arg_value(arguments, 1), arg_usize(arguments, 2), arg_usize(arguments, 3))?;
            let mut value = crc ^ 0xFFFF_FFFF;
            for &byte in &data {
                value = crc32c_update(value, byte);
            }
            Value::Int(crc32c_finalize(value) as i32)
        }
        (b"java/util/zip/CRC32C", b"updateByteBuffer0", b"(IJII)I") => {
            let crc = arg_i64(arguments, 0) as u32;
            let address = arg_i64(arguments, 1) as u64;
            let offset = arg_usize(arguments, 2);
            let length = arg_usize(arguments, 3);
            let memory = OffHeapMemory::global();
            let data = memory.read_bytes(address + offset as u64, length)?;
            let mut value = crc ^ 0xFFFF_FFFF;
            for byte in data {
                value = crc32c_update(value, byte);
            }
            Value::Int(crc32c_finalize(value) as i32)
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

fn crc32c_finalize(value: u32) -> u32 {
    !value
}

fn crc32c_update(crc: u32, byte: u8) -> u32 {
    let mut crc = crc ^ u32::from(byte);
    for _ in 0..8 {
        crc = if crc & 1 != 0 { (crc >> 1) ^ 0x82F6_3B78 } else { crc >> 1 };
    }
    crc
}

fn adler32(adler: u32, data: &[u8]) -> u32 {
    let mut a = u64::from(adler & 0xFFFF);
    let mut b = u64::from(adler >> 16);
    const MOD_ADLER: u64 = 65_521;
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u64::from(byte);
            b += a;
        }
        a %= MOD_ADLER;
        b %= MOD_ADLER;
    }
    ((b << 16) | a) as u32
}
