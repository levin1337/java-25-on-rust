use super::c1_IR::{BasicBlock, ControlFlowGraph};
use super::c1_Instruction::{
    ArrayElemKind, BlockBegin, CallInfo, CompareKind, Condition, ConvertKind, HirInstruction,
    ValueKind,
};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExceptionHandler {
    pub start_pc: usize,
    pub end_pc: usize,
    pub handler_pc: usize,
    pub catch_type: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub kind: ValueKind,
    pub offset: Option<usize>,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSiteInfo {
    pub opcode: u8,
    pub cp_index: u32,
    pub name: String,
    pub arg_kinds: Vec<ValueKind>,
    pub return_kind: ValueKind,
    pub is_static: bool,
    pub is_interface: bool,
    pub target: Option<u64>,
}

impl CallSiteInfo {
    pub fn to_call_info(&self) -> CallInfo {
        CallInfo {
            opcode: self.opcode,
            cp_index: self.cp_index,
            name: self.name.clone(),
            arg_kinds: self.arg_kinds.clone(),
            return_kind: self.return_kind,
            is_static: self.is_static,
            is_interface: self.is_interface,
            target: self.target,
            resolved: true,
            site: 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MethodMetadata {
    pub fields: BTreeMap<u32, FieldInfo>,
    pub call_sites: BTreeMap<u32, CallSiteInfo>,
    pub ldc_constants: BTreeMap<u32, HirInstruction>,
    pub exception_handlers: Vec<ExceptionHandler>,
    /// Constant-pool index -> resolved site description for every
    /// heap-touching instruction (field get/put, static get/put,
    /// new/checkcast/instanceof/anewarray).  The LIR lowers these to
    /// `CallRuntime` stubs that receive a leaked `StubSite` pointer.
    pub sites: BTreeMap<u32, SiteInfo>,
}

/// Resolved constant-pool reference passed to JIT runtime stubs.
/// `class` is the internal (slash-separated) class name; `name`/`desc`
/// are empty for pure class references (new/checkcast/instanceof/
/// anewarray), which only need `class`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteInfo {
    pub class: String,
    pub name: String,
    pub desc: String,
}

#[derive(Clone, Debug)]
struct StackValue {
    expr: HirInstruction,
    kind: ValueKind,
    slots: usize,
}

impl StackValue {
    fn new(expr: HirInstruction, kind: ValueKind) -> Self {
        Self { expr, kind, slots: kind.stack_slots() }
    }

    fn load(slot: usize, kind: ValueKind) -> Self {
        Self::new(HirInstruction::LoadStack { slot, kind }, kind)
    }
}

/// `true` when re-emitting `expr` is not semantics-preserving: the
/// node allocates, calls into the runtime, reads mutable heap state
/// or can throw.  `dup*` handlers spill such expressions to a frame
/// slot so the lowered code evaluates them once; pure trees (constants,
/// local/stack loads, arithmetic, conversions, comparisons) may be
/// shared freely because re-emission is side-effect free.
fn expr_needs_spill(expr: &HirInstruction) -> bool {
    match expr {
        HirInstruction::ConstantInt(_)
        | HirInstruction::ConstantLong(_)
        | HirInstruction::ConstantFloat(_)
        | HirInstruction::ConstantDouble(_)
        | HirInstruction::ConstantNull
        | HirInstruction::LoadLocal { .. }
        | HirInstruction::LoadStack { .. } => false,
        HirInstruction::Add(l, r)
        | HirInstruction::Sub(l, r)
        | HirInstruction::Mul(l, r)
        | HirInstruction::And(l, r)
        | HirInstruction::Or(l, r)
        | HirInstruction::Xor(l, r)
        | HirInstruction::Shl(l, r)
        | HirInstruction::Shr(l, r)
        | HirInstruction::Ushr(l, r) => expr_needs_spill(l) || expr_needs_spill(r),
        HirInstruction::Neg(v) => expr_needs_spill(v),
        HirInstruction::Convert { value, .. } => expr_needs_spill(value),
        HirInstruction::Compare { left, right, .. } => {
            expr_needs_spill(left) || expr_needs_spill(right)
        }
        _ => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AbsValue {
    kind: ValueKind,
    slots: usize,
}

impl AbsValue {
    fn new(kind: ValueKind) -> Self {
        Self { kind, slots: kind.stack_slots() }
    }
}

/// Scans raw bytecode for `ldc`/`ldc_w`/`ldc2_w` constant-pool indices.
/// Used by the compile broker to pre-resolve numeric constants into
/// `MethodMetadata::ldc_constants` before graph building.
pub fn ldc_cp_indices(code: &[u8]) -> Vec<u32> {
    let gb = GraphBuilder::new(code, 0);
    let mut out = Vec::new();
    let mut bci = 0usize;
    while bci < code.len() {
        match code[bci] {
            0x12 => {
                if bci + 1 < code.len() {
                    out.push(code[bci + 1] as u32);
                }
            }
            0x13 | 0x14 => {
                if bci + 2 < code.len() {
                    out.push(u16::from_be_bytes([code[bci + 1], code[bci + 2]]) as u32);
                }
            }
            _ => {}
        }
        bci = gb.next_bci(bci);
    }
    out
}

/// Resolves every heap-touching constant-pool index in `code`
/// (field get/put, static get/put, new/checkcast/instanceof/
/// anewarray) against `klass` and merges the results into
/// `metadata.sites`/`metadata.fields`.  Entries already present are
/// kept so broker-supplied metadata wins.  This is what lets the
/// executable tier lower heap ops to `CallRuntime` stubs with real
/// `StubSite` pointers instead of deoptimizing unconditionally.
pub fn resolve_heap_sites(
    klass: &crate::share::oops::instanceKlass::InstanceKlass<'_>,
    code: &[u8],
    metadata: &mut MethodMetadata,
) {
    let gb = GraphBuilder::new(code, 0);
    let mut bci = 0usize;
    while bci < code.len() {
        let op = code[bci];
        let cp_ops = matches!(
            op,
            0xb2 | 0xb3 | 0xb4 | 0xb5 | 0xb6..=0xba | 0xbb | 0xbd | 0xc0 | 0xc1
        );
        if cp_ops && bci + 2 < code.len() {
            let idx = u16::from_be_bytes([code[bci + 1], code[bci + 2]]) as u32;
            if idx != 0 && !metadata.sites.contains_key(&idx) {
                match op {
                    // Invoke instructions: the site triple feeds the
                    // CallMethod stub (owner/name/descriptor), exactly
                    // like the heap-op sites feed the field stubs.
                    0xb6..=0xba => {
                        if let Ok([owner, name, desc]) = klass.memberRef(idx as u16, false) {
                            metadata.sites.insert(
                                idx,
                                SiteInfo {
                                    class: String::from_utf8_lossy(owner).into_owned(),
                                    name: String::from_utf8_lossy(name).into_owned(),
                                    desc: String::from_utf8_lossy(desc).into_owned(),
                                },
                            );
                        }
                    }
                    0xb2 | 0xb3 | 0xb4 | 0xb5 => {
                        if let Ok([owner, name, desc]) = klass.memberRef(idx as u16, true) {
                            metadata.fields.entry(idx).or_insert_with(|| FieldInfo {
                                kind: ValueKind::from_descriptor(desc),
                                offset: None,
                                name: String::from_utf8_lossy(name).into_owned(),
                            });
                            metadata.sites.insert(
                                idx,
                                SiteInfo {
                                    class: String::from_utf8_lossy(owner).into_owned(),
                                    name: String::from_utf8_lossy(name).into_owned(),
                                    desc: String::from_utf8_lossy(desc).into_owned(),
                                },
                            );
                        }
                    }
                    _ => {
                        if let Ok(class) = klass.className(idx as u16) {
                            metadata.sites.insert(
                                idx,
                                SiteInfo {
                                    class: String::from_utf8_lossy(class).into_owned(),
                                    name: String::new(),
                                    desc: String::new(),
                                },
                            );
                        }
                    }
                }
            }
        }
        bci = gb.next_bci(bci);
    }
}

/// Maps a resolved class-file `Constant` to the HIR constant node used
/// for `ldc`/`ldc2_w`.  Returns `None` for reference constants
/// (String/Class/MethodType/condy...) which cannot live in the
/// primitive executable subset anyway.
pub fn constant_to_hir(c: &crate::share::classfile::classFileParser::Constant<'_>) -> Option<HirInstruction> {
    use crate::share::classfile::classFileParser::Constant;
    Some(match c {
        Constant::Integer(v) => HirInstruction::ConstantInt(*v),
        Constant::Long(v) => HirInstruction::ConstantLong(*v),
        Constant::Float(v) => HirInstruction::ConstantFloat(v.to_bits()),
        Constant::Double(v) => HirInstruction::ConstantDouble(v.to_bits()),
        _ => return None,
    })
}

pub struct GraphBuilder<'a> {
    pub code: &'a [u8],
    pub max_locals: usize,
    pub metadata: MethodMetadata,
    /// Spill slots handed out to duplicated non-trivial expressions
    /// (see `spill_value`); `build_cfg` folds the final count into
    /// `ControlFlowGraph::max_stack_entries`.
    spill_count: Cell<usize>,
}

impl<'a> GraphBuilder<'a> {
    pub fn new(code: &'a [u8], max_locals: usize) -> Self {
        Self { code, max_locals, metadata: MethodMetadata::default(), spill_count: Cell::new(0) }
    }

    pub fn with_metadata(code: &'a [u8], max_locals: usize, metadata: MethodMetadata) -> Self {
        Self { code, max_locals, metadata, spill_count: Cell::new(0) }
    }

    pub fn max_locals(&self) -> usize {
        self.max_locals
    }

    pub fn build(code: &'a [u8], max_locals: usize) -> ControlFlowGraph {
        Self::new(code, max_locals).build_cfg()
    }

    pub fn build_full(code: &'a [u8], max_locals: usize, metadata: MethodMetadata) -> ControlFlowGraph {
        Self::with_metadata(code, max_locals, metadata).build_cfg()
    }

    pub fn build_cfg(&self) -> ControlFlowGraph {
        let mut cfg = ControlFlowGraph::new();
        cfg.max_locals = self.max_locals;
        if self.code.is_empty() {
            return cfg;
        }
        let leaders = self.identify_leaders();
        let bci_to_block_id: BTreeMap<usize, usize> = leaders.iter().enumerate().map(|(id, &bci)| (bci, id)).collect();
        for (id, &bci) in leaders.iter().enumerate() {
            let mut block = BasicBlock::with_bci(id, bci);
            block.begin = BlockBegin::new(id, bci);
            cfg.blocks.push(block);
        }
        for handler in &self.metadata.exception_handlers {
            if let Some(&id) = bci_to_block_id.get(&handler.handler_pc) {
                cfg.blocks[id].begin.is_exception_entry = true;
            }
        }
        let states = self.compute_stack_states(&leaders, &bci_to_block_id);
        cfg.max_stack_entries = states.iter().flatten().map(Vec::len).max().unwrap_or(0);
        for i in 0..leaders.len() {
            let (start, end) = self.block_range(&leaders, i);
            self.build_block(&mut cfg, i, start, end, &bci_to_block_id, states.get(i).and_then(|s| s.as_ref()));
        }
        // Spill slots live above the operand-stack region; grow it so
        // `stack_count` covers every `LoadStack`/`StoreStack` index the
        // dup handlers may have emitted.
        cfg.max_stack_entries += self.spill_count.get();
        cfg.compute_predecessors();
        cfg
    }

    fn switch_padding(&self, bci: usize) -> usize {
        (4 - ((bci + 1) % 4)) % 4
    }

    fn switch_target(&self, bci: usize, off: i32) -> usize {
        bci.wrapping_add_signed(off as isize)
    }

    fn tableswitch_parts(&self, bci: usize) -> Option<(usize, i32, i32, Vec<usize>)> {
        let base = bci + 1 + self.switch_padding(bci);
        if base + 12 > self.code.len() { return None; }
        let default = self.switch_target(bci, i32::from_be_bytes(self.code[base..base + 4].try_into().ok()?));
        let low = i32::from_be_bytes(self.code[base + 4..base + 8].try_into().ok()?);
        let high = i32::from_be_bytes(self.code[base + 8..base + 12].try_into().ok()?);
        if high < low || (high as i64 - low as i64) > 1_000_000 { return Some((default, low, high, Vec::new())); }
        let count = (high - low + 1) as usize;
        let mut targets = Vec::with_capacity(count);
        let mut p = base + 12;
        for _ in 0..count {
            if p + 4 > self.code.len() { break; }
            targets.push(self.switch_target(bci, i32::from_be_bytes(self.code[p..p + 4].try_into().ok()?)));
            p += 4;
        }
        Some((default, low, high, targets))
    }

    fn lookupswitch_parts(&self, bci: usize) -> Option<(usize, Vec<(i32, usize)>)> {
        let base = bci + 1 + self.switch_padding(bci);
        if base + 8 > self.code.len() { return None; }
        let default = self.switch_target(bci, i32::from_be_bytes(self.code[base..base + 4].try_into().ok()?));
        let npairs = i32::from_be_bytes(self.code[base + 4..base + 8].try_into().ok()?);
        if npairs < 0 || npairs as usize > 1_000_000 { return Some((default, Vec::new())); }
        let mut pairs = Vec::with_capacity(npairs as usize);
        let mut p = base + 8;
        for _ in 0..npairs {
            if p + 8 > self.code.len() { break; }
            let key = i32::from_be_bytes(self.code[p..p + 4].try_into().ok()?);
            let target = self.switch_target(bci, i32::from_be_bytes(self.code[p + 4..p + 8].try_into().ok()?));
            pairs.push((key, target));
            p += 8;
        }
        Some((default, pairs))
    }

    fn instruction_len_at(&self, bci: usize) -> usize {
        if bci >= self.code.len() { return 1; }
        let op = self.code[bci];
        match op {
            0xc4 => match self.code.get(bci + 1).copied() {
                Some(0x15..=0x19 | 0x36..=0x3a | 0xa9) => 4,
                Some(0x84) => 6,
                _ => 2,
            },
            0xaa => {
                let pad = self.switch_padding(bci);
                self.tableswitch_parts(bci).map(|(_, low, high, _)| {
                    1 + pad + 12 + 4 * (high.saturating_sub(low).saturating_add(1).max(0) as usize)
                }).unwrap_or(self.code.len() - bci)
            }
            0xab => {
                let pad = self.switch_padding(bci);
                self.lookupswitch_parts(bci).map(|(_, pairs)| 1 + pad + 8 + 8 * pairs.len()).unwrap_or(self.code.len() - bci)
            }
            _ => match op {
                0x00..=0x0f | 0x1a..=0x35 | 0x3b..=0x56 | 0x57..=0x83 | 0x85..=0x98
                | 0xac..=0xb1 | 0xbe..=0xbf | 0xc2..=0xc3 => 1,
                0x10 | 0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xbc => 2,
                0x11 | 0x13 | 0x14 | 0x84 | 0x99..=0xa8 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0
                | 0xc1 | 0xc6 | 0xc7 => 3,
                0xb9 | 0xba | 0xc8 | 0xc9 => 5,
                0xc5 => 4,
                _ => 1,
            },
        }
        .min(self.code.len() - bci)
    }

    fn next_bci(&self, bci: usize) -> usize {
        (bci + self.instruction_len_at(bci)).min(self.code.len())
    }

    fn identify_leaders(&self) -> Vec<usize> {
        let mut leaders = BTreeSet::new();
        leaders.insert(0);
        for h in &self.metadata.exception_handlers {
            if h.handler_pc < self.code.len() { leaders.insert(h.handler_pc); }
        }
        let mut bci = 0;
        while bci < self.code.len() {
            let op = self.code[bci];
            let next = self.next_bci(bci);
            match op {
                0xa7 | 0xc8 => {
                    if let Some(t) = self.branch_target(bci, op == 0xc8) {
                        if t < self.code.len() { leaders.insert(t); }
                    }
                }
                0x99..=0xa6 | 0xc6 | 0xc7 => {
                    if let Some(t) = self.branch_target(bci, false) {
                        if t < self.code.len() { leaders.insert(t); }
                    }
                    if next < self.code.len() { leaders.insert(next); }
                }
                0xaa => {
                    if let Some((defaultBci, _, _, targets)) = self.tableswitch_parts(bci) {
                        for t in targets {
                            let tb = t as usize;
                            if tb < self.code.len() { leaders.insert(tb); }
                        }
                        if defaultBci < self.code.len() { leaders.insert(defaultBci); }
                    }
                }
                0xab => {
                    if let Some((defaultBci, pairs)) = self.lookupswitch_parts(bci) {
                        for (_, t) in pairs {
                            let tb = t as usize;
                            if tb < self.code.len() { leaders.insert(tb); }
                        }
                        if defaultBci < self.code.len() { leaders.insert(defaultBci); }
                    }
                }
                0xac..=0xb1 | 0xbf => {
                    if next < self.code.len() { leaders.insert(next); }
                }
                _ => {}
            }
            bci = next;
        }
        leaders.into_iter().collect()
    }

    fn branch_target(&self, bci: usize, wide: bool) -> Option<usize> {
        if wide {
            if bci + 5 > self.code.len() { return None; }
            let off = i32::from_be_bytes(self.code[bci + 1..bci + 5].try_into().ok()?);
            Some(bci.wrapping_add_signed(off as isize))
        } else {
            if bci + 3 > self.code.len() { return None; }
            let off = i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
            Some(bci.wrapping_add_signed(off as isize))
        }
    }

    fn block_range(&self, leaders: &[usize], id: usize) -> (usize, usize) {
        let start = leaders[id];
        let end = if id + 1 < leaders.len() { leaders[id + 1] } else { self.code.len() };
        (start, end)
    }

    fn compute_stack_states(&self, leaders: &[usize], bci_to_block_id: &BTreeMap<usize, usize>) -> Vec<Option<Vec<AbsValue>>> {
        let n = leaders.len();
        let mut states: Vec<Option<Vec<AbsValue>>> = vec![None; n];
        if n == 0 { return states; }
        states[0] = Some(Vec::new());
        let mut queue = VecDeque::new();
        queue.push_back(0usize);
        let mut in_queue = vec![false; n];
        in_queue[0] = true;
        while let Some(id) = queue.pop_front() {
            in_queue[id] = false;
            let Some(start_state) = states[id].clone() else { continue };
            let (start, end) = self.block_range(leaders, id);
            let outgoing = self.simulate_block(start_state, start, end, id, leaders, bci_to_block_id);
            for (succ, stack) in outgoing {
                if succ >= n { continue; }
                let changed = match &states[succ] {
                    None => { states[succ] = Some(stack); true }
                    Some(current) => {
                        if *current == stack { false }
                        else if current.len() == stack.len() {
                            let mut merged = current.clone();
                            let mut diff = false;
                            for (i, v) in stack.iter().enumerate() {
                                if merged[i].kind == ValueKind::Void { merged[i] = *v; diff = true; }
                            }
                            if diff { states[succ] = Some(merged); true } else { false }
                        } else { false }
                    }
                };
                if changed && !in_queue[succ] { queue.push_back(succ); in_queue[succ] = true; }
            }
        }
        states
    }

    fn simulate_block(&self, mut stack: Vec<AbsValue>, start: usize, end: usize, block_id: usize, leaders: &[usize], bci_to_block_id: &BTreeMap<usize, usize>) -> Vec<(usize, Vec<AbsValue>)> {
        let mut result = Vec::new();
        let mut bci = start;
        while bci < end {
            let op = self.code[bci];
            let next = self.next_bci(bci);
            match op {
                0xc4 => self.simulate_wide(&mut stack, bci),
                0x00 => {}
                0x01 => stack.push(AbsValue::new(ValueKind::Ref)),
                0x02..=0x08 => stack.push(AbsValue::new(ValueKind::Int32)),
                0x09 | 0x0a => stack.push(AbsValue::new(ValueKind::Int64)),
                0x0b..=0x0d => stack.push(AbsValue::new(ValueKind::Float32)),
                0x0e | 0x0f => stack.push(AbsValue::new(ValueKind::Float64)),
                0x10 | 0x11 => stack.push(AbsValue::new(ValueKind::Int32)),
                0x12..=0x14 => stack.push(AbsValue::new(ValueKind::Ref)),
                0x15 | 0x1a..=0x1d => stack.push(AbsValue::new(ValueKind::Int32)),
                0x16 | 0x1e..=0x21 => stack.push(AbsValue::new(ValueKind::Int64)),
                0x17 | 0x22..=0x25 => stack.push(AbsValue::new(ValueKind::Float32)),
                0x18 | 0x26..=0x29 => stack.push(AbsValue::new(ValueKind::Float64)),
                0x19 | 0x2a..=0x2d => stack.push(AbsValue::new(ValueKind::Ref)),
                0x2e..=0x35 => {
                    self.abs_pop(&mut stack);
                    self.abs_pop(&mut stack);
                    stack.push(AbsValue::new(array_elem(op).value_kind()));
                }
                0x36..=0x3a | 0x3b..=0x3e => { self.abs_pop(&mut stack); }
                0x37 | 0x3f..=0x42 => { self.abs_pop(&mut stack); }
                0x38 | 0x43..=0x46 => { self.abs_pop(&mut stack); }
                0x39 | 0x47..=0x4a => { self.abs_pop(&mut stack); }
                0x3a | 0x4b..=0x4e => { self.abs_pop(&mut stack); }
                0x4f..=0x56 => { self.abs_pop(&mut stack); self.abs_pop(&mut stack); self.abs_pop(&mut stack); }
                0x57 => { self.abs_pop(&mut stack); }
                0x58 => { self.abs_pop2(&mut stack); }
                0x59 => { self.abs_dup(&mut stack); }
                0x5a | 0x5b => { self.abs_dup_x(&mut stack, 1); }
                0x5c => { self.abs_dup2(&mut stack); }
                0x5d | 0x5e => { self.abs_dup2_x(&mut stack, if op == 0x5d { 1 } else { 2 }); }
                0x5f => { self.abs_swap(&mut stack); }
                0x60..=0x83 => {
                    if matches!(op, 0x74..=0x77) {
                        self.abs_pop(&mut stack);
                        stack.push(AbsValue::new(self.arith_kind(op)));
                    } else {
                        self.abs_binary(&mut stack, self.arith_kind(op));
                    }
                }
                0x84 => {}
                0x85..=0x93 => {
                    self.abs_pop(&mut stack);
                    stack.push(AbsValue::new(convert_result_kind(self.convert_opcode(op))));
                }
                0x94..=0x98 => {
                    self.abs_binary(&mut stack, ValueKind::Int32);
                }
                0x99..=0x9e | 0xc6 | 0xc7 => {
                    self.abs_pop(&mut stack);
                    for tb in self.branch_successor_bcis(bci, op) {
                        if let Some(&succ) = bci_to_block_id.get(&tb) { result.push((succ, stack.clone())); }
                    }
                    if let Some(&succ) = bci_to_block_id.get(&next) { result.push((succ, stack.clone())); }
                    return result;
                }
                0x9f..=0xa6 => {
                    self.abs_pop(&mut stack);
                    self.abs_pop(&mut stack);
                    for tb in self.branch_successor_bcis(bci, op) {
                        if let Some(&succ) = bci_to_block_id.get(&tb) { result.push((succ, stack.clone())); }
                    }
                    if let Some(&succ) = bci_to_block_id.get(&next) { result.push((succ, stack.clone())); }
                    return result;
                }
                0xa7 | 0xc8 => {
                    for tb in self.branch_successor_bcis(bci, op) {
                        if let Some(&succ) = bci_to_block_id.get(&tb) { result.push((succ, stack.clone())); }
                    }
                    return result;
                }
                0xaa => {
                    self.abs_pop(&mut stack);
                    for tb in self.branch_successor_bcis(bci, op) {
                        if let Some(&succ) = bci_to_block_id.get(&tb) { result.push((succ, stack.clone())); }
                    }
                    return result;
                }
                0xab => {
                    self.abs_pop(&mut stack);
                    for tb in self.branch_successor_bcis(bci, op) {
                        if let Some(&succ) = bci_to_block_id.get(&tb) { result.push((succ, stack.clone())); }
                    }
                    return result;
                }
                0xac..=0xb0 => { self.abs_pop(&mut stack); return result; }
                0xb1 => return result,
                0xb2 => { stack.push(AbsValue::new(self.static_kind(bci, true))); }
                0xb3 => { self.abs_pop(&mut stack); }
                0xb4 => { self.abs_pop(&mut stack); stack.push(AbsValue::new(self.field_kind(bci))); }
                0xb5 => { self.abs_pop(&mut stack); self.abs_pop(&mut stack); }
                0xb6..=0xba => {
                    let call = self.call_info_for(bci, op);
                    if !call.is_static { self.abs_pop(&mut stack); }
                    for _ in 0..call.arg_kinds.len() { self.abs_pop(&mut stack); }
                    if call.return_kind != ValueKind::Void { stack.push(AbsValue::new(call.return_kind)); }
                }
                0xbb => { stack.push(AbsValue::new(ValueKind::Ref)); }
                0xbc | 0xbd => { self.abs_pop(&mut stack); stack.push(AbsValue::new(ValueKind::Ref)); }
                0xbe => { self.abs_pop(&mut stack); stack.push(AbsValue::new(ValueKind::Int32)); }
                0xbf => { self.abs_pop(&mut stack); return result; }
                0xc0 => { self.abs_pop(&mut stack); stack.push(AbsValue::new(ValueKind::Ref)); }
                0xc1 => { self.abs_pop(&mut stack); stack.push(AbsValue::new(ValueKind::Int32)); }
                0xc2 | 0xc3 => { self.abs_pop(&mut stack); }
                0xc5 => {
                    let dims = self.code.get(bci + 3).copied().unwrap_or(0) as usize;
                    for _ in 0..=dims { self.abs_pop(&mut stack); }
                    stack.push(AbsValue::new(ValueKind::Ref));
                }
                _ => {}
            }
            bci = next;
        }
        if block_id + 1 < leaders.len() { result.push((block_id + 1, stack)); }
        result
    }

    fn simulate_wide(&self, stack: &mut Vec<AbsValue>, bci: usize) {
        let Some(&inner) = self.code.get(bci + 1) else { return };
        match inner {
            0x15 | 0x19 => stack.push(AbsValue::new(ValueKind::Int32)),
            0x16 => stack.push(AbsValue::new(ValueKind::Int64)),
            0x17 => stack.push(AbsValue::new(ValueKind::Float32)),
            0x18 => stack.push(AbsValue::new(ValueKind::Float64)),
            0x36 => { self.abs_pop(stack); }
            0x37 => { self.abs_pop(stack); }
            0x38 => { self.abs_pop(stack); }
            0x39 => { self.abs_pop(stack); }
            0x3a => { self.abs_pop(stack); }
            0xa9 => { self.abs_pop(stack); stack.push(AbsValue::new(ValueKind::Int32)); }
            _ => {}
        }
    }

    fn abs_pop(&self, stack: &mut Vec<AbsValue>) { stack.pop(); }

    fn abs_pop2(&self, stack: &mut Vec<AbsValue>) {
        if stack.last().map(|v| v.slots) == Some(2) { stack.pop(); } else { stack.pop(); stack.pop(); }
    }

    fn abs_dup(&self, stack: &mut Vec<AbsValue>) {
        if let Some(v) = stack.last().copied() { stack.push(v); }
    }

    fn abs_dup_x(&self, stack: &mut Vec<AbsValue>, below: usize) {
        if stack.len() <= below { return; }
        let top = stack.pop().unwrap();
        let split = stack.len().saturating_sub(below);
        let mut tail = Vec::with_capacity(stack.len() + below + 1);
        tail.extend_from_slice(&stack[..split]);
        tail.push(top);
        tail.extend_from_slice(&stack[split..]);
        tail.push(top);
        *stack = tail;
    }

    fn abs_dup2(&self, stack: &mut Vec<AbsValue>) {
        if stack.last().map(|v| v.slots) == Some(2) { self.abs_dup(stack); return; }
        if stack.len() >= 2 {
            let a = stack[stack.len() - 2];
            let b = stack[stack.len() - 1];
            stack.push(a);
            stack.push(b);
        }
    }

    fn abs_dup2_x(&self, stack: &mut Vec<AbsValue>, below: usize) {
        if stack.len() <= below + 1 { return; }
        let group: Vec<AbsValue> = if stack.last().map(|v| v.slots) == Some(2) { vec![stack.pop().unwrap()] } else { vec![stack.remove(stack.len() - 2), stack.pop().unwrap()] };
        let split = stack.len().saturating_sub(below);
        let mut tail = Vec::new();
        tail.extend_from_slice(&stack[..split]);
        tail.extend(group.iter().copied());
        tail.extend_from_slice(&stack[split..]);
        tail.extend(group);
        *stack = tail;
    }

    fn abs_swap(&self, stack: &mut Vec<AbsValue>) {
        if stack.len() >= 2 {
            let n = stack.len();
            stack.swap(n - 1, n - 2);
        }
    }

    fn abs_binary(&self, stack: &mut Vec<AbsValue>, kind: ValueKind) {
        self.abs_pop(stack);
        self.abs_pop(stack);
        stack.push(AbsValue::new(kind));
    }

    fn arith_kind(&self, op: u8) -> ValueKind {
        match op {
            0x61 | 0x65 | 0x69 | 0x6d | 0x71 | 0x75 | 0x79 | 0x7b | 0x7d | 0x7f | 0x81 | 0x83 => ValueKind::Int64,
            0x62 | 0x66 | 0x6a | 0x6e | 0x72 | 0x76 => ValueKind::Float32,
            0x63 | 0x67 | 0x6b | 0x6f | 0x73 | 0x77 => ValueKind::Float64,
            _ => ValueKind::Int32,
        }
    }

    fn branch_successor_bcis(&self, bci: usize, op: u8) -> Vec<usize> {
        let mut out = Vec::new();
        match op {
            0xaa => {
                if let Some((defaultBci, _, _, targets)) = self.tableswitch_parts(bci) { out.extend(targets.into_iter().map(|t| t as usize)); out.push(defaultBci); }
            }
            0xab => {
                if let Some((defaultBci, pairs)) = self.lookupswitch_parts(bci) { out.extend(pairs.into_iter().map(|(_, t)| t as usize)); out.push(defaultBci); }
            }
            0xc8 => { if let Some(t) = self.branch_target(bci, true) { out.push(t); } }
            _ => { if let Some(t) = self.branch_target(bci, false) { out.push(t); } }
        }
        out
    }

    fn call_info_for(&self, bci: usize, op: u8) -> CallInfo {
        let idx = self.cp_index(bci);
        if let Some(site) = self.metadata.call_sites.get(&idx) { return site.to_call_info(); }
        CallInfo::unresolved(op, idx)
    }

    fn field_kind(&self, bci: usize) -> ValueKind {
        self.metadata.fields.get(&self.cp_index(bci)).map(|f| f.kind).unwrap_or(ValueKind::Int32)
    }

    fn static_kind(&self, bci: usize, _get: bool) -> ValueKind {
        self.metadata.fields.get(&self.cp_index(bci)).map(|f| f.kind).unwrap_or(ValueKind::Int32)
    }

    fn cp_index(&self, bci: usize) -> u32 {
        if bci + 2 < self.code.len() { u16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as u32 } else { 0 }
    }

    fn build_block(&self, cfg: &mut ControlFlowGraph, block_id: usize, start_bci: usize, end_bci: usize, bci_to_block_id: &BTreeMap<usize, usize>, entry_state: Option<&Vec<AbsValue>>) {
        let mut stack: Vec<StackValue> = entry_state.map(|s| s.iter().enumerate().map(|(i, v)| StackValue::load(i, v.kind)).collect()).unwrap_or_default();
        let mut bci = start_bci;
        let mut terminated = false;
        while bci < end_bci && !terminated {
            let op = self.code[bci];
            let next = self.next_bci(bci);
            match op {
                0xc4 => self.build_wide(cfg, block_id, &mut stack, bci),
                0x00 => {}
                0x01 => stack.push(StackValue::new(HirInstruction::ConstantNull, ValueKind::Ref)),
                0x02..=0x08 => stack.push(StackValue::new(HirInstruction::ConstantInt(op as i32 - 0x03), ValueKind::Int32)),
                0x09 | 0x0a => stack.push(StackValue::new(HirInstruction::ConstantLong(op as i64 - 0x09), ValueKind::Int64)),
                0x0b..=0x0d => { let v = match op { 0x0b => 0.0f32, 0x0c => 1.0, _ => 2.0 }; stack.push(StackValue::new(HirInstruction::ConstantFloat(v.to_bits()), ValueKind::Float32)); }
                0x0e | 0x0f => { let v = if op == 0x0e { 0.0f64 } else { 1.0 }; stack.push(StackValue::new(HirInstruction::ConstantDouble(v.to_bits()), ValueKind::Float64)); }
                0x10 => { if bci + 1 < self.code.len() { stack.push(StackValue::new(HirInstruction::ConstantInt(self.code[bci + 1] as i8 as i32), ValueKind::Int32)); } }
                0x11 => { if bci + 2 < self.code.len() { stack.push(StackValue::new(HirInstruction::ConstantInt(i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32), ValueKind::Int32)); } }
                0x12..=0x14 => {
                    if let Some(c) = self.ldc_constant(bci, op) { let kind = c.value_kind(); stack.push(StackValue::new(c, kind)); }
                    else { stack.push(StackValue::new(self.unsupported(bci, op, "unresolved ldc"), ValueKind::Ref)); }
                }
                0x15..=0x19 => { let kind = load_kind_generic(op); let idx = self.load_index(bci, op); stack.push(StackValue::new(HirInstruction::LoadLocal { slot: idx, kind }, kind)); }
                0x1a..=0x2d => { let (idx, kind) = load_n(op); stack.push(StackValue::new(HirInstruction::LoadLocal { slot: idx, kind }, kind)); }
                0x2e..=0x35 => self.array_load(&mut stack, op),
                0x36..=0x4e => {
                    let (slot, kind) = if op <= 0x3a {
                        self.store_info(bci, op)
                    } else {
                        self.store_n(op)
                    };
                    let value = self.pop(&mut stack).map_or_else(|| self.default_for(kind), |value| value.expr);
                    self.storeLocal(cfg, block_id, &mut stack, slot, kind, value);
                }
                0x4f..=0x56 => self.array_store(cfg, block_id, &mut stack, op),
                0x57 => { self.pop(&mut stack); }
                0x58 => { self.pop2(&mut stack); }
                0x59 => { self.dup_top(cfg, block_id, &mut stack); }
                0x5a => { self.dup_x(cfg, block_id, &mut stack, 1, false); }
                0x5b => { self.dup_x(cfg, block_id, &mut stack, 2, false); }
                0x5c => { self.dup2(cfg, block_id, &mut stack); }
                0x5d => { self.dup2_x(cfg, block_id, &mut stack, 1); }
                0x5e => { self.dup2_x(cfg, block_id, &mut stack, 2); }
                0x5f => { if stack.len() >= 2 { let n = stack.len(); stack.swap(n - 1, n - 2); } }
                0x60..=0x83 => {
                    let kind = self.arith_kind(op);
                    if matches!(op, 0x74..=0x77) {
                        let v = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| self.default_for(kind));
                        stack.push(StackValue::new(HirInstruction::Neg(Box::new(v)), kind));
                    } else {
                        let r = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| self.default_for(kind));
                        let l = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| self.default_for(kind));
                        stack.push(StackValue::new(self.binary_expr(op, l, r), kind));
                    }
                }
                0x84 => {
                    if bci + 2 < self.code.len() {
                        let idx = self.code[bci + 1] as usize;
                        let inc = self.code[bci + 2] as i8 as i32;
                        self.incrementLocal(cfg, block_id, &mut stack, idx, inc);
                    }
                }
                0x85..=0x93 => {
                    let v = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| self.default_for(ValueKind::Int32));
                    let kind = convert_result_kind(self.convert_opcode(op));
                    stack.push(StackValue::new(HirInstruction::Convert { kind: self.convert_opcode(op), value: Box::new(v) }, kind));
                }
                0x94..=0x98 => {
                    let r = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
                    let l = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
                    let kind = match op { 0x94 => CompareKind::Int64, 0x95 => CompareKind::Float32NanLow, 0x96 => CompareKind::Float32NanHigh, 0x97 => CompareKind::Float64NanLow, _ => CompareKind::Float64NanHigh };
                    stack.push(StackValue::new(HirInstruction::Compare { kind, left: Box::new(l), right: Box::new(r) }, ValueKind::Int32));
                }
                0x99..=0x9e => {
                    let cond = match op { 0x99 => Condition::Eq, 0x9a => Condition::Ne, 0x9b => Condition::Lt, 0x9c => Condition::Ge, 0x9d => Condition::Gt, _ => Condition::Le };
                    let left = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
                    self.flush_stack(cfg, block_id, &mut stack);
                    let (tb, fb) = self.branch_blocks(bci, block_id, bci_to_block_id, next);
                    cfg.blocks[block_id].push(HirInstruction::If { left: Box::new(left), cond, right: Box::new(HirInstruction::ConstantInt(0)), true_block: tb, false_block: fb });
                    terminated = true;
                }
                0x9f..=0xa6 => {
                    let cond = match op { 0x9f | 0xa5 => Condition::Eq, 0xa0 | 0xa6 => Condition::Ne, 0xa1 => Condition::Lt, 0xa2 => Condition::Ge, 0xa3 => Condition::Gt, _ => Condition::Le };
                    let r = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
                    let l = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
                    self.flush_stack(cfg, block_id, &mut stack);
                    let (tb, fb) = self.branch_blocks(bci, block_id, bci_to_block_id, next);
                    cfg.blocks[block_id].push(HirInstruction::If { left: Box::new(l), cond, right: Box::new(r), true_block: tb, false_block: fb });
                    terminated = true;
                }
                0xa7 | 0xc8 => {
                    self.flush_stack(cfg, block_id, &mut stack);
                    if let Some(t) = self.branch_target(bci, op == 0xc8) {
                        if let Some(&tb) = bci_to_block_id.get(&t) { cfg.blocks[block_id].push(HirInstruction::Goto(tb)); }
                    }
                    terminated = true;
                }
                0xaa => {
                    let index = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
                    self.flush_stack(cfg, block_id, &mut stack);
                    if let Some((defaultBci, low, high, targets)) = self.tableswitch_parts(bci) {
                        let default_block = bci_to_block_id.get(&defaultBci).copied().unwrap_or(block_id + 1);
                        let target_blocks = targets.iter().map(|t| bci_to_block_id.get(&(*t as usize)).copied().unwrap_or(default_block)).collect();
                        cfg.blocks[block_id].push(HirInstruction::TableSwitch { index: Box::new(index), low, high, default_block, targets: target_blocks });
                    }
                    terminated = true;
                }
                0xab => {
                    let key = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
                    self.flush_stack(cfg, block_id, &mut stack);
                    if let Some((defaultBci, pairs)) = self.lookupswitch_parts(bci) {
                        let default_block = bci_to_block_id.get(&defaultBci).copied().unwrap_or(block_id + 1);
                        let out = pairs.into_iter().map(|(k, t)| (k, bci_to_block_id.get(&(t as usize)).copied().unwrap_or(default_block))).collect();
                        cfg.blocks[block_id].push(HirInstruction::LookupSwitch { key: Box::new(key), pairs: out, default_block });
                    }
                    terminated = true;
                }
                0xac..=0xb0 => { let val = self.pop(&mut stack).map(|x| x.expr); cfg.blocks[block_id].push(HirInstruction::Return(val.map(Box::new))); terminated = true; }
                0xb1 => { cfg.blocks[block_id].push(HirInstruction::Return(None)); terminated = true; }
                0xb2 => {
                    let idx = self.cp_index(bci);
                    let kind = self.static_kind(bci, true);
                    let name = self.metadata.fields.get(&idx).map(|f| f.name.clone()).unwrap_or_else(|| format!("cp#{}", idx));
                    stack.push(StackValue::new(HirInstruction::GetStatic { cp_index: idx, offset: None, kind, name }, kind));
                }
                0xb3 => {
                    let idx = self.cp_index(bci);
                    let kind = self.static_kind(bci, false);
                    let name = self.metadata.fields.get(&idx).map(|f| f.name.clone()).unwrap_or_else(|| format!("cp#{}", idx));
                    let val = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| self.default_for(kind));
                    cfg.blocks[block_id].push(HirInstruction::PutStatic { cp_index: idx, offset: None, value: Box::new(val), kind, name });
                }
                0xb4 | 0xb5 => {
                    let idx = self.cp_index(bci);
                    let f = self.metadata.fields.get(&idx);
                    let kind = f.map(|x| x.kind).unwrap_or(ValueKind::Int32);
                    let offset = f.and_then(|x| x.offset);
                    let name = f.map(|x| x.name.clone()).unwrap_or_else(|| format!("cp#{}", idx));
                    if op == 0xb4 {
                        let obj = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull);
                        let expr = HirInstruction::LoadField { obj: Box::new(obj), cp_index: idx, offset, kind, name };
                        stack.push(StackValue::new(expr, kind));
                    } else {
                        let value = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| self.default_for(kind));
                        let obj = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull);
                        cfg.blocks[block_id].push(HirInstruction::StoreField { obj: Box::new(obj), cp_index: idx, offset, value: Box::new(value), kind, name });
                    }
                }
                0xb6..=0xba => self.build_invoke(cfg, block_id, &mut stack, bci, op),
                0xbb => { let idx = self.cp_index(bci); stack.push(StackValue::new(HirInstruction::NewObject { cp_index: idx, name: format!("cp#{}", idx) }, ValueKind::Ref)); }
                0xbc => { let len = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0)); let elem = newarray_elem(self.code.get(bci + 1).copied().unwrap_or(4)); stack.push(StackValue::new(HirInstruction::NewArray { elem, length: Box::new(len) }, ValueKind::Ref)); }
                0xbd => { let len = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0)); let idx = self.cp_index(bci); stack.push(StackValue::new(HirInstruction::AnewArray { cp_index: idx, length: Box::new(len) }, ValueKind::Ref)); }
                0xbe => { let arr = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull); stack.push(StackValue::new(HirInstruction::ArrayLength(Box::new(arr)), ValueKind::Int32)); }
                0xbf => { let val = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull); cfg.blocks[block_id].push(HirInstruction::Throw(Box::new(val))); terminated = true; }
                0xc0 => { let val = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull); let idx = self.cp_index(bci); stack.push(StackValue::new(HirInstruction::CheckCast { value: Box::new(val), cp_index: idx, name: format!("cp#{}", idx) }, ValueKind::Ref)); }
                0xc1 => { let val = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull); let idx = self.cp_index(bci); stack.push(StackValue::new(HirInstruction::InstanceOf { value: Box::new(val), cp_index: idx, name: format!("cp#{}", idx) }, ValueKind::Int32)); }
                0xc2 | 0xc3 => { let val = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull); if op == 0xc2 { cfg.blocks[block_id].push(HirInstruction::MonitorEnter(Box::new(val))); } else { cfg.blocks[block_id].push(HirInstruction::MonitorExit(Box::new(val))); } }
                0xc5 => { let dims = self.code.get(bci + 3).copied().unwrap_or(0) as usize; for _ in 0..=dims { self.pop(&mut stack); } stack.push(StackValue::new(self.unsupported(bci, op, "multianewarray"), ValueKind::Ref)); }
                0xc6 | 0xc7 => {
                    let cond = if op == 0xc6 { Condition::Eq } else { Condition::Ne };
                    let left = self.pop(&mut stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull);
                    self.flush_stack(cfg, block_id, &mut stack);
                    let (tb, fb) = self.branch_blocks(bci, block_id, bci_to_block_id, next);
                    cfg.blocks[block_id].push(HirInstruction::If { left: Box::new(left), cond, right: Box::new(HirInstruction::ConstantNull), true_block: tb, false_block: fb });
                    terminated = true;
                }
                _ => { stack.push(StackValue::new(self.unsupported(bci, op, "unsupported opcode"), ValueKind::Ref)); }
            }
            bci = next;
        }
        if !terminated {
            self.flush_stack(cfg, block_id, &mut stack);
            if block_id + 1 < cfg.blocks.len() { cfg.blocks[block_id].push(HirInstruction::Goto(block_id + 1)); }
        }
    }

    fn build_wide(&self, cfg: &mut ControlFlowGraph, block_id: usize, stack: &mut [StackValue], bci: usize) {
        let Some(&inner) = self.code.get(bci + 1) else { return };
        match inner {
            0x15 | 0x19 => { let idx = self.u2(bci + 2); cfg.blocks[block_id].instructions.push(HirInstruction::Unsupported { opcode: inner, bci, reason: format!("wide load {}", idx) }); }
            0x16 => { let idx = self.u2(bci + 2); cfg.blocks[block_id].instructions.push(HirInstruction::Unsupported { opcode: inner, bci, reason: format!("wide lload {}", idx) }); }
            0x17 => { let idx = self.u2(bci + 2); cfg.blocks[block_id].instructions.push(HirInstruction::Unsupported { opcode: inner, bci, reason: format!("wide fload {}", idx) }); }
            0x18 => { let idx = self.u2(bci + 2); cfg.blocks[block_id].instructions.push(HirInstruction::Unsupported { opcode: inner, bci, reason: format!("wide dload {}", idx) }); }
            0x36..=0x3a => { let idx = self.u2(bci + 2); cfg.blocks[block_id].instructions.push(HirInstruction::Unsupported { opcode: inner, bci, reason: format!("wide store {}", idx) }); }
            0x84 => {
                let idx = self.u2(bci + 2);
                let inc = i16::from_be_bytes([self.code.get(bci + 4).copied().unwrap_or(0), self.code.get(bci + 5).copied().unwrap_or(0)]) as i32;
                self.incrementLocal(cfg, block_id, stack, idx, inc);
            }
            _ => cfg.blocks[block_id].instructions.push(HirInstruction::Unsupported { opcode: inner, bci, reason: "wide".to_string() }),
        }
    }

    fn incrementLocal(
        &self,
        cfg: &mut ControlFlowGraph,
        blockId: usize,
        stack: &mut [StackValue],
        slot: usize,
        increment: i32,
    ) {
        let kind = ValueKind::Int32;
        let load = HirInstruction::LoadLocal { slot, kind };
        let value = HirInstruction::Add(Box::new(load), Box::new(HirInstruction::ConstantInt(increment)));
        self.storeLocal(cfg, blockId, stack, slot, kind, value);
    }

    fn storeLocal(
        &self,
        cfg: &mut ControlFlowGraph,
        blockId: usize,
        stack: &mut [StackValue],
        slot: usize,
        kind: ValueKind,
        value: HirInstruction,
    ) {
        self.materializeStack(cfg, blockId, stack, |expr| {
            !matches!(
                expr,
                HirInstruction::LoadStack { .. }
                    | HirInstruction::ConstantInt(_)
                    | HirInstruction::ConstantLong(_)
                    | HirInstruction::ConstantFloat(_)
                    | HirInstruction::ConstantDouble(_)
                    | HirInstruction::ConstantNull
            )
        });
        cfg.blocks[blockId].push(HirInstruction::StoreLocal {
            slot,
            kind,
            value: Box::new(value),
        });
    }

    fn materializeStack(
        &self,
        cfg: &mut ControlFlowGraph,
        blockId: usize,
        stack: &mut [StackValue],
        needsSpill: impl Fn(&HirInstruction) -> bool,
    ) {
        for value in stack {
            if needsSpill(&value.expr) {
                let expr = std::mem::replace(&mut value.expr, HirInstruction::ConstantInt(0));
                value.expr = self.spill_value(cfg, blockId, expr, value.kind);
            }
        }
    }

    fn build_invoke(&self, cfg: &mut ControlFlowGraph, blockId: usize, stack: &mut Vec<StackValue>, bci: usize, op: u8) {
        let info = self.call_info_for(bci, op);
        if !info.is_resolved() {
            cfg.blocks[blockId].push(self.unsupported(bci, op, "unresolved invoke"));
            return;
        }

        let argumentCount = info.arg_kinds.len() + usize::from(!info.is_static);
        let returnKind = info.return_kind;
        let mut operands = Vec::with_capacity(argumentCount);
        for _ in 0..argumentCount {
            let value = self.pop(stack).map_or(HirInstruction::ConstantInt(0), |value| value.expr);
            operands.push(Box::new(value));
        }
        operands.reverse();
        self.materializeStack(cfg, blockId, stack, expr_needs_spill);

        let call = HirInstruction::Call { info, operands };
        if returnKind == ValueKind::Void {
            cfg.blocks[blockId].push(call);
        } else {
            let result = self.spill_value(cfg, blockId, call, returnKind);
            stack.push(StackValue::new(result, returnKind));
        }
    }

    fn array_load(&self, stack: &mut Vec<StackValue>, op: u8) {
        let elem = array_elem(op);
        let index = self.pop(stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
        let array = self.pop(stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull);
        stack.push(StackValue::new(HirInstruction::LoadIndexed { array: Box::new(array), index: Box::new(index), elem }, elem.value_kind()));
    }

    fn array_store(&self, cfg: &mut ControlFlowGraph, block_id: usize, stack: &mut Vec<StackValue>, op: u8) {
        let elem = array_elem(op);
        let value = self.pop(stack).map(|x| x.expr).unwrap_or_else(|| self.default_for(elem.value_kind()));
        let index = self.pop(stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantInt(0));
        let array = self.pop(stack).map(|x| x.expr).unwrap_or_else(|| HirInstruction::ConstantNull);
        cfg.blocks[block_id].push(HirInstruction::StoreIndexed { array: Box::new(array), index: Box::new(index), value: Box::new(value), elem });
    }

    fn pop(&self, stack: &mut Vec<StackValue>) -> Option<StackValue> { stack.pop() }

    fn pop2(&self, stack: &mut Vec<StackValue>) {
        if stack.last().map(|v| v.slots) == Some(2) { stack.pop(); } else { stack.pop(); stack.pop(); }
    }

    /// `dup`: duplicates the top stack value.  Trivial expressions
    /// (constants, local/stack loads, pure arithmetic) may share the
    /// tree — re-emission is side-effect free — but anything that can
    /// allocate, call, throw or observe mutable heap state is spilled
    /// to a dedicated stack slot first so it is evaluated exactly
    /// once, like the JVM's operand-stack value.
    fn dup_top(&self, cfg: &mut ControlFlowGraph, block_id: usize, stack: &mut Vec<StackValue>) {
        let Some(v) = stack.last_mut() else { return };
        let kind = v.kind;
        if !expr_needs_spill(&v.expr) {
            if let Some(top) = stack.last().cloned() {
                stack.push(top);
            }
            return;
        }
        let expr = std::mem::replace(&mut v.expr, HirInstruction::ConstantInt(0));
        let spilled = self.spill_value(cfg, block_id, expr, kind);
        v.expr = spilled.clone();
        stack.push(StackValue::new(spilled, kind));
    }

    /// Emits `StoreStack{spill_slot} = expr` into the current block
    /// and returns the `LoadStack` that reads it back.  Spill slots
    /// are allocated above `max_stack_entries` (the operand-stack
    /// region) and never reused within the method, so a spilled value
    /// stays valid in its frame slot even when the `LoadStack` is
    /// consumed by a successor block.
    fn spill_value(
        &self,
        cfg: &mut ControlFlowGraph,
        block_id: usize,
        expr: HirInstruction,
        kind: ValueKind,
    ) -> HirInstruction {
        let slot = cfg.max_stack_entries + self.spill_count.get();
        self.spill_count.set(self.spill_count.get() + 1);
        cfg.blocks[block_id].push(HirInstruction::StoreStack {
            slot,
            kind,
            value: Box::new(expr),
        });
        HirInstruction::LoadStack { slot, kind }
    }

    /// Replaces `value.expr` with a spill `LoadStack` when the
    /// expression must be evaluated once; returns the operand both
    /// duplicated stack entries should carry.
    fn dup_operand(
        &self,
        cfg: &mut ControlFlowGraph,
        block_id: usize,
        value: &mut StackValue,
    ) -> HirInstruction {
        let kind = value.kind;
        if !expr_needs_spill(&value.expr) {
            return value.expr.clone();
        }
        let expr = std::mem::replace(&mut value.expr, HirInstruction::ConstantInt(0));
        self.spill_value(cfg, block_id, expr, kind)
    }

    fn dup_x(&self, cfg: &mut ControlFlowGraph, block_id: usize, stack: &mut Vec<StackValue>, below: usize, _wide: bool) {
        if stack.len() <= below { return; }
        let mut top = stack.pop().unwrap();
        let split = stack.len().saturating_sub(below);
        let operand = self.dup_operand(cfg, block_id, &mut top);
        let mut tail = Vec::with_capacity(stack.len() + below + 1);
        tail.extend(stack.drain(..split));
        tail.push(StackValue::new(operand.clone(), top.kind));
        tail.extend(stack.drain(..));
        tail.push(StackValue::new(operand, top.kind));
        *stack = tail;
    }

    fn dup2(&self, cfg: &mut ControlFlowGraph, block_id: usize, stack: &mut Vec<StackValue>) {
        if stack.last().map(|v| v.slots) == Some(2) {
            self.dup_top(cfg, block_id, stack);
            return;
        }
        if stack.len() >= 2 {
            let mut a = stack.remove(stack.len() - 2);
            let mut b = stack.pop().unwrap();
            let oa = self.dup_operand(cfg, block_id, &mut a);
            let ob = self.dup_operand(cfg, block_id, &mut b);
            stack.push(StackValue::new(oa, a.kind));
            stack.push(StackValue::new(ob, b.kind));
            stack.push(a);
            stack.push(b);
        }
    }

    fn dup2_x(&self, cfg: &mut ControlFlowGraph, block_id: usize, stack: &mut Vec<StackValue>, below: usize) {
        if stack.len() <= below + 1 { return; }
        let mut group: Vec<StackValue> = if stack.last().map(|v| v.slots) == Some(2) {
            vec![stack.pop().unwrap()]
        } else {
            vec![stack.remove(stack.len() - 2), stack.pop().unwrap()]
        };
        let operands: Vec<HirInstruction> = group
            .iter_mut()
            .map(|v| self.dup_operand(cfg, block_id, v))
            .collect();
        let split = stack.len().saturating_sub(below);
        let mut tail = Vec::new();
        tail.extend(stack.drain(..split));
        for (v, op) in group.iter().zip(operands.iter()) {
            tail.push(StackValue::new(op.clone(), v.kind));
        }
        tail.extend(stack.drain(..));
        for (v, op) in group.iter().zip(operands.iter()) {
            tail.push(StackValue::new(op.clone(), v.kind));
        }
        *stack = tail;
    }

    fn flush_stack(&self, cfg: &mut ControlFlowGraph, block_id: usize, stack: &mut Vec<StackValue>) {
        for (i, v) in stack.iter().enumerate() {
            let already = matches!(&v.expr, HirInstruction::LoadStack { slot, kind } if *slot == i && *kind == v.kind);
            if !already {
                cfg.blocks[block_id].instructions.push(HirInstruction::StoreStack { slot: i, kind: v.kind, value: Box::new(v.expr.clone()) });
            }
        }
    }

    fn default_for(&self, kind: ValueKind) -> HirInstruction {
        match kind {
            ValueKind::Int64 => HirInstruction::ConstantLong(0),
            ValueKind::Float32 => HirInstruction::ConstantFloat(0),
            ValueKind::Float64 => HirInstruction::ConstantDouble(0),
            ValueKind::Ref => HirInstruction::ConstantNull,
            _ => HirInstruction::ConstantInt(0),
        }
    }

    fn unsupported(&self, bci: usize, op: u8, reason: &str) -> HirInstruction {
        HirInstruction::Unsupported { opcode: op, bci, reason: reason.to_string() }
    }

    fn ldc_constant(&self, bci: usize, op: u8) -> Option<HirInstruction> {
        let idx = match op {
            0x12 => self.code.get(bci + 1).copied().unwrap_or(0) as u32,
            0x13 | 0x14 => { if bci + 2 >= self.code.len() { return None; } u16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as u32 }
            _ => return None,
        };
        self.metadata.ldc_constants.get(&idx).cloned()
    }

    fn load_index(&self, bci: usize, op: u8) -> usize {
        if (0x15..=0x19).contains(&op) { self.code.get(bci + 1).copied().unwrap_or(0) as usize } else { 0 }
    }

    fn store_info(&self, bci: usize, op: u8) -> (usize, ValueKind) {
        if (0x36..=0x3a).contains(&op) {
            (self.code.get(bci + 1).copied().unwrap_or(0) as usize, match op { 0x36 => ValueKind::Int32, 0x37 => ValueKind::Int64, 0x38 => ValueKind::Float32, 0x39 => ValueKind::Float64, _ => ValueKind::Ref })
        } else { (0, ValueKind::Int32) }
    }

    fn store_n(&self, op: u8) -> (usize, ValueKind) {
        let idx = match op { 0x3b..=0x3e => op - 0x3b, 0x3f..=0x42 => op - 0x3f, 0x43..=0x46 => op - 0x43, 0x47..=0x4a => op - 0x47, _ => op - 0x4b };
        let kind = match op { 0x3b..=0x3e => ValueKind::Int32, 0x3f..=0x42 => ValueKind::Int64, 0x43..=0x46 => ValueKind::Float32, 0x47..=0x4a => ValueKind::Float64, _ => ValueKind::Ref };
        (idx as usize, kind)
    }

    fn binary_expr(&self, op: u8, l: HirInstruction, r: HirInstruction) -> HirInstruction {
        let l = Box::new(l);
        let r = Box::new(r);
        match op {
            0x60 | 0x61 | 0x62 | 0x63 => HirInstruction::Add(l, r),
            0x64 | 0x65 | 0x66 | 0x67 => HirInstruction::Sub(l, r),
            0x68 | 0x69 | 0x6a | 0x6b => HirInstruction::Mul(l, r),
            0x6c | 0x6d | 0x6e | 0x6f => HirInstruction::Div(l, r),
            0x70 | 0x71 | 0x72 | 0x73 => HirInstruction::Rem(l, r),
            0x78 | 0x79 => HirInstruction::Shl(l, r),
            0x7a | 0x7b => HirInstruction::Shr(l, r),
            0x7c | 0x7d => HirInstruction::Ushr(l, r),
            0x7e | 0x7f => HirInstruction::And(l, r),
            0x80 | 0x81 => HirInstruction::Or(l, r),
            0x82 | 0x83 => HirInstruction::Xor(l, r),
            _ => HirInstruction::Unsupported { opcode: op, bci: 0, reason: "binary".to_string() },
        }
    }

    fn convert_opcode(&self, op: u8) -> ConvertKind {
        match op {
            0x85 => ConvertKind::I2L, 0x86 => ConvertKind::I2F, 0x87 => ConvertKind::I2D,
            0x88 => ConvertKind::L2I, 0x89 => ConvertKind::L2F, 0x8a => ConvertKind::L2D,
            0x8b => ConvertKind::F2I, 0x8c => ConvertKind::F2L, 0x8d => ConvertKind::F2D,
            0x8e => ConvertKind::D2I, 0x8f => ConvertKind::D2L, 0x90 => ConvertKind::D2F,
            0x91 => ConvertKind::I2B, 0x92 => ConvertKind::I2C, _ => ConvertKind::I2S,
        }
    }

    fn branch_blocks(&self, bci: usize, block_id: usize, map: &BTreeMap<usize, usize>, next: usize) -> (usize, usize) {
        let target_bci = self.branch_target(bci, false).unwrap_or(usize::MAX);
        (map.get(&target_bci).copied().unwrap_or(block_id + 1), map.get(&next).copied().unwrap_or(block_id + 1))
    }

    fn u2(&self, bci: usize) -> usize {
        if bci + 1 < self.code.len() { u16::from_be_bytes([self.code[bci], self.code[bci + 1]]) as usize } else { 0 }
    }
}

fn load_kind_generic(op: u8) -> ValueKind {
    match op { 0x15 | 0x19 => if op == 0x19 { ValueKind::Ref } else { ValueKind::Int32 }, 0x16 => ValueKind::Int64, 0x17 => ValueKind::Float32, 0x18 => ValueKind::Float64, _ => ValueKind::Int32 }
}

fn load_n(op: u8) -> (usize, ValueKind) {
    let (base, kind) = match op {
        0x1a..=0x1d => (0x1a, ValueKind::Int32),
        0x1e..=0x21 => (0x1e, ValueKind::Int64),
        0x22..=0x25 => (0x22, ValueKind::Float32),
        0x26..=0x29 => (0x26, ValueKind::Float64),
        _ => (0x2a, ValueKind::Ref),
    };
    ((op - base) as usize, kind)
}

fn convert_result_kind(kind: ConvertKind) -> ValueKind {
    match kind {
        ConvertKind::I2L | ConvertKind::F2L | ConvertKind::D2L => ValueKind::Int64,
        ConvertKind::I2F | ConvertKind::L2F | ConvertKind::D2F => ValueKind::Float32,
        ConvertKind::I2D | ConvertKind::L2D | ConvertKind::F2D => ValueKind::Float64,
        _ => ValueKind::Int32,
    }
}

fn newarray_elem(dim: u8) -> ArrayElemKind {
    match dim { 4 => ArrayElemKind::Boolean, 5 => ArrayElemKind::Char, 6 => ArrayElemKind::Float, 7 => ArrayElemKind::Double, 8 => ArrayElemKind::Byte, 9 => ArrayElemKind::Short, 10 => ArrayElemKind::Int, 11 => ArrayElemKind::Long, _ => ArrayElemKind::Int }
}

fn array_elem(op: u8) -> ArrayElemKind {
    match op {
        0x2e => ArrayElemKind::Int, 0x2f => ArrayElemKind::Long, 0x30 => ArrayElemKind::Float, 0x31 => ArrayElemKind::Double,
        0x32 => ArrayElemKind::Ref, 0x33 => ArrayElemKind::Boolean, 0x34 => ArrayElemKind::Char, 0x35 => ArrayElemKind::Short,
        0x4f => ArrayElemKind::Int, 0x50 => ArrayElemKind::Long, 0x51 => ArrayElemKind::Float, 0x52 => ArrayElemKind::Double,
        0x53 => ArrayElemKind::Ref, 0x54 => ArrayElemKind::Boolean, 0x55 => ArrayElemKind::Char, 0x56 => ArrayElemKind::Short,
        _ => ArrayElemKind::Int,
    }
}
