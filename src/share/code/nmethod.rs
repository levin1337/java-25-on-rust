/*
 * Port of OpenJDK 25 HotSpot nmethod (share/code/nmethod.hpp / nmethod.cpp)
 *
 * nmethod represents a JIT compiled Java method in machine code, managing
 * compilation tiers, deoptimization state, on-stack replacement (OSR),
 * exception handler tables, oop maps and scope/pC-desc debug info.
 */

use super::dependencies::DependencySet;
use super::exceptionHandlerTable::{ExceptionHandlerTable, PcRangeHandlerTable};
use super::pcDesc::{PcDesc, PcDescContainer};
use super::scopeDesc::{ScopeDesc, decode_scope_chain_with_objects};
use crate::share::compiler::compilerDefinitions::CompLevel;
use crate::share::compiler::oopMap::OopMapSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-bci live-reference map for deoptimization (`ScopeValue`-level
/// detail lives in the scope-data blob; this is the conservative
/// slot-level map the interpreter needs to rebuild a frame: which
/// local-variable slots and operand-stack slots hold live references
/// at a given bytecode index).
///
/// Slot numbering follows the JVM frame layout: locals are indexed
/// 0..max_locals (long/double occupy two slots, the second marked
/// dead), operand-stack slots follow at max_locals..max_locals+depth.
/// A slot listed here is known to contain a reference at this bci;
/// slots NOT listed may still hold references the compiler could not
/// prove dead — callers that need full safety should treat the map as
/// "at least these are live" and combine it with the oop maps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeoptLiveMap {
    bci: i32,
    /// Local-variable slot indices holding live references.
    local_ref_slots: Vec<u16>,
    /// Operand-stack slot indices (0-based within the operand stack,
    /// NOT including the locals base) holding live references.
    stack_ref_slots: Vec<u16>,
}

impl DeoptLiveMap {
    pub fn new(bci: i32) -> Self {
        Self {
            bci,
            local_ref_slots: Vec::new(),
            stack_ref_slots: Vec::new(),
        }
    }

    /// Most conservative map: every slot of the frame is a live
    /// reference.  Used when the compiler cannot compute precise liveness
    /// (e.g. C1 without full dataflow) — deopt then preserves every
    /// reference-typed slot conservatively.
    pub fn conservative(bci: i32, local_count: usize, stack_depth: usize) -> Self {
        let mut m = Self::new(bci);
        m.local_ref_slots = (0..local_count.min(u16::MAX as usize) as u16).collect();
        m.stack_ref_slots = (0..stack_depth.min(u16::MAX as usize) as u16).collect();
        m
    }

    /// Build from explicit reference-slot sets (locals + operand stack).
    pub fn from_ref_slots(bci: i32, local_ref_slots: Vec<u16>, stack_ref_slots: Vec<u16>) -> Self {
        Self {
            bci,
            local_ref_slots,
            stack_ref_slots,
        }
    }

    #[inline]
    pub fn bci(&self) -> i32 {
        self.bci
    }

    pub fn add_local_ref_slot(&mut self, slot: u16) {
        if !self.local_ref_slots.contains(&slot) {
            self.local_ref_slots.push(slot);
            self.local_ref_slots.sort_unstable();
        }
    }

    pub fn add_stack_ref_slot(&mut self, slot: u16) {
        if !self.stack_ref_slots.contains(&slot) {
            self.stack_ref_slots.push(slot);
            self.stack_ref_slots.sort_unstable();
        }
    }

    #[inline]
    pub fn local_ref_slots(&self) -> &[u16] {
        &self.local_ref_slots
    }

    #[inline]
    pub fn stack_ref_slots(&self) -> &[u16] {
        &self.stack_ref_slots
    }

    /// Whether local slot `slot` is a live reference at this bci.
    pub fn is_live_local_ref(&self, slot: u16) -> bool {
        self.local_ref_slots.binary_search(&slot).is_ok()
    }

    /// Whether operand-stack slot `slot` (0-based) is a live reference.
    pub fn is_live_stack_ref(&self, slot: u16) -> bool {
        self.stack_ref_slots.binary_search(&slot).is_ok()
    }

    /// Total live references recorded.
    pub fn live_ref_count(&self) -> usize {
        self.local_ref_slots.len() + self.stack_ref_slots.len()
    }

    /// Serialized size estimate for code-cache metadata accounting.
    pub fn encoded_size(&self) -> usize {
        8 + self.local_ref_slots.len() * 2 + self.stack_ref_slots.len() * 2
    }
}

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

/// `nmethod::DeoptimizationStatus` (share/code/nmethod.hpp): handshake
/// state of stack deoptimization for this nmethod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u64)]
pub enum DeoptimizationStatus {
    NotMarked = 0,
    Deoptimize = 1,
    DeoptimizeNoUpdate = 2,
    DeoptimizeDone = 3,
}

/// Code-section layout of an installed nmethod
/// (`nmethod` SECT_CONSTS + immutable-data layout analogue).  All
/// addresses are absolute (code-cache) addresses; `metadata_*` spans the
/// immutable data section that holds dependencies, the handler table,
/// serialized pc descs and the scopes-data blob offsets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NMethodSections {
    /// SECT_CONSTS: constant pool / metadata constants emitted before
    /// the instruction stream (HotSpot: consts_begin..consts_end ==
    /// content_begin..code_begin).
    pub consts_begin: usize,
    pub consts_end: usize,
    /// Immutable data section (C-heap analogue `_immutable_data`).
    pub metadata_begin: usize,
    pub metadata_end: usize,
    /// Serialized PcDesc array region inside the metadata section
    /// (`scopes_pcs_begin..scopes_pcs_end` analogue).
    pub scopes_pcs_begin: usize,
    pub scopes_pcs_end: usize,
}

/// Shared, atomically-updated state of an installed nmethod.
/// Mirrors the mutable nmethod header fields that the sweeper, deopt
/// machinery and stack walking update concurrently.
#[derive(Debug)]
pub struct NMethodShared {
    state: AtomicU64,        // NMethodState as u64
    deopt_status: AtomicU64, // DeoptimizationStatus as u64
    lock_count: AtomicU64,
    stack_traversal_mark: AtomicU64,
    has_unsafe_access: AtomicU64,
    has_monitors: AtomicU64,
    has_scoped_access: AtomicU64,
    has_wide_vectors: AtomicU64,
    has_method_handle_invokes: AtomicU64,
    has_flushed_dependencies: AtomicU64,
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
    /// Per-bci live-reference maps for deoptimization, sorted by bci.
    pub live_maps: std::sync::Mutex<Vec<DeoptLiveMap>>,
    /// Compiled frame size in bytes (needed to unwind/rebuild the
    /// interpreter frame on deopt).
    pub frame_size: AtomicU64,
    /// Dependencies recorded at compile time (klass/method unloading
    /// assumptions; `Dependencies` analogue).
    pub dependencies: std::sync::Mutex<DependencySet>,
}

impl NMethodShared {
    fn new() -> Self {
        Self {
            state: AtomicU64::new(NMethodState::InUse as u64),
            deopt_status: AtomicU64::new(DeoptimizationStatus::NotMarked as u64),
            lock_count: AtomicU64::new(0),
            stack_traversal_mark: AtomicU64::new(0),
            has_unsafe_access: AtomicU64::new(0),
            has_monitors: AtomicU64::new(0),
            has_scoped_access: AtomicU64::new(0),
            has_wide_vectors: AtomicU64::new(0),
            has_method_handle_invokes: AtomicU64::new(0),
            has_flushed_dependencies: AtomicU64::new(0),
            invalidation_reason: std::sync::Mutex::new(None),
            code_bytes: std::sync::Mutex::new(Vec::new()),
            scope_data: std::sync::Mutex::new(Vec::new()),
            handler_table: std::sync::Mutex::new(ExceptionHandlerTable::new(8)),
            pc_handlers: std::sync::Mutex::new(PcRangeHandlerTable::new()),
            oop_maps: std::sync::Mutex::new(OopMapSet::new()),
            live_maps: std::sync::Mutex::new(Vec::new()),
            frame_size: AtomicU64::new(0),
            dependencies: std::sync::Mutex::new(DependencySet::new()),
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
    sections: NMethodSections,
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
            sections: self.sections,
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
            sections: NMethodSections::default(),
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
            sections: NMethodSections::default(),
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
            sections: NMethodSections::default(),
            shared,
        }
    }

    /// Relocate the nmethod to its allocated code-cache address
    /// (called once at install time, before publication).  Absolute
    /// section addresses recorded in `sections` are shifted by the same
    /// delta.
    pub fn set_code_location(&mut self, offset: usize, size: usize) {
        let delta = offset as isize - self.code_offset as isize;
        self.code_offset = offset;
        self.code_size = size;
        self.entry_point = offset;
        self.verified_entry_point = offset;
        self.osr_entry_point = offset;
        if delta != 0 {
            let s = &mut self.sections;
            for addr in [
                &mut s.consts_begin,
                &mut s.consts_end,
                &mut s.metadata_begin,
                &mut s.metadata_end,
                &mut s.scopes_pcs_begin,
                &mut s.scopes_pcs_end,
            ] {
                if *addr != 0 {
                    *addr = (*addr as isize + delta) as usize;
                }
            }
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
    pub fn set_entry_point(&mut self, addr: usize) {
        self.entry_point = addr;
    }

    #[inline]
    pub fn verified_entry_point(&self) -> usize {
        self.verified_entry_point
    }

    /// `nmethod::verified_entry_offset` analogue.
    #[inline]
    pub fn verified_entry_offset(&self) -> usize {
        self.verified_entry_point.saturating_sub(self.code_begin())
    }

    pub fn set_verified_entry_point(&mut self, addr: usize) {
        self.verified_entry_point = addr;
    }

    #[inline]
    pub fn osr_entry_point(&self) -> usize {
        self.osr_entry_point
    }

    /// `nmethod::osr_entry_bci` analogue (== entry_bci for OSR nmethods).
    #[inline]
    pub fn osr_entry_bci(&self) -> i32 {
        debug_assert!(self.is_osr_method(), "wrong kind of nmethod");
        self.entry_bci
    }

    /// `nmethod::osr_entry` analogue.
    #[inline]
    pub fn osr_entry(&self) -> usize {
        debug_assert!(self.is_osr_method(), "wrong kind of nmethod");
        self.osr_entry_point
    }

    pub fn set_osr_entry_point(&mut self, addr: usize) {
        self.osr_entry_point = addr;
    }

    /// `nmethod::is_entrant` analogue: the nmethod may be entered by new
    /// activations (in_use).  NotEntrant code stays alive for existing
    /// frames but takes no new ones.
    #[inline]
    pub fn is_entrant(&self) -> bool {
        self.is_in_use()
    }

    /// `nmethod::is_java_method` analogue: every nmethod in this port
    /// describes a Java method (native wrappers are not modelled).
    #[inline]
    pub fn is_java_method(&self) -> bool {
        true
    }

    /// `nmethod::can_be_deoptimized`: java methods can; native wrappers
    /// cannot.
    #[inline]
    pub fn can_be_deoptimized(&self) -> bool {
        self.is_java_method()
    }

    // ---- code sections (consts / immutable metadata) ------------------

    #[inline]
    pub fn sections(&self) -> NMethodSections {
        self.sections
    }

    pub fn set_sections(&mut self, sections: NMethodSections) {
        self.sections = sections;
    }

    /// SECT_CONSTS region (`nmethod::consts_begin/end` analogue).
    #[inline]
    pub fn consts_begin(&self) -> usize {
        self.sections.consts_begin
    }

    #[inline]
    pub fn consts_end(&self) -> usize {
        self.sections.consts_end
    }

    #[inline]
    pub fn consts_size(&self) -> usize {
        self.sections
            .consts_end
            .saturating_sub(self.sections.consts_begin)
    }

    #[inline]
    pub fn consts_contains(&self, addr: usize) -> bool {
        addr >= self.sections.consts_begin && addr < self.sections.consts_end
    }

    /// Immutable data section (`nmethod::immutable_data_begin/end`).
    #[inline]
    pub fn metadata_begin(&self) -> usize {
        self.sections.metadata_begin
    }

    #[inline]
    pub fn metadata_end(&self) -> usize {
        self.sections.metadata_end
    }

    #[inline]
    pub fn metadata_size(&self) -> usize {
        self.sections
            .metadata_end
            .saturating_sub(self.sections.metadata_begin)
    }

    /// Dependencies occupy the head of the immutable data section
    /// (`nmethod::dependencies_begin/end` analogue).
    #[inline]
    pub fn dependencies_begin(&self) -> usize {
        self.sections.metadata_begin
    }

    #[inline]
    pub fn dependencies_end(&self) -> usize {
        self.sections.metadata_begin + self.dependencies_size()
    }

    #[inline]
    pub fn dependencies_size(&self) -> usize {
        self.shared.dependencies.lock().unwrap().size_in_bytes()
    }

    /// Serialized PcDesc region inside the metadata section
    /// (`nmethod::scopes_pcs_begin/end` analogue).
    #[inline]
    pub fn scopes_pcs_begin(&self) -> usize {
        self.sections.scopes_pcs_begin
    }

    #[inline]
    pub fn scopes_pcs_end(&self) -> usize {
        self.sections.scopes_pcs_end
    }

    #[inline]
    pub fn scopes_pcs_size(&self) -> usize {
        self.sections
            .scopes_pcs_end
            .saturating_sub(self.sections.scopes_pcs_begin)
    }

    #[inline]
    pub fn has_dependencies(&self) -> bool {
        self.dependencies_size() != 0
    }

    // ---- header flags --------------------------------------------------

    #[inline]
    pub fn has_monitors(&self) -> bool {
        self.shared.has_monitors.load(Ordering::Acquire) != 0
    }

    pub fn set_has_monitors(&self, val: bool) {
        self.shared
            .has_monitors
            .store(val as u64, Ordering::Release);
    }

    #[inline]
    pub fn has_scoped_access(&self) -> bool {
        self.shared.has_scoped_access.load(Ordering::Acquire) != 0
    }

    pub fn set_has_scoped_access(&self, val: bool) {
        self.shared
            .has_scoped_access
            .store(val as u64, Ordering::Release);
    }

    #[inline]
    pub fn has_wide_vectors(&self) -> bool {
        self.shared.has_wide_vectors.load(Ordering::Acquire) != 0
    }

    pub fn set_has_wide_vectors(&self, val: bool) {
        self.shared
            .has_wide_vectors
            .store(val as u64, Ordering::Release);
    }

    #[inline]
    pub fn has_method_handle_invokes(&self) -> bool {
        self.shared
            .has_method_handle_invokes
            .load(Ordering::Acquire)
            != 0
    }

    pub fn set_has_method_handle_invokes(&self, val: bool) {
        self.shared
            .has_method_handle_invokes
            .store(val as u64, Ordering::Release);
    }

    #[inline]
    pub fn has_flushed_dependencies(&self) -> bool {
        self.shared.has_flushed_dependencies.load(Ordering::Acquire) != 0
    }

    /// `nmethod::set_has_flushed_dependencies` (one-shot in HotSpot).
    pub fn set_has_flushed_dependencies(&self, val: bool) {
        self.shared
            .has_flushed_dependencies
            .store(val as u64, Ordering::Release);
    }

    // ---- dependencies ---------------------------------------------------

    /// Record a dependency (`nmethod::add_dependency` analogue, via the
    /// Dependencies recorder).
    pub fn add_dependency(&self, dep: super::dependencies::Dependency) {
        self.shared.dependencies.lock().unwrap().add(dep);
    }

    /// Snapshot of the recorded dependency set.
    pub fn dependencies(&self) -> DependencySet {
        self.shared.dependencies.lock().unwrap().clone()
    }

    /// `nmethod::flush_dependencies` analogue: drop all recorded
    /// dependencies and mark the nmethod as flushed.
    pub fn flush_dependencies(&self) {
        self.shared.dependencies.lock().unwrap().clear();
        self.set_has_flushed_dependencies(true);
    }

    /// `nmethod::print_dependencies_on` analogue.
    pub fn print_dependencies_on(&self, f: &mut dyn std::fmt::Write) {
        let deps = self.shared.dependencies.lock().unwrap();
        deps.print_on(f);
    }

    /// Check whether any recorded dependency is invalidated by a change
    /// to `klass_name`.  `is_subtype(a, b)` reports whether klass `a`
    /// is a subtype of `b` (DependencyContext analogue).
    pub fn dependencies_affected_by<F>(&self, klass_name: &str, is_subtype: F) -> bool
    where
        F: Fn(&str, &str) -> bool,
    {
        self.shared
            .dependencies
            .lock()
            .unwrap()
            .has_dependency_on(klass_name, is_subtype)
    }

    // ---- verification ---------------------------------------------------

    /// Structural verification of inline-cache / debug-info legality
    /// (`verify_legal_inline_cache` analogue): every PcDesc must point
    /// inside the code stream, pc_descs must be sorted, scope decode
    /// offsets must reference the scopes-data blob, and the handler
    /// tables must stay inside the code range.  Returns the first
    /// problem found.
    pub fn verify_legal_inline_cache(&self) -> Result<(), String> {
        let code_size = self.code_size as i64;
        let scope_len = self.shared.scope_data.lock().unwrap().len() as i64;
        let mut prev = i64::MIN;
        for (i, pd) in self.pc_descs.iter().enumerate() {
            let off = pd.pc_offset() as i64;
            // Sentinel records bracket the array and legitimately sit
            // outside the code stream.
            if off == PcDesc::LOWER_OFFSET_LIMIT as i64 || off == PcDesc::UPPER_OFFSET_LIMIT as i64
            {
                continue;
            }
            if off < 0 || off > code_size {
                return Err(format!(
                    "pc_desc[{}] pc_offset {} outside code [0, {})",
                    i, off, code_size
                ));
            }
            if i > 0 && off < prev {
                return Err(format!("pc_desc[{}] out of order: {} < {}", i, off, prev));
            }
            prev = off;
            let sdo = pd.scope_decode_offset() as i64;
            if sdo != super::scopeDesc::SERIALIZED_NULL as i64
                && sdo != super::scopeDesc::NO_SENDER_DECODE_OFFSET as i64
                && (sdo < 0 || scope_len == 0 || sdo >= scope_len)
            {
                return Err(format!(
                    "pc_desc[{}] scope_decode_offset {} outside scopes data (len {})",
                    i, sdo, scope_len
                ));
            }
            if pd.is_method_handle_invoke() && sdo <= 0 {
                return Err(format!(
                    "pc_desc[{}] method-handle invoke without scope info",
                    i
                ));
            }
        }
        // Handler tables must reference pcs inside the code stream.
        let pc_handlers = self.shared.pc_handlers.lock().unwrap();
        for e in pc_handlers.entries() {
            if e.start_pc < 0
                || e.end_pc > self.code_size as i32
                || e.handler_pc < 0
                || e.handler_pc >= self.code_size as i32
            {
                return Err(format!(
                    "handler entry [{},{}) -> {} outside code size {}",
                    e.start_pc, e.end_pc, e.handler_pc, self.code_size
                ));
            }
        }
        Ok(())
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

    // ---- deoptimization status ----------------------------------------

    #[inline]
    pub fn deoptimization_status(&self) -> DeoptimizationStatus {
        match self.shared.deopt_status.load(Ordering::Acquire) {
            1 => DeoptimizationStatus::Deoptimize,
            2 => DeoptimizationStatus::DeoptimizeNoUpdate,
            3 => DeoptimizationStatus::DeoptimizeDone,
            _ => DeoptimizationStatus::NotMarked,
        }
    }

    fn set_deoptimization_status(&self, status: DeoptimizationStatus) {
        self.shared
            .deopt_status
            .store(status as u64, Ordering::Release);
    }

    /// Mark for deoptimization (nmethod::mark_for_deoptimization):
    /// transitions not_marked -> deoptimize.  Returns true if this call
    /// performed the transition.
    pub fn mark_for_deoptimization(&self) -> bool {
        self.shared
            .deopt_status
            .compare_exchange(
                DeoptimizationStatus::NotMarked as u64,
                DeoptimizationStatus::Deoptimize as u64,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Mark for deoptimization without recompile-count update
    /// (deoptimize_noupdate, e.g. uncommon traps).
    pub fn mark_for_deoptimization_noupdate(&self) -> bool {
        self.shared
            .deopt_status
            .compare_exchange(
                DeoptimizationStatus::NotMarked as u64,
                DeoptimizationStatus::DeoptimizeNoUpdate as u64,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// `nmethod::is_marked_for_deoptimization`: true for every status
    /// other than not_marked (including deoptimize_done).
    #[inline]
    pub fn is_marked_for_deoptimization(&self) -> bool {
        self.deoptimization_status() != DeoptimizationStatus::NotMarked
    }

    /// `nmethod::has_been_deoptimized`.
    #[inline]
    pub fn has_been_deoptimized(&self) -> bool {
        self.deoptimization_status() == DeoptimizationStatus::DeoptimizeDone
    }

    /// `nmethod::update_recompile_counts`: whether the caller should
    /// update invocation/backedge recompile counts.
    #[inline]
    pub fn update_recompile_counts(&self) -> bool {
        !matches!(
            self.deoptimization_status(),
            DeoptimizationStatus::DeoptimizeNoUpdate | DeoptimizationStatus::DeoptimizeDone
        )
    }

    /// Called when the deoptimization has completed on all stacks
    /// (`nmethod::set_deoptimized_done`).
    pub fn deoptimization_done(&self) {
        self.set_deoptimization_status(DeoptimizationStatus::DeoptimizeDone);
    }

    /// `nmethod::set_deoptimized_done` alias.
    pub fn set_deoptimized_done(&self) {
        self.deoptimization_done();
    }

    #[inline]
    pub fn is_deoptimization_done(&self) -> bool {
        self.deoptimization_status() == DeoptimizationStatus::DeoptimizeDone
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
    /// scope chain from the scopes-data blob.  PcDesc flags
    /// (reexecute/return_oop/rethrow/has_ea_local/arg_escape) are applied
    /// to the top scope, matching `ScopeDesc::decode_body` + PcDesc flag
    /// reads in HotSpot.
    pub fn scope_desc_at_pc(&self, pc: usize) -> Vec<ScopeDesc> {
        let data = self.shared.scope_data.lock().unwrap();
        if data.is_empty() {
            return Vec::new();
        }
        if let Some(pd) = self.pc_desc_at(pc) {
            let mut chain = decode_scope_chain_with_objects(
                &data,
                pd.scope_decode_offset(),
                pd.obj_decode_offset(),
            );
            if let Some(top) = chain.first_mut() {
                top.apply_pc_desc_flags(
                    pd.should_reexecute(),
                    pd.is_return_oop(),
                    pd.rethrow_exception(),
                    pd.has_ea_local_in_scope(),
                    pd.arg_escape(),
                );
            }
            return chain;
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
        self.shared
            .oop_maps
            .lock()
            .unwrap()
            .add_gc_map(pc_offset, map);
    }

    // ---- deoptimization data -----------------------------------------

    /// Compiled frame size in bytes (0 until the compiler reports it).
    #[inline]
    pub fn frame_size(&self) -> usize {
        self.shared.frame_size.load(Ordering::Acquire) as usize
    }

    pub fn set_frame_size(&self, size: usize) {
        self.shared.frame_size.store(size as u64, Ordering::Release);
    }

    /// Replace the per-bci live-reference maps (kept sorted by bci).
    pub fn set_live_maps(&self, mut maps: Vec<DeoptLiveMap>) {
        maps.sort_by_key(|m| m.bci);
        maps.dedup_by_key(|m| m.bci);
        *self.shared.live_maps.lock().unwrap() = maps;
    }

    /// Add or replace the live map for one bci.
    pub fn add_live_map(&self, map: DeoptLiveMap) {
        let mut maps = self.shared.live_maps.lock().unwrap();
        match maps.binary_search_by_key(&map.bci, |m| m.bci) {
            Ok(idx) => maps[idx] = map,
            Err(idx) => maps.insert(idx, map),
        }
    }

    /// All recorded live maps, sorted by bci.
    pub fn live_maps(&self) -> Vec<DeoptLiveMap> {
        self.shared.live_maps.lock().unwrap().clone()
    }

    /// Exact live-reference map for `bci`.
    pub fn live_map_for_bci(&self, bci: i32) -> Option<DeoptLiveMap> {
        let maps = self.shared.live_maps.lock().unwrap();
        maps.binary_search_by_key(&bci, |m| m.bci)
            .ok()
            .map(|idx| maps[idx].clone())
    }

    /// Greatest live map with bci <= `bci` (safepoint granularity is
    /// per-pc-desc; when no exact bci map exists the previous one is the
    /// conservative over-approximation of liveness).
    pub fn live_map_near_bci(&self, bci: i32) -> Option<DeoptLiveMap> {
        let maps = self.shared.live_maps.lock().unwrap();
        let idx = maps.partition_point(|m| m.bci <= bci);
        if idx == 0 {
            None
        } else {
            Some(maps[idx - 1].clone())
        }
    }

    /// Combined deopt lookup for a native `pc`: resolves the PcDesc
    /// (which carries the deopt bci) and returns `(pc_desc, live_map)`.
    /// The live map is exact-bci first, then nearest-lower fallback.
    pub fn deopt_info_at_pc(&self, pc: usize) -> Option<(PcDesc, Option<DeoptLiveMap>)> {
        let pd = self.pc_desc_near(pc)?;
        let bci = pd.bci();
        let map = if bci != PcDesc::UNKNOWN_BCI {
            self.live_map_for_bci(bci)
                .or_else(|| self.live_map_near_bci(bci))
        } else {
            None
        };
        Some((pd, map))
    }

    /// Conservative live map covering the whole frame when the compiler
    /// produced none: every local slot and a caller-provided operand
    /// depth are treated as live references.
    pub fn conservative_live_map(
        &self,
        bci: i32,
        local_count: usize,
        stack_depth: usize,
    ) -> DeoptLiveMap {
        DeoptLiveMap::conservative(bci, local_count, stack_depth)
    }

    /// Total encoded size of the deopt live maps (metadata accounting).
    pub fn live_maps_size(&self) -> usize {
        self.shared
            .live_maps
            .lock()
            .unwrap()
            .iter()
            .map(DeoptLiveMap::encoded_size)
            .sum()
    }

    /// `nmethod::print_on` analogue: header line plus dependency, pc-desc,
    /// scope, handler-table and live-map summaries.
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
        let _ = writeln!(
            f,
            "  entry_point={} verified={} osr_entry={} frame_size={}",
            self.entry_point,
            self.verified_entry_point,
            self.osr_entry_point,
            self.frame_size(),
        );
        if self.sections.consts_begin != 0 || self.sections.metadata_begin != 0 {
            let _ = writeln!(
                f,
                "  consts=[{}..{}) metadata=[{}..{}) scopes_pcs=[{}..{}) deps_size={}",
                self.sections.consts_begin,
                self.sections.consts_end,
                self.sections.metadata_begin,
                self.sections.metadata_end,
                self.sections.scopes_pcs_begin,
                self.sections.scopes_pcs_end,
                self.dependencies_size(),
            );
        }
        if let Some(reason) = self.invalidation_reason() {
            let _ = writeln!(f, "  invalidation: {}", reason.as_str());
        }
        let _ = writeln!(f, "  deopt_status={:?}", self.deoptimization_status());
        if self.is_marked_for_deoptimization() {
            let _ = writeln!(f, "  marked for deoptimization");
        }
        let flags = [
            ("monitors", self.has_monitors()),
            ("scoped_access", self.has_scoped_access()),
            ("wide_vectors", self.has_wide_vectors()),
            ("method_handle_invokes", self.has_method_handle_invokes()),
            ("unsafe_access", self.has_unsafe_access()),
        ];
        let active: Vec<&str> = flags.iter().filter(|(_, v)| *v).map(|(n, _)| *n).collect();
        if !active.is_empty() {
            let _ = writeln!(f, "  flags: {}", active.join(", "));
        }
        let _ = writeln!(
            f,
            "  pc_descs={} scopes={} oop_maps={} live_maps={}",
            self.pc_descs.len(),
            self.scopes.len(),
            self.shared.oop_maps.lock().unwrap().size(),
            self.shared.live_maps.lock().unwrap().len(),
        );
        // Compact pc-desc listing (HotSpot prints each PcDesc under
        // PrintNMethods/PrintCompilation).
        for pd in self.pc_descs.iter().take(64) {
            let _ = writeln!(
                f,
                "    pc_desc offset={} scope_off={} obj_off={} bci={} flags=0x{:x}",
                pd.pc_offset(),
                pd.scope_decode_offset(),
                pd.obj_decode_offset(),
                pd.bci(),
                pd.flags(),
            );
        }
        if self.pc_descs.len() > 64 {
            let _ = writeln!(f, "    ... {} more pc_descs", self.pc_descs.len() - 64);
        }
        self.print_dependencies_on(f);
        // Scope summary: decode the recorded in-memory scopes.
        for s in self.scopes.iter().take(16) {
            s.print_on(f);
        }
        if self.scopes.len() > 16 {
            let _ = writeln!(f, "    ... {} more scopes", self.scopes.len() - 16);
        }
        let ht = self.shared.handler_table.lock().unwrap();
        if !ht.is_empty() {
            let _ = writeln!(f, "  handler_table:");
            ht.print_on(f, Some(self.code_begin()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::compiler::compilerDefinitions::CompLevel;

    fn test_nm() -> NMethod {
        NMethod::with_entry_points(
            "holder".to_string(),
            "foo".to_string(),
            "()V".to_string(),
            CompLevel::FullProfile,
            -1,
            0x1000,
            256,
            0x1000,
            0x1000,
            0x1000,
            vec![
                PcDesc::new(PcDesc::LOWER_OFFSET_LIMIT, 0, 0),
                PcDesc::with_bci(10, 0, 0, PcDesc::PCDESC_REEXECUTE, 4),
                PcDesc::with_bci(20, 0, 0, 0, 8),
                PcDesc::new(PcDesc::UPPER_OFFSET_LIMIT, 0, 0),
            ],
            Vec::new(),
        )
    }

    #[test]
    fn live_maps_sorted_and_lookup() {
        let nm = test_nm();
        nm.set_live_maps(vec![
            DeoptLiveMap::from_ref_slots(8, vec![0, 2], vec![1]),
            DeoptLiveMap::from_ref_slots(4, vec![0], vec![]),
        ]);
        let maps = nm.live_maps();
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].bci(), 4);
        assert_eq!(maps[1].bci(), 8);
        let m = nm.live_map_for_bci(8).unwrap();
        assert!(m.is_live_local_ref(2));
        assert!(!m.is_live_local_ref(1));
        assert!(m.is_live_stack_ref(1));
        // near lookup falls back to greatest bci <= target
        let near = nm.live_map_near_bci(6).unwrap();
        assert_eq!(near.bci(), 4);
        assert!(nm.live_map_near_bci(3).is_none());
    }

    #[test]
    fn deopt_info_at_pc_uses_pcdesc_bci() {
        let nm = test_nm();
        nm.set_live_maps(vec![DeoptLiveMap::from_ref_slots(4, vec![0], vec![])]);
        // pc 0x1000+15 -> nearest pc_desc is offset 10 -> bci 4
        let (pd, map) = nm.deopt_info_at_pc(0x1000 + 15).unwrap();
        assert_eq!(pd.bci(), 4);
        assert!(map.is_some());
        // pc before first real desc still resolves to sentinel (bci unknown)
        let (pd2, map2) = nm.deopt_info_at_pc(0x1000).unwrap();
        assert_eq!(pd2.bci(), PcDesc::UNKNOWN_BCI);
        assert!(map2.is_none());
    }

    #[test]
    fn conservative_map_covers_frame() {
        let m = DeoptLiveMap::conservative(3, 4, 2);
        assert_eq!(m.live_ref_count(), 6);
        assert!(m.is_live_local_ref(3));
        assert!(m.is_live_stack_ref(1));
        assert!(!m.is_live_stack_ref(2));
    }

    #[test]
    fn state_transitions() {
        let nm = test_nm();
        assert!(nm.is_in_use());
        assert!(nm.make_not_entrant_with_reason(InvalidationReason::MarkedForDeoptimization));
        assert!(nm.is_not_entrant());
        nm.make_not_entrant(); // already not in use: no-op
        nm.mark_for_deoptimization();
        assert!(nm.is_marked_for_deoptimization());
        nm.make_zombie();
        assert!(nm.is_zombie());
        assert!(nm.is_unloaded());
        nm.deoptimization_done();
        assert!(nm.is_deoptimization_done());
    }

    #[test]
    fn frame_size_accessor() {
        let nm = test_nm();
        assert_eq!(nm.frame_size(), 0);
        nm.set_frame_size(48);
        assert_eq!(nm.frame_size(), 48);
    }

    #[test]
    fn deopt_status_transitions() {
        let nm = test_nm();
        assert_eq!(nm.deoptimization_status(), DeoptimizationStatus::NotMarked);
        assert!(nm.update_recompile_counts());
        assert!(nm.mark_for_deoptimization());
        // Second mark is a no-op (already past not_marked).
        assert!(!nm.mark_for_deoptimization());
        assert!(nm.is_marked_for_deoptimization());
        assert!(!nm.has_been_deoptimized());
        nm.deoptimization_done();
        assert!(nm.has_been_deoptimized());
        assert!(nm.is_marked_for_deoptimization());
        assert!(!nm.update_recompile_counts());
        // noupdate variant
        let nm2 = test_nm();
        assert!(nm2.mark_for_deoptimization_noupdate());
        assert!(!nm2.update_recompile_counts());
    }

    #[test]
    fn sections_and_dependencies() {
        let mut nm = test_nm();
        nm.set_sections(NMethodSections {
            consts_begin: 0x800,
            consts_end: 0x900,
            metadata_begin: 0x900,
            metadata_end: 0xA00,
            scopes_pcs_begin: 0x920,
            scopes_pcs_end: 0x980,
        });
        assert_eq!(nm.consts_size(), 0x100);
        assert!(nm.consts_contains(0x850));
        assert_eq!(nm.metadata_size(), 0x100);
        assert_eq!(nm.scopes_pcs_size(), 0x60);
        assert_eq!(nm.dependencies_begin(), 0x900);
        nm.add_dependency(crate::share::code::dependencies::Dependency::new(
            crate::share::code::dependencies::DepType::LeafType,
            vec![crate::share::code::dependencies::DependencyArgument::Klass(
                "pkg/Base".to_string(),
            )],
        ));
        assert!(nm.has_dependencies());
        assert!(nm.dependencies_affected_by("pkg/Base", |a, b| a == b));
        assert!(nm.dependencies_affected_by("pkg/Sub", |_, b| b == "pkg/Base"));
        assert!(!nm.dependencies_affected_by("pkg/Other", |_, _| false));
        let deps = nm.dependencies();
        assert_eq!(deps.len(), 1);
        nm.flush_dependencies();
        assert!(!nm.has_dependencies());
        assert!(nm.has_flushed_dependencies());
        // Relocation shifts absolute section addresses.
        nm.set_sections(NMethodSections {
            consts_begin: 0x800,
            consts_end: 0x900,
            metadata_begin: 0x900,
            metadata_end: 0xA00,
            scopes_pcs_begin: 0,
            scopes_pcs_end: 0,
        });
        nm.set_code_location(0x2000, 256);
        // delta = 0x2000 - 0x1000 = 0x1000; 0x800 + 0x1000 = 0x1800.
        assert_eq!(nm.consts_begin(), 0x1800);
        assert_eq!(nm.metadata_begin(), 0x1900);
    }

    #[test]
    fn verify_legal_inline_cache_ok_and_bad() {
        let nm = test_nm();
        // pc_descs offsets are within code_size=256; scope offsets are 0
        // (serialized_null) so verification passes.
        assert!(nm.verify_legal_inline_cache().is_ok());
        // Corrupt: unsorted pc_descs.
        let mut nm2 = test_nm();
        nm2.pc_descs = vec![
            PcDesc::new(PcDesc::LOWER_OFFSET_LIMIT, 0, 0),
            PcDesc::with_bci(100, 0, 0, 0, 1),
            PcDesc::with_bci(50, 0, 0, 0, 2),
            PcDesc::new(PcDesc::UPPER_OFFSET_LIMIT, 0, 0),
        ];
        assert!(nm2.verify_legal_inline_cache().is_err());
        // Out-of-range handler entry.
        let nm3 = test_nm();
        nm3.set_pc_handlers({
            let mut t = PcRangeHandlerTable::new();
            t.add(0, 300, 10, None);
            t
        });
        assert!(nm3.verify_legal_inline_cache().is_err());
    }

    #[test]
    fn header_flags() {
        let nm = test_nm();
        assert!(!nm.has_monitors());
        nm.set_has_monitors(true);
        nm.set_has_method_handle_invokes(true);
        assert!(nm.has_monitors());
        assert!(nm.has_method_handle_invokes());
        assert!(nm.is_entrant());
        assert!(nm.can_be_deoptimized());
        let osr_nm = NMethod::with_entry_points(
            "O".to_string(),
            "osr".to_string(),
            "()V".to_string(),
            CompLevel::Simple,
            7,
            0,
            0,
            0,
            0,
            0,
            vec![],
            vec![],
        );
        assert_eq!(osr_nm.osr_entry_bci(), 7);
        assert!(osr_nm.is_osr_method());
        let mut s = String::new();
        nm.print_on(&mut s);
        assert!(s.contains("nmethod"));
        assert!(s.contains("dependencies"));
    }
}
