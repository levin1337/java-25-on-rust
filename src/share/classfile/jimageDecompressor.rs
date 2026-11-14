/*
 * Port of OpenJDK jdk.internal.jimage.decompressor
 * (java.base/jdk/internal/jimage/decompressor package)
 *
 * Decompresses resources stored inside a JImage (lib/modules) file:
 * - "zip": zlib/deflate compressed payloads
 * - "compact-cp": constant-pool string sharing for .class resources
 *
 * Compressed resources are wrapped in a sequence of 29-byte headers
 * (magic 0xCAFEFAFA, sizes in the image's native byte order — little
 * endian on all supported targets) naming the decompressor plugin.
 */

use super::classFileParser::{decodeString, encodeModifiedUtf8};
use std::io::{self, Error};

pub const COMPRESSED_RESOURCE_MAGIC: u32 = 0xCAFE_FFFA;
pub const COMPRESSED_RESOURCE_HEADER_SIZE: usize = 29;

const TAG_EXTERNALIZED_STRING: u8 = 23;
const TAG_EXTERNALIZED_STRING_DESCRIPTOR: u8 = 25;

const CONSTANT_Utf8: u8 = 1;
const CONSTANT_Long: u8 = 5;
const CONSTANT_Double: u8 = 6;

/// Fixed payload sizes per constant pool tag, mirroring
/// StringSharingDecompressor.SIZES (index = tag, 0 means "not copyable").
const SIZES: [usize; 21] = [
    0, // 0 unused
    0, // 1 CONSTANT_Utf8 (variable)
    0, // 2 unused
    4, // 3 CONSTANT_Integer
    4, // 4 CONSTANT_Float
    8, // 5 CONSTANT_Long
    8, // 6 CONSTANT_Double
    2, // 7 CONSTANT_Class
    2, // 8 CONSTANT_String
    4, // 9 CONSTANT_Fieldref
    4, // 10 CONSTANT_Methodref
    4, // 11 CONSTANT_InterfaceMethodref
    4, // 12 CONSTANT_NameAndType
    0, // 13 unused
    0, // 14 unused
    3, // 15 CONSTANT_MethodHandle
    2, // 16 CONSTANT_MethodType
    0, // 17 CONSTANT_Dynamic (not supported by compact-cp)
    4, // 18 CONSTANT_InvokeDynamic
    2, // 19 CONSTANT_Module
    2, // 20 CONSTANT_Package
];

/// Provider for strings stored in the jimage strings table.
/// `get_string(offset)` must return the MUTF-8 decoded contents.
pub trait StringsProvider {
    fn get_string(&self, offset: u32) -> Option<String>;
}

#[derive(Debug)]
struct CompressedResourceHeader {
    decompressor_name_offset: u32,
    is_terminal: bool,
}

impl CompressedResourceHeader {
    /// Reads the 29-byte header at the start of `content`.
    /// Returns None when the content is not a compressed resource.
    fn read(content: &[u8]) -> Option<Self> {
        if content.len() < COMPRESSED_RESOURCE_HEADER_SIZE {
            return None;
        }
        let magic = u32::from_le_bytes(content[0..4].try_into().ok()?);
        if magic != COMPRESSED_RESOURCE_MAGIC {
            return None;
        }
        let decompressor_name_offset = u32::from_le_bytes(content[20..24].try_into().ok()?);
        let is_terminal = content[28] == 1;
        Some(Self {
            decompressor_name_offset,
            is_terminal,
        })
    }
}

/// Decompresses `content`, which may be wrapped in one or more
/// compressed-resource headers (nested compression).
pub fn decompress_resource<P: StringsProvider>(
    provider: &P,
    mut content: Vec<u8>,
) -> io::Result<Vec<u8>> {
    while let Some(header) = CompressedResourceHeader::read(&content) {
        let plugin = provider
            .get_string(header.decompressor_name_offset)
            .ok_or_else(|| Error::other("JImage decompressor name not found"))?;
        let _ = header.is_terminal;
        content = match plugin.as_str() {
            "zip" => decompress_zip(&content)?,
            "compact-cp" => decompress_compact_cp(provider, &content)?,
            other => {
                return Err(Error::other(format!(
                    "JImage decompressor plugin not found: {other}"
                )));
            }
        };
    }
    Ok(content)
}

fn decompress_zip(content: &[u8]) -> io::Result<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let payload = &content[COMPRESSED_RESOURCE_HEADER_SIZE..];
    let mut decoder = ZlibDecoder::new(payload);
    let mut out = Vec::with_capacity(payload.len() * 2);
    match decoder.read_to_end(&mut out) {
        Ok(_) => Ok(out),
        Err(zlib_error) => {
            // Some producers emit raw deflate streams; retry without the
            // zlib wrapper before giving up.
            use flate2::read::DeflateDecoder;
            let mut decoder = DeflateDecoder::new(payload);
            let mut raw = Vec::with_capacity(payload.len() * 2);
            decoder.read_to_end(&mut raw).map_err(|_| zlib_error)?;
            Ok(raw)
        }
    }
}

/// Reads a CompressIndexes-encoded integer: either a plain 4-byte big-endian
/// int or a compressed form headed by a byte with the top bit set.
fn read_compressed_index(stream: &[u8], pos: &mut usize) -> io::Result<u32> {
    let invalid = || Error::other("Truncated compressed index");
    let header = *stream.get(*pos).ok_or_else(invalid)?;
    *pos += 1;
    if header & 0x80 == 0 {
        let bytes = stream.get(*pos..*pos + 3).ok_or_else(invalid)?;
        *pos += 3;
        return Ok(u32::from_be_bytes([header, bytes[0], bytes[1], bytes[2]]));
    }
    let size = usize::from((header >> 5) & 0x03);
    if size > 3 {
        return Err(Error::other("Invalid compressed index header"));
    }
    let mut value = u32::from(header & 0x1f);
    for _ in 1..size {
        let byte = *stream.get(*pos).ok_or_else(invalid)?;
        *pos += 1;
        value = (value << 8) | u32::from(byte);
    }
    Ok(value)
}

fn decompress_flow(bytes: &[u8]) -> io::Result<Vec<u32>> {
    let mut values = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let header = bytes[pos];
        let size = if header & 0x80 != 0 {
            usize::from((header >> 5) & 0x03)
        } else {
            4
        };
        if size == 0 || pos + size > bytes.len() {
            return Err(Error::other("Invalid compressed index flow"));
        }
        let mut value = u32::from(if header & 0x80 != 0 {
            header & 0x1f
        } else {
            header
        });
        for &byte in &bytes[pos + 1..pos + size] {
            value = (value << 8) | u32::from(byte);
        }
        values.push(value);
        pos += size;
    }
    Ok(values)
}

struct Stream<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Stream<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let slice = self
            .data
            .get(self.pos..self.pos + count)
            .ok_or_else(|| Error::other("Truncated compact-cp resource"))?;
        self.pos += count;
        Ok(slice)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn compressed_index(&mut self) -> io::Result<u32> {
        read_compressed_index(self.data, &mut self.pos)
    }

    /// Reads a modified-UTF-8 string prefixed by a big-endian u16 byte length
    /// (DataInputStream.readUTF) and returns its MUTF-8 encoding.
    fn read_utf_bytes(&mut self) -> io::Result<Vec<u8>> {
        let length = usize::from(self.u16()?);
        let bytes = self.take(length)?;
        // Validate decodability, mirroring readUTF failure on bad MUTF-8.
        decodeString(bytes)?;
        Ok(bytes.to_vec())
    }
}

/// StringSharingDecompressor.normalize: rebuilds a standard class file
/// constant pool from the compact-cp representation, resolving
/// externalized strings and descriptors through the jimage strings table.
fn decompress_compact_cp<P: StringsProvider>(provider: &P, content: &[u8]) -> io::Result<Vec<u8>> {
    let mut stream = Stream {
        data: content,
        pos: COMPRESSED_RESOURCE_HEADER_SIZE,
    };
    let mut out = Vec::with_capacity(content.len() + 64);

    // Copy magic + version header (8 bytes) and constant pool count.
    out.extend_from_slice(stream.take(8)?);
    let count = usize::from(stream.u16()?);
    out.extend_from_slice(&count.to_be_bytes()[..2]);

    let mut index = 1usize;
    while index < count {
        let tag = stream.u8()?;
        match tag {
            CONSTANT_Utf8 => {
                out.push(tag);
                out.extend_from_slice(&stream.read_utf_bytes()?);
            }
            TAG_EXTERNALIZED_STRING => {
                let string_index = stream.compressed_index()?;
                let text = provider
                    .get_string(string_index)
                    .ok_or_else(|| Error::other("Externalized string not found"))?;
                let encoded = encodeModifiedUtf8(&text);
                out.push(CONSTANT_Utf8);
                out.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
                out.extend_from_slice(&encoded);
            }
            TAG_EXTERNALIZED_STRING_DESCRIPTOR => {
                let descriptor = reconstruct_descriptor(provider, &mut stream)?;
                out.push(CONSTANT_Utf8);
                out.extend_from_slice(&descriptor);
            }
            CONSTANT_Long | CONSTANT_Double => {
                index += 1;
                out.push(tag);
                out.extend_from_slice(stream.take(8)?);
            }
            _ => {
                let size = *SIZES
                    .get(usize::from(tag))
                    .ok_or_else(|| Error::other(format!("Invalid constant pool tag {tag}")))?;
                out.push(tag);
                out.extend_from_slice(stream.take(size)?);
            }
        }
        index += 1;
    }

    // Copy the remainder of the class file verbatim.
    out.extend_from_slice(&content[stream.pos..]);
    Ok(out)
}

/// Reconstructs an externalized descriptor: the strings table holds a
/// skeleton descriptor ("L;" style) plus a compressed flow of
/// (package, class) string index pairs. Returns a length-prefixed
/// MUTF-8 encoding ready to be written as a CONSTANT_Utf8 payload.
fn reconstruct_descriptor<P: StringsProvider>(
    provider: &P,
    stream: &mut Stream<'_>,
) -> io::Result<Vec<u8>> {
    let descriptor_index = stream.compressed_index()?;
    let formatted = provider
        .get_string(descriptor_index)
        .ok_or_else(|| Error::other("Externalized descriptor not found"))?;
    let encoded_desc = encodeModifiedUtf8(&formatted);

    let indexes_length = usize::try_from(stream.compressed_index()?)
        .map_err(|_| Error::other("Invalid descriptor index count"))?;
    let index_bytes = stream.take(indexes_length)?;
    let indices = decompress_flow(index_bytes)?;

    let mut arg_index = 0usize;
    let mut body = Vec::with_capacity(encoded_desc.len() + 32);
    let mut next_index = || -> io::Result<u32> {
        let value = *indices
            .get(arg_index)
            .ok_or_else(|| Error::other("Missing descriptor string index"))?;
        arg_index += 1;
        Ok(value)
    };

    for &byte in &encoded_desc {
        if byte == b'L' {
            body.push(b'L');
            let package_index = next_index()?;
            let package = provider
                .get_string(package_index)
                .ok_or_else(|| Error::other("Descriptor package string not found"))?;
            if !package.is_empty() {
                body.extend_from_slice(&encodeModifiedUtf8(&format!("{package}/")));
            }
            let class_index = next_index()?;
            let class = provider
                .get_string(class_index)
                .ok_or_else(|| Error::other("Descriptor class string not found"))?;
            body.extend_from_slice(&encodeModifiedUtf8(&class));
        } else {
            body.push(byte);
        }
    }

    // Emit u16 length prefix + MUTF-8 payload (equivalent of writeUTF).
    if body.len() > u16::MAX as usize {
        return Err(Error::other("Descriptor too long"));
    }
    let mut out = Vec::with_capacity(body.len() + 2);
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}
