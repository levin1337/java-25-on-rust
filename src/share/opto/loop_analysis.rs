/*
 * Port of the loop analysis foundation of OpenJDK 25 HotSpot C2
 * (share/opto/loopnode.cpp LoopNode tree construction, domgraph.cpp
 * dominator computation) reduced to the Sea-of-Nodes control graph this
 * port builds: control edges are recovered from the fixed input
 * positions the bytecode builder uses (Region preds, If/IfTrue/IfFalse,
 * MergeMem region input, Phi region input, ctrl slots of effectful
 * nodes).
 *
 * Produces the natural-loop forest (LoopTree) with headers, latches,
 * bodies, nesting parents and depths — the same structure
 * PhaseIdealLoop consumes for induction-variable analysis, loop
 * unswitching, peeling and LICM.
 */

use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode, NONE};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

#[derive(Clone, Debug)]
pub struct LoopInfo {
    pub header: NodeId,
    pub latch: NodeId,
    pub body: BTreeSet<NodeId>,
    pub parent: Option<usize>,
    pub depth: usize,
    pub exits: Vec<NodeId>,
}

#[derive(Clone, Debug, Default)]
pub struct LoopTree {
    pub loops: Vec<LoopInfo>,
}

impl LoopTree {
    pub fn innermost(&self, node: NodeId) -> Option<usize> {
        self.loops
            .iter()
            .enumerate()
            .filter(|(_, l)| l.body.contains(&node))
            .max_by_key(|(_, l)| l.depth)
            .map(|(i, _)| i)
    }

    pub fn is_in_loop(&self, node: NodeId) -> bool {
        self.loops.iter().any(|l| l.body.contains(&node))
    }

    pub fn containing(&self, node: NodeId) -> Vec<usize> {
        self.loops
            .iter()
            .enumerate()
            .filter(|(_, l)| l.body.contains(&node))
            .map(|(i, _)| i)
            .collect()
    }
}

pub struct ControlGraph {
    pub preds: HashMap<NodeId, Vec<NodeId>>,
    pub succs: HashMap<NodeId, Vec<NodeId>>,
    pub nodes: BTreeSet<NodeId>,
    pub entry: NodeId,
}

fn control_input_indices(op: &Opcode, ninputs: usize) -> Vec<usize> {
    use Opcode::*;
    match op {
        Region => (0..ninputs).collect(),
        MergeMem | Phi => {
            if ninputs == 0 {
                Vec::new()
            } else {
                vec![ninputs - 1]
            }
        }
        If | IfTrue | IfFalse | IfException => vec![0],
        Return => vec![0],
        Athrow => {
            if ninputs > 1 {
                vec![1]
            } else {
                Vec::new()
            }
        }
        LoadI | LoadL | LoadF | LoadD | LoadP | LoadB | LoadC | LoadS | StoreI | StoreL
        | StoreF | StoreD | StoreP | StoreB | StoreC | StoreS | Call(_) | New(_)
        | NewArray(_) | NewArrayRef(_) | MultianewArray | CheckCast(_)
        | InstanceOf(_) | ArrayLength | MonitorEnter | MonitorExit => {
            if ninputs > 1 {
                vec![1]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

impl ControlGraph {
    pub fn build(gvn: &PhaseGVN) -> Self {
        let mut preds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut succs: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut nodes: BTreeSet<NodeId> = BTreeSet::new();
        for node in gvn.nodes.iter() {
            let idxs = control_input_indices(&node.op, node.inputs.len());
            for i in idxs {
                let p = node.inputs[i];
                if p == NONE || p >= gvn.node_count() {
                    continue;
                }
                preds.entry(node.id).or_default().push(p);
                succs.entry(p).or_default().push(node.id);
                nodes.insert(p);
                nodes.insert(node.id);
            }
        }
        if gvn.start != NONE {
            nodes.insert(gvn.start);
        }
        Self {
            preds,
            succs,
            nodes,
            entry: gvn.start,
        }
    }

    pub fn reachable_from_entry(&self) -> HashSet<NodeId> {
        let mut seen = HashSet::new();
        if self.entry == NONE {
            return seen;
        }
        let mut q = VecDeque::new();
        q.push_back(self.entry);
        seen.insert(self.entry);
        while let Some(n) = q.pop_front() {
            if let Some(ss) = self.succs.get(&n) {
                for &s in ss {
                    if seen.insert(s) {
                        q.push_back(s);
                    }
                }
            }
        }
        seen
    }

    pub fn dominators(&self) -> HashMap<NodeId, HashSet<NodeId>> {
        let reachable = self.reachable_from_entry();
        let mut dom: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();
        let all: HashSet<NodeId> = reachable.iter().copied().collect();
        for &n in &reachable {
            if n == self.entry {
                let mut s = HashSet::new();
                s.insert(n);
                dom.insert(n, s);
            } else {
                dom.insert(n, all.clone());
            }
        }
        loop {
            let mut changed = false;
            for &n in &reachable {
                if n == self.entry {
                    continue;
                }
                let mut new_set: Option<HashSet<NodeId>> = None;
                if let Some(ps) = self.preds.get(&n) {
                    for &p in ps {
                        if !reachable.contains(&p) {
                            continue;
                        }
                        match &new_set {
                            None => new_set = Some(dom.get(&p).cloned().unwrap_or_default()),
                            Some(cur) => {
                                let pd = dom.get(&p).cloned().unwrap_or_default();
                                let inter: HashSet<NodeId> =
                                    cur.intersection(&pd).copied().collect();
                                new_set = Some(inter);
                            }
                        }
                    }
                }
                let mut new_set = new_set.unwrap_or_default();
                new_set.insert(n);
                if dom.get(&n) != Some(&new_set) {
                    dom.insert(n, new_set);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        dom
    }

    pub fn back_edges(&self, dom: &HashMap<NodeId, HashSet<NodeId>>) -> Vec<(NodeId, NodeId)> {
        let mut out = Vec::new();
        for (&n, ss) in &self.succs {
            for &s in ss {
                if dom.get(&n).map_or(false, |dn| dn.contains(&s)) {
                    out.push((n, s));
                }
            }
        }
        out
    }
}

pub struct LoopAnalysis;

impl LoopAnalysis {
    pub fn analyze(gvn: &PhaseGVN) -> LoopTree {
        let cfg = ControlGraph::build(gvn);
        let dom = cfg.dominators();
        let mut loops: Vec<LoopInfo> = Vec::new();
        let mut seen_headers: HashSet<NodeId> = HashSet::new();

        let mut back_edges = cfg.back_edges(&dom);
        back_edges.sort();
        back_edges.dedup();

        for (latch, header) in back_edges {
            if header == cfg.entry || !seen_headers.insert(header) {
                continue;
            }
            let body = natural_loop_body(&cfg, header, latch);
            let mut exits = Vec::new();
            for &m in &body {
                if let Some(ss) = cfg.succs.get(&m) {
                    for &s in ss {
                        if !body.contains(&s) {
                            exits.push(s);
                        }
                    }
                }
            }
            exits.sort();
            exits.dedup();
            loops.push(LoopInfo {
                header,
                latch,
                body,
                parent: None,
                depth: 0,
                exits,
            });
        }

        loops.sort_by_key(|l| l.header);
        assign_nesting(&mut loops);
        LoopTree { loops }
    }
}

fn natural_loop_body(cfg: &ControlGraph, header: NodeId, latch: NodeId) -> BTreeSet<NodeId> {
    let mut body = BTreeSet::new();
    body.insert(header);
    if latch == header {
        return body;
    }
    body.insert(latch);
    let mut stack = vec![latch];
    while let Some(n) = stack.pop() {
        if let Some(ps) = cfg.preds.get(&n) {
            for &p in ps {
                if p == header {
                    continue;
                }
                if body.insert(p) {
                    stack.push(p);
                }
            }
        }
    }
    body
}

pub fn loop_invariant_nodes(gvn: &PhaseGVN, body: &BTreeSet<NodeId>) -> HashSet<NodeId> {
    let mut inv: HashSet<NodeId> = HashSet::new();
    loop {
        let mut changed = false;
        for &n in body {
            if inv.contains(&n) {
                continue;
            }
            let node = &gvn.nodes[n];
            let pure = !node.op.is_pinned() || matches!(node.op, Opcode::Phi);
            if !pure {
                continue;
            }
            let all_inv = node.inputs.iter().all(|&i| {
                i == NONE
                    || i >= gvn.node_count()
                    || !body.contains(&i)
                    || inv.contains(&i)
                    || gvn.nodes[i].op.is_const()
            });
            if all_inv {
                inv.insert(n);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    inv
}

fn assign_nesting(loops: &mut Vec<LoopInfo>) {
    for i in 0..loops.len() {
        let mut best: Option<usize> = None;
        for j in 0..loops.len() {
            if i == j {
                continue;
            }
            if loops[j].body.contains(&loops[i].header)
                && loops[j].body.len() > loops[i].body.len()
            {
                let dominated = match best {
                    None => true,
                    Some(b) => loops[j].body.len() < loops[b].body.len(),
                };
                if dominated {
                    best = Some(j);
                }
            }
        }
        loops[i].parent = best;
    }
    for i in 0..loops.len() {
        let mut d = 0;
        let mut p = loops[i].parent;
        while let Some(pi) = p {
            d += 1;
            p = loops[pi].parent;
        }
        loops[i].depth = d;
    }
    loops.sort_by(|a, b| b.depth.cmp(&a.depth).then(a.header.cmp(&b.header)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::opto::gvn::PhaseGVN;
    use crate::share::opto::node::Opcode;

    #[test]
    fn detects_simple_loop() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        gvn.start = start;
        let c0 = gvn.transform(Opcode::ConI(0), vec![]);
        let c1 = gvn.transform(Opcode::ConI(1), vec![]);
        let test_pre = gvn.transform(Opcode::BoolCmp(crate::share::opto::node::CmpCondition::Lt), vec![c0, c1]);
        let if_pre = gvn.alloc(Opcode::If, vec![start, test_pre]);
        let body_ctrl = gvn.alloc(Opcode::IfTrue, vec![if_pre, start]);
        let header = gvn.alloc(Opcode::Region, vec![body_ctrl, NONE]);
        let phi = gvn.alloc(Opcode::Phi, vec![c0, c1, header]);
        let _ = phi;
        let test = gvn.transform(Opcode::BoolCmp(crate::share::opto::node::CmpCondition::Lt), vec![c1, c0]);
        let ifn = gvn.alloc(Opcode::If, vec![header, test]);
        let latch_t = gvn.alloc(Opcode::IfTrue, vec![ifn, start]);
        let latch_f = gvn.alloc(Opcode::IfFalse, vec![ifn, start]);
        gvn.node_mut(header).inputs[1] = latch_t;
        let mut phi_inputs = gvn.node(phi).inputs.clone();
        phi_inputs[1] = c1;
        let _ = phi_inputs;
        let _ = latch_f;
        let tree = LoopAnalysis::analyze(&gvn);
        assert_eq!(tree.loops.len(), 1);
        let l = &tree.loops[0];
        assert_eq!(l.header, header);
        assert!(l.body.contains(&header));
        assert!(l.body.contains(&latch_t));
        assert!(l.body.contains(&ifn));
    }

    #[test]
    fn acyclic_no_loops() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        gvn.start = start;
        let a = gvn.alloc(Opcode::ConI(1), vec![]);
        let b = gvn.alloc(Opcode::AddI, vec![a, a]);
        let _ = b;
        let tree = LoopAnalysis::analyze(&gvn);
        assert!(tree.loops.is_empty());
    }
}
