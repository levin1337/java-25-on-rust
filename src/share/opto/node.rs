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
    ArrayLength,
    CheckCast(u32),
    InstanceOf(u32),
    MonitorEnter,
    MonitorExit,
    Athrow,

    Call(u32),
}

impl Opcode {
    pub fn is_pinned(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            Root
                | Start
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

    pub fn is_const(&self) -> bool {
        matches!(
            self,
            Opcode::ConI(_)
                | Opcode::ConL(_)
                | Opcode::ConF(_)
                | Opcode::ConD(_)
                | Opcode::ConP(_)
                | Opcode::Bool(_)
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
            AddI | SubI | MulI | DivI | RemI | AndI | OrI | XorI | LShiftI | RShiftI | URShiftI
                | AddL | SubL | MulL | DivL | RemL | AndL | OrL | XorL | LShiftL | RShiftL
                | URShiftL | AddF | SubF | MulF | DivF | RemF | AddD | SubD | MulD | DivD | RemD
                | CmpI | CmpL | CmpF(_) | CmpD(_) | BoolCmp(_)
        )
    }

    pub fn is_commutative(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            AddI | MulI | AndI | OrI | XorI | AddL | MulL | AndL | OrL | XorL | AddF | MulF
                | AddD | MulD
        )
    }

    pub fn int_like(&self) -> bool {
        use Opcode::*;
        matches!(
            self,
            ConI(_) | AddI | SubI | MulI | DivI | RemI | NegI | AndI | OrI | XorI | LShiftI
                | RShiftI | URShiftI | CmpI | BoolCmp(_)
        )
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
        ConI(_) | CmpI | CmpL | CmpF(_) | CmpD(_) | InstanceOf(_) | ArrayLength | LoadI
        | LoadB | LoadC | LoadS | I2B | I2C | I2S => Type::Int,
        ConL(_) | LoadL => Type::Long,
        ConF(_) | LoadF => Type::Float,
        ConD(_) | LoadD => Type::Double,
        ConP(_) | LoadP | CheckCast(_) | New(_) | NewArray(_) | NewArrayRef(_) => Type::Ref,
        Bool(_) | BoolCmp(_) => Type::Bool,
        Parm(_) | Call(_) => Type::Any,
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
