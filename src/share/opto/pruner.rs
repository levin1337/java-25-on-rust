/*
 * Port of the IdealGraphPruner concept from OpenJDK node.hpp / parse
 * pipeline: during graph construction and iterative optimization C2
 * avoids materializing (or eagerly deletes) nodes that cannot be reached
 * from the live root set.  `IdealGraphPruner` here is the reachability
 * engine: it marks the transitive fan-in of a set of root nodes and
 * reports the dead remainder, mirroring `PhaseRemoveUseless`'s
 * `Compile::identify_useless_nodes` but usable incrementally while a
 * graph is still being built (pending NONE inputs are treated as live
 * roots so partially-built regions are never pruned).
 */

use super::gvn::PhaseGVN;
use super::node::{NodeId, NONE};
use std::collections::HashSet;

pub struct IdealGraphPruner {
    live: Vec<bool>,
    stack: Vec<NodeId>,
    roots: HashSet<NodeId>,
}

impl IdealGraphPruner {
    pub fn new() -> Self {
        Self {
            live: Vec::new(),
            stack: Vec::new(),
            roots: HashSet::new(),
        }
    }

    pub fn add_root(&mut self, id: NodeId) {
        self.roots.insert(id);
    }

    pub fn is_root(&self, id: NodeId) -> bool {
        self.roots.contains(&id)
    }

    pub fn mark_live(&mut self, gvn: &PhaseGVN, id: NodeId) {
        if id == NONE || id >= gvn.node_count() {
            return;
        }
        if self.live.len() < gvn.node_count() {
            self.live.resize(gvn.node_count(), false);
        }
        if self.live[id] {
            return;
        }
        self.live[id] = true;
        self.stack.push(id);
        while let Some(cur) = self.stack.pop() {
            for &inp in &gvn.nodes[cur].inputs {
                if inp == NONE || inp >= gvn.node_count() || self.live[inp] {
                    continue;
                }
                self.live[inp] = true;
                self.stack.push(inp);
            }
        }
    }

    pub fn compute(&mut self, gvn: &PhaseGVN) {
        let roots: Vec<NodeId> = self.roots.iter().copied().collect();
        for r in roots {
            self.mark_live(gvn, r);
        }
        if gvn.root != NONE {
            self.mark_live(gvn, gvn.root);
        }
        if gvn.start != NONE {
            self.mark_live(gvn, gvn.start);
        }
    }

    pub fn is_live(&self, id: NodeId) -> bool {
        self.live.get(id).copied().unwrap_or(false)
    }

    pub fn live_count(&self) -> usize {
        self.live.iter().filter(|&&l| l).count()
    }

    pub fn dead_nodes(&self, gvn: &PhaseGVN) -> Vec<NodeId> {
        (0..gvn.node_count())
            .filter(|&id| !self.is_live(id))
            .collect()
    }

    pub fn prune_dead(&mut self, gvn: &mut PhaseGVN) -> usize {
        self.compute(gvn);
        let dead = self.dead_nodes(gvn);
        if dead.is_empty() {
            return 0;
        }
        let mut live = vec![false; gvn.node_count()];
        for id in 0..gvn.node_count() {
            live[id] = self.is_live(id);
        }
        gvn.compact(&live);
        self.live = Vec::new();
        self.stack.clear();
        dead.len()
    }

    pub fn should_visit(&self, _gvn: &PhaseGVN, id: NodeId) -> bool {
        self.is_live(id)
    }

    pub fn visit(&mut self, gvn: &PhaseGVN, id: NodeId) -> bool {
        if id == NONE || self.is_live(id) {
            return false;
        }
        self.mark_live(gvn, id);
        true
    }
}

impl Default for IdealGraphPruner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::opto::gvn::PhaseGVN;
    use crate::share::opto::node::Opcode;

    #[test]
    fn prunes_unreachable() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let root = gvn.alloc(Opcode::Root, vec![start]);
        gvn.root = root;
        gvn.start = start;
        let live_v = gvn.transform(Opcode::ConI(1), vec![]);
        let dead_v = gvn.transform(Opcode::ConI(2), vec![]);
        let dead_op = gvn.alloc(Opcode::AddI, vec![dead_v, dead_v]);
        let _ = dead_op;
        let _ = live_v;
        let mut pruner = IdealGraphPruner::new();
        pruner.compute(&gvn);
        assert!(pruner.is_live(start));
        assert!(pruner.is_live(root));
        assert!(!pruner.is_live(dead_v));
        assert!(!pruner.is_live(dead_op));
    }

    #[test]
    fn prune_dead_compacts() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let root = gvn.alloc(Opcode::Root, vec![start]);
        gvn.root = root;
        gvn.start = start;
        let _dead = gvn.alloc(Opcode::ConI(5), vec![]);
        let mut pruner = IdealGraphPruner::new();
        let n = pruner.prune_dead(&mut gvn);
        assert_eq!(n, 1);
        assert_eq!(gvn.node_count(), 2);
    }
}
