/*
 * OpenJDK 25 HotSpot jfr/writers/jfrEncoders.hpp + jfrEncoding.hpp port
 *
 * Encoding policies:
 *  - BigEndianEncoderImpl: raw big-endian ("Java") byte order.
 *  - Varint128EncoderImpl: msb(it) 128-bit encoding — 1 extension bit +
 *    7 value bits per byte, least significant byte order (little-endian
 *    7-bit groups, 0x80 continuation).
 */

pub const EXT_BIT: u8 = 0x80;

fn less_than_128(v: u64) -> bool {
    v & !((EXT_BIT as u64) - 1) == 0
}

// ---- Big endian ----

pub fn be_write_u1(value: u8, dest: &mut [u8]) -> usize {
    dest[0] = value;
    1
}

pub fn be_write_u2(value: u16, dest: &mut [u8]) -> usize {
    dest[0..2].copy_from_slice(&value.to_be_bytes());
    2
}

pub fn be_write_u4(value: u32, dest: &mut [u8]) -> usize {
    dest[0..4].copy_from_slice(&value.to_be_bytes());
    4
}

pub fn be_write_u8(value: u64, dest: &mut [u8]) -> usize {
    dest[0..8].copy_from_slice(&value.to_be_bytes());
    8
}

pub fn be_write_i1(value: i8, dest: &mut [u8]) -> usize {
    be_write_u1(value as u8, dest)
}

pub fn be_write_i2(value: i16, dest: &mut [u8]) -> usize {
    be_write_u2(value as u16, dest)
}

pub fn be_write_i4(value: i32, dest: &mut [u8]) -> usize {
    be_write_u4(value as u32, dest)
}

pub fn be_write_i8(value: i64, dest: &mut [u8]) -> usize {
    be_write_u8(value as u64, dest)
}

// ---- Varint128 ----

pub fn varint_encode_u64(value: u64, dest: &mut [u8]) -> usize {
    // 9 bytes maximum for u64
    let mut v = value;
    let mut i = 0;
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            dest[i] = b;
            return i + 1;
        }
        dest[i] = b | EXT_BIT;
        i += 1;
        if i == 8 {
            dest[i] = (v & 0x7f) as u8 | EXT_BIT;
            // v should fit in remaining bits; write final byte
            dest[i + 1] = ((v >> 7) & 0x7f) as u8;
            return i + 2;
        }
    }
}

// Exact C++ semantics: at most 9 bytes, last byte carries bits 56..63.
pub fn varint128_encode(value: u64, dest: &mut [u8]) -> usize {
    if value < 128 {
        dest[0] = value as u8;
        return 1;
    }
    let mut i = 0usize;
    let mut shift = 0u32;
    loop {
        let byte = ((value >> shift) & 0x7f) as u8;
        shift += 7;
        if value >> shift == 0 {
            dest[i] = byte;
            return i + 1;
        }
        dest[i] = byte | EXT_BIT;
        i += 1;
        if i == 8 {
            dest[i] = (value >> 56) as u8;
            return 9;
        }
    }
}

// Padded varint: fixed width per type (u16->2, u32->4, u64->8 bytes),
// all but last byte carry the extension bit.
pub fn varint128_encode_padded(value: u64, size: usize, dest: &mut [u8]) -> usize {
    match size {
        1 => {
            dest[0] = value as u8;
            1
        }
        2 => {
            dest[0] = (value as u8) | EXT_BIT;
            dest[1] = (value >> 7) as u8;
            2
        }
        4 => {
            dest[0] = (value as u8) | EXT_BIT;
            dest[1] = ((value >> 7) as u8) | EXT_BIT;
            dest[2] = ((value >> 14) as u8) | EXT_BIT;
            dest[3] = (value >> 21) as u8;
            4
        }
        8 => {
            dest[0] = (value as u8) | EXT_BIT;
            dest[1] = ((value >> 7) as u8) | EXT_BIT;
            dest[2] = ((value >> 14) as u8) | EXT_BIT;
            dest[3] = ((value >> 21) as u8) | EXT_BIT;
            dest[4] = ((value >> 28) as u8) | EXT_BIT;
            dest[5] = ((value >> 35) as u8) | EXT_BIT;
            dest[6] = ((value >> 42) as u8) | EXT_BIT;
            dest[7] = ((value >> 49) & 0x7f) as u8;
            8
        }
        _ => panic!("unsupported padded varint size"),
    }
}

pub fn varint128_size_in_bytes(value: u64) -> usize {
    if value < 128 {
        return 1;
    }
    let mut v = value >> 7;
    let mut size = 2;
    while v >= 128 {
        v >>= 7;
        size += 1;
        if size == 9 {
            return 9;
        }
    }
    size
}

// ---- Decoding (for tests / verification) ----

pub fn be_read_u1(src: &[u8]) -> u8 {
    src[0]
}

pub fn be_read_u2(src: &[u8]) -> u16 {
    u16::from_be_bytes([src[0], src[1]])
}

pub fn be_read_u4(src: &[u8]) -> u32 {
    u32::from_be_bytes([src[0], src[1], src[2], src[3]])
}

pub fn be_read_u8(src: &[u8]) -> u64 {
    u64::from_be_bytes(src[0..8].try_into().unwrap())
}

pub fn varint128_decode(src: &[u8]) -> (u64, usize) {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    let mut i = 0usize;
    loop {
        let b = src[i];
        if i == 8 {
            // last byte: full 8 bits, no continuation
            result |= (b as u64) << shift;
            return (result, 9);
        }
        result |= ((b & 0x7f) as u64) << shift;
        i += 1;
        if b & EXT_BIT == 0 {
            return (result, i);
        }
        shift += 7;
    }
}
