/*
 * Port of OpenJDK 25 HotSpot C2 Compiler Node (share/opto/node.hpp / node.cpp)
 *
 * Sea-of-Nodes Ideal Graph representation. Each Node is an operation with
 * data/control/memory edges stored as a flat input list; nodes are stored in
 * the PhaseGVN arena and referenced by NodeId.
 */

pub type NodeId = usize;

pub const NONE: NodeId = usize::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmpCondition {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Any,
    None,
    Bool,
    Int,
    Long,
    Float,
    Double,
    Ref,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Opcode {
    Root,
    Start,
    Halt,

    Return,

    If,
    IfTrue,
    IfFalse,
    IfException,

    Region,
    Phi,
    MergeMem,
    Proj(u32),

    ConI(i32),
    ConL(i64),
    ConF(u32),
    ConD(u64),
    ConP(u32),
    Bool(bool),
    Parm(u16),

    /// OSR entry inputs: index into the marshal buffer written by the
    /// interpreter (locals `[0, max_locals)`, then stack values).
    /// Kind is fixed by the opcode so width/regalloc classification
    /// works without typer feedback.
    OsrI(u32),
    OsrL(u32),
    OsrF(u32),
    OsrD(u32),
    OsrP(u32),

    I2L,
    I2F,
    I2D,
    L2I,
    L2F,
    L2D,
    F2I,
    F2L,
    F2D,
    D2I,
    D2L,
    D2F,
    I2B,
    I2C,
    I2S,

    AddI,
    SubI,
    MulI,
    DivI,
    RemI,
    NegI,
    AndI,
    OrI,
    XorI,
    LShiftI,
    RShiftI,
    URShiftI,

    AddL,
    SubL,
    MulL,
    DivL,
    RemL,
    NegL,
    AndL,
    OrL,
    XorL,
    LShiftL,
    RShiftL,
    URShiftL,

    AddF,
    SubF,
    MulF,
    DivF,
    RemF,
    NegF,

    AddD,
    SubD,
    MulD,
    DivD,
    RemD,
    NegD,

    CmpI,
    CmpL,
    CmpF(bool),
    CmpD(bool),

    BoolCmp(CmpCondition),

    LoadI,
    LoadL,
    LoadF,
    LoadD,
    LoadP,
    LoadB,
    LoadC,
    LoadS,

    StoreI,
    StoreL,
    StoreF,
    StoreD,
    StoreP,
    StoreB,
    StoreC,
    StoreS,

    New(u32),
    NewArray(u32),
    NewArrayRef(u32),
    MultianewArray,
    ArrayLength,
    CheckCast(u32),
    InstanceOf(u32),
    MonitorEnter,
    MonitorExit,
    Athrow,

    /// Constant leaked `StubSite` pointer (owner/name/descriptor) passed
    /// to runtime-stub-backed field/class/call sites.  The payload is
    /// the raw `u64` address of a `c1_LIR::StubSite`; 0 means the
    /// constant-pool site was unresolved and the stub deoptimizes.
    SitePtr(u64),

    /// Method invocation lowered to the `UnresolvedCall` runtime stub.
    /// Payload packs `pack_call(cp_index, invoke_opcode, result_kind)`.
    Call(u64),
}

/// Result category of a `Call` node, packed into the `Call` payload and
/// used by the register allocator / emitter to pick the result channel
/// (rax vs xmm0) and width.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallResult {
    Void,
    I32,
    I64,
    F32,
    F64,
    Ref,
}

impl CallResult {
    pub fn code(self) -> u64 {
        match self {
            Self::Void => 0,
            Self::I32 => 1,
            Self::I64 => 2,
            Self::F32 => 3,
            Self::F64 => 4,
            Self::Ref => 5,
        }
    }

    pub fn from_code(code: u64) -> Self {
        match code {
            1 => Self::I32,
            2 => Self::I64,
            3 => Self::F32,
            4 => Self::F64,
            5 => Self::Ref,
            _ => Self::Void,
        }
    }

    pub fn is_xmm(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    pub fn is_wide_gp(self) -> bool {
        matches!(self, Self::I64 | Self::Ref)
    }
}

/// Packs a `Call` node payload: bits[0:16) constant-pool index,
/// bits[16:24) JVM invoke opcode, bits[24:32) `CallResult` code.
pub fn call_pack(cp: u32, invoke_opcode: u8, result: CallResult) -> u64 {
    (cp as u64 & 0xffff) | ((invoke_opcode as u64) << 16) | (result.code() << 24)
}

/// Inverse of `call_pack`.
pub fn call_unpack(payload: u64) -> (u32, u8, CallResult) {
    (
        (payload & 0xffff) as u32,
        ((payload >> 16) & 0xff) as u8,
        CallResult::from_code((payload >> 24) & 0xff),
    )
}

impl Opcode {
    pub fn is_pinned(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            Root | Start
                | Halt
                | Return
                | If
                | IfTrue
                | IfFalse
                | IfException
                | Region
                | Phi
                | MergeMem
                | Proj(_)
                | New(_)
                | NewArray(_)
                | NewArrayRef(_)
                | MultianewArray
                | MonitorEnter
                | MonitorExit
                | Athrow
                | Call(_)
                | StoreI
                | StoreL
                | StoreF
                | StoreD
                | StoreP
                | StoreB
                | StoreC
                | StoreS
        )
    }

    /// Nodes that read the heap or can trap even when their result value
    /// is unused (dead-code elimination must treat them as live roots).
    /// Loads may fault on null receivers, `checkcast`/`instanceof` observe
    /// heap/class state, `arraylength` can NPE, and integer div/rem raise
    /// `ArithmeticException` on zero divisors.
    pub fn is_effectful_value(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            LoadI
                | LoadL
                | LoadF
                | LoadD
                | LoadP
                | LoadB
                | LoadC
                | LoadS
                | ArrayLength
                | CheckCast(_)
                | InstanceOf(_)
                | DivI
                | RemI
                | DivL
                | RemL
        )
    }

    pub fn is_const(&self) -> bool {
        matches!(
            self,
            Opcode::ConI(_)
                | Opcode::ConL(_)
                | Opcode::ConF(_)
                | Opcode::ConD(_)
                | Opcode::ConP(_)
                | Opcode::Bool(_)
                | Opcode::SitePtr(_)
        )
    }

    pub fn unary_arith(&self) -> bool {
        use Opcode::*;
        matches!(self, NegI | NegL | NegF | NegD)
    }

    pub fn binary_arith(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            AddI | SubI
                | MulI
                | DivI
                | RemI
                | AndI
                | OrI
                | XorI
                | LShiftI
                | RShiftI
                | URShiftI
                | AddL
                | SubL
                | MulL
                | DivL
                | RemL
                | AndL
                | OrL
                | XorL
                | LShiftL
                | RShiftL
                | URShiftL
                | AddF
                | SubF
                | MulF
                | DivF
                | RemF
                | AddD
                | SubD
                | MulD
                | DivD
                | RemD
                | CmpI
                | CmpL
                | CmpF(_)
                | CmpD(_)
                | BoolCmp(_)
        )
    }

    pub fn is_commutative(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            AddI | MulI
                | AndI
                | OrI
                | XorI
                | AddL
                | MulL
                | AndL
                | OrL
                | XorL
                | AddF
                | MulF
                | AddD
                | MulD
        )
    }

    pub fn int_like(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            ConI(_)
                | AddI
                | SubI
                | MulI
                | DivI
                | RemI
                | NegI
                | AndI
                | OrI
                | XorI
                | LShiftI
                | RShiftI
                | URShiftI
                | CmpI
                | BoolCmp(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArrayElemKind {
    Int,
    Long,
    Float,
    Double,
    Ref,
    Byte,
    Char,
    Short,
}

impl ArrayElemKind {
    pub fn code(self) -> i32 {
        match self {
            Self::Int => 0,
            Self::Long => 1,
            Self::Float => 2,
            Self::Double => 3,
            Self::Ref => 12,
            Self::Byte => 5,
            Self::Char => 6,
            Self::Short => 7,
        }
    }

    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Int),
            1 => Some(Self::Long),
            2 => Some(Self::Float),
            3 => Some(Self::Double),
            12 => Some(Self::Ref),
            5 => Some(Self::Byte),
            6 => Some(Self::Char),
            7 => Some(Self::Short),
            _ => None,
        }
    }

    pub fn element_size(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Char | Self::Short => 2,
            Self::Int | Self::Float | Self::Ref => 4,
            Self::Long | Self::Double => 8,
        }
    }

    pub fn element_align(self) -> usize {
        self.element_size()
    }

    pub fn is_xmm(self) -> bool {
        matches!(self, Self::Float | Self::Double)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub op: Opcode,
    pub inputs: Vec<NodeId>,
    pub ty: Type,
    pub out_cnt: usize,
}

impl Node {
    pub fn new(id: NodeId, op: Opcode, inputs: Vec<NodeId>) -> Self {
        let ty = compute_type(&op);
        Self {
            id,
            op,
            inputs,
            ty,
            out_cnt: 0,
        }
    }

    pub fn is_pinned(&self) -> bool {
        self.op.is_pinned()
    }

    pub fn is_const(&self) -> bool {
        self.op.is_const()
    }

    pub fn as_con_i(&self) -> Option<i32> {
        match self.op {
            Opcode::ConI(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_con_l(&self) -> Option<i64> {
        match self.op {
            Opcode::ConL(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_con_f(&self) -> Option<f32> {
        match self.op {
            Opcode::ConF(v) => Some(f32::from_bits(v)),
            _ => None,
        }
    }

    pub fn as_con_d(&self) -> Option<f64> {
        match self.op {
            Opcode::ConD(v) => Some(f64::from_bits(v)),
            _ => None,
        }
    }

    pub fn as_con_p(&self) -> Option<u32> {
        match self.op {
            Opcode::ConP(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.op {
            Opcode::Bool(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_int_const(&self) -> Option<i64> {
        match self.op {
            Opcode::ConI(v) => Some(v as i64),
            Opcode::ConL(v) => Some(v),
            _ => None,
        }
    }

    pub fn in_node(&self, idx: usize) -> Option<NodeId> {
        self.inputs.get(idx).copied()
    }

    pub fn req(&self) -> usize {
        self.inputs.len()
    }

    pub fn is_region(&self) -> bool {
        matches!(self.op, Opcode::Region)
    }

    pub fn is_phi(&self) -> bool {
        matches!(self.op, Opcode::Phi)
    }

    pub fn is_if(&self) -> bool {
        matches!(self.op, Opcode::If)
    }

    pub fn is_root(&self) -> bool {
        matches!(self.op, Opcode::Root)
    }

    pub fn is_start(&self) -> bool {
        matches!(self.op, Opcode::Start)
    }

    pub fn is_call(&self) -> bool {
        matches!(self.op, Opcode::Call(_))
    }
}

fn compute_type(op: &Opcode) -> Type {
    use Opcode::*;
    match op {
        ConI(_) | CmpI | CmpL | CmpF(_) | CmpD(_) | InstanceOf(_) | ArrayLength | LoadI | LoadB
        | LoadC | LoadS | I2B | I2C | I2S | OsrI(_) => Type::Int,
        ConL(_) | LoadL | OsrL(_) => Type::Long,
        ConF(_) | LoadF | OsrF(_) => Type::Float,
        ConD(_) | LoadD | OsrD(_) => Type::Double,
        ConP(_) | LoadP | CheckCast(_) | New(_) | NewArray(_) | NewArrayRef(_) | MultianewArray
        | OsrP(_) => {
            Type::Ref
        }
        Bool(_) | BoolCmp(_) => Type::Bool,
        Parm(_) => Type::Any,
        Call(p) => match call_unpack(*p).2 {
            CallResult::Void => Type::None,
            CallResult::I32 => Type::Int,
            CallResult::I64 => Type::Long,
            CallResult::F32 => Type::Float,
            CallResult::F64 => Type::Double,
            CallResult::Ref => Type::Ref,
        },
        L2I => Type::Int,
        L2F => Type::Float,
        L2D => Type::Double,
        F2I => Type::Int,
        F2L => Type::Long,
        F2D => Type::Double,
        D2I => Type::Int,
        D2L => Type::Long,
        D2F => Type::Float,
        I2L => Type::Long,
        I2F => Type::Float,
        I2D => Type::Double,
        AddL | SubL | MulL | DivL | RemL | NegL | AndL | OrL | XorL | LShiftL | RShiftL
        | URShiftL => Type::Long,
        AddF | SubF | MulF | DivF | RemF | NegF => Type::Float,
        AddD | SubD | MulD | DivD | RemD | NegD => Type::Double,
        AddI | SubI | MulI | DivI | RemI | NegI | AndI | OrI | XorI | LShiftI | RShiftI
        | URShiftI => Type::Int,
        _ => Type::None,
    }
}
