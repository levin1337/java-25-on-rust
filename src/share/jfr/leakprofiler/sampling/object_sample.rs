

use std::time::Instant;

pub type ObjectHandle = usize;

#[derive(Clone, Debug)]
pub struct ObjectSample {
    pub object: Option<ObjectHandle>,
    pub allocation_time: Instant,
    pub allocation_nanos: i64,
    pub thread_id: i64,
    pub stack_trace_id: i64,
    pub stack_trace_hash: i64,
    pub span: usize,
    pub allocated: usize,
    pub heap_used_at_last_gc: usize,
    pub index: i32,
    pub virtual_thread: bool,
    pub thread_exited: bool,
    pub stacktrace_ref: Option<i64>,
    pub thread_ref: Option<i64>,
    pub type_set_ref: Option<i64>,
}

impl Default for ObjectSample {
    fn default() -> Self {
        Self {
            object: None,
            allocation_time: Instant::now(),
            allocation_nanos: 0,
            thread_id: 0,
            stack_trace_id: 0,
            stack_trace_hash: 0,
            span: 0,
            allocated: 0,
            heap_used_at_last_gc: 0,
            index: 0,
            virtual_thread: false,
            thread_exited: false,
            stacktrace_ref: None,
            thread_ref: None,
            type_set_ref: None,
        }
    }
}

impl ObjectSample {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn is_alive_and_older_than(&self, timestamp_nanos: i64) -> bool {
        self.object.is_some() && self.allocation_nanos <= timestamp_nanos
    }

    pub fn has_stack_trace_id(&self) -> bool {
        self.stack_trace_id != 0
    }

    pub fn has_stacktrace(&self) -> bool {
        self.stacktrace_ref.is_some() || self.has_stack_trace_id()
    }

    pub fn has_thread(&self) -> bool {
        self.thread_id != 0 || self.thread_ref.is_some()
    }

    pub fn has_type_set(&self) -> bool {
        self.type_set_ref.is_some()
    }

    pub fn add_span(&mut self, span: usize) {
        self.span = self.span.saturating_add(span);
    }
}
