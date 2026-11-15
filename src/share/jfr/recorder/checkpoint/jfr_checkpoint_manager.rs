/*
 * OpenJDK 25 HotSpot jfr/recorder/checkpoint/jfrCheckpointManager.hpp + .cpp port
 *
 * Buffer management for checkpoint writers:
 *  - global mspace: epoch live lists + free list (512k buffers)
 *  - thread-local mspace: per-thread buffers
 *  - virtual-thread-local mspace: per-vthread buffers
 *
 * write()/flush_type_set() serialize live-list buffers into the chunk
 * writer as checkpoint events.
 */

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::share::jfr::recorder::repository::jfr_chunk_writer::JfrChunkWriter;
use crate::share::jfr::recorder::storage::jfr_buffer::JfrBuffer;
use crate::share::jfr::utilities::jfr_time::JfrTicks;
use crate::share::jfr::utilities::jfr_types::{EVENT_CHECKPOINT,
    JfrCheckpointBufferKind, JfrCheckpointType, TraceId,
};

const GLOBAL_BUFFER_SIZE: usize = 512 * 1024;
const THREAD_LOCAL_BUFFER_SIZE: usize = 256 * 1024;
const VTHREAD_LOCAL_BUFFER_SIZE: usize = 4 * 1024;
const GLOBAL_BUFFER_PREALLOC: usize = 4;

// Epoch tag for checkpoint buffers (JfrTraceIdEpoch equivalent).
static EPOCH: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn current_epoch() -> u8 {
    EPOCH.load(std::sync::atomic::Ordering::Acquire)
}

pub fn shift_epoch() {
    EPOCH.fetch_xor(1, std::sync::atomic::Ordering::AcqRel);
}

pub fn epoch_changed() -> bool {
    // has_changed_tag_state — set when type tagging state flips; the
    // recorder service sets this on safepoint transitions.
    CHANGED_TAG_STATE.swap(false, std::sync::atomic::Ordering::AcqRel)
}

static CHANGED_TAG_STATE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_tag_state_changed() {
    CHANGED_TAG_STATE.store(true, std::sync::atomic::Ordering::Release);
}

struct EpochLiveLists {
    lists: [Mutex<Vec<JfrBuffer>>; 2],
    free: Mutex<Vec<JfrBuffer>>,
}

impl EpochLiveLists {
    fn new(size: usize, prealloc: usize) -> EpochLiveLists {
        let mut free = Vec::with_capacity(prealloc * 2);
        for _ in 0..prealloc * 2 {
            free.push(JfrBuffer::new(size, 0));
        }
        EpochLiveLists {
            lists: [Mutex::new(Vec::new()), Mutex::new(Vec::new())],
            free: Mutex::new(free),
        }
    }

    fn acquire(&self, size: usize, epoch: u8) -> JfrBuffer {
        let mut buf = {
            let mut free = self.free.lock().unwrap();
            free.pop()
        };
        let mut buf = match buf.take() {
            Some(mut b) => {
                if b.size() < size {
                    b = JfrBuffer::new(size.max(GLOBAL_BUFFER_SIZE), 0);
                } else {
                    b.reinitialize();
                }
                b
            }
            None => JfrBuffer::new(size.max(4096), 0),
        };
        buf.clear_transient();
        buf.clear_retired();
        buf.set_context(epoch);
        buf
    }

    fn release(&self, buf: JfrBuffer, epoch: u8) {
        let idx = (epoch & 1) as usize;
        self.lists[idx].lock().unwrap().push(buf);
    }

    // Drain the live list for `epoch`, invoking f on each buffer's
    // committed content, then recycle buffers to the free list.
    fn iterate<F: FnMut(&JfrBuffer)>(&self, epoch: u8, mut f: F) {
        let idx = (epoch & 1) as usize;
        let buffers = std::mem::take(&mut *self.lists[idx].lock().unwrap());
        for b in &buffers {
            if !b.empty() {
                f(b);
            }
        }
        let mut free = self.free.lock().unwrap();
        for mut b in buffers {
            b.reinitialize();
            free.push(b);
        }
    }

    fn discard_epoch(&self, epoch: u8) {
        let idx = (epoch & 1) as usize;
        let buffers = std::mem::take(&mut *self.lists[idx].lock().unwrap());
        let mut free = self.free.lock().unwrap();
        for mut b in buffers {
            b.reinitialize();
            free.push(b);
        }
    }
}

struct ThreadLocalMspace {
    // tid -> buffer; epoch stored in buffer context
    buffers: Mutex<HashMap<u64, JfrBuffer>>,
    size: usize,
}

impl ThreadLocalMspace {
    fn new(size: usize) -> ThreadLocalMspace {
        ThreadLocalMspace {
            buffers: Mutex::new(HashMap::new()),
            size,
        }
    }

    fn acquire(&self, size: usize, tid: u64) -> JfrBuffer {
        let mut map = self.buffers.lock().unwrap();
        match map.get_mut(&tid) {
            Some(b) => {
                if b.free_size() < size {
                    // migrate: return old to pending list handled by caller
                    // via flush(); here signal by returning a fresh buffer
                    // and stashing the old one in `overflow`.
                    let old = map.remove(&tid).unwrap();
                    let mut nb = JfrBuffer::new(size.max(self.size), 0);
                    nb.reinitialize();
                    // stash old for the writer to migrate — encoded by
                    // returning tuple through overflow map
                    OVERFLOW.lock().unwrap().insert(tid, old);
                    nb.set_context(current_epoch());
                    map.insert(tid, nb.clone_shallow());
                    nb
                } else {
                    b.set_context(current_epoch());
                    // return a handle: we can't clone JfrBuffer; instead the
                    // manager returns an index-based approach — see acquire_buffer
                    unreachable!("use acquire_index")
                }
            }
            None => {
                let mut b = JfrBuffer::new(size.max(self.size), 0);
                b.reinitialize();
                b.set_context(current_epoch());
                map.insert(tid, b);
                unreachable!("use acquire_index")
            }
        }
    }
}

// Because JfrBuffer is not cloneable, thread-local acquisition works on
// indices: the manager hands out a "lease" identified by (tid, epoch).
// The actual JfrBuffer stays in the map; the checkpoint writer accesses it
// through the manager's buffer slot API.
//
// To keep ownership simple, JfrCheckpointWriter for thread-local kinds
// takes the buffer OUT of the map (remove), writes, and release_buffer
// puts it back (or queues it for serialization).

static OVERFLOW: std::sync::LazyLock<Mutex<HashMap<u64, JfrBuffer>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct JfrCheckpointManager {
    global: EpochLiveLists,
    thread_local: Mutex<HashMap<u64, JfrBuffer>>,
    vthread_local: Mutex<HashMap<u64, JfrBuffer>>,
    thread_local_size: usize,
    vthread_local_size: usize,
}

impl JfrCheckpointManager {
    pub fn new() -> JfrCheckpointManager {
        JfrCheckpointManager {
            global: EpochLiveLists::new(GLOBAL_BUFFER_SIZE, GLOBAL_BUFFER_PREALLOC),
            thread_local: Mutex::new(HashMap::new()),
            vthread_local: Mutex::new(HashMap::new()),
            thread_local_size: THREAD_LOCAL_BUFFER_SIZE,
            vthread_local_size: VTHREAD_LOCAL_BUFFER_SIZE,
        }
    }

    fn instance() -> &'static JfrCheckpointManager {
        static INSTANCE: std::sync::OnceLock<JfrCheckpointManager> = std::sync::OnceLock::new();
        INSTANCE.get_or_init(JfrCheckpointManager::new)
    }

    pub fn initialize() -> bool {
        true
    }

    // ---- acquire ----

    // Returns (buffer, seg_start). The buffer is removed from the pool and
    // owned by the caller until release_buffer().
    pub fn acquire(
        kind: JfrCheckpointBufferKind,
        thread: TraceId,
        previous_epoch: bool,
    ) -> (JfrBuffer, usize) {
        let m = Self::instance();
        match kind {
            JfrCheckpointBufferKind::Global => {
                let epoch = if previous_epoch {
                    current_epoch() ^ 1
                } else {
                    current_epoch()
                };
                let buf = m.global.acquire(GLOBAL_BUFFER_SIZE, epoch);
                let seg = buf.pos();
                (buf, seg)
            }
            JfrCheckpointBufferKind::ThreadLocal => {
                let mut map = m.thread_local.lock().unwrap();
                let key = thread as u64;
                let buf = match map.remove(&key) {
                    Some(mut b) => {
                        if b.free_size() < 64 {
                            // too small; keep it queued for serialization
                            m.queue_thread_local(b, key);
                            JfrBuffer::new(m.thread_local_size, 0)
                        } else {
                            b.reinitialize();
                            b
                        }
                    }
                    None => JfrBuffer::new(m.thread_local_size, 0),
                };
                let mut buf = buf;
                buf.reinitialize();
                buf.set_context(current_epoch());
                let seg = buf.pos();
                (buf, seg)
            }
            JfrCheckpointBufferKind::VirtualThreadLocal => {
                let mut map = m.vthread_local.lock().unwrap();
                let key = thread as u64;
                let mut buf = match map.remove(&key) {
                    Some(mut b) => {
                        b.reinitialize();
                        b
                    }
                    None => JfrBuffer::new(m.vthread_local_size, 0),
                };
                buf.set_context(current_epoch());
                let seg = buf.pos();
                (buf, seg)
            }
        }
    }

    pub fn lease_global(previous_epoch: bool, _size: usize) -> (JfrBuffer, usize) {
        let epoch = if previous_epoch {
            current_epoch() ^ 1
        } else {
            current_epoch()
        };
        let buf = Self::instance().global.acquire(GLOBAL_BUFFER_SIZE, epoch);
        let seg = buf.pos();
        (buf, seg)
    }

    // ---- release ----

    pub fn release_buffer(
        buf: JfrBuffer,
        kind: JfrCheckpointBufferKind,
        thread: TraceId,
        previous_epoch: bool,
    ) {
        let m = Self::instance();
        match kind {
            JfrCheckpointBufferKind::Global => {
                let epoch = if previous_epoch {
                    current_epoch() ^ 1
                } else {
                    current_epoch()
                };
                m.global.release(buf, epoch);
            }
            JfrCheckpointBufferKind::ThreadLocal => {
                // thread-local buffers stay checked out to the thread until
                // rotation; store back keyed by thread.
                let mut map = m.thread_local.lock().unwrap();
                map.insert(thread as u64, buf);
            }
            JfrCheckpointBufferKind::VirtualThreadLocal => {
                let mut map = m.vthread_local.lock().unwrap();
                map.insert(thread as u64, buf);
            }
        }
    }

    fn queue_thread_local(&self, buf: JfrBuffer, key: u64) {
        // small leftover buffer: move to pending serialization
        let mut map = self.thread_local.lock().unwrap();
        map.insert(key ^ 0x8000_0000_0000_0000, buf);
    }

    // ---- flush (buffer overflow) ----

    // JfrCheckpointManager::flush — retire `old` (truncating the
    // uncommitted tail) and return a new buffer sized used+requested.
    // `used` = used_size of the writer = pos - seg_start; committed prefix
    // length = old.pos - used.
    pub fn flush(
        mut old: JfrBuffer,
        used: usize,
        requested: usize,
        kind: JfrCheckpointBufferKind,
        thread: TraceId,
        previous_epoch: bool,
    ) -> Option<(JfrBuffer, usize)> {
        let committed_end = old.pos().saturating_sub(used);
        old.set_pos(committed_end);
        old.set_top(committed_end);
        // retire old into the appropriate live list / thread map
        Self::instance().release_buffer_quiet(old, kind, thread, previous_epoch);
        let new_size = used + requested + used / 2 + 64;
        let mut nb = match kind {
            JfrCheckpointBufferKind::Global => {
                let epoch = if previous_epoch {
                    current_epoch() ^ 1
                } else {
                    current_epoch()
                };
                Self::instance().global.acquire(new_size.max(GLOBAL_BUFFER_SIZE), epoch)
            }
            JfrCheckpointBufferKind::ThreadLocal => {
                JfrBuffer::new(new_size.max(THREAD_LOCAL_BUFFER_SIZE), 0)
            }
            JfrCheckpointBufferKind::VirtualThreadLocal => {
                JfrBuffer::new(new_size.max(VTHREAD_LOCAL_BUFFER_SIZE), 0)
            }
        };
        nb.reinitialize();
        nb.set_pos(0);
        Some((nb, 0))
    }

    fn release_buffer_quiet(
        &self,
        buf: JfrBuffer,
        kind: JfrCheckpointBufferKind,
        thread: TraceId,
        previous_epoch: bool,
    ) {
        match kind {
            JfrCheckpointBufferKind::Global => {
                let epoch = if previous_epoch {
                    current_epoch() ^ 1
                } else {
                    current_epoch()
                };
                self.global.release(buf, epoch);
            }
            JfrCheckpointBufferKind::ThreadLocal => {
                let mut map = self.thread_local.lock().unwrap();
                // keep per-thread: replace existing entry
                map.insert(thread as u64, buf);
            }
            JfrCheckpointBufferKind::VirtualThreadLocal => {
                let mut map = self.vthread_local.lock().unwrap();
                map.insert(thread as u64, buf);
            }
        }
    }

    // ---- serialization ----

    // Write all live checkpoint buffers for `epoch` into the chunk writer.
    // Returns number of checkpoint entries processed.
    pub fn write_epoch(&self, epoch: u8, cw: &mut JfrChunkWriter) -> usize {
        let mut elements = 0usize;
        // thread-local buffers
        {
            let mut map = self.thread_local.lock().unwrap();
            let keys: Vec<u64> = map
                .iter()
                .filter(|(_, b)| b.context() == epoch && !b.empty())
                .map(|(k, _)| *k)
                .collect();
            for k in keys {
                if let Some(b) = map.remove(&k) {
                    elements += write_buffer_checkpoints(cw, &b);
                    // recycle
                    let mut free_buf = b;
                    free_buf.reinitialize();
                    self.global.free.lock().unwrap().push(free_buf);
                }
            }
        }
        // vthread-local buffers — collapsed into a single THREADS checkpoint
        {
            let mut map = self.vthread_local.lock().unwrap();
            let keys: Vec<u64> = map
                .iter()
                .filter(|(_, b)| b.context() == epoch && !b.empty())
                .map(|(k, _)| *k)
                .collect();
            for k in keys {
                if let Some(b) = map.remove(&k) {
                    elements += write_buffer_checkpoints(cw, &b);
                    let mut free_buf = b;
                    free_buf.reinitialize();
                    self.global.free.lock().unwrap().push(free_buf);
                }
            }
        }
        // global live list
        self.global.iterate(epoch, |b| {
            elements += write_buffer_checkpoints(cw, b);
        });
        elements
    }

    // JfrCheckpointManager::write — serialize previous epoch list.
    pub fn write(&self, cw: &mut JfrChunkWriter) -> usize {
        let prev = current_epoch() ^ 1;
        self.write_epoch(prev, cw)
    }

    // JfrCheckpointManager::clear — discard previous epoch buffers.
    pub fn clear(&self) {
        let prev = current_epoch() ^ 1;
        self.global.discard_epoch(prev);
        let mut map = self.thread_local.lock().unwrap();
        let keys: Vec<u64> = map
            .iter()
            .filter(|(_, b)| b.context() == prev)
            .map(|(k, _)| *k)
            .collect();
        for k in keys {
            if let Some(mut b) = map.remove(&k) {
                b.reinitialize();
                self.global.free.lock().unwrap().push(b);
            }
        }
        let mut vmap = self.vthread_local.lock().unwrap();
        let vkeys: Vec<u64> = vmap
            .iter()
            .filter(|(_, b)| b.context() == prev)
            .map(|(k, _)| *k)
            .collect();
        for k in vkeys {
            if let Some(mut b) = vmap.remove(&k) {
                b.reinitialize();
                self.global.free.lock().unwrap().push(b);
            }
        }
    }

    // JfrCheckpointManager::shift_epoch
    pub fn shift_epoch(&self) {
        shift_epoch();
    }

    pub fn flush_type_set(&self, cw: &mut JfrChunkWriter) -> usize {
        if epoch_changed() {
            // type set serialization handled by caller (JfrTypeSet)
        }
        // current epoch buffers on demand
        self.write_epoch(current_epoch(), cw)
    }
}

// Serialize one buffer's content as a sequence of checkpoint events.
// Buffer content = concatenated JfrCheckpointEntry records; each record
// starts with an i64 total size.
pub fn write_buffer_checkpoints(cw: &mut JfrChunkWriter, buf: &JfrBuffer) -> usize {
    let data = &buf.data[..buf.pos()];
    let mut processed = 0usize;
    let mut next = 0usize;
    while next + 8 <= data.len() {
        let size = i64::from_be_bytes(data[next..next + 8].try_into().unwrap());
        if size <= 0 || next + size as usize > data.len() {
            break;
        }
        write_checkpoint_event(cw, &data[next..next + size as usize]);
        next += size as usize;
        processed += 1;
    }
    processed
}

// write_checkpoint_event — JfrCheckpointManager.cpp equivalent.
fn write_checkpoint_event(cw: &mut JfrChunkWriter, data: &[u8]) {
    // data = JfrCheckpointEntry: i64 size, i64 start, i64 duration,
    // u32 type, u32 count, payload...
    let size = i64::from_be_bytes(data[0..8].try_into().unwrap());
    let start = i64::from_be_bytes(data[8..16].try_into().unwrap());
    let duration = i64::from_be_bytes(data[16..24].try_into().unwrap());
    let cpt_type = u32::from_be_bytes(data[24..28].try_into().unwrap());
    let type_count = u32::from_be_bytes(data[28..32].try_into().unwrap());
    let payload = &data[32..size as usize];

    let event_begin = cw.current_offset();
    let last_checkpoint = cw.last_checkpoint_offset();
    cw.set_last_checkpoint_offset(event_begin);
    let delta = if last_checkpoint == 0 {
        0
    } else {
        last_checkpoint - event_begin
    };

    // compute event size: size slot + EVENT_CHECKPOINT + fields + payload
    // size slot = compressed size of event_size itself; iterate once.
    let mut event_size = cw.size_in_bytes_u64(EVENT_CHECKPOINT)
        + cw.size_in_bytes_i64(start)
        + cw.size_in_bytes_i64(duration)
        + cw.size_in_bytes_i64(delta)
        + cw.size_in_bytes_u32(cpt_type)
        + cw.size_in_bytes_u32(type_count)
        + payload.len();
    loop {
        let slot = cw.size_in_bytes_u64(event_size as u64);
        let total = slot + event_size;
        if total == event_size + slot && cw.size_in_bytes_u64(total as u64) == slot {
            event_size = total;
            break;
        }
        event_size = total;
    }
    // C++ writes: cw.write(event_size) [compressed u64], EVENT_CHECKPOINT [u8],
    // start, duration, delta, type u4, count u4, payload bytes.
    cw.write_u64(event_size as u64);
    cw.write_u64(crate::share::jfr::utilities::jfr_types::EVENT_CHECKPOINT);
    cw.write_i8(start);
    cw.write_i8(duration);
    cw.write_i8(delta);
    cw.write_u4(cpt_type);
    cw.write_u4(type_count);
    cw.write_bytes(payload);
    debug_assert_eq!(cw.current_offset() - event_begin, event_size as i64);
}

pub type CheckpointBuffer = JfrBuffer;
pub type CheckpointType = JfrCheckpointType;

// ---- static convenience API used by recorder service / storage ----

impl JfrCheckpointManager {
    pub fn acquire_global_buffer(kind: JfrCheckpointBufferKind) -> Option<JfrBuffer> {
        let (buf, _seg) = Self::acquire(kind, 0, false);
        Some(buf)
    }

    pub fn acquire_thread_local_buffer() -> Option<JfrBuffer> {
        let (buf, _seg) = Self::acquire(JfrCheckpointBufferKind::ThreadLocal, 0, false);
        Some(buf)
    }

    pub fn do_checkpoint(_time: crate::share::jfr::utilities::jfr_time::JfrTicks, _metadata: bool, _flushpoint: bool, _statics: bool) -> usize {
        // Emits a checkpoint event into the global checkpoint buffer.
        let mut cw = crate::share::jfr::recorder::checkpoint::jfr_checkpoint_writer::JfrCheckpointWriter::global(
            true,
            JfrCheckpointType::Generic,
            JfrCheckpointBufferKind::Global,
        );
        let n = cw.used_size();
        cw.release();
        n
    }

    pub fn on_stop() {}
    pub fn on_rotation() { shift_epoch(); }
}
