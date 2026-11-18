/*
 * Port of OpenJDK 25 HotSpot C2 register allocator front-end
 * (share/opto/regalloc*.cpp) reduced to a real linear-scan allocator over the
 * scheduled sea-of-nodes graph.
 *
 * Values that must occupy machine locations are the materialized nodes:
 * Phi merges, loads, calls, allocations and casts.  Pure ideal nodes are
 * re-materialized by the matcher inside the block that uses them, so they do
 * not participate in global liveness.
 *
 * The allocator computes per-block use/def sets (phi edge copies are uses in
 * the predecessor), runs standard backward liveness over the CFG, builds live
 * intervals in reverse-postorder coordinates, and performs a linear scan over
 * two register classes:
 *   - GP:   RBX, R12-R15 (callee-saved under the SysV-style internal ABI)
 *   - XMM:  XMM8-XMM14 (XMM15 is reserved as the parallel-copy scratch)
 * Everything else is spilled to an 8-byte frame slot.  Calls clobber only
 * caller-saved registers, so GP values survive runtime calls; XMM values are
 * spilled/reloaded around safepoint polls by the code generator.
 */

use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode, Type, NONE};
use super::scheduler::{
    collect_materialized, effect_value_indices, phis_of_region, region_pred_index,
    terminal_value_indices, Schedule,
};
use super::asm::register::{Register, XMMRegister};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loc {
    Gp(Register),
    Xmm(XMMRegister),
    Spill(usize),
}

impl Loc {
    pub fn is_xmm(&self) -> bool {
        matches!(self, Self::Xmm(_))
    }
}

#[derive(Clone, Debug)]
pub struct Allocation {
    /// Machine location for every materialized value node.
    pub loc: HashMap<NodeId, Loc>,
    /// Number of JVM parameter slots reserved at the bottom of the frame.
    pub param_slots: usize,
    /// Number of spill slots after the parameter area.
    pub spill_count: usize,
    /// Callee-saved GP registers that must be saved/restored, in push order.
    pub saved_regs: Vec<Register>,
    /// Frame bytes to `sub rsp, imm` after saving registers.
    pub frame_size: usize,
    /// Live-in value sets per block (block id -> nodes), used by codegen for
    /// safepoint register preservation.
    pub live_in: HashMap<usize, HashSet<NodeId>>,
}

pub const POS_STRIDE: usize = 4096;

const GP_POOL: [Register; 5] = [
    Register::RBX,
    Register::R12,
    Register::R13,
    Register::R14,
    Register::R15,
];

const XMM_POOL: [XMMRegister; 7] = [
    XMMRegister::XMM8,
    XMMRegister::XMM9,
    XMMRegister::XMM10,
    XMMRegister::XMM11,
    XMMRegister::XMM12,
    XMMRegister::XMM13,
    XMMRegister::XMM14,
];

/// Trailing frame slots for operand staging and phi parallel copies.
pub const TEMP_SLOTS: usize = 8;

fn is_xmm_value(gvn: &PhaseGVN, node: NodeId) -> bool {
    is_xmm_value_deep(gvn, node, 0)
}

fn is_xmm_value_deep(gvn: &PhaseGVN, node: NodeId, depth: u8) -> bool {
    let n = gvn.node(node);
    if matches!(n.op, Opcode::Phi) {
        // Phi carries no static type; its kind is that of its value
        // inputs (all but the trailing Region control).  Depth cap
        // keeps phi cycles terminating.
        if depth >= 32 {
            return false;
        }
        let values = &n.inputs[..n.inputs.len().saturating_sub(1)];
        return values
            .iter()
            .any(|&i| i != NONE && is_xmm_value_deep(gvn, i, depth + 1));
    }
    match n.op {
        Opcode::ConF(_) | Opcode::ConD(_) | Opcode::LoadF | Opcode::LoadD => true,
        _ => matches!(n.ty, Type::Float | Type::Double),
    }
}

fn block_rpo(blocks: &[super::scheduler::Block], id: usize) -> usize {
    blocks[id].rpo
}

pub struct RegisterAllocator<'a> {
    gvn: &'a PhaseGVN,
    schedule: &'a Schedule,
    order: Vec<usize>,
}

impl<'a> RegisterAllocator<'a> {
    pub fn new(gvn: &'a PhaseGVN, schedule: &'a Schedule) -> Self {
        let mut order: Vec<usize> = (0..schedule.blocks.len()).collect();
        order.sort_by_key(|&b| schedule.blocks[b].rpo);
        Self {
            gvn,
            schedule,
            order,
        }
    }

    pub fn allocate(&self, param_slots: usize) -> Result<Allocation, String> {
        let use_def = self.compute_use_def()?;
        let live = self.compute_liveness(&use_def);
        let vregs = self.collect_vregs(&use_def);
        let (loc, spill_count) = self.linear_scan(&vregs, &use_def, &live)?;
        let saved_regs = Self::saved_regs(&loc);
        let frame_size = Self::frame_size(param_slots, spill_count, saved_regs.len());
        Ok(Allocation {
            loc,
            param_slots,
            spill_count,
            saved_regs,
            frame_size,
            live_in: live.into_iter().map(|(b, s)| (b, s.0)).collect(),
        })
    }

    fn compute_use_def(&self) -> Result<HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>, String> {
        let mut out: HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)> = HashMap::new();
        for &b in &self.order {
            let block = &self.schedule.blocks[b];
            let mut def = HashSet::new();
            let mut use_set = HashSet::new();
            // Phi definitions live at the block entry.
            if matches!(self.gvn.node(block.entry).op, Opcode::Region) {
                for phi in phis_of_region(self.gvn, block.entry) {
                    def.insert(phi);
                }
            }
            for &n in &block.instrs {
                let op = &self.gvn.node(n).op;
                if super::scheduler::produces_value(op) {
                    def.insert(n);
                }
                self.collect_operands(n, &mut use_set);
            }
            if let Some(term) = &block.term {
                let indices = terminal_value_indices(&self.gvn.node(term.node).op);
                for &i in indices {
                    if let Some(&inp) = self.gvn.node(term.node).inputs.get(i) {
                        self.collect_value(inp, &mut use_set);
                    }
                }
            }
            // Phi copies are uses in this predecessor block.
            for e in block.succs.iter().chain(block.exc_succs.iter()) {
                let succ_entry = self.schedule.blocks[e.block].entry;
                if self.gvn.node(succ_entry).op != Opcode::Region {
                    continue;
                }
                for phi in phis_of_region(self.gvn, succ_entry) {
                    if let Some(idx) = region_pred_index(self.gvn, succ_entry, e.ctrl) {
                        let inputs = &self.gvn.node(phi).inputs;
                        if let Some(&src) = inputs.get(idx) {
                            self.collect_value(src, &mut use_set);
                        }
                    }
                }
            }
            // Exception-edge tokens are defined by the dispatch sequence
            // emitted after the trapping instruction.
            for e in &block.exc_succs {
                if matches!(self.gvn.node(e.ctrl).op, Opcode::IfException) {
                    def.insert(e.ctrl);
                }
            }
            out.insert(b, (use_set, def));
        }
        Ok(out)
    }

    fn collect_operands(&self, node: NodeId, use_set: &mut HashSet<NodeId>) {
        let indices = effect_value_indices(&self.gvn.node(node).op);
        for &i in indices {
            if let Some(&inp) = self.gvn.node(node).inputs.get(i) {
                self.collect_value(inp, use_set);
            }
        }
        // Calls have a variable argument tail after [mem, ctrl, receiver].
        if let Opcode::Call(_) = self.gvn.node(node).op {
            for inp in self.gvn.node(node).inputs.iter().skip(2) {
                self.collect_value(*inp, use_set);
            }
        }
    }

    fn collect_value(&self, node: NodeId, use_set: &mut HashSet<NodeId>) {
        let mut visiting = HashSet::new();
        let mut tmp = HashSet::new();
        collect_materialized(self.gvn, node, &mut tmp, &mut visiting);
        use_set.extend(tmp);
    }

    fn compute_liveness(
        &self,
        use_def: &HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>,
    ) -> HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)> {
        let mut live_in: HashMap<usize, HashSet<NodeId>> = HashMap::new();
        let mut live_out: HashMap<usize, HashSet<NodeId>> = HashMap::new();
        for &b in &self.order {
            live_in.insert(b, HashSet::new());
            live_out.insert(b, HashSet::new());
        }
        loop {
            let mut changed = false;
            for &b in self.order.iter().rev() {
                let mut out = HashSet::new();
                for e in self.schedule.blocks[b]
                    .succs
                    .iter()
                    .chain(self.schedule.blocks[b].exc_succs.iter())
                {
                    if let Some(s) = live_in.get(&e.block) {
                        out.extend(s.iter().copied());
                    }
                }
                let (use_set, def_set) = &use_def[&b];
                let mut inn = out.clone();
                inn.retain(|v| !def_set.contains(v));
                inn.extend(use_set.iter().copied());
                if out != *live_out.get(&b).unwrap() {
                    *live_out.get_mut(&b).unwrap() = out;
                    changed = true;
                }
                if inn != *live_in.get(&b).unwrap() {
                    *live_in.get_mut(&b).unwrap() = inn;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.order
            .iter()
            .map(|&b| {
                (
                    b,
                    (
                        live_in[&b].clone(),
                        live_out[&b].clone(),
                    ),
                )
            })
            .collect()
    }

    fn collect_vregs(
        &self,
        use_def: &HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>,
    ) -> Vec<NodeId> {
        let mut v = HashSet::new();
        for (_, (_, def)) in use_def {
            v.extend(def.iter().copied());
        }
        let mut v: Vec<NodeId> = v.into_iter().collect();
        v.sort_unstable();
        v
    }

    fn def_key(&self, v: NodeId, use_def: &HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>) -> (usize, usize) {
        // Exception-edge tokens are defined on the edge out of their
        // source block, not inside any instruction list.
        if let Some(&(b, p)) = self.schedule.exc_def.get(&v) {
            return (block_rpo(&self.schedule.blocks, b), p);
        }
        for &b in &self.order {
            let block = &self.schedule.blocks[b];
            let (_, def) = &use_def[&b];
            if !def.contains(&v) {
                continue;
            }
            if matches!(self.gvn.node(block.entry).op, Opcode::Region)
                && phis_of_region(self.gvn, block.entry).contains(&v)
            {
                return (block_rpo(&self.schedule.blocks, b), 0);
            }
            if let Some(pos) = block.instrs.iter().position(|&n| n == v) {
                return (block_rpo(&self.schedule.blocks, b), pos + 1);
            }
        }
        (usize::MAX, usize::MAX)
    }

    fn end_key(
        &self,
        v: NodeId,
        use_def: &HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>,
        live: &HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>,
    ) -> usize {
        let mut end = 0usize;
        for &b in &self.order {
            let rpo = block_rpo(&self.schedule.blocks, b);
            let live_here = live[&b].0.contains(&v)
                || live[&b].1.contains(&v)
                || use_def[&b].0.contains(&v)
                || use_def[&b].1.contains(&v);
            if live_here {
                end = end.max(rpo * POS_STRIDE + POS_STRIDE - 1);
            }
        }
        end
    }

    fn linear_scan(
        &self,
        vregs: &[NodeId],
        use_def: &HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>,
        live: &HashMap<usize, (HashSet<NodeId>, HashSet<NodeId>)>,
    ) -> Result<(HashMap<NodeId, Loc>, usize), String> {
        let mut intervals: Vec<(NodeId, usize, usize, bool)> = Vec::with_capacity(vregs.len());
        for &v in vregs {
            let (br, pos) = self.def_key(v, use_def);
            if br == usize::MAX {
                return Err(format!("materialized node {} has no definition", v));
            }
            let start = br * POS_STRIDE + pos;
            let mut end = self.end_key(v, use_def, live);
            if end < start {
                end = start;
            }
            intervals.push((v, start, end, is_xmm_value(self.gvn, v)));
        }
        intervals.sort_by_key(|&(_, s, e, _)| (s, e));

        let mut loc: HashMap<NodeId, Loc> = HashMap::new();
        let mut spill_count = 0usize;
        let mut active_gp: Vec<(usize, Register)> = Vec::new();
        let mut active_xmm: Vec<(usize, XMMRegister)> = Vec::new();

        for (v, start, end, xmm) in intervals {
            if xmm {
                active_xmm.retain(|&(e, _)| e >= start);
                let used: HashSet<XMMRegister> = active_xmm.iter().map(|&(_, r)| r).collect();
                if let Some(reg) = XMM_POOL.iter().copied().find(|r| !used.contains(r)) {
                    active_xmm.push((end, reg));
                    loc.insert(v, Loc::Xmm(reg));
                } else {
                    loc.insert(v, Loc::Spill(param_spill_base(self.param_slots_placeholder()) + spill_count));
                    spill_count += 1;
                }
            } else {
                active_gp.retain(|&(e, _)| e >= start);
                let used: HashSet<Register> = active_gp.iter().map(|&(_, r)| r).collect();
                if let Some(reg) = GP_POOL.iter().copied().find(|r| !used.contains(r)) {
                    active_gp.push((end, reg));
                    loc.insert(v, Loc::Gp(reg));
                } else {
                    loc.insert(v, Loc::Spill(param_spill_base(self.param_slots_placeholder()) + spill_count));
                    spill_count += 1;
                }
            }
        }
        Ok((loc, spill_count))
    }

    // The spill slot numbering is finalized by the caller through
    // `Allocation::spill_slot`; during scanning we only need a dense index.
    fn param_slots_placeholder(&self) -> usize {
        0
    }

    fn saved_regs(loc: &HashMap<NodeId, Loc>) -> Vec<Register> {
        let mut used: HashSet<Register> = HashSet::new();
        for l in loc.values() {
            if let Loc::Gp(r) = l {
                used.insert(*r);
            }
        }
        GP_POOL.iter().copied().filter(|r| used.contains(r)).collect()
    }

    fn frame_size(param_slots: usize, spill_count: usize, saved: usize) -> usize {
        // Seven trailing slots are reserved for XMM safepoint saves
        // (XMM8-XMM14).  They are always present so loop-header polls can
        // preserve vector registers without a second allocation pass.
        let bytes = (param_slots + spill_count + XMM_POOL.len() + TEMP_SLOTS) * 8;
        let aligned = bytes.div_ceil(16) * 16;
        // After `push rbp` rsp is 16-aligned.  Each saved register moves rsp by
        // 8; keep the body 16-aligned for runtime calls by padding when the
        // number of saved registers is odd.
        if saved % 2 == 1 { aligned + 8 } else { aligned }
    }
}

fn param_spill_base(_param_slots: usize) -> usize {
    0
}

impl Allocation {
    /// Frame slot index (0-based) for a spilled value.  Parameter slots occupy
    /// `[0, param_slots)`; spills follow.
    pub fn spill_slot_index(&self, node: NodeId) -> Option<usize> {
        match self.loc.get(&node) {
            Some(Loc::Spill(i)) => Some(*i),
            _ => None,
        }
    }

    /// Absolute frame slot index for a spill location.
    pub fn spill_frame_slot(&self, node: NodeId) -> Option<usize> {
        self.spill_slot_index(node)
            .map(|i| self.param_slots + i)
    }

    /// Frame slot used to save an allocatable XMM register around a call.
    pub fn xmm_save_slot(&self, reg: XMMRegister) -> usize {
        self.param_slots + self.spill_count + (reg.encoding() as usize).saturating_sub(8)
    }
}
