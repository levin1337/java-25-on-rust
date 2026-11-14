/*
 * Port of OpenJDK 25 HotSpot stackChunkOop (share/oops/stackChunkOop.hpp, stackChunkOop.cpp)
 *
 * StackChunkOop holds serialized stack frames of suspended Project Loom continuations
 * (Virtual Threads) in the Java heap.
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackChunkOop {
    sp: usize,
    size: usize,
    max_size: usize,
    pc: u64,
    argsize: usize,
    max_thawing_size: usize,
    lockstack_size: u8,
    flags: u8,
    data: Vec<u64>,
}

impl StackChunkOop {
    pub const FLAG_HAS_INTERPRETED_FRAMES: u8 = 1 << 0;
    pub const FLAG_CLAIM_RELATIVIZE: u8 = 1 << 1;
    pub const FLAG_NOTIFY_RELATIVIZE: u8 = 1 << 2;
    pub const FLAG_GC_MODE: u8 = 1 << 3;
    pub const FLAG_HAS_BITMAP: u8 = 1 << 4;
    pub const FLAG_HAS_LOCKSTACK: u8 = 1 << 5;
    pub const FLAG_PREEMPTED: u8 = 1 << 6;

    pub fn new(max_size: usize) -> Self {
        Self {
            sp: 0,
            size: 0,
            max_size,
            pc: 0,
            argsize: 0,
            max_thawing_size: 0,
            lockstack_size: 0,
            flags: 0,
            data: Vec::with_capacity(max_size),
        }
    }

    #[inline]
    pub fn sp(&self) -> usize {
        self.sp
    }

    #[inline]
    pub fn set_sp(&mut self, sp: usize) {
        self.sp = sp;
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    #[inline]
    pub fn max_size(&self) -> usize {
        self.max_size
    }

    #[inline]
    pub fn pc(&self) -> u64 {
        self.pc
    }

    #[inline]
    pub fn set_pc(&mut self, pc: u64) {
        self.pc = pc;
    }

    #[inline]
    pub fn argsize(&self) -> usize {
        self.argsize
    }

    #[inline]
    pub fn set_argsize(&mut self, argsize: usize) {
        self.argsize = argsize;
    }

    #[inline]
    pub fn max_thawing_size(&self) -> usize {
        self.max_thawing_size
    }

    #[inline]
    pub fn set_max_thawing_size(&mut self, size: usize) {
        self.max_thawing_size = size;
    }

    #[inline]
    pub fn lockstack_size(&self) -> u8 {
        self.lockstack_size
    }

    #[inline]
    pub fn set_lockstack_size(&mut self, size: u8) {
        self.lockstack_size = size;
    }

    #[inline]
    pub fn flags(&self) -> u8 {
        self.flags
    }

    #[inline]
    pub fn set_flags(&mut self, flags: u8) {
        self.flags = flags;
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[inline]
    pub fn has_interpreted_frames(&self) -> bool {
        (self.flags & Self::FLAG_HAS_INTERPRETED_FRAMES) != 0
    }

    #[inline]
    pub fn is_preempted(&self) -> bool {
        (self.flags & Self::FLAG_PREEMPTED) != 0
    }

    #[inline]
    pub fn gc_mode(&self) -> bool {
        (self.flags & Self::FLAG_GC_MODE) != 0
    }

    #[inline]
    pub fn set_gc_mode(&mut self, val: bool) {
        if val {
            self.flags |= Self::FLAG_GC_MODE;
        } else {
            self.flags &= !Self::FLAG_GC_MODE;
        }
    }

    #[inline]
    pub fn has_bitmap(&self) -> bool {
        (self.flags & Self::FLAG_HAS_BITMAP) != 0
    }

    #[inline]
    pub fn has_lockstack(&self) -> bool {
        (self.flags & Self::FLAG_HAS_LOCKSTACK) != 0
    }

    pub fn push_words(&mut self, words: &[u64]) -> Result<(), &'static str> {
        if self.data.len() + words.len() > self.max_size {
            return Err("StackChunkOop overflow");
        }
        self.data.extend_from_slice(words);
        self.size = self.data.len();
        self.sp = self.size;
        Ok(())
    }

    pub fn drain_words(&mut self) -> Vec<u64> {
        self.sp = 0;
        self.size = 0;
        std::mem::take(&mut self.data)
    }

    #[inline]
    pub fn data(&self) -> &[u64] {
        &self.data
    }

    pub fn clear(&mut self) {
        self.sp = 0;
        self.size = 0;
        self.flags = 0;
        self.pc = 0;
        self.argsize = 0;
        self.max_thawing_size = 0;
        self.lockstack_size = 0;
        self.data.clear();
    }
}
