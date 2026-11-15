/*
 * OpenJDK 25 HotSpot jfr/recorder/storage/jfrBuffer.hpp + jfrBuffer.cpp port
 */

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

// u1 _flags bits
pub const FLAG_RETIRED: u8 = 1;
pub const FLAG_TRANSIENT: u8 = 2;
pub const FLAG_LEASE: u8 = 4;

const TOP_CRITICAL_SECTION: usize = usize::MAX;

// JfrBuffer — fixed-capacity byte buffer with a lock-free top/pos protocol.
// `data` is the payload area; pos is the write cursor, top is the
// reader-visible position (top <= pos always).
pub struct JfrBuffer {
    pub data: Vec<u8>,
    pos: AtomicUsize,
    top: AtomicUsize,
    owner: AtomicUsize,
    flags: AtomicU8,
    context: AtomicU8,
    pub header_size: usize,
}

impl JfrBuffer {
    pub fn new(size: usize, header_size: usize) -> JfrBuffer {
        JfrBuffer {
            data: vec![0u8; size],
            pos: AtomicUsize::new(0),
            top: AtomicUsize::new(0),
            owner: AtomicUsize::new(0),
            flags: AtomicU8::new(0),
            context: AtomicU8::new(0),
            header_size,
        }
    }

    pub fn initialize(&mut self, header_size: usize, size: usize) {
        self.header_size = header_size;
        self.data = vec![0u8; size];
        self.pos.store(0, Ordering::Release);
        self.top.store(0, Ordering::Release);
        self.flags.store(0, Ordering::Release);
        self.context.store(0, Ordering::Release);
    }

    pub fn reinitialize(&mut self) {
        self.pos.store(0, Ordering::Release);
        self.top.store(0, Ordering::Release);
        self.flags.store(0, Ordering::Release);
        self.owner.store(0, Ordering::Release);
    }

    // In C++ the payload begins after an in-allocation header; here the
    // bookkeeping fields live in the struct so payload index 0 is start().
    pub fn start(&self) -> usize {
        0
    }

    pub fn end(&self) -> usize {
        self.size()
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn total_size(&self) -> usize {
        self.header_size + self.size()
    }

    pub fn pos(&self) -> usize {
        self.pos.load(Ordering::Acquire)
    }

    pub fn set_pos(&self, new_pos: usize) {
        debug_assert!(new_pos <= self.end());
        self.pos.store(new_pos, Ordering::Release);
    }

    pub fn set_pos_add(&self, size: usize) {
        let p = self.pos();
        self.set_pos(p + size);
    }

    pub fn pos_ptr(&self) -> usize {
        self.pos()
    }

    // stable_top — acquire semantics for pos
    fn stable_top(&self) -> usize {
        let t = self.top.load(Ordering::Acquire);
        let _ = self.pos.load(Ordering::Acquire);
        t
    }

    pub fn top(&self) -> usize {
        let t = self.top.load(Ordering::Acquire);
        if t == TOP_CRITICAL_SECTION {
            // caller must hold critical section; return pos as fallback
            return self.pos.load(Ordering::Acquire);
        }
        t
    }

    pub fn set_top(&self, new_top: usize) {
        self.top.store(new_top, Ordering::Release);
    }

    pub fn acquire_critical_section_top(&self) -> usize {
        loop {
            let cur = self.top.load(Ordering::Acquire);
            if cur == TOP_CRITICAL_SECTION {
                std::hint::spin_loop();
                continue;
            }
            if self
                .top
                .compare_exchange(cur, TOP_CRITICAL_SECTION, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return cur;
            }
        }
    }

    pub fn release_critical_section_top(&self, new_top: usize) {
        debug_assert_ne!(new_top, TOP_CRITICAL_SECTION);
        self.set_top(new_top);
    }

    pub fn free_size(&self) -> usize {
        self.end().saturating_sub(self.pos.load(Ordering::Acquire))
    }

    pub fn unflushed_size(&self) -> usize {
        self.pos() - self.stable_top()
    }

    pub fn empty(&self) -> bool {
        self.pos.load(Ordering::Acquire) == self.start()
    }

    // ---- ownership ----

    pub fn identity(&self) -> usize {
        self.owner.load(Ordering::Acquire)
    }

    pub fn set_identity(&self, id: usize) {
        debug_assert_eq!(self.owner.load(Ordering::Acquire), 0);
        self.owner.store(id, Ordering::Release);
    }

    pub fn acquire(&self, id: usize) {
        loop {
            let cur = self.owner.load(Ordering::Acquire);
            if cur == id {
                return;
            }
            if cur == 0
                && self
                    .owner
                    .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                return;
            }
            std::hint::spin_loop();
        }
    }

    pub fn try_acquire(&self, id: usize) -> bool {
        self.owner
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn acquired_by(&self, id: usize) -> bool {
        self.identity() == id
    }

    pub fn release(&self) {
        self.owner.store(0, Ordering::Release);
    }

    // ---- flags ----

    fn test_flag(&self, flag: u8) -> bool {
        self.flags.load(Ordering::Acquire) & flag == flag
    }

    fn set_flag(&self, flag: u8) {
        self.flags.fetch_or(flag, Ordering::AcqRel);
    }

    fn clear_flag(&self, flag: u8) {
        self.flags.fetch_and(!flag, Ordering::AcqRel);
    }

    pub fn transient(&self) -> bool {
        self.test_flag(FLAG_TRANSIENT)
    }

    pub fn set_transient(&self) {
        self.set_flag(FLAG_TRANSIENT);
    }

    pub fn clear_transient(&self) {
        if self.transient() {
            self.clear_flag(FLAG_TRANSIENT);
        }
    }

    pub fn lease(&self) -> bool {
        self.test_flag(FLAG_LEASE)
    }

    pub fn set_lease(&self) {
        self.set_flag(FLAG_LEASE);
    }

    pub fn clear_lease(&self) {
        if self.lease() {
            self.clear_flag(FLAG_LEASE);
        }
    }

    pub fn retired(&self) -> bool {
        self.test_flag(FLAG_RETIRED)
    }

    pub fn set_retired(&self) {
        self.set_flag(FLAG_RETIRED);
    }

    pub fn clear_retired(&self) {
        self.clear_flag(FLAG_RETIRED);
    }

    pub fn context(&self) -> u8 {
        self.context.load(Ordering::Acquire)
    }

    pub fn set_context(&self, context: u8) {
        self.context.store(context, Ordering::Release);
    }

    pub fn clear_context(&self) {
        self.context.store(0, Ordering::Release);
    }

    // ---- data movement ----

    // JfrBuffer::move — move [top, pos) bytes into `to` at its pos.
    pub fn move_to(&self, to: &JfrBuffer, size: usize) {
        debug_assert!(to.free_size() >= size);
        let current_top = self.acquire_critical_section_top();
        let actual_size = self.pos().saturating_sub(current_top);
        debug_assert!(actual_size <= size);
        if actual_size > 0 {
            let src = self.data[current_top..self.pos()].to_vec();
            let dst = to.pos();
            to.data[dst..dst + actual_size].copy_from_slice(&src);
            to.set_pos(dst + actual_size);
        }
        to.release();
        self.set_pos(self.start());
        self.release_critical_section_top(self.start());
    }

    // JfrBuffer::discard — publish pos as top, return discarded count.
    pub fn discard(&self) -> usize {
        let position = self.pos();
        let current_top = self.stable_top();
        self.set_top(position);
        position - current_top
    }

    // Writable view helpers used by writer code.
    pub fn write_region(&mut self) -> &mut [u8] {
        let pos = self.pos();
        &mut self.data[pos..]
    }

    pub fn bytes(&self, range: std::ops::Range<usize>) -> &[u8] {
        &self.data[range]
    }

    pub fn patch(&mut self, offset: usize, data: &[u8]) {
        self.data[offset..offset + data.len()].copy_from_slice(data);
    }
}
