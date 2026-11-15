/*
 * Port of OpenJDK 25 HotSpot Shenandoah Task Queues
 * (share/gc/shenandoah/shenandoahTaskqueue.hpp / shenandoahTaskqueue.cpp /
 *  shenandoahTaskqueue.inline.hpp)
 *
 * ShenandoahMarkTask packs an oop plus optional array chunking
 * (chunk, pow) and skip-live/weak flags into a single u64. Work
 * stealing uses ParallelClaimableQueueSet over per-thread
 * BufferedOverflowTaskQueue instances.
 */

use std::sync::atomic::{AtomicIsize, Ordering};

pub const CHUNK_BITS: u32 = 10;
pub const POW_BITS: u32 = 5;
const OOP_BITS: u32 = 64 - CHUNK_BITS - POW_BITS;
const OOP_SHIFT: u32 = 0;
const POW_SHIFT: u32 = OOP_BITS;
const CHUNK_SHIFT: u32 = OOP_BITS + POW_BITS;
const OOP_EXTRACT_MASK: u64 = (1u64 << OOP_BITS) - 4;
const SKIP_LIVE_EXTRACT_MASK: u64 = 1;
const WEAK_EXTRACT_MASK: u64 = 1 << 1;
const CHUNK_POW_EXTRACT_MASK: u64 = !((1u64 << OOP_BITS) - 1);
const CHUNK_RANGE_MASK: u64 = (1u64 << CHUNK_BITS) - 1;
const POW_RANGE_MASK: u64 = (1u64 << POW_BITS) - 1;

pub const TASKQUEUE_SIZE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShenandoahMarkTask {
    obj: u64,
}

impl ShenandoahMarkTask {
    pub fn new(obj: u64, skip_live: bool, weak: bool) -> Self {
        Self {
            obj: Self::encode_oop(obj, skip_live, weak),
        }
    }

    pub fn chunked(obj: u64, skip_live: bool, weak: bool, chunk: u32, pow: u32) -> Self {
        let enc = Self::encode_oop(obj, skip_live, weak)
            | Self::encode_chunk(chunk)
            | Self::encode_pow(pow);
        debug_assert_eq!(Self::decode_oop(enc), obj);
        debug_assert!(Self::decode_cnt_live(enc) == !skip_live);
        debug_assert_eq!(Self::decode_weak(enc), weak);
        debug_assert_eq!(Self::decode_chunk(enc), chunk as u64);
        debug_assert_eq!(Self::decode_pow(enc), pow as u64);
        debug_assert!(!Self::decode_not_chunked(enc));
        Self { obj: enc }
    }

    #[inline]
    fn decode_oop(val: u64) -> u64 {
        val & OOP_EXTRACT_MASK
    }

    #[inline]
    fn decode_not_chunked(val: u64) -> bool {
        (val & CHUNK_POW_EXTRACT_MASK) == 0
    }

    #[inline]
    fn decode_chunk(val: u64) -> u64 {
        (val >> CHUNK_SHIFT) & CHUNK_RANGE_MASK
    }

    #[inline]
    fn decode_pow(val: u64) -> u64 {
        (val >> POW_SHIFT) & POW_RANGE_MASK
    }

    #[inline]
    fn decode_weak(val: u64) -> bool {
        (val & WEAK_EXTRACT_MASK) != 0
    }

    #[inline]
    fn decode_cnt_live(val: u64) -> bool {
        (val & SKIP_LIVE_EXTRACT_MASK) == 0
    }

    #[inline]
    fn encode_oop(obj: u64, skip_live: bool, weak: bool) -> u64 {
        debug_assert_eq!(OOP_SHIFT, 0);
        let mut encoded = obj & OOP_EXTRACT_MASK;
        if skip_live {
            encoded |= SKIP_LIVE_EXTRACT_MASK;
        }
        if weak {
            encoded |= WEAK_EXTRACT_MASK;
        }
        encoded
    }

    #[inline]
    fn encode_chunk(chunk: u32) -> u64 {
        (chunk as u64) << CHUNK_SHIFT
    }

    #[inline]
    fn encode_pow(pow: u32) -> u64 {
        (pow as u64) << POW_SHIFT
    }

    #[inline]
    pub fn obj(&self) -> u64 {
        Self::decode_oop(self.obj)
    }

    #[inline]
    pub fn chunk(&self) -> u64 {
        Self::decode_chunk(self.obj)
    }

    #[inline]
    pub fn pow(&self) -> u64 {
        Self::decode_pow(self.obj)
    }

    #[inline]
    pub fn is_not_chunked(&self) -> bool {
        Self::decode_not_chunked(self.obj)
    }

    #[inline]
    pub fn is_weak(&self) -> bool {
        Self::decode_weak(self.obj)
    }

    #[inline]
    pub fn count_liveness(&self) -> bool {
        Self::decode_cnt_live(self.obj)
    }

    pub fn max_addressable() -> u64 {
        1u64 << OOP_BITS
    }

    pub fn chunk_size() -> u64 {
        1u64 << CHUNK_BITS
    }
}

pub struct BufferedOverflowTaskQueue {
    buf_empty: bool,
    elem: Option<ShenandoahMarkTask>,
    local: Vec<ShenandoahMarkTask>,
    overflow: Vec<ShenandoahMarkTask>,
}

impl BufferedOverflowTaskQueue {
    pub fn new() -> Self {
        Self {
            buf_empty: true,
            elem: None,
            local: Vec::with_capacity(TASKQUEUE_SIZE),
            overflow: Vec::new(),
        }
    }

    pub fn push(&mut self, t: ShenandoahMarkTask) {
        if self.buf_empty {
            self.elem = Some(t);
            self.buf_empty = false;
        } else {
            self.local.push(self.elem.take().unwrap());
            self.elem = Some(t);
        }
    }

    pub fn pop(&mut self) -> Option<ShenandoahMarkTask> {
        if !self.buf_empty {
            self.buf_empty = true;
            return self.elem.take();
        }
        self.local.pop().or_else(|| self.overflow.pop())
    }

    pub fn clear(&mut self) {
        self.buf_empty = true;
        self.elem = None;
        self.local.clear();
        self.overflow.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.buf_empty && self.local.is_empty() && self.overflow.is_empty()
    }

    pub fn size(&self) -> usize {
        self.local.len() + self.overflow.len() + if self.buf_empty { 0 } else { 1 }
    }
}

impl Default for BufferedOverflowTaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

pub type ShenandoahObjToScanQueue = BufferedOverflowTaskQueue;

pub struct ParallelClaimableQueueSet {
    queues: Vec<ShenandoahObjToScanQueue>,
    claimed_index: AtomicIsize,
    reserved: usize,
}

impl ParallelClaimableQueueSet {
    pub fn new(n: usize) -> Self {
        Self {
            queues: (0..n).map(|_| ShenandoahObjToScanQueue::new()).collect(),
            claimed_index: AtomicIsize::new(0),
            reserved: 0,
        }
    }

    pub fn size(&self) -> usize {
        self.queues.len()
    }

    pub fn queue(&self, index: usize) -> &ShenandoahObjToScanQueue {
        &self.queues[index]
    }

    pub fn queue_mut(&mut self, index: usize) -> &mut ShenandoahObjToScanQueue {
        &mut self.queues[index]
    }

    pub fn clear_claimed(&self) {
        self.claimed_index.store(0, Ordering::Relaxed);
    }

    pub fn reserve(&mut self, n: usize) {
        debug_assert!(n <= self.queues.len());
        self.claimed_index.store(n as isize, Ordering::Relaxed);
        self.reserved = n;
    }

    pub fn get_reserved(&self) -> usize {
        self.reserved
    }

    pub fn claim_next(&self) -> Option<usize> {
        let size = self.queues.len() as isize;
        let mut old = self.claimed_index.load(Ordering::Relaxed);
        if old >= size {
            return None;
        }
        loop {
            let index = old + 1;
            match self.claimed_index.compare_exchange_weak(
                old,
                index,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if index <= size {
                        return Some((index - 1) as usize);
                    } else {
                        return None;
                    }
                }
                Err(cur) => old = cur,
            }
            if old >= size {
                return None;
            }
        }
    }
}

pub struct ShenandoahObjToScanQueueSet {
    inner: ParallelClaimableQueueSet,
}

impl ShenandoahObjToScanQueueSet {
    pub fn new(n: usize) -> Self {
        Self {
            inner: ParallelClaimableQueueSet::new(n),
        }
    }

    pub fn size(&self) -> usize {
        self.inner.size()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.queues.iter().all(|q| q.is_empty())
    }

    pub fn clear(&mut self) {
        for q in self.inner.queues.iter_mut() {
            q.clear();
        }
        self.inner.clear_claimed();
    }

    pub fn claim_next(&self) -> Option<usize> {
        self.inner.claim_next()
    }

    pub fn reserve(&mut self, n: usize) {
        self.inner.reserve(n);
    }

    pub fn queue_mut(&mut self, index: usize) -> &mut ShenandoahObjToScanQueue {
        self.inner.queue_mut(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_task_encoding() {
        let obj = 0x0000_7f00_0000_1234u64;
        let t = ShenandoahMarkTask::new(obj, false, false);
        assert_eq!(t.obj(), obj);
        assert!(t.is_not_chunked());
        assert!(t.count_liveness());
        assert!(!t.is_weak());

        let tw = ShenandoahMarkTask::new(obj, true, true);
        assert_eq!(tw.obj(), obj);
        assert!(!tw.count_liveness());
        assert!(tw.is_weak());

        let tc = ShenandoahMarkTask::chunked(obj, false, false, 3, 5);
        assert_eq!(tc.obj(), obj);
        assert!(!tc.is_not_chunked());
        assert_eq!(tc.chunk(), 3);
        assert_eq!(tc.pow(), 5);
    }

    #[test]
    fn buffered_queue_lifo_order() {
        let mut q = BufferedOverflowTaskQueue::new();
        assert!(q.is_empty());
        q.push(ShenandoahMarkTask::new(0x10, false, false));
        q.push(ShenandoahMarkTask::new(0x20, false, false));
        q.push(ShenandoahMarkTask::new(0x30, false, false));
        assert_eq!(q.pop().unwrap().obj(), 0x30);
        assert_eq!(q.pop().unwrap().obj(), 0x20);
        assert_eq!(q.pop().unwrap().obj(), 0x10);
        assert!(q.pop().is_none());
    }

    #[test]
    fn claim_next_distributes() {
        let set = ParallelClaimableQueueSet::new(4);
        let mut claimed = Vec::new();
        while let Some(i) = set.claim_next() {
            claimed.push(i);
        }
        assert_eq!(claimed, vec![0, 1, 2, 3]);
        assert!(set.claim_next().is_none());
    }
}
