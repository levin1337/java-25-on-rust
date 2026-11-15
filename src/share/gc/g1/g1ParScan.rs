/*
 * Port of OpenJDK 25 HotSpot parallel marking structures:
 * G1ParScanThreadState (g1ParScanThreadState.hpp),
 * G1ScannerTasksQueue / G1ScannerTasksQueueSet (g1SATBMarkQueueSet /
 * g1ParScanThreadState task queue part), G1ParTask (g1ParTask.hpp in
 * share/gc/g1) and the evacuation-failure bookkeeping
 * (g1EvacFailureRegions.hpp, g1EvacStats.hpp).
 *
 * Oops are `usize` handles; region attributes come from
 * `G1HeapRegionAttr`; closures are injected via the `G1OopClosure`
 * trait so `memory/heap.rs` can plug in real marking later.
 */

use super::g1ConcurrentMarkBitMap::G1ConcurrentMarkBitMap;
use super::g1OopClosures::{G1OopClosure, ScanTask};
use super::g1OopStarChunkedList::G1OopStarChunkedList;
use super::heapRegion::G1HeapRegionAttr;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// `G1ScannerTasksQueue`: per-worker task queue with work stealing.
#[derive(Debug, Default)]
pub struct G1ScannerTasksQueue {
    tasks: VecDeque<ScanTask>,
    /// Number of tasks stolen by other workers (stats).
    pub stolen_count: u64,
}

impl G1ScannerTasksQueue {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn push(&mut self, task: ScanTask) {
        self.tasks.push_back(task);
    }

    #[inline]
    pub fn pop(&mut self) -> Option<ScanTask> {
        self.tasks.pop_front()
    }

    /// `steal`: takes up to `max` tasks from the tail (LIFO side for
    /// the owner, FIFO-ish for thieves like HotSpot's deque).
    pub fn steal(&mut self, max: usize) -> Vec<ScanTask> {
        let n = self.tasks.len().min(max);
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            if let Some(t) = self.tasks.pop_back() {
                out.push(t);
            }
        }
        self.stolen_count += out.len() as u64;
        out
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.tasks.len()
    }
}

/// `G1ScannerTasksQueueSet`: set of per-worker queues.
#[derive(Debug)]
pub struct G1ScannerTasksQueueSet {
    queues: Vec<Mutex<G1ScannerTasksQueue>>,
}

impl G1ScannerTasksQueueSet {
    pub fn new(num_workers: usize) -> Self {
        Self {
            queues: (0..num_workers).map(|_| Mutex::new(G1ScannerTasksQueue::new())).collect(),
        }
    }

    pub fn push(&self, worker: usize, task: ScanTask) {
        if let Some(q) = self.queues.get(worker) {
            q.lock().unwrap().push(task);
        }
    }

    pub fn pop(&self, worker: usize) -> Option<ScanTask> {
        self.queues.get(worker)?.lock().unwrap().pop()
    }

    /// `steal_and_trim_queue` analogue: steal from `victim`.
    pub fn steal(&self, victim: usize, max: usize) -> Vec<ScanTask> {
        self.queues
            .get(victim)
            .map(|q| q.lock().unwrap().steal(max))
            .unwrap_or_default()
    }

    /// Find a non-empty queue to steal from (round-robin start at
    /// `start`).
    pub fn find_victim(&self, start: usize, me: usize) -> Option<usize> {
        let n = self.queues.len();
        for i in 0..n {
            let v = (start + i) % n;
            if v != me && !self.queues[v].lock().unwrap().is_empty() {
                return Some(v);
            }
        }
        None
    }

    pub fn num_workers(&self) -> usize {
        self.queues.len()
    }
}

/// Per-region card statistics accumulated by a scan worker,
/// mirroring `G1GCParPhases` card-set statistics.
#[derive(Debug, Default, Clone)]
pub struct G1CardStats {
    /// Cards scanned by this worker.
    pub scanned_cards: u64,
    /// Cards claimed but not yet scanned.
    pub claimed_cards: u64,
    /// Dirty cards discovered during scanning.
    pub dirty_cards: u64,
}

/// `G1ParScanThreadState`: per-worker state for the parallel
/// evacuation/marking task.
#[derive(Debug)]
pub struct G1ParScanThreadState {
    worker_id: usize,
    /// Tenuring threshold captured at GC start.
    tenuring_threshold: u32,
    /// Main scan stack of oops to visit.
    pub stack: G1OopStarChunkedList<usize>,
    /// Stack of nmethod/code roots to scan.
    pub nmethod_stack: G1OopStarChunkedList<usize>,
    /// Trim queue: entries deferred while stack is over threshold.
    pub trim_queue: VecDeque<usize>,
    /// Overflow worklist for partial arrays / deep objects.
    pub overflow_stack: Vec<ScanTask>,
    /// Cards claimed but not yet processed.
    pub card_stats: G1CardStats,
    /// Last card enqueued (dedup fast path).
    last_enqueued_card: Option<u64>,
    /// Stack trim thresholds (HotSpot ergonomics).
    stack_trim_upper_threshold: usize,
    stack_trim_lower_threshold: usize,
    /// Words surviving in young gen (stats flushed at GC end).
    surviving_words_length: usize,
    /// Old gen full flag affecting promotion decisions.
    pub old_gen_is_full: bool,
    /// Evacuation failure enqueued cards count.
    pub evac_failure_enqueued_cards: u64,
}

impl G1ParScanThreadState {
    pub const DEFAULT_CHUNK_SIZE: usize = 64;
    pub const DEFAULT_TRIM_UPPER: usize = 4096;
    pub const DEFAULT_TRIM_LOWER: usize = 2048;

    pub fn new(worker_id: usize, tenuring_threshold: u32) -> Self {
        Self {
            worker_id,
            tenuring_threshold,
            stack: G1OopStarChunkedList::new(Self::DEFAULT_CHUNK_SIZE),
            nmethod_stack: G1OopStarChunkedList::new(Self::DEFAULT_CHUNK_SIZE),
            trim_queue: VecDeque::new(),
            overflow_stack: Vec::new(),
            card_stats: G1CardStats::default(),
            last_enqueued_card: None,
            stack_trim_upper_threshold: Self::DEFAULT_TRIM_UPPER,
            stack_trim_lower_threshold: Self::DEFAULT_TRIM_LOWER,
            surviving_words_length: 0,
            old_gen_is_full: false,
            evac_failure_enqueued_cards: 0,
        }
    }

    #[inline]
    pub fn worker_id(&self) -> usize {
        self.worker_id
    }

    #[inline]
    pub fn tenuring_threshold(&self) -> u32 {
        self.tenuring_threshold
    }

    #[inline]
    pub fn queue_is_empty(&self) -> bool {
        self.stack.is_empty()
            && self.nmethod_stack.is_empty()
            && self.trim_queue.is_empty()
            && self.overflow_stack.is_empty()
    }

    /// `push_on_queue`.
    pub fn push_on_queue(&mut self, task: ScanTask) {
        self.overflow_stack.push(task);
    }

    /// `trim_queue`: moves stack entries to the trim queue when the
    /// stack exceeds the upper threshold.
    pub fn trim_queue_to_threshold(&mut self, threshold: usize) {
        while self.stack.len() > threshold {
            if let Some(item) = self.stack.pop() {
                self.trim_queue.push_back(item);
            } else {
                break;
            }
        }
    }

    /// `trim_queue` at the configured thresholds.
    pub fn trim_queue(&mut self) {
        if self.needs_partial_trimming() {
            self.trim_queue_to_threshold(self.stack_trim_lower_threshold);
        }
    }

    #[inline]
    pub fn needs_partial_trimming(&self) -> bool {
        self.stack.len() >= self.stack_trim_upper_threshold
    }

    /// Move trim-queue entries back onto the stack.
    pub fn untrim_queue(&mut self) {
        while let Some(item) = self.trim_queue.pop_front() {
            self.stack.push(item);
        }
    }

    /// `steal_and_trim_queue`: steal tasks from another worker's
    /// queue set and trim own queue.
    pub fn steal_and_trim_queue(&mut self, queue_set: &G1ScannerTasksQueueSet, me: usize) {
        if self.queue_is_empty() {
            if let Some(victim) = queue_set.find_victim(me.wrapping_add(1), me) {
                for task in queue_set.steal(victim, 32) {
                    self.overflow_stack.push(task);
                }
            }
        }
        self.trim_queue();
    }

    /// `flush_stats`: drains per-worker stats into the returned
    /// aggregate (HotSpot returns surviving words + per-worker stats).
    pub fn flush_stats(&mut self) -> G1CardStats {
        std::mem::take(&mut self.card_stats)
    }

    /// `record_evacuation_failed_region` bookkeeping.
    pub fn record_evacuation_failed_region(&mut self) {
        self.evac_failure_enqueued_cards += 1;
    }

    /// `reset` for reuse across GC cycles.
    pub fn reset(&mut self) {
        self.stack.clear();
        self.nmethod_stack.clear();
        self.trim_queue.clear();
        self.overflow_stack.clear();
        self.card_stats = G1CardStats::default();
        self.last_enqueued_card = None;
        self.surviving_words_length = 0;
        self.old_gen_is_full = false;
        self.evac_failure_enqueued_cards = 0;
    }
}

/// `G1ParScanThreadStateSet`: the set of all workers' states.
#[derive(Debug)]
pub struct G1ParScanThreadStateSet {
    states: Vec<G1ParScanThreadState>,
    task_queues: G1ScannerTasksQueueSet,
    flushed: bool,
}

impl G1ParScanThreadStateSet {
    pub fn new(num_workers: usize, tenuring_threshold: u32) -> Self {
        Self {
            states: (0..num_workers)
                .map(|i| G1ParScanThreadState::new(i, tenuring_threshold))
                .collect(),
            task_queues: G1ScannerTasksQueueSet::new(num_workers),
            flushed: false,
        }
    }

    #[inline]
    pub fn state(&self, worker: usize) -> Option<&G1ParScanThreadState> {
        self.states.get(worker)
    }

    #[inline]
    pub fn state_mut(&mut self, worker: usize) -> Option<&mut G1ParScanThreadState> {
        self.states.get_mut(worker)
    }

    #[inline]
    pub fn task_queues(&self) -> &G1ScannerTasksQueueSet {
        &self.task_queues
    }

    pub fn num_workers(&self) -> usize {
        self.states.len()
    }

    /// `flush_stats`: aggregates per-worker card stats.
    pub fn flush_stats(&mut self) -> G1CardStats {
        let mut total = G1CardStats::default();
        for s in &mut self.states {
            let st = s.flush_stats();
            total.scanned_cards += st.scanned_cards;
            total.claimed_cards += st.claimed_cards;
            total.dirty_cards += st.dirty_cards;
        }
        self.flushed = true;
        total
    }

    /// True when every worker's queues are empty.
    pub fn all_queues_empty(&self) -> bool {
        self.states.iter().all(|s| s.queue_is_empty())
    }
}

/// `G1EvacFailureRegions`: regions that failed evacuation, kept for
/// the GC end-of-pause handling (revert to non-CSet).
#[derive(Debug, Default)]
pub struct G1EvacFailureRegions {
    regions: Vec<usize>,
    /// worker id -> region that worker failed on (for stats).
    worker_failures: Vec<u64>,
}

impl G1EvacFailureRegions {
    pub fn new(num_workers: usize) -> Self {
        Self { regions: Vec::new(), worker_failures: vec![0; num_workers] }
    }

    /// `record`/`add`: registers a failed region.
    pub fn add(&mut self, region: usize, worker: usize, _pinned: bool) {
        if !self.regions.contains(&region) {
            self.regions.push(region);
        }
        if worker < self.worker_failures.len() {
            self.worker_failures[worker] += 1;
        }
    }

    pub fn contains(&self, region: usize) -> bool {
        self.regions.contains(&region)
    }

    pub fn regions(&self) -> &[usize] {
        &self.regions
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// `clear` for next GC.
    pub fn clear(&mut self) {
        self.regions.clear();
        for w in &mut self.worker_failures {
            *w = 0;
        }
    }
}

/// `G1ParTask`: the parallel GC task skeleton — owns the worker state
/// set, the shared mark bitmap, the collection set claim iterator,
/// and runs `G1ParScanThreadState`s until queues drain.
///
/// Real scanning is delegated to `G1OopClosure` implementations
/// provided by the caller.
pub struct G1ParTask<'a, C: G1OopClosure> {
    pub states: G1ParScanThreadStateSet,
    pub mark_bitmap: Option<&'a mut G1ConcurrentMarkBitMap>,
    closure: C,
    /// Regions claimed so far (atomic work distribution).
    next_region: AtomicUsize,
    collection_set: Vec<usize>,
}

impl<'a, C: G1OopClosure> G1ParTask<'a, C> {
    pub fn new(
        num_workers: usize,
        tenuring_threshold: u32,
        collection_set: Vec<usize>,
        closure: C,
    ) -> Self {
        Self {
            states: G1ParScanThreadStateSet::new(num_workers, tenuring_threshold),
            mark_bitmap: None,
            closure,
            next_region: AtomicUsize::new(0),
            collection_set,
        }
    }

    /// `claim_next_region`: work distribution across workers.
    pub fn claim_next_region(&self) -> Option<usize> {
        let idx = self.next_region.fetch_add(1, Ordering::Relaxed);
        self.collection_set.get(idx).copied()
    }

    /// `set_mark_bitmap`.
    pub fn set_mark_bitmap(&mut self, bitmap: &'a mut G1ConcurrentMarkBitMap) {
        self.mark_bitmap = Some(bitmap);
    }

    /// Process one worker step: pops tasks from its stack and feeds
    /// them to the closure. Returns entries processed.
    pub fn process_worker_step(&mut self, worker: usize) -> usize {
        let mut processed = 0;
        let Some(state) = self.states.state_mut(worker) else {
            return 0;
        };
        // Drain overflow tasks and stack entries.
        while let Some(task) = state.overflow_stack.pop() {
            self.closure.scan_task(task, state);
            processed += 1;
        }
        while let Some(oop) = state.stack.pop() {
            self.closure.do_oop(oop, state);
            processed += 1;
        }
        while let Some(nm) = state.nmethod_stack.pop() {
            self.closure.scan_nmethod(nm, state);
            processed += 1;
        }
        processed
    }

    /// `is_work_empty`: no worker has pending work.
    pub fn is_work_empty(&self) -> bool {
        self.states.all_queues_empty()
    }

    /// Number of workers.
    pub fn num_workers(&self) -> usize {
        self.states.num_workers()
    }

    /// Access the closure (for setup of roots etc.).
    pub fn closure(&self) -> &C {
        &self.closure
    }

    /// Mutable closure access (barrier callbacks may need it).
    pub fn closure_mut(&mut self) -> &mut C {
        &mut self.closure
    }
}

/// Shared reference-counted wrapper for running `G1ParTask` across
/// scoped threads.
pub type SharedParTask<'a, C> = Arc<Mutex<G1ParTask<'a, C>>>;
