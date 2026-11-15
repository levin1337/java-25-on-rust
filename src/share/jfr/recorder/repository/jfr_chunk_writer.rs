/*
 * OpenJDK 25 HotSpot jfr/recorder/repository/jfrChunkWriter.hpp + .cpp port
 *
 * JfrChunkWriter — the chunk file writer. Writes the FLR header, events,
 * checkpoint events, constant pools and metadata into a chunk.
 *
 * Rust mapping: the C++ StreamWriterHost+MallocAdapter(1MB) pipeline is
 * represented by an in-memory JfrWriter; the finished buffer is written
 * to the chunk file on close/flush. seek()/patch offsets behave
 * identically (absolute stream offsets; the chunk writer never commits
 * mid-stream so segment start stays 0).
 */

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::share::jfr::recorder::repository::jfr_chunk::{JfrChunk, COMPLETE, GUARD, PAD};
use crate::share::jfr::utilities::jfr_time::JfrTicks;
use crate::share::jfr::utilities::jfr_types::{JfrCheckpointType, JfrTypeId, EVENT_CHECKPOINT};
use crate::share::jfr::writers::jfr_writer::JfrWriter;

// chunk header layout offsets
const MAGIC_LEN: i64 = 4;
const SIZE_OFFSET: i64 = 8;
const SLOT_SIZE: i64 = 8;
const CHECKPOINT_OFFSET: i64 = SIZE_OFFSET + SLOT_SIZE;
const METADATA_OFFSET: i64 = CHECKPOINT_OFFSET + SLOT_SIZE;
const START_NANOS_OFFSET: i64 = METADATA_OFFSET + SLOT_SIZE;
const DURATION_NANOS_OFFSET: i64 = START_NANOS_OFFSET + SLOT_SIZE;
const START_TICKS_OFFSET: i64 = DURATION_NANOS_OFFSET + SLOT_SIZE;
const CPU_FREQUENCY_OFFSET: i64 = START_TICKS_OFFSET + SLOT_SIZE;
const GENERATION_OFFSET: i64 = CPU_FREQUENCY_OFFSET + SLOT_SIZE;
const FLAG_OFFSET: i64 = GENERATION_OFFSET + 2;
const HEADER_SIZE: i64 = FLAG_OFFSET + 2; // 68

const CHUNK_INITIAL_CAPACITY: usize = 8 * 1024 * 1024;

pub struct JfrChunkWriter {
    writer: JfrWriter,
    chunk: JfrChunk,
    path: Option<PathBuf>,
    open: bool,
}

impl JfrChunkWriter {
    pub fn new() -> JfrChunkWriter {
        JfrChunkWriter {
            writer: JfrWriter::with_capacity(CHUNK_INITIAL_CAPACITY, true),
            chunk: JfrChunk::new(),
            path: None,
            open: false,
        }
    }

    // ---- accessors ----

    pub fn writer_mut(&mut self) -> &mut JfrWriter {
        &mut self.writer
    }

    pub fn writer(&self) -> &JfrWriter {
        &self.writer
    }

    pub fn is_valid(&self) -> bool {
        self.writer.is_valid() && self.open
    }

    pub fn current_offset(&self) -> i64 {
        self.writer.current_offset()
    }

    pub fn size_written(&self) -> i64 {
        if self.is_valid() {
            self.current_offset()
        } else {
            0
        }
    }

    pub fn last_checkpoint_offset(&self) -> i64 {
        self.chunk.last_checkpoint_offset()
    }

    pub fn current_chunk_start_nanos(&self) -> i64 {
        self.chunk.start_nanos()
    }

    pub fn set_last_checkpoint_offset(&mut self, offset: i64) {
        self.chunk.set_last_checkpoint_offset(offset);
    }

    pub fn set_last_metadata_offset(&mut self, offset: i64) {
        self.chunk.set_last_metadata_offset(offset);
    }

    pub fn has_metadata(&self) -> bool {
        self.chunk.has_metadata()
    }

    pub fn set_time_stamp(&mut self) {
        self.chunk.set_time_stamp();
    }

    pub fn mark_chunk_final(&mut self) {
        self.chunk.mark_final();
    }

    pub fn set_path(&mut self, path: Option<String>) {
        self.chunk.set_path(path.clone());
        self.path = path.map(PathBuf::from);
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    // ---- forwarded write primitives (compressed ints, BE for header) ----

    pub fn write_u64(&mut self, v: u64) {
        self.writer.write_u8(v);
    }
    pub fn write_u32(&mut self, v: u32) {
        self.writer.write_u4(v);
    }
    pub fn write_u8_id(&mut self, v: u64) {
        self.writer.write_u8(v);
    }
    pub fn write_i8(&mut self, v: i64) {
        self.writer.write_i8(v);
    }
    pub fn write_u4(&mut self, v: u32) {
        self.writer.write_u4(v);
    }
    pub fn write_bytes(&mut self, data: &[u8]) {
        self.writer.write_bytes(data);
    }
    pub fn write_buffered(&mut self, data: &[u8]) {
        self.writer.write_bytes(data);
    }
    pub fn write_unbuffered(&mut self, data: &[u8]) {
        self.writer.write_bytes(data);
    }
    pub fn reserve(&mut self, size: usize) -> i64 {
        self.writer.reserve(size)
    }
    pub fn seek(&mut self, offset: i64) {
        self.writer.seek(offset);
    }
    pub fn write_padded_at_offset_u32(&mut self, v: u32, off: i64) {
        self.writer.write_padded_at_offset_u32(v, off);
    }
    pub fn write_padded_at_offset_u64(&mut self, v: u64, off: i64) {
        let cur = self.writer.current_offset();
        self.writer.seek(off);
        self.writer.write_padded_u8(v);
        self.writer.seek(cur);
    }
    pub fn write_be_at_offset_u64(&mut self, v: u64, off: i64) {
        self.writer.write_be_at_offset_u64(v, off);
    }
    pub fn write_be_at_offset_i64(&mut self, v: i64, off: i64) {
        self.writer.write_be_at_offset_u64(v as u64, off);
    }
    pub fn write_at_offset_u32(&mut self, v: u32, off: i64) {
        self.writer.write_at_offset_u32(v, off);
    }
    pub fn write_str(&mut self, v: Option<&str>) {
        self.writer.write_str(v);
    }
    pub fn write_utf8(&mut self, v: Option<&str>) {
        self.writer.write_utf8(v);
    }
    pub fn write_empty_string(&mut self) {
        self.writer.write_empty_string();
    }
    pub fn write_ticks(&mut self, t: JfrTicks) {
        self.writer.write_ticks(t);
    }
    pub fn write_bool(&mut self, v: bool) {
        self.writer.write_bool(v);
    }
    pub fn write_f64(&mut self, v: f64) {
        self.writer.write_f64(v);
    }
    pub fn write_be_u8(&mut self, v: u64) {
        self.writer.be_write_u8(v);
    }
    pub fn write_be_u4(&mut self, v: u32) {
        self.writer.be_write_u4(v);
    }
    pub fn write_be_u2(&mut self, v: u16) {
        self.writer.be_write_u2(v);
    }
    pub fn write_be_u1(&mut self, v: u8) {
        self.writer.be_write_u1(v);
    }

    pub fn size_in_bytes_u64(&self, v: u64) -> usize {
        self.writer.size_in_bytes_u64(v)
    }
    pub fn size_in_bytes_i64(&self, v: i64) -> usize {
        self.writer.size_in_bytes_u64(v as u64)
    }
    pub fn size_in_bytes_u32(&self, v: u32) -> usize {
        self.writer.size_in_bytes_u64(v as u64)
    }

    // ---- lifecycle ----

    pub fn open(&mut self) -> bool {
        if let Some(p) = self.path.clone() {
            match OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&p)
            {
                Ok(mut f) => {
                    let _ = f.write_all(&[0u8; HEADER_SIZE as usize]);
                    let _ = f.flush();
                }
                Err(_) => return false,
            }
        }
        self.chunk.update_start_nanos_pub();
        self.chunk.update_start_ticks_pub();
        self.open = true;
        self.write_header_initial();
        true
    }

    fn write_header_initial(&mut self) {
        // JfrChunkHeadWriter::initialize — magic, version, size slots,
        // checkpoint/metadata offsets, times, cpu freq, generation, flags.
        let w = &mut self.writer;
        w.write_bytes(self.chunk.magic());
        w.be_write_u2(self.chunk.major_version());
        w.be_write_u2(self.chunk.minor_version());
        w.be_write_u8(HEADER_SIZE as u64); // size (patched on flush/close)
        w.be_write_u8(0); // last checkpoint offset
        w.be_write_u8(0); // last metadata offset
        w.be_write_u8(self.chunk.start_nanos() as u64);
        w.be_write_u8(self.chunk.duration() as u64);
        w.be_write_u8(self.chunk.start_ticks() as u64);
        w.be_write_u8(self.chunk.cpu_frequency() as u64);
        w.be_write_u1(self.chunk.generation());
        w.be_write_u1(PAD);
        w.be_write_u2(self.chunk.flags());
        debug_assert_eq!(w.current_offset(), HEADER_SIZE);
    }

    // write_chunk_header_checkpoint — appends the header constant-pool
    // checkpoint event and embeds the current header image inside it.
    pub fn write_chunk_header_checkpoint(&mut self, flushpoint: bool) -> i64 {
        let event_size_offset = self.current_offset();
        let header_content_pos = self.prepare_chunk_header_constant_pool(event_size_offset, flushpoint);
        // embedded header image (68 bytes) at header_content_pos
        let sz_estimate = self.size_written();
        let img = self.header_image(sz_estimate, event_size_offset, !flushpoint, false);
        self.writer
            .write_bytes_at_offset(&img, header_content_pos);
        let checkpoint_size = (self.current_offset() - event_size_offset) as u32;
        self.write_padded_at_offset_u32(checkpoint_size, event_size_offset);
        self.set_last_checkpoint_offset(event_size_offset);
        let sz_written = self.size_written();
        // patch the size field inside the embedded image
        self.write_be_at_offset_i64(sz_written, header_content_pos + 8);
        sz_written
    }

    // Builds the 68-byte header image:
    // magic(4) major(2) minor(2) size(8) checkpoint(8) metadata(8)
    // start_nanos(8) duration_nanos(8) start_ticks(8) cpu(8)
    // generation(1) pad(1) flags(2)
    fn header_image(
        &mut self,
        size: i64,
        checkpoint_offset: i64,
        finalize: bool,
        next_gen: bool,
    ) -> Vec<u8> {
        let mut img = Vec::with_capacity(HEADER_SIZE as usize);
        img.extend_from_slice(self.chunk.magic());
        img.extend_from_slice(&self.chunk.major_version().to_be_bytes());
        img.extend_from_slice(&self.chunk.minor_version().to_be_bytes());
        img.extend_from_slice(&(size as u64).to_be_bytes());
        img.extend_from_slice(&(checkpoint_offset as u64).to_be_bytes());
        img.extend_from_slice(&(self.chunk.last_metadata_offset() as u64).to_be_bytes());
        if finalize {
            img.extend_from_slice(&self.chunk.previous_start_nanos().to_be_bytes());
            img.extend_from_slice(&self.chunk.last_chunk_duration().to_be_bytes());
            img.extend_from_slice(&self.chunk.previous_start_ticks().to_be_bytes());
        } else {
            img.extend_from_slice(&self.chunk.start_nanos().to_be_bytes());
            img.extend_from_slice(&self.chunk.duration().to_be_bytes());
            img.extend_from_slice(&self.chunk.start_ticks().to_be_bytes());
        }
        img.extend_from_slice(&self.chunk.cpu_frequency().to_be_bytes());
        if finalize {
            img.push(COMPLETE);
        } else if next_gen {
            img.push(self.chunk.next_generation());
        } else {
            img.push(self.chunk.generation_pub());
        }
        img.push(PAD);
        img.extend_from_slice(&self.chunk.flags().to_be_bytes());
        debug_assert_eq!(img.len(), HEADER_SIZE as usize);
        img
    }

    fn prepare_chunk_header_constant_pool(&mut self, event_offset: i64, flushpoint: bool) -> i64 {
        let delta = if self.last_checkpoint_offset() == 0 {
            0
        } else {
            self.last_checkpoint_offset() - event_offset
        };
        let checkpoint_type = if flushpoint {
            JfrCheckpointType::Flush.bits() | JfrCheckpointType::Header.bits()
        } else {
            JfrCheckpointType::Header.bits()
        };
        self.writer.reserve(4);
        self.writer.write_u8(EVENT_CHECKPOINT);
        self.writer.write_i8(JfrTicks::now().value());
        self.writer.write_i8(0); // duration
        self.writer.write_i8(delta);
        self.writer.write_u4(checkpoint_type);
        self.writer.write_u4(1); // pool count
        self.writer.write_u8(JfrTypeId::ChunkHeader.as_u64());
        self.writer.write_u4(1); // count
        self.writer.write_u8(1); // key
        self.writer.write_u4(HEADER_SIZE as u32); // byte array length
        self.writer.current_offset()
    }

    pub fn flush_chunk(&mut self, flushpoint: bool) -> i64 {
        let sz_written = self.write_chunk_header_checkpoint(flushpoint);
        // JfrChunkHeadWriter head(this, SIZE_OFFSET): guard generation
        // field, then patch size..flags at SIZE_OFFSET.
        self.seek(GENERATION_OFFSET);
        self.write_be_u1(GUARD);
        self.write_be_u1(PAD);
        self.seek(SIZE_OFFSET);
        self.write_size_to_generation(sz_written, !flushpoint);
        self.persist();
        sz_written
    }

    fn write_size_to_generation(&mut self, size: i64, finalize: bool) {
        self.write_be_i64_pub(size);
        self.write_be_i64_pub(self.chunk.last_checkpoint_offset());
        self.write_be_i64_pub(self.chunk.last_metadata_offset());
        if finalize {
            self.write_be_i64_pub(self.chunk.previous_start_nanos());
            self.write_be_i64_pub(self.chunk.last_chunk_duration());
            self.write_be_i64_pub(self.chunk.previous_start_ticks());
        } else {
            self.chunk.update_current_nanos_pub();
            self.write_be_i64_pub(self.chunk.start_nanos());
            self.write_be_i64_pub(self.chunk.duration());
            self.write_be_i64_pub(self.chunk.start_ticks());
        }
        self.write_be_i64_pub(self.chunk.cpu_frequency());
        let r#gen = if finalize {
            COMPLETE
        } else {
            self.chunk.generation_pub()
        };
        let flags = self.chunk.flags();
        self.write_be_u1(r#gen);
        self.write_be_u1(PAD);
        self.write_be_u2(flags);
    }

    fn write_be_i64_pub(&mut self, v: i64) {
        self.writer.be_write_i8(v);
    }

    fn persist(&mut self) {
        if let Some(p) = self.path.clone() {
            if let Ok(mut f) = OpenOptions::new().create(true).write(true).open(&p) {
                let _ = f.write_all(self.writer.data());
                let _ = f.flush();
            }
        }
    }

    pub fn close(&mut self) -> i64 {
        let size = self.flush_chunk(false);
        self.persist();
        self.open = false;
        size
    }
}

impl Default for JfrChunkWriter {
    fn default() -> Self {
        Self::new()
    }
}
