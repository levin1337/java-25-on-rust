/*
 * Port of OpenJDK 25 HotSpot Deoptimization (share/runtime/deoptimization.hpp / deoptimization.cpp)
 *
 * Deoptimization translates compiled code frames back to interpreter frames
 * when speculative optimizations are invalidated or unexpected traps occur.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeoptReason {
    ReasonNone = 0,
    ReasonNullCheck = 1,
    ReasonRangeCheck = 2,
    ReasonClassCheck = 3,
    ReasonArrayStoreException = 4,
    ReasonUnreachedCode = 5,
    ReasonDivByZero = 6,
    ReasonConstraint = 7,
    ReasonSpeculateType = 8,
    ReasonSpeculateNullCheck = 9,
    ReasonTransferToInterpreter = 10,
}

impl DeoptReason {
    pub fn name(self) -> &'static str {
        match self {
            Self::ReasonNone => "none",
            Self::ReasonNullCheck => "null_check",
            Self::ReasonRangeCheck => "range_check",
            Self::ReasonClassCheck => "class_check",
            Self::ReasonArrayStoreException => "array_store",
            Self::ReasonUnreachedCode => "unreached",
            Self::ReasonDivByZero => "div0_check",
            Self::ReasonConstraint => "constraint",
            Self::ReasonSpeculateType => "speculate_type",
            Self::ReasonSpeculateNullCheck => "speculate_null_check",
            Self::ReasonTransferToInterpreter => "transfer_to_interpreter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeoptAction {
    ActionNone = 0,
    ActionMaybeRecompile = 1,
    ActionReinterpret = 2,
    ActionMakeNotEntrant = 3,
    ActionMakeNotCompilable = 4,
}

impl DeoptAction {
    pub fn name(self) -> &'static str {
        match self {
            Self::ActionNone => "none",
            Self::ActionMaybeRecompile => "maybe_recompile",
            Self::ActionReinterpret => "reinterpret",
            Self::ActionMakeNotEntrant => "make_not_entrant",
            Self::ActionMakeNotCompilable => "make_not_compilable",
        }
    }
}

pub struct Deoptimization;

impl Deoptimization {
    pub fn trap_request(reason: DeoptReason, action: DeoptAction) -> i32 {
        ((reason as i32) << 4) | (action as i32)
    }

    pub fn trap_request_reason(trap_req: i32) -> DeoptReason {
        match (trap_req >> 4) & 0xF {
            1 => DeoptReason::ReasonNullCheck,
            2 => DeoptReason::ReasonRangeCheck,
            3 => DeoptReason::ReasonClassCheck,
            4 => DeoptReason::ReasonArrayStoreException,
            5 => DeoptReason::ReasonUnreachedCode,
            6 => DeoptReason::ReasonDivByZero,
            7 => DeoptReason::ReasonConstraint,
            8 => DeoptReason::ReasonSpeculateType,
            9 => DeoptReason::ReasonSpeculateNullCheck,
            10 => DeoptReason::ReasonTransferToInterpreter,
            _ => DeoptReason::ReasonNone,
        }
    }

    pub fn trap_request_action(trap_req: i32) -> DeoptAction {
        match trap_req & 0xF {
            1 => DeoptAction::ActionMaybeRecompile,
            2 => DeoptAction::ActionReinterpret,
            3 => DeoptAction::ActionMakeNotEntrant,
            4 => DeoptAction::ActionMakeNotCompilable,
            _ => DeoptAction::ActionNone,
        }
    }

    pub fn trap_request_name(trap_req: i32) -> String {
        let reason = Self::trap_request_reason(trap_req);
        let action = Self::trap_request_action(trap_req);
        format!("trap[reason={}, action={}]", reason.name(), action.name())
    }
}
