/*
 * Port of OpenJDK 25 HotSpot nmethod (share/code/nmethod.hpp / nmethod.cpp)
 *
 * nmethod represents a JIT compiled Java method in machine code,
 * managing compilation tiers, deoptimization state, on-stack replacement (OSR),
 * and debug/scope mappings.
 */

use super::pcDesc::PcDesc;
use super::scopeDesc::ScopeDesc;
use crate::share::compiler::compilerDefinitions::CompLevel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NMethodState {
    NotInstalled,
    InUse,
    NotEntrant,
    Zombie,
    Unloaded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidationReason {
    C1CodePatch,
    C1Deoptimize,
    MarkedForDeoptimization,
    UncommonTrap,
    WhiteboxDeoptimization,
    Zombie,
    NotUsed,
    OsrInvalidation,
    MissingExceptionHandler,
}

impl InvalidationReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::C1CodePatch => "C1 code patch",
            Self::C1Deoptimize => "C1 deoptimized",
            Self::MarkedForDeoptimization => "marked for deoptimization",
            Self::UncommonTrap => "uncommon trap",
            Self::WhiteboxDeoptimization => "whitebox deoptimization",
            Self::Zombie => "zombie",
            Self::NotUsed => "not used",
            Self::OsrInvalidation => "OSR invalidation",
            Self::MissingExceptionHandler => "missing exception handler",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NMethod {
    method_holder: String,
    method_name: String,
    method_signature: String,
    comp_level: CompLevel,
    state: NMethodState,
    invalidation_reason: Option<InvalidationReason>,
    entry_bci: i32,
    code_offset: usize,
    code_size: usize,
    entry_point: usize,
    verified_entry_point: usize,
    pc_descs: Vec<PcDesc>,
    scopes: Vec<ScopeDesc>,
    marked_for_deoptimization: bool,
    deoptimized_done: bool,
    has_unsafe_access: bool,
    stack_traversal_mark: u64,
}

impl NMethod {
    pub const INVOCATION_ENTRY_BCI: i32 = -1;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        method_holder: String,
        method_name: String,
        method_signature: String,
        comp_level: CompLevel,
        entry_bci: i32,
        code_offset: usize,
        code_size: usize,
        pc_descs: Vec<PcDesc>,
        scopes: Vec<ScopeDesc>,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            comp_level,
            state: NMethodState::InUse,
            invalidation_reason: None,
            entry_bci,
            code_offset,
            code_size,
            entry_point: code_offset,
            verified_entry_point: code_offset,
            pc_descs,
            scopes,
            marked_for_deoptimization: false,
            deoptimized_done: false,
            has_unsafe_access: false,
            stack_traversal_mark: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_entry_points(
        method_holder: String,
        method_name: String,
        method_signature: String,
        comp_level: CompLevel,
        entry_bci: i32,
        code_offset: usize,
        code_size: usize,
        entry_point: usize,
        verified_entry_point: usize,
        pc_descs: Vec<PcDesc>,
        scopes: Vec<ScopeDesc>,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            comp_level,
            state: NMethodState::InUse,
            invalidation_reason: None,
            entry_bci,
            code_offset,
            code_size,
            entry_point,
            verified_entry_point,
            pc_descs,
            scopes,
            marked_for_deoptimization: false,
            deoptimized_done: false,
            has_unsafe_access: false,
            stack_traversal_mark: 0,
        }
    }

    #[inline]
    pub fn method_holder(&self) -> &str {
        &self.method_holder
    }

    #[inline]
    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    #[inline]
    pub fn method_signature(&self) -> &str {
        &self.method_signature
    }

    #[inline]
    pub fn comp_level(&self) -> CompLevel {
        self.comp_level
    }

    #[inline]
    pub fn state(&self) -> NMethodState {
        self.state
    }

    #[inline]
    pub fn invalidation_reason(&self) -> Option<InvalidationReason> {
        self.invalidation_reason
    }

    #[inline]
    pub fn is_in_use(&self) -> bool {
        self.state == NMethodState::InUse
    }

    #[inline]
    pub fn is_not_entrant(&self) -> bool {
        self.state == NMethodState::NotEntrant
    }

    #[inline]
    pub fn is_zombie(&self) -> bool {
        self.state == NMethodState::Zombie
    }

    #[inline]
    pub fn is_unloaded(&self) -> bool {
        self.state == NMethodState::Unloaded
    }

    #[inline]
    pub fn is_osr_method(&self) -> bool {
        self.entry_bci != Self::INVOCATION_ENTRY_BCI
    }

    #[inline]
    pub fn entry_bci(&self) -> i32 {
        self.entry_bci
    }

    #[inline]
    pub fn code_offset(&self) -> usize {
        self.code_offset
    }

    #[inline]
    pub fn code_size(&self) -> usize {
        self.code_size
    }

    #[inline]
    pub fn code_begin(&self) -> usize {
        self.code_offset
    }

    #[inline]
    pub fn code_end(&self) -> usize {
        self.code_offset + self.code_size
    }

    #[inline]
    pub fn entry_point(&self) -> usize {
        self.entry_point
    }

    #[inline]
    pub fn verified_entry_point(&self) -> usize {
        self.verified_entry_point
    }

    #[inline]
    pub fn make_not_entrant(&mut self) {
        self.make_not_entrant_with_reason(InvalidationReason::NotUsed);
    }

    pub fn make_not_entrant_with_reason(&mut self, reason: InvalidationReason) -> bool {
        if self.state == NMethodState::InUse {
            self.state = NMethodState::NotEntrant;
            self.invalidation_reason = Some(reason);
            true
        } else {
            false
        }
    }

    pub fn make_zombie(&mut self) {
        self.state = NMethodState::Zombie;
        self.invalidation_reason = Some(InvalidationReason::Zombie);
    }

    pub fn mark_for_deoptimization(&mut self) {
        self.marked_for_deoptimization = true;
        self.make_not_entrant_with_reason(InvalidationReason::MarkedForDeoptimization);
    }

    #[inline]
    pub fn is_marked_for_deoptimization(&self) -> bool {
        self.marked_for_deoptimization
    }

    #[inline]
    pub fn set_deoptimized_done(&mut self) {
        self.deoptimized_done = true;
    }

    #[inline]
    pub fn has_been_deoptimized(&self) -> bool {
        self.deoptimized_done
    }

    #[inline]
    pub fn can_be_deoptimized(&self) -> bool {
        true
    }

    #[inline]
    pub fn has_unsafe_access(&self) -> bool {
        self.has_unsafe_access
    }

    #[inline]
    pub fn set_has_unsafe_access(&mut self, val: bool) {
        self.has_unsafe_access = val;
    }

    #[inline]
    pub fn stack_traversal_mark(&self) -> u64 {
        self.stack_traversal_mark
    }

    #[inline]
    pub fn set_stack_traversal_mark(&mut self, mark: u64) {
        self.stack_traversal_mark = mark;
    }

    #[inline]
    pub fn is_on_stack(&self, current_mark: u64) -> bool {
        self.stack_traversal_mark == current_mark
    }

    pub fn find_pc_desc(&self, pc_offset: i32) -> Option<&PcDesc> {
        self.pc_descs
            .iter()
            .find(|desc| desc.pc_offset() == pc_offset)
    }

    #[inline]
    pub fn pc_descs(&self) -> &[PcDesc] {
        &self.pc_descs
    }

    pub fn find_scope(&self, bci: i32) -> Option<&ScopeDesc> {
        self.scopes.iter().find(|s| s.bci() == bci)
    }

    #[inline]
    pub fn scopes(&self) -> &[ScopeDesc] {
        &self.scopes
    }

    pub fn scopes_for_pc(&self, pc_offset: i32) -> Vec<&ScopeDesc> {
        if let Some(pc_desc) = self.find_pc_desc(pc_offset) {
            let decode_offset = pc_desc.scope_decode_offset();
            self.scopes
                .iter()
                .filter(|s| s.bci() == decode_offset || s.sender_decode_offset() == decode_offset)
                .collect()
        } else {
            Vec::new()
        }
    }
}
