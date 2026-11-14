/*
 * Port of OpenJDK 25 HotSpot C2 PhaseGVN (share/opto/phaseX.hpp / phaseX.cpp)
 *
 * Implements Global Value Numbering (GVN), algebraic identity simplifications,
 * constant folding, and Common Subexpression Elimination (CSE) over the Sea-of-Nodes IR.
 */

use super::node::{Node, NodeId, Opcode};
use std::collections::HashMap;

pub struct PhaseGVN {
    nodes: Vec<Node>,
    // Hash table for common subexpression elimination: (Opcode, Inputs) -> NodeId
    cse_table: HashMap<(OpcodeKey, Vec<NodeId>), NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum OpcodeKey {
    Root,
    Start,
    Halt,
    Return,
    Region,
    Phi,
    If,
    IfTrue,
    IfFalse,
    Bool(u8),
    ConI(i32),
    ConL(i64),
    ConF(u32),
    ConD(u64),
    AddI,
    SubI,
    MulI,
    DivI,
    AddL,
    SubL,
    MulL,
    DivL,
    AndI,
    OrI,
    XorI,
    LShiftI,
    RShiftI,
    URShiftI,
    CmpI,
    CmpL,
    LoadI,
    StoreI,
    LoadL,
    StoreL,
    Proj(u32),
}

impl From<&Opcode> for OpcodeKey {
    fn from(op: &Opcode) -> Self {
        match op {
            Opcode::Root => OpcodeKey::Root,
            Opcode::Start => OpcodeKey::Start,
            Opcode::Halt => OpcodeKey::Halt,
            Opcode::Return => OpcodeKey::Return,
            Opcode::Region => OpcodeKey::Region,
            Opcode::Phi => OpcodeKey::Phi,
            Opcode::If => OpcodeKey::If,
            Opcode::IfTrue => OpcodeKey::IfTrue,
            Opcode::IfFalse => OpcodeKey::IfFalse,
            Opcode::Bool(cond) => OpcodeKey::Bool(*cond as u8),
            Opcode::ConI(v) => OpcodeKey::ConI(*v),
            Opcode::ConL(v) => OpcodeKey::ConL(*v),
            Opcode::ConF(v) => OpcodeKey::ConF(v.to_bits()),
            Opcode::ConD(v) => OpcodeKey::ConD(v.to_bits()),
            Opcode::AddI => OpcodeKey::AddI,
            Opcode::SubI => OpcodeKey::SubI,
            Opcode::MulI => OpcodeKey::MulI,
            Opcode::DivI => OpcodeKey::DivI,
            Opcode::AddL => OpcodeKey::AddL,
            Opcode::SubL => OpcodeKey::SubL,
            Opcode::MulL => OpcodeKey::MulL,
            Opcode::DivL => OpcodeKey::DivL,
            Opcode::AndI => OpcodeKey::AndI,
            Opcode::OrI => OpcodeKey::OrI,
            Opcode::XorI => OpcodeKey::XorI,
            Opcode::LShiftI => OpcodeKey::LShiftI,
            Opcode::RShiftI => OpcodeKey::RShiftI,
            Opcode::URShiftI => OpcodeKey::URShiftI,
            Opcode::CmpI => OpcodeKey::CmpI,
            Opcode::CmpL => OpcodeKey::CmpL,
            Opcode::LoadI => OpcodeKey::LoadI,
            Opcode::StoreI => OpcodeKey::StoreI,
            Opcode::LoadL => OpcodeKey::LoadL,
            Opcode::StoreL => OpcodeKey::StoreL,
            Opcode::Proj(idx) => OpcodeKey::Proj(*idx),
        }
    }
}

impl PhaseGVN {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            cse_table: HashMap::new(),
        }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Adds an integer constant, deduplicated via CSE
    pub fn con_i(&mut self, value: i32) -> NodeId {
        self.transform(Opcode::ConI(value), vec![])
    }

    /// Adds a long constant, deduplicated via CSE
    pub fn con_l(&mut self, value: i64) -> NodeId {
        self.transform(Opcode::ConL(value), vec![])
    }

    /// Performs Ideal transformations (constant folding, algebraic identities)
    /// and Common Subexpression Elimination (CSE).
    pub fn transform(&mut self, op: Opcode, inputs: Vec<NodeId>) -> NodeId {
        // 1. Try Ideal Simplifications
        if let Some(simplified_id) = self.ideal_transform(&op, &inputs) {
            return simplified_id;
        }

        // 2. Check CSE Hash Table
        let key = (OpcodeKey::from(&op), inputs.clone());
        if let Some(&existing_id) = self.cse_table.get(&key) {
            return existing_id;
        }

        // 3. Allocate new node
        let id = self.nodes.len();
        for &in_id in &inputs {
            if in_id < self.nodes.len() {
                self.nodes[in_id].out_cnt += 1;
            }
        }

        let node = Node::new(id, op, inputs);
        self.nodes.push(node);
        self.cse_table.insert(key, id);
        id
    }

    fn ideal_transform(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        match op {
            // AddI transformations
            Opcode::AddI if inputs.len() == 2 => {
                let (n1, n2) = (inputs[0], inputs[1]);
                let c1 = self.nodes[n1].as_con_i();
                let c2 = self.nodes[n2].as_con_i();

                // Constant folding: ConI(a) + ConI(b) => ConI(a + b)
                if let (Some(a), Some(b)) = (c1, c2) {
                    return Some(self.con_i(a.wrapping_add(b)));
                }

                // Identity: x + 0 => x
                if c2 == Some(0) {
                    return Some(n1);
                }
                if c1 == Some(0) {
                    return Some(n2);
                }
                None
            }

            // SubI transformations
            Opcode::SubI if inputs.len() == 2 => {
                let (n1, n2) = (inputs[0], inputs[1]);
                let c1 = self.nodes[n1].as_con_i();
                let c2 = self.nodes[n2].as_con_i();

                // Constant folding: ConI(a) - ConI(b) => ConI(a - b)
                if let (Some(a), Some(b)) = (c1, c2) {
                    return Some(self.con_i(a.wrapping_sub(b)));
                }

                // Identity: x - 0 => x
                if c2 == Some(0) {
                    return Some(n1);
                }

                // Identity: x - x => 0
                if n1 == n2 {
                    return Some(self.con_i(0));
                }
                None
            }

            // MulI transformations
            Opcode::MulI if inputs.len() == 2 => {
                let (n1, n2) = (inputs[0], inputs[1]);
                let c1 = self.nodes[n1].as_con_i();
                let c2 = self.nodes[n2].as_con_i();

                // Constant folding: ConI(a) * ConI(b) => ConI(a * b)
                if let (Some(a), Some(b)) = (c1, c2) {
                    return Some(self.con_i(a.wrapping_mul(b)));
                }

                // Identity: x * 1 => x
                if c2 == Some(1) {
                    return Some(n1);
                }
                if c1 == Some(1) {
                    return Some(n2);
                }

                // Identity: x * 0 => 0
                if c2 == Some(0) || c1 == Some(0) {
                    return Some(self.con_i(0));
                }
                None
            }

            // DivI transformations
            Opcode::DivI if inputs.len() == 2 => {
                let (n1, n2) = (inputs[0], inputs[1]);
                let c1 = self.nodes[n1].as_con_i();
                let c2 = self.nodes[n2].as_con_i();

                if let (Some(a), Some(b)) = (c1, c2) {
                    if b != 0 {
                        return Some(self.con_i(a.wrapping_div(b)));
                    }
                }

                // Identity: x / 1 => x
                if c2 == Some(1) {
                    return Some(n1);
                }
                None
            }

            // Bitwise operations
            Opcode::AndI if inputs.len() == 2 => {
                let (n1, n2) = (inputs[0], inputs[1]);
                let c1 = self.nodes[n1].as_con_i();
                let c2 = self.nodes[n2].as_con_i();

                if let (Some(a), Some(b)) = (c1, c2) {
                    return Some(self.con_i(a & b));
                }
                if c2 == Some(0) || c1 == Some(0) {
                    return Some(self.con_i(0));
                }
                if c2 == Some(-1) {
                    return Some(n1);
                }
                if c1 == Some(-1) {
                    return Some(n2);
                }
                if n1 == n2 {
                    return Some(n1);
                }
                None
            }

            Opcode::OrI if inputs.len() == 2 => {
                let (n1, n2) = (inputs[0], inputs[1]);
                let c1 = self.nodes[n1].as_con_i();
                let c2 = self.nodes[n2].as_con_i();

                if let (Some(a), Some(b)) = (c1, c2) {
                    return Some(self.con_i(a | b));
                }
                if c2 == Some(0) {
                    return Some(n1);
                }
                if c1 == Some(0) {
                    return Some(n2);
                }
                if n1 == n2 {
                    return Some(n1);
                }
                None
            }

            Opcode::XorI if inputs.len() == 2 => {
                let (n1, n2) = (inputs[0], inputs[1]);
                let c1 = self.nodes[n1].as_con_i();
                let c2 = self.nodes[n2].as_con_i();

                if let (Some(a), Some(b)) = (c1, c2) {
                    return Some(self.con_i(a ^ b));
                }
                if c2 == Some(0) {
                    return Some(n1);
                }
                if c1 == Some(0) {
                    return Some(n2);
                }
                if n1 == n2 {
                    return Some(self.con_i(0));
                }
                None
            }

            _ => None,
        }
    }
}

impl Default for PhaseGVN {
    fn default() -> Self {
        Self::new()
    }
}
