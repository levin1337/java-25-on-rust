/*
 * OpenJDK 25 HotSpot jfr/writers port:
 *  jfrWriterHost.hpp/.inline.hpp, jfrStorageHost.hpp/.inline.hpp,
 *  jfrMemoryWriterHost.hpp/.inline.hpp, jfrEventWriterHost.hpp/.inline.hpp,
 *  jfrStreamWriterHost.hpp/.inline.hpp, jfrStorageAdapter.hpp (MallocAdapter),
 *  jfrPosition.hpp/.inline.hpp
 *
 * Rust mapping notes:
 *  - Position<> maps to (pos, seg_start) indices into `buf`.
 *  - StorageHost/Adapter maps to the `file: Option<File>` sink; for memory
 *    writers `buf` grows on demand (equivalent of MallocAdapter::flush).
 *  - acquire/release model the MemoryWriterHost lease protocol; commit()
 *    advances the segment start like StorageHost::commit().
 */

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::jfr_encoding::*;
use crate::share::jfr::utilities::jfr_time::JfrTicks;

pub const NULL_STRING: u8 = 0;
pub const EMPTY_STRING: u8 = 1;
pub const STRING_CONSTANT: u8 = 2;
pub const UTF8: u8 = 3;
pub const UTF16: u8 = 4;
pub const LATIN1: u8 = 5;
pub const NOF_STRING_ENCODINGS: u8 = 6;

const MAX_JSHORT: usize = u16::MAX as usize;
const MAX_JINT: usize = i32::MAX as usize;
const SIZE_SAFETY_CUSHION: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JfrStringEncoding {
    NullString = 0,
    EmptyString = 1,
    StringConstant = 2,
    Utf8 = 3,
    Utf16 = 4,
    Latin1 = 5,
}

impl JfrStringEncoding {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// The chunk writer page size mirrors JfrChunkBuffer = MallocAdapter<M> (1 mb).
pub const CHUNK_BUFFER_SIZE: usize = 1024 * 1024;

pub struct JfrWriter {
    buf: Vec<u8>,
    pos: usize,
    seg_start: usize,
    base: i64,
    file: Option<File>,
    compressed_integers: bool,
    valid: bool,
    acquired: bool,
}

impl JfrWriter {
    // Memory writer over caller-provided storage (size + cushion, like
    // WriterHost(storage, size)).
    pub fn with_capacity(size: usize, compressed_integers: bool) -> JfrWriter {
        JfrWriter {
            buf: vec![0u8; size + SIZE_SAFETY_CUSHION],
            pos: 0,
            seg_start: 0,
            base: 0,
            file: None,
            compressed_integers,
            valid: true,
            acquired: false,
        }
    }

    // Streaming writer to a file (JfrChunkWriter equivalent). The file is
    // written through a 1MB page buffer; seek() flushes and repositions,
    // matching StreamWriterHost semantics.
    pub fn to_file(path: &Path, compressed_integers: bool) -> std::io::Result<JfrWriter> {
        let file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(JfrWriter {
            buf: vec![0u8; CHUNK_BUFFER_SIZE],
            pos: 0,
            seg_start: 0,
            base: 0,
            file: Some(file),
            compressed_integers,
            valid: true,
            acquired: false,
        })
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn is_acquired(&self) -> bool {
        self.acquired
    }

    pub fn has_file(&self) -> bool {
        self.file.is_some()
    }

    fn flush_page(&mut self) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.seek(SeekFrom::Start(self.base as u64));
            let _ = f.write_all(&self.buf[..self.pos]);
            let _ = f.flush();
            self.base += self.pos as i64;
            self.pos = 0;
            self.seg_start = 0;
        }
    }

    fn ensure_size(&mut self, requested: usize) -> bool {
        if !self.valid {
            return false;
        }
        if self.pos + requested > self.buf.len() {
            if self.file.is_some() {
                self.flush_page();
                if requested > self.buf.len() {
                    // single write larger than a page: write through
                    self.valid = false;
                    return false;
                }
            } else {
                let needed = self.pos + requested;
                if needed > self.buf.len() {
                    let new_len = (needed * 2).max(self.buf.len() * 2);
                    self.buf.resize(new_len, 0);
                }
            }
        }
        true
    }

    // ---- position primitives (Position<>) ----

    // Position::current_offset — for memory hosts this is the
    // segment-relative used offset; StreamWriterHost overrides it to the
    // absolute stream position (base + pos).
    pub fn current_offset(&self) -> i64 {
        if self.file.is_some() {
            self.base + self.pos as i64
        } else {
            self.used_offset()
        }
    }

    pub fn used_offset(&self) -> i64 {
        self.pos as i64 - self.seg_start as i64
    }

    fn seg_offset(&self) -> i64 {
        if self.file.is_some() {
            // seg_start is an index into the current page; its absolute
            // offset is base + seg_start only while the page is live.
            // After a page flush both are reset to 0, keeping this valid.
            self.base + self.seg_start as i64
        } else {
            self.seg_start as i64
        }
    }

    pub fn used_size(&self) -> usize {
        self.pos.saturating_sub(self.seg_start)
    }

    pub fn available_size(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn current_pos_slice(&self) -> &[u8] {
        &self.buf[..self.pos]
    }

    pub fn set_current_pos(&mut self, size: usize) {
        self.pos += size;
    }

    pub fn seek(&mut self, offset: i64) {
        if !self.is_valid() {
            return;
        }
        if offset < 0 {
            return;
        }
        if self.file.is_some() {
            // absolute stream seek (StreamWriterHost::seek)
            self.flush_page();
            if let Some(f) = self.file.as_mut() {
                let _ = f.seek(SeekFrom::Start(offset as u64));
            }
            self.base = offset;
            self.pos = 0;
            self.seg_start = 0;
        } else {
            // memory host: start_pos() + offset
            let target = self.seg_start as i64 + offset;
            if target >= 0 && (target as usize) <= self.buf.len() {
                self.pos = target as usize;
            }
        }
    }

    pub fn soft_reset(&mut self) {
        // set_start_pos(current_pos): segment start follows write cursor
        self.seg_start = self.pos;
    }

    pub fn hard_reset(&mut self) {
        self.seg_start = self.pos;
        if self.file.is_some() {
            self.base += 0; // base already tracks page start
        }
    }

    pub fn reset(&mut self) {
        self.pos = self.seg_start;
    }

    pub fn cancel(&mut self) {
        self.valid = false;
    }

    // ---- StorageHost::accommodate/commit/release ----

    pub fn accommodate(&mut self, _used: usize, _requested: usize) -> bool {
        if self.file.is_some() {
            self.flush_page();
            return self.buf.len() >= _requested;
        }
        if _requested > self.available_size() {
            let needed = self.pos + _requested;
            if needed > self.buf.len() {
                let new_len = (needed * 2).max(self.buf.len() * 2);
                self.buf.resize(new_len, 0);
            }
        }
        true
    }

    pub fn commit(&mut self) {
        if self.is_valid() {
            self.seg_start = self.pos;
            if self.file.is_some() {
                // StreamWriterHost keeps page; nothing else to do.
            }
        }
    }

    pub fn release(&mut self) {
        // lease return; for file writers flush pending page lazily on demand.
    }

    pub fn flush(&mut self) {
        self.accommodate(self.used_size(), 0);
        if self.file.is_some() {
            self.flush_page();
        }
    }

    // MemoryWriterHost::acquire
    pub fn acquire(&mut self) {
        if !self.is_valid() {
            let _ = self.accommodate(self.used_size(), 0);
        }
        self.acquired = true;
    }

    pub fn release_acquire(&mut self) {
        self.acquired = false;
        self.release();
    }

    // ---- EventWriterHost ----

    pub fn begin_write(&mut self) {
        self.acquire();
        debug_assert_eq!(self.used_offset(), 0);
    }

    pub fn end_write(&mut self) -> i64 {
        if self.is_valid() {
            self.used_offset()
        } else {
            0
        }
    }

    pub fn begin_event_write(&mut self, large: bool) {
        self.begin_write();
        if large {
            self.reserve(4);
        } else {
            self.reserve(1);
        }
    }

    // Returns bytes written for the event (0 when aborted/invalid).
    pub fn end_event_write(&mut self, large: bool) -> i64 {
        if !self.is_valid() {
            self.release_acquire();
            return 0;
        }
        let written = self.end_write() as u32;
        if large {
            if written > 4 {
                self.write_padded_at_offset_u32(written, 0);
                self.commit();
            }
        } else {
            if written > 127 {
                self.reset();
                self.release_acquire();
                return 0;
            } else if written > 1 {
                self.write_at_offset_u32(written, 0);
                self.commit();
            }
        }
        self.release_acquire();
        written as i64
    }

    // ---- WriterHost write primitives ----

    // reserve returns the offset (absolute for file writers,
    // segment-relative for memory writers — matching current_offset()).
    pub fn reserve(&mut self, size: usize) -> i64 {
        if self.ensure_size(size) {
            let off = self.current_offset();
            self.pos += size;
            off
        } else {
            self.cancel();
            0
        }
    }

    fn write_slice(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if !self.ensure_size(data.len()) {
            return;
        }
        let pos = self.pos;
        self.buf[pos..pos + data.len()].copy_from_slice(data);
        self.pos += data.len();
    }

    fn write_encoded(&mut self, f: impl FnOnce(&mut [u8]) -> usize, size_hint: usize) {
        if !self.ensure_size(size_hint.max(1)) {
            return;
        }
        let pos = self.pos;
        let n = f(&mut self.buf[pos..]);
        self.pos += n;
    }

    fn write_int_padded_or_plain(&mut self, value: u64, size: usize, padded: bool) {
        if !self.ensure_size(if padded { size } else { 10 }) {
            return;
        }
        let pos = self.pos;
        let n = if self.compressed_integers {
            if padded {
                varint128_encode_padded(value, size, &mut self.buf[pos..])
            } else {
                varint128_encode(value, &mut self.buf[pos..])
            }
        } else if padded {
            match size {
                1 => be_write_u1(value as u8, &mut self.buf[pos..]),
                2 => be_write_u2(value as u16, &mut self.buf[pos..]),
                4 => be_write_u4(value as u32, &mut self.buf[pos..]),
                8 => be_write_u8(value, &mut self.buf[pos..]),
                _ => 0,
            }
        } else {
            match size {
                1 => be_write_u1(value as u8, &mut self.buf[pos..]),
                2 => be_write_u2(value as u16, &mut self.buf[pos..]),
                4 => be_write_u4(value as u32, &mut self.buf[pos..]),
                8 => be_write_u8(value, &mut self.buf[pos..]),
                _ => 0,
            }
        };
        self.pos += n;
    }

    pub fn write_u1(&mut self, v: u8) {
        self.write_int_padded_or_plain(v as u64, 1, false);
    }
    pub fn write_u2(&mut self, v: u16) {
        self.write_int_padded_or_plain(v as u64, 2, false);
    }
    pub fn write_u4(&mut self, v: u32) {
        self.write_int_padded_or_plain(v as u64, 4, false);
    }
    pub fn write_u8(&mut self, v: u64) {
        self.write_int_padded_or_plain(v, 8, false);
    }
    pub fn write_i1(&mut self, v: i8) {
        self.write_int_padded_or_plain(v as u8 as u64, 1, false);
    }
    pub fn write_i2(&mut self, v: i16) {
        self.write_int_padded_or_plain(v as u16 as u64, 2, false);
    }
    pub fn write_i4(&mut self, v: i32) {
        self.write_int_padded_or_plain(v as u32 as u64, 4, false);
    }
    pub fn write_i8(&mut self, v: i64) {
        self.write_int_padded_or_plain(v as u64, 8, false);
    }

    pub fn write_bool(&mut self, v: bool) {
        // be_write((u1)value)
        if !self.ensure_size(1) {
            return;
        }
        self.buf[self.pos] = v as u8;
        self.pos += 1;
    }

    pub fn write_f32(&mut self, v: f32) {
        self.write_be_bits(v.to_bits() as u64, 4);
    }

    pub fn write_f64(&mut self, v: f64) {
        self.write_be_bits(v.to_bits(), 8);
    }

    fn write_be_bits(&mut self, v: u64, size: usize) {
        if !self.ensure_size(size) {
            return;
        }
        let pos = self.pos;
        match size {
            4 => {
                self.buf[pos..pos + 4].copy_from_slice(&(v as u32).to_be_bytes());
            }
            8 => {
                self.buf[pos..pos + 8].copy_from_slice(&v.to_be_bytes());
            }
            _ => {}
        }
        self.pos += size;
    }

    // Padded writes (compressed: varint128 padded; else big-endian)
    pub fn write_padded_u1(&mut self, v: u8) {
        self.write_int_padded_or_plain(v as u64, 1, true);
    }
    pub fn write_padded_u2(&mut self, v: u16) {
        self.write_int_padded_or_plain(v as u64, 2, true);
    }
    pub fn write_padded_u4(&mut self, v: u32) {
        self.write_int_padded_or_plain(v as u64, 4, true);
    }
    pub fn write_padded_u8(&mut self, v: u64) {
        self.write_int_padded_or_plain(v, 8, true);
    }
    pub fn write_padded_i4(&mut self, v: i32) {
        self.write_int_padded_or_plain(v as u32 as u64, 4, true);
    }
    pub fn write_padded_i8(&mut self, v: i64) {
        self.write_int_padded_or_plain(v as u64, 8, true);
    }

    // Big-endian (uncompressed) writes — be_write
    pub fn be_write_u1(&mut self, v: u8) {
        self.write_be_bits(v as u64, 1);
    }
    pub fn be_write_u2(&mut self, v: u16) {
        self.write_be_bits(v as u64, 2);
    }
    pub fn be_write_u4(&mut self, v: u32) {
        self.write_be_bits(v as u64, 4);
    }
    pub fn be_write_u8(&mut self, v: u64) {
        self.write_be_bits(v, 8);
    }
    pub fn be_write_i8(&mut self, v: i64) {
        self.write_be_bits(v as u64, 8);
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        self.write_slice(data);
    }

    pub fn write_str(&mut self, value: Option<&str>) {
        // write(const char*) — UTF-8 with u2 length prefix
        match value {
            None => self.write_u2(0),
            Some(s) => {
                let len = s.len().min(MAX_JSHORT);
                self.write_u2(len as u16);
                self.write_slice(&s.as_bytes()[..len]);
            }
        }
    }

    // WriterHost::write_utf8 — encoding byte + compressed len + bytes
    pub fn write_utf8(&mut self, value: Option<&str>) {
        match value {
            None => self.write_u1(NULL_STRING),
            Some(s) => {
                self.write_u1(UTF8);
                let len = s.len().min(MAX_JINT);
                self.write_i4(len as i32);
                self.write_slice(&s.as_bytes()[..len]);
            }
        }
    }

    pub fn write_utf8_u2_len(&mut self, value: Option<&str>) {
        match value {
            None => self.write_u2(0),
            Some(s) => {
                let len = s.len().min(MAX_JSHORT);
                self.write_u2(len as u16);
                self.write_slice(&s.as_bytes()[..len]);
            }
        }
    }

    pub fn write_empty_string(&mut self) {
        self.write_u1(EMPTY_STRING);
    }

    // jstring equivalent: latin1 or utf16 java string
    pub fn write_java_string(&mut self, value: Option<&str>) {
        match value {
            None => self.write_u1(NULL_STRING),
            Some(s) => {
                if s.is_empty() {
                    self.write_empty_string();
                    return;
                }
                if s.is_ascii() {
                    self.write_u1(LATIN1);
                    self.write_u4(s.len() as u32);
                    self.write_slice(s.as_bytes());
                } else {
                    self.write_u1(UTF16);
                    self.write_u4(s.encode_utf16().count() as u32);
                    let mut tmp = Vec::with_capacity(s.len() * 2);
                    for c in s.encode_utf16() {
                        tmp.extend_from_slice(&c.to_be_bytes());
                    }
                    self.write_slice(&tmp);
                }
            }
        }
    }

    pub fn write_ticks(&mut self, ticks: JfrTicks) {
        self.write_i8(ticks.value());
    }

    pub fn size_in_bytes_u64(&self, value: u64) -> usize {
        if self.compressed_integers {
            varint128_size_in_bytes(value)
        } else {
            8
        }
    }

    // ---- at-offset patching ----

    fn patch_at(&mut self, offset: i64, f: impl FnOnce(&mut JfrWriter)) {
        if !self.is_valid() {
            return;
        }
        let current = self.current_offset();
        self.seek(offset);
        f(self);
        self.seek(current);
    }

    pub fn write_at_offset_u32(&mut self, value: u32, offset: i64) {
        let compressed = self.compressed_integers;
        self.patch_at(offset, move |w| {
            if compressed {
                // compressed write at offset
                let pos = w.pos;
                let n = varint128_encode(value as u64, &mut w.buf[pos..]);
                w.pos += n;
            } else {
                w.write_u4(value);
            }
        });
    }

    pub fn write_padded_at_offset_u32(&mut self, value: u32, offset: i64) {
        let compressed = self.compressed_integers;
        self.patch_at(offset, move |w| {
            if compressed {
                let pos = w.pos;
                let n = varint128_encode_padded(value as u64, 4, &mut w.buf[pos..]);
                w.pos += n;
            } else {
                w.write_padded_u4(value);
            }
        });
    }

    pub fn write_padded_at_offset_i64(&mut self, value: i64, offset: i64) {
        let compressed = self.compressed_integers;
        self.patch_at(offset, move |w| {
            if compressed {
                let pos = w.pos;
                let n = varint128_encode_padded(value as u64, 8, &mut w.buf[pos..]);
                w.pos += n;
            } else {
                w.write_padded_i8(value);
            }
        });
    }

    pub fn write_be_at_offset_u64(&mut self, value: u64, offset: i64) {
        self.patch_at(offset, move |w| {
            w.be_write_u8(value);
        });
    }

    pub fn write_be_at_offset_u32(&mut self, value: u32, offset: i64) {
        self.patch_at(offset, move |w| {
            w.be_write_u4(value);
        });
    }

    // Patch arbitrary bytes at an absolute (file) or segment-relative
    // (memory) offset, restoring the position afterwards.
    pub fn write_bytes_at_offset(&mut self, data: &[u8], offset: i64) {
        if !self.is_valid() {
            return;
        }
        let current = self.current_offset();
        self.seek(offset);
        self.write_bytes(data);
        self.seek(current);
    }

    // Direct big-endian poke used by JfrCheckpointWriter header.
    pub fn poke_be_u64(&mut self, offset: usize, value: u64) {
        if offset + 8 <= self.buf.len() {
            self.buf[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
        }
    }

    pub fn poke_be_u32(&mut self, offset: usize, value: u32) {
        if offset + 4 <= self.buf.len() {
            self.buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.buf[..self.pos]
    }

    // Write pending file page and return full contents (file mode).
    pub fn into_bytes(mut self) -> Vec<u8> {
        if self.file.is_some() {
            self.flush_page();
            // read back? For chunk writer we keep data in memory instead.
        }
        self.buf.truncate(self.pos);
        self.buf.clone()
    }

    pub fn file_mut(&mut self) -> Option<&mut File> {
        self.file.as_mut()
    }

    pub fn finish_file(mut self) -> std::io::Result<()> {
        self.flush_page();
        if let Some(f) = self.file.as_mut() {
            f.flush()?;
        }
        Ok(())
    }
}

impl Read for JfrWriter {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let avail = self.pos;
        let n = out.len().min(avail);
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf.copy_within(n..avail, 0);
        self.pos = avail - n;
        self.seg_start = self.seg_start.saturating_sub(n);
        Ok(n)
    }
}

/// Minimal writer abstraction shared with stack-trace serialization.
pub trait JfrWriterLike {
    fn write_u4(&mut self, v: u32);
    fn write_u8(&mut self, v: u64);
}

impl JfrWriterLike for JfrWriter {
    fn write_u4(&mut self, v: u32) {
        JfrWriter::write_u4(self, v);
    }
    fn write_u8(&mut self, v: u64) {
        JfrWriter::write_u8(self, v);
    }
}
