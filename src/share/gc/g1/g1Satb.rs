/*
 * Port of OpenJDK 25 HotSpot G1SATB structures
 * (share/gc/g1/g1SATBMarkQueueSet.hpp, g1SATBQueue.hpp and the SATB
 * half of g1ThreadLocalData.hpp).
 *
 * SATB (snapshot-at-the-beginning) marking records the pre-write
 * value of every reference field into per-thread buffers; the
 * concurrent marker drains the buffers and marks those objects.
 *
 * Oops are represented as `usize` handles consistent with the rest
 * of this port.
 */

use std::collections::VecDeque;

/// Buffer threshold constants mirroring HotSpot ergonomics:
/// `SATBProcessBufferThreshold`-style soft limit before a buffer is
/// enqueued for processing.
pub const SATB_PROCESS_BUFFER_THRESHOLD: usize = 256;
/// Hard cap mirroring `SATB_BUFFER_MEM_SIZE`-style bounding (4 MiB of
/// pointers ~= 512k entries on 64-bit; we keep a smaller default).
pub const SATB_BUFFER_MAX_ENTRIES: usize = 1 << 16;

/// `G1SATBMarkQueue`: a single thread's SATB buffer.
#[derive(Debug, Default)]
pub struct G1SATBMarkQueue {
    buffer: VecDeque<usize>,
    /// Enqueued-but-not-yet-drained entries count (for stats).
    enqueued: usize,
    /// Entries dropped due to buffer overflow (HotSpot logs these;
    /// here we count them).
    pub dropped: usize,
}

impl G1SATBMarkQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// `enqueue`: adds an oop to the buffer. Returns `false` if the
    /// buffer overflowed (entry dropped) — the caller should treat
    /// this as a hint to start an SATB pause or enlarge buffers.
    pub fn enqueue(&mut self, oop: usize) -> bool {
        if self.buffer.len() >= SATB_BUFFER_MAX_ENTRIES {
            self.dropped += 1;
            return false;
        }
        self.buffer.push_back(oop);
        self.enqueued += 1;
        true
    }

    /// `has_buffer` / `is_empty`.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    #[inline]
    pub fn enqueued(&self) -> usize {
        self.enqueued
    }

    /// `apply_to_buffer`: drains the buffer calling `f` per oop.
    /// Returns the number of processed entries.
    pub fn apply_to_buffer<F: FnMut(usize)>(&mut self, mut f: F) -> usize {
        let n = self.buffer.len();
        while let Some(oop) = self.buffer.pop_front() {
            f(oop);
        }
        self.enqueued = self.enqueued.saturating_sub(n);
        n
    }

    /// `flush` / partial drain: processes up to `limit` entries.
    pub fn flush<F: FnMut(usize)>(&mut self, limit: usize, mut f: F) -> usize {
        let mut done = 0;
        while done < limit {
            match self.buffer.pop_front() {
                Some(oop) => {
                    f(oop);
                    done += 1;
                }
                None => break,
            }
        }
        self.enqueued = self.enqueued.saturating_sub(done);
        done
    }

    /// `reset` / `clear`.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.enqueued = 0;
        self.dropped = 0;
    }

    /// `merge` another buffer into this one.
    pub fn merge(&mut self, other: &mut Self) {
        while let Some(oop) = other.buffer.pop_front() {
            if self.buffer.len() >= SATB_BUFFER_MAX_ENTRIES {
                self.dropped += 1;
                break;
            }
            self.buffer.push_back(oop);
        }
        self.enqueued += other.enqueued.saturating_sub(other.buffer.len());
        self.dropped += other.dropped;
        other.clear();
    }
}

/// Refinement/marking statistics per queue, mirroring
/// `G1SATBMarkQueueSetRefinementStats`.
#[derive(Debug, Default, Clone)]
pub struct G1SATBRefinementStats {
    /// Entries processed by refinement threads.
    pub processed: u64,
    /// Entries enqueued during refinement.
    pub enqueued: u64,
    /// Buffers that had to be re-enqueued because the queue set was
    /// full.
    pub re_enqueued: u64,
}

/// `G1SATBMarkQueueSet`: the set of all threads' SATB buffers plus the
/// shared "completed" buffer list used by the marker.
#[derive(Debug, Default)]
pub struct G1SATBMarkQueueSet {
    queues: Vec<G1SATBMarkQueue>,
    /// Buffers that completed threads handed back.
    completed: Vec<G1SATBMarkQueue>,
    pub stats: G1SATBRefinementStats,
}

impl G1SATBMarkQueueSet {
    pub fn new(num_threads: usize) -> Self {
        Self {
            queues: (0..num_threads).map(|_| G1SATBMarkQueue::new()).collect(),
            completed: Vec::new(),
            stats: G1SATBRefinementStats::default(),
        }
    }

    #[inline]
    pub fn queue(&self, thread: usize) -> Option<&G1SATBMarkQueue> {
        self.queues.get(thread)
    }

    #[inline]
    pub fn queue_mut(&mut self, thread: usize) -> Option<&mut G1SATBMarkQueue> {
        self.queues.get_mut(thread)
    }

    /// `enqueue` into a specific thread's buffer.
    pub fn enqueue(&mut self, thread: usize, oop: usize) -> bool {
        match self.queues.get_mut(thread) {
            Some(q) => q.enqueue(oop),
            None => false,
        }
    }

    /// `is_complete`: all buffers drained.
    pub fn is_complete(&self) -> bool {
        self.queues.iter().all(|q| q.is_empty())
            && self.completed.iter().all(|q| q.is_empty())
    }

    /// `has_pending_buffers`.
    pub fn has_pending_buffers(&self) -> bool {
        !self.is_complete()
    }

    /// Total buffered entries across all queues.
    pub fn total_buffered(&self) -> usize {
        self.queues.iter().map(|q| q.len()).sum::<usize>()
            + self.completed.iter().map(|q| q.len()).sum::<usize>()
    }

    /// `flush` all buffers through `f`; returns entries processed.
    pub fn flush_all<F: FnMut(usize)>(&mut self, mut f: F) -> usize {
        let mut done = 0;
        for q in &mut self.queues {
            done += q.apply_to_buffer(&mut f);
        }
        for q in &mut self.completed {
            done += q.apply_to_buffer(&mut f);
        }
        self.stats.processed += done as u64;
        done
    }

    /// Drain a single buffer (used by refinement threads picking work).
    /// Returns number of entries processed.
    pub fn drain_one<F: FnMut(usize)>(&mut self, mut f: F) -> usize {
        // Prefer queues with the most pending work.
        let idx = self
            .queues
            .iter()
            .enumerate()
            .filter(|(_, q)| !q.is_empty())
            .max_by_key(|(_, q)| q.len())
            .map(|(i, _)| i);
        let n = match idx {
            Some(i) => self.queues[i].apply_to_buffer(&mut f),
            None => 0,
        };
        self.stats.processed += n as u64;
        n
    }

    /// `merge`: absorb another queue set (e.g. after a thread exits).
    pub fn merge(&mut self, other: &mut Self) {
        for (i, q) in other.queues.drain(..).enumerate() {
            if let Some(dst) = self.queues.get_mut(i) {
                dst.merge(&mut { let mut q = q; q });
            }
        }
        self.completed.append(&mut other.completed);
    }

    /// `reset` for a new marking cycle.
    pub fn reset(&mut self) {
        for q in &mut self.queues {
            q.clear();
        }
        for q in &mut self.completed {
            q.clear();
        }
        self.stats = G1SATBRefinementStats::default();
    }

    pub fn num_threads(&self) -> usize {
        self.queues.len()
    }
}
