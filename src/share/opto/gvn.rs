/*
 * Port of OpenJDK 25 HotSpot C2 PhaseGVN (share/opto/phaseX.hpp / phaseX.cpp)
 *
 * Global Value Numbering over the Sea-of-Nodes IR: hash-consing of pure nodes,
 * constant folding and algebraic identities (AddNode/SubNode/MulNode/DivNode
 * Ideal logic), commutative canonicalization, plus dead-code elimination and
 * graph compaction (Compile::renumber_live analog).
 */

use super::node::{CmpCondition, Node, NodeId, Opcode, Type, NONE};
use std::collections::HashMap;

pub struct PhaseGVN {
    pub nodes: Vec<Node>,
    cse_table: HashMap<(Opcode, Vec<NodeId>), NodeId>,
    pub root: NodeId,
    pub start: NodeId,
}

impl PhaseGVN {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            cse_table: HashMap::new(),
            root: NONE,
            start: NONE,
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

    pub fn con_i(&mut self, value: i32) -> NodeId {
        self.transform(Opcode::ConI(value), vec![])
    }

    pub fn con_l(&mut self, value: i64) -> NodeId {
        self.transform(Opcode::ConL(value), vec![])
    }

    pub fn con_p(&mut self, value: u32) -> NodeId {
        self.transform(Opcode::ConP(value), vec![])
    }

    pub fn bool_const(&mut self, value: bool) -> NodeId {
        self.transform(Opcode::Bool(value), vec![])
    }

    pub fn alloc(&mut self, op: Opcode, inputs: Vec<NodeId>) -> NodeId {
        let id = self.nodes.len();
        for &in_id in &inputs {
            if in_id != NONE && in_id < self.nodes.len() {
                self.nodes[in_id].out_cnt += 1;
            }
        }
        let node = Node::new(id, op, inputs);
        if node.op == Opcode::Root && self.root == NONE {
            self.root = id;
        }
        if node.op == Opcode::Start && self.start == NONE {
            self.start = id;
        }
        self.nodes.push(node);
        id
    }

    pub fn transform(&mut self, op: Opcode, mut inputs: Vec<NodeId>) -> NodeId {
        if op.is_pinned() {
            return self.alloc(op, inputs);
        }
        if op.is_commutative() && inputs.len() == 2 && inputs[0] > inputs[1] {
            inputs.swap(0, 1);
        }
        if let Some(simplified) = self.ideal_transform(&op, &inputs) {
            return simplified;
        }
        let key = (op.clone(), inputs.clone());
        if let Some(&existing) = self.cse_table.get(&key) {
            return existing;
        }
        let id = self.alloc(op, inputs);
        self.cse_table.insert(key, id);
        id
    }

    pub fn optimize_node(&mut self, id: NodeId) -> Option<NodeId> {
        let (op, inputs) = {
            let n = &self.nodes[id];
            (n.op.clone(), n.inputs.clone())
        };
        if op.is_pinned() {
            return None;
        }
        if let Some(rep) = self.ideal_transform(&op, &inputs) {
            return Some(rep);
        }
        let key = (op, inputs);
        if let Some(&existing) = self.cse_table.get(&key) {
            if existing != id {
                return Some(existing);
            }
        }
        None
    }

    pub fn apply_replacements(&mut self, replacements: &HashMap<NodeId, NodeId>) {
        if replacements.is_empty() {
            return;
        }
        let resolve = |mut x: NodeId| {
            let mut depth = 0;
            while let Some(&next) = replacements.get(&x) {
                if next == x || depth > 64 {
                    break;
                }
                x = next;
                depth += 1;
            }
            x
        };
        for node in self.nodes.iter_mut() {
            for inp in node.inputs.iter_mut() {
                if *inp != NONE {
                    *inp = resolve(*inp);
                }
            }
        }
        self.rebuild_cse_table();
    }

    pub fn rebuild_cse_table(&mut self) {
        self.cse_table.clear();
        for node in self.nodes.iter() {
            if !node.op.is_pinned() && !node.inputs.contains(&NONE) {
                self.cse_table
                    .insert((node.op.clone(), node.inputs.clone()), node.id);
            }
        }
    }

    pub fn iter_gvn(&mut self) -> usize {
        let mut rounds = 0;
        loop {
            let mut replacements: HashMap<NodeId, NodeId> = HashMap::new();
            for id in 0..self.nodes.len() {
                if let Some(rep) = self.optimize_node(id) {
                    if rep != id {
                        replacements.insert(id, rep);
                    }
                }
            }
            if replacements.is_empty() {
                break;
            }
            rounds += 1;
            self.apply_replacements(&replacements);
        }
        rounds
    }

    pub fn live_nodes(&self) -> Vec<bool> {
        let mut live = vec![false; self.nodes.len()];
        if self.root == NONE || self.root >= self.nodes.len() {
            return live;
        }
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if id == NONE || id >= self.nodes.len() || live[id] {
                continue;
            }
            live[id] = true;
            for &inp in &self.nodes[id].inputs {
                if inp != NONE {
                    stack.push(inp);
                }
            }
        }
        live
    }

    pub fn dead_code_elimination(&mut self) -> usize {
        let live = self.live_nodes();
        let dead = live.iter().filter(|&&l| !l).count();
        self.compact(&live);
        dead
    }

    pub fn compact(&mut self, live: &[bool]) {
        let mut map: Vec<NodeId> = vec![NONE; self.nodes.len()];
        let mut new_nodes: Vec<Node> = Vec::with_capacity(live.iter().filter(|&&l| l).count());
        for (old_id, node) in self.nodes.iter().enumerate() {
            if live[old_id] {
                map[old_id] = new_nodes.len();
                let mut nn = node.clone();
                nn.id = new_nodes.len();
                new_nodes.push(nn);
            }
        }
        for node in new_nodes.iter_mut() {
            for inp in node.inputs.iter_mut() {
                if *inp != NONE {
                    *inp = map[*inp];
                }
            }
            node.out_cnt = 0;
        }
        for i in 0..new_nodes.len() {
            let ins = new_nodes[i].inputs.clone();
            for inp in ins {
                if inp != NONE {
                    new_nodes[inp].out_cnt += 1;
                }
            }
        }
        self.root = if self.root != NONE { map[self.root] } else { NONE };
        self.start = if self.start != NONE { map[self.start] } else { NONE };
        self.nodes = new_nodes;
        self.rebuild_cse_table();
    }

    pub fn has_pending_inputs(&self) -> usize {
        self.nodes
            .iter()
            .flat_map(|n| n.inputs.iter())
            .filter(|&&i| i == NONE)
            .count()
    }

    fn ideal_transform(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        match op {
            Opcode::AddI | Opcode::SubI | Opcode::MulI | Opcode::DivI | Opcode::RemI
            | Opcode::AddL | Opcode::SubL | Opcode::MulL | Opcode::DivL | Opcode::RemL
                if inputs.len() == 2 =>
            {
                self.fold_binary_i(op, inputs)
            }
            Opcode::NegI | Opcode::NegL if inputs.len() == 1 => {
                let c = self.int_const(inputs[0])?;
                Some(match op {
                    Opcode::NegI => self.con_i((-c as i32).wrapping_neg() as i32),
                    _ => self.con_l(c.wrapping_neg()),
                })
            }
            Opcode::NegF | Opcode::NegD if inputs.len() == 1 => {
                match self.nodes[inputs[0]].op {
                    Opcode::ConF(v) => {
                        let x = f32::from_bits(v);
                        Some(self.transform(Opcode::ConF((-x).to_bits()), vec![]))
                    }
                    Opcode::ConD(v) => {
                        let x = f64::from_bits(v);
                        Some(self.transform(Opcode::ConD((-x).to_bits()), vec![]))
                    }
                    _ => None,
                }
            }
            Opcode::AddF | Opcode::SubF | Opcode::MulF | Opcode::DivF | Opcode::RemF
            | Opcode::AddD | Opcode::SubD | Opcode::MulD | Opcode::DivD | Opcode::RemD
                if inputs.len() == 2 =>
            {
                self.fold_binary_fp(op, inputs)
            }
            Opcode::AndI | Opcode::OrI | Opcode::XorI | Opcode::AndL | Opcode::OrL
            | Opcode::XorL if inputs.len() == 2 => self.fold_bitwise(op, inputs),
            Opcode::LShiftI | Opcode::RShiftI | Opcode::URShiftI | Opcode::LShiftL
            | Opcode::RShiftL | Opcode::URShiftL if inputs.len() == 2 => {
                self.fold_shift(op, inputs)
            }
            Opcode::CmpI | Opcode::CmpL | Opcode::CmpF(_) | Opcode::CmpD(_) if inputs.len() == 2 => {
                self.fold_cmp(op, inputs)
            }
            Opcode::BoolCmp(_) if inputs.len() == 2 => self.fold_bool_cmp(op, inputs),
            Opcode::I2L | Opcode::I2F | Opcode::I2D | Opcode::L2I | Opcode::L2F | Opcode::L2D
            | Opcode::F2I | Opcode::F2L | Opcode::F2D | Opcode::D2I | Opcode::D2L | Opcode::D2F
            | Opcode::I2B | Opcode::I2C | Opcode::I2S if inputs.len() == 1 => {
                self.fold_convert(op, inputs[0])
            }
            _ => None,
        }
    }

    fn int_const(&self, id: NodeId) -> Option<i64> {
        self.nodes.get(id)?.as_int_const()
    }

    fn long_const(&self, id: NodeId) -> Option<i64> {
        self.nodes.get(id)?.as_con_l()
    }

    fn is_int_op(&self, op: &Opcode) -> bool {
        matches!(
            op,
            Opcode::AddI
                | Opcode::SubI
                | Opcode::MulI
                | Opcode::DivI
                | Opcode::RemI
                | Opcode::AndI
                | Opcode::OrI
                | Opcode::XorI
                | Opcode::LShiftI
                | Opcode::RShiftI
                | Opcode::URShiftI
        )
    }

    fn is_long_op(&self, op: &Opcode) -> bool {
        matches!(
            op,
            Opcode::AddL
                | Opcode::SubL
                | Opcode::MulL
                | Opcode::DivL
                | Opcode::RemL
                | Opcode::AndL
                | Opcode::OrL
                | Opcode::XorL
                | Opcode::LShiftL
                | Opcode::RShiftL
                | Opcode::URShiftL
        )
    }

    fn fold_binary_i(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        let (n1, n2) = (inputs[0], inputs[1]);
        if n1 == n2 {
            match op {
                Opcode::SubI | Opcode::SubL | Opcode::XorI | Opcode::XorL => {
                    return Some(self.zero_for(op));
                }
                Opcode::DivI | Opcode::RemI | Opcode::DivL | Opcode::RemL => return None,
                _ => {}
            }
        }
        let c1 = self.int_const(n1);
        let c2 = self.int_const(n2);
        if let (Some(a), Some(b)) = (c1, c2) {
            if self.is_int_op(op) {
                let (a, b) = (a as i32, b as i32);
                let r = match op {
                    Opcode::AddI => a.wrapping_add(b),
                    Opcode::SubI => a.wrapping_sub(b),
                    Opcode::MulI => a.wrapping_mul(b),
                    Opcode::DivI => {
                        if b == 0 {
                            return None;
                        }
                        a.wrapping_div(b)
                    }
                    Opcode::RemI => {
                        if b == 0 {
                            return None;
                        }
                        a.wrapping_rem(b)
                    }
                    _ => return None,
                };
                return Some(self.con_i(r));
            }
            if self.is_long_op(op) {
                let r = match op {
                    Opcode::AddL => a.wrapping_add(b),
                    Opcode::SubL => a.wrapping_sub(b),
                    Opcode::MulL => a.wrapping_mul(b),
                    Opcode::DivL => {
                        if b == 0 {
                            return None;
                        }
                        a.wrapping_div(b)
                    }
                    Opcode::RemL => {
                        if b == 0 {
                            return None;
                        }
                        a.wrapping_rem(b)
                    }
                    _ => return None,
                };
                return Some(self.con_l(r));
            }
        }
        match op {
            Opcode::AddI => {
                if c2 == Some(0) {
                    return Some(n1);
                }
                if c1 == Some(0) {
                    return Some(n2);
                }
            }
            Opcode::AddL => {
                if c2 == Some(0) {
                    return Some(n1);
                }
                if c1 == Some(0) {
                    return Some(n2);
                }
            }
            Opcode::SubI | Opcode::SubL => {
                if c2 == Some(0) {
                    return Some(n1);
                }
            }
            Opcode::MulI | Opcode::MulL => {
                if c2 == Some(1) {
                    return Some(n1);
                }
                if c1 == Some(1) {
                    return Some(n2);
                }
                if c2 == Some(0) || c1 == Some(0) {
                    return Some(self.zero_for(op));
                }
            }
            Opcode::DivI | Opcode::DivL => {
                if c2 == Some(1) {
                    return Some(n1);
                }
            }
            _ => {}
        }
        None
    }

    fn zero_for(&mut self, op: &Opcode) -> NodeId {
        if self.is_long_op(op) {
            self.con_l(0)
        } else {
            self.con_i(0)
        }
    }

    fn fold_bitwise(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        let (n1, n2) = (inputs[0], inputs[1]);
        if n1 == n2 {
            match op {
                Opcode::AndI | Opcode::AndL => return Some(n1),
                Opcode::OrI | Opcode::OrL => return Some(n1),
                Opcode::XorI | Opcode::XorL => return Some(self.zero_for(op)),
                _ => {}
            }
        }
        let c1 = self.int_const(n1);
        let c2 = self.int_const(n2);
        if let (Some(a), Some(b)) = (c1, c2) {
            let r = match op {
                Opcode::AndI => (a as i32 & b as i32) as i64,
                Opcode::OrI => (a as i32 | b as i32) as i64,
                Opcode::XorI => (a as i32 ^ b as i32) as i64,
                Opcode::AndL => a & b,
                Opcode::OrL => a | b,
                Opcode::XorL => a ^ b,
                _ => return None,
            };
            return Some(if matches!(op, Opcode::AndL | Opcode::OrL | Opcode::XorL) {
                self.con_l(r)
            } else {
                self.con_i(r as i32)
            });
        }
        match op {
            Opcode::AndI | Opcode::AndL => {
                if c2 == Some(0) || c1 == Some(0) {
                    return Some(self.zero_for(op));
                }
                if c2 == Some(-1) {
                    return Some(n1);
                }
                if c1 == Some(-1) {
                    return Some(n2);
                }
            }
            Opcode::OrI | Opcode::OrL => {
                if c2 == Some(0) {
                    return Some(n1);
                }
                if c1 == Some(0) {
                    return Some(n2);
                }
                if c2 == Some(-1) || c1 == Some(-1) {
                    return Some(if matches!(op, Opcode::OrL) {
                        self.con_l(-1)
                    } else {
                        self.con_i(-1)
                    });
                }
            }
            Opcode::XorI | Opcode::XorL => {
                if c2 == Some(0) {
                    return Some(n1);
                }
                if c1 == Some(0) {
                    return Some(n2);
                }
                if n1 == n2 {
                    return Some(self.zero_for(op));
                }
            }
            _ => {}
        }
        None
    }

    fn fold_shift(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        let (n1, n2) = (inputs[0], inputs[1]);
        if self.int_const(n2) == Some(0) {
            return Some(n1);
        }
        let c1 = self.int_const(n1);
        let c2 = self.int_const(n2);
        if let (Some(a), Some(b)) = (c1, c2) {
            let sh = (b & 63) as u32;
            let r = match op {
                Opcode::LShiftI => ((a as i32) << sh) as i64,
                Opcode::RShiftI => ((a as i32) >> sh) as i64,
                Opcode::URShiftI => (((a as i32 as u32) >> sh) as i32) as i64,
                Opcode::LShiftL => a << sh,
                Opcode::RShiftL => a >> sh,
                Opcode::URShiftL => ((a as u64) >> sh) as i64,
                _ => return None,
            };
            return Some(if self.is_long_op(op) {
                self.con_l(r)
            } else {
                self.con_i(r as i32)
            });
        }
        None
    }

    fn fold_cmp(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        let (n1, n2) = (inputs[0], inputs[1]);
        if n1 == n2 {
            match op {
                Opcode::CmpI | Opcode::CmpL | Opcode::CmpF(_) | Opcode::CmpD(_) => {
                    return Some(self.con_i(0));
                }
                _ => {}
            }
        }
        match op {
            Opcode::CmpI => {
                let (a, b) = (self.int_const(n1)?, self.int_const(n2)?);
                let r = (a as i32).cmp(&(b as i32));
                Some(self.con_i(match r {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                }))
            }
            Opcode::CmpL => {
                let (a, b) = (self.long_const(n1)?, self.long_const(n2)?);
                let r = a.cmp(&b);
                Some(self.con_i(match r {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                }))
            }
            Opcode::CmpF(g) => {
                let a = self.nodes[n1].as_con_f()?;
                let b = self.nodes[n2].as_con_f()?;
                let r = if a.is_nan() || b.is_nan() {
                    if *g { 1 } else { -1 }
                } else {
                    a.partial_cmp(&b).map(|o| match o {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    })?
                };
                Some(self.con_i(r))
            }
            Opcode::CmpD(g) => {
                let a = self.nodes[n1].as_con_d()?;
                let b = self.nodes[n2].as_con_d()?;
                let r = if a.is_nan() || b.is_nan() {
                    if *g { 1 } else { -1 }
                } else {
                    a.partial_cmp(&b).map(|o| match o {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    })?
                };
                Some(self.con_i(r))
            }
            _ => None,
        }
    }

    fn fold_bool_cmp(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        let cond = match op {
            Opcode::BoolCmp(c) => *c,
            _ => return None,
        };
        let (n1, n2) = (inputs[0], inputs[1]);
        if n1 == n2 {
            let r = match cond {
                CmpCondition::Eq | CmpCondition::Le | CmpCondition::Ge => true,
                CmpCondition::Ne | CmpCondition::Lt | CmpCondition::Gt => false,
            };
            return Some(self.bool_const(r));
        }
        let b1 = self.nodes[n1].as_bool();
        let b2 = self.nodes[n2].as_bool();
        if let (Some(x), Some(y)) = (b1, b2) {
            let r = match cond {
                CmpCondition::Eq => x == y,
                CmpCondition::Ne => x != y,
                _ => return None,
            };
            return Some(self.bool_const(r));
        }
        let c1 = self.int_const(n1);
        let c2 = self.int_const(n2);
        if let (Some(a), Some(b)) = (c1, c2) {
            let r = match cond {
                CmpCondition::Eq => a == b,
                CmpCondition::Ne => a != b,
                CmpCondition::Lt => a < b,
                CmpCondition::Le => a <= b,
                CmpCondition::Gt => a > b,
                CmpCondition::Ge => a >= b,
            };
            return Some(self.bool_const(r));
        }
        let p1 = self.nodes[n1].as_con_p();
        let p2 = self.nodes[n2].as_con_p();
        if let (Some(a), Some(b)) = (p1, p2) {
            let r = match cond {
                CmpCondition::Eq => a == b,
                CmpCondition::Ne => a != b,
                _ => return None,
            };
            return Some(self.bool_const(r));
        }
        None
    }

    fn fold_convert(&mut self, op: &Opcode, input: NodeId) -> Option<NodeId> {
        match op {
            Opcode::I2B => {
                let v = self.int_const(input)? as i32;
                Some(self.con_i((v as i8) as i32))
            }
            Opcode::I2C => {
                let v = self.int_const(input)? as i32;
                Some(self.con_i((v as u16) as i32))
            }
            Opcode::I2S => {
                let v = self.int_const(input)? as i32;
                Some(self.con_i((v as i16) as i32))
            }
            Opcode::I2L => {
                let v = self.int_const(input)?;
                Some(self.con_l(v))
            }
            Opcode::L2I => {
                let v = self.long_const(input)?;
                Some(self.con_i(v as i32))
            }
            Opcode::I2F => {
                let v = self.int_const(input)? as i32;
                Some(self.transform(Opcode::ConF((v as f32).to_bits()), vec![]))
            }
            Opcode::I2D => {
                let v = self.int_const(input)? as i32;
                Some(self.transform(Opcode::ConD((v as f64).to_bits()), vec![]))
            }
            Opcode::L2F => {
                let v = self.long_const(input)?;
                Some(self.transform(Opcode::ConF((v as f32).to_bits()), vec![]))
            }
            Opcode::L2D => {
                let v = self.long_const(input)?;
                Some(self.transform(Opcode::ConD((v as f64).to_bits()), vec![]))
            }
            Opcode::F2I => {
                let v = self.nodes[input].as_con_f()?;
                Some(self.con_i(v as i32))
            }
            Opcode::F2L => {
                let v = self.nodes[input].as_con_f()?;
                Some(self.con_l(v as i64))
            }
            Opcode::F2D => {
                let v = self.nodes[input].as_con_f()?;
                Some(self.transform(Opcode::ConD((v as f64).to_bits()), vec![]))
            }
            Opcode::D2I => {
                let v = self.nodes[input].as_con_d()?;
                Some(self.con_i(v as i32))
            }
            Opcode::D2L => {
                let v = self.nodes[input].as_con_d()?;
                Some(self.con_l(v as i64))
            }
            Opcode::D2F => {
                let v = self.nodes[input].as_con_d()?;
                Some(self.transform(Opcode::ConF((v as f32).to_bits()), vec![]))
            }
            _ => None,
        }
    }

    fn fold_binary_fp(&mut self, op: &Opcode, inputs: &[NodeId]) -> Option<NodeId> {
        let (n1, n2) = (inputs[0], inputs[1]);
        let is_f = matches!(
            op,
            Opcode::AddF | Opcode::SubF | Opcode::MulF | Opcode::DivF | Opcode::RemF
        );
        if is_f {
            let a = self.nodes[n1].as_con_f()?;
            let b = self.nodes[n2].as_con_f()?;
            let r = match op {
                Opcode::AddF => a + b,
                Opcode::SubF => a - b,
                Opcode::MulF => a * b,
                Opcode::DivF => {
                    if b == 0.0 {
                        return None;
                    }
                    a / b
                }
                Opcode::RemF => {
                    if b == 0.0 {
                        return None;
                    }
                    a % b
                }
                _ => return None,
            };
            return Some(self.transform(Opcode::ConF(r.to_bits()), vec![]));
        }
        let a = self.nodes[n1].as_con_d()?;
        let b = self.nodes[n2].as_con_d()?;
        let r = match op {
            Opcode::AddD => a + b,
            Opcode::SubD => a - b,
            Opcode::MulD => a * b,
            Opcode::DivD => {
                if b == 0.0 {
                    return None;
                }
                a / b
            }
            Opcode::RemD => {
                if b == 0.0 {
                    return None;
                }
                a % b
            }
            _ => return None,
        };
        Some(self.transform(Opcode::ConD(r.to_bits()), vec![]))
    }
}

impl Default for PhaseGVN {
    fn default() -> Self {
        Self::new()
    }
}
