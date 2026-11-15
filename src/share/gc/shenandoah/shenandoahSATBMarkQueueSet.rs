/*
 * Port of OpenJDK 25 HotSpot SATB Mark Queue infrastructure used by
 * Shenandoah
 * (share/gc/shared/ptrQueue.hpp, share/gc/shared/satbMarkQueue.hpp,
 *  share/gc/shared/satbMarkQueue.cpp,
 *  share/gc/shared/bufferNode.hpp, share/gc/shared/bufferNode.cpp,
 *  share/gc/shenandoah/shenandoahSATBMarkQueueSet.hpp/.cpp,
 *  ShenandoahSATBBufferSize = 1K entries per buffer)
 *
 * Each mutator thread owns a SATBMarkQueue: a buffer of oop addresses
 * filled from the top (index starts at capacity and counts down).
 * Full buffers become completed BufferNodes owned by the queue set.
 */

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const SATB_BUFFER_SIZE: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferNode {
    index: usize,
    capacity: usize,
    buffer: [u64; SATB_BUFFER_SIZE],
}

impl BufferNode {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity <= SATB_BUFFER_SIZE);
        Self {
            index: capacity,
            capacity,
            buffer: [0; SATB_BUFFER_SIZE],
        }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn is_empty(&self) -> bool {
        self.index == self.capacity
    }

    pub fn size(&self) -> usize {
        self.capacity - self.index
    }

    pub fn buffer(&self) -> &[u64] {
        &self.buffer[..self.capacity]
    }

    pub fn buffer_mut(&mut self) -> &mut [u64] {
        &mut self.buffer[..self.capacity]
    }
}

pub struct SatbBufferAllocator {
    buffer_capacity: usize,
}

impl SatbBufferAllocator {
    pub fn new(buffer_capacity: usize) -> Self {
        Self { buffer_capacity }
    }

    pub fn buffer_capacity(&self) -> usize {
        self.buffer_capacity
    }

    pub fn allocate(&self) -> BufferNode {
        BufferNode::new(self.buffer_capacity)
    }

    pub fn allocate_buffer(&self) -> Vec<u64> {
        vec![0; self.buffer_capacity]
    }
}

pub struct SatbMarkQueue {
    index: usize,
    buf: Option<Vec<u64>>,
    active: bool,
}

impl SatbMarkQueue {
    pub fn new() -> Self {
        Self {
            index: 0,
            buf: None,
            active: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_active(&mut self, value: bool) {
        self.active = value;
    }

    pub fn buffer(&self) -> Option<&[u64]> {
        self.buf.as_deref()
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn set_index(&mut self, new_index: usize) {
        debug_assert!(new_index <= self.current_capacity());
        self.index = new_index;
    }

    pub fn current_capacity(&self) -> usize {
        self.buf.as_ref().map_or(0, |b| b.len())
    }

    pub fn is_empty(&self) -> bool {
        self.index == self.current_capacity()
    }

    pub fn size(&self) -> usize {
        self.current_capacity().saturating_sub(self.index)
    }

    pub fn set_buffer(&mut self, buffer: Vec<u64>) {
        self.index = buffer.len();
        self.buf = Some(buffer);
    }

    pub fn take_buffer(&mut self) -> Option<Vec<u64>> {
        self.index = 0;
        self.buf.take()
    }

}

pub struct SatbMarkQueueSet {
    allocator: SatbBufferAllocator,
    completed: parking_lot::Mutex<VecDeque<BufferNode>>,
    count_and_process_flag: AtomicUsize,
    process_completed_buffers_threshold: AtomicUsize,
    buffer_enqueue_threshold: AtomicUsize,
    all_active: AtomicUsize,
    queues: parking_lot::Mutex<Vec<SatbMarkQueue>>,
}

unsafe impl Send for SatbMarkQueueSet {}
unsafe impl Sync for SatbMarkQueueSet {}

impl SatbMarkQueueSet {
    pub fn new(buffer_capacity: usize) -> Self {
        Self {
            allocator: SatbBufferAllocator::new(buffer_capacity),
            completed: parking_lot::Mutex::new(VecDeque::new()),
            count_and_process_flag: AtomicUsize::new(0),
            process_completed_buffers_threshold: AtomicUsize::new(usize::MAX),
            buffer_enqueue_threshold: AtomicUsize::new(0),
            all_active: AtomicUsize::new(0),
            queues: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn allocator(&self) -> &SatbBufferAllocator {
        &self.allocator
    }

    pub fn buffer_capacity(&self) -> usize {
        self.allocator.buffer_capacity()
    }

    pub fn is_active(&self) -> bool {
        self.all_active.load(Ordering::Acquire) != 0
    }

    pub fn set_active_all(&self, active: bool) {
        self.all_active.store(active as usize, Ordering::Release);
        let mut queues = self.queues.lock();
        for q in queues.iter_mut() {
            if active {
                debug_assert!(q.is_empty());
            } else {
                q.set_index(q.current_capacity());
            }
            q.set_active(active);
        }
    }

    pub fn register_queue(&self) -> usize {
        let mut queues = self.queues.lock();
        queues.push(SatbMarkQueue::new());
        queues.len() - 1
    }

    pub fn set_process_completed_buffers_threshold(&self, value: usize) {
        let scaled = value.checked_shl(1).unwrap_or(usize::MAX);
        self.process_completed_buffers_threshold
            .store(scaled | 1, Ordering::Relaxed);
    }

    pub fn set_buffer_enqueue_threshold_percentage(&self, value: u32) {
        let size = self.buffer_capacity();
        let enqueue_qty = (size * value as usize) / 100;
        self.buffer_enqueue_threshold
            .store((size - enqueue_qty).max(1), Ordering::Relaxed);
    }

    pub fn completed_buffers_num(&self) -> usize {
        self.count_and_process_flag.load(Ordering::Relaxed) >> 1
    }

    pub fn process_completed_buffers(&self) -> bool {
        (self.count_and_process_flag.load(Ordering::Relaxed) & 1) != 0
    }

    fn increment_count(&self) {
        let threshold = self.process_completed_buffers_threshold.load(Ordering::Relaxed);
        let cf = &self.count_and_process_flag;
        let mut value = cf.load(Ordering::Relaxed);
        loop {
            let old = value;
            value = value.wrapping_add(2);
            debug_assert!(value > old);
            if value > threshold {
                value |= 1;
            }
            match cf.compare_exchange_weak(old, value, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(v) => value = v,
            }
        }
    }

    fn decrement_count(&self) {
        let cf = &self.count_and_process_flag;
        let mut value = cf.load(Ordering::Relaxed);
        loop {
            debug_assert!((value >> 1) != 0, "underflow");
            let old = value;
            value = value.wrapping_sub(2);
            if value <= 1 {
                value = 0;
            }
            match cf.compare_exchange_weak(old, value, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(v) => value = v,
            }
        }
    }

    fn exchange_buffer_with_new(&self, queue: &mut SatbMarkQueue) -> Option<Vec<u64>> {
        let old = queue.take_buffer();
        let cap = self.buffer_capacity();
        queue.set_buffer(vec![0; cap]);
        old
    }

    fn install_new_buffer(&self, queue: &mut SatbMarkQueue) {
        let cap = self.buffer_capacity();
        queue.set_buffer(vec![0; cap]);
    }

    fn handle_zero_index(&self, queue: &mut SatbMarkQueue) {
        if let Some(old) = self.exchange_buffer_with_new(queue) {
            self.enqueue_completed_buffer(old);
        }
    }

    fn should_enqueue_buffer(&self, queue: &SatbMarkQueue) -> bool {
        let threshold = self
            .buffer_enqueue_threshold
            .load(Ordering::Relaxed);
        threshold != 0 && queue.size() >= threshold
    }

    fn try_enqueue(&self, queue: &mut SatbMarkQueue, obj: u64) -> bool {
        if queue.buf.is_none() {
            self.install_new_buffer(queue);
        }
        if queue.index == 0 {
            return false;
        }
        queue.index -= 1;
        if let Some(buf) = queue.buf.as_mut() {
            buf[queue.index] = obj;
        }
        if queue.index == 0 || self.should_enqueue_buffer(queue) {
            self.handle_zero_index(queue);
        }
        true
    }

    fn retry_enqueue(&self, queue: &mut SatbMarkQueue, obj: u64) {
        assert!(self.try_enqueue(queue, obj));
    }

    pub fn enqueue_known_active(&self, thread: usize, obj: u64) {
        debug_assert!(self.is_active());
        debug_assert!(obj != 0);
        let mut queues = self.queues.lock();
        if let Some(queue) = queues.get_mut(thread) {
            debug_assert!(queue.is_active());
            self.retry_enqueue(queue, obj);
        }
    }

    pub fn filter<F: Fn(u64) -> bool>(&self, thread: usize, filter_out: F) {
        let mut queues = self.queues.lock();
        if let Some(queue) = queues.get_mut(thread) {
            Self::apply_filter(&mut queue, filter_out);
        }
    }

    fn apply_filter<F: Fn(u64) -> bool>(queue: &mut SatbMarkQueue, filter_out: F) {
        let Some(buf) = queue.buf.as_mut() else {
            return;
        };
        let index = queue.index;
        let capacity = queue.current_capacity();
        let mut src = index;
        let mut dst = capacity;
        while src < dst {
            let entry = buf[src];
            if !filter_out(entry) {
                loop {
                    dst -= 1;
                    if !(src < dst) {
                        break;
                    }
                    if filter_out(buf[dst]) {
                        buf[dst] = entry;
                        break;
                    }
                }
            }
            src += 1;
        }
        queue.index = dst;
    }

    fn enqueue_completed_buffer(&self, buffer: Vec<u64>) {
        let mut node = BufferNode::new(self.buffer_capacity());
        node.buffer_mut().copy_from_slice(&buffer);
        node.set_index_pub(0);
        {
            let mut completed = self.completed.lock();
            completed.push_back(node);
        }
        self.increment_count();
    }

    fn get_completed_buffer(&self) -> Option<BufferNode> {
        let mut completed = self.completed.lock();
        completed.pop_front().inspect(|_| self.decrement_count())
    }

    pub fn apply_closure_to_completed_buffer<F: FnMut(&[u64])>(&self, mut cl: F) -> bool {
        let Some(node) = self.get_completed_buffer() else {
            return false;
        };
        let cap = node.capacity();
        let idx = node.index();
        cl(&node.buffer()[idx..cap]);
        true
    }

    pub fn flush_queue(&self, thread: usize) {
        let mut queues = self.queues.lock();
        if let Some(queue) = queues.get_mut(thread) {
            if !queue.is_empty() {
                if let Some(buf) = queue.take_buffer() {
                    self.enqueue_completed_buffer(buf);
                }
                self.install_new_buffer(queue);
            }
        }
    }

    pub fn flush_all(&self) {
        let threads: Vec<usize> = (0..self.queues.lock().len()).collect();
        for t in threads {
            self.flush_queue(t);
        }
    }

    pub fn abandon_partial_marking(&self) {
        let mut queues = self.queues.lock();
        for queue in queues.iter_mut() {
            queue.set_index(queue.current_capacity());
            queue.set_active(false);
            queue.buf = None;
        }
        let mut completed = self.completed.lock();
        completed.clear();
        self.count_and_process_flag.store(0, Ordering::Relaxed);
    }

    pub fn all_entries<F: FnMut(u64)>(&self, mut f: F) {
        {
            let mut completed = self.completed.lock();
            for node in completed.iter() {
                let cap = node.capacity();
                let idx = node.index();
                for &e in &node.buffer()[idx..cap] {
                    f(e);
                }
            }
        }
        let queues = self.queues.lock();
        for queue in queues.iter() {
            if let Some(buf) = queue.buffer() {
                for &e in &buf[queue.index()..] {
                    f(e);
                }
            }
        }
    }

    pub fn num_queues(&self) -> usize {
        self.queues.lock().len()
    }
}

impl BufferNode {
    fn set_index_pub(&mut self, index: usize) {
        self.index = index;
    }
}

impl Default for SatbMarkQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn satb_enqueue_and_drain() {
        let set = SatbMarkQueueSet::new(SATB_BUFFER_SIZE);
        let t0 = set.register_queue();
        set.set_active_all(true);
        for i in 1..=10u64 {
            set.enqueue_known_active(t0, i * 8);
        }
        let mut entries = Vec::new();
        set.all_entries(|e| entries.push(e));
        assert_eq!(entries.len(), 10);
        assert!(entries.contains(&8));
        assert!(entries.contains(&80));
        set.abandon_partial_marking();
        assert_eq!(set.completed_buffers_num(), 0);
    }

    #[test]
    fn satb_completed_buffer_threshold() {
        let set = SatbMarkQueueSet::new(SATB_BUFFER_SIZE);
        set.set_process_completed_buffers_threshold(1);
        let t0 = set.register_queue();
        set.set_active_all(true);
        for i in 0..SATB_BUFFER_SIZE {
            set.enqueue_known_active(t0, (i + 1) as u64);
        }
        assert!(set.completed_buffers_num() >= 1);
        let mut count = 0usize;
        while set.apply_closure_to_completed_buffer(|buf| count += buf.len()) {}
        assert_eq!(count, SATB_BUFFER_SIZE);
        assert_eq!(set.completed_buffers_num(), 0);
    }

    #[test]
    fn satb_filter_compaction() {
        let set = SatbMarkQueueSet::new(SATB_BUFFER_SIZE);
        let t0 = set.register_queue();
        set.set_active_all(true);
        for i in 1..=6u64 {
            set.enqueue_known_active(t0, i);
        }
        set.filter(t0, |e| e % 2 == 0);
        let mut entries = Vec::new();
        set.all_entries(|e| entries.push(e));
        entries.sort_unstable();
        assert_eq!(entries, vec![2, 4, 6]);
    }
}
