

use super::{
    object_sample::{ObjectHandle, ObjectSample},
    sample_list::SampleList,
    sample_priority_queue::SamplePriorityQueue,
};
use crate::share::classfile::classLoader::ClassLoader;
use std::sync::{Mutex, MutexGuard};

static SAMPLER: Mutex<Option<Box<ObjectSampler>>> = Mutex::new(None);
static DEAD_SAMPLES: Mutex<bool> = Mutex::new(false);
static LAST_SWEEP: Mutex<i64> = Mutex::new(0);
static UNRESOLVED: Mutex<bool> = Mutex::new(false);

fn sampler_lock() -> MutexGuard<'static, Option<Box<ObjectSampler>>> {
    SAMPLER.lock().unwrap()
}

fn now_ticks() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampledKind {
    Object,
    ObjectArray,
}

#[derive(Clone, Copy, Debug)]
pub struct SampledRef {
    pub kind: SampledKind,
    pub handle: ObjectHandle,
}

pub struct ObjectSampler {
    priority_queue: SamplePriorityQueue,
    list: SampleList,
    total_allocated: usize,
    threshold: usize,
    size: usize,

    refs: Vec<Option<SampledRef>>,
}

impl ObjectSampler {
    fn new(size: usize) -> Self {
        *LAST_SWEEP.lock().unwrap() = now_ticks();
        *DEAD_SAMPLES.lock().unwrap() = false;
        Self {
            priority_queue: SamplePriorityQueue::new(size),
            list: SampleList::new(size, size / 4 + 1),
            total_allocated: 0,
            threshold: 0,
            size,
            refs: Vec::with_capacity(size),
        }
    }

    pub fn is_created() -> bool {
        sampler_lock().is_some()
    }

    pub fn create(size: usize) -> bool {
        let mut guard = sampler_lock();
        if guard.is_some() {
            return true;
        }
        *guard = Some(Box::new(ObjectSampler::new(size)));
        true
    }

    pub fn destroy() {
        *sampler_lock() = None;
        *DEAD_SAMPLES.lock().unwrap() = false;
        *UNRESOLVED.lock().unwrap() = false;
    }

    pub fn with_sampler<R>(f: impl FnOnce(&mut ObjectSampler) -> R) -> Option<R> {
        let mut guard = sampler_lock();
        guard.as_mut().map(|s| f(s.as_mut()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sample(
        loader: &ClassLoader<'_>,
        object: SampledRef,
        allocated: usize,
        thread_id: i64,
        virtual_thread: bool,
        stack_trace_id: i64,
        stack_trace_hash: i64,
    ) {
        if thread_id == 0 || !Self::is_created() {
            return;
        }
        if *DEAD_SAMPLES.lock().unwrap() {
            *DEAD_SAMPLES.lock().unwrap() = false;
            Self::with_sampler(|s| s.scavenge(loader));
        }
        Self::with_sampler(|s| {
            s.add(
                loader,
                object,
                allocated,
                thread_id,
                virtual_thread,
                stack_trace_id,
                stack_trace_hash,
            );
        });
    }

    fn add(
        &mut self,
        loader: &ClassLoader<'_>,
        object: SampledRef,
        allocated: usize,
        thread_id: i64,
        virtual_thread: bool,
        stack_trace_id: i64,
        stack_trace_hash: i64,
    ) {
        self.total_allocated = self.total_allocated.saturating_add(allocated);
        let span = self
            .total_allocated
            .saturating_sub(self.priority_queue.total());

        let mut sample_index;
        let mut sample_span = span;
        if self.priority_queue.len() == self.size {
            let Some(peek) = self.priority_queue.peek() else {
                return;
            };
            if self
                .list
                .sample(peek)
                .is_some_and(|s| s.span > span)
            {
                return;
            }
            let Some((popped, popped_span)) = self.priority_queue.pop() else {
                return;
            };

            let previous = self.youngest_after_pop();
            sample_index = match self.list.reuse(popped) {
                Some(index) => index,
                None => return,
            };
            if let Some(prev) = previous {
                self.push_span(prev, popped_span);
                sample_span = span;
            } else {
                sample_span = span.saturating_add(popped_span);
            }
        } else {
            let Some(index) = self.list.get() else {
                return;
            };
            sample_index = index;
            if self.refs.len() <= index {
                self.refs.resize(index + 1, None);
            }
        }

        *UNRESOLVED.lock().unwrap() = true;
        if let Some(sample) = self.list.sample_mut(sample_index) {
            sample.reset();
            sample.span = sample_span;
            sample.thread_id = thread_id;
            sample.virtual_thread = virtual_thread;
            sample.stack_trace_id = stack_trace_id;
            sample.stack_trace_hash = stack_trace_hash;
            sample.allocated = allocated;
            sample.allocation_nanos = now_ticks();
            sample.heap_used_at_last_gc = heap_used(loader);
        }
        if self.refs.len() <= sample_index {
            self.refs.resize(sample_index + 1, None);
        }
        if let Some(sample) = self.list.sample_mut(sample_index) {
            self.refs[sample_index] = Some(object);
            sample.object = Some(object.handle);
        }
        self.priority_queue.push(sample_index, sample_span);
    }

    fn youngest_after_pop(&self) -> Option<usize> {
        self.priority_queue.peek()
    }

    pub fn scavenge(&mut self, loader: &ClassLoader<'_>) {
        let dead: Vec<usize> = self
            .list
            .indices()
            .filter(|&index| {
                let alive = self.refs.get(index).and_then(|r| *r).is_some_and(|r| {
                    is_alive(loader, r)
                });
                !alive
            })
            .collect();
        for index in dead {
            self.remove_dead(loader, index);
        }
        *LAST_SWEEP.lock().unwrap() = now_ticks();
        *DEAD_SAMPLES.lock().unwrap() = false;
    }

    fn remove_dead(&mut self, loader: &ClassLoader<'_>, index: usize) {
        let span = self.list.sample(index).map(|s| s.span).unwrap_or(0);
        if let Some(previous) = self.youngest_after_remove(index) {
            self.push_span(previous, span);
        }
        self.priority_queue.remove(index);
        self.list.release(index);
        if let Some(slot) = self.refs.get_mut(index) {
            *slot = None;
        }
        let _ = loader;
    }

    fn youngest_after_remove(&self, _removed: usize) -> Option<usize> {
        self.priority_queue.peek()
    }

    fn push_span(&mut self, index: usize, span: usize) {
        self.priority_queue.remove(index);
        if let Some(sample) = self.list.sample_mut(index) {
            sample.add_span(span);
            let new_span = sample.span;
            self.priority_queue.push(index, new_span);
        }
    }

    pub fn for_each_sample(&self, mut f: impl FnMut(usize, &ObjectSample)) {
        for index in self.list.indices() {
            if let Some(sample) = self.list.sample(index) {
                f(index, sample);
            }
        }
    }

    pub fn sample_count(&self) -> usize {
        self.priority_queue.len()
    }

    pub fn item_count(&self) -> usize {
        self.list.len()
    }

    pub fn set_last_resolved(&mut self, index: usize) {
        self.list.set_last_resolved(Some(index));
        *UNRESOLVED.lock().unwrap() = false;
    }

    pub fn has_unresolved_entry() -> bool {
        *UNRESOLVED.lock().unwrap()
    }

    pub fn last_sweep() -> i64 {
        *LAST_SWEEP.lock().unwrap()
    }

    pub fn mark_dead_samples() {
        *DEAD_SAMPLES.lock().unwrap() = true;
    }
}

fn is_alive(loader: &ClassLoader<'_>, reference: SampledRef) -> bool {
    match reference.kind {
        SampledKind::Object => loader
            .objects_live
            .borrow()
            .get(reference.handle)
            .copied()
            .unwrap_or(false),
        SampledKind::ObjectArray => loader
            .obj_arrays_live
            .borrow()
            .get(reference.handle)
            .copied()
            .unwrap_or(false),
    }
}

fn heap_used(_loader: &ClassLoader<'_>) -> usize {
    0
}

pub fn on_gc() {
    ObjectSampler::mark_dead_samples();
}
