/*
 * Port of OpenJDK 25 HotSpot Continuation (share/runtime/continuation.hpp / continuation.cpp)
 *
 * Implements Project Loom virtual thread continuation suspension (freeze)
 * and resumption (thaw), with pinning detection (critical section, native, monitor)
 * and stack chunk management.
 */

use super::continuationEntry::ContinuationEntry;
use super::javaThread::JavaThread;
use crate::share::oops::stackChunkOop::StackChunkOop;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeResult {
    FreezeOk = 0,
    FreezeOkBottom = 1,
    FreezePinnedCriticalSection = 2,
    FreezePinnedNative = 3,
    FreezePinnedMonitor = 4,
    FreezeException = 5,
    FreezeNotMounted = 6,
    FreezeUnsupported = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThawKind {
    ThawTop = 0,
    ThawReturnBarrier = 1,
    ThawException = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationScope {
    name: String,
}

impl ContinuationScope {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct Continuation {
    scope: ContinuationScope,
    chunk: Option<StackChunkOop>,
    entry: Option<ContinuationEntry>,
    pin_count: u32,
    mounted: bool,
}

impl Continuation {
    pub const DEFAULT_CHUNK_WORDS: usize = 4096;
    pub const NATIVE_FRAME_MARKER: u64 = 0xFFFF_FFFF_DEAD_0001;

    pub fn new(scope_name: String) -> Self {
        Self {
            scope: ContinuationScope::new(scope_name),
            chunk: None,
            entry: None,
            pin_count: 0,
            mounted: false,
        }
    }

    #[inline]
    pub fn scope(&self) -> &ContinuationScope {
        &self.scope
    }

    #[inline]
    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.pin_count > 0
    }

    #[inline]
    pub fn pin_count(&self) -> u32 {
        self.pin_count
    }

    #[inline]
    pub fn pin(&mut self) {
        self.pin_count = self.pin_count.saturating_add(1);
    }

    #[inline]
    pub fn unpin(&mut self) {
        self.pin_count = self.pin_count.saturating_sub(1);
    }

    pub fn mount(&mut self, carrier_sp: usize) {
        self.entry = Some(ContinuationEntry::new(carrier_sp));
        self.mounted = true;
    }

    pub fn unmount(&mut self) {
        self.entry = None;
        self.mounted = false;
    }

    #[inline]
    pub fn entry(&self) -> Option<&ContinuationEntry> {
        self.entry.as_ref()
    }

    #[inline]
    pub fn chunk(&self) -> Option<&StackChunkOop> {
        self.chunk.as_ref()
    }

    #[inline]
    pub fn has_chunk(&self) -> bool {
        self.chunk.is_some()
    }

    /// Freeze Java stack frames into a StackChunkOop, suspending the virtual thread continuation.
    /// Performs comprehensive pinning detection:
    /// - Not mounted
    /// - Explicit pinning (critical section)
    /// - Thread holding synchronized monitors (held_monitor_count > 0)
    /// - Thread running in native or native frame present on stack
    /// - Pending exception
    pub fn freeze(&mut self, thread: &mut JavaThread, frames: &[u64]) -> FreezeResult {
        if !self.mounted {
            return FreezeResult::FreezeNotMounted;
        }

        if self.is_pinned() {
            return FreezeResult::FreezePinnedCriticalSection;
        }

        if thread.held_monitor_count() > 0 {
            return FreezeResult::FreezePinnedMonitor;
        }

        if thread.is_in_native() || frames.iter().any(|&f| f == Self::NATIVE_FRAME_MARKER) {
            return FreezeResult::FreezePinnedNative;
        }

        if thread.has_pending_exception() {
            return FreezeResult::FreezeException;
        }

        let mut chunk = StackChunkOop::new(Self::DEFAULT_CHUNK_WORDS);
        chunk.set_flags(StackChunkOop::FLAG_HAS_INTERPRETED_FRAMES);

        if chunk.push_words(frames).is_err() {
            return FreezeResult::FreezeUnsupported;
        }

        self.chunk = Some(chunk);
        self.unmount();
        FreezeResult::FreezeOk
    }

    /// Attempts cooperative preemption of the virtual thread.
    pub fn try_preempt(&mut self, thread: &mut JavaThread, frames: &[u64]) -> FreezeResult {
        let res = self.freeze(thread, frames);
        if res == FreezeResult::FreezeOk {
            if let Some(chunk) = &mut self.chunk {
                chunk.set_flags(chunk.flags() | StackChunkOop::FLAG_PREEMPTED);
            }
        }
        res
    }

    /// Thaw previously frozen stack frames, restoring virtual thread execution state.
    pub fn thaw(&mut self, carrier_sp: usize) -> Result<Vec<u64>, &'static str> {
        self.thaw_with_kind(carrier_sp, ThawKind::ThawTop)
    }

    /// Thaw previously frozen stack frames with specified thaw kind (ThawTop, ThawReturnBarrier, ThawException).
    pub fn thaw_with_kind(
        &mut self,
        carrier_sp: usize,
        _kind: ThawKind,
    ) -> Result<Vec<u64>, &'static str> {
        let mut chunk = self.chunk.take().ok_or("No frozen stack chunk to thaw")?;
        let restored_frames = chunk.drain_words();
        self.mount(carrier_sp);
        Ok(restored_frames)
    }
}
