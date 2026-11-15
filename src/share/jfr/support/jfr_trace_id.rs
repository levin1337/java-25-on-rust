/*
 * OpenJDK 25 HotSpot jfr/support/jfrTraceId.hpp + .cpp port
 *
 * Trace ids used to correlate events across threads/epochs.
 */

use crate::share::jfr::utilities::jfr_types::TraceId;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JfrTraceId(pub TraceId);

impl JfrTraceId {
    pub fn new() -> Self {
        JfrTraceId(NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed) as TraceId)
    }

    pub fn as_u64(&self) -> u64 {
        self.0 as u64
    }
}

impl Default for JfrTraceId {
    fn default() -> Self {
        Self::new()
    }
}

// Epoch-scoped trace id for events written to a specific recording epoch.
pub fn trace_id_for_epoch(epoch: u32, thread: TraceId) -> TraceId {
    ((epoch as u64) << 32) | (thread as u64)
}
