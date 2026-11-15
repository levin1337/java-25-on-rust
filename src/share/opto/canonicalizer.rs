/*
 * Port of OpenJDK 25 HotSpot C2 canonicalization driver
 * (PhaseCanonicalizer analog of the iterative Ideal/Identity rewrites in
 * phaseX.cpp combined with the local graph reshaping rules from
 * cfgnode.cpp / castnode.cpp / subtypenode.cpp).
 *
 * Each round scans the graph, collects node->replacement mappings and
 * applies them in one pass (the same replace-uses discipline HotSpot
 * uses in PhaseGVN::replace_node).  Implemented rules:
 *
 *   - Phi collapse: all value inputs identical -> the value.
 *   - Region collapse: single distinct control predecessor.
 *   - MergeMem collapse: single distinct memory input.
 *   - Constant branch folding: If with constant test redirects
 *     IfTrue/IfFalse to the taken control or Halt.
 *   - CheckCast of the null constant -> null constant.
 *   - InstanceOf of the null constant -> 0.
 *   - ArrayLength of NewArray/NewArrayRef with constant size -> size.
 *   - Narrowing conversion chains (I2B/I2C/I2S of the same op).
 *   - Identity of a node with itself is never emitted.
 */

use super::builder::CON_P_NULL;
use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode, NONE};
use std::collections::HashMap;

pub struct PhaseCanonicalizer;

impl PhaseCanonicalizer {
    pub fn optimize(gvn: &mut PhaseGVN) -> usize {
        let mut total = 0;
        for _ in 0..8 {
            let reps = Self::scan(gvn);
            if reps.is_empty() {
                break;
            }
            total += reps.len();
            gvn.apply_replacements(&reps);
        }
        total
    }

    fn scan(gvn: &PhaseGVN) -> HashMap<NodeId, NodeId> {
        let mut reps: HashMap<NodeId, NodeId> = HashMap::new();
        let halt = gvn
            .nodes
            .iter()
            .find(|n| n.op == Opcode::Halt)
            .map(|n| n.id);
        for node in gvn.nodes.iter() {
            let id = node.id;
            match &node.op {
                Opcode::Phi => {
                    if let Some(v) = phi_collapse_value(node) {
                        if v != id {
                            reps.insert(id, v);
                        }
                    }
                }
                Opcode::Region => {
                    if let Some(c) = single_distinct(&node.inputs) {
                        if c != id {
                            reps.insert(id, c);
                        }
                    }
                }
                Opcode::MergeMem => {
                    let mems = &node.inputs[..node.inputs.len().saturating_sub(1)];
                    if let Some(m) = single_distinct(mems) {
                        if m != id {
                            reps.insert(id, m);
                        }
                    }
                }
                Opcode::If => {
                    if let Some((ctrl, taken)) = const_branch(gvn, node) {
                        if let Some(h) = halt {
                            for (proj_op, target) in [
                                (Opcode::IfTrue, if taken { ctrl } else { h }),
                                (Opcode::IfFalse, if taken { h } else { ctrl }),
                            ] {
                                if let Some(proj) = find_proj(gvn, id, &proj_op) {
                                    reps.insert(proj, target);
                                }
                            }
                        }
                    }
                }
                Opcode::CheckCast(_) => {
                    if node.inputs.len() >= 3 {
                        if let Some(c) = null_const(gvn, node.inputs[2]) {
                            reps.insert(id, c);
                        }
                    }
                }
                Opcode::InstanceOf(_) => {
                    if node.inputs.len() >= 3 {
                        if null_const(gvn, node.inputs[2]).is_some() {
                            if let Some(z) = find_const_i(gvn, 0) {
                                reps.insert(id, z);
                            }
                        }
                    }
                }
                Opcode::ArrayLength => {
                    if node.inputs.len() >= 3 {
                        if let Some(sz) = array_length_const(gvn, node.inputs[2]) {
                            reps.insert(id, sz);
                        }
                    }
                }
                Opcode::I2B | Opcode::I2C | Opcode::I2S => {
                    if node.inputs.len() == 1 {
                        let inp = node.inputs[0];
                        if inp != NONE && gvn.nodes[inp].op == node.op {
                            reps.insert(id, inp);
                        }
                    }
                }
                _ => {}
            }
        }
        reps.retain(|k, v| k != v && *v != NONE);
        reps
    }
}

fn single_distinct(inputs: &[NodeId]) -> Option<NodeId> {
    let mut first: Option<NodeId> = None;
    for &i in inputs {
        if i == NONE {
            continue;
        }
        match first {
            None => first = Some(i),
            Some(f) if f == i => {}
            Some(_) => return None,
        }
    }
    first
}

fn phi_collapse_value(node: &super::node::Node) -> Option<NodeId> {
    let n = node.inputs.len();
    if n <= 1 {
        return None;
    }
    let values = &node.inputs[..n - 1];
    single_distinct(values)
}

fn const_branch(gvn: &PhaseGVN, node: &super::node::Node) -> Option<(NodeId, bool)> {
    if node.inputs.len() < 2 {
        return None;
    }
    let test = node.inputs[1];
    if test == NONE {
        return None;
    }
    let tn = &gvn.nodes[test];
    match tn.op {
        Opcode::Bool(b) => Some((node.inputs[0], b)),
        Opcode::ConI(v) => Some((node.inputs[0], v != 0)),
        _ => None,
    }
}

fn find_proj(gvn: &PhaseGVN, if_id: NodeId, op: &Opcode) -> Option<NodeId> {
    for n in gvn.nodes.iter() {
        if n.op == *op && n.inputs.first().copied() == Some(if_id) {
            return Some(n.id);
        }
    }
    None
}

fn null_const(gvn: &PhaseGVN, id: NodeId) -> Option<NodeId> {
    if id == NONE {
        return None;
    }
    match gvn.nodes[id].op {
        Opcode::ConP(p) if p == CON_P_NULL => Some(id),
        _ => None,
    }
}

fn find_const_i(gvn: &PhaseGVN, v: i32) -> Option<NodeId> {
    gvn.nodes
        .iter()
        .find(|n| n.op == Opcode::ConI(v))
        .map(|n| n.id)
}

fn array_length_const(gvn: &PhaseGVN, arr: NodeId) -> Option<NodeId> {
    if arr == NONE {
        return None;
    }
    let an = &gvn.nodes[arr];
    match an.op {
        Opcode::NewArray(_) | Opcode::NewArrayRef(_) => {
            if an.inputs.len() >= 3 {
                let sz = an.inputs[2];
                if sz != NONE {
                    if let Opcode::ConI(v) = gvn.nodes[sz].op {
                        return find_const_i(gvn, v);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::opto::gvn::PhaseGVN;
    use crate::share::opto::node::Opcode;

    #[test]
    fn phi_collapse() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let region = gvn.alloc(Opcode::Region, vec![start, start]);
        let v = gvn.transform(Opcode::ConI(9), vec![]);
        let phi = gvn.alloc(Opcode::Phi, vec![v, v, region]);
        let user = gvn.alloc(Opcode::AddI, vec![phi, phi]);
        let n = PhaseCanonicalizer::optimize(&mut gvn);
        assert!(n > 0);
        assert_eq!(gvn.node(user).inputs, vec![v, v]);
    }

    #[test]
    fn const_branch_fold() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let start2 = gvn.alloc(Opcode::Start, vec![]);
        let halt = gvn.alloc(Opcode::Halt, vec![]);
        let _ = halt;
        let region = gvn.alloc(Opcode::Region, vec![start, start2]);
        let test = gvn.transform(Opcode::Bool(true), vec![]);
        let ifn = gvn.alloc(Opcode::If, vec![region, test]);
        let ift = gvn.alloc(Opcode::IfTrue, vec![ifn, start]);
        let iff = gvn.alloc(Opcode::IfFalse, vec![ifn, start]);
        let use_true = gvn.alloc(Opcode::Region, vec![ift]);
        let use_false = gvn.alloc(Opcode::Region, vec![iff]);
        PhaseCanonicalizer::optimize(&mut gvn);
        assert_eq!(gvn.node(use_true).inputs[0], region);
        assert_ne!(gvn.node(use_false).inputs[0], iff);
    }

    #[test]
    fn instanceof_null() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let region = gvn.alloc(Opcode::Region, vec![start]);
        let null = gvn.con_p(CON_P_NULL);
        let zero = gvn.con_i(0);
        let io = gvn.alloc(Opcode::InstanceOf(7), vec![start, region, null]);
        let user = gvn.alloc(Opcode::AddI, vec![io, io]);
        PhaseCanonicalizer::optimize(&mut gvn);
        assert_eq!(gvn.node(user).inputs, vec![zero, zero]);
    }
}
