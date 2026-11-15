/*
 * OpenJDK 25 HotSpot jfr/recorder/storage/jfrStorageTypes.hpp port
 *
 * Storage types used by JfrStorageAdapter/JfrCheckpointWriter.
 */

use crate::share::jfr::utilities::jfr_types::TraceId;
use std::sync::atomic::{AtomicUsize, Ordering};

static NATIVE_THREAD_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JfrStorage {
    Global = 0,
    ThreadLocal = 1,
    VirtualThreadLocal = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JfrBufferType {
    Checkpoint = 0,
    ConstantPool = 1,
    Event = 2,
    Blob = 3,
}

// Trace id helpers: native thread id allocation (simplified — uses an
// atomic counter instead of thread-local index mapping).
pub fn allocate_native_thread_id() -> TraceId {
    NATIVE_THREAD_ID.fetch_add(1, Ordering::Relaxed) as TraceId
}

pub fn release_native_thread_id(_id: TraceId) {}
