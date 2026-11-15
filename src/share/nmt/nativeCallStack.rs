/*
 * Port of OpenJDK 25 HotSpot NativeCallStack (share/utilities/nativeCallStack.*)
 *
 * Captures up to NMT_TrackingStackDepth program counter frames for an
 * allocation site. In HotSpot frames are obtained via os::get_native_stack;
 * here they are collected from std::backtrace, which gives frame IPs on
 * all supported platforms. Stacks can also be constructed explicitly from
 * PC arrays (used by tests and by callers that already resolved frames).
 */

use crate::share::nmt::nmtCommon::NMT_TRACKING_STACK_DEPTH;

/// A captured native call stack: up to NMT_TRACKING_STACK_DEPTH PCs.
/// Zeroed tail means "no more frames" (C++ nullptr terminator).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NativeCallStack {
    pcs: [usize; NMT_TRACKING_STACK_DEPTH],
}

impl NativeCallStack {
    /// Empty stack (C++ default ctor).
    pub fn empty() -> Self {
        Self::default()
    }

    /// C++ empty_stack() singleton equivalent.
    pub fn empty_stack() -> &'static Self {
        static EMPTY: NativeCallStack = NativeCallStack { pcs: [0; NMT_TRACKING_STACK_DEPTH] };
        &EMPTY
    }

    /// Build from an explicit PC array (C++ NativeCallStack(address* pc,
    /// int frameCount)).
    pub fn from_frames(frames: &[usize]) -> Self {
        let mut s = Self::empty();
        let n = frames.len().min(NMT_TRACKING_STACK_DEPTH);
        s.pcs[..n].copy_from_slice(&frames[..n]);
        s
    }

    /// Capture the current call stack, skipping `to_skip` frames
    /// (C++ NativeCallStack(int toSkip)).
    pub fn capture(to_skip: usize) -> Self {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let mut pcs = [0usize; NMT_TRACKING_STACK_DEPTH];
        let mut idx = 0;
        for (i, frame) in backtrace.frames().iter().enumerate() {
            if i < to_skip {
                continue;
            }
            if idx >= NMT_TRACKING_STACK_DEPTH {
                break;
            }
            pcs[idx] = frame.ip() as usize;
            idx += 1;
        }
        Self { pcs }
    }

    /// if it is an empty stack
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pcs[0] == 0
    }

    /// number of stack frames captured
    pub fn frames(&self) -> usize {
        self.pcs.iter().position(|&pc| pc == 0).unwrap_or(NMT_TRACKING_STACK_DEPTH)
    }

    /// memcmp over the whole frame array (C++ compare).
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        self.pcs.cmp(&other.pcs)
    }

    #[inline]
    pub fn equals(&self, other: &Self) -> bool {
        self.pcs == other.pcs
    }

    #[inline]
    pub fn get_frame(&self, index: usize) -> usize {
        debug_assert!(index < NMT_TRACKING_STACK_DEPTH, "Index out of bound");
        self.pcs[index]
    }

    /// Helper; calculates a hash value over the stack frames.
    /// Same algorithm as C++: sum of PCs truncated to u32.
    pub fn calculate_hash(&self) -> u32 {
        let mut hash: usize = 0;
        for &pc in &self.pcs {
            hash = hash.wrapping_add(pc);
        }
        hash as u32
    }

    /// Render like C++ print_on: one "[0x...]" frame per line.
    pub fn print_on(&self) -> String {
        let mut out = String::new();
        for &pc in &self.pcs {
            if pc == 0 {
                break;
            }
            out.push_str(&format!("[0x{:x}]\n", pc));
        }
        out
    }
}

impl std::fmt::Display for NativeCallStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.print_on())
    }
}

/// CURRENT_PC macro equivalent: capture a call stack when NMT detail
/// tracking is on, otherwise return an empty (fake) stack.
#[inline]
pub fn current_pc(detail: bool) -> NativeCallStack {
    if detail {
        NativeCallStack::capture(1)
    } else {
        NativeCallStack::empty()
    }
}

/// CALLER_PC macro equivalent.
#[inline]
pub fn caller_pc(detail: bool) -> NativeCallStack {
    if detail {
        NativeCallStack::capture(2)
    } else {
        NativeCallStack::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_hash() {
        let s = NativeCallStack::from_frames(&[0x1000, 0x2000, 0x3000]);
        assert_eq!(s.frames(), 3);
        assert!(!s.is_empty());
        assert_eq!(s.calculate_hash(), (0x1000 + 0x2000 + 0x3000) as u32);
        let e = NativeCallStack::empty();
        assert!(e.is_empty());
        assert_eq!(e.frames(), 0);
        assert_ne!(s.compare(&e), std::cmp::Ordering::Equal);
    }

    #[test]
    fn capture_works() {
        let s = NativeCallStack::capture(0);
        assert!(s.frames() > 0);
    }
}
