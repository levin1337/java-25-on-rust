/*
 * Port of OpenJDK 25 HotSpot C2 Compiler Node (share/opto/node.hpp / node.cpp)
 *
 * Implements Cliff Click's Sea-of-Nodes Ideal Graph representation.
 * Each Node represents an operation with data and/or control dependencies (edges).
 */

pub type NodeId = usize;

#[derive(Clone, Debug, PartialEq)]
pub enum Opcode {
    Root,
    Start,
    Halt,
    Return,
    Region,
    Phi,
    If,
    IfTrue,
    IfFalse,
    Bool(CmpCondition),

    // Constants
    ConI(i32),
    ConL(i64),
    ConF(f32),
    ConD(f64),

    // Arithmetic
    AddI,
    SubI,
    MulI,
    DivI,
    AddL,
    SubL,
    MulL,
    DivL,

    // Bitwise
    AndI,
    OrI,
    XorI,
    LShiftI,
    RShiftI,
    URShiftI,

    // Comparisons
    CmpI,
    CmpL,

    // Memory
    LoadI,
    StoreI,
    LoadL,
    StoreL,

    // Projections
    Proj(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmpCondition {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub op: Opcode,
    pub inputs: Vec<NodeId>,
    pub out_cnt: usize,
}

impl Node {
    pub fn new(id: NodeId, op: Opcode, inputs: Vec<NodeId>) -> Self {
        Self {
            id,
            op,
            inputs,
            out_cnt: 0,
        }
    }

    pub fn is_con(&self) -> bool {
        matches!(
            self.op,
            Opcode::ConI(_) | Opcode::ConL(_) | Opcode::ConF(_) | Opcode::ConD(_)
        )
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

    pub fn in_node(&self, idx: usize) -> Option<NodeId> {
        self.inputs.get(idx).copied()
    }

    pub fn req(&self) -> usize {
        self.inputs.len()
    }
}
