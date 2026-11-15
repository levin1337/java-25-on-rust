/*
 * Port of OpenJDK 25 HotSpot nmethod (share/code/nmethod.hpp / nmethod.cpp)
 *
 * nmethod represents a JIT compiled Java method in machine code, managing
 * compilation tiers, deoptimization state, on-stack replacement (OSR),
 * exception handler tables, oop maps and scope/pC-desc debug info.
 */

use super::exceptionHandlerTable::{ExceptionHandlerTable, PcRangeHandlerTable};
use super::pcDesc::{PcDesc, PcDescContainer};
use super::scopeDesc::{decode_scope_chain, ScopeDesc};
use crate::share::compiler::compilerDefinitions::CompLevel;
use crate::share::compiler::oopMap::OopMapSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

/// Shared, atomically-updated state of an installed nmethod.
/// Mirrors the mutable nmethod header fields that the sweeper, deopt
/// machinery and stack walking update concurrently.
#[derive(Debug)]
pub struct NMethodShared {
    state: AtomicU64, // NMethodState as u64
    marked_for_deoptimization: AtomicU64,
    deoptimized_done: AtomicU64,
    lock_count: AtomicU64,
    stack_traversal_mark: AtomicU64,
    has_unsafe_access: AtomicU64,
    pub invalidation_reason: std::sync::Mutex<Option<InvalidationReason>>,
    /// Compiled machine code bytes (empty until a backend installs them).
    pub code_bytes: std::sync::Mutex<Vec<u8>>,
    /// Serialized scopes-data blob (ScopeDescWriter output).
    pub scope_data: std::sync::Mutex<Vec<u8>>,
    /// Compiled exception handler subtable (CatchNode layout).
    pub handler_table: std::sync::Mutex<ExceptionHandlerTable>,
    /// Pc-range -> handler lookup table for frame unwinding.
    pub pc_handlers: std::sync::Mutex<PcRangeHandlerTable>,
    /// Safepoint oop maps.
    pub oop_maps: std::sync::Mutex<OopMapSet>,
}

impl NMethodShared {
    fn new() -> Self {
        Self {
            state: AtomicU64::new(NMethodState::InUse as u64),
            marked_for_deoptimization: AtomicU64::new(0),
            deoptimized_done: AtomicU64::new(0),
            lock_count: AtomicU64::new(0),
            stack_traversal_mark: AtomicU64::new(0),
            has_unsafe_access: AtomicU64::new(0),
            invalidation_reason: std::sync::Mutex::new(None),
            code_bytes: std::sync::Mutex::new(Vec::new()),
            scope_data: std::sync::Mutex::new(Vec::new()),
            handler_table: std::sync::Mutex::new(ExceptionHandlerTable::new(8)),
            pc_handlers: std::sync::Mutex::new(PcRangeHandlerTable::new()),
            oop_maps: std::sync::Mutex::new(OopMapSet::new()),
        }
    }
}

#[derive(Debug)]
pub struct NMethod {
    method_holder: String,
    method_name: String,
    method_signature: String,
    comp_level: CompLevel,
    entry_bci: i32,
    code_offset: usize,
    code_size: usize,
    entry_point: usize,
    verified_entry_point: usize,
    osr_entry_point: usize,
    pc_descs: Vec<PcDesc>,
    scopes: Vec<ScopeDesc>,
    shared: Arc<NMethodShared>,
}

impl Clone for NMethod {
    fn clone(&self) -> Self {
        Self {
            method_holder: self.method_holder.clone(),
            method_name: self.method_name.clone(),
            method_signature: self.method_signature.clone(),
            comp_level: self.comp_level,
            entry_bci: self.entry_bci,
            code_offset: self.code_offset,
            code_size: self.code_size,
            entry_point: self.entry_point,
            verified_entry_point: self.verified_entry_point,
            osr_entry_point: self.osr_entry_point,
            pc_descs: self.pc_descs.clone(),
            scopes: self.scopes.clone(),
            shared: Arc::clone(&self.shared),
        }
    }
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
            entry_bci,
            code_offset,
            code_size,
            entry_point: code_offset,
            verified_entry_point: code_offset,
            osr_entry_point: code_offset,
            pc_descs,
            scopes,
            shared: Arc::new(NMethodShared::new()),
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
        osr_entry_point: usize,
        pc_descs: Vec<PcDesc>,
        scopes: Vec<ScopeDesc>,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            comp_level,
            entry_bci,
            code_offset,
            code_size,
            entry_point,
            verified_entry_point,
            osr_entry_point,
            pc_descs,
            scopes,
            shared: Arc::new(NMethodShared::new()),
        }
    }

    /// Wrap an already-built shared state (used when the nmethod metadata
    /// was filled in by the compiler before installation).
    pub fn with_shared(
        method_holder: String,
        method_name: String,
        method_signature: String,
        comp_level: CompLevel,
        entry_bci: i32,
        code_offset: usize,
        code_size: usize,
        entry_point: usize,
        verified_entry_point: usize,
        osr_entry_point: usize,
        pc_descs: Vec<PcDesc>,
        scopes: Vec<ScopeDesc>,
        shared: Arc<NMethodShared>,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            comp_level,
            entry_bci,
            code_offset,
            code_size,
            entry_point,
            verified_entry_point,
            osr_entry_point,
            pc_descs,
            scopes,
            shared,
        }
    }

    /// Relocate the nmethod to its allocated code-cache address
    /// (called once at install time, before publication).
    pub fn set_code_location(&mut self, offset: usize, size: usize) {
        self.code_offset = offset;
        self.code_size = size;
        self.entry_point = offset;
        self.verified_entry_point = offset;
        self.osr_entry_point = offset;
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
    fn state_u64(&self) -> u64 {
        self.shared.state.load(Ordering::Acquire)
    }

    #[inline]
    pub fn state(&self) -> NMethodState {
        match self.state_u64() {
            0 => NMethodState::NotInstalled,
            1 => NMethodState::InUse,
            2 => NMethodState::NotEntrant,
            3 => NMethodState::Zombie,
            _ => NMethodState::Unloaded,
        }
    }

    fn set_state(&self, s: NMethodState) {
        self.shared.state.store(s as u64, Ordering::Release);
    }

    #[inline]
    pub fn invalidation_reason(&self) -> Option<InvalidationReason> {
        *self.shared.invalidation_reason.lock().unwrap()
    }

    #[inline]
    pub fn is_in_use(&self) -> bool {
        self.state() == NMethodState::InUse
    }

    #[inline]
    pub fn is_not_entrant(&self) -> bool {
        self.state() == NMethodState::NotEntrant
    }

    #[inline]
    pub fn is_zombie(&self) -> bool {
        self.state() == NMethodState::Zombie
    }

    #[inline]
    pub fn is_unloaded(&self) -> bool {
        matches!(self.state(), NMethodState::Zombie | NMethodState::Unloaded)
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
    pub fn osr_entry_point(&self) -> usize {
        self.osr_entry_point
    }

    /// Lifecycle: transition InUse -> NotEntrant (nmethod::make_not_entrant).
    /// Returns false if the nmethod was not in use.
    pub fn make_not_entrant_with_reason(&self, reason: InvalidationReason) -> bool {
        // CAS loop on the shared state.
        loop {
            let cur = self.state_u64();
            if cur != NMethodState::InUse as u64 {
                return false;
            }
            if self
                .shared
                .state
                .compare_exchange(
                    cur,
                    NMethodState::NotEntrant as u64,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                *self.shared.invalidation_reason.lock().unwrap() = Some(reason);
                return true;
            }
        }
    }

    pub fn make_not_entrant(&self) {
        self.make_not_entrant_with_reason(InvalidationReason::NotUsed);
    }

    /// Mark for deoptimization (nmethod::mark_for_deoptimization).
    pub fn mark_for_deoptimization(&self) {
        self.shared
            .marked_for_deoptimization
            .store(1, Ordering::Release);
    }

    #[inline]
    pub fn is_marked_for_deoptimization(&self) -> bool {
        self.shared.marked_for_deoptimization.load(Ordering::Acquire) != 0
    }

    /// Called when the deoptimization has completed on all stacks.
    pub fn deoptimization_done(&self) {
        self.shared.deoptimized_done.store(1, Ordering::Release);
    }

    #[inline]
    pub fn is_deoptimization_done(&self) -> bool {
        self.shared.deoptimized_done.load(Ordering::Acquire) != 0
    }

    /// Transition NotEntrant -> Zombie once no activations remain
    /// (sweeper cooperation, `nmethod::make_zombie`).
    pub fn make_zombie(&self) {
        self.set_state(NMethodState::Zombie);
        *self.shared.invalidation_reason.lock().unwrap() = Some(InvalidationReason::Zombie);
    }

    pub fn make_unloaded(&self) {
        self.set_state(NMethodState::Unloaded);
    }

    #[inline]
    pub fn lock_count(&self) -> u64 {
        self.shared.lock_count.load(Ordering::Acquire)
    }

    pub fn lock(&self) {
        self.shared.lock_count.fetch_add(1, Ordering::AcqRel);
    }

    pub fn unlock(&self) {
        self.shared.lock_count.fetch_sub(1, Ordering::AcqRel);
    }

    #[inline]
    pub fn has_unsafe_access(&self) -> bool {
        self.shared.has_unsafe_access.load(Ordering::Acquire) != 0
    }

    pub fn set_has_unsafe_access(&self, val: bool) {
        self.shared
            .has_unsafe_access
            .store(val as u64, Ordering::Release);
    }

    #[inline]
    pub fn stack_traversal_mark(&self) -> u64 {
        self.shared.stack_traversal_mark.load(Ordering::Acquire)
    }

    pub fn set_stack_traversal_mark(&self, mark: u64) {
        self.shared
            .stack_traversal_mark
            .store(mark, Ordering::Release);
    }

    #[inline]
    pub fn is_on_stack(&self, current_mark: u64) -> bool {
        self.stack_traversal_mark() == current_mark
    }

    // ---- pc desc / scope lookup -------------------------------------

    /// `nmethod::pc_desc_at`: exact pc -> PcDesc via PcDescContainer.
    pub fn pc_desc_at(&self, pc: usize) -> Option<PcDesc> {
        if pc < self.code_begin() || pc >= self.code_end() {
            return None;
        }
        let pc_offset = (pc - self.code_begin()) as i32;
        let mut container = PcDescContainer::new(&self.pc_descs);
        container
            .find_pc_desc(&self.pc_descs, pc_offset, false)
            .map(|idx| self.pc_descs[idx])
    }

    /// `nmethod::pc_desc_near`: greatest PcDesc with offset <= pc.
    pub fn pc_desc_near(&self, pc: usize) -> Option<PcDesc> {
        if pc < self.code_begin() {
            return None;
        }
        let pc_offset = (pc - self.code_begin()) as i32;
        let idx = self
            .pc_descs
            .partition_point(|d| d.pc_offset() <= pc_offset);
        if idx == 0 {
            None
        } else {
            Some(self.pc_descs[idx - 1])
        }
    }

    #[inline]
    pub fn pc_descs(&self) -> &[PcDesc] {
        &self.pc_descs
    }

    /// Sorted accessor: pc_descs are kept sorted by pc_offset.
    pub fn pc_descs_sorted(&self) -> &[PcDesc] {
        &self.pc_descs
    }

    /// `ScopeDesc` lookup for a pc: find the PcDesc, then decode the
    /// scope chain from the scopes-data blob.
    pub fn scope_desc_at_pc(&self, pc: usize) -> Vec<ScopeDesc> {
        let data = self.shared.scope_data.lock().unwrap();
        if data.is_empty() {
            return Vec::new();
        }
        if let Some(pd) = self.pc_desc_at(pc) {
            return decode_scope_chain(&data, pd.scope_decode_offset());
        }
        Vec::new()
    }

    /// Decode the full inlinee chain for a given bci (used by deopt).
    pub fn scope_chain_for_bci(&self, bci: i32) -> Vec<ScopeDesc> {
        self.scopes
            .iter()
            .filter(|s| s.bci() == bci)
            .cloned()
            .collect()
    }

    pub fn find_scope(&self, bci: i32) -> Option<&ScopeDesc> {
        self.scopes.iter().find(|s| s.bci() == bci)
    }

    #[inline]
    pub fn scopes(&self) -> &[ScopeDesc] {
        &self.scopes
    }

    /// Exception handler lookup during frame unwinding: returns the
    /// handler pc for `pc` throwing `thrown_type`.
    pub fn exception_handler_for(
        &self,
        pc: usize,
        thrown_type: &crate::share::oops::symbol::Symbol,
    ) -> Option<usize> {
        if pc < self.code_begin() {
            return None;
        }
        let pc_offset = (pc - self.code_begin()) as i32;
        let table = self.shared.pc_handlers.lock().unwrap();
        table
            .lookup_exact(pc_offset, thrown_type)
            .map(|e| self.code_begin() + e.handler_pc as usize)
    }

    /// The oop map covering `pc` (greatest safepoint offset <= pc).
    pub fn oop_map_for_pc(&self, pc: usize) -> Option<crate::share::compiler::oopMap::OopMap> {
        if pc < self.code_begin() {
            return None;
        }
        let pc_offset = (pc - self.code_begin()) as i32;
        self.shared
            .oop_maps
            .lock()
            .unwrap()
            .map_for_pc(pc_offset)
            .cloned()
    }

    // ---- shared mutable state accessors ------------------------------

    pub fn shared(&self) -> &Arc<NMethodShared> {
        &self.shared
    }

    pub fn code_bytes(&self) -> Vec<u8> {
        self.shared.code_bytes.lock().unwrap().clone()
    }

    pub fn set_code_bytes(&self, bytes: Vec<u8>) {
        *self.shared.code_bytes.lock().unwrap() = bytes;
    }

    pub fn scope_data(&self) -> Vec<u8> {
        self.shared.scope_data.lock().unwrap().clone()
    }

    pub fn set_scope_data(&self, data: Vec<u8>) {
        *self.shared.scope_data.lock().unwrap() = data;
    }

    pub fn handler_table(&self) -> ExceptionHandlerTable {
        self.shared.handler_table.lock().unwrap().clone()
    }

    pub fn set_handler_table(&self, table: ExceptionHandlerTable) {
        *self.shared.handler_table.lock().unwrap() = table;
    }

    pub fn pc_handlers(&self) -> PcRangeHandlerTable {
        self.shared.pc_handlers.lock().unwrap().clone()
    }

    pub fn set_pc_handlers(&self, table: PcRangeHandlerTable) {
        *self.shared.pc_handlers.lock().unwrap() = table;
    }

    pub fn oop_maps(&self) -> OopMapSet {
        self.shared.oop_maps.lock().unwrap().clone()
    }

    pub fn add_oop_map(&self, pc_offset: i32, map: crate::share::compiler::oopMap::OopMap) {
        self.shared.oop_maps.lock().unwrap().add_gc_map(pc_offset, map);
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(
            f,
            "nmethod {}.{}{} level={} state={:?} osr={} bci={} code=[{}..{}) size={}",
            self.method_holder,
            self.method_name,
            self.method_signature,
            self.comp_level.name(),
            self.state(),
            self.is_osr_method(),
            self.entry_bci,
            self.code_begin(),
            self.code_end(),
            self.code_size,
        );
        if let Some(reason) = self.invalidation_reason() {
            let _ = writeln!(f, "  invalidation: {}", reason.as_str());
        }
        if self.is_marked_for_deoptimization() {
            let _ = writeln!(f, "  marked for deoptimization");
        }
        let _ = writeln!(f, "  pc_descs={} scopes={} oop_maps={}",
            self.pc_descs.len(),
            self.scopes.len(),
            self.shared.oop_maps.lock().unwrap().size());
    }
}
