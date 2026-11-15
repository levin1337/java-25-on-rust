/*
 * Port of OpenJDK 25 HotSpot C2 liveness and interference-graph front
 * end (share/opto/live.cpp, ifg.cpp, chaitin.cpp entry points) reduced
 * to the scheduled block structure produced by scheduler.rs.
 *
 * Computes, for every ideal node, the set of blocks where its value is
 * live (backward dataflow over the schedule's RPO block order, with
 * phi uses attributed to the corresponding predecessor edge exactly
 * like Live::add_use_edge / Live::compute_uses in live.cpp), then
 * derives linear live intervals over the scheduled instruction stream
 * and the interference relation between overlapping intervals —
 * the same quantities PhaseLIVE and PhaseInterferenceGraph produce in
 * HotSpot.  Exposed directly for the register allocator and tests.
 */

use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode, NONE};
use super::scheduler::Schedule;
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug, Default)]
pub struct LiveInfo {
    /// Blocks (schedule indices) where the node is live-in or live-through.
    pub live_blocks: BTreeSet<usize>,
    /// Block where the node is defined (first def), if scheduled.
    pub def_block: Option<usize>,
    /// Blocks where the node is used before being re-defined.
    pub use_blocks: BTreeSet<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct InterferenceGraph {
    /// node -> set of nodes whose live ranges intersect.
    pub edges: HashMap<NodeId, HashSet<NodeId>>,
    pub node_count: usize,
}

impl InterferenceGraph {
    pub fn interferes(&self, a: NodeId, b: NodeId) -> bool {
        self.edges.get(&a).map_or(false, |s| s.contains(&b))
    }

    pub fn neighbors(&self, n: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.edges.get(&n).into_iter().flatten().copied()
    }

    pub fn degree(&self, n: NodeId) -> usize {
        self.edges.get(&n).map_or(0, |s| s.len())
    }
}

pub struct PhaseLive;

impl PhaseLive {
    pub fn analyze(gvn: &PhaseGVN, schedule: &Schedule) -> LiveInfoMap {
        let n = gvn.node_count();
        let mut use_sets: Vec<HashSet<NodeId>> = vec![HashSet::new(); schedule.blocks.len()];
        let mut def_sets: Vec<HashSet<NodeId>> = vec![HashSet::new(); schedule.blocks.len()];
        let mut def_block: HashMap<NodeId, usize> = HashMap::new();

        for (bi, block) in schedule.blocks.iter().enumerate() {
            let mut defined: HashSet<NodeId> = HashSet::new();
            for &node in &block.instrs {
                record_use_def(gvn, node, bi, &mut use_sets, &mut def_sets, &mut defined);
            }
            if let Some(t) = &block.term {
                record_use_def(gvn, t.node, bi, &mut use_sets, &mut def_sets, &mut defined);
            }
            for &node in &block.instrs {
                if defined.contains(&node) && !def_block.contains_key(&node) {
                    def_block.insert(node, bi);
                }
            }
            if let Some(t) = &block.term {
                if defined.contains(&t.node) && !def_block.contains_key(&t.node) {
                    def_block.insert(t.node, bi);
                }
            }
        }

        record_phi_edge_uses(gvn, schedule, &mut use_sets);

        let live_in = backward_liveness(schedule, &use_sets, &def_sets);

        let mut info = LiveInfoMap {
            info: vec![LiveInfo::default(); n],
        };
        for bi in 0..schedule.blocks.len() {
            for &node in live_in[bi].iter() {
                if node < n {
                    info.info[node].live_blocks.insert(bi);
                }
            }
            for &node in use_sets[bi].iter() {
                if node < n {
                    info.info[node].use_blocks.insert(bi);
                }
            }
        }
        for (node, bi) in def_block {
            if node < n {
                info.info[node].def_block = Some(bi);
            }
        }
        info
    }

    pub fn interference(gvn: &PhaseGVN, schedule: &Schedule) -> InterferenceGraph {
        const SPAN: i64 = 1_000_000;
        let pos = |bi: usize, idx: i64| bi as i64 * SPAN + idx;
        let mut def_pos: HashMap<NodeId, i64> = HashMap::new();
        let mut use_pos: HashMap<NodeId, i64> = HashMap::new();
        for (bi, block) in schedule.blocks.iter().enumerate() {
            for (ii, &node) in block.instrs.iter().enumerate() {
                let p = pos(bi, ii as i64);
                if node < gvn.node_count() && !gvn.nodes[node].op.is_const() {
                    def_pos.entry(node).or_insert(p);
                }
                note_use(gvn, node, p, &mut use_pos);
            }
            if let Some(t) = &block.term {
                let p = pos(bi, SPAN - 1);
                if t.node < gvn.node_count() && !gvn.nodes[t.node].op.is_const() {
                    def_pos.entry(t.node).or_insert(p);
                }
                note_use(gvn, t.node, p, &mut use_pos);
            }
        }
        for (phi, region) in &schedule.phi_region {
            if *phi >= gvn.node_count() || *region >= gvn.node_count() {
                continue;
            }
            let pnode = &gvn.nodes[*phi];
            let rnode = &gvn.nodes[*region];
            let nvalues = pnode.inputs.len().saturating_sub(1);
            for i in 0..nvalues.min(rnode.inputs.len()) {
                let value = pnode.inputs[i];
                let ctrl = rnode.inputs[i];
                if value == NONE || ctrl == NONE || gvn.nodes[value].op.is_const() {
                    continue;
                }
                if let Some(&bi) = schedule.entry_block.get(&ctrl) {
                    let p = pos(bi, SPAN - 1);
                    let e = use_pos.entry(value).or_insert(i64::MIN);
                    if p > *e {
                        *e = p;
                    }
                }
            }
        }
        let mut edges: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();
        let mut intervals: Vec<(NodeId, i64, i64)> = Vec::new();
        for node in 0..gvn.node_count() {
            let n = &gvn.nodes[node];
            if n.op.is_pinned() && !matches!(n.op, Opcode::Phi) {
                continue;
            }
            if n.op.is_const() {
                continue;
            }
            let last_use = use_pos.get(&node).copied();
            let d = def_pos.get(&node).copied();
            let (start, end) = match (d, last_use) {
                (Some(s), Some(e)) => (s, e),
                (Some(s), None) => (s, s),
                (None, Some(e)) => {
                    let first_use = use_pos_first(gvn, schedule, node);
                    (first_use.unwrap_or(e), e)
                }
                (None, None) => continue,
            };
            if end < start {
                continue;
            }
            intervals.push((node, start, end));
        }
        for i in 0..intervals.len() {
            for j in i + 1..intervals.len() {
                let (a, sa, ea) = intervals[i];
                let (b, sb, eb) = intervals[j];
                if sa <= eb && sb <= ea {
                    edges.entry(a).or_default().insert(b);
                    edges.entry(b).or_default().insert(a);
                }
            }
        }
        InterferenceGraph {
            edges,
            node_count: gvn.node_count(),
        }
    }
}

pub struct LiveInfoMap {
    pub info: Vec<LiveInfo>,
}

impl LiveInfoMap {
    pub fn of(&self, node: NodeId) -> &LiveInfo {
        static EMPTY: LiveInfo = LiveInfo {
            live_blocks: BTreeSet::new(),
            def_block: None,
            use_blocks: BTreeSet::new(),
        };
        self.info.get(node).unwrap_or(&EMPTY)
    }
}

fn note_use(gvn: &PhaseGVN, user: NodeId, upos: i64, use_pos: &mut HashMap<NodeId, i64>) {
    if user == NONE || user >= gvn.node_count() {
        return;
    }
    for &inp in &gvn.nodes[user].inputs {
        if inp == NONE || inp >= gvn.node_count() || gvn.nodes[inp].op.is_const() {
            continue;
        }
        let e = use_pos.entry(inp).or_insert(i64::MIN);
        if upos > *e {
            *e = upos;
        }
    }
}

fn use_pos_first(gvn: &PhaseGVN, schedule: &Schedule, node: NodeId) -> Option<i64> {
    const SPAN: i64 = 1_000_000;
    let mut best: Option<i64> = None;
    for (bi, block) in schedule.blocks.iter().enumerate() {
        for (ii, &n) in block.instrs.iter().enumerate() {
            if n != NONE
                && n < gvn.node_count()
                && gvn.nodes[n].inputs.contains(&node)
            {
                let p = bi as i64 * SPAN + ii as i64;
                if best.map_or(true, |b| p < b) {
                    best = Some(p);
                }
            }
        }
        if let Some(t) = &block.term {
            if t.node != NONE
                && t.node < gvn.node_count()
                && gvn.nodes[t.node].inputs.contains(&node)
            {
                let p = bi as i64 * SPAN + SPAN - 1;
                if best.map_or(true, |b| p < b) {
                    best = Some(p);
                }
            }
        }
    }
    best
}

fn record_use_def(
    gvn: &PhaseGVN,
    node: NodeId,
    bi: usize,
    use_sets: &mut [HashSet<NodeId>],
    def_sets: &mut [HashSet<NodeId>],
    defined: &mut HashSet<NodeId>,
) {
    if node == NONE || node >= gvn.node_count() {
        return;
    }
    for &inp in &gvn.nodes[node].inputs {
        if inp == NONE
            || inp >= gvn.node_count()
            || gvn.nodes[inp].op.is_const()
        {
            continue;
        }
        if !defined.contains(&inp) {
            use_sets[bi].insert(inp);
        }
    }
    if !gvn.nodes[node].op.is_const() {
        def_sets[bi].insert(node);
        defined.insert(node);
    }
}

fn record_phi_edge_uses(
    gvn: &PhaseGVN,
    schedule: &Schedule,
    use_sets: &mut [HashSet<NodeId>],
) {
    for (phi, region) in &schedule.phi_region {
        if *phi >= gvn.node_count() || *region >= gvn.node_count() {
            continue;
        }
        let pnode = &gvn.nodes[*phi];
        let rnode = &gvn.nodes[*region];
        let nvalues = pnode.inputs.len().saturating_sub(1);
        for i in 0..nvalues.min(rnode.inputs.len()) {
            let value = pnode.inputs[i];
            let ctrl = rnode.inputs[i];
            if value == NONE || ctrl == NONE {
                continue;
            }
            if let Some(&bi) = schedule.entry_block.get(&ctrl) {
                use_sets[bi].insert(value);
            }
        }
    }
}

fn backward_liveness(
    schedule: &Schedule,
    use_sets: &[HashSet<NodeId>],
    def_sets: &[HashSet<NodeId>],
) -> Vec<HashSet<NodeId>> {
    let nb = schedule.blocks.len();
    let mut live_in: Vec<HashSet<NodeId>> = vec![HashSet::new(); nb];
    let mut live_out: Vec<HashSet<NodeId>> = vec![HashSet::new(); nb];
    for _ in 0..nb * 4 + 8 {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut out: HashSet<NodeId> = HashSet::new();
            for e in &schedule.blocks[bi].succs {
                out.extend(live_in[e.block].iter().copied());
            }
            let mut inn: HashSet<NodeId> = use_sets[bi].clone();
            for &d in &out {
                if !def_sets[bi].contains(&d) {
                    inn.insert(d);
                }
            }
            if out != live_out[bi] || inn != live_in[bi] {
                live_out[bi] = out;
                live_in[bi] = inn;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    live_in
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::opto::gvn::PhaseGVN;
    use crate::share::opto::node::Opcode;
    use crate::share::opto::scheduler::Scheduler;

    #[test]
    fn liveness_across_blocks() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        gvn.start = start;
        let root = gvn.alloc(Opcode::Root, vec![start]);
        gvn.root = root;
        let v = gvn.transform(Opcode::ConI(3), vec![]);
        let add = gvn.alloc(Opcode::AddI, vec![v, v]);
        let ret = gvn.alloc(Opcode::Return, vec![start, start, add]);
        gvn.node_mut(root).inputs.push(ret);
        let sched = Scheduler::new(&gvn).schedule();
        assert!(sched.unsupported.is_empty());
        assert_eq!(sched.blocks.len(), 1);
        let live = PhaseLive::analyze(&gvn, &sched);
        assert!(live.of(add).use_blocks.contains(&0));
        assert!(live.of(add).live_blocks.contains(&0));
        let ifg = PhaseLive::interference(&gvn, &sched);
        assert_eq!(ifg.node_count, gvn.node_count());
    }

    #[test]
    fn interference_empty_on_tiny_graph() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        gvn.start = start;
        let root = gvn.alloc(Opcode::Root, vec![start]);
        gvn.root = root;
        let sched = Scheduler::new(&gvn).schedule();
        let ifg = PhaseLive::interference(&gvn, &sched);
        assert_eq!(ifg.node_count, gvn.node_count());
    }
}
