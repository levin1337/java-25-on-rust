/*
 * OpenJDK 25 HotSpot jfr/support/jfrStackTrace.hpp + .cpp port
 *
 * Captures a Java stack trace for an event. In HotSpot this walks the
 * VM thread's frames at a safepoint; in this Rust port the JVM-side
 * frame walking is provided by the caller, and this module defines the
 * trace storage format used by writers (stacktrace constant pool).
 */

use crate::share::jfr::utilities::jfr_types::TraceId;

// A captured stack trace: sequence of frame method/class ids.
#[derive(Clone, Debug, Default)]
pub struct JfrStackTrace {
    pub frames: Vec<u64>, // method ids (TypeMethod) or encoded frame info
    pub thread: TraceId,
    pub truncated: bool,
}

impl JfrStackTrace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    // JfrStackTrace::serialize — writes the trace into a checkpoint
    // writer as a constant-pool trace element (simplified encoding:
    // u4 frame count + frame ids).
    pub fn write_to<W: crate::share::jfr::writers::jfr_writer::JfrWriterLike>(&self, w: &mut W) {
        w.write_u4(self.frames.len() as u32);
        for f in &self.frames {
            w.write_u8(*f);
        }
    }
}

// Minimal writer abstraction so JfrStackTrace doesn't depend on the
// concrete checkpoint writer type.
pub trait JfrWriterLike {
    fn write_u4(&mut self, v: u32);
    fn write_u8(&mut self, v: u64);
}
