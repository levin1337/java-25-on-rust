/*
 * Port of OpenJDK 25 HotSpot C2 bytecode parser (share/opto/parse*.cpp,
 * graphKit.cpp)
 *
 * Builds the initial Sea-of-Nodes ideal graph from JVM bytecode: basic-block
 * construction, SSA merge states with Phi/MergeMem nodes at control joins
 * (loop back edges are patched after the fact), exception edges via
 * IfException nodes, and Root/Return terminals.
 */

use super::ci_shim::{CiSignature, CiSignatureKind};
use super::gvn::PhaseGVN;
use super::node::{ArrayElemKind, CallResult, CmpCondition, NONE, NodeId, Opcode, call_pack};
use crate::share::interpreter::bytecode::{self, Bytecode};
use crate::share::interpreter::bytecodes::{self, Code};
use std::collections::{BTreeSet, HashMap};

pub const CON_P_NULL: u32 = 0;
pub const CON_P_STATIC: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Int,
    Long,
    Float,
    Double,
    Ref,
    Byte,
    Short,
    Char,
    Bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldAccess {
    pub offset: u32,
    pub kind: FieldKind,
    pub is_static: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct MethodAccess {
    /// Number of argument values popped from the operand stack
    /// (receiver excluded; long/double count as one stack value).
    pub nargs: usize,
    pub has_receiver: bool,
    pub result: CallResult,
}

impl MethodAccess {
    pub fn from_descriptor(desc: &str, has_receiver: bool) -> Option<Self> {
        let sig = CiSignature::parse(desc)?;
        let result = match sig.ret {
            CiSignatureKind::Void => CallResult::Void,
            CiSignatureKind::Int => CallResult::I32,
            CiSignatureKind::Long => CallResult::I64,
            CiSignatureKind::Float => CallResult::F32,
            CiSignatureKind::Double => CallResult::F64,
            CiSignatureKind::Object => CallResult::Ref,
        };
        Some(Self {
            nargs: sig.args.len(),
            has_receiver,
            result,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ConstVal {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(u32),
}

/// Resolved constant-pool site triple (owner class / member name /
/// descriptor) consumed by the runtime stubs through a leaked
/// `StubSite` pointer — the C2 analogue of the ciInterface field /
/// method / klass resolution in `share/opto/parse*.cpp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteInfo {
    pub class: String,
    pub name: String,
    pub desc: String,
}

pub struct BuilderContext<'a> {
    pub fields: &'a HashMap<u16, FieldAccess>,
    pub methods: &'a HashMap<u16, MethodAccess>,
    pub constants: &'a HashMap<u16, ConstVal>,
    pub sites: &'a HashMap<u16, SiteInfo>,
}

impl<'a> BuilderContext<'a> {
    pub fn empty() -> Self {
        static EMPTY_F: std::sync::LazyLock<HashMap<u16, FieldAccess>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_M: std::sync::LazyLock<HashMap<u16, MethodAccess>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_C: std::sync::LazyLock<HashMap<u16, ConstVal>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_S: std::sync::LazyLock<HashMap<u16, SiteInfo>> =
            std::sync::LazyLock::new(HashMap::new);
        Self {
            fields: &EMPTY_F,
            methods: &EMPTY_M,
            constants: &EMPTY_C,
            sites: &EMPTY_S,
        }
    }

    pub fn with_sites(sites: &'a HashMap<u16, SiteInfo>) -> Self {
        let mut ctx = Self::empty();
        ctx.sites = sites;
        ctx
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExceptionHandler {
    pub start_pc: u32,
    pub end_pc: u32,
    pub handler_pc: u32,
    pub catch_type: u32,
}

#[derive(Clone, Debug)]
pub struct MethodCode<'a> {
    pub name: &'a str,
    pub max_locals: usize,
    pub max_stack: usize,
    pub param_slots: usize,
    pub code: &'a [u8],
    pub handlers: &'a [ExceptionHandler],
}

#[derive(Clone, Debug, Default)]
pub struct BuildResult {
    pub blocks: usize,
    pub instructions: usize,
    pub returns: usize,
    pub throws: usize,
    pub unsupported: Vec<String>,
    pub warnings: usize,
}

#[derive(Clone, Copy, Debug)]
enum PredRef {
    Normal,
    Exc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuccKind {
    Fall,
    True,
    False,
    Case(usize),
}

#[derive(Clone, Copy, Debug)]
struct Succ {
    target: usize,
    slot: usize,
}

#[derive(Clone, Debug)]
struct EdgeInfo {
    ctrl: NodeId,
    mem: NodeId,
    locals: Vec<NodeId>,
    stack: Vec<StackVal>,
}

#[derive(Clone, Debug)]
struct OpenInfo {
    region: NodeId,
    phis: Vec<(NodeId, usize)>,
    mem_merge: NodeId,
    stack_base: usize,
}

#[derive(Clone, Debug)]
struct Block {
    start: usize,
    end: usize,
    preds: Vec<PredRef>,
    succs: Vec<Succ>,
    open: Option<OpenInfo>,
}

#[derive(Clone, Copy, Debug)]
struct ExcEdge {
    handler_block: usize,
    slot: usize,
}

#[derive(Clone, Copy, Debug)]
struct StackVal {
    node: NodeId,
    cat2: bool,
}

struct Frame {
    locals: Vec<NodeId>,
    stack: Vec<StackVal>,
    mem: NodeId,
    ctrl: NodeId,
}

/// Kind of one OSR marshal-buffer slot, captured from the interpreter
/// frame at the backedge that triggered compilation.  Types at a given
/// bytecode index are static, so request-time kinds stay valid at the
/// later OSR entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsrKind {
    I32,
    I64,
    F32,
    F64,
    Ref,
}

/// OSR compilation request: enter at `entry_bci` (a backward-branch
/// target) with `local_kinds` describing every local slot
/// (`max_locals` entries, padded with `I32` when short) and
/// `stack_kinds` the operand-stack values live at that point.
pub struct OsrSpec {
    pub entry_bci: u32,
    pub local_kinds: Vec<OsrKind>,
    pub stack_kinds: Vec<OsrKind>,
}

pub struct BytecodeGraphBuilder<'a> {
    gvn: &'a mut PhaseGVN,
    code: &'a [u8],
    ctx: &'a BuilderContext<'a>,
    blocks: Vec<Block>,
    exc_edges: Vec<ExcEdge>,
    exc_at: HashMap<usize, Vec<usize>>,
    delivered: HashMap<(usize, usize), EdgeInfo>,
    edge_ctrls: Vec<NodeId>,
    cur_block: usize,
    root_id: NodeId,
    zero: NodeId,
    halt: NodeId,
    site_cache: HashMap<u16, NodeId>,
    result: BuildResult,
}

impl<'a> BytecodeGraphBuilder<'a> {
    pub fn build(
        gvn: &'a mut PhaseGVN,
        method: &'a MethodCode<'a>,
        ctx: &'a BuilderContext<'a>,
        start: NodeId,
        root: NodeId,
    ) -> BuildResult {
        Self::build_inner(gvn, method, ctx, start, root, None)
    }

    /// OSR variant: the graph entry is the block containing
    /// `osr.entry_bci`; its frame is seeded from `OsrArg` nodes that
    /// read the interpreter-marshalled buffer at run time.  Blocks not
    /// reachable from the entry are dropped from the CFG.
    pub fn build_osr(
        gvn: &'a mut PhaseGVN,
        method: &'a MethodCode<'a>,
        ctx: &'a BuilderContext<'a>,
        start: NodeId,
        root: NodeId,
        osr: &OsrSpec,
    ) -> BuildResult {
        Self::build_inner(gvn, method, ctx, start, root, Some(osr))
    }

    fn build_inner(
        gvn: &'a mut PhaseGVN,
        method: &'a MethodCode<'a>,
        ctx: &'a BuilderContext<'a>,
        start: NodeId,
        root: NodeId,
        osr_spec: Option<&OsrSpec>,
    ) -> BuildResult {
        let mut b = Self {
            gvn,
            code: method.code,
            ctx,
            blocks: Vec::new(),
            exc_edges: Vec::new(),
            exc_at: HashMap::new(),
            delivered: HashMap::new(),
            edge_ctrls: Vec::new(),
            cur_block: usize::MAX,
            root_id: root,
            zero: NONE,
            halt: NONE,
            site_cache: HashMap::new(),
            result: BuildResult::default(),
        };
        b.zero = b.gvn.con_i(0);
        b.halt = b.gvn.alloc(Opcode::Halt, vec![]);
        b.build_cfg(method);
        b.build_exception_edges(method);
        if let Some(osr) = osr_spec {
            b.restrict_to_osr(method, osr, start);
        }
        for i in 0..b.blocks.len() {
            b.process_block(i, method, start);
        }
        b.finish();
        b.result.blocks = b.blocks.len();
        b.result
    }

    fn instr_len(&self, bci: usize) -> usize {
        let op = self.code[bci];
        if op == 0xaa {
            if let Some(ts) = bytecode::BytecodeTableswitch::new(self.code, bci) {
                if let (Some(low), Some(high)) = (ts.low_key(), ts.high_key()) {
                    if high >= low {
                        let n = (high - low + 1) as usize;
                        let pad = (4 - ((bci + 1) % 4)) % 4;
                        return 1 + pad + 12 + 4 * n;
                    }
                }
            }
            return 1;
        }
        if op == 0xab {
            if let Some(ls) = bytecode::BytecodeLookupswitch::new(self.code, bci) {
                if let Some(n) = ls.number_of_pairs() {
                    let pad = (4 - ((bci + 1) % 4)) % 4;
                    return 1 + pad + 8 + 8 * n;
                }
            }
            return 1;
        }
        bytecodes::length_at(self.code, bci)
    }

    fn build_cfg(&mut self, method: &MethodCode) {
        let code = method.code;
        let mut leaders: BTreeSet<usize> = BTreeSet::new();
        leaders.insert(0);
        let mut bci = 0usize;
        while bci < code.len() {
            let len = self.instr_len(bci).max(1);
            let next = bci + len;
            if let Some(bc) = Bytecode::new(code, bci) {
                if let Some(c) = bc.code() {
                    match c {
                        Code::Ifeq
                        | Code::Ifne
                        | Code::Iflt
                        | Code::Ifge
                        | Code::Ifgt
                        | Code::Ifle
                        | Code::IfIcmpeq
                        | Code::IfIcmpne
                        | Code::IfIcmplt
                        | Code::IfIcmpge
                        | Code::IfIcmpgt
                        | Code::IfIcmple
                        | Code::IfAcmpeq
                        | Code::IfAcmpne
                        | Code::Ifnull
                        | Code::Ifnonnull => {
                            if let Some(t) = bc.get_s2(1) {
                                let tgt = (bci as i64 + t as i64) as usize;
                                if tgt < code.len() {
                                    leaders.insert(tgt);
                                }
                            }
                            leaders.insert(next);
                        }
                        Code::Goto | Code::Jsr => {
                            if let Some(t) = bc.get_s2(1) {
                                let tgt = (bci as i64 + t as i64) as usize;
                                if tgt < code.len() {
                                    leaders.insert(tgt);
                                }
                            }
                            leaders.insert(next);
                        }
                        Code::GotoW | Code::JsrW => {
                            if let Some(t) = bc.get_s4(1) {
                                let tgt = (bci as i64 + t as i64) as usize;
                                if tgt < code.len() {
                                    leaders.insert(tgt);
                                }
                            }
                            leaders.insert(next);
                        }
                        Code::Tableswitch => {
                            if let Some(ts) = bytecode::BytecodeTableswitch::new(code, bci) {
                                if let Some(d) = ts.default_offset() {
                                    let t = (bci as i64 + d as i64) as usize;
                                    if t < code.len() {
                                        leaders.insert(t);
                                    }
                                }
                                if let (Some(lo), Some(hi)) = (ts.low_key(), ts.high_key()) {
                                    for k in 0..=(hi - lo) {
                                        if let Some(off) = ts.dest_offset_at(k as usize) {
                                            let t = (bci as i64 + off as i64) as usize;
                                            if t < code.len() {
                                                leaders.insert(t);
                                            }
                                        }
                                    }
                                }
                            }
                            leaders.insert(next);
                        }
                        Code::Lookupswitch => {
                            if let Some(ls) = bytecode::BytecodeLookupswitch::new(code, bci) {
                                if let Some(d) = ls.default_offset() {
                                    let t = (bci as i64 + d as i64) as usize;
                                    if t < code.len() {
                                        leaders.insert(t);
                                    }
                                }
                                if let Some(n) = ls.number_of_pairs() {
                                    for i in 0..n {
                                        if let Some(off) = ls.offset_at(i) {
                                            let t = (bci as i64 + off as i64) as usize;
                                            if t < code.len() {
                                                leaders.insert(t);
                                            }
                                        }
                                    }
                                }
                            }
                            leaders.insert(next);
                        }
                        Code::Ireturn
                        | Code::Lreturn
                        | Code::Freturn
                        | Code::Dreturn
                        | Code::Areturn
                        | Code::Return
                        | Code::Athrow => {
                            leaders.insert(next);
                        }
                        _ => {}
                    }
                }
            }
            bci = next;
        }
        for h in method.handlers {
            let hp = h.handler_pc as usize;
            if hp < code.len() {
                leaders.insert(hp);
            }
        }
        let leaders: Vec<usize> = leaders.into_iter().collect();
        for (i, &l) in leaders.iter().enumerate() {
            let end = if i + 1 < leaders.len() {
                leaders[i + 1]
            } else {
                code.len()
            };
            self.blocks.push(Block {
                start: l,
                end,
                preds: Vec::new(),
                succs: Vec::new(),
                open: None,
            });
        }
        self.build_edges();
    }

    fn block_index_of(&self, bci: usize) -> Option<usize> {
        self.blocks
            .iter()
            .position(|b| bci >= b.start && bci < b.end)
    }

    fn target_block(&self, bci: i64, code_len: usize) -> Option<usize> {
        if bci < 0 || bci as usize >= code_len {
            return None;
        }
        self.block_index_of(bci as usize)
    }

    fn build_edges(&mut self) {
        let n = self.blocks.len();
        let code_len = self.code.len();
        let mut succs: Vec<Vec<(usize, SuccKind)>> = vec![Vec::new(); n];
        for i in 0..n {
            let (s, e) = (self.blocks[i].start, self.blocks[i].end);
            let mut bci = s;
            let mut term = false;
            while bci < e && bci < code_len {
                let len = self.instr_len(bci).max(1);
                if let Some(bc) = Bytecode::new(self.code, bci) {
                    if let Some(c) = bc.code() {
                        match c {
                            Code::Ifeq
                            | Code::Ifne
                            | Code::Iflt
                            | Code::Ifge
                            | Code::Ifgt
                            | Code::Ifle
                            | Code::IfIcmpeq
                            | Code::IfIcmpne
                            | Code::IfIcmplt
                            | Code::IfIcmpge
                            | Code::IfIcmpgt
                            | Code::IfIcmple
                            | Code::IfAcmpeq
                            | Code::IfAcmpne
                            | Code::Ifnull
                            | Code::Ifnonnull => {
                                if let Some(t) = bc.get_s2(1) {
                                    let tgt = bci as i64 + t as i64;
                                    if let Some(tb) = self.target_block(tgt, code_len) {
                                        succs[i].push((tb, SuccKind::True));
                                    }
                                }
                                if let Some(fb) = self.target_block((bci + len) as i64, code_len) {
                                    succs[i].push((fb, SuccKind::False));
                                }
                            }
                            Code::Goto | Code::Jsr => {
                                if let Some(t) = bc.get_s2(1) {
                                    let tgt = bci as i64 + t as i64;
                                    if let Some(tb) = self.target_block(tgt, code_len) {
                                        succs[i].push((tb, SuccKind::Fall));
                                    }
                                }
                                if matches!(c, Code::Jsr) {
                                    if let Some(fb) =
                                        self.target_block((bci + len) as i64, code_len)
                                    {
                                        succs[i].push((fb, SuccKind::Fall));
                                    }
                                }
                            }
                            Code::GotoW | Code::JsrW => {
                                if let Some(t) = bc.get_s4(1) {
                                    let tgt = bci as i64 + t as i64;
                                    if let Some(tb) = self.target_block(tgt, code_len) {
                                        succs[i].push((tb, SuccKind::Fall));
                                    }
                                }
                                if matches!(c, Code::JsrW) {
                                    if let Some(fb) =
                                        self.target_block((bci + len) as i64, code_len)
                                    {
                                        succs[i].push((fb, SuccKind::Fall));
                                    }
                                }
                            }
                            Code::Tableswitch => {
                                if let Some(ts) = bytecode::BytecodeTableswitch::new(self.code, bci)
                                {
                                    if let (Some(lo), Some(hi)) = (ts.low_key(), ts.high_key()) {
                                        for k in 0..=(hi - lo) {
                                            if let Some(off) = ts.dest_offset_at(k as usize) {
                                                let tgt = bci as i64 + off as i64;
                                                if let Some(tb) = self.target_block(tgt, code_len) {
                                                    succs[i].push((tb, SuccKind::Case(k as usize)));
                                                }
                                            }
                                        }
                                    }
                                    if let Some(d) = ts.default_offset() {
                                        let tgt = bci as i64 + d as i64;
                                        if let Some(tb) = self.target_block(tgt, code_len) {
                                            succs[i].push((tb, SuccKind::Fall));
                                        }
                                    }
                                }
                            }
                            Code::Lookupswitch => {
                                if let Some(ls) =
                                    bytecode::BytecodeLookupswitch::new(self.code, bci)
                                {
                                    if let Some(np) = ls.number_of_pairs() {
                                        for k in 0..np {
                                            if let Some(off) = ls.offset_at(k) {
                                                let tgt = bci as i64 + off as i64;
                                                if let Some(tb) = self.target_block(tgt, code_len) {
                                                    succs[i].push((tb, SuccKind::Case(k)));
                                                }
                                            }
                                        }
                                    }
                                    if let Some(d) = ls.default_offset() {
                                        let tgt = bci as i64 + d as i64;
                                        if let Some(tb) = self.target_block(tgt, code_len) {
                                            succs[i].push((tb, SuccKind::Fall));
                                        }
                                    }
                                }
                            }
                            Code::Ireturn
                            | Code::Lreturn
                            | Code::Freturn
                            | Code::Dreturn
                            | Code::Areturn
                            | Code::Return
                            | Code::Athrow
                            | Code::Ret => {
                                term = true;
                            }
                            _ => {}
                        }
                    }
                }
                bci += len;
            }
            if succs[i].is_empty() && !term && i + 1 < n {
                succs[i].push((i + 1, SuccKind::Fall));
            }
        }
        for i in 0..n {
            for (tb, _kind) in succs[i].drain(..) {
                let slot = self.blocks[tb].preds.len();
                self.blocks[tb].preds.push(PredRef::Normal);
                self.blocks[i].succs.push(Succ { target: tb, slot });
            }
        }
    }

    /// Restricts the CFG to blocks reachable from the OSR entry and
    /// seeds the entry block's frame from marshal-buffer inputs.  The
    /// seed is injected as an extra already-delivered predecessor so
    /// the normal merge/phi machinery treats it like any other edge.
    fn restrict_to_osr(&mut self, method: &MethodCode, osr: &OsrSpec, start: NodeId) {
        if !method.handlers.is_empty() {
            self.result
                .unsupported
                .push("osr: exception handlers are not supported".to_string());
            return;
        }
        let entry_bci = osr.entry_bci as usize;
        let Some(entry) = self.blocks.iter().position(|b| b.start == entry_bci) else {
            self.result
                .unsupported
                .push(format!("osr: bci {} is not a block start", osr.entry_bci));
            return;
        };
        let n = self.blocks.len();
        let mut reach = vec![false; n];
        let mut work = vec![entry];
        reach[entry] = true;
        while let Some(b) = work.pop() {
            for s in &self.blocks[b].succs {
                if !reach[s.target] {
                    reach[s.target] = true;
                    work.push(s.target);
                }
            }
        }
        // Remap old block indices to the filtered list.
        let mut map = vec![usize::MAX; n];
        let mut new_blocks: Vec<Block> = Vec::new();
        for (i, blk) in self.blocks.iter().enumerate() {
            if reach[i] {
                map[i] = new_blocks.len();
                new_blocks.push(blk.clone());
            }
        }
        for blk in new_blocks.iter_mut() {
            blk.preds.clear();
            blk.succs.clear();
        }
        for i in 0..n {
            if !reach[i] {
                continue;
            }
            let ni = map[i];
            let succs = self.blocks[i].succs.clone();
            for s in succs {
                if !reach[s.target] {
                    continue;
                }
                let nt = map[s.target];
                let slot = new_blocks[nt].preds.len();
                new_blocks[nt].preds.push(PredRef::Normal);
                new_blocks[ni].succs.push(Succ {
                    target: nt,
                    slot,
                });
            }
        }
        self.blocks = new_blocks;
        let entry = map[entry];

        // Seed frame: locals come from buffer slots [0, max_locals),
        // stack values from [max_locals, max_locals + depth).
        let mut locals = Vec::with_capacity(method.max_locals);
        for i in 0..method.max_locals {
            let kind = osr.local_kinds.get(i).copied().unwrap_or(OsrKind::I32);
            locals.push(self.osr_node(kind, i as u32));
        }
        let mut stack = Vec::with_capacity(osr.stack_kinds.len());
        for (j, &kind) in osr.stack_kinds.iter().enumerate() {
            let cat2 = matches!(kind, OsrKind::I64 | OsrKind::F64);
            stack.push(StackVal {
                node: self.osr_node(kind, method.max_locals as u32 + j as u32),
                cat2,
            });
        }
        let seed_slot = self.blocks[entry].preds.len();
        self.blocks[entry].preds.push(PredRef::Normal);
        self.delivered.insert(
            (entry, seed_slot),
            EdgeInfo {
                ctrl: start,
                mem: start,
                locals,
                stack,
            },
        );
    }

    fn osr_node(&mut self, kind: OsrKind, idx: u32) -> NodeId {
        let op = match kind {
            OsrKind::I32 => Opcode::OsrI(idx),
            OsrKind::I64 => Opcode::OsrL(idx),
            OsrKind::F32 => Opcode::OsrF(idx),
            OsrKind::F64 => Opcode::OsrD(idx),
            OsrKind::Ref => Opcode::OsrP(idx),
        };
        self.gvn.alloc(op, vec![])
    }

    fn build_exception_edges(&mut self, method: &MethodCode) {
        for (hi, h) in method.handlers.iter().enumerate() {
            let hb = match self.block_index_of(h.handler_pc as usize) {
                Some(b) => b,
                None => continue,
            };
            let _ = hi;
            let (s, e) = (h.start_pc as usize, h.end_pc as usize);
            for bci in self.throwing_bcis(s, e) {
                let edge_id = self.exc_edges.len();
                let slot = self.blocks[hb].preds.len();
                self.blocks[hb].preds.push(PredRef::Exc);
                self.exc_edges.push(ExcEdge {
                    handler_block: hb,
                    slot,
                });
                self.exc_at.entry(bci).or_default().push(edge_id);
            }
        }
    }

    fn throwing_bcis(&self, s: usize, e: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut bci = s;
        while bci < e && bci < self.code.len() {
            let op = self.code[bci];
            let sub = if op == 0xc4 && bci + 1 < self.code.len() {
                self.code[bci + 1]
            } else {
                op
            };
            if bytecodes::can_trap(sub) {
                out.push(bci);
            }
            bci += self.instr_len(bci).max(1);
        }
        out
    }

    fn process_block(&mut self, idx: usize, method: &MethodCode, start: NodeId) {
        self.cur_block = idx;
        let preds = self.blocks[idx].preds.clone();
        let k = preds.len();
        let frame = if k == 0 {
            self.fresh_frame(method, start)
        } else {
            let mut infos: Vec<Option<EdgeInfo>> = Vec::with_capacity(k);
            for slot in 0..k {
                infos.push(self.delivered.get(&(idx, slot)).cloned());
            }
            if k == 1 && infos[0].is_some() {
                let info = infos[0].take().unwrap();
                self.adopt_frame(method, info)
            } else {
                self.merge_frame(method, idx, k, infos)
            }
        };
        let mut frame = frame;
        self.simulate(idx, method, &mut frame);
    }

    fn fresh_frame(&mut self, method: &MethodCode, start: NodeId) -> Frame {
        let mut locals = Vec::with_capacity(method.max_locals);
        for i in 0..method.max_locals {
            if i < method.param_slots {
                locals.push(self.gvn.alloc(Opcode::Parm(i as u16), vec![]));
            } else {
                locals.push(self.zero);
            }
        }
        Frame {
            locals,
            stack: Vec::new(),
            mem: start,
            ctrl: start,
        }
    }

    fn adopt_frame(&mut self, method: &MethodCode, info: EdgeInfo) -> Frame {
        let mut locals = info.locals;
        locals.resize(method.max_locals, self.zero);
        locals.truncate(method.max_locals);
        Frame {
            locals,
            stack: info.stack,
            mem: info.mem,
            ctrl: info.ctrl,
        }
    }

    fn merge_frame(
        &mut self,
        method: &MethodCode,
        idx: usize,
        k: usize,
        infos: Vec<Option<EdgeInfo>>,
    ) -> Frame {
        let nlocals = method.max_locals;
        let mut stack_h = 0usize;
        for info in infos.iter().flatten() {
            stack_h = stack_h.max(info.stack.len());
        }
        let frame_len = nlocals + stack_h;
        // Seed the Region inputs from already-delivered predecessors; edges
        // that arrive later (backedges) are patched in by `deliver`.
        let region_inputs: Vec<NodeId> = infos
            .iter()
            .map(|i| i.as_ref().map(|x| x.ctrl).unwrap_or(NONE))
            .collect();
        let region = self.gvn.alloc(Opcode::Region, region_inputs);
        let mut phis: Vec<(NodeId, usize)> = Vec::new();
        let mut merged: Vec<NodeId> = Vec::with_capacity(frame_len);
        for e in 0..frame_len {
            let mut vals: Vec<NodeId> = Vec::with_capacity(k);
            let mut all_known = true;
            let mut all_same = true;
            let mut first: Option<NodeId> = None;
            for info in infos.iter() {
                let v = match info {
                    Some(i) => self.frame_elem(i, e, nlocals),
                    None => {
                        all_known = false;
                        all_same = false;
                        NONE
                    }
                };
                vals.push(v);
                if v != NONE {
                    match first {
                        None => first = Some(v),
                        Some(f) => {
                            if f != v {
                                all_same = false;
                            }
                        }
                    }
                }
            }
            if all_known && all_same {
                merged.push(first.unwrap_or(self.zero));
            } else {
                let mut inputs = vals;
                inputs.push(region);
                let phi = self.gvn.alloc(Opcode::Phi, inputs);
                phis.push((phi, e));
                merged.push(phi);
            }
        }
        let mut mems: Vec<NodeId> = Vec::with_capacity(k);
        for info in infos.iter() {
            mems.push(info.as_ref().map(|i| i.mem).unwrap_or(NONE));
        }
        mems.push(region);
        let mem_merge = self.gvn.alloc(Opcode::MergeMem, mems);
        self.blocks[idx].open = Some(OpenInfo {
            region,
            phis,
            mem_merge,
            stack_base: nlocals,
        });
        let mut locals = merged[..nlocals].to_vec();
        locals.resize(nlocals, self.zero);
        let mut stack: Vec<StackVal> = Vec::with_capacity(stack_h);
        for s in 0..stack_h {
            let e = nlocals + s;
            let node = merged.get(e).copied().unwrap_or(self.zero);
            let cat2 = infos
                .iter()
                .flatten()
                .find_map(|i| i.stack.get(s).map(|sv| sv.cat2))
                .unwrap_or(false);
            stack.push(StackVal { node, cat2 });
        }
        Frame {
            locals,
            stack,
            mem: mem_merge,
            ctrl: region,
        }
    }

    fn frame_elem(&self, info: &EdgeInfo, e: usize, nlocals: usize) -> NodeId {
        if e < nlocals {
            info.locals.get(e).copied().unwrap_or(self.zero)
        } else {
            info.stack
                .get(e - nlocals)
                .map(|sv| sv.node)
                .unwrap_or(self.zero)
        }
    }

    fn simulate(&mut self, idx: usize, method: &MethodCode, frame: &mut Frame) {
        let (s, e) = (self.blocks[idx].start, self.blocks[idx].end);
        let mut bci = s;
        self.edge_ctrls.clear();
        while bci < e && bci < self.code.len() {
            let len = self.instr_len(bci).max(1);
            self.exec_instr(bci, method, frame);
            self.result.instructions += 1;
            bci += len;
        }
        self.finish_block(idx, frame);
    }

    fn finish_block(&mut self, idx: usize, frame: &Frame) {
        let succs = self.blocks[idx].succs.clone();
        if succs.is_empty() {
            return;
        }
        for (i, s) in succs.iter().enumerate() {
            let ctrl = self.edge_ctrls.get(i).copied().unwrap_or(frame.ctrl);
            let info = EdgeInfo {
                ctrl,
                mem: frame.mem,
                locals: frame.locals.clone(),
                stack: frame.stack.clone(),
            };
            self.deliver(s.target, s.slot, info);
        }
    }

    fn deliver(&mut self, target: usize, slot: usize, info: EdgeInfo) {
        self.delivered.insert((target, slot), info.clone());
        let open = match &self.blocks[target].open {
            Some(o) => o.clone(),
            None => return,
        };
        if open.region != NONE {
            let len = self.gvn.node(open.region).inputs.len();
            if slot < len {
                self.gvn.node_mut(open.region).inputs[slot] = info.ctrl;
            }
        }
        let nlocals = open.stack_base;
        for &(phi, e) in &open.phis {
            let v = self.frame_elem(&info, e, nlocals);
            let inputs_len = self.gvn.node(phi).inputs.len();
            if slot + 1 < inputs_len {
                self.gvn.node_mut(phi).inputs[slot] = v;
            }
        }
        let mem_len = self.gvn.node(open.mem_merge).inputs.len();
        if slot + 1 < mem_len {
            self.gvn.node_mut(open.mem_merge).inputs[slot] = info.mem;
        }
    }

    fn pop(&mut self, frame: &mut Frame) -> NodeId {
        match frame.stack.pop() {
            Some(sv) => sv.node,
            None => {
                self.result.warnings += 1;
                self.zero
            }
        }
    }

    fn pop2_slots(&mut self, frame: &mut Frame) {
        match frame.stack.last() {
            Some(sv) if sv.cat2 => {
                frame.stack.pop();
            }
            _ => {
                frame.stack.pop();
                frame.stack.pop();
            }
        }
    }

    fn push(&mut self, frame: &mut Frame, node: NodeId, cat2: bool) {
        frame.stack.push(StackVal { node, cat2 });
    }

    fn load_op(kind: FieldKind) -> Opcode {
        match kind {
            FieldKind::Int => Opcode::LoadI,
            FieldKind::Long => Opcode::LoadL,
            FieldKind::Float => Opcode::LoadF,
            FieldKind::Double => Opcode::LoadD,
            FieldKind::Ref => Opcode::LoadP,
            FieldKind::Byte | FieldKind::Bool => Opcode::LoadB,
            FieldKind::Char => Opcode::LoadC,
            FieldKind::Short => Opcode::LoadS,
        }
    }

    fn store_op(kind: FieldKind) -> Opcode {
        match kind {
            FieldKind::Int => Opcode::StoreI,
            FieldKind::Long => Opcode::StoreL,
            FieldKind::Float => Opcode::StoreF,
            FieldKind::Double => Opcode::StoreD,
            FieldKind::Ref => Opcode::StoreP,
            FieldKind::Byte | FieldKind::Bool => Opcode::StoreB,
            FieldKind::Char => Opcode::StoreC,
            FieldKind::Short => Opcode::StoreS,
        }
    }

    fn array_elem(code: Code) -> Option<ArrayElemKind> {
        use ArrayElemKind::*;
        Some(match code {
            Code::Iaload => Int,
            Code::Laload => Long,
            Code::Faload => Float,
            Code::Daload => Double,
            Code::Aaload => Ref,
            Code::Baload => Byte,
            Code::Caload => Char,
            Code::Saload => Short,
            Code::Iastore => Int,
            Code::Lastore => Long,
            Code::Fastore => Float,
            Code::Dastore => Double,
            Code::Aastore => Ref,
            Code::Bastore => Byte,
            Code::Castore => Char,
            Code::Sastore => Short,
            _ => return None,
        })
    }

    fn site_node(&mut self, cp: u16) -> NodeId {
        if let Some(&n) = self.site_cache.get(&cp) {
            return n;
        }
        let ptr = self
            .ctx
            .sites
            .get(&cp)
            .map(|s| leak_stub_site(s))
            .unwrap_or(0);
        let n = self.gvn.transform(Opcode::SitePtr(ptr), vec![]);
        self.site_cache.insert(cp, n);
        n
    }

    fn field_kind_for(&self, cp: u16, fa: Option<&FieldAccess>) -> Option<FieldKind> {
        if let Some(fa) = fa {
            return Some(fa.kind);
        }
        let site = self.ctx.sites.get(&cp)?;
        kind_from_desc(&site.desc)
    }

    fn method_access_for(&self, cp: u16, has_receiver: bool) -> Option<MethodAccess> {
        if let Some(m) = self.ctx.methods.get(&cp) {
            return Some(*m);
        }
        let site = self.ctx.sites.get(&cp)?;
        MethodAccess::from_descriptor(&site.desc, has_receiver)
    }

    fn exec_instr(&mut self, bci: usize, method: &MethodCode, frame: &mut Frame) {
        let op_byte = self.code[bci];
        if op_byte == 0xc4 {
            if bci + 1 < self.code.len() {
                let sub = self.code[bci + 1];
                if let Some(c) = Code::from_u8(sub) {
                    self.exec_wide(bci + 1, c, frame);
                    return;
                }
            }
            self.unsupported(bci, "wide");
            return;
        }
        let bc = match Bytecode::new(self.code, bci) {
            Some(b) => b,
            None => return,
        };
        let code = match bc.code() {
            Some(c) => c,
            None => {
                self.unsupported(bci, "unknown opcode");
                return;
            }
        };
        self.exec_code(bci, code, &bc, method, frame);
    }

    fn unsupported(&mut self, bci: usize, what: &str) {
        let name = bytecodes::name(self.code[bci]).unwrap_or("?");
        self.result
            .unsupported
            .push(format!("bci {}: {} ({})", bci, name, what));
    }

    fn exec_wide(&mut self, bci: usize, code: Code, frame: &mut Frame) {
        let bc = Bytecode::new(self.code, bci).unwrap();
        match code {
            Code::Iload | Code::Lload | Code::Fload | Code::Dload | Code::Aload => {
                let idx = bc.get_u2(1).unwrap_or(0) as usize;
                let v = frame.locals.get(idx).copied().unwrap_or(self.zero);
                self.push(frame, v, false);
            }
            Code::Istore | Code::Lstore | Code::Fstore | Code::Dstore | Code::Astore => {
                let idx = bc.get_u2(1).unwrap_or(0) as usize;
                let v = self.pop(frame);
                if idx < frame.locals.len() {
                    frame.locals[idx] = v;
                } else {
                    self.result.warnings += 1;
                }
            }
            Code::Iinc => {
                // wide iinc: u2 index, s2 const (JVMS layout).
                let idx = bc.get_u2(1).unwrap_or(0) as usize;
                let c = bc.get_s2(3).unwrap_or(0) as i32;
                if idx < frame.locals.len() {
                    let k = self.gvn.con_i(c);
                    let cur = frame.locals[idx];
                    frame.locals[idx] = self.gvn.transform(Opcode::AddI, vec![cur, k]);
                } else {
                    self.result.warnings += 1;
                }
            }
            _ => self.unsupported(bci - 1, "wide"),
        }
    }

    fn local_get(&mut self, idx: usize, frame: &mut Frame) {
        let v = frame.locals.get(idx).copied().unwrap_or(self.zero);
        self.push(frame, v, false);
    }

    fn local_set(&mut self, idx: usize, frame: &mut Frame) {
        let v = self.pop(frame);
        if idx < frame.locals.len() {
            frame.locals[idx] = v;
        } else {
            self.result.warnings += 1;
        }
    }

    fn arith(&mut self, frame: &mut Frame, op: Opcode, cat2: bool) -> NodeId {
        let b = self.pop(frame);
        let a = self.pop(frame);
        // Integer div/rem can trap (ArithmeticException on a zero
        // divisor), so they are control+memory dependent like loads:
        // the extra inputs pin them to this program point, which both
        // schedules them and keeps GVN from merging a trapping div
        // inside a try region with an identical one outside it.
        let n = if matches!(
            op,
            Opcode::DivI | Opcode::DivL | Opcode::RemI | Opcode::RemL
        ) {
            self.gvn
                .transform(op, vec![frame.mem, frame.ctrl, a, b])
        } else {
            self.gvn.transform(op, vec![a, b])
        };
        self.push(frame, n, cat2);
        n
    }

    fn unop(&mut self, frame: &mut Frame, op: Opcode, cat2: bool) -> NodeId {
        let a = self.pop(frame);
        let n = self.gvn.transform(op, vec![a]);
        self.push(frame, n, cat2);
        n
    }

    fn emit_if(&mut self, ctrl: NodeId, mem: NodeId, test: NodeId) -> (NodeId, NodeId, NodeId) {
        let if_node = self.gvn.alloc(Opcode::If, vec![ctrl, test]);
        let ift = self.gvn.alloc(Opcode::IfTrue, vec![if_node, mem]);
        let iff = self.gvn.alloc(Opcode::IfFalse, vec![if_node, mem]);
        (if_node, ift, iff)
    }

    fn branch(&mut self, frame: &mut Frame, cond: CmpCondition, a: NodeId, b: Option<NodeId>) {
        let test = match b {
            Some(bn) => self.gvn.transform(Opcode::BoolCmp(cond), vec![a, bn]),
            None => {
                let zero = self.gvn.con_i(0);
                self.gvn.transform(Opcode::BoolCmp(cond), vec![a, zero])
            }
        };
        if let Some(tv) = self.gvn.node(test).as_bool() {
            let taken = if tv { frame.ctrl } else { self.halt };
            let other = if tv { self.halt } else { frame.ctrl };
            self.edge_ctrls = vec![taken, other];
            return;
        }
        let (_ifn, ift, iff) = self.emit_if(frame.ctrl, frame.mem, test);
        self.edge_ctrls = vec![ift, iff];
    }

    fn exec_code(
        &mut self,
        bci: usize,
        code: Code,
        bc: &Bytecode,
        method: &MethodCode,
        frame: &mut Frame,
    ) {
        use Code::*;
        match code {
            Nop => {}
            AconstNull => {
                let n = self.gvn.con_p(CON_P_NULL);
                self.push(frame, n, false);
            }
            IconstM1 => self.push_const_i(frame, -1),
            Iconst0 | Iconst1 | Iconst2 | Iconst3 | Iconst4 | Iconst5 => {
                let v = (code as i32) - (Iconst0 as i32);
                self.push_const_i(frame, v);
            }
            Lconst0 => {
                let n = self.gvn.con_l(0);
                self.push(frame, n, true);
            }
            Lconst1 => {
                let n = self.gvn.con_l(1);
                self.push(frame, n, true);
            }
            Fconst0 => self.push_f(frame, 0.0),
            Fconst1 => self.push_f(frame, 1.0),
            Fconst2 => self.push_f(frame, 2.0),
            Dconst0 => self.push_d(frame, 0.0),
            Dconst1 => self.push_d(frame, 1.0),
            Bipush => {
                let v = bc.get_u1(1).unwrap_or(0) as i8 as i32;
                self.push_const_i(frame, v);
            }
            Sipush => {
                let v = bc.get_s2(1).unwrap_or(0) as i32;
                self.push_const_i(frame, v);
            }
            Ldc | LdcW | Ldc2W => {
                // `ldc` carries a u1 index; `ldc_w`/`ldc2_w` carry u2.
                let idx = if code == Ldc {
                    bc.get_u1(1).unwrap_or(0) as u16
                } else {
                    bc.get_u2(1).unwrap_or(0)
                };
                match self.ctx.constants.get(&idx) {
                    Some(ConstVal::Int(v)) => self.push_const_i(frame, *v),
                    Some(ConstVal::Long(v)) => {
                        let n = self.gvn.con_l(*v);
                        self.push(frame, n, true);
                    }
                    Some(ConstVal::Float(v)) => self.push_f(frame, *v),
                    Some(ConstVal::Double(v)) => self.push_d(frame, *v),
                    Some(ConstVal::String(p)) => {
                        let n = self.gvn.con_p(*p);
                        self.push(frame, n, false);
                    }
                    None => {
                        self.unsupported(bci, "ldc unresolved");
                        let n = self.gvn.con_i(0);
                        self.push(frame, n, false);
                    }
                }
            }
            c if Self::is_local_load(c) => {
                let idx = self.local_index(code, bc);
                self.local_get(idx, frame);
            }
            c if Self::is_local_store(c) => {
                let idx = self.local_index(code, bc);
                self.local_set(idx, frame);
            }
            Iaload | Laload | Faload | Daload | Aaload | Baload | Caload | Saload => {
                let ekind = Self::array_elem(code).unwrap();
                let kind = match ekind {
                    ArrayElemKind::Long => FieldKind::Long,
                    ArrayElemKind::Float => FieldKind::Float,
                    ArrayElemKind::Double => FieldKind::Double,
                    ArrayElemKind::Ref => FieldKind::Ref,
                    ArrayElemKind::Byte => FieldKind::Byte,
                    ArrayElemKind::Char => FieldKind::Char,
                    ArrayElemKind::Short => FieldKind::Short,
                    ArrayElemKind::Int => FieldKind::Int,
                };
                let idx = self.pop(frame);
                let arr = self.pop(frame);
                let op = Self::load_op(kind);
                let ek = self.gvn.con_i(ekind.code());
                let n = self
                    .gvn
                    .transform(op, vec![frame.mem, frame.ctrl, arr, idx, ek]);
                let cat2 = matches!(kind, FieldKind::Long | FieldKind::Double);
                self.push(frame, n, cat2);
                self.maybe_throw(bci, n, frame);
            }
            Iastore | Lastore | Fastore | Dastore | Aastore | Bastore | Castore | Sastore => {
                let ekind = Self::array_elem(code).unwrap();
                let v = self.pop(frame);
                let idx = self.pop(frame);
                let arr = self.pop(frame);
                let kind = match ekind {
                    ArrayElemKind::Long => FieldKind::Long,
                    ArrayElemKind::Float => FieldKind::Float,
                    ArrayElemKind::Double => FieldKind::Double,
                    ArrayElemKind::Ref => FieldKind::Ref,
                    ArrayElemKind::Byte => FieldKind::Byte,
                    ArrayElemKind::Char => FieldKind::Char,
                    ArrayElemKind::Short => FieldKind::Short,
                    ArrayElemKind::Int => FieldKind::Int,
                };
                let op = Self::store_op(kind);
                let ek = self.gvn.con_i(ekind.code());
                let n = self
                    .gvn
                    .alloc(op, vec![frame.mem, frame.ctrl, arr, idx, v, ek]);
                frame.mem = n;
                self.maybe_throw(bci, n, frame);
            }
            Pop => {
                self.pop(frame);
            }
            Pop2 => self.pop2_slots(frame),
            Dup => {
                if let Some(top) = frame.stack.last().copied() {
                    self.push(frame, top.node, top.cat2);
                } else {
                    self.result.warnings += 1;
                }
            }
            DupX1 => {
                let n = frame.stack.len();
                if n >= 2 {
                    let top = frame.stack[n - 1];
                    frame.stack.insert(n - 2, top);
                } else {
                    self.result.warnings += 1;
                }
            }
            DupX2 => {
                let n = frame.stack.len();
                if n >= 3 {
                    let top = frame.stack[n - 1];
                    frame.stack.insert(n - 3, top);
                } else {
                    self.result.warnings += 1;
                }
            }
            Dup2 => {
                let n = frame.stack.len();
                if n >= 1 && frame.stack[n - 1].cat2 {
                    let t = frame.stack[n - 1];
                    self.push(frame, t.node, true);
                } else if n >= 2 {
                    let a = frame.stack[n - 2];
                    let b = frame.stack[n - 1];
                    self.push(frame, a.node, a.cat2);
                    self.push(frame, b.node, b.cat2);
                } else {
                    self.result.warnings += 1;
                }
            }
            Dup2X1 => {
                let n = frame.stack.len();
                if n >= 3 {
                    let e3 = frame.stack[n - 1];
                    let e2 = frame.stack[n - 2];
                    let e1 = frame.stack[n - 3];
                    frame.stack.truncate(n - 3);
                    frame.stack.push(e2);
                    frame.stack.push(e3);
                    frame.stack.push(e1);
                    frame.stack.push(e2);
                    frame.stack.push(e3);
                } else {
                    self.result.warnings += 1;
                }
            }
            Dup2X2 => {
                let n = frame.stack.len();
                if n >= 4 {
                    let e4 = frame.stack[n - 1];
                    let e3 = frame.stack[n - 2];
                    let e2 = frame.stack[n - 3];
                    let e1 = frame.stack[n - 4];
                    frame.stack.truncate(n - 4);
                    frame.stack.push(e3);
                    frame.stack.push(e4);
                    frame.stack.push(e1);
                    frame.stack.push(e2);
                    frame.stack.push(e3);
                    frame.stack.push(e4);
                } else {
                    self.result.warnings += 1;
                }
            }
            Swap => {
                let n = frame.stack.len();
                if n >= 2 {
                    frame.stack.swap(n - 1, n - 2);
                } else {
                    self.result.warnings += 1;
                }
            }
            Iadd => {
                self.arith(frame, Opcode::AddI, false);
            }
            Ladd => {
                self.arith(frame, Opcode::AddL, true);
            }
            Fadd => {
                self.arith(frame, Opcode::AddF, false);
            }
            Dadd => {
                self.arith(frame, Opcode::AddD, true);
            }
            Isub => {
                self.arith(frame, Opcode::SubI, false);
            }
            Lsub => {
                self.arith(frame, Opcode::SubL, true);
            }
            Fsub => {
                self.arith(frame, Opcode::SubF, false);
            }
            Dsub => {
                self.arith(frame, Opcode::SubD, true);
            }
            Imul => {
                self.arith(frame, Opcode::MulI, false);
            }
            Lmul => {
                self.arith(frame, Opcode::MulL, true);
            }
            Fmul => {
                self.arith(frame, Opcode::MulF, false);
            }
            Dmul => {
                self.arith(frame, Opcode::MulD, true);
            }
            Idiv => {
                let n = self.arith(frame, Opcode::DivI, false);
                self.maybe_throw(bci, n, frame);
            }
            Ldiv => {
                let n = self.arith(frame, Opcode::DivL, true);
                self.maybe_throw(bci, n, frame);
            }
            Fdiv => {
                self.arith(frame, Opcode::DivF, false);
            }
            Ddiv => {
                self.arith(frame, Opcode::DivD, true);
            }
            Irem => {
                let n = self.arith(frame, Opcode::RemI, false);
                self.maybe_throw(bci, n, frame);
            }
            Lrem => {
                let n = self.arith(frame, Opcode::RemL, true);
                self.maybe_throw(bci, n, frame);
            }
            Frem => {
                self.arith(frame, Opcode::RemF, false);
            }
            Drem => {
                self.arith(frame, Opcode::RemD, true);
            }
            Ineg => {
                self.unop(frame, Opcode::NegI, false);
            }
            Lneg => {
                self.unop(frame, Opcode::NegL, true);
            }
            Fneg => {
                self.unop(frame, Opcode::NegF, false);
            }
            Dneg => {
                self.unop(frame, Opcode::NegD, true);
            }
            Ishl => {
                self.arith(frame, Opcode::LShiftI, false);
            }
            Lshl => {
                self.arith(frame, Opcode::LShiftL, true);
            }
            Ishr => {
                self.arith(frame, Opcode::RShiftI, false);
            }
            Lshr => {
                self.arith(frame, Opcode::RShiftL, true);
            }
            Iushr => {
                self.arith(frame, Opcode::URShiftI, false);
            }
            Lushr => {
                self.arith(frame, Opcode::URShiftL, true);
            }
            Iand => {
                self.arith(frame, Opcode::AndI, false);
            }
            Land => {
                self.arith(frame, Opcode::AndL, true);
            }
            Ior => {
                self.arith(frame, Opcode::OrI, false);
            }
            Lor => {
                self.arith(frame, Opcode::OrL, true);
            }
            Ixor => {
                self.arith(frame, Opcode::XorI, false);
            }
            Lxor => {
                self.arith(frame, Opcode::XorL, true);
            }
            Iinc => {
                let idx = bc.get_u1(1).unwrap_or(0) as usize;
                let c = bc.get_u1(2).unwrap_or(0) as i8 as i32;
                if idx < frame.locals.len() {
                    let k = self.gvn.con_i(c);
                    let cur = frame.locals[idx];
                    frame.locals[idx] = self.gvn.transform(Opcode::AddI, vec![cur, k]);
                } else {
                    self.result.warnings += 1;
                }
            }
            I2l => {
                self.unop(frame, Opcode::I2L, true);
            }
            I2f => {
                self.unop(frame, Opcode::I2F, false);
            }
            I2d => {
                self.unop(frame, Opcode::I2D, true);
            }
            L2i => {
                self.unop(frame, Opcode::L2I, false);
            }
            L2f => {
                self.unop(frame, Opcode::L2F, false);
            }
            L2d => {
                self.unop(frame, Opcode::L2D, true);
            }
            F2i => {
                self.unop(frame, Opcode::F2I, false);
            }
            F2l => {
                self.unop(frame, Opcode::F2L, true);
            }
            F2d => {
                self.unop(frame, Opcode::F2D, true);
            }
            D2i => {
                self.unop(frame, Opcode::D2I, false);
            }
            D2l => {
                self.unop(frame, Opcode::D2L, true);
            }
            D2f => {
                self.unop(frame, Opcode::D2F, false);
            }
            I2b => {
                self.unop(frame, Opcode::I2B, false);
            }
            I2c => {
                self.unop(frame, Opcode::I2C, false);
            }
            I2s => {
                self.unop(frame, Opcode::I2S, false);
            }
            Lcmp => {
                self.arith(frame, Opcode::CmpL, false);
            }
            Fcmpl => {
                self.arith(frame, Opcode::CmpF(false), false);
            }
            Fcmpg => {
                self.arith(frame, Opcode::CmpF(true), false);
            }
            Dcmpl => {
                self.arith(frame, Opcode::CmpD(false), false);
            }
            Dcmpg => {
                self.arith(frame, Opcode::CmpD(true), false);
            }
            Ifeq => self.branch_un(frame, CmpCondition::Eq),
            Ifne => self.branch_un(frame, CmpCondition::Ne),
            Iflt => self.branch_un(frame, CmpCondition::Lt),
            Ifge => self.branch_un(frame, CmpCondition::Ge),
            Ifgt => self.branch_un(frame, CmpCondition::Gt),
            Ifle => self.branch_un(frame, CmpCondition::Le),
            Ifnull => {
                let a = self.pop(frame);
                let null = self.gvn.con_p(CON_P_NULL);
                self.branch(frame, CmpCondition::Eq, a, Some(null));
            }
            Ifnonnull => {
                let a = self.pop(frame);
                let null = self.gvn.con_p(CON_P_NULL);
                self.branch(frame, CmpCondition::Ne, a, Some(null));
            }
            IfIcmpeq => self.branch_bin(frame, CmpCondition::Eq),
            IfIcmpne => self.branch_bin(frame, CmpCondition::Ne),
            IfIcmplt => self.branch_bin(frame, CmpCondition::Lt),
            IfIcmpge => self.branch_bin(frame, CmpCondition::Ge),
            IfIcmpgt => self.branch_bin(frame, CmpCondition::Gt),
            IfIcmple => self.branch_bin(frame, CmpCondition::Le),
            IfAcmpeq => self.branch_bin(frame, CmpCondition::Eq),
            IfAcmpne => self.branch_bin(frame, CmpCondition::Ne),
            Goto | GotoW => {
                self.edge_ctrls = vec![frame.ctrl];
            }
            Jsr | JsrW | Ret | Breakpoint => self.unsupported(bci, "subroutine/breakpoint"),
            Tableswitch => self.exec_tableswitch(bci, frame),
            Lookupswitch => self.exec_lookupswitch(bci, frame),
            Ireturn | Lreturn | Freturn | Dreturn | Areturn => {
                let v = self.pop(frame);
                let r = self
                    .gvn
                    .alloc(Opcode::Return, vec![frame.ctrl, frame.mem, v]);
                self.push_root(r);
                self.result.returns += 1;
            }
            Return => {
                let r = self.gvn.alloc(Opcode::Return, vec![frame.ctrl, frame.mem]);
                self.push_root(r);
                self.result.returns += 1;
            }
            Getstatic | Putstatic => {
                let cp = bc.get_u2(1).unwrap_or(0) as u16;
                let fa = self.ctx.fields.get(&cp).copied();
                match self.field_kind_for(cp, fa.as_ref()) {
                    Some(kind) => {
                        let site = self.site_node(cp);
                        if code == Getstatic {
                            let op = Self::load_op(kind);
                            let n = self.gvn.transform(op, vec![frame.mem, frame.ctrl, site]);
                            let cat2 = matches!(kind, FieldKind::Long | FieldKind::Double);
                            self.push(frame, n, cat2);
                            self.maybe_throw(bci, n, frame);
                        } else {
                            let v = self.pop(frame);
                            let op = Self::store_op(kind);
                            let n = self.gvn.alloc(op, vec![frame.mem, frame.ctrl, v, site]);
                            frame.mem = n;
                            self.maybe_throw(bci, n, frame);
                        }
                    }
                    None => self.unsupported(bci, "static field unresolved"),
                }
            }
            Getfield | Putfield => {
                let cp = bc.get_u2(1).unwrap_or(0) as u16;
                let fa = self.ctx.fields.get(&cp).copied();
                match self.field_kind_for(cp, fa.as_ref()) {
                    Some(kind) => {
                        let site = self.site_node(cp);
                        let off = fa.map(|f| f.offset as i32).unwrap_or(0);
                        let offn = self.gvn.con_i(off);
                        if code == Getfield {
                            let obj = self.pop(frame);
                            let op = Self::load_op(kind);
                            let n = self
                                .gvn
                                .transform(op, vec![frame.mem, frame.ctrl, obj, offn, site]);
                            let cat2 = matches!(kind, FieldKind::Long | FieldKind::Double);
                            self.push(frame, n, cat2);
                            self.maybe_throw(bci, n, frame);
                        } else {
                            let v = self.pop(frame);
                            let obj = self.pop(frame);
                            let op = Self::store_op(kind);
                            let n = self
                                .gvn
                                .alloc(op, vec![frame.mem, frame.ctrl, obj, offn, v, site]);
                            frame.mem = n;
                            self.maybe_throw(bci, n, frame);
                        }
                    }
                    None => self.unsupported(bci, "field unresolved"),
                }
            }
            Invokevirtual | Invokespecial | Invokestatic | Invokeinterface | Invokedynamic => {
                let cp = bc.get_u2(1).unwrap_or(0) as u16;
                let has_recv = !matches!(code, Invokestatic | Invokedynamic);
                let ma = self.method_access_for(cp, has_recv);
                let (nargs, has_receiver, res) = match ma {
                    Some(m) => (m.nargs, m.has_receiver, m.result),
                    None => {
                        self.unsupported(bci, "invoke unresolved");
                        (0, false, CallResult::Void)
                    }
                };
                let mut args: Vec<NodeId> = Vec::with_capacity(nargs);
                for _ in 0..nargs {
                    args.push(self.pop(frame));
                }
                args.reverse();
                let recv = if has_receiver {
                    Some(self.pop(frame))
                } else {
                    None
                };
                let mut inputs = vec![frame.mem, frame.ctrl];
                if let Some(r) = recv {
                    inputs.push(r);
                }
                inputs.extend(args);
                inputs.push(self.site_node(cp));
                let payload = call_pack(cp as u32, code as u8, res);
                let call = self.gvn.alloc(Opcode::Call(payload), inputs);
                frame.ctrl = call;
                frame.mem = call;
                if res != CallResult::Void {
                    self.push(
                        frame,
                        call,
                        matches!(res, CallResult::I64 | CallResult::F64),
                    );
                }
                self.maybe_throw(bci, call, frame);
            }
            New => {
                let cp = bc.get_u2(1).unwrap_or(0) as u16;
                let site = self.site_node(cp);
                let n = self
                    .gvn
                    .alloc(Opcode::New(cp as u32), vec![frame.mem, frame.ctrl, site]);
                frame.ctrl = n;
                frame.mem = n;
                self.push(frame, n, false);
                self.maybe_throw(bci, n, frame);
            }
            Newarray => {
                let atype = bc.get_u1(1).unwrap_or(0);
                let size = self.pop(frame);
                let n = self.gvn.alloc(
                    Opcode::NewArray(atype as u32),
                    vec![frame.mem, frame.ctrl, size],
                );
                frame.ctrl = n;
                frame.mem = n;
                self.push(frame, n, false);
                self.maybe_throw(bci, n, frame);
            }
            Anewarray => {
                let cp = bc.get_u2(1).unwrap_or(0) as u16;
                let size = self.pop(frame);
                let site = self.site_node(cp);
                let n = self.gvn.alloc(
                    Opcode::NewArrayRef(cp as u32),
                    vec![frame.mem, frame.ctrl, size, site],
                );
                frame.ctrl = n;
                frame.mem = n;
                self.push(frame, n, false);
                self.maybe_throw(bci, n, frame);
            }
            Multianewarray => {
                let dims = bc.get_u1(3).unwrap_or(0) as usize;
                for _ in 0..dims {
                    self.pop(frame);
                }
                let n = self
                    .gvn
                    .alloc(Opcode::MultianewArray, vec![frame.mem, frame.ctrl]);
                frame.ctrl = n;
                frame.mem = n;
                self.push(frame, n, false);
                self.maybe_throw(bci, n, frame);
            }
            Arraylength => {
                let arr = self.pop(frame);
                let n = self
                    .gvn
                    .alloc(Opcode::ArrayLength, vec![frame.mem, frame.ctrl, arr]);
                self.push(frame, n, false);
                self.maybe_throw(bci, n, frame);
            }
            Athrow => {
                let obj = self.pop(frame);
                let n = self
                    .gvn
                    .alloc(Opcode::Athrow, vec![frame.mem, frame.ctrl, obj]);
                self.result.throws += 1;
                if self.exc_at.contains_key(&bci) {
                    self.deliver_exc_athrow(bci, n, obj, frame);
                } else {
                    self.push_root(n);
                }
            }
            Checkcast => {
                let cp = bc.get_u2(1).unwrap_or(0) as u16;
                let obj = self.pop(frame);
                let site = self.site_node(cp);
                let n = self.gvn.transform(
                    Opcode::CheckCast(cp as u32),
                    vec![frame.mem, frame.ctrl, obj, site],
                );
                self.push(frame, n, false);
                self.maybe_throw(bci, n, frame);
            }
            Instanceof => {
                let cp = bc.get_u2(1).unwrap_or(0) as u16;
                let obj = self.pop(frame);
                let site = self.site_node(cp);
                let n = self.gvn.alloc(
                    Opcode::InstanceOf(cp as u32),
                    vec![frame.mem, frame.ctrl, obj, site],
                );
                self.push(frame, n, false);
                self.maybe_throw(bci, n, frame);
            }
            Monitorenter | Monitorexit => {
                let obj = self.pop(frame);
                let op = if code == Monitorenter {
                    Opcode::MonitorEnter
                } else {
                    Opcode::MonitorExit
                };
                let n = self.gvn.alloc(op, vec![frame.mem, frame.ctrl, obj]);
                frame.ctrl = n;
                frame.mem = n;
                self.maybe_throw(bci, n, frame);
            }
            _ => self.unsupported(bci, "unhandled"),
        }
        let _ = method;
    }

    fn push_root(&mut self, node: NodeId) {
        if self.root_id != NONE {
            self.gvn.node_mut(self.root_id).inputs.push(node);
        }
    }

    fn push_const_i(&mut self, frame: &mut Frame, v: i32) {
        let n = self.gvn.con_i(v);
        self.push(frame, n, false);
    }

    fn push_f(&mut self, frame: &mut Frame, v: f32) {
        let n = self.gvn.transform(Opcode::ConF(v.to_bits()), vec![]);
        self.push(frame, n, false);
    }

    fn push_d(&mut self, frame: &mut Frame, v: f64) {
        let n = self.gvn.transform(Opcode::ConD(v.to_bits()), vec![]);
        self.push(frame, n, true);
    }

    fn is_local_load(c: Code) -> bool {
        use Code::*;
        matches!(
            c,
            Iload
                | Iload0
                | Iload1
                | Iload2
                | Iload3
                | Lload
                | Lload0
                | Lload1
                | Lload2
                | Lload3
                | Fload
                | Fload0
                | Fload1
                | Fload2
                | Fload3
                | Dload
                | Dload0
                | Dload1
                | Dload2
                | Dload3
                | Aload
                | Aload0
                | Aload1
                | Aload2
                | Aload3
        )
    }

    fn is_local_store(c: Code) -> bool {
        use Code::*;
        matches!(
            c,
            Istore
                | Istore0
                | Istore1
                | Istore2
                | Istore3
                | Lstore
                | Lstore0
                | Lstore1
                | Lstore2
                | Lstore3
                | Fstore
                | Fstore0
                | Fstore1
                | Fstore2
                | Fstore3
                | Dstore
                | Dstore0
                | Dstore1
                | Dstore2
                | Dstore3
                | Astore
                | Astore0
                | Astore1
                | Astore2
                | Astore3
        )
    }

    fn local_index(&self, code: Code, bc: &Bytecode) -> usize {
        use Code::*;
        match code {
            Iload0 | Lload0 | Fload0 | Dload0 | Aload0 | Istore0 | Lstore0 | Fstore0 | Dstore0
            | Astore0 => 0,
            Iload1 | Lload1 | Fload1 | Dload1 | Aload1 | Istore1 | Lstore1 | Fstore1 | Dstore1
            | Astore1 => 1,
            Iload2 | Lload2 | Fload2 | Dload2 | Aload2 | Istore2 | Lstore2 | Fstore2 | Dstore2
            | Astore2 => 2,
            Iload3 | Lload3 | Fload3 | Dload3 | Aload3 | Istore3 | Lstore3 | Fstore3 | Dstore3
            | Astore3 => 3,
            _ => bc.get_u1(1).unwrap_or(0) as usize,
        }
    }

    fn branch_un(&mut self, frame: &mut Frame, cond: CmpCondition) {
        let a = self.pop(frame);
        self.branch(frame, cond, a, None);
    }

    fn branch_bin(&mut self, frame: &mut Frame, cond: CmpCondition) {
        let b = self.pop(frame);
        let a = self.pop(frame);
        self.branch(frame, cond, a, Some(b));
    }

    fn exec_tableswitch(&mut self, bci: usize, frame: &mut Frame) {
        let val = self.pop(frame);
        if let Some(ts) = bytecode::BytecodeTableswitch::new(self.code, bci) {
            let lo = ts.low_key().unwrap_or(0);
            let hi = ts.high_key().unwrap_or(lo);
            let mut ctrl = frame.ctrl;
            let mut ctrls: Vec<NodeId> = Vec::new();
            for k in lo..=hi {
                let tgt = ts
                    .dest_offset_at((k - lo) as usize)
                    .map(|o| bci as i64 + o as i64);
                if tgt
                    .and_then(|t| self.target_block(t, self.code.len()))
                    .is_none()
                {
                    continue;
                }
                let key = self.gvn.con_i(k);
                let test = self
                    .gvn
                    .transform(Opcode::BoolCmp(CmpCondition::Eq), vec![val, key]);
                let (_ifn, ift, iff) = self.emit_if(ctrl, frame.mem, test);
                ctrls.push(ift);
                ctrl = iff;
                let _ = tgt;
            }
            ctrls.push(ctrl);
            self.edge_ctrls = ctrls;
        } else {
            self.unsupported(bci, "tableswitch decode");
        }
    }

    fn exec_lookupswitch(&mut self, bci: usize, frame: &mut Frame) {
        let val = self.pop(frame);
        if let Some(ls) = bytecode::BytecodeLookupswitch::new(self.code, bci) {
            let n = ls.number_of_pairs().unwrap_or(0);
            let mut ctrl = frame.ctrl;
            let mut ctrls: Vec<NodeId> = Vec::new();
            for i in 0..n {
                if let Some(m) = ls.match_at(i) {
                    let tgt = ls.offset_at(i).map(|o| bci as i64 + o as i64);
                    if tgt
                        .and_then(|t| self.target_block(t, self.code.len()))
                        .is_none()
                    {
                        continue;
                    }
                    let key = self.gvn.con_i(m);
                    let test = self
                        .gvn
                        .transform(Opcode::BoolCmp(CmpCondition::Eq), vec![val, key]);
                    let (_ifn, ift, iff) = self.emit_if(ctrl, frame.mem, test);
                    ctrls.push(ift);
                    ctrl = iff;
                }
            }
            ctrls.push(ctrl);
            self.edge_ctrls = ctrls;
        } else {
            self.unsupported(bci, "lookupswitch decode");
        }
    }

    fn maybe_throw(&mut self, bci: usize, trap_node: NodeId, frame: &Frame) {
        let edges = match self.exc_at.get(&bci) {
            Some(e) => e.clone(),
            None => return,
        };
        for eid in edges {
            let (hb, slot) = {
                let ed = &self.exc_edges[eid];
                (ed.handler_block, ed.slot)
            };
            let ie = self.gvn.alloc(Opcode::IfException, vec![trap_node]);
            let stack = vec![StackVal {
                node: ie,
                cat2: false,
            }];
            let info = EdgeInfo {
                ctrl: ie,
                mem: frame.mem,
                locals: frame.locals.clone(),
                stack,
            };
            self.deliver(hb, slot, info);
        }
    }

    fn deliver_exc_athrow(&mut self, bci: usize, athrow_node: NodeId, obj: NodeId, frame: &Frame) {
        let edges = match self.exc_at.get(&bci) {
            Some(e) => e.clone(),
            None => return,
        };
        for eid in edges {
            let (hb, slot) = {
                let ed = &self.exc_edges[eid];
                (ed.handler_block, ed.slot)
            };
            // Route the edge through a dedicated IfException token so every
            // exception edge has a unique control point (the athrow node
            // itself is the block terminator and may feed several handlers).
            // Its value is never read: the handler sees `obj` directly.
            let ie = self.gvn.alloc(Opcode::IfException, vec![athrow_node]);
            let stack = vec![StackVal {
                node: obj,
                cat2: false,
            }];
            let info = EdgeInfo {
                ctrl: ie,
                mem: frame.mem,
                locals: frame.locals.clone(),
                stack,
            };
            self.deliver(hb, slot, info);
        }
    }

    fn finish(&mut self) {
        let pending = self.gvn.has_pending_inputs();
        if pending > 0 {
            self.result.warnings += pending;
            let zero = self.zero;
            let start_id = self.gvn.start;
            let fill = if start_id != NONE { start_id } else { zero };
            for node in self.gvn.nodes.iter_mut() {
                let is_ctrl_merge = matches!(node.op, Opcode::Region | Opcode::MergeMem);
                for inp in node.inputs.iter_mut() {
                    if *inp == NONE {
                        *inp = if is_ctrl_merge { fill } else { zero };
                    }
                }
            }
        }
    }
}

fn leak_bytes(bytes: &[u8]) -> *const u8 {
    let leaked: &'static mut [u8] = Box::leak(bytes.to_vec().into_boxed_slice());
    leaked.as_ptr()
}

pub fn leak_stub_site(site: &SiteInfo) -> u64 {
    let leaked = Box::leak(Box::new(crate::share::c1::c1_LIR::StubSite {
        class: leak_bytes(site.class.as_bytes()),
        class_len: site.class.len(),
        name: leak_bytes(site.name.as_bytes()),
        name_len: site.name.len(),
        desc: leak_bytes(site.desc.as_bytes()),
        desc_len: site.desc.len(),
    }));
    leaked as *const crate::share::c1::c1_LIR::StubSite as u64
}

pub fn kind_from_desc(desc: &str) -> Option<FieldKind> {
    Some(match desc.as_bytes().first()? {
        b'I' => FieldKind::Int,
        b'J' => FieldKind::Long,
        b'F' => FieldKind::Float,
        b'D' => FieldKind::Double,
        b'B' => FieldKind::Byte,
        b'C' => FieldKind::Char,
        b'S' => FieldKind::Short,
        b'Z' => FieldKind::Bool,
        b'L' | b'[' => FieldKind::Ref,
        _ => return None,
    })
}

pub fn resolve_sites(
    klass: &crate::share::oops::instanceKlass::InstanceKlass<'_>,
    code: &[u8],
) -> HashMap<u16, SiteInfo> {
    let mut sites = HashMap::new();
    for (bci, op, _len) in bytecodes::instructions(code) {
        let cp = match op {
            Code::Getstatic
            | Code::Putstatic
            | Code::Getfield
            | Code::Putfield
            | Code::Invokevirtual
            | Code::Invokespecial
            | Code::Invokestatic
            | Code::Invokeinterface
            | Code::Invokedynamic
            | Code::New
            | Code::Anewarray
            | Code::Checkcast
            | Code::Instanceof
            | Code::Multianewarray => {
                if bci + 2 >= code.len() {
                    continue;
                }
                u16::from_be_bytes([code[bci + 1], code[bci + 2]])
            }
            _ => continue,
        };
        if cp == 0 || sites.contains_key(&cp) {
            continue;
        }
        let is_field = matches!(
            op,
            Code::Getstatic | Code::Putstatic | Code::Getfield | Code::Putfield
        );
        let is_method = matches!(
            op,
            Code::Invokevirtual
                | Code::Invokespecial
                | Code::Invokestatic
                | Code::Invokeinterface
                | Code::Invokedynamic
        );
        if is_field || is_method {
            if let Ok([owner, name, desc]) = klass.memberRef(cp, is_field) {
                sites.insert(
                    cp,
                    SiteInfo {
                        class: String::from_utf8_lossy(owner).into_owned(),
                        name: String::from_utf8_lossy(name).into_owned(),
                        desc: String::from_utf8_lossy(desc).into_owned(),
                    },
                );
            }
        } else if let Ok(class) = klass.className(cp) {
            sites.insert(
                cp,
                SiteInfo {
                    class: String::from_utf8_lossy(class).into_owned(),
                    name: String::new(),
                    desc: String::new(),
                },
            );
        }
    }
    sites
}
