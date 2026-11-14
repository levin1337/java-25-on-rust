/*
 * Port of OpenJDK 25 HotSpot StackWalk
 * (share/prims/stackwalk.hpp / stackwalk.cpp)
 *
 * Implements JEP 259 Stack-Walking API runtime support (java.lang.StackWalker):
 * StackWalker options, stack frame batch fetching, and caller-sensitive method inspection.
 */

use crate::share::oops::symbol::Symbol;

pub const JVM_STACKWALK_FILL_CLASS_REFS_ONLY: i32 = 0x2;
pub const JVM_STACKWALK_GET_CALLER_CLASS: i32 = 0x4;
pub const JVM_STACKWALK_SHOW_HIDDEN_FRAMES: i32 = 0x20;
pub const JVM_STACKWALK_FILL_LIVE_STACK_FRAMES: i32 = 0x100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackWalkOptions {
    pub retain_class_reference: bool,
    pub show_reflect_frames: bool,
    pub show_hidden_frames: bool,
}

impl Default for StackWalkOptions {
    fn default() -> Self {
        Self {
            retain_class_reference: false,
            show_reflect_frames: false,
            show_hidden_frames: false,
        }
    }
}

impl StackWalkOptions {
    pub fn from_flags(flags: i32) -> Self {
        Self {
            retain_class_reference: (flags & JVM_STACKWALK_FILL_CLASS_REFS_ONLY) != 0,
            show_reflect_frames: false,
            show_hidden_frames: (flags & JVM_STACKWALK_SHOW_HIDDEN_FRAMES) != 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrameInfo {
    pub class_name: Symbol,
    pub method_name: Symbol,
    pub signature: Symbol,
    pub bci: i32,
    pub line_number: i32,
    pub is_native: bool,
}

impl StackFrameInfo {
    pub fn new(
        class_name: Symbol,
        method_name: Symbol,
        signature: Symbol,
        bci: i32,
        line_number: i32,
        is_native: bool,
    ) -> Self {
        Self {
            class_name,
            method_name,
            signature,
            bci,
            line_number,
            is_native,
        }
    }

    #[inline]
    pub fn is_reflection_frame(&self) -> bool {
        self.class_name
            .as_str()
            .starts_with("jdk/internal/reflect/")
            || self.class_name.as_str().starts_with("java/lang/reflect/")
    }

    #[inline]
    pub fn is_hidden_frame(&self) -> bool {
        self.class_name
            .as_str()
            .starts_with("java/lang/invoke/LambdaForm")
    }
}

pub struct StackWalk;

impl StackWalk {
    /// Filters and collects stack frames according to the requested StackWalker options
    pub fn filter_frames(
        frames: &[StackFrameInfo],
        options: &StackWalkOptions,
        skip_count: usize,
        max_batch_size: usize,
    ) -> Vec<StackFrameInfo> {
        let mut result = Vec::new();
        let mut skipped = 0;

        for frame in frames {
            if !options.show_reflect_frames && frame.is_reflection_frame() {
                continue;
            }
            if !options.show_hidden_frames && frame.is_hidden_frame() {
                continue;
            }

            if skipped < skip_count {
                skipped += 1;
                continue;
            }

            result.push(frame.clone());
            if result.len() >= max_batch_size {
                break;
            }
        }

        result
    }

    /// Finds the immediate caller class, skipping reflection and StackWalker infrastructure
    pub fn get_caller_class(frames: &[StackFrameInfo]) -> Option<Symbol> {
        let mut passed_walker = false;
        for frame in frames {
            let cls = frame.class_name.as_str();
            if cls.starts_with("java/lang/StackWalker") {
                passed_walker = true;
                continue;
            }
            if passed_walker {
                if frame.is_reflection_frame() || frame.is_hidden_frame() {
                    continue;
                }
                return Some(frame.class_name.clone());
            }
        }
        frames.get(1).map(|f| f.class_name.clone())
    }
}
