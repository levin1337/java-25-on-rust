

use crate::share::jfr::leakprofiler::chains::path_to_gc_roots_operation::PathToRoot;
use crate::share::jfr::leakprofiler::sampling::object_sample::ObjectSample;

#[derive(Clone, Debug)]
pub struct ObjectSampleDescription {
    pub allocation_time_ticks: i64,
    pub object_size: usize,
    pub thread_id: i64,
    pub stack_trace_id: i64,
    pub object_class: Option<String>,
    pub path_to_root: Option<PathToRoot>,
    pub last_heartbeat_ms: i64,
}

impl ObjectSampleDescription {
    pub fn from_sample(
        sample: &ObjectSample,
        object_size: usize,
        object_class: Option<String>,
        path_to_root: Option<PathToRoot>,
        last_heartbeat_ms: i64,
    ) -> Self {
        Self {
            allocation_time_ticks: sample.allocation_nanos,
            object_size,
            thread_id: sample.thread_id,
            stack_trace_id: sample.stack_trace_id,
            object_class,
            path_to_root,
            last_heartbeat_ms,
        }
    }
}
