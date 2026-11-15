/*
 * Port of OpenJDK 25 HotSpot CompressedStream / CompressedReadStream /
 * CompressedWriteStream and UNSIGNED5 (share/code/compressedStream.hpp/.cpp,
 * share/utilities/unsigned5.hpp).
 *
 * UNSIGNED5 is the modified Pack200 unsigned integer coding: each byte
 * carries a value >= 1 (byte 0 is excluded); bytes below X+L are terminal
 * "low" codes, bytes >= X+L are "high" continuation codes contributing
 * 6 bits per position.
 */

pub const MAX_LENGTH: usize = 5;
pub const MAX_VALUE: u32 = u32::MAX;

const X: u32 = 1;
const H: u32 = 1 << 6; // 64
const LG_H: u32 = 6;
const MAX_B: u32 = 255;
const L: u32 = (MAX_B + 1) - X - H; // 191

/// Decodes one unsigned 32-bit value from `data` at `pos`, advancing `pos`.
/// Returns `None` on malformed input (excluded byte / overrun).
pub fn read_uint(data: &[u8], pos: &mut usize) -> Option<u32> {
    if *pos >= data.len() {
        return None;
    }
    let b0 = data[*pos] as u32;
    if b0 < X {
        return None;
    }
    let mut sum = b0 - X;
    if sum < L {
        *pos += 1;
        return Some(sum);
    }
    let mut lg_h_i = LG_H;
    for i in 1..MAX_LENGTH {
        if *pos + i >= data.len() {
            return None;
        }
        let b_i = data[*pos + i] as u32;
        if b_i < X {
            return None;
        }
        sum = sum.wrapping_add((b_i - X) << lg_h_i);
        if b_i < X + L || i == MAX_LENGTH - 1 {
            *pos += i + 1;
            return Some(sum);
        }
        lg_h_i += LG_H;
    }
    None
}

/// Writes one unsigned 32-bit value, appending to `out`.
pub fn write_uint(value: u32, out: &mut Vec<u8>) {
    if value < L {
        out.push((X + value) as u8);
        return;
    }
    let mut sum = value;
    let mut i = 0usize;
    loop {
        if sum < L || i == MAX_LENGTH - 1 {
            out.push((X + sum) as u8);
            return;
        }
        sum -= L;
        let b_i = X + L + (sum % H);
        out.push(b_i as u8);
        sum >>= LG_H;
        i += 1;
    }
}

/// Number of bytes the encoding of `value` occupies.  Mirrors
/// `write_uint` exactly: one terminal byte for `value < L`, otherwise
/// one high byte per 6-bit digit plus a final terminal byte.
pub fn encoded_length(value: u32) -> usize {
    if value < L {
        return 1;
    }
    let mut sum = value;
    let mut len = 0usize;
    let mut i = 0usize;
    loop {
        if sum < L || i == MAX_LENGTH - 1 {
            return len + 1;
        }
        sum = sum.saturating_sub(L);
        sum >>= LG_H;
        len += 1;
        i += 1;
    }
}

/// `UNSIGNED5::encode_sign`: zig-zag style encoding of a signed value.
#[inline]
pub fn encode_sign(value: i32) -> u32 {
    ((value as u32) << 1) ^ ((value >> 31) as u32)
}

/// `UNSIGNED5::decode_sign`.
#[inline]
pub fn decode_sign(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}

/// Read-only view over a compressed stream.
#[derive(Clone, Debug)]
pub struct CompressedReadStream<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> CompressedReadStream<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn new_at(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos: pos.min(data.len()),
        }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn read(&mut self) -> Option<u8> {
        if self.eof() {
            return None;
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Some(b)
    }

    pub fn read_bool(&mut self) -> Option<bool> {
        self.read().map(|b| b != 0)
    }

    pub fn read_byte(&mut self) -> Option<i8> {
        self.read().map(|b| b as i8)
    }

    pub fn read_int(&mut self) -> Option<u32> {
        read_uint(self.data, &mut self.pos)
    }

    pub fn read_signed_int(&mut self) -> Option<i32> {
        self.read_int().map(decode_sign)
    }

    pub fn read_char(&mut self) -> Option<u16> {
        self.read_int().map(|v| v as u16)
    }

    pub fn read_short(&mut self) -> Option<i16> {
        self.read_signed_int().map(|v| v as i16)
    }
}

/// Write-only compressed stream with growable backing buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompressedWriteStream {
    pub buf: Vec<u8>,
}

impl Default for CompressedWriteStream {
    fn default() -> Self {
        Self::with_capacity(32)
    }
}

impl CompressedWriteStream {
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    pub fn position(&self) -> usize {
        self.buf.len()
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_buffer(self) -> Vec<u8> {
        self.buf
    }

    #[inline]
    pub fn write(&mut self, b: u8) {
        self.buf.push(b);
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write(value as u8);
    }

    pub fn write_byte(&mut self, value: i8) {
        self.write(value as u8);
    }

    pub fn write_char(&mut self, value: u16) {
        self.write_int(value as u32);
    }

    pub fn write_short(&mut self, value: i16) {
        self.write_signed_int(value as i32);
    }

    pub fn write_int(&mut self, value: u32) {
        write_uint(value, &mut self.buf);
    }

    pub fn write_signed_int(&mut self, value: i32) {
        self.write_int(encode_sign(value));
    }

    pub fn write_float(&mut self, value: f32) {
        self.write_int(value.to_bits().reverse_bits());
    }

    pub fn write_double(&mut self, value: f64) {
        let bits = value.to_bits();
        self.write_int((bits >> 32) as u32);
        self.write_int(bits as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint_round_trip() {
        for v in [
            0u32,
            1,
            2,
            63,
            64,
            100,
            190,
            191,
            192,
            193,
            255,
            256,
            1000,
            4096,
            65535,
            65536,
            1_000_000,
            70_000,
            u32::MAX,
        ] {
            let mut buf = Vec::new();
            write_uint(v, &mut buf);
            assert_eq!(buf.len(), encoded_length(v), "len mismatch for {}", v);
            let mut pos = 0usize;
            let got = read_uint(&buf, &mut pos).unwrap();
            assert_eq!(got, v, "round trip failed for {}", v);
            assert_eq!(pos, buf.len());
        }
    }

    #[test]
    fn sign_round_trip() {
        for v in [
            0i32,
            1,
            -1,
            63,
            -64,
            1000,
            -1000,
            i32::MAX,
            i32::MIN,
            12345,
            -12345,
        ] {
            let enc = encode_sign(v);
            assert_eq!(decode_sign(enc), v);
        }
    }

    #[test]
    fn stream_int_round_trip() {
        let mut w = CompressedWriteStream::new();
        w.write_signed_int(-5);
        w.write_int(70000);
        w.write_bool(true);
        w.write_byte(-3);
        w.write_char(0xABCD);
        w.write_short(-1234);
        w.write_float(1.5);
        w.write_double(-2.25);
        let data = w.into_buffer();

        let mut r = CompressedReadStream::new(&data);
        assert_eq!(r.read_signed_int(), Some(-5));
        assert_eq!(r.read_int(), Some(70000));
        assert_eq!(r.read_bool(), Some(true));
        assert_eq!(r.read_byte(), Some(-3));
        assert_eq!(r.read_char(), Some(0xABCD));
        assert_eq!(r.read_short(), Some(-1234));
        assert_eq!(r.read_int(), Some(1.5f32.to_bits().reverse_bits()));
        // double: high word then low word
        let hi = r.read_int().unwrap() as u64;
        let lo = r.read_int().unwrap() as u64;
        assert_eq!((hi << 32) | lo, (-2.25f64).to_bits());
        assert!(r.eof());
    }

    #[test]
    fn malformed_input_rejected() {
        // byte 0 is excluded by UNSIGNED5
        assert_eq!(read_uint(&[0x00], &mut 0), None);
        // truncated multi-byte sequence
        let mut buf = Vec::new();
        write_uint(u32::MAX, &mut buf);
        buf.pop();
        assert_eq!(read_uint(&buf, &mut 0), None);
        // empty input
        assert_eq!(read_uint(&[], &mut 0), None);
        let mut r = CompressedReadStream::new(&[]);
        assert_eq!(r.read_int(), None);
        assert_eq!(r.read(), None);
    }

    #[test]
    fn new_at_offsets() {
        let mut w = CompressedWriteStream::new();
        w.write_int(11);
        w.write_int(22);
        let data = w.into_buffer();
        let mut r = CompressedReadStream::new_at(&data, 1);
        // skip first encoded int (1 -> single byte 0x02)
        assert_eq!(r.read_int(), Some(22));
    }

    #[test]
    fn boundary_190_191() {
        // UNSIGNED5: values < L (191) fit in one byte (stored +1);
        // 191 is the first two-byte value.
        for v in [189u32, 190] {
            let mut buf = Vec::new();
            write_uint(v, &mut buf);
            assert_eq!(buf.len(), 1, "value {} must be single-byte", v);
        }
        let mut buf = Vec::new();
        write_uint(191, &mut buf);
        assert_eq!(buf.len(), 2);
        let mut pos = 0usize;
        assert_eq!(read_uint(&buf, &mut pos), Some(191));
        for v in [192u32, 1000, 1 << 20, u32::MAX] {
            let mut buf = Vec::new();
            write_uint(v, &mut buf);
            let mut pos = 0usize;
            assert_eq!(read_uint(&buf, &mut pos), Some(v), "round trip {}", v);
            assert_eq!(pos, buf.len());
        }
    }

    #[test]
    fn encoded_length_boundaries() {
        assert_eq!(encoded_length(0), 1);
        assert_eq!(encoded_length(190), 1);
        assert_eq!(encoded_length(191), 2);
        assert_eq!(encoded_length(u32::MAX), MAX_LENGTH);
        // Length is monotonic in value.
        let mut prev = 0usize;
        for v in [1u32, 100, 190, 191, 500, 5000, 100_000, u32::MAX] {
            let l = encoded_length(v);
            assert!(l >= prev);
            prev = l;
        }
    }

    #[test]
    fn malformed_continuations() {
        // A lone high-marker byte (>= 192) always promises continuation
        // bytes; truncation must be rejected.
        for first in [192u8, 200, 255] {
            assert_eq!(read_uint(&[first], &mut 0), None, "byte {}", first);
        }
        // 255 needs four continuation bytes (MAX_LENGTH - 1); two is
        // still truncated.
        assert_eq!(read_uint(&[255u8, 255, 255], &mut 0), None);
        // A zero byte anywhere is excluded.
        assert_eq!(read_uint(&[0u8], &mut 0), None);
        let mut buf = Vec::new();
        write_uint(1000, &mut buf);
        buf.push(0);
        let mut pos = 0usize;
        assert_eq!(read_uint(&buf, &mut pos), Some(1000));
        assert_eq!(read_uint(&buf, &mut pos), None);
    }

    #[test]
    fn concatenated_multibyte() {
        let mut w = CompressedWriteStream::new();
        for v in [0u32, 191, 192, 500, u32::MAX] {
            w.write_int(v);
        }
        let data = w.into_buffer();
        let mut r = CompressedReadStream::new(&data);
        for expected in [0u32, 191, 192, 500, u32::MAX] {
            assert_eq!(r.read_int(), Some(expected));
        }
        assert!(r.eof());
    }

    #[test]
    fn seek_position_reads() {
        let mut w = CompressedWriteStream::new();
        w.write_int(42);
        w.write_int(5000);
        w.write_int(7);
        let data = w.into_buffer();
        // Record the offset of the second value by streaming.
        let mut probe = CompressedReadStream::new(&data);
        assert_eq!(probe.read_int(), Some(42));
        let off2 = probe.pos;
        let mut r = CompressedReadStream::new_at(&data, off2);
        assert_eq!(r.read_int(), Some(5000));
    }
}
