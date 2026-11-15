/*
 * Port of OpenJDK 25 HotSpot C2 scheduler front-end (share/opto/schedule.cpp,
 * block.cpp) reduced to the CFG/liveness structure needed by the Rust C2
 * backend.
 *
 * The ideal graph is a sea-of-nodes graph.  This module extracts the control
 * DAG (Start / Region / IfTrue / IfFalse entries, If/Return/Athrow terminals),
 * orders memory and control effects inside each block, computes reverse
 * postorder and loop backedges, and records phi-copy edges.  Pure nodes are
 * deliberately *not* scheduled here: they are selected on demand by the
 * matcher/code generator, which is the C2 "late scheduling" model for this
 * subset.
 */

use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode, NONE};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermKind {
    If,
    Return,
    Athrow,
    Jump,
    Halt,
}

#[derive(Clone, Debug)]
pub struct Terminal {
    pub kind: TermKind,
    pub node: NodeId,
}

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    /// Successor block id.
    pub block: usize,
    /// Control node identifying this edge.  For a Region successor this is
    /// the predecessor control node stored in `Region.inputs`; for an If
    /// successor it is the IfTrue/IfFalse projection node.
    pub ctrl: NodeId,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: usize,
    /// Control point that begins the block (Start, Region, IfTrue, IfFalse).
    pub entry: NodeId,
    /// Memory state at block entry (Start, IfTrue/IfFalse mem input, or the
    /// MergeMem belonging to a Region).
    pub entry_mem: NodeId,
    /// Effect nodes in scheduled order.  With the executable subset enabled
    /// this is normally empty; loads/stores are kept in memory-chain order.
    pub instrs: Vec<NodeId>,
    pub term: Option<Terminal>,
    pub succs: Vec<Edge>,
    /// Control token at the end of the block for fallthrough edges.
    pub exit_ctrl: NodeId,
    pub rpo: usize,
    /// True when this block is the target of a loop backedge.
    pub loop_header: bool,
}

#[derive(Clone, Debug)]
pub struct Schedule {
    pub blocks: Vec<Block>,
    /// Block id for each control-point node (Start/Region/IfTrue/IfFalse).
    pub entry_block: HashMap<NodeId, usize>,
    /// MergeMem node for each Region entry, if present.
    pub region_mem: HashMap<NodeId, NodeId>,
    /// Region node for each Phi node (last input).
    pub phi_region: HashMap<NodeId, NodeId>,
    pub unsupported: Vec<String>,
}

pub fn is_control_entry(op: &Opcode) -> bool {
    matches!(
        op,
        Opcode::Start | Opcode::Region | Opcode::IfTrue | Opcode::IfFalse | Opcode::IfException
    )
}

/// Memory input index for nodes that carry a memory edge, or None.
pub fn mem_index(op: &Opcode) -> Option<usize> {
    use Opcode::*;
    match op {
        LoadI | LoadL | LoadF | LoadD | LoadP | LoadB | LoadC | LoadS
        | StoreI | StoreL | StoreF | StoreD | StoreP | StoreB | StoreC | StoreS
        | New(_) | NewArray(_) | NewArrayRef(_) | Call(_) | CheckCast(_) | InstanceOf(_)
        | MonitorEnter | MonitorExit | Athrow => Some(0),
        _ => None,
    }
}

/// Control input index for nodes that are control dependent, or None.
pub fn ctrl_index(op: &Opcode) -> Option<usize> {
    use Opcode::*;
    match op {
        If | IfTrue | IfFalse | Return => Some(0),
        Athrow => Some(1),
        LoadI | LoadL | LoadF | LoadD | LoadP | LoadB | LoadC | LoadS
        | StoreI | StoreL | StoreF | StoreD | StoreP | StoreB | StoreC | StoreS
        | New(_) | NewArray(_) | NewArrayRef(_) | Call(_) | CheckCast(_) | InstanceOf(_)
        | MonitorEnter | MonitorExit | ArrayLength => Some(1),
        _ => None,
    }
}

/// True when the node produces a new memory state (its id becomes the mem
/// token for following effects).  Stores update memory but not control.
pub fn produces_mem(op: &Opcode) -> bool {
    matches!(
        op,
        Opcode::StoreI
            | Opcode::StoreL
            | Opcode::StoreF
            | Opcode::StoreD
            | Opcode::StoreP
            | Opcode::StoreB
            | Opcode::StoreC
            | Opcode::StoreS
            | Opcode::New(_)
            | Opcode::NewArray(_)
            | Opcode::NewArrayRef(_)
            | Opcode::Call(_)
    )
}

/// True when the node produces a value that must occupy a machine location.
pub fn produces_value(op: &Opcode) -> bool {
    matches!(
        op,
        Opcode::Phi
            | Opcode::LoadI
            | Opcode::LoadL
            | Opcode::LoadF
            | Opcode::LoadD
            | Opcode::LoadP
            | Opcode::LoadB
            | Opcode::LoadC
            | Opcode::LoadS
            | Opcode::New(_)
            | Opcode::NewArray(_)
            | Opcode::NewArrayRef(_)
            | Opcode::CheckCast(_)
            | Opcode::InstanceOf(_)
            | Opcode::ArrayLength
            | Opcode::Call(_)
            | Opcode::IfException
    )
}

/// True for nodes that update the control token inside a block.  The
/// executable subset rejects these, but the scheduler understands them.
pub fn updates_ctrl(op: &Opcode) -> bool {
    matches!(
        op,
        Opcode::New(_)
            | Opcode::NewArray(_)
            | Opcode::NewArrayRef(_)
            | Opcode::Call(_)
            | Opcode::MonitorEnter
            | Opcode::MonitorExit
    )
}

fn value_operand_indices(op: &Opcode) -> &'static [usize] {
    use Opcode::*;
    match op {
        // [mem, ctrl, object/base, offset/index]
        LoadI | LoadL | LoadF | LoadD | LoadP | LoadB | LoadC | LoadS => &[2, 3],
        // [mem, ctrl, object/base, offset/index, value]
        StoreI | StoreL | StoreF | StoreD | StoreP | StoreB | StoreC | StoreS => &[2, 3, 4],
        // [mem, ctrl, size/cp]
        NewArray(_) | NewArrayRef(_) => &[2],
        // [mem, ctrl, receiver, args...]
        Call(_) => &[2],
        // [mem, ctrl, obj]
        CheckCast(_) | InstanceOf(_) | MonitorEnter | MonitorExit | ArrayLength => &[2],
        _ => &[],
    }
}

fn terminator_value_indices(op: &Opcode) -> &'static [usize] {
    match op {
        Opcode::If => &[1],
        Opcode::Return => &[2],
        Opcode::Athrow => &[2],
        _ => &[],
    }
}

fn phi_value_indices(gvn: &PhaseGVN, phi: NodeId) -> Vec<usize> {
    let n = gvn.node(phi);
    let region = *n.inputs.last().unwrap_or(&NONE);
    let k = gvn
        .node(region)
        .inputs
        .len()
        .min(n.inputs.len().saturating_sub(1));
    (0..k).collect()
}

pub struct Scheduler<'a> {
    gvn: &'a PhaseGVN,
    ctrl_users: HashMap<NodeId, Vec<NodeId>>,
    region_of_phi: HashMap<NodeId, NodeId>,
    mem_of_region: HashMap<NodeId, NodeId>,
    unsupported: Vec<String>,
}

impl<'a> Scheduler<'a> {
    pub fn new(gvn: &'a PhaseGVN) -> Self {
        let mut ctrl_users: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut region_of_phi = HashMap::new();
        let mut mem_of_region = HashMap::new();
        for node in &gvn.nodes {
            match node.op {
                Opcode::Phi => {
                    if let Some(&r) = node.inputs.last() {
                        if r != NONE {
                            region_of_phi.insert(node.id, r);
                        }
                    }
                }
                Opcode::MergeMem => {
                    if let Some(&r) = node.inputs.last() {
                        if r != NONE {
                            mem_of_region.insert(r, node.id);
                        }
                    }
                }
                _ => {}
            }
            if let Some(ci) = ctrl_index(&node.op)
                && let Some(&c) = node.inputs.get(ci)
                && c != NONE
            {
                ctrl_users.entry(c).or_default().push(node.id);
            }
        }
        Self {
            gvn,
            ctrl_users,
            region_of_phi,
            mem_of_region,
            unsupported: Vec::new(),
        }
    }

    pub fn schedule(&mut self) -> Schedule {
        let mut entry_block = HashMap::new();
        let mut blocks: Vec<Block> = Vec::new();
        let start = self.gvn.start;
        if start == NONE {
            return Schedule {
                blocks,
                entry_block,
                region_mem: self.mem_of_region.clone(),
                phi_region: self.region_of_phi.clone(),
                unsupported: vec!["missing Start node".to_string()],
            };
        }
        let start_id = self.add_block(start, &mut blocks, &mut entry_block);
        let mut queue = VecDeque::from([start_id]);
        let mut seen = HashSet::from([start_id]);
        while let Some(bid) = queue.pop_front() {
            let succs = self.scan_block(bid, &mut blocks);
            for e in succs {
                if seen.insert(e.block) {
                    queue.push_back(e.block);
                }
            }
        }
        // Reverse postorder over the discovered CFG.
        let rpo = self.reverse_postorder(&blocks);
        for (pos, &bid) in rpo.iter().enumerate() {
            blocks[bid].rpo = pos;
        }
        // Mark loop headers: targets of edges that do not go forward in RPO.
        for b in 0..blocks.len() {
            let succs = blocks[b].succs.clone();
            for e in succs {
                if blocks[e.block].rpo <= blocks[b].rpo {
                    blocks[e.block].loop_header = true;
                }
            }
        }
        entry_block.clear();
        for b in &blocks {
            entry_block.insert(b.entry, b.id);
        }
        Schedule {
            blocks,
            entry_block,
            region_mem: self.mem_of_region.clone(),
            phi_region: self.region_of_phi.clone(),
            unsupported: self.unsupported.clone(),
        }
    }

    fn add_block(
        &self,
        entry: NodeId,
        blocks: &mut Vec<Block>,
        entry_block: &mut HashMap<NodeId, usize>,
    ) -> usize {
        let id = blocks.len();
        blocks.push(Block {
            id,
            entry,
            entry_mem: NONE,
            instrs: Vec::new(),
            term: None,
            succs: Vec::new(),
            exit_ctrl: entry,
            rpo: usize::MAX,
            loop_header: false,
        });
        entry_block.insert(entry, id);
        id
    }

    fn scan_block(&mut self, bid: usize, blocks: &mut Vec<Block>) -> Vec<Edge> {
        let entry = blocks[bid].entry;
        let entry_mem = self.entry_mem(entry);
        blocks[bid].entry_mem = entry_mem;
        let mut cur_mem = entry_mem;
        let mut instrs = Vec::new();
        let mut term = None;
        let mut succs = Vec::new();
        let mut exit_ctrl = entry;
        let entry_op = self.gvn.node(entry).op.clone();
        loop {
            // Candidates control-dependent on this block's entry control.
            let candidates = self
                .ctrl_users
                .get(&entry)
                .cloned()
                .unwrap_or_default();
            // Emit every non-memory-updating effect that reads cur_mem.
            let mut emitted_effect = false;
            for &n in &candidates {
                let op = &self.gvn.node(n).op;
                if produces_mem(op) || is_terminal(op) || updates_ctrl(op) {
                    continue;
                }
                if let Some(mi) = mem_index(op) {
                    if self.gvn.node(n).inputs.get(mi).copied() != Some(cur_mem) {
                        continue;
                    }
                }
                instrs.push(n);
                emitted_effect = true;
            }
            if emitted_effect {
                continue;
            }
            // Find the next memory-state updater on this memory edge.
            let mut next_mem = NONE;
            for &n in &candidates {
                let op = &self.gvn.node(n).op;
                if !produces_mem(op) {
                    continue;
                }
                if let Some(mi) = mem_index(op)
                    && self.gvn.node(n).inputs.get(mi).copied() == Some(cur_mem)
                {
                    next_mem = n;
                    break;
                }
            }
            if next_mem != NONE {
                instrs.push(next_mem);
                cur_mem = next_mem;
                // Calls/news also update control; the executable subset
                // rejects them, but keep the token moving for completeness.
                if updates_ctrl(&self.gvn.node(next_mem).op) {
                    exit_ctrl = next_mem;
                }
                continue;
            }
            // No more memory effects: look for the block terminator.
            let mut found_term = false;
            for &n in &candidates {
                let node = self.gvn.node(n);
                let kind = match node.op {
                    Opcode::If => Some(TermKind::If),
                    Opcode::Return => Some(TermKind::Return),
                    Opcode::Athrow => Some(TermKind::Athrow),
                    Opcode::Halt => Some(TermKind::Halt),
                    _ => None,
                };
                let Some(kind) = kind else { continue };
                // Return/Athrow carry mem at index 0/1 respectively; require it
                // to be the final memory state.  If has no memory input.
                let mem_ok = match node.op {
                    Opcode::Return => node.inputs.get(1).copied() == Some(cur_mem),
                    Opcode::Athrow => node.inputs.get(0).copied() == Some(cur_mem),
                    _ => true,
                };
                if !mem_ok {
                    continue;
                }
                term = Some(Terminal { kind, node: n });
                found_term = true;
                if node.op == Opcode::If {
                    // Successors are the IfTrue/IfFalse projection nodes.
                    let if_id = n;
                    for m in 0..self.gvn.node_count() {
                        let mn = self.gvn.node(m);
                        if matches!(mn.op, Opcode::IfTrue | Opcode::IfFalse)
                            && mn.inputs.first().copied() == Some(if_id)
                        {
                            if let Some(sb) = self.block_for_control_or_add(
                                m,
                                blocks,
                                &mut succs,
                            ) {
                                succs.push(Edge { block: sb, ctrl: m });
                            }
                        }
                    }
                }
                break;
            }
            if found_term {
                break;
            }
            // Fallthrough: the current control token is an input of a Region.
            let mut fall = false;
            for (rid, node) in self.gvn.nodes.iter().enumerate() {
                if node.op != Opcode::Region {
                    continue;
                }
                if node.inputs.contains(&exit_ctrl) {
                    if let Some(sb) =
                        self.block_for_control_or_add(rid, blocks, &mut succs)
                    {
                        succs.push(Edge { block: sb, ctrl: exit_ctrl });
                        fall = true;
                    }
                }
            }
            if fall {
                break;
            }
            // A block may end by handing control to a single-predecessor
            // successor that adopted the same control token; there is no new
            // control point, so there is nothing more to emit.
            break;
        }
        blocks[bid].instrs = instrs;
        blocks[bid].term = term;
        blocks[bid].succs = succs.clone();
        blocks[bid].exit_ctrl = exit_ctrl;
        let _ = entry_op;
        succs
    }

    fn entry_mem(&self, entry: NodeId) -> NodeId {
        match self.gvn.node(entry).op {
            Opcode::Start => entry,
            Opcode::IfTrue | Opcode::IfFalse => self
                .gvn
                .node(entry)
                .inputs
                .get(1)
                .copied()
                .unwrap_or(entry),
            Opcode::Region => self
                .mem_of_region
                .get(&entry)
                .copied()
                .unwrap_or(entry),
            _ => entry,
        }
    }

    fn block_for_control_or_add(
        &self,
        ctrl: NodeId,
        blocks: &mut Vec<Block>,
        succs: &mut Vec<Edge>,
    ) -> Option<usize> {
        // Blocks are created by the BFS in schedule(); during scanning we only
        // need ids.  We maintain a side map through entry_block is awkward, so
        // scan_block returns edges and schedule() materializes blocks lazily.
        // To keep this function total, we look for an existing block by entry.
        if let Some(b) = blocks.iter().position(|b| b.entry == ctrl) {
            return Some(b);
        }
        // Not yet created: reserve a placeholder block; schedule() will fill it
        // when it reaches the id.  The placeholder keeps edge targets stable.
        let id = blocks.len();
        blocks.push(Block {
            id,
            entry: ctrl,
            entry_mem: NONE,
            instrs: Vec::new(),
            term: None,
            succs: Vec::new(),
            exit_ctrl: ctrl,
            rpo: usize::MAX,
            loop_header: false,
        });
        let _ = succs;
        Some(id)
    }

    fn reverse_postorder(&self, blocks: &[Block]) -> Vec<usize> {
        // Iterative postorder from block 0 (Start) over already-discovered edges.
        let n = blocks.len();
        let mut visited = vec![false; n];
        let mut post = Vec::with_capacity(n);
        let mut stack = vec![(0usize, 0usize)];
        visited[0] = true;
        while let Some((b, i)) = stack.last_mut() {
            let b = *b;
            if *i < blocks[b].succs.len() {
                let s = blocks[b].succs[*i].block;
                *i += 1;
                if !visited[s] {
                    visited[s] = true;
                    stack.push((s, 0));
                }
            } else {
                post.push(b);
                stack.pop();
            }
        }
        // Include any unreachable placeholders defensively.
        for i in 0..n {
            if !visited[i] {
                post.push(i);
            }
        }
        post.reverse();
        post
    }
}

fn is_terminal(op: &Opcode) -> bool {
    matches!(
        op,
        Opcode::If | Opcode::Return | Opcode::Athrow | Opcode::Halt
    )
}

/// Collect the set of materialized value nodes needed to evaluate `node`.
/// Pure nodes are transparent; constants and parameters need no location.
pub fn collect_materialized(
    gvn: &PhaseGVN,
    node: NodeId,
    out: &mut HashSet<NodeId>,
    visiting: &mut HashSet<NodeId>,
) {
    if node == NONE || !visiting.insert(node) {
        return;
    }
    let n = gvn.node(node);
    match n.op {
        Opcode::ConI(_)
        | Opcode::ConL(_)
        | Opcode::ConF(_)
        | Opcode::ConD(_)
        | Opcode::ConP(_)
        | Opcode::Bool(_)
        | Opcode::Parm(_) => {}
        Opcode::Phi
        | Opcode::LoadI
        | Opcode::LoadL
        | Opcode::LoadF
        | Opcode::LoadD
        | Opcode::LoadP
        | Opcode::LoadB
        | Opcode::LoadC
        | Opcode::LoadS
        | Opcode::New(_)
        | Opcode::NewArray(_)
        | Opcode::NewArrayRef(_)
        | Opcode::CheckCast(_)
        | Opcode::InstanceOf(_)
        | Opcode::ArrayLength
        | Opcode::Call(_)
        | Opcode::IfException => {
            out.insert(node);
        }
        _ => {
            for &inp in &n.inputs {
                collect_materialized(gvn, inp, out, visiting);
            }
        }
    }
    visiting.remove(&node);
}

/// Value input indices for a scheduled effect node (skips mem/ctrl edges).
pub fn effect_value_indices(op: &Opcode) -> &'static [usize] {
    value_operand_indices(op)
}

/// Value input indices for a terminator node.
pub fn terminal_value_indices(op: &Opcode) -> &'static [usize] {
    terminator_value_indices(op)
}

/// Value input indices for a Phi node (excludes the trailing Region input).
pub fn phi_input_count(gvn: &PhaseGVN, phi: NodeId) -> usize {
    phi_value_indices(gvn, phi).len()
}

/// Region input index used by a given predecessor control node, if any.
pub fn region_pred_index(gvn: &PhaseGVN, region: NodeId, ctrl: NodeId) -> Option<usize> {
    gvn.node(region)
        .inputs
        .iter()
        .position(|&c| c == ctrl)
}

/// All Phi nodes whose merge region is `region`.
pub fn phis_of_region(gvn: &PhaseGVN, region: NodeId) -> Vec<NodeId> {
    gvn.nodes
        .iter()
        .filter(|n| n.op == Opcode::Phi && n.inputs.last().copied() == Some(region))
        .map(|n| n.id)
        .collect()
}
