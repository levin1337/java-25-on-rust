/*
 * Port of OpenJDK 25 HotSpot Deoptimization (share/runtime/deoptimization.hpp / deoptimization.cpp)
 *
 * Deoptimization translates compiled code frames back to interpreter frames
 * when speculative optimizations are invalidated or unexpected traps occur.
 *
 * This module owns the *metadata* half of that protocol:
 *  - DeoptReason / DeoptAction enums with the exact HotSpot numbering,
 *  - the trap_request bit packing used by uncommon-trap stubs and the
 *    compiled-code trap bits,
 *  - the MethodData trap_state lattice (reason + recompiled bit),
 *  - the reason classification predicates (recorded-per-bytecode, speculate,
 *    null/class check helpers) used by profile-trap accounting,
 *  - the UnrollBlock / UnpackType descriptions consumed by the unpack stub,
 *  - the ScopeValue/MonitorValue/FrameDesc data model that describes the
 *    interpreter frames a compiled frame decomposes into.
 *
 * Actual frame materialization (register restore, stack rewrite, bci resume)
 * is performed by the jitruntime trampolines and the interpreter's
 * `execute_labeled` re-entry path; this file provides the reason codes and
 * descriptors they consult.
 */

use std::fmt;

// ---------------------------------------------------------------------------
// DeoptReason / DeoptAction — must stay in sync with
// deoptimization.hpp `enum DeoptReason` / `enum DeoptAction` and the
// `_trap_reason_name` / `_trap_action_name` tables in deoptimization.cpp.
// ---------------------------------------------------------------------------

/// Why the deoptimization happened.  Values match HotSpot's
/// `Deoptimization::DeoptReason` ordinals exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum DeoptReason {
    /// Presence of several reasons.
    ReasonMany = -1,
    /// Absence of a relevant deopt.
    ReasonNone = 0,
    /// Saw unexpected null or zero divisor (@bci).
    ReasonNullCheck = 1,
    /// Saw unexpected non-null or non-zero (@bci).
    ReasonNullAssert = 2,
    /// Saw unexpected array index (@bci).
    ReasonRangeCheck = 3,
    /// Saw unexpected object class (@bci).
    ReasonClassCheck = 4,
    /// Saw unexpected array class (aastore @bci).
    ReasonArrayCheck = 5,
    /// Saw unexpected operand to intrinsic (@bci).
    ReasonIntrinsic = 6,
    /// Saw unexpected object class in bimorphic inlining (@bci).
    ReasonBimorphic = 7,
    /// Compiler generated predicate moved from a frequent loop branch failed.
    ReasonProfilePredicate = 8,
    /// Compiler generated (speculative) auto-vectorization checks failed.
    ReasonAutoVectorizationCheck = 9,
    /// Unloaded class or constant-pool entry.
    ReasonUnloaded = 10,
    /// Bad class state (uninitialized).
    ReasonUninitialized = 11,
    /// Class has been fully initialized.
    ReasonInitialized = 12,
    /// Code is not reached, compiler.
    ReasonUnreached = 13,
    /// Arbitrary compiler limitation.
    ReasonUnhandled = 14,
    /// Arbitrary runtime constraint violated.
    ReasonConstraint = 15,
    /// A null_check due to division by zero.
    ReasonDiv0Check = 16,
    /// Nmethod too old; tier threshold reached.
    ReasonAge = 17,
    /// Compiler generated predicate failed.
    ReasonPredicate = 18,
    /// Compiler generated loop limits check failed.
    ReasonLoopLimitCheck = 19,
    /// Saw unexpected object class from type speculation.
    ReasonSpeculateClassCheck = 20,
    /// Saw unexpected null from type speculation.
    ReasonSpeculateNullCheck = 21,
    /// Saw unexpected null from type speculation.
    ReasonSpeculateNullAssert = 22,
    /// A branch predicted always false was taken.
    ReasonUnstableIf = 23,
    /// Fused two ifs that had each one untaken branch; one is now taken.
    ReasonUnstableFusedIf = 24,
    /// Receiver subtype check failed.
    ReasonReceiverConstraint = 25,
    /// JVMCI: optimistic assumption about aliasing failed.
    ReasonAliasing = 26,
    /// JVMCI: explicit transferToInterpreter().
    ReasonTransferToInterpreter = 27,
    /// JVMCI: not compiled exception handler.
    ReasonNotCompiledExceptionHandler = 28,
    /// JVMCI: unresolved.
    ReasonUnresolved = 29,
    /// JVMCI: jsr mismatch.
    ReasonJsrMismatch = 30,
    /// Age of the code has reached the limit.  In C++ this sits at
    /// `Reason_TRAP_HISTORY_LENGTH`; the enum cannot carry two variants
    /// with the same discriminant, so the marker is exposed as a const.
    ReasonTenured = 31,
    /// One past the last reason.
    ReasonLimit = 32,
}

/// `Reason_TRAP_HISTORY_LENGTH`: reasons strictly below this value are
/// counted in the trap-history histogram; `ReasonTenured` (same value)
/// is accounted separately.
pub const REASON_TRAP_HISTORY_LENGTH: i32 = DeoptReason::ReasonTenured as i32;

impl DeoptReason {
    /// `trap_reason_name` table, in `_trap_reason_name` order.
    pub fn name(self) -> &'static str {
        match self {
            Self::ReasonMany => "many",
            Self::ReasonNone => "none",
            Self::ReasonNullCheck => "null_check",
            Self::ReasonNullAssert => "null_assert",
            Self::ReasonRangeCheck => "range_check",
            Self::ReasonClassCheck => "class_check",
            Self::ReasonArrayCheck => "array_check",
            Self::ReasonIntrinsic => "intrinsic",
            Self::ReasonBimorphic => "bimorphic",
            Self::ReasonProfilePredicate => "profile_predicate",
            Self::ReasonAutoVectorizationCheck => "auto_vectorization_check",
            Self::ReasonUnloaded => "unloaded",
            Self::ReasonUninitialized => "uninitialized",
            Self::ReasonInitialized => "initialized",
            Self::ReasonUnreached => "unreached",
            Self::ReasonUnhandled => "unhandled",
            Self::ReasonConstraint => "constraint",
            Self::ReasonDiv0Check => "div0_check",
            Self::ReasonAge => "age",
            Self::ReasonPredicate => "predicate",
            Self::ReasonLoopLimitCheck => "loop_limit_check",
            Self::ReasonSpeculateClassCheck => "speculate_class_check",
            Self::ReasonSpeculateNullCheck => "speculate_null_check",
            Self::ReasonSpeculateNullAssert => "speculate_null_assert",
            Self::ReasonUnstableIf => "unstable_if",
            Self::ReasonUnstableFusedIf => "unstable_fused_if",
            Self::ReasonReceiverConstraint => "receiver_constraint",
            Self::ReasonAliasing => "aliasing",
            Self::ReasonTransferToInterpreter => "transfer_to_interpreter",
            Self::ReasonNotCompiledExceptionHandler => "not_compiled_exception_handler",
            Self::ReasonUnresolved => "unresolved",
            Self::ReasonJsrMismatch => "jsr_mismatch",
            Self::ReasonTenured => "tenured",
            Self::ReasonLimit => "reason_limit",
        }
    }

    /// `trap_reason_name(int)`: falls back to `reasonN` for out-of-range
    /// values, like the C++ static buffer formatting.
    pub fn name_of(raw: i32) -> String {
        if raw == DeoptReason::ReasonMany as i32 {
            return "many".to_string();
        }
        match DeoptReason::try_from(raw) {
            Ok(reason) if raw >= 0 && (raw as i32) < DeoptReason::ReasonLimit as i32 => {
                reason.name().to_string()
            }
            _ => format!("reason{raw}"),
        }
    }

    pub fn from_i32(raw: i32) -> Option<Self> {
        match raw {
            -1 => Some(Self::ReasonMany),
            0 => Some(Self::ReasonNone),
            1 => Some(Self::ReasonNullCheck),
            2 => Some(Self::ReasonNullAssert),
            3 => Some(Self::ReasonRangeCheck),
            4 => Some(Self::ReasonClassCheck),
            5 => Some(Self::ReasonArrayCheck),
            6 => Some(Self::ReasonIntrinsic),
            7 => Some(Self::ReasonBimorphic),
            8 => Some(Self::ReasonProfilePredicate),
            9 => Some(Self::ReasonAutoVectorizationCheck),
            10 => Some(Self::ReasonUnloaded),
            11 => Some(Self::ReasonUninitialized),
            12 => Some(Self::ReasonInitialized),
            13 => Some(Self::ReasonUnreached),
            14 => Some(Self::ReasonUnhandled),
            15 => Some(Self::ReasonConstraint),
            16 => Some(Self::ReasonDiv0Check),
            17 => Some(Self::ReasonAge),
            18 => Some(Self::ReasonPredicate),
            19 => Some(Self::ReasonLoopLimitCheck),
            20 => Some(Self::ReasonSpeculateClassCheck),
            21 => Some(Self::ReasonSpeculateNullCheck),
            22 => Some(Self::ReasonSpeculateNullAssert),
            23 => Some(Self::ReasonUnstableIf),
            24 => Some(Self::ReasonUnstableFusedIf),
            25 => Some(Self::ReasonReceiverConstraint),
            26 => Some(Self::ReasonAliasing),
            27 => Some(Self::ReasonTransferToInterpreter),
            28 => Some(Self::ReasonNotCompiledExceptionHandler),
            29 => Some(Self::ReasonUnresolved),
            30 => Some(Self::ReasonJsrMismatch),
            31 => Some(Self::ReasonTenured),
            _ => None,
        }
    }

    /// `Deoptimization::reason_is_recorded_per_bytecode`: reasons
    /// `Reason_null_check..=Reason_profile_predicate` are stored in the
    /// per-bytecode trap bits of DataLayout.
    pub fn is_recorded_per_bytecode(self) -> bool {
        (self as i32) > DeoptReason::ReasonNone as i32
            && (self as i32) <= DeoptReason::ReasonProfilePredicate as i32
    }

    /// `Deoptimization::reason_recorded_per_bytecode_if_any`: maps
    /// method-level reasons onto the bytecode reason they correspond to.
    pub fn recorded_per_bytecode_if_any(self) -> DeoptReason {
        if self.is_recorded_per_bytecode() {
            return self;
        }
        match self {
            Self::ReasonDiv0Check => Self::ReasonNullCheck,
            Self::ReasonSpeculateClassCheck => Self::ReasonClassCheck,
            Self::ReasonSpeculateNullCheck => Self::ReasonNullCheck,
            Self::ReasonSpeculateNullAssert => Self::ReasonNullAssert,
            Self::ReasonUnstableIf => Self::ReasonIntrinsic,
            Self::ReasonUnstableFusedIf => Self::ReasonRangeCheck,
            _ => Self::ReasonNone,
        }
    }

    /// `Deoptimization::reason_is_speculate`.
    pub fn is_speculate(self) -> bool {
        matches!(
            self,
            Self::ReasonSpeculateClassCheck
                | Self::ReasonSpeculateNullCheck
                | Self::ReasonSpeculateNullAssert
        )
    }

    /// `Deoptimization::reason_null_check(speculative)`.
    pub fn null_check(speculative: bool) -> Self {
        if speculative {
            Self::ReasonSpeculateNullCheck
        } else {
            Self::ReasonNullCheck
        }
    }

    /// `Deoptimization::reason_class_check(speculative)`.
    pub fn class_check(speculative: bool) -> Self {
        if speculative {
            Self::ReasonSpeculateClassCheck
        } else {
            Self::ReasonClassCheck
        }
    }

    /// `Deoptimization::reason_null_assert(speculative)`.
    pub fn null_assert(speculative: bool) -> Self {
        if speculative {
            Self::ReasonSpeculateNullAssert
        } else {
            Self::ReasonNullAssert
        }
    }

    /// `Deoptimization::per_method_trap_limit` thresholds (flag values
    /// `PerMethodTrapLimit` / `PerMethodSpecTrapLimit`, HotSpot defaults
    /// 4000 / 500).
    pub fn per_method_trap_limit(self) -> u32 {
        if self.is_speculate() {
            500
        } else {
            4000
        }
    }
}

impl TryFrom<i32> for DeoptReason {
    type Error = ();
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::from_i32(value).ok_or(())
    }
}

/// What action must be taken by the runtime.  Values match
/// `Deoptimization::DeoptAction` ordinals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum DeoptAction {
    /// Just interpret, do not invalidate the nmethod.
    ActionNone = 0,
    /// Recompile the nmethod; need not invalidate.
    ActionMaybeRecompile = 1,
    /// Invalidate the nmethod, reset IC, maybe recompile.
    ActionReinterpret = 2,
    /// Invalidate the nmethod, recompile (probably).
    ActionMakeNotEntrant = 3,
    /// Invalidate the nmethod and do not compile.
    ActionMakeNotCompilable = 4,
    /// One past the last action.
    ActionLimit = 5,
}

impl DeoptAction {
    /// `_trap_action_name` table.
    pub fn name(self) -> &'static str {
        match self {
            Self::ActionNone => "none",
            Self::ActionMaybeRecompile => "maybe_recompile",
            Self::ActionReinterpret => "reinterpret",
            Self::ActionMakeNotEntrant => "make_not_entrant",
            Self::ActionMakeNotCompilable => "make_not_compilable",
            Self::ActionLimit => "limit",
        }
    }

    pub fn from_i32(raw: i32) -> Option<Self> {
        match raw {
            0 => Some(Self::ActionNone),
            1 => Some(Self::ActionMaybeRecompile),
            2 => Some(Self::ActionReinterpret),
            3 => Some(Self::ActionMakeNotEntrant),
            4 => Some(Self::ActionMakeNotCompilable),
            _ => None,
        }
    }

    /// Actions that retire or dirty the nmethod.
    pub fn invalidates_nmethod(self) -> bool {
        matches!(
            self,
            Self::ActionReinterpret
                | Self::ActionMakeNotEntrant
                | Self::ActionMakeNotCompilable
        )
    }
}

// ---------------------------------------------------------------------------
// Trap request encoding (deoptimization.hpp constants).
// ---------------------------------------------------------------------------

const ACTION_BITS: u32 = 3;
const REASON_BITS: u32 = 5;
const DEBUG_ID_BITS: u32 = 23;
const ACTION_SHIFT: u32 = 0;
const REASON_SHIFT: u32 = ACTION_SHIFT + ACTION_BITS;
const DEBUG_ID_SHIFT: u32 = REASON_SHIFT + REASON_BITS;

const ACTION_MASK: i32 = (1 << ACTION_BITS) - 1;
const REASON_MASK: i32 = (1 << REASON_BITS) - 1;
const DEBUG_ID_MASK: i32 = (1 << DEBUG_ID_BITS) - 1;

/// How the interpreter should resume the deoptimized top frame.
/// Matches `Deoptimization::UnpackType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum UnpackType {
    /// Normal deoptimization; use the pc computed by unpack_vframe_on_stack.
    UnpackDeopt = 0,
    /// Exception is pending.
    UnpackException = 1,
    /// Redo the last byte code (C2 uncommon traps).
    UnpackUncommonTrap = 2,
    /// Reexecute bytecode (C1 uncommon traps / reexecute flag).
    UnpackReexecute = 3,
    /// Not deoptimizing the frame; JVMTI realloc/relock only.
    UnpackNone = 4,
    /// One past the last unpack type.
    UnpackLimit = 5,
}

impl UnpackType {
    pub fn from_i32(raw: i32) -> Option<Self> {
        match raw {
            0 => Some(Self::UnpackDeopt),
            1 => Some(Self::UnpackException),
            2 => Some(Self::UnpackUncommonTrap),
            3 => Some(Self::UnpackReexecute),
            4 => Some(Self::UnpackNone),
            _ => None,
        }
    }

    /// Whether execution resumes by re-executing the current bytecode
    /// (uncommon trap / explicit reexecute).
    pub fn reexecutes_bytecode(self) -> bool {
        matches!(self, Self::UnpackUncommonTrap | Self::UnpackReexecute)
    }
}

/// An uncommon-trap request carried through the stub/trampoline layer:
/// the reason+action pair plus the optional constant-pool index used
/// for unloaded-entry traps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UncommonTrap {
    /// Raw `trap_request` value (positive = cp index, negative = encoded
    /// reason/action/debug-id triple).
    pub trap_request: i32,
    /// `UnpackType` the fetch phase selected.
    pub exec_mode: UnpackType,
}

impl UncommonTrap {
    pub fn reason(&self) -> DeoptReason {
        Deoptimization::trap_request_reason(self.trap_request)
    }

    pub fn action(&self) -> DeoptAction {
        Deoptimization::trap_request_action(self.trap_request)
    }

    /// `trap_request_index`: >= 0 when the request is a constant-pool
    /// index (unloaded entry), -1 when it is an encoded reason triple.
    pub fn index(&self) -> i32 {
        Deoptimization::trap_request_index(self.trap_request)
    }

    /// `format_trap_request`: `reason='foo' action='bar' index='123'`
    /// suitable for both XML and tty output.
    pub fn describe(&self) -> String {
        format!(
            "reason='{}' action='{}' index='{}'",
            self.reason().name(),
            self.action().name(),
            self.index()
        )
    }
}

/// `Deoptimization::UnrollBlock`: continuation information produced by
/// `fetch_unroll_info` and consumed by the generated unpack stub.  Field
/// layout mirrors the C++ class one-for-one.
#[derive(Debug, Clone)]
pub struct UnrollBlock {
    /// Size, in bytes, of the current deoptimized frame.
    pub size_of_deoptimized_frame: i32,
    /// Adjustment, in bytes, to the caller's SP by the initial
    /// interpreted frame.
    pub caller_adjustment: i32,
    /// Number of frames to unroll.
    pub number_of_frames: i32,
    /// Total of number*sizes frames.
    pub total_frame_sizes: i32,
    /// Array of frame sizes, in bytes, for unrolling the stack.
    pub frame_sizes: Vec<i32>,
    /// Array of frame pcs, in bytes, for unrolling the stack.
    pub frame_pcs: Vec<u64>,
    /// Block for storing callee-saved registers.
    pub register_block: Vec<u64>,
    /// Tells whether we have to restore a double or long return value.
    pub return_type: ReturnKind,
    /// Platform dependent data for the sender frame.
    pub initial_info: i64,
    /// The number of actual arguments at the interpreted caller of the
    /// deoptimized frame.
    pub caller_actual_parameters: i32,
    /// Exec mode that can be changed during fetch_unroll_info.
    pub unpack_kind: UnpackType,
    /// Temps used during the unpacking phase (C++ x86 comment).
    pub counter_temp: i64,
    pub sender_sp_temp: i64,
}

/// `BasicType` of a call return value carried through the unpack stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnKind {
    Void,
    Int,
    Long,
    Float,
    Double,
    Object,
}

impl UnrollBlock {
    /// C++ constructor signature.
    pub fn new(
        size_of_deoptimized_frame: i32,
        caller_adjustment: i32,
        caller_actual_parameters: i32,
        number_of_frames: i32,
        frame_sizes: Vec<i32>,
        frame_pcs: Vec<u64>,
        return_type: ReturnKind,
        unpack_kind: UnpackType,
    ) -> Self {
        let total_frame_sizes = frame_sizes.iter().map(|s| *s as i64).sum::<i64>() as i32;
        Self {
            size_of_deoptimized_frame,
            caller_adjustment,
            number_of_frames,
            total_frame_sizes,
            frame_sizes,
            frame_pcs,
            register_block: Vec::new(),
            return_type,
            initial_info: 0,
            caller_actual_parameters,
            unpack_kind,
            counter_temp: 0,
            sender_sp_temp: 0,
        }
    }

    /// `size_of_frames`: total size of all frames to materialize.
    pub fn size_of_frames(&self) -> i32 {
        self.frame_sizes.iter().sum()
    }

    /// `set_initial_info`.
    pub fn set_initial_info(&mut self, info: i64) {
        self.initial_info = info;
    }
}

// ---------------------------------------------------------------------------
// MethodData trap_state lattice (deoptimization.cpp
// trap_state_reason/add_reason/set_recompiled).
// ---------------------------------------------------------------------------

/// `DataLayout::trap_mask`: the MDO trap_state field is 32 bits wide;
/// the high bit is the recompiled flag, the low 31 bits encode the
/// reason lattice.
const DS_REASON_MASK: i32 = i32::MAX;
const DS_RECOMPILE_BIT: i32 = i32::MIN;

/// Decodes the per-bytecode `trap_state` recorded in MethodData.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrapState {
    /// Decoded reason (`Reason_many` when conflicting reasons were seen).
    pub reason: DeoptReason,
    /// Whether a deopt here already triggered a recompilation.
    pub recompiled: bool,
}

impl TrapState {
    /// `Deoptimization::trap_state_reason`.
    pub fn reason(trap_state: i32) -> DeoptReason {
        let recompile_bit = trap_state & DS_RECOMPILE_BIT;
        let state = trap_state.wrapping_sub(recompile_bit);
        if state == DS_REASON_MASK {
            DeoptReason::ReasonMany
        } else {
            DeoptReason::from_i32(state).unwrap_or(DeoptReason::ReasonNone)
        }
    }

    /// `Deoptimization::trap_state_has_reason`.
    pub fn has_reason(trap_state: i32, reason: i32) -> bool {
        let recompile_bit = trap_state & DS_RECOMPILE_BIT;
        let state = trap_state.wrapping_sub(recompile_bit);
        if state == DS_REASON_MASK {
            true
        } else if state == reason {
            true
        } else if state == 0 {
            reason == DeoptReason::ReasonNone as i32
        } else {
            false
        }
    }

    /// `Deoptimization::trap_state_add_reason`: folds a newly observed
    /// bytecode reason into the stored trap_state.
    pub fn add_reason(trap_state: i32, reason: DeoptReason) -> i32 {
        let recompile_bit = trap_state & DS_RECOMPILE_BIT;
        let state = trap_state.wrapping_sub(recompile_bit);
        let new_state = if state == DS_REASON_MASK {
            DS_REASON_MASK
        } else if state == reason as i32 {
            state
        } else if state == 0 {
            reason as i32
        } else {
            DS_REASON_MASK
        };
        new_state.wrapping_add(recompile_bit)
    }

    /// `Deoptimization::trap_state_is_recompiled`.
    pub fn is_recompiled(trap_state: i32) -> bool {
        (trap_state & DS_RECOMPILE_BIT) != 0
    }

    /// `Deoptimization::trap_state_set_recompiled`.
    pub fn set_recompiled(trap_state: i32, z: bool) -> i32 {
        if z {
            trap_state | DS_RECOMPILE_BIT
        } else {
            trap_state & !DS_RECOMPILE_BIT
        }
    }

    /// `Deoptimization::format_trap_state`.
    pub fn format(trap_state: i32) -> String {
        let reason = Self::reason(trap_state);
        let recomp_flag = Self::is_recompiled(trap_state);
        if !(reason.is_recorded_per_bytecode() || reason == DeoptReason::ReasonMany) {
            return format!("#{trap_state}");
        }
        let mut decoded_state = 0;
        if reason.is_recorded_per_bytecode() || reason == DeoptReason::ReasonMany {
            decoded_state = Self::add_reason(decoded_state, reason);
        }
        if decoded_state != trap_state {
            return format!("#{trap_state}");
        }
        format!(
            "{}{}",
            reason.name(),
            if recomp_flag { " recompiled" } else { "" }
        )
    }
}

// ---------------------------------------------------------------------------
// Deoptimization facade (Deoptimization:: static methods).
// ---------------------------------------------------------------------------

pub struct Deoptimization;

impl Deoptimization {
    // -- trap_request coding -------------------------------------------------

    /// `Deoptimization::make_trap_request`.  `index >= 0` produces the
    /// positive constant-pool-index form used for unloaded entries;
    /// otherwise the complemented reason/action pair is stored (the
    /// debug-id field stays zero, matching the C++ expression).
    pub fn make_trap_request(reason: DeoptReason, action: DeoptAction, index: i32) -> i32 {
        if index != -1 {
            return index;
        }
        !(((reason as i32) << REASON_SHIFT) + ((action as i32) << ACTION_SHIFT))
    }

    /// `Deoptimization::trap_request_reason`.
    pub fn trap_request_reason(trap_request: i32) -> DeoptReason {
        if trap_request < 0 {
            let reason = (!trap_request >> REASON_SHIFT) & REASON_MASK;
            DeoptReason::from_i32(reason).unwrap_or(DeoptReason::ReasonNone)
        } else {
            DeoptReason::ReasonUnloaded
        }
    }

    /// `Deoptimization::trap_request_action`.
    pub fn trap_request_action(trap_request: i32) -> DeoptAction {
        if trap_request < 0 {
            let action = (!trap_request >> ACTION_SHIFT) & ACTION_MASK;
            DeoptAction::from_i32(action).unwrap_or(DeoptAction::ActionNone)
        } else {
            // `_unloaded_action` — the standard action for unloaded CP
            // entries is Action_reinterpret.
            DeoptAction::ActionReinterpret
        }
    }

    /// `Deoptimization::trap_request_debug_id`.
    pub fn trap_request_debug_id(trap_request: i32) -> i32 {
        if trap_request < 0 {
            (!trap_request >> DEBUG_ID_SHIFT) & DEBUG_ID_MASK
        } else {
            0
        }
    }

    /// `Deoptimization::trap_request_index`.
    pub fn trap_request_index(trap_request: i32) -> i32 {
        if trap_request < 0 {
            -1
        } else {
            trap_request
        }
    }

    /// `Deoptimization::trap_request_name`.
    pub fn trap_request_name(trap_request: i32) -> String {
        format!(
            "trap[reason={}, action={}]",
            Self::trap_request_reason(trap_request).name(),
            Self::trap_request_action(trap_request).name()
        )
    }

    /// `Deoptimization::format_trap_request`.
    pub fn format_trap_request(trap_request: i32) -> String {
        let trap = UncommonTrap {
            trap_request,
            exec_mode: UnpackType::UnpackDeopt,
        };
        trap.describe()
    }

    // -- reason classification ------------------------------------------------

    /// `Deoptimization::reason_is_recorded_per_bytecode`.
    pub fn reason_is_recorded_per_bytecode(reason: DeoptReason) -> bool {
        reason.is_recorded_per_bytecode()
    }

    /// `Deoptimization::reason_recorded_per_bytecode_if_any`.
    pub fn reason_recorded_per_bytecode_if_any(reason: DeoptReason) -> DeoptReason {
        reason.recorded_per_bytecode_if_any()
    }

    /// `Deoptimization::reason_is_speculate`.
    pub fn reason_is_speculate(reason: DeoptReason) -> bool {
        reason.is_speculate()
    }

    /// `Deoptimization::reason_null_check(speculative)`.
    pub fn reason_null_check(speculative: bool) -> DeoptReason {
        DeoptReason::null_check(speculative)
    }

    /// `Deoptimization::reason_class_check(speculative)`.
    pub fn reason_class_check(speculative: bool) -> DeoptReason {
        DeoptReason::class_check(speculative)
    }

    /// `Deoptimization::reason_null_assert(speculative)`.
    pub fn reason_null_assert(speculative: bool) -> DeoptReason {
        DeoptReason::null_assert(speculative)
    }

    /// `Deoptimization::per_method_trap_limit`.
    pub fn per_method_trap_limit(reason: DeoptReason) -> u32 {
        reason.per_method_trap_limit()
    }

    /// `Deoptimization::trap_reason_name(int)`.
    pub fn trap_reason_name(reason: i32) -> String {
        DeoptReason::name_of(reason)
    }

    /// `Deoptimization::trap_action_name(int)`.
    pub fn trap_action_name(action: i32) -> String {
        match DeoptAction::from_i32(action) {
            Some(a) if (action as u32) < DeoptAction::ActionLimit as u32 => {
                a.name().to_string()
            }
            _ => format!("action{action}"),
        }
    }

    // -- frame predicates ------------------------------------------------------

    /// Whether the deopt request invalidates or recompiles the nmethod
    /// (`Action_reinterpret` and stronger).
    pub fn action_invalidates_nmethod(action: DeoptAction) -> bool {
        action.invalidates_nmethod()
    }

    /// Whether a deoptimization is warranted at all (reason != none).
    pub fn is_deopt_reason(reason: DeoptReason) -> bool {
        reason != DeoptReason::ReasonNone
    }
}

// ---------------------------------------------------------------------------
// Frame description data model (vframeArray / ScopeDesc inputs).
// ---------------------------------------------------------------------------
//
// When a compiled frame deoptimizes it expands into a chain of
// interpreter frames described by ScopeValue trees.  The structures
// below are the read-only *descriptions* — jitruntime/interpreter code
// materializes the real frames; this module defines the vocabulary.

/// `Location`/`LocationValue`: where a value lives in the compiled
/// frame (register, stack slot, or constant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocationKind {
    /// Normal (non-derived) location.
    Normal,
    /// Derived pointer: base oop plus constant offset.
    Derived,
    /// Oop written to a stack location in the caller frame.
    OopWrite,
}

/// A concrete location inside the compiled frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    pub kind: LocationKind,
    /// Register id or stack-slot offset depending on `is_register`.
    pub value: i32,
    /// Whether `value` names a register (true) or a stack offset (false).
    pub is_register: bool,
    /// For `Derived` locations: offset added to the base oop.
    pub offset: i32,
    /// Type width of the location (slot count).
    pub width: u8,
}

/// `ConstantIntValue`/`ConstantLongValue`/... collapsed into one variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstantValue {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Null,
}

/// `ScopeValue` subtree: a single JVM value inside a described frame.
/// Mirrors the C++ `ScopeValue` type hierarchy as an owned enum.
#[derive(Debug, Clone, PartialEq)]
pub enum ScopeValue {
    /// Value held in a `Location` of the compiled frame.
    Location(Location),
    /// Constant primitive value.
    Constant(ConstantValue),
    /// Constant oop reference (object-arena handle).
    ConstantObject(usize),
    /// Reconstructed virtual object (`ObjectValue`).
    Object(ObjectValue),
    /// Reconstructed array (`ArrayValue`).
    Array(ArrayValue),
    /// Derived pointer value.
    DerivedPointer(DerivedPointerValue),
    /// A second use of another scope value index.
    Duplicate(usize),
}

/// `ObjectValue`: fields of a virtual (escape-analyzed) object.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectValue {
    /// Internal class name of the virtual object.
    pub klass: Vec<u8>,
    /// Field values in declaration order.
    pub fields: Vec<ScopeValue>,
}

/// `ArrayValue`: elements of a virtual array.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayValue {
    /// Element kind code (`signature::field` kind).
    pub element_kind: u8,
    /// Element values.
    pub elements: Vec<ScopeValue>,
}

/// `DerivedPointerValue`: base oop value plus constant byte offset.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedPointerValue {
    pub base: Box<ScopeValue>,
    pub offset: i32,
}

/// `MonitorValue`: an eliminated or real monitor in a described frame.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorValue {
    /// The lock object (owner oop or virtual object).
    pub owner: ScopeValue,
    /// Location of the BasicLock / displaced header.
    pub location: Location,
    /// Whether the monitor was eliminated by escape analysis.
    pub eliminated: bool,
}

/// `ScopeDesc` analogue: description of one JVM frame inside a
/// deoptimized compiled activation, innermost first.
#[derive(Debug, Clone)]
pub struct FrameDesc {
    /// Internal name of the method's declaring class.
    pub holder: Vec<u8>,
    /// Method name.
    pub name: Vec<u8>,
    /// Method descriptor.
    pub descriptor: Vec<u8>,
    /// Bytecode index to resume at.
    pub bci: usize,
    /// Local variable values (`locals` in the interpreter frame).
    pub locals: Vec<ScopeValue>,
    /// Operand stack values.
    pub expressions: Vec<ScopeValue>,
    /// Monitors held by this frame.
    pub monitors: Vec<MonitorValue>,
    /// Virtual objects referenced by this scope (escape-analyzed).
    pub objects: Vec<ObjectValue>,
    /// Reexecute the bytecode at `bci` (should_reexecute).
    pub should_reexecute: bool,
    /// Re-throw a pending exception through this frame.
    pub rethrow_exception: bool,
    /// The frame returns an oop (return_oop flag).
    pub return_oop: bool,
    /// Index of the sender frame in the same `FrameDescription`, or
    /// `usize::MAX` when this is the outermost scope.
    pub sender: usize,
}

impl FrameDesc {
    /// `ScopeDesc::is_top`: outermost described scope.
    pub fn is_top(&self) -> bool {
        self.sender == usize::MAX
    }

    /// `ScopeDesc::sender` resolved against the description chain.
    pub fn sender_desc<'a>(&self, description: &'a FrameDescription) -> Option<&'a FrameDesc> {
        if self.sender == usize::MAX {
            None
        } else {
            description.frames.get(self.sender)
        }
    }
}

/// `vframeArray` analogue: the ordered list of interpreter frames a
/// compiled activation expands into.
#[derive(Debug, Clone, Default)]
pub struct FrameDescription {
    /// Described frames, innermost (index 0) first.
    pub frames: Vec<FrameDesc>,
}

impl FrameDescription {
    /// `vframeArray::unpack` input validation: every frame must carry a
    /// consistent sender chain terminating at the outermost scope.
    pub fn validate(&self) -> Result<(), &'static str> {
        for (index, frame) in self.frames.iter().enumerate() {
            if frame.sender == usize::MAX {
                if index + 1 != self.frames.len() {
                    return Err("non-outermost frame claims to be top");
                }
            } else if frame.sender <= index || frame.sender >= self.frames.len() {
                return Err("sender index out of order");
            }
        }
        Ok(())
    }

    /// Iterates sender chain from `index` to the outermost frame.
    pub fn sender_chain(&self, index: usize) -> impl Iterator<Item = &FrameDesc> + '_ {
        let mut current = self.frames.get(index);
        std::iter::from_fn(move || {
            let frame = current?;
            current = frame.sender_desc(self);
            Some(frame)
        })
    }
}

impl fmt::Display for DeoptReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for DeoptAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trap_request_roundtrip() {
        for reason in [
            DeoptReason::ReasonNullCheck,
            DeoptReason::ReasonRangeCheck,
            DeoptReason::ReasonSpeculateClassCheck,
            DeoptReason::ReasonConstraint,
        ] {
            for action in [
                DeoptAction::ActionNone,
                DeoptAction::ActionMaybeRecompile,
                DeoptAction::ActionReinterpret,
                DeoptAction::ActionMakeNotEntrant,
                DeoptAction::ActionMakeNotCompilable,
            ] {
                let request = Deoptimization::make_trap_request(reason, action, -1);
                assert!(request < 0);
                assert_eq!(Deoptimization::trap_request_reason(request), reason);
                assert_eq!(Deoptimization::trap_request_action(request), action);
                assert_eq!(Deoptimization::trap_request_debug_id(request), 0);
                assert_eq!(Deoptimization::trap_request_index(request), -1);
            }
        }
    }

    #[test]
    fn trap_request_index_form() {
        // Positive trap_request is a constant-pool index: reason is
        // `unloaded` and action is `reinterpret` per HotSpot defaults.
        let request = 42;
        assert_eq!(Deoptimization::trap_request_reason(request), DeoptReason::ReasonUnloaded);
        assert_eq!(Deoptimization::trap_request_action(request), DeoptAction::ActionReinterpret);
        assert_eq!(Deoptimization::trap_request_index(request), 42);
    }

    #[test]
    fn trap_state_lattice() {
        let mut state = 0;
        assert_eq!(TrapState::reason(state), DeoptReason::ReasonNone);
        state = TrapState::add_reason(state, DeoptReason::ReasonNullCheck);
        assert_eq!(TrapState::reason(state), DeoptReason::ReasonNullCheck);
        state = TrapState::add_reason(state, DeoptReason::ReasonNullCheck);
        assert_eq!(TrapState::reason(state), DeoptReason::ReasonNullCheck);
        state = TrapState::add_reason(state, DeoptReason::ReasonRangeCheck);
        assert_eq!(TrapState::reason(state), DeoptReason::ReasonMany);
        state = TrapState::set_recompiled(state, true);
        assert!(TrapState::is_recompiled(state));
        assert_eq!(TrapState::reason(state), DeoptReason::ReasonMany);
    }

    #[test]
    fn reason_classification() {
        assert!(DeoptReason::ReasonNullCheck.is_recorded_per_bytecode());
        assert!(DeoptReason::ReasonProfilePredicate.is_recorded_per_bytecode());
        assert!(!DeoptReason::ReasonDiv0Check.is_recorded_per_bytecode());
        assert_eq!(
            DeoptReason::ReasonDiv0Check.recorded_per_bytecode_if_any(),
            DeoptReason::ReasonNullCheck
        );
        assert_eq!(
            DeoptReason::ReasonSpeculateClassCheck.recorded_per_bytecode_if_any(),
            DeoptReason::ReasonClassCheck
        );
        assert_eq!(
            DeoptReason::ReasonUnstableIf.recorded_per_bytecode_if_any(),
            DeoptReason::ReasonIntrinsic
        );
        assert!(DeoptReason::ReasonSpeculateNullCheck.is_speculate());
        assert!(!DeoptReason::ReasonNullCheck.is_speculate());
        assert_eq!(DeoptReason::null_check(true), DeoptReason::ReasonSpeculateNullCheck);
        assert_eq!(DeoptReason::class_check(false), DeoptReason::ReasonClassCheck);
        assert_eq!(DeoptReason::null_assert(true), DeoptReason::ReasonSpeculateNullAssert);
    }

    #[test]
    fn unpack_type_resume() {
        assert!(UnpackType::UnpackUncommonTrap.reexecutes_bytecode());
        assert!(UnpackType::UnpackReexecute.reexecutes_bytecode());
        assert!(!UnpackType::UnpackDeopt.reexecutes_bytecode());
        assert!(!UnpackType::UnpackException.reexecutes_bytecode());
    }

    #[test]
    fn frame_description_chain() {
        let mut description = FrameDescription::default();
        for index in 0..3 {
            description.frames.push(FrameDesc {
                holder: b"a/B".to_vec(),
                name: b"m".to_vec(),
                descriptor: b"()V".to_vec(),
                bci: index,
                locals: Vec::new(),
                expressions: Vec::new(),
                monitors: Vec::new(),
                objects: Vec::new(),
                should_reexecute: false,
                rethrow_exception: false,
                return_oop: false,
                sender: if index + 1 == 3 { usize::MAX } else { index + 1 },
            });
        }
        assert!(description.validate().is_ok());
        let chain: Vec<_> = description.sender_chain(0).collect();
        assert_eq!(chain.len(), 3);
        assert!(chain[2].is_top());
    }
}
