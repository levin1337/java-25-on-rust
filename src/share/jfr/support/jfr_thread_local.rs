/*
 * OpenJDK 25 HotSpot jfr/support/jfrThreadLocal.hpp + .cpp port
 *
 * Per-thread JFR state container. Rust: thread_local! storage keyed by
 * thread id; provides access to the thread's JfrThreadLocal event
 * buffer state.
 */

use crate::share::jfr::recorder::storage::jfr_buffer::JfrBuffer;
use crate::share::jfr::utilities::jfr_types::TraceId;
use std::cell::RefCell;

thread_local! {
    static THREAD_BUFFER: RefCell<Option<JfrBuffer>> = const { RefCell::new(None) };
    static THREAD_ID: RefCell<TraceId> = const { RefCell::new(0) };
}

pub struct JfrThreadLocal;

impl JfrThreadLocal {
    pub fn thread_id() -> TraceId {
        THREAD_ID.with(|id| *id.borrow())
    }

    pub fn set_thread_id(id: TraceId) {
        THREAD_ID.with(|tid| *tid.borrow_mut() = id);
    }

    pub fn buffer() -> Option<JfrBuffer> {
        THREAD_BUFFER.with(|b| b.borrow_mut().take())
    }

    pub fn set_buffer(buf: JfrBuffer) {
        THREAD_BUFFER.with(|b| *b.borrow_mut() = Some(buf));
    }

    pub fn has_buffer() -> bool {
        THREAD_BUFFER.with(|b| b.borrow().is_some())
    }
}
