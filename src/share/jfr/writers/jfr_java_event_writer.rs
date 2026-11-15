/*
 * OpenJDK 25 HotSpot jfr/writers/jfrJavaEventWriter.cpp port (buffer side)
 *
 * The C++ version pokes fields of jdk.jfr.internal.event.EventWriter via
 * JNI. In this port the java event writer state is tracked natively per
 * thread: positions into the thread's java JfrBuffer. The JNI field
 * marshalling is handled by the jni layer calling into these functions.
 */

use crate::share::jfr::utilities::jfr_types::TraceId;
use crate::share::jfr::writers::jfr_writer::JfrWriter;

#[derive(Clone, Copy, Debug, Default)]
pub struct JfrJavaEventWriterState {
    pub start_position: i64,
    pub current_position: i64,
    pub max_position: i64,
    pub excluded: bool,
    pub thread_id: i64,
    pub valid: bool,
    pub pin_virtual_thread: bool,
}

impl JfrJavaEventWriterState {
    pub fn new() -> JfrJavaEventWriterState {
        JfrJavaEventWriterState {
            valid: true,
            ..Default::default()
        }
    }
}

// flush(writer, used, requested): equivalent of JfrJavaEventWriter::flush —
// ensures the java buffer can accommodate used+requested bytes and updates
// the writer positions. `buffer_start` is the absolute offset of buffer
// position 0 in the writer's coordinate space.
pub fn flush_state(
    state: &mut JfrJavaEventWriterState,
    buffer_start: i64,
    buffer_end: i64,
    used: i64,
    requested: i64,
    new_buffer: bool,
) -> bool {
    let is_valid = buffer_end - buffer_start >= used + requested;
    let new_current = if is_valid {
        buffer_start + used
    } else {
        buffer_start
    };
    state.start_position = buffer_start;
    state.current_position = new_current;
    state.max_position = if new_buffer { buffer_end } else { state.max_position };
    is_valid
}

// commit(next_position): JfrJavaEventWriter::commit
pub fn commit_state(state: &mut JfrJavaEventWriterState, next_position: i64) {
    state.current_position = next_position;
    state.valid = state.current_position <= state.max_position;
}

pub fn exclude_state(state: &mut JfrJavaEventWriterState) {
    state.excluded = true;
}

pub fn include_state(state: &mut JfrJavaEventWriterState) {
    state.excluded = false;
}

// Per-thread registry of java event writer states.
pub struct JfrJavaEventWriterRegistry {
    states: parking_lot::Mutex<std::collections::HashMap<TraceId, JfrJavaEventWriterState>>,
}

impl JfrJavaEventWriterRegistry {
    pub fn new() -> JfrJavaEventWriterRegistry {
        JfrJavaEventWriterRegistry {
            states: parking_lot::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn state(&self, tid: TraceId) -> JfrJavaEventWriterState {
        self.states
            .lock()
            .get(&tid)
            .copied()
            .unwrap_or_else(JfrJavaEventWriterState::new)
    }

    pub fn set_state(&self, tid: TraceId, state: JfrJavaEventWriterState) {
        self.states.lock().insert(tid, state);
    }

    pub fn exclude(&self, tid: TraceId) {
        let mut map = self.states.lock();
        let mut s = map.get(&tid).copied().unwrap_or_else(JfrJavaEventWriterState::new);
        exclude_state(&mut s);
        map.insert(tid, s);
    }

    pub fn include(&self, tid: TraceId) {
        let mut map = self.states.lock();
        let mut s = map.get(&tid).copied().unwrap_or_else(JfrJavaEventWriterState::new);
        include_state(&mut s);
        map.insert(tid, s);
    }

    pub fn remove(&self, tid: TraceId) {
        self.states.lock().remove(&tid);
    }
}

impl Default for JfrJavaEventWriterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export for convenience: a JfrWriter bound to a java buffer segment.
pub type JavaBufferWriter<'a> = JfrWriterView<'a>;

pub struct JfrWriterView<'a> {
    pub writer: &'a mut JfrWriter,
    pub state: JfrJavaEventWriterState,
}
