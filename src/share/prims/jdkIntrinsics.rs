//! Fast paths for hot pure-Java JDK helper methods.
//!
//! These are ordinary (non-native) JDK methods that show up in inner loops
//! of classfile/ZIP parsing (`java.util.zip.ZipFile$Source.initCEN`,
//! `ArraysSupport`, `Preconditions`). Interpreting each call costs a full
//! frame; the semantics here are exact, small, and side-effect free.

use std::io;

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::oops::typeArrayOop::TypeArrayOop;

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

fn int_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i32> {
    match arguments.get(index).copied() {
        Some(Value::Int(value)) => Ok(value),
        _ => Err(invalid("Invalid int argument")),
    }
}

fn matchJarManifest<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>]) -> Option<Value<'a>> {
    let [
        _,
        Value::PrimitiveArray(pattern),
        Value::PrimitiveArray(bytes),
        Value::PrimitiveArray(last),
        Value::PrimitiveArray(shift),
    ] = arguments
    else {
        return None;
    };
    let arrays = loader.arrays.borrow();
    let read = |handle| match arrays.get(handle)? {
        TypeArrayOop::Byte(bytes) => Some(bytes.as_slice()),
        _ => None,
    };
    let (pattern, bytes, last, shift) =
        (read(*pattern)?, read(*bytes)?, read(*last)?, read(*shift)?);
    let Some(limit) = bytes.len().checked_sub(pattern.len()) else {
        return Some(Value::Int(-1));
    };
    let mut offset = 0;
    'search: while offset <= limit {
        for (index, &expected) in pattern.iter().enumerate().rev() {
            let byte = bytes[offset + index] as u8;
            if !(32..=122).contains(&byte) {
                offset += pattern.len();
                continue 'search;
            }
            let byte = if byte >= 97 { byte - 32 } else { byte };
            if byte as i8 != expected {
                let badCharacter = index as i32 + 1 - i32::from(*last.get(usize::from(byte - 32))?);
                let advance = badCharacter.max(i32::from(*shift.get(index)?));
                if advance <= 0 {
                    return None;
                }
                offset += advance as usize;
                continue 'search;
            }
        }
        return Some(Value::Int(i32::try_from(offset).ok()?));
    }
    Some(Value::Int(-1))
}

fn zipName<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    metadata: bool,
) -> Option<Value<'a>> {
    let [
        Value::PrimitiveArray(handle),
        Value::Int(start),
        Value::Int(length),
    ] = arguments
    else {
        return None;
    };
    if metadata && *length <= 9 || !metadata && *length == 0 {
        return Some(Value::Int(0));
    }
    let start = usize::try_from(*start).ok()?;
    let length = usize::try_from(*length).ok()?;
    let arrays = loader.arrays.borrow();
    let TypeArrayOop::Byte(bytes) = arrays.get(*handle)? else {
        return None;
    };
    let name = bytes.get(start..start.checked_add(length)?)?;
    if metadata {
        return Some(Value::Int(i32::from(
            name.last() != Some(&(b'/' as i8))
                && name[..9]
                    .iter()
                    .zip(b"META-INF/")
                    .all(|(&a, b)| (a as u8).eq_ignore_ascii_case(b)),
        )));
    }
    let hash = |value: i32, unit: u16| value.wrapping_mul(31).wrapping_add(i32::from(unit));
    let value = if name.iter().all(|&byte| byte >= 0) {
        name.iter().map(|&byte| byte as u16).fold(0, hash)
    } else {
        let bytes: Vec<u8> = name.iter().map(|&byte| byte as u8).collect();
        std::str::from_utf8(&bytes)
            .ok()?
            .encode_utf16()
            .fold(0, hash)
    };
    Some(Value::Int(if name.last() == Some(&(b'/' as i8)) {
        value
    } else {
        hash(value, u16::from(b'/'))
    }))
}

fn mismatchIndex<T: Copy>(left: &[T], right: &[i8], convert: impl Fn(T) -> i8) -> Option<usize> {
    left.iter()
        .zip(right)
        .position(|(&a, &b)| convert(a) != b)
        .or_else(|| (left.len() != right.len()).then(|| left.len().min(right.len())))
}

fn compareZipName<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>]) -> Option<Value<'a>> {
    let [
        _,
        name,
        Value::PrimitiveArray(handle),
        Value::Int(start),
        Value::Int(length),
        Value::Int(addSlash),
    ] = args
    else {
        return None;
    };
    let name = super::stringNatives::string(loader, *name).ok()??;
    let end = usize::try_from(start.checked_add(*length)?).ok()?;
    let start = usize::try_from(*start).ok()?;
    let arrays = loader.arrays.borrow();
    let TypeArrayOop::Byte(bytes) = arrays.get(*handle)? else {
        return None;
    };
    let entry = bytes.get(start..end)?;
    let result = match mismatchIndex(name.as_bytes(), entry, |byte| byte as i8) {
        None => 0,
        Some(index) if *addSlash != 0 && index + 1 == entry.len() && entry[index] == b'/' as i8 => {
            1
        }
        Some(_) => 2,
    };
    Some(Value::Int(result))
}

fn byteRangeMismatch<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>]) -> Option<Value<'a>> {
    let [
        Value::PrimitiveArray(a),
        Value::Int(aFrom),
        Value::Int(aTo),
        Value::PrimitiveArray(b),
        Value::Int(bFrom),
        Value::Int(bTo),
    ] = args
    else {
        return None;
    };
    let arrays = loader.arrays.borrow();
    let range = |handle, from, to| {
        let TypeArrayOop::Byte(bytes) = arrays.get(handle)? else {
            return None;
        };
        bytes.get(usize::try_from(from).ok()?..usize::try_from(to).ok()?)
    };
    let left = range(*a, *aFrom, *aTo)?;
    let right = range(*b, *bFrom, *bTo)?;
    Some(Value::Int(
        mismatchIndex(left, right, |byte| byte).map_or(-1, |index| index as i32),
    ))
}

fn searchUnits(
    source: &[i8],
    target: &[i8],
    sourceWidth: usize,
    targetWidth: usize,
    from: usize,
    reverse: bool,
) -> Option<usize> {
    let count = target.len() / targetWidth;
    let last = (source.len() / sourceWidth).checked_sub(count)?;
    let matches = |index: usize| {
        let start = index * sourceWidth;
        let candidate = &source[start..start + count * sourceWidth];
        if sourceWidth == targetWidth {
            return candidate == target;
        }
        candidate.chunks_exact(2).zip(target).all(|(unit, &byte)| {
            u16::from_ne_bytes([unit[0] as u8, unit[1] as u8]) == u16::from(byte as u8)
        })
    };
    if reverse {
        (0..=from.min(last)).rev().find(|&index| matches(index))
    } else {
        (from..=last).find(|&index| matches(index))
    }
}

fn stringSearch<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> Option<Value<'a>> {
    let arrays = loader.arrays.borrow();
    let read = |handle| match arrays.get(handle)? {
        TypeArrayOop::Byte(bytes) => Some(bytes.as_slice()),
        _ => None,
    };
    let width = if owner == b"java/lang/StringUTF16" {
        2
    } else {
        1
    };
    let targetWidth = if name.ends_with(b"Latin1") { 1 } else { width };
    let reverse = name.starts_with(b"last");
    let found = match (descriptor, args) {
        (b"([B[B)I", [Value::PrimitiveArray(source), Value::PrimitiveArray(target)])
            if !reverse =>
        {
            let source = read(*source)?;
            let target = read(*target)?;
            if target.is_empty() {
                return Some(Value::Int(0));
            }
            searchUnits(source, target, width, targetWidth, 0, false)
        }
        (
            b"([BI[BII)I",
            [
                Value::PrimitiveArray(source),
                Value::Int(sourceLength),
                Value::PrimitiveArray(target),
                Value::Int(targetLength),
                Value::Int(from),
            ],
        ) => {
            let source =
                read(*source)?.get(..usize::try_from(*sourceLength).ok()?.checked_mul(width)?)?;
            let target = read(*target)?.get(
                ..usize::try_from(*targetLength)
                    .ok()?
                    .checked_mul(targetWidth)?,
            )?;
            let from = usize::try_from(*from).ok()?;
            if target.is_empty()
                || from > source.len() / width
                || (reverse && from.checked_add(target.len() / targetWidth)? > source.len() / width)
            {
                return None;
            }
            searchUnits(source, target, width, targetWidth, from, reverse)
        }
        (
            b"([BIII)I",
            [
                Value::PrimitiveArray(source),
                Value::Int(character),
                Value::Int(from),
                Value::Int(end),
            ],
        ) if !reverse => {
            let from = usize::try_from(*from).ok()?;
            let end = usize::try_from(*end).ok()?;
            if from >= end {
                return Some(Value::Int(-1));
            }
            let source = read(*source)?.get(..end.checked_mul(width)?)?;
            searchCharacter(source, width, *character, from, false)
        }
        (
            b"([BII)I",
            [
                Value::PrimitiveArray(source),
                Value::Int(character),
                Value::Int(from),
            ],
        ) if reverse => {
            let Ok(from) = usize::try_from(*from) else {
                return Some(Value::Int(-1));
            };
            searchCharacter(read(*source)?, width, *character, from, true)
        }
        _ => return None,
    };
    Some(Value::Int(found.map_or(-1, |index| index as i32)))
}

fn searchCharacter(
    source: &[i8],
    width: usize,
    character: i32,
    from: usize,
    reverse: bool,
) -> Option<usize> {
    if width == 1 {
        let byte = u8::try_from(character).ok()? as i8;
        return searchUnits(source, &[byte], 1, 1, from, reverse);
    }
    let mut units = [0; 2];
    let units = if let Ok(unit) = u16::try_from(character) {
        units[0] = unit;
        &units[..1]
    } else {
        char::from_u32(u32::try_from(character).ok()?)?.encode_utf16(&mut units)
    };
    let mut encoded = [0; 4];
    for (unit, bytes) in units.iter().zip(encoded.chunks_exact_mut(2)) {
        let [a, b] = unit.to_ne_bytes();
        bytes[0] = a as i8;
        bytes[1] = b as i8;
    }
    searchUnits(source, &encoded[..units.len() * 2], 2, 2, from, reverse)
}

fn latin1UppercaseRange(character: i32) -> bool {
    matches!(character, 0x41..=0x5a | 0xc0..=0xd6 | 0xd8..=0xde)
}

fn latin1Character<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> Option<Value<'a>> {
    if let (b"digit", b"(II)I", [_, Value::Int(character), Value::Int(radix)]) =
        (name, descriptor, args)
    {
        let Value::PrimitiveArray(handle) = klass.staticField(b"DIGITS", b"[B").ok()?.get() else {
            return None;
        };
        let arrays = loader.arrays.borrow();
        let TypeArrayOop::Byte(digits) = arrays.get(handle)? else {
            return None;
        };
        let digit = i32::from(*digits.get(usize::try_from(*character).ok()?)?);
        return Some(Value::Int(
            if (2..=36).contains(radix) && (0..*radix).contains(&digit) {
                digit
            } else {
                -1
            },
        ));
    }

    let [_, Value::Int(character)] = args else {
        return None;
    };
    let character = i32::from(u8::try_from(*character).ok()?);
    let read = |field, signature| {
        let Value::PrimitiveArray(handle) = klass.staticField(field, signature).ok()?.get() else {
            return None;
        };
        let arrays = loader.arrays.borrow();
        let index = character as usize;
        match arrays.get(handle)? {
            TypeArrayOop::Int(values) => values.get(index).copied(),
            TypeArrayOop::Char(values) => values.get(index).copied().map(i32::from),
            _ => None,
        }
    };
    let extended = || read(b"B", b"[C");
    let properties = || read(b"A", b"[I");
    let value = match (name, descriptor) {
        (b"isDigit", b"(I)Z") => i32::from((0x30..=0x39).contains(&character)),
        (b"toLowerCase", b"(I)I") => {
            if latin1UppercaseRange(character) {
                character | 0x20
            } else {
                character
            }
        }
        (b"toUpperCase" | b"toTitleCase", b"(I)I") => match character {
            0xff => 0x178,
            0xb5 => 0x39c,
            value if latin1UppercaseRange(value & !0x20) => value & !0x20,
            value => value,
        },
        (b"getPropertiesEx", b"(I)I") => extended()?,
        (b"getProperties", b"(I)I") => properties()?,
        (b"getType", b"(I)I") => properties()? & 0x1f,
        (b"getDirectionality", b"(I)B") => (properties()? & 0x78000000) >> 27,
        (b"isMirrored", b"(I)Z") => i32::from(properties()? < 0),
        (b"isJavaIdentifierStart", b"(I)Z") => i32::from(properties()? & 0x7000 >= 0x5000),
        (b"isJavaIdentifierPart", b"(I)Z") => i32::from(properties()? & 0x3000 != 0),
        (b"isIdentifierIgnorable", b"(I)Z") => i32::from(properties()? & 0x7000 == 0x1000),
        (b"isUnicodeIdentifierPart", b"(I)Z") => {
            i32::from(extended()? & 32 != 0 || properties()? & 0x7000 == 0x1000)
        }
        (b"isWhitespace", b"(I)Z") => i32::from(properties()? & 0x7000 == 0x4000),
        (b"getNumericValue", b"(I)I") => {
            let value = properties()?;
            let digit = (character + ((value & 0x03e0) >> 5)) & 31;
            match value & 0x0c00 {
                0x0400 => digit,
                0x0800 => -2,
                0x0c00 => digit + 10,
                _ => -1,
            }
        }
        (b"toUpperCaseEx", b"(I)I") => {
            let value = properties()?;
            if value & 0x10000 == 0 {
                character
            } else if value & 0x07fc0000 == 0x07fc0000 {
                if character == 0xb5 { 0x39c } else { -1 }
            } else {
                character - ((value << 5) >> 23)
            }
        }
        (method, b"(I)Z") => {
            let flag = match method {
                b"isLowerCase" => 1,
                b"isUpperCase" => 2,
                b"isOtherAlphabetic" => 4,
                b"isIdeographic" => 8,
                b"isUnicodeIdentifierStart" => 16,
                b"isEmoji" => 64,
                b"isEmojiPresentation" => 128,
                b"isEmojiModifier" => 256,
                b"isEmojiModifierBase" => 512,
                b"isEmojiComponent" => 1024,
                b"isExtendedPictographic" => 2048,
                _ => return None,
            };
            i32::from(extended()? & flag != 0)
        }
        _ => return None,
    };
    Some(Value::Int(value))
}

pub(crate) fn invokeInstance<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> Option<Value<'a>> {
    match (klass.class.name, name, descriptor) {
        (b"java/lang/CharacterDataLatin1", _, _) => {
            latin1Character(loader, klass, name, descriptor, args)
        }
        (b"java/util/jar/JarFile", b"match", b"([B[B[B[B)I") => matchJarManifest(loader, args),
        (b"java/util/zip/ZipCoder$UTF8ZipCoder", b"checkedHash", b"([BII)I") => {
            zipName(loader, args.get(1..)?, false)
        }
        (b"java/util/zip/ZipCoder$UTF8ZipCoder", b"compare", b"(Ljava/lang/String;[BIIZ)B") => {
            compareZipName(loader, args)
        }
        (b"sun/security/provider/DigestBase", b"implCompressMultiBlock0", b"([BII)I")
        | (b"sun/security/provider/SHA", b"implCompressMultiBlock0", b"([BII)I")
        | (b"sun/security/provider/SHA2", b"implCompressMultiBlock0", b"([BII)I")
        | (b"sun/security/provider/SHA3", b"implCompressMultiBlock0", b"([BII)I")
        | (b"sun/security/provider/SHA5", b"implCompressMultiBlock0", b"([BII)I") => {
            digestMultiBlock(loader, args)
        }
        (b"java/util/zip/ZipFile$Source", b"nextEntryPos", b"(III)I") => {
            let [
                Value::Object(receiver),
                Value::Int(header),
                Value::Int(name),
                Value::Int(length),
            ] = args
            else {
                return None;
            };
            let Value::PrimitiveArray(handle) =
                super::methodHandleNatives::object_field(loader, *receiver, b"cen", b"[B").ok()?
            else {
                return None;
            };
            let arrays = loader.arrays.borrow();
            let TypeArrayOop::Byte(bytes) = arrays.get(handle)? else {
                return None;
            };
            let header = bytes.get(usize::try_from(*header).ok()?..)?;
            let extra = centralDirectoryTail(header)?;
            Some(Value::Int(name.wrapping_add(*length).wrapping_add(extra)))
        }
        _ => None,
    }
}

pub(crate) fn invokeVoidInstance<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> bool {
    match (owner, name, descriptor) {
        (b"sun/security/provider/SHA", b"implCompress0", b"([BI)V") => {
            let ok = compressSha1(loader, args).is_some();
            if !ok && std::env::var_os("JVM25_SHA_DEBUG").is_some() {
                eprintln!("[sha1] compressSha1 returned None");
            }
            ok
        }
        (b"sun/security/provider/SHA2", b"implCompress0", b"([BI)V") => {
            compressSha256(loader, args).is_some()
        }
        (b"sun/security/provider/SHA5", b"implCompress0", b"([BI)V") => {
            compressSha512(loader, args).is_some()
        }
        (b"sun/security/provider/DigestBase", b"engineUpdate", b"([BII)V")
        | (b"sun/security/provider/SHA", b"engineUpdate", b"([BII)V")
        | (b"sun/security/provider/SHA2", b"engineUpdate", b"([BII)V")
        | (b"sun/security/provider/SHA3", b"engineUpdate", b"([BII)V")
        | (b"sun/security/provider/SHA5", b"engineUpdate", b"([BII)V") => {
            digestEngineUpdate(loader, args).is_some()
        }
        _ => false,
    }
}

/// Shared argument extraction for `implCompress0([BI)V`: receiver object,
/// byte[] input, int offset. Returns the input block bytes and the state
/// array handle.
fn digestBlockArgs<'a>(
    loader: &ClassLoader<'a>,
    args: &[Value<'a>],
    state_desc: &[u8],
    block_len: usize,
) -> Option<(usize, usize, Vec<i8>)> {
    let [Value::Object(receiver), Value::PrimitiveArray(input), Value::Int(offset)] = args
    else {
        return None;
    };
    let Value::PrimitiveArray(state) =
        super::methodHandleNatives::object_field(loader, *receiver, b"state", state_desc).ok()?
    else {
        return None;
    };
    let offset = usize::try_from(*offset).ok()?;
    let mut arrays = loader.arrays.borrow();
    let TypeArrayOop::Byte(bytes) = arrays.get(*input)? else {
        return None;
    };
    let block = bytes.get(offset..offset.checked_add(block_len)?)?;
    Some((*input, state, block.to_vec()))
}

/// `DigestBase.implCompressMultiBlock0([BII)I`: compresses every full block in
/// `[off, end]` and returns the offset past the last compressed block.
fn digestMultiBlock<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>]) -> Option<Value<'a>> {
    let [
        Value::Object(receiver),
        Value::PrimitiveArray(input),
        Value::Int(off),
        Value::Int(end),
    ] = args
    else {
        return None;
    };
    let klass = super::methodHandleNatives::object_klass(loader, *receiver).ok()?;
    let name = klass.class.name;
    let has = |needle: &[u8]| name.windows(needle.len()).any(|w| w == needle);
    let (state_desc, block_size, algo): (&[u8], i32, u8) = if has(b"SHA5") {
        (b"[J", 128, 2)
    } else if has(b"SHA2") {
        (b"[I", 64, 1)
    } else if has(b"SHA") {
        (b"[I", 64, 0)
    } else {
        return None;
    };
    // Resolve the digest state array once; per-block field lookups dominated
    // the multi-block loop.
    let Value::PrimitiveArray(state) =
        super::methodHandleNatives::object_field(loader, *receiver, b"state", state_desc).ok()?
    else {
        return None;
    };
    let dbg = std::env::var_os("JVM25_SHA_DEBUG").is_some();
    let t0 = std::time::Instant::now();
    let mut off = *off;
    let mut nblocks = 0u64;
    while off <= *end {
        let ok = match algo {
            0 => sha1Block(loader, state, *input, off),
            1 => sha256Block(loader, state, *input, off),
            _ => sha512Block(loader, state, *input, off),
        };
        if ok.is_none() {
            if dbg {
                eprintln!("[sha-mb] block failed klass={} off={} end={}", String::from_utf8_lossy(name), off, end);
            }
            return None;
        }
        nblocks += 1;
        off = off.wrapping_add(block_size);
    }
    if dbg {
        eprintln!("[sha-mb] blocks={} took={:?}", nblocks, t0.elapsed());
    }
    Some(Value::Int(off))
}

/// Resolves the digest algorithm, block size and state-array handle for a
/// `DigestBase` subclass instance (SHA/SHA2/SHA3/SHA5).
fn digestState<'a>(
    loader: &ClassLoader<'a>,
    receiver: usize,
) -> Option<(usize, i32, u8)> {
    let klass = super::methodHandleNatives::object_klass(loader, receiver).ok()?;
    let name = klass.class.name;
    let has = |needle: &[u8]| name.windows(needle.len()).any(|w| w == needle);
    let (state_desc, block_size, algo): (&[u8], i32, u8) = if has(b"SHA5") {
        (b"[J", 128, 2)
    } else if has(b"SHA2") {
        (b"[I", 64, 1)
    } else if has(b"SHA") {
        (b"[I", 64, 0)
    } else {
        return None;
    };
    let Value::PrimitiveArray(state) =
        super::methodHandleNatives::object_field(loader, receiver, b"state", state_desc).ok()?
    else {
        return None;
    };
    Some((state, block_size, algo))
}

fn compressBlock<'a>(loader: &ClassLoader<'a>, algo: u8, state: usize, input: usize, off: i32) -> Option<()> {
    match algo {
        0 => sha1Block(loader, state, input, off),
        1 => sha256Block(loader, state, input, off),
        _ => sha512Block(loader, state, input, off),
    }
}

/// `DigestBase.engineReset` equivalent for the fast path: restores the
/// chaining state to its initial value and clears the message-schedule
/// array `W`. `DigestBase.engineUpdate` invokes this whenever
/// `bytesProcessed < 0` (the flag `engineDigest` leaves behind), so a
/// `MessageDigest` instance stays reusable across `digest()` calls.
fn reset_digest<'a>(loader: &ClassLoader<'a>, receiver: usize, algo: u8) -> Option<()> {
    let state_desc: &[u8] = if algo == 2 { b"[J" } else { b"[I" };
    let Value::PrimitiveArray(state) =
        super::methodHandleNatives::object_field(loader, receiver, b"state", state_desc).ok()?
    else {
        return None;
    };
    if algo == 2 {
        // SHA-384/512: state is long[8] seeded from `initialHashes` ([J).
        let initial: Vec<i64> = match super::methodHandleNatives::object_field(
            loader, receiver, b"initialHashes", b"[J",
        ) {
            Ok(Value::PrimitiveArray(h)) => {
                let arrays = loader.arrays.borrow();
                let TypeArrayOop::Long(words) = arrays.get(h)? else {
                    return None;
                };
                words.to_vec()
            }
            _ => return None,
        };
        let mut arrays = loader.arrays.borrow_mut();
        let TypeArrayOop::Long(state_words) = arrays.get_mut(state)? else {
            return None;
        };
        for (slot, word) in state_words.iter_mut().zip(initial.iter()) {
            *slot = *word;
        }
    } else if algo == 1 {
        // SHA-224/256: state is int[8] seeded from `initialHashes` ([I).
        let initial: Vec<i32> = match super::methodHandleNatives::object_field(
            loader, receiver, b"initialHashes", b"[I",
        ) {
            Ok(Value::PrimitiveArray(h)) => {
                let arrays = loader.arrays.borrow();
                let TypeArrayOop::Int(words) = arrays.get(h)? else {
                    return None;
                };
                words.to_vec()
            }
            _ => return None,
        };
        let mut arrays = loader.arrays.borrow_mut();
        let TypeArrayOop::Int(state_words) = arrays.get_mut(state)? else {
            return None;
        };
        for (slot, word) in state_words.iter_mut().zip(initial.iter()) {
            *slot = *word;
        }
    } else {
        // SHA-1: state is int[5] with fixed IV constants (SHA.resetHashes).
        const SHA1_IV: [i32; 5] = [
            0x67452301u32 as i32,
            0xefcdab89u32 as i32,
            0x98badcfeu32 as i32,
            0x10325476u32 as i32,
            0xc3d2e1f0u32 as i32,
        ];
        let mut arrays = loader.arrays.borrow_mut();
        let TypeArrayOop::Int(state_words) = arrays.get_mut(state)? else {
            return None;
        };
        for (slot, word) in state_words.iter_mut().zip(SHA1_IV.iter()) {
            *slot = *word;
        }
    }
    // implReset also clears W ([I for SHA/SHA2, [J for SHA5) when present.
    let w_desc: &[u8] = if algo == 2 { b"[J" } else { b"[I" };
    if let Ok(Value::PrimitiveArray(w)) =
        super::methodHandleNatives::object_field(loader, receiver, b"W", w_desc)
    {
        let mut arrays = loader.arrays.borrow_mut();
        match arrays.get_mut(w) {
            Some(TypeArrayOop::Int(words)) => words.fill(0),
            Some(TypeArrayOop::Long(words)) => words.fill(0),
            _ => {}
        }
    }
    Some(())
}

/// `DigestBase.engineUpdate([BII)V`: byte-for-byte port of the JDK buffering
/// logic with native block compression.
fn digestEngineUpdate<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>]) -> Option<()> {
    let [
        Value::Object(receiver),
        Value::PrimitiveArray(input),
        Value::Int(ofs),
        Value::Int(len),
    ] = args
    else {
        return None;
    };
    if *len == 0 {
        return Some(());
    }
    let dbg = std::env::var_os("JVM25_SHA_DEBUG").is_some();
    let t0 = std::time::Instant::now();
    if dbg {
        eprintln!("[engineUpdate] enter ofs={} len={}", ofs, len);
    }
    let (state, block_size, algo) = match digestState(loader, *receiver) {
        Some(v) => v,
        None => {
            if dbg { eprintln!("[engineUpdate] digestState failed"); }
            return None;
        }
    };
    let field = |name: &[u8], desc: &[u8]| -> Option<Value<'a>> {
        match super::methodHandleNatives::object_field(loader, *receiver, name, desc) {
            Ok(v) => Some(v),
            Err(_) => {
                if dbg { eprintln!("[engineUpdate] field {}{} missing", String::from_utf8_lossy(name), String::from_utf8_lossy(desc)); }
                None
            }
        }
    };
    let buffer = match field(b"buffer", b"[B") {
        Some(Value::PrimitiveArray(h)) => h,
        _ => return None,
    };
    let mut buf_ofs = match field(b"bufOfs", b"I") {
        Some(Value::Int(v)) => v,
        _ => return None,
    };
    let mut ofs = *ofs;
    let mut len = *len;

    // bytesProcessed += len. A negative value means the previous
    // engineDigest() left the object un-reset; DigestBase.engineUpdate
    // runs engineReset() first — mirror that here (DigestBase.java:111).
    if let Some(Value::Long(processed)) = field(b"bytesProcessed", b"J") {
        let mut base = processed;
        if processed < 0 {
            reset_digest(loader, *receiver, algo)?;
            buf_ofs = 0;
            base = 0;
        }
        super::methodHandleNatives::set_object_field(
            loader,
            *receiver,
            b"bytesProcessed",
            b"J",
            Value::Long(base.wrapping_add(i64::from(len))),
        )
        .ok()?;
    }

    // Fill the pending partial block first.
    if buf_ofs != 0 {
        let n = len.min(block_size - buf_ofs);
        if copy_bytes(loader, *input, usize::try_from(ofs).ok()?, buffer, usize::try_from(buf_ofs).ok()?, usize::try_from(n).ok()?).is_none() {
            if dbg { eprintln!("[engineUpdate] copy_bytes(fill) failed"); }
            return None;
        }
        buf_ofs += n;
        ofs += n;
        len -= n;
        if buf_ofs >= block_size {
            if compressBlock(loader, algo, state, buffer, 0).is_none() {
                if dbg { eprintln!("[engineUpdate] compressBlock(buffer) failed"); }
                return None;
            }
            buf_ofs = 0;
        }
    }
    // Compress every complete block in [ofs, ofs+len).
    if len >= block_size {
        let limit = ofs + len;
        let mut o = ofs;
        while o <= limit - block_size {
            if compressBlock(loader, algo, state, *input, o).is_none() {
                if dbg { eprintln!("[engineUpdate] compressBlock(input) failed o={}", o); }
                return None;
            }
            o = o.wrapping_add(block_size);
        }
        len = limit - o;
        ofs = o;
    }
    // Stash the remainder into the pending buffer.
    if len > 0 {
        if copy_bytes(loader, *input, usize::try_from(ofs).ok()?, buffer, 0, usize::try_from(len).ok()?).is_none() {
            if dbg { eprintln!("[engineUpdate] copy_bytes failed"); }
            return None;
        }
        buf_ofs = len;
    }
    if super::methodHandleNatives::set_object_field(loader, *receiver, b"bufOfs", b"I", Value::Int(buf_ofs)).is_err() {
        if dbg { eprintln!("[engineUpdate] set bufOfs failed"); }
        return None;
    }
    if dbg {
        eprintln!("[engineUpdate] len={} took={:?}", len, t0.elapsed());
    }
    Some(())
}

/// Copies `n` bytes between two byte[] primitive arrays.
fn copy_bytes<'a>(
    loader: &ClassLoader<'a>,
    src: usize,
    src_off: usize,
    dst: usize,
    dst_off: usize,
    n: usize,
) -> Option<()> {
    let src_end = src_off.checked_add(n)?;
    let dst_end = dst_off.checked_add(n)?;
    let bytes = {
        let arrays = loader.arrays.borrow();
        let TypeArrayOop::Byte(src_bytes) = arrays.get(src)? else {
            return None;
        };
        src_bytes.get(src_off..src_end)?.to_vec()
    };
    let mut arrays = loader.arrays.borrow_mut();
    let TypeArrayOop::Byte(dst_bytes) = arrays.get_mut(dst)? else {
        return None;
    };
    dst_bytes.get_mut(dst_off..dst_end)?.copy_from_slice(&bytes);
    Some(())
}

fn compressSha1<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>]) -> Option<()> {
    let (_input, state, block) = digestBlockArgs(loader, args, b"[I", 64)?;
    let mut words = [0u32; 80];
    for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes([bytes[0] as u8, bytes[1] as u8, bytes[2] as u8, bytes[3] as u8]);
    }
    let mut arrays = loader.arrays.borrow_mut();
    let TypeArrayOop::Int(state) = arrays.get_mut(state)? else {
        return None;
    };
    let state = state.first_chunk_mut::<5>()?;
    let [mut a, mut b, mut c, mut d, mut e] = state.map(|word| word as u32);
    for round in 0..80 {
        if round >= 16 {
            words[round] =
                (words[round - 3] ^ words[round - 8] ^ words[round - 14] ^ words[round - 16])
                    .rotate_left(1);
        }
        let (mix, constant) = match round {
            0..=19 => ((b & c) | (!b & d), 0x5a827999),
            20..=39 => (b ^ c ^ d, 0x6ed9eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
            _ => (b ^ c ^ d, 0xca62c1d6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(mix)
            .wrapping_add(e)
            .wrapping_add(constant)
            .wrapping_add(words[round]);
        [a, b, c, d, e] = [next, a, b.rotate_left(30), c, d];
    }
    for (state, word) in state.iter_mut().zip([a, b, c, d, e]) {
        *state = state.wrapping_add(word as i32);
    }
    Some(())
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

fn compressSha256<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>]) -> Option<()> {
    let (_input, state, block) = digestBlockArgs(loader, args, b"[I", 64)?;
    let mut words = [0u32; 64];
    for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes([bytes[0] as u8, bytes[1] as u8, bytes[2] as u8, bytes[3] as u8]);
    }
    for round in 16..64 {
        let s0 = words[round - 15].rotate_right(7)
            ^ words[round - 15].rotate_right(18)
            ^ (words[round - 15] >> 3);
        let s1 = words[round - 2].rotate_right(17)
            ^ words[round - 2].rotate_right(19)
            ^ (words[round - 2] >> 10);
        words[round] = words[round - 16]
            .wrapping_add(s0)
            .wrapping_add(words[round - 7])
            .wrapping_add(s1);
    }
    let mut arrays = loader.arrays.borrow_mut();
    let TypeArrayOop::Int(state) = arrays.get_mut(state)? else {
        return None;
    };
    let state = state.first_chunk_mut::<8>()?;
    let mut vars: [u32; 8] = state.map(|word| word as u32);
    for round in 0..64 {
        let big_s1 = vars[4].rotate_right(6) ^ vars[4].rotate_right(11) ^ vars[4].rotate_right(25);
        let choice = (vars[4] & vars[5]) ^ (!vars[4] & vars[6]);
        let temp1 = vars[7]
            .wrapping_add(big_s1)
            .wrapping_add(choice)
            .wrapping_add(SHA256_K[round])
            .wrapping_add(words[round]);
        let big_s0 = vars[0].rotate_right(2) ^ vars[0].rotate_right(13) ^ vars[0].rotate_right(22);
        let majority = (vars[0] & vars[1]) ^ (vars[0] & vars[2]) ^ (vars[1] & vars[2]);
        let temp2 = big_s0.wrapping_add(majority);
        vars = [
            temp1.wrapping_add(temp2),
            vars[0],
            vars[1],
            vars[2],
            vars[3].wrapping_add(temp1),
            vars[4],
            vars[5],
            vars[6],
        ];
    }
    for (state, word) in state.iter_mut().zip(vars) {
        *state = state.wrapping_add(word as i32);
    }
    Some(())
}

const SHA512_K: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

fn compressSha512<'a>(loader: &ClassLoader<'a>, args: &[Value<'a>]) -> Option<()> {
    let (_input, state, block) = digestBlockArgs(loader, args, b"[J", 128)?;
    let mut words = [0u64; 80];
    for (word, bytes) in words.iter_mut().zip(block.chunks_exact(8)) {
        *word = u64::from_be_bytes([
            bytes[0] as u8, bytes[1] as u8, bytes[2] as u8, bytes[3] as u8,
            bytes[4] as u8, bytes[5] as u8, bytes[6] as u8, bytes[7] as u8,
        ]);
    }
    for round in 16..80 {
        let s0 = words[round - 15].rotate_right(1)
            ^ words[round - 15].rotate_right(8)
            ^ (words[round - 15] >> 7);
        let s1 = words[round - 2].rotate_right(19)
            ^ words[round - 2].rotate_right(61)
            ^ (words[round - 2] >> 6);
        words[round] = words[round - 16]
            .wrapping_add(s0)
            .wrapping_add(words[round - 7])
            .wrapping_add(s1);
    }
    let mut arrays = loader.arrays.borrow_mut();
    let TypeArrayOop::Long(state) = arrays.get_mut(state)? else {
        return None;
    };
    let state = state.first_chunk_mut::<8>()?;
    let mut vars: [u64; 8] = state.map(|word| word as u64);
    for round in 0..80 {
        let big_s1 = vars[4].rotate_right(14) ^ vars[4].rotate_right(18) ^ vars[4].rotate_right(41);
        let choice = (vars[4] & vars[5]) ^ (!vars[4] & vars[6]);
        let temp1 = vars[7]
            .wrapping_add(big_s1)
            .wrapping_add(choice)
            .wrapping_add(SHA512_K[round])
            .wrapping_add(words[round]);
        let big_s0 = vars[0].rotate_right(28) ^ vars[0].rotate_right(34) ^ vars[0].rotate_right(39);
        let majority = (vars[0] & vars[1]) ^ (vars[0] & vars[2]) ^ (vars[1] & vars[2]);
        let temp2 = big_s0.wrapping_add(majority);
        vars = [
            temp1.wrapping_add(temp2),
            vars[0],
            vars[1],
            vars[2],
            vars[3].wrapping_add(temp1),
            vars[4],
            vars[5],
            vars[6],
        ];
    }
    for (state, word) in state.iter_mut().zip(vars) {
        *state = state.wrapping_add(word as i64);
    }
    Some(())
}

/// Fast per-block compressors used by `digestMultiBlock`: the state array
/// handle and input array handle are resolved once by the caller, so each
/// block only pays for the array borrow plus the round function.
fn sha1Block<'a>(loader: &ClassLoader<'a>, state: usize, input: usize, off: i32) -> Option<()> {
    let mut arrays = loader.arrays.borrow_mut();
    let mut words = [0u32; 80];
    {
        let TypeArrayOop::Byte(bytes) = arrays.get(input)? else {
            return None;
        };
        let start = usize::try_from(off).ok()?;
        let block = bytes.get(start..start.checked_add(64)?)?;
        for (word, b) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([b[0] as u8, b[1] as u8, b[2] as u8, b[3] as u8]);
        }
    }
    let TypeArrayOop::Int(state_arr) = arrays.get_mut(state)? else {
        return None;
    };
    let state_arr = state_arr.first_chunk_mut::<5>()?;
    let [mut a, mut b, mut c, mut d, mut e] = state_arr.map(|word| word as u32);
    for round in 0..80 {
        if round >= 16 {
            words[round] =
                (words[round - 3] ^ words[round - 8] ^ words[round - 14] ^ words[round - 16])
                    .rotate_left(1);
        }
        let (mix, constant) = match round {
            0..=19 => ((b & c) | (!b & d), 0x5a827999),
            20..=39 => (b ^ c ^ d, 0x6ed9eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
            _ => (b ^ c ^ d, 0xca62c1d6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(mix)
            .wrapping_add(e)
            .wrapping_add(constant)
            .wrapping_add(words[round]);
        [a, b, c, d, e] = [next, a, b.rotate_left(30), c, d];
    }
    for (state, word) in state_arr.iter_mut().zip([a, b, c, d, e]) {
        *state = state.wrapping_add(word as i32);
    }
    Some(())
}

fn sha256Block<'a>(loader: &ClassLoader<'a>, state: usize, input: usize, off: i32) -> Option<()> {
    let mut arrays = loader.arrays.borrow_mut();
    let mut words = [0u32; 64];
    {
        let TypeArrayOop::Byte(bytes) = arrays.get(input)? else {
            return None;
        };
        let start = usize::try_from(off).ok()?;
        let block = bytes.get(start..start.checked_add(64)?)?;
        for (word, b) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes([b[0] as u8, b[1] as u8, b[2] as u8, b[3] as u8]);
        }
    }
    for round in 16..64 {
        let s0 = words[round - 15].rotate_right(7)
            ^ words[round - 15].rotate_right(18)
            ^ (words[round - 15] >> 3);
        let s1 = words[round - 2].rotate_right(17)
            ^ words[round - 2].rotate_right(19)
            ^ (words[round - 2] >> 10);
        words[round] = words[round - 16]
            .wrapping_add(s0)
            .wrapping_add(words[round - 7])
            .wrapping_add(s1);
    }
    let TypeArrayOop::Int(state_arr) = arrays.get_mut(state)? else {
        return None;
    };
    let state_arr = state_arr.first_chunk_mut::<8>()?;
    let mut vars: [u32; 8] = state_arr.map(|word| word as u32);
    for round in 0..64 {
        let big_s1 = vars[4].rotate_right(6) ^ vars[4].rotate_right(11) ^ vars[4].rotate_right(25);
        let choice = (vars[4] & vars[5]) ^ (!vars[4] & vars[6]);
        let temp1 = vars[7]
            .wrapping_add(big_s1)
            .wrapping_add(choice)
            .wrapping_add(SHA256_K[round])
            .wrapping_add(words[round]);
        let big_s0 = vars[0].rotate_right(2) ^ vars[0].rotate_right(13) ^ vars[0].rotate_right(22);
        let majority = (vars[0] & vars[1]) ^ (vars[0] & vars[2]) ^ (vars[1] & vars[2]);
        let temp2 = big_s0.wrapping_add(majority);
        vars = [
            temp1.wrapping_add(temp2),
            vars[0],
            vars[1],
            vars[2],
            vars[3].wrapping_add(temp1),
            vars[4],
            vars[5],
            vars[6],
        ];
    }
    for (state, word) in state_arr.iter_mut().zip(vars) {
        *state = state.wrapping_add(word as i32);
    }
    Some(())
}

fn sha512Block<'a>(loader: &ClassLoader<'a>, state: usize, input: usize, off: i32) -> Option<()> {
    let mut arrays = loader.arrays.borrow_mut();
    let mut words = [0u64; 80];
    {
        let TypeArrayOop::Byte(bytes) = arrays.get(input)? else {
            return None;
        };
        let start = usize::try_from(off).ok()?;
        let block = bytes.get(start..start.checked_add(128)?)?;
        for (word, b) in words.iter_mut().zip(block.chunks_exact(8)) {
            *word = u64::from_be_bytes([
                b[0] as u8, b[1] as u8, b[2] as u8, b[3] as u8,
                b[4] as u8, b[5] as u8, b[6] as u8, b[7] as u8,
            ]);
        }
    }
    for round in 16..80 {
        let s0 = words[round - 15].rotate_right(1)
            ^ words[round - 15].rotate_right(8)
            ^ (words[round - 15] >> 7);
        let s1 = words[round - 2].rotate_right(19)
            ^ words[round - 2].rotate_right(61)
            ^ (words[round - 2] >> 6);
        words[round] = words[round - 16]
            .wrapping_add(s0)
            .wrapping_add(words[round - 7])
            .wrapping_add(s1);
    }
    let TypeArrayOop::Long(state_arr) = arrays.get_mut(state)? else {
        return None;
    };
    let state_arr = state_arr.first_chunk_mut::<8>()?;
    let mut vars: [u64; 8] = state_arr.map(|word| word as u64);
    for round in 0..80 {
        let big_s1 = vars[4].rotate_right(14) ^ vars[4].rotate_right(18) ^ vars[4].rotate_right(41);
        let choice = (vars[4] & vars[5]) ^ (!vars[4] & vars[6]);
        let temp1 = vars[7]
            .wrapping_add(big_s1)
            .wrapping_add(choice)
            .wrapping_add(SHA512_K[round])
            .wrapping_add(words[round]);
        let big_s0 = vars[0].rotate_right(28) ^ vars[0].rotate_right(34) ^ vars[0].rotate_right(39);
        let majority = (vars[0] & vars[1]) ^ (vars[0] & vars[2]) ^ (vars[1] & vars[2]);
        let temp2 = big_s0.wrapping_add(majority);
        vars = [
            temp1.wrapping_add(temp2),
            vars[0],
            vars[1],
            vars[2],
            vars[3].wrapping_add(temp1),
            vars[4],
            vars[5],
            vars[6],
        ];
    }
    for (state, word) in state_arr.iter_mut().zip(vars) {
        *state = state.wrapping_add(word as i64);
    }
    Some(())
}

const centralDirectoryHeaderSize: usize = 46;

fn zipWord(bytes: &[i8], offset: usize) -> Option<i32> {
    let [a, b] = *bytes.get(offset..offset.checked_add(2)?)? else {
        return None;
    };
    Some(i32::from(u16::from_le_bytes([a as u8, b as u8])))
}

fn centralDirectoryTail(header: &[i8]) -> Option<i32> {
    Some(zipWord(header, 30)? + zipWord(header, 32)?)
}

fn countCentralDirectoryHeaders(mut bytes: &[i8]) -> Option<i32> {
    let mut count = 0;
    while bytes.len() >= centralDirectoryHeaderSize {
        let size = centralDirectoryHeaderSize
            + (zipWord(bytes, 28)? + centralDirectoryTail(bytes)?) as usize;
        bytes = bytes.get(size..)?;
        count += 1;
    }
    Some(count)
}

fn zipField<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> Option<Value<'a>> {
    let (offset, width) = match (name, descriptor) {
        (b"get16", b"([BI)I") => (0, 2),
        (b"get32" | b"CENSIG", b"([BI)J") => (0, 4),
        (b"CENVEM", b"([BI)I") => (4, 2),
        (b"CENVEM_FA", b"([BI)I") => (5, 1),
        (b"CENVER", b"([BI)I") => (6, 2),
        (b"CENFLG", b"([BI)I") => (8, 2),
        (b"CENHOW", b"([BI)I") => (10, 2),
        (b"CENTIM", b"([BI)J") => (12, 4),
        (b"CENCRC", b"([BI)J") => (16, 4),
        (b"CENSIZ", b"([BI)J") => (20, 4),
        (b"CENLEN", b"([BI)J") => (24, 4),
        (b"CENNAM", b"([BI)I") => (28, 2),
        (b"CENEXT", b"([BI)I") => (30, 2),
        (b"CENCOM", b"([BI)I") => (32, 2),
        (b"CENDSK", b"([BI)I") => (34, 2),
        (b"CENATT", b"([BI)I") => (36, 2),
        (b"CENATX", b"([BI)J") => (38, 4),
        (b"CENATX_PERMS", b"([BI)I") => (40, 2),
        (b"CENOFF", b"([BI)J") => (42, 4),
        _ => return None,
    };
    let [Value::PrimitiveArray(handle), Value::Int(base)] = arguments else {
        return None;
    };
    let start = usize::try_from(base.checked_add(offset)?).ok()?;
    let arrays = loader.arrays.borrow();
    let TypeArrayOop::Byte(bytes) = arrays.get(*handle)? else {
        return None;
    };
    if width == 2 {
        return zipWord(bytes, start).map(Value::Int);
    }
    match bytes.get(start..start + width)? {
        [a] => Some(Value::Int(i32::from(*a as u8))),
        [a, b, c, d] => Some(Value::Long(i64::from(u32::from_le_bytes([
            *a as u8, *b as u8, *c as u8, *d as u8,
        ])))),
        _ => None,
    }
}

/// `hash = 31*hash + (a[i] & 0xff)` over `bytes[offset..offset+len]`.
fn byteHash(
    loader: &ClassLoader<'_>,
    handle: usize,
    offset: usize,
    len: usize,
    init: i32,
    signed: bool,
) -> Option<i32> {
    let arrays = loader.arrays.borrow();
    let Some(TypeArrayOop::Byte(bytes)) = arrays.get(handle) else {
        return None;
    };
    let bytes = bytes.get(offset..offset.checked_add(len)?)?;
    Some(bytes.iter().fold(init, |hash, &byte| {
        let value = if signed {
            i32::from(byte)
        } else {
            i32::from(byte as u8)
        };
        hash.wrapping_mul(31).wrapping_add(value)
    }))
}

// ---------------------------------------------------------------------------
// sun.security.provider.ByteArrayAccess — bulk byte[] <-> int[]/long[]
// conversions used by the SHA/MD message digests. The JDK implements these
// with signature-polymorphic VarHandle accesses; each element costs a full
// method-handle dispatch in this VM, so the loops run here instead.
// ---------------------------------------------------------------------------

fn byte_array_access<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    use TypeArrayOop as T;

    fn i32_arg(arguments: &[Value<'_>], index: usize) -> Option<i32> {
        match arguments.get(index) {
            Some(Value::Int(value)) => Some(*value),
            _ => None,
        }
    }
    fn parr(arguments: &[Value<'_>], index: usize) -> Option<usize> {
        match arguments.get(index) {
            Some(Value::PrimitiveArray(handle)) => Some(*handle),
            _ => None,
        }
    }
    /// Borrows two distinct arena slots at once.
    fn pair<'b>(
        arrays: &'b mut [T],
        a: usize,
        b: usize,
    ) -> Option<(&'b mut T, &'b mut T)> {
        if a == b {
            return None;
        }
        Some(if a < b {
            let (left, right) = arrays.split_at_mut(b);
            (&mut left[a], &mut right[0])
        } else {
            let (left, right) = arrays.split_at_mut(a);
            (&mut right[0], &mut left[b])
        })
    }
    fn rd_i32(bytes: &[i8], offset: usize, be: bool) -> Option<i32> {
        let s = bytes.get(offset..offset + 4)?;
        let raw = [s[0] as u8, s[1] as u8, s[2] as u8, s[3] as u8];
        Some(if be {
            i32::from_be_bytes(raw)
        } else {
            i32::from_le_bytes(raw)
        })
    }
    fn rd_i64(bytes: &[i8], offset: usize, be: bool) -> Option<i64> {
        let s = bytes.get(offset..offset + 8)?;
        let raw = [
            s[0] as u8, s[1] as u8, s[2] as u8, s[3] as u8, s[4] as u8, s[5] as u8, s[6] as u8,
            s[7] as u8,
        ];
        Some(if be {
            i64::from_be_bytes(raw)
        } else {
            i64::from_le_bytes(raw)
        })
    }
    fn wr_i32(bytes: &mut [i8], offset: usize, value: i32, be: bool) -> Option<()> {
        let raw = if be {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        bytes
            .get_mut(offset..offset + 4)?
            .copy_from_slice(&raw.map(|b| b as i8));
        Some(())
    }
    fn wr_i64(bytes: &mut [i8], offset: usize, value: i64, be: bool) -> Option<()> {
        let raw = if be {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        bytes
            .get_mut(offset..offset + 8)?
            .copy_from_slice(&raw.map(|b| b as i8));
        Some(())
    }

    let mut arrays = loader.arrays.borrow_mut();
    let arrays: &mut [T] = &mut arrays;
    let done = Ok(Some(Value::Null));

    // Bulk converters share the JDK loop shape: `count = ceil(len / elem)`,
    // element `i` sits at `ofs + i * elem` on the byte side and `ofs + i`
    // on the typed side. `len <= 0` is a no-op.
    match (name, descriptor) {
        // ---- typed[] -> byte[] ----
        (b"i2bLittle" | b"i2bBig", b"([II[BII)V")
        | (b"l2bLittle" | b"l2bBig", b"([JI[BII)V") => {
            let is_long = name[0] == b'l';
            let be = name.ends_with(b"Big");
            let (Some(src_h), Some(dst_h)) = (parr(arguments, 0), parr(arguments, 2)) else {
                return Ok(None);
            };
            let (Some(in_ofs), Some(out_ofs), Some(len)) = (
                i32_arg(arguments, 1),
                i32_arg(arguments, 3),
                i32_arg(arguments, 4),
            ) else {
                return Ok(None);
            };
            if len <= 0 {
                return done;
            }
            let elem = if is_long { 8 } else { 4 };
            let count = (len as usize).div_ceil(elem);
            let Some((src, dst)) = pair(arrays, src_h, dst_h) else {
                return Ok(None);
            };
            match (src, dst) {
                (T::Int(input), T::Byte(output)) if !is_long => {
                    let (in_ofs, out_ofs) = (in_ofs as usize, out_ofs as usize);
                    if in_ofs.checked_add(count).is_none_or(|e| e > input.len())
                        || out_ofs
                            .checked_add(count * 4)
                            .is_none_or(|e| e > output.len())
                    {
                        return Ok(None);
                    }
                    for i in 0..count {
                        wr_i32(output, out_ofs + i * 4, input[in_ofs + i], be)
                            .ok_or_else(|| invalid("range"))?;
                    }
                }
                (T::Long(input), T::Byte(output)) if is_long => {
                    let (in_ofs, out_ofs) = (in_ofs as usize, out_ofs as usize);
                    if in_ofs.checked_add(count).is_none_or(|e| e > input.len())
                        || out_ofs
                            .checked_add(count * 8)
                            .is_none_or(|e| e > output.len())
                    {
                        return Ok(None);
                    }
                    for i in 0..count {
                        wr_i64(output, out_ofs + i * 8, input[in_ofs + i], be)
                            .ok_or_else(|| invalid("range"))?;
                    }
                }
                _ => return Ok(None),
            }
            done
        }
        // ---- byte[] -> typed[] ----
        (b"b2iBig", b"([BI[III)V") | (b"b2lBig" | b"b2lLittle", b"([BI[JII)V") => {
            let is_long = name[1] == b'l';
            let be = name != b"b2lLittle";
            let (Some(src_h), Some(dst_h)) = (parr(arguments, 0), parr(arguments, 2)) else {
                return Ok(None);
            };
            let (Some(in_ofs), Some(out_ofs), Some(len)) = (
                i32_arg(arguments, 1),
                i32_arg(arguments, 3),
                i32_arg(arguments, 4),
            ) else {
                return Ok(None);
            };
            if len <= 0 {
                return done;
            }
            let elem = if is_long { 8 } else { 4 };
            let count = (len as usize).div_ceil(elem);
            let Some((src, dst)) = pair(arrays, src_h, dst_h) else {
                return Ok(None);
            };
            match (src, dst) {
                (T::Byte(input), T::Int(output)) if !is_long => {
                    let (in_ofs, out_ofs) = (in_ofs as usize, out_ofs as usize);
                    if in_ofs
                        .checked_add(count * 4)
                        .is_none_or(|e| e > input.len())
                        || out_ofs.checked_add(count).is_none_or(|e| e > output.len())
                    {
                        return Ok(None);
                    }
                    for i in 0..count {
                        output[out_ofs + i] = rd_i32(input, in_ofs + i * 4, be)
                            .ok_or_else(|| invalid("range"))?;
                    }
                }
                (T::Byte(input), T::Long(output)) if is_long => {
                    let (in_ofs, out_ofs) = (in_ofs as usize, out_ofs as usize);
                    if in_ofs
                        .checked_add(count * 8)
                        .is_none_or(|e| e > input.len())
                        || out_ofs.checked_add(count).is_none_or(|e| e > output.len())
                    {
                        return Ok(None);
                    }
                    for i in 0..count {
                        output[out_ofs + i] = rd_i64(input, in_ofs + i * 8, be)
                            .ok_or_else(|| invalid("range"))?;
                    }
                }
                _ => return Ok(None),
            }
            done
        }
        // ---- single-element / fixed-count variants ----
        (b"i2bLittle4" | b"i2bBig4", b"(I[BI)V") => {
            let be = name == b"i2bBig4";
            let (Some(value), Some(out_h), Some(out_ofs)) = (
                i32_arg(arguments, 0),
                parr(arguments, 1),
                i32_arg(arguments, 2),
            ) else {
                return Ok(None);
            };
            let Some(T::Byte(output)) = arrays.get_mut(out_h) else {
                return Ok(None);
            };
            wr_i32(output, out_ofs as usize, value, be)
                .ok_or_else(|| invalid("range"))?;
            done
        }
        (b"b2iBig64", b"([BI[I)V") => {
            let (Some(src_h), Some(in_ofs), Some(dst_h)) = (
                parr(arguments, 0),
                i32_arg(arguments, 1),
                parr(arguments, 2),
            ) else {
                return Ok(None);
            };
            let Some((src, dst)) = pair(arrays, src_h, dst_h) else {
                return Ok(None);
            };
            let (T::Byte(input), T::Int(output)) = (src, dst) else {
                return Ok(None);
            };
            let in_ofs = in_ofs as usize;
            if in_ofs.checked_add(64).is_none_or(|e| e > input.len()) || output.len() < 16 {
                return Ok(None);
            }
            for i in 0..16 {
                output[i] = rd_i32(input, in_ofs + i * 4, true)
                    .ok_or_else(|| invalid("range"))?;
            }
            done
        }
        (b"b2lBig128", b"([BI[J)V") => {
            let (Some(src_h), Some(in_ofs), Some(dst_h)) = (
                parr(arguments, 0),
                i32_arg(arguments, 1),
                parr(arguments, 2),
            ) else {
                return Ok(None);
            };
            let Some((src, dst)) = pair(arrays, src_h, dst_h) else {
                return Ok(None);
            };
            let (T::Byte(input), T::Long(output)) = (src, dst) else {
                return Ok(None);
            };
            let in_ofs = in_ofs as usize;
            if in_ofs.checked_add(128).is_none_or(|e| e > input.len()) || output.len() < 16 {
                return Ok(None);
            }
            for i in 0..16 {
                output[i] = rd_i64(input, in_ofs + i * 8, true)
                    .ok_or_else(|| invalid("range"))?;
            }
            done
        }
        _ => Ok(None),
    }
}

/// Handles a single static JDK helper call. Returns `Ok(None)` when the
/// (owner, name, descriptor) triple is not one of the fast-pathed methods.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let value = match (owner, name, descriptor) {
        (b"java/lang/CharacterDataLatin1", b"equalsIgnoreCase", b"(BB)Z") => {
            let a = int_arg(arguments, 0)?;
            let b = int_arg(arguments, 1)?;
            let upper = a & 0xdf;
            Value::Int(i32::from(
                a == b || (latin1UppercaseRange(upper) && upper == b & 0xdf),
            ))
        }
        (b"java/lang/StringLatin1" | b"java/lang/StringUTF16", b"indexOf" | b"lastIndexOf", _)
        | (b"java/lang/StringUTF16", b"indexOfLatin1" | b"lastIndexOfLatin1", _) => {
            return Ok(stringSearch(loader, owner, name, descriptor, arguments));
        }
        // java.lang.Byte
        (b"java/lang/Byte", b"toUnsignedInt", b"(B)I") => Value::Int(int_arg(arguments, 0)? & 0xff),
        (b"java/lang/Byte", b"toUnsignedLong", b"(B)J") => {
            Value::Long((int_arg(arguments, 0)? & 0xff) as i64)
        }
        // java.lang.Integer unsigned helpers (hot in comparisons/sorts)
        (b"java/lang/Integer", b"toUnsignedLong", b"(I)J") => {
            Value::Long((int_arg(arguments, 0)? as u32) as i64)
        }
        (b"java/lang/Integer", b"compareUnsigned", b"(II)I") => {
            let a = int_arg(arguments, 0)? as u32;
            let b = int_arg(arguments, 1)? as u32;
            Value::Int(a.cmp(&b) as i32)
        }
        (b"java/lang/Integer", b"divideUnsigned", b"(II)I") => {
            let a = int_arg(arguments, 0)? as u32;
            let b = int_arg(arguments, 1)? as u32;
            if b == 0 {
                // Let the real JDK implementation throw ArithmeticException.
                return Ok(None);
            }
            Value::Int((a / b) as i32)
        }
        (b"java/lang/Integer", b"remainderUnsigned", b"(II)I") => {
            let a = int_arg(arguments, 0)? as u32;
            let b = int_arg(arguments, 1)? as u32;
            if b == 0 {
                return Ok(None);
            }
            Value::Int((a % b) as i32)
        }
        // java.util.zip.ZipUtils little-endian readers
        (b"java/util/zip/ZipUtils", _, _) => {
            return Ok(zipField(loader, name, descriptor, arguments));
        }
        (b"java/util/Arrays", b"mismatch", b"([BII[BII)I") => {
            return Ok(byteRangeMismatch(loader, arguments));
        }
        (b"java/util/zip/ZipFile$Source", b"isMetaName", b"([BII)Z") => {
            return Ok(zipName(loader, arguments, true));
        }
        (b"java/util/zip/ZipFile$Source", b"countCENHeaders", b"([B)I") => {
            let [Value::PrimitiveArray(handle)] = arguments else {
                return Ok(None);
            };
            let arrays = loader.arrays.borrow();
            let Some(TypeArrayOop::Byte(bytes)) = arrays.get(*handle) else {
                return Ok(None);
            };
            return Ok(countCentralDirectoryHeaders(bytes).map(Value::Int));
        }
        // java.lang.Short
        (b"java/lang/Short", b"toUnsignedInt", b"(S)I") => {
            Value::Int(int_arg(arguments, 0)? & 0xffff)
        }
        // jdk.internal.util.Preconditions — fast path only when in bounds;
        // out-of-bounds falls through to the real implementation which
        // throws the proper IndexOutOfBoundsException via the formatter.
        (
            b"jdk/internal/util/Preconditions",
            b"checkIndex",
            b"(IILjava/util/function/BiFunction;)I",
        ) => {
            let index = int_arg(arguments, 0)?;
            let length = int_arg(arguments, 1)?;
            if index >= 0 && index < length {
                return Ok(Some(Value::Int(index)));
            }
            return Ok(None);
        }
        // sun.security.provider.ByteArrayAccess — digest byte/int/long
        // conversions (void methods; see interpreter's `)V` handling).
        (b"sun/security/provider/ByteArrayAccess", _, _) => {
            return byte_array_access(loader, name, descriptor, arguments);
        }
        // jdk.internal.util.ArraysSupport
        // hashCodeOfUnsigned(byte[] a, int off, int len, int init)
        (b"jdk/internal/util/ArraysSupport", b"hashCodeOfUnsigned", b"([BIII)I") => {
            let handle = match arguments.first().copied() {
                Some(Value::PrimitiveArray(handle)) => handle,
                _ => return Ok(None),
            };
            let offset = int_arg(arguments, 1)? as usize;
            let len = int_arg(arguments, 2)? as usize;
            let init = int_arg(arguments, 3)?;
            return Ok(byteHash(loader, handle, offset, len, init, false).map(Value::Int));
        }
        (
            b"jdk/internal/util/ArraysSupport",
            b"vectorizedHashCode",
            b"(Ljava/lang/Object;IIII)I",
        ) => {
            let handle = match arguments.first().copied() {
                Some(Value::PrimitiveArray(handle)) => handle,
                _ => return Ok(None),
            };
            let offset = int_arg(arguments, 1)? as usize;
            let len = int_arg(arguments, 2)? as usize;
            let init = int_arg(arguments, 3)?;
            let kind = int_arg(arguments, 4)?;
            if !matches!(kind, 4 | 8) {
                return Ok(None);
            }
            return Ok(byteHash(loader, handle, offset, len, init, kind == 8).map(Value::Int));
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
