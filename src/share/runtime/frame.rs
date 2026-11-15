/*
 * Port of OpenJDK 25 HotSpot Frame (share/runtime/frame.hpp / frame.cpp)
 *
 * HotSpot's `frame` is a raw-stack cursor; this port models the same
 * API over the interpreter's registered frame list
 * (`bytecodeInterpreter::for_each_active_frame`) plus descriptions of
 * compiled/JIT activations supplied by the caller.
 *
 * Provided surface mirrors frame.hpp:
 *  - `FrameType` classification (interpreter / compiled / entry / stub /
 *    deoptimization-entry / adapter / continuation),
 *  - `Frame` predicates: `is_interpreter_frame`, `is_compiled_frame`,
 *    `is_blob_frame`, `is_entry_frame`, `is_stub_frame`,
 *    `is_deoptimized_frame`, `is_first_frame`, `can_be_deoptimized`,
 *    `should_be_deoptimized`,
 *  - `sender()` chain walking innermost -> outermost,
 *  - `FrameIter` used for stack-trace collection (StackWalker).
 */

use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::method::Code;
use crate::share::prims::stackwalk::StackFrameInfo;

// ---------------------------------------------------------------------------
// Existing interpreter-local frame model (kept for compatibility).
// ---------------------------------------------------------------------------

pub struct StackFrame<'a> {
    pub code: Code<'a>,
    pub bci: usize,
    pub locals: Vec<Value<'a>>,
    pub stack: Vec<Value<'a>>,
}

impl<'a> StackFrame<'a> {
    pub fn new(code: Code<'a>, locals: Vec<Value<'a>>) -> Self {
        Self {
            stack: Vec::with_capacity(code.maxStack),
            code,
            bci: 0,
            locals,
        }
    }

    pub fn bci(&self) -> usize {
        self.bci
    }

    pub fn set_bci(&mut self, bci: usize) {
        self.bci = bci;
    }
}

// ---------------------------------------------------------------------------
// FrameType — mirrors the blob-kind classification of frame.cpp.
// ---------------------------------------------------------------------------

/// Classification of a frame on the logical stack.  Values correspond to
/// the `CodeBlobKind` families frame.cpp dispatches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// Bytecode-interpreted Java method activation.
    InterpreterFrame,
    /// JIT-compiled Java method activation (nmethod).
    CompiledFrame,
    /// Native method / JNI boundary frame (Java called from C).
    EntryFrame,
    /// Runtime stub / safepoint blob frame.
    StubFrame,
    /// Deoptimization entry blob frame.
    DeoptimizationEntryFrame,
    /// Adapter frame (i2c / c2i / exception / continuation adapters).
    AdapterFrame,
    /// Virtual-thread continuation entry frame (Project Loom).
    ContinuationFrame,
    /// Upcall stub frame (JVM -> Java -> JVM callback boundary).
    UpcallStubFrame,
    /// Unknown / not yet classified.
    UnknownFrame,
}

impl FrameType {
    /// Whether frames of this type carry an oop map and Java state that
    /// deoptimization can consume (`is_deopt_frame` in frame.cpp terms).
    pub fn is_deoptimizable(self) -> bool {
        matches!(self, Self::CompiledFrame)
    }

    /// Whether this is a "blob" frame: any VM-generated code frame that
    /// is not interpreted Java code (`frame::is_interpreted_frame` == false
    /// && `cb() != nullptr` family).
    pub fn is_blob(self) -> bool {
        !matches!(self, Self::InterpreterFrame | Self::UnknownFrame)
    }
}

// ---------------------------------------------------------------------------
// Frame — read-only description of one activation.
// ---------------------------------------------------------------------------

/// One stack frame description.  Interpreter frames are populated from
/// the registered `bytecodeInterpreter::Frame` list; compiled and blob
/// frames can be pushed explicitly by the JIT/runtime layers.
#[derive(Debug, Clone)]
pub struct Frame {
    kind: FrameType,
    /// Declaring class internal name (`a/b/C`).
    pub holder: Vec<u8>,
    /// Method name.
    pub name: Vec<u8>,
    /// Method descriptor.
    pub descriptor: Vec<u8>,
    /// Bytecode index (interpreted) or pc offset (compiled/blob).
    pub bci: usize,
    /// Source line number, -1 when unknown.
    pub line_number: i32,
    /// Owning Java thread id (HotSpot `thread()->thread_identifier()`).
    pub thread_id: u64,
    /// Whether this frame is executing a native method.
    pub is_native: bool,
    /// Frame index in the owning `FrameIter` (0 = innermost).
    index: usize,
    /// Marked for deoptimization (`should_be_deoptimized`).
    marked_for_deopt: bool,
    /// Extra stack-trace metadata copied from `StackFrameInfo`.
    pub info: Option<StackFrameInfo>,
}

impl Frame {
    /// Builds an interpreter-frame description from the live interpreter
    /// `Frame` object.  `index` is the position in the iteration order
    /// (0 = innermost), matching HotSpot's top-of-stack-first walk.
    pub(crate) fn from_interpreter(
        index: usize,
        thread_id: u64,
        info: &StackFrameInfo,
    ) -> Self {
        Self {
            kind: FrameType::InterpreterFrame,
            holder: info.class_name.as_bytes().to_vec(),
            name: info.method_name.as_bytes().to_vec(),
            descriptor: info.signature.as_bytes().to_vec(),
            bci: info.bci.max(0) as usize,
            line_number: info.line_number,
            thread_id,
            is_native: info.is_native,
            index,
            marked_for_deopt: false,
            info: Some(info.clone()),
        }
    }

    /// Builds a compiled/blob frame description from nmethod metadata.
    /// Used when the JIT layer reports an active compiled activation to
    /// the stack walker.
    pub fn compiled(
        index: usize,
        holder: &[u8],
        name: &[u8],
        descriptor: &[u8],
        bci: usize,
        thread_id: u64,
    ) -> Self {
        Self {
            kind: FrameType::CompiledFrame,
            holder: holder.to_vec(),
            name: name.to_vec(),
            descriptor: descriptor.to_vec(),
            bci,
            line_number: -1,
            thread_id,
            is_native: false,
            index,
            marked_for_deopt: false,
            info: None,
        }
    }

    /// Builds an entry/stub frame description (runtime boundary).
    pub fn blob(index: usize, kind: FrameType, thread_id: u64) -> Self {
        debug_assert!(kind.is_blob());
        Self {
            kind,
            holder: Vec::new(),
            name: Vec::new(),
            descriptor: Vec::new(),
            bci: 0,
            line_number: -1,
            thread_id,
            is_native: false,
            index,
            marked_for_deopt: false,
            info: None,
        }
    }

    // -- type testers (frame.hpp) -------------------------------------------

    /// `frame::frame_type`.
    #[inline]
    pub fn frame_type(&self) -> FrameType {
        self.kind
    }

    /// `frame::is_interpreted_frame`.
    #[inline]
    pub fn is_interpreter_frame(&self) -> bool {
        self.kind == FrameType::InterpreterFrame
    }

    /// `frame::is_java_frame`: interpreted or compiled Java activation.
    #[inline]
    pub fn is_java_frame(&self) -> bool {
        matches!(
            self.kind,
            FrameType::InterpreterFrame | FrameType::CompiledFrame
        )
    }

    /// `frame::is_compiled_frame`.
    #[inline]
    pub fn is_compiled_frame(&self) -> bool {
        self.kind == FrameType::CompiledFrame
    }

    /// `frame::is_entry_frame` — Java frame called from C/JNI.
    #[inline]
    pub fn is_entry_frame(&self) -> bool {
        self.kind == FrameType::EntryFrame
    }

    /// `frame::is_stub_frame`.
    #[inline]
    pub fn is_stub_frame(&self) -> bool {
        self.kind == FrameType::StubFrame
    }

    /// `frame::is_blob_frame`: any VM-generated code frame (everything
    /// that is not interpreted Java code and has a CodeBlob).
    #[inline]
    pub fn is_blob_frame(&self) -> bool {
        self.kind.is_blob()
    }

    /// `frame::is_deoptimized_frame` analogue: frames that were produced
    /// by materializing a deoptimization (marked frames in this model).
    #[inline]
    pub fn is_deoptimized_frame(&self) -> bool {
        self.marked_for_deopt
    }

    /// `frame::is_compiled_caller`.
    #[inline]
    pub fn is_compiled_caller(&self) -> bool {
        matches!(self.kind, FrameType::CompiledFrame | FrameType::UpcallStubFrame)
    }

    /// `frame::is_first_frame`: no sender (outermost activation).
    pub fn is_first_frame(&self, total: usize) -> bool {
        self.index + 1 >= total
    }

    /// `frame::should_be_deoptimized`.
    #[inline]
    pub fn should_be_deoptimized(&self) -> bool {
        self.marked_for_deopt
    }

    /// `frame::mark_for_deoptimization` (lazy deopt marking).
    pub fn mark_for_deoptimization(&mut self) {
        self.marked_for_deopt = true;
    }

    /// `frame::can_be_deoptimized`: only compiled frames carrying
    /// debug info can be lazily deoptimized.
    #[inline]
    pub fn can_be_deoptimized(&self) -> bool {
        self.kind.is_deoptimizable()
    }

    /// `frame::bci`.
    #[inline]
    pub fn bci(&self) -> usize {
        self.bci
    }

    /// `frame::pc` analogue: the instruction pointer as a frame offset.
    #[inline]
    pub fn pc(&self) -> usize {
        self.bci
    }

    /// Resolves the sender frame within the owning iteration.  Returns
    /// `None` for the outermost frame (`is_first_frame`).
    pub fn sender<'a>(&self, frames: &'a [Frame]) -> Option<&'a Frame> {
        frames.get(self.index + 1)
    }

    /// `frame::java_sender` analogue: first sender that is a Java frame.
    pub fn java_sender<'a>(&self, frames: &'a [Frame]) -> Option<&'a Frame> {
        frames[self.index + 1..]
            .iter()
            .find(|frame| frame.is_java_frame())
    }

    /// `frame::is_younger` analogue: `self` activated after `other`.
    #[inline]
    pub fn is_younger_than(&self, other: &Frame) -> bool {
        self.index < other.index
    }

    /// Human-readable site string (`site()` analogue used by the
    /// watchdog diagnostics): `holder.name@descriptor:bci`.
    pub fn site(&self) -> String {
        format!(
            "{} {}.{}:{}",
            self.kind_name(),
            String::from_utf8_lossy(&self.holder),
            String::from_utf8_lossy(&self.name),
            self.bci
        )
    }

    fn kind_name(&self) -> &'static str {
        match self.kind {
            FrameType::InterpreterFrame => "interpreter",
            FrameType::CompiledFrame => "compiled",
            FrameType::EntryFrame => "entry",
            FrameType::StubFrame => "stub",
            FrameType::DeoptimizationEntryFrame => "deopt",
            FrameType::AdapterFrame => "adapter",
            FrameType::ContinuationFrame => "continuation",
            FrameType::UpcallStubFrame => "upcall",
            FrameType::UnknownFrame => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// FrameIter — stack-walk iterator (innermost -> outermost).
// ---------------------------------------------------------------------------

/// Snapshot of the active frame list in HotSpot's walk order
/// (innermost first).  `StackWalker`/`Throwable.fillInStackTrace` use
/// this; entries belonging to other threads are filtered out by
/// `current_thread`.
pub struct FrameIter {
    frames: Vec<Frame>,
    position: usize,
}

impl FrameIter {
    /// Collects every active interpreter frame plus any caller-pushed
    /// compiled/blob frames.  Iteration order is innermost -> outermost,
    /// matching `frame::sender` walks.
    pub fn new(current_thread: u64) -> Self {
        // ACTIVE_FRAMES hands out short-lived `&Frame` borrows, so the
        // owned `StackFrameInfo` snapshot is taken inside the callback.
        let mut collected = Vec::new();
        crate::share::interpreter::bytecodeInterpreter::for_each_active_frame(
            &mut |frame| {
                if frame.threadId() == current_thread {
                    collected.push((frame.threadId(), frame.stackWalkInfo()));
                }
            },
        );
        // ACTIVE_FRAMES is stored outermost -> innermost; reverse for the
        // HotSpot top-down walk order.
        collected.reverse();
        let frames = collected
            .iter()
            .enumerate()
            .map(|(index, (thread_id, info))| {
                Frame::from_interpreter(index, *thread_id, info)
            })
            .collect();
        Self { frames, position: 0 }
    }

    /// Builds an iterator over an explicit frame list (used by tests and
    /// by layers that inject compiled activations).
    pub fn from_frames(frames: Vec<Frame>) -> Self {
        Self {
            frames,
            position: 0,
        }
    }

    /// Appends a frame description (compiled activation reported by the
    /// JIT layer) at the innermost position.
    pub fn push_front(&mut self, frame: Frame) {
        self.frames.insert(0, frame);
        for (index, entry) in self.frames.iter_mut().enumerate() {
            entry.index = index;
        }
    }

    /// The full snapshot in walk order.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// Number of frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the walk is empty.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Returns the frame at walk position `index` without consuming.
    pub fn peek(&self) -> Option<&Frame> {
        self.frames.get(self.position)
    }

    /// Collects all remaining frames.
    pub fn collect_rest(&self) -> Vec<Frame> {
        self.frames[self.position..].to_vec()
    }
}

impl Iterator for FrameIter {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = self.frames.get(self.position)?.clone();
        self.position += 1;
        Some(frame)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.frames.len() - self.position;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for FrameIter {}

/// Convenience: collects the current thread's stack as `StackFrameInfo`
/// records for `StackWalk` — kept here so runtime callers do not need to
/// reach into `prims` directly.
pub fn stack_frame_infos(current_thread: u64) -> Vec<StackFrameInfo> {
    FrameIter::new(current_thread)
        .frames()
        .iter()
        .filter_map(|frame| frame.info.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc_frame(index: usize, kind: FrameType) -> Frame {
        let mut frame = Frame::blob(index, kind, 7);
        frame.holder = b"a/B".to_vec();
        frame.name = b"m".to_vec();
        frame.descriptor = b"()V".to_vec();
        frame
    }

    #[test]
    fn frame_type_predicates() {
        let mut interp = desc_frame(0, FrameType::InterpreterFrame);
        assert!(interp.is_interpreter_frame());
        assert!(interp.is_java_frame());
        assert!(!interp.is_compiled_frame());
        assert!(!interp.is_blob_frame());

        let compiled = desc_frame(1, FrameType::CompiledFrame);
        assert!(compiled.is_compiled_frame());
        assert!(compiled.is_java_frame());
        assert!(compiled.is_blob_frame());
        assert!(compiled.can_be_deoptimized());
        assert!(compiled.is_compiled_caller());

        let stub = desc_frame(2, FrameType::StubFrame);
        assert!(stub.is_blob_frame());
        assert!(stub.is_stub_frame());
        assert!(!stub.is_java_frame());
        assert!(!stub.can_be_deoptimized());

        interp.mark_for_deoptimization();
        assert!(interp.should_be_deoptimized());
        assert!(interp.is_deoptimized_frame());
    }

    #[test]
    fn sender_chain() {
        let frames = vec![
            desc_frame(0, FrameType::InterpreterFrame),
            desc_frame(1, FrameType::StubFrame),
            desc_frame(2, FrameType::CompiledFrame),
        ];
        let top = &frames[0];
        assert_eq!(top.sender(&frames).unwrap().frame_type(), FrameType::StubFrame);
        // java_sender skips the stub frame.
        let java = top.java_sender(&frames).unwrap();
        assert_eq!(java.frame_type(), FrameType::CompiledFrame);
        assert!(frames[2].sender(&frames).is_none());
        assert!(frames[2].is_first_frame(frames.len()));
        assert!(top.is_younger_than(&frames[2]));
    }

    #[test]
    fn iter_order_and_exact_size() {
        let frames = vec![
            desc_frame(0, FrameType::CompiledFrame),
            desc_frame(1, FrameType::InterpreterFrame),
        ];
        let mut iter = FrameIter::from_frames(frames);
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.next().unwrap().index, 0);
        assert_eq!(iter.next().unwrap().index, 1);
        assert!(iter.next().is_none());
    }
}
