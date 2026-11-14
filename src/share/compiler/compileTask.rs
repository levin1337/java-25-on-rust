// Compile task ported from HotSpot compileTask.hpp / .cpp

use super::compilerDefinitions::CompLevel;
use crate::share::oops::symbol::Symbol;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompileReason {
    ReasonNone = 0,
    ReasonInvocationCount = 1,
    ReasonBackedgeCount = 2,
    ReasonTiered = 3,
    ReasonReplay = 4,
    ReasonWhitebox = 5,
    ReasonMustBeCompiled = 6,
    ReasonBootstrap = 7,
}

impl CompileReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReasonNone => "no_reason",
            Self::ReasonInvocationCount => "count",
            Self::ReasonBackedgeCount => "backedge_count",
            Self::ReasonTiered => "tiered",
            Self::ReasonReplay => "replay",
            Self::ReasonWhitebox => "whitebox",
            Self::ReasonMustBeCompiled => "must_be_compiled",
            Self::ReasonBootstrap => "bootstrap",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InliningResult {
    Success,
    Failure,
}

#[derive(Clone, Debug)]
pub struct CompileTask {
    pub compile_id: usize,
    pub class_name: Symbol,
    pub method_name: Symbol,
    pub descriptor: Symbol,
    pub comp_level: CompLevel,
    pub is_osr: bool,
    pub osr_bci: usize,
    pub reason: CompileReason,
    pub is_blocking: bool,
    pub num_inlined_bytecodes: usize,
}

impl CompileTask {
    pub fn new(
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
    ) -> Self {
        Self {
            compile_id: 0,
            class_name,
            method_name,
            descriptor,
            comp_level,
            is_osr: false,
            osr_bci: 0,
            reason: CompileReason::ReasonTiered,
            is_blocking: false,
            num_inlined_bytecodes: 0,
        }
    }

    pub fn new_osr(
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
        osr_bci: usize,
    ) -> Self {
        Self {
            compile_id: 0,
            class_name,
            method_name,
            descriptor,
            comp_level,
            is_osr: true,
            osr_bci,
            reason: CompileReason::ReasonBackedgeCount,
            is_blocking: false,
            num_inlined_bytecodes: 0,
        }
    }

    pub fn with_id(mut self, compile_id: usize) -> Self {
        self.compile_id = compile_id;
        self
    }

    pub fn with_reason(mut self, reason: CompileReason) -> Self {
        self.reason = reason;
        self
    }

    pub fn with_blocking(mut self, is_blocking: bool) -> Self {
        self.is_blocking = is_blocking;
        self
    }

    pub fn compile_id(&self) -> usize {
        self.compile_id
    }

    pub fn comp_level(&self) -> CompLevel {
        self.comp_level
    }

    pub fn is_osr(&self) -> bool {
        self.is_osr
    }

    pub fn osr_bci(&self) -> usize {
        self.osr_bci
    }

    pub fn reason(&self) -> CompileReason {
        self.reason
    }
}
