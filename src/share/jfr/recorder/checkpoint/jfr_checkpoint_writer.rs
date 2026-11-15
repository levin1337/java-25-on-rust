/*
 * OpenJDK 25 HotSpot jfr/recorder/checkpoint/jfrCheckpointWriter.hpp + .cpp port
 *
 * JfrCheckpointWriter — EventWriterHost over a checkpoint JfrBuffer.
 * Writes one or more checkpoint entries (JfrCheckpointEntry header +
 * constant-pool payload) into the acquired buffer.
 */

use crate::share::jfr::recorder::checkpoint::jfr_checkpoint_manager::JfrCheckpointManager;
use crate::share::jfr::recorder::storage::jfr_buffer::JfrBuffer;
use crate::share::jfr::utilities::jfr_time::JfrTicks;
use crate::share::jfr::utilities::jfr_types::{
    JfrCheckpointBufferKind, JfrCheckpointContext, JfrCheckpointType, JfrTypeId, TraceId,
};
use crate::share::jfr::utilities::jfr_blob::JfrBlobHandle;
use crate::share::jfr::writers::jfr_encoding::*;

// JfrCheckpointEntry layout (big-endian):
//   i64 size, i64 starttime, i64 duration, u32 checkpoint_type, u32 type_count
pub const CHECKPOINT_ENTRY_HEADER_SIZE: usize = 8 + 8 + 8 + 4 + 4;

pub struct JfrCheckpointWriter {
    buffer: Option<JfrBuffer>,
    pos: usize,
    seg_start: usize,
    time: JfrTicks,
    offset: i64,
    count: u32,
    cpt_type: JfrCheckpointType,
    header: bool,
    compressed: bool,
    valid: bool,
    kind: JfrCheckpointBufferKind,
    thread: TraceId,
    previous_epoch: bool,
    released: bool,
}

impl JfrCheckpointWriter {
    fn new(
        buffer: JfrBuffer,
        seg_start: usize,
        header: bool,
        cpt_type: JfrCheckpointType,
        kind: JfrCheckpointBufferKind,
        thread: TraceId,
        previous_epoch: bool,
    ) -> JfrCheckpointWriter {
        let mut w = JfrCheckpointWriter {
            pos: buffer.pos(),
            seg_start,
            buffer: Some(buffer),
            time: JfrTicks::now(),
            offset: 0,
            count: 0,
            cpt_type,
            header,
            compressed: true,
            valid: true,
            kind,
            thread,
            previous_epoch,
            released: false,
        };
        if header {
            w.reserve(CHECKPOINT_ENTRY_HEADER_SIZE);
        }
        w
    }

    pub fn global(
        header: bool,
        cpt_type: JfrCheckpointType,
        kind: JfrCheckpointBufferKind,
    ) -> JfrCheckpointWriter {
        let (buf, seg) = JfrCheckpointManager::acquire(kind, 0, false);
        JfrCheckpointWriter::new(buf, seg, header, cpt_type, kind, 0, false)
    }

    pub fn for_thread(
        thread: TraceId,
        header: bool,
        cpt_type: JfrCheckpointType,
        kind: JfrCheckpointBufferKind,
    ) -> JfrCheckpointWriter {
        let (buf, seg) = JfrCheckpointManager::acquire(kind, thread, false);
        JfrCheckpointWriter::new(buf, seg, header, cpt_type, kind, thread, false)
    }

    pub fn previous_epoch_global(
        previous_epoch: bool,
        thread: TraceId,
        header: bool,
        cpt_type: JfrCheckpointType,
    ) -> JfrCheckpointWriter {
        let (buf, seg) = JfrCheckpointManager::lease_global(previous_epoch, 0);
        JfrCheckpointWriter::new(
            buf,
            seg,
            header,
            cpt_type,
            JfrCheckpointBufferKind::Global,
            thread,
            previous_epoch,
        )
    }

    // ---- state ----

    pub fn is_valid(&self) -> bool {
        self.valid && self.buffer.is_some()
    }

    pub fn has_data(&self) -> bool {
        self.used_size()
            > if self.header {
                CHECKPOINT_ENTRY_HEADER_SIZE
            } else {
                0
            }
    }

    pub fn used_size(&self) -> usize {
        self.pos.saturating_sub(self.seg_start)
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn set_count(&mut self, count: u32) {
        self.count = count;
    }

    pub fn context(&self) -> JfrCheckpointContext {
        JfrCheckpointContext {
            offset: self.used_offset(),
            count: self.count,
        }
    }

    pub fn set_context(&mut self, ctx: JfrCheckpointContext) {
        self.seek(ctx.offset);
        self.count = ctx.count;
    }

    fn used_offset(&self) -> i64 {
        (self.pos - self.seg_start) as i64
    }

    // current_offset — segment-relative, matching Position::current_offset
    // for memory hosts.
    fn current_offset(&self) -> i64 {
        self.used_offset()
    }

    pub fn cancel(&mut self) {
        self.valid = false;
    }

    // ---- buffer access ----

    fn buf(&self) -> &JfrBuffer {
        self.buffer.as_ref().unwrap()
    }

    fn buf_mut(&mut self) -> &mut JfrBuffer {
        self.buffer.as_mut().unwrap()
    }

    fn ensure(&mut self, n: usize) -> bool {
        if !self.is_valid() {
            return false;
        }
        if self.pos + n <= self.buf().size() {
            return true;
        }
        let used = self.used_size();
        // outstanding (uncommitted) bytes of this writer: [seg_start, pos)
        let outstanding: Vec<u8> = self.buf().data[self.seg_start..self.pos].to_vec();
        let old = self.buffer.take().unwrap();
        match JfrCheckpointManager::flush(old, used, n, self.kind, self.thread, self.previous_epoch)
        {
            Some((mut nb, _seg)) => {
                nb.data[..outstanding.len()].copy_from_slice(&outstanding);
                nb.set_pos(outstanding.len());
                self.buffer = Some(nb);
                self.pos = outstanding.len();
                self.seg_start = 0;
                true
            }
            None => {
                self.valid = false;
                false
            }
        }
    }

    pub fn reserve(&mut self, size: usize) -> i64 {
        if self.ensure(size) {
            let off = self.current_offset();
            self.pos += size;
            off
        } else {
            self.valid = false;
            0
        }
    }

    pub fn seek(&mut self, offset: i64) {
        if !self.is_valid() || offset < 0 {
            return;
        }
        let target = self.seg_start as i64 + offset;
        if target >= 0 && (target as usize) <= self.buf().size() {
            self.pos = target as usize;
        }
    }

    // ---- scalar writes ----

    fn write_int(&mut self, value: u64, size: usize, padded: bool) {
        if !self.ensure(if padded { size } else { 10 }) {
            return;
        }
        let pos = self.pos;
        let n = if self.compressed {
            if padded {
                varint128_encode_padded(value, size, &mut self.buf_mut().data[pos..])
            } else {
                varint128_encode(value, &mut self.buf_mut().data[pos..])
            }
        } else {
            match size {
                1 => be_write_u1(value as u8, &mut self.buf_mut().data[pos..]),
                2 => be_write_u2(value as u16, &mut self.buf_mut().data[pos..]),
                4 => be_write_u4(value as u32, &mut self.buf_mut().data[pos..]),
                8 => be_write_u8(value, &mut self.buf_mut().data[pos..]),
                _ => 0,
            }
        };
        self.pos += n;
    }

    fn be_write(&mut self, value: u64, size: usize) {
        if !self.ensure(size) {
            return;
        }
        let pos = self.pos;
        let n = match size {
            1 => be_write_u1(value as u8, &mut self.buf_mut().data[pos..]),
            2 => be_write_u2(value as u16, &mut self.buf_mut().data[pos..]),
            4 => be_write_u4(value as u32, &mut self.buf_mut().data[pos..]),
            8 => be_write_u8(value, &mut self.buf_mut().data[pos..]),
            _ => 0,
        };
        self.pos += n;
    }

    pub fn write_u1(&mut self, v: u8) {
        self.write_int(v as u64, 1, false);
    }
    pub fn write_u2(&mut self, v: u16) {
        self.write_int(v as u64, 2, false);
    }
    pub fn write_u4(&mut self, v: u32) {
        self.write_int(v as u64, 4, false);
    }
    pub fn write_u8(&mut self, v: u64) {
        self.write_int(v, 8, false);
    }
    pub fn write_u64(&mut self, v: u64) {
        self.write_u8(v);
    }
    pub fn write_i4(&mut self, v: i32) {
        self.write_int(v as u32 as u64, 4, false);
    }
    pub fn write_i8(&mut self, v: i64) {
        self.write_int(v as u64, 8, false);
    }
    pub fn write_bool(&mut self, v: bool) {
        // be_write((u1)value)
        if !self.ensure(1) {
            return;
        }
        { let pos = self.pos; self.buf_mut().data[pos] = v as u8; }
        self.pos += 1;
    }
    pub fn write_f64(&mut self, v: f64) {
        self.be_write(v.to_bits(), 8);
    }
    pub fn write_f32(&mut self, v: f32) {
        self.be_write(v.to_bits() as u64, 4);
    }

    pub fn write_padded_u4(&mut self, v: u32) {
        self.write_int(v as u64, 4, true);
    }
    pub fn write_padded_u8(&mut self, v: u64) {
        self.write_int(v, 8, true);
    }
    pub fn write_padded_i8(&mut self, v: i64) {
        self.write_int(v as u64, 8, true);
    }

    pub fn be_write_u8(&mut self, v: u64) {
        self.be_write(v, 8);
    }
    pub fn be_write_u4(&mut self, v: u32) {
        self.be_write(v as u64, 4);
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if !self.ensure(data.len()) {
            return;
        }
        let pos = self.pos;
        self.buf_mut().data[pos..pos + data.len()].copy_from_slice(data);
        self.pos += data.len();
    }

    pub fn write_str(&mut self, value: Option<&str>) {
        match value {
            None => self.write_u2(0),
            Some(s) => {
                let len = s.len().min(u16::MAX as usize);
                self.write_u2(len as u16);
                self.write_bytes(&s.as_bytes()[..len]);
            }
        }
    }

    pub fn write_utf8(&mut self, value: Option<&str>) {
        match value {
            None => self.write_u1(0),
            Some(s) => {
                self.write_u1(3); // UTF8
                let len = s.len().min(i32::MAX as usize);
                self.write_i4(len as i32);
                self.write_bytes(&s.as_bytes()[..len]);
            }
        }
    }

    pub fn write_empty_string(&mut self) {
        self.write_u1(1); // EMPTY_STRING
    }

    pub fn write_ticks(&mut self, ticks: JfrTicks) {
        self.write_i8(ticks.value());
    }

    // ---- type set protocol ----

    pub fn write_type_id(&mut self, type_id: JfrTypeId) {
        self.write_u8(type_id.as_u64());
        self.count += 1;
    }

    pub fn write_key(&mut self, key: u64) {
        self.write_u8(key);
    }

    pub fn write_count(&mut self, nof_entries: u32) {
        self.write_padded_u4(nof_entries);
    }

    pub fn write_count_at(&mut self, nof_entries: u32, offset: i64) {
        self.write_padded_at_offset(nof_entries as u64, 4, offset);
    }

    fn patch_at(&mut self, offset: i64, f: impl FnOnce(&mut Self)) {
        if !self.is_valid() {
            return;
        }
        let current = self.current_offset();
        self.seek(offset);
        f(self);
        self.seek(current);
    }

    fn write_padded_at_offset(&mut self, value: u64, size: usize, offset: i64) {
        let compressed = self.compressed;
        self.patch_at(offset, move |w| {
            if compressed {
                if !w.ensure(size) {
                    return;
                }
                let pos = w.pos;
                let n = varint128_encode_padded(value, size, &mut w.buf_mut().data[pos..]);
                w.pos += n;
            } else {
                w.write_int(value, size, true);
            }
        });
    }

    pub fn write_padded_at_offset_i64(&mut self, value: i64, offset: i64) {
        let compressed = self.compressed;
        self.patch_at(offset, move |w| {
            if compressed {
                if !w.ensure(8) {
                    return;
                }
                let pos = w.pos;
                let n = varint128_encode_padded(value as u64, 8, &mut w.buf_mut().data[pos..]);
                w.pos += n;
            } else {
                w.write_padded_i8(value);
            }
        });
    }

    // ---- lifecycle ----

    fn write_checkpoint_header(&mut self) {
        if !self.header || !self.is_valid() {
            return;
        }
        let size = self.pos - self.seg_start;
        let start = self.seg_start;
        let time_v = self.time.0;
        let dur_v = (JfrTicks::now().0 - time_v) as u64;
        let cpt_v = self.cpt_type.bits() as u64;
        let count_v = self.count as u64;
        let buf = self.buf_mut();
        let mut be = |off: usize, v: u64, n: usize| match n {
            8 => buf.data[start + off..start + off + 8].copy_from_slice(&v.to_be_bytes()),
            4 => buf.data[start + off..start + off + 4]
                .copy_from_slice(&(v as u32).to_be_bytes()),
            _ => {}
        };
        be(0, size as u64, 8); // total size
        be(8, time_v as u64, 8); // starttime (ticks)
        be(16, dur_v, 8); // duration
        be(24, cpt_v, 4);
        be(28, count_v, 4);
    }

    // session_data — returns the checkpoint entry bytes [seg_start, pos)
    // and rewinds pos to seg_start when move_ == true.
    pub fn session_data(&mut self, move_: bool) -> Option<Vec<u8>> {
        if !self.is_valid() {
            return None;
        }
        if self.header {
            self.write_checkpoint_header();
            self.header = false;
        }
        let data = self.buf().data[self.seg_start..self.pos].to_vec();
        if move_ {
            self.seek(self.offset);
        }
        Some(data)
    }

    pub fn copy(&mut self) -> Option<JfrBlobHandle> {
        self.session_data(false).map(|d| JfrBlobHandle::make(&d))
    }

    pub fn move_blob(&mut self) -> Option<JfrBlobHandle> {
        self.session_data(true).map(|d| JfrBlobHandle::make(&d))
    }

    pub fn release(&mut self) {
        if !self.is_valid() || self.used_size() == 0 {
            return;
        }
        // write through: commit buffer pos
        if let Some(buf) = self.buffer.take() {
            buf.set_pos(self.pos);
            buf.set_top(self.pos);
            JfrCheckpointManager::release_buffer(buf, self.kind, self.thread, self.previous_epoch);
        }
        self.released = true;
    }
}

impl Drop for JfrCheckpointWriter {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        if !self.is_valid() || !self.header {
            self.release();
            return;
        }
        if self.count() == 0 {
            // nothing written; rewind and release
            self.seek(self.offset);
            self.release();
            return;
        }
        // finalize header then release
        let size = self.pos - self.seg_start;
        debug_assert!(size > CHECKPOINT_ENTRY_HEADER_SIZE);
        self.write_checkpoint_header();
        self.header = false;
        self.release();
    }
}
