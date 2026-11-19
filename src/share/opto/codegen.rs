/*
 * Port of OpenJDK 25 HotSpot C2 matcher/code generator front-end
 * (share/opto/matcher.cpp, c2_MacroAssembler_x86.cpp) for the executable
 * primitive subset.
 *
 * Pipeline position: Schedule -> RegisterAllocator -> X86Emitter.
 *
 * The matcher selects ideal nodes directly to x86-64 encodings.  Pure nodes
 * are re-materialized in the block that uses them (late scheduling); Phi and
 * other materialized values use the locations assigned by the linear-scan
 * register allocator.  Frame layout follows the same internal SysV-style ABI
 * used by the C1 backend and the interpreter trampoline:
 *   - args: rdi, rsi, rdx, rcx, r8, r9, then stack at [rbp+16+8k]
 *   - integer/ref result: rax ; float/double result: xmm0
 *   - callee-saved GP regs used by the allocator are saved in the prologue.
 */

use super::asm::code_buffer::{CodeBuffer, CodeBufferError, DEFAULT_BUFFER_SIZE};
use super::asm::register::{Condition as XCond, Register, Scale, XMMRegister};
use super::asm::x64::{AluOp, ShiftOp, X64Asm};
use super::gvn::PhaseGVN;
use super::node::{
    ArrayElemKind, CallResult, CmpCondition, NONE, NodeId, Opcode, Type, call_unpack,
};
use super::regalloc::{Allocation, Loc, TEMP_SLOTS};
use super::scheduler::{Schedule, TermKind, phis_of_region, region_pred_index};
use crate::share::c1::c1_LIR::RuntimeStub;
use crate::share::code::pcDesc::PcDesc;
use crate::share::oops::objLayout::ObjLayoutMode;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    I32,
    I64,
    F32,
    F64,
    Ref,
}

impl ParamKind {
    fn is_xmm(&self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
    fn is_wide(&self) -> bool {
        matches!(self, Self::I64 | Self::F64 | Self::Ref)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParamSlot {
    pub local_slot: usize,
    pub kind: ParamKind,
}

#[derive(Clone, Debug)]
pub struct BackendOptions {
    pub stubs: BTreeMap<RuntimeStub, u64>,
    /// Non-unwinding trampoline addresses for the same stubs, used for
    /// calls whose node has an exception-handler edge: the generated
    /// code then runs `ExcCheck` itself and dispatches to the handler
    /// instead of letting the trampoline unwind the frame.
    pub stubs_no_unwind: BTreeMap<RuntimeStub, u64>,
    /// Incoming arguments in ABI order with their JVM local slot.
    pub params: Vec<ParamSlot>,
    /// OSR entry: no ABI parameters; rcx carries the marshal-buffer
    /// pointer which the prologue stores in frame slot 0.
    pub osr: bool,
    pub emit_safepoints: bool,
    pub obj_layout: ObjLayoutMode,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            stubs: BTreeMap::new(),
            stubs_no_unwind: BTreeMap::new(),
            params: Vec::new(),
            osr: false,
            emit_safepoints: true,
            obj_layout: ObjLayoutMode::Compressed,
        }
    }
}

#[derive(Debug)]
pub enum BackendError {
    CodeBuffer(CodeBufferError),
    Unsupported(String),
    MissingRuntimeStub(RuntimeStub),
    InvalidGraph(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodeBuffer(e) => write!(f, "code buffer: {}", e),
            Self::Unsupported(s) => write!(f, "unsupported: {}", s),
            Self::MissingRuntimeStub(s) => write!(f, "missing runtime stub {:?}", s),
            Self::InvalidGraph(s) => write!(f, "invalid graph: {}", s),
        }
    }
}
impl std::error::Error for BackendError {}
impl From<CodeBufferError> for BackendError {
    fn from(e: CodeBufferError) -> Self {
        Self::CodeBuffer(e)
    }
}

#[derive(Clone, Debug)]
pub struct EmitResult {
    pub code: Vec<u8>,
    pub frame_size: usize,
    pub pc_descs: Vec<PcDesc>,
    pub block_offsets: HashMap<usize, usize>,
}

/// One exception-handler edge out of a trapping node, ordered by
/// exception-table priority for the typed-catch dispatch chain.
#[derive(Clone, Copy)]
struct ExcDispatch {
    block: usize,
    ctrl: NodeId,
    /// `SitePtr` node for the catch type; `NONE`/`SitePtr(0)` = catch-all.
    site: NodeId,
    prio: i32,
}

#[derive(Clone, Copy)]
struct BranchPatch {
    disp: usize,
    target: usize,
}

const XMM_SAVE_SLOTS: usize = 7; // XMM8-XMM14
// Windows x64 argument registers used by the interpreter<->nmethod bridge
// (jitruntime::invoke_raw): rcx, rdx, r8, r9; stack args sit above the
// 32-byte shadow space at [rbp+48].
const ARG_REGS: [Register; 4] = [Register::RCX, Register::RDX, Register::R8, Register::R9];

pub struct X86Emitter<'a> {
    buf: &'a CodeBuffer,
    asm: X64Asm<'a>,
    gvn: &'a PhaseGVN,
    schedule: &'a Schedule,
    alloc: &'a Allocation,
    options: &'a BackendOptions,
    order: Vec<usize>,
    block_offsets: HashMap<usize, usize>,
    patches: Vec<BranchPatch>,
    pc_descs: Vec<PcDesc>,
    temp_sp: usize,
    param_kinds: HashMap<usize, ParamKind>,
    /// Trapping node -> ordered exception-handler dispatches
    /// (exception-table priority order) for nodes with handler edges.
    exc_by_trap: HashMap<NodeId, Vec<ExcDispatch>>,
}

impl<'a> X86Emitter<'a> {
    pub fn new(
        buf: &'a CodeBuffer,
        gvn: &'a PhaseGVN,
        schedule: &'a Schedule,
        alloc: &'a Allocation,
        options: &'a BackendOptions,
    ) -> Self {
        let mut order: Vec<usize> = (0..schedule.blocks.len()).collect();
        order.sort_by_key(|&b| schedule.blocks[b].rpo);
        let param_kinds = options
            .params
            .iter()
            .map(|p| (p.local_slot, p.kind))
            .collect();
        let mut exc_by_trap: HashMap<NodeId, Vec<ExcDispatch>> = HashMap::new();
        for block in &schedule.blocks {
            for e in &block.exc_succs {
                let cn = gvn.node(e.ctrl);
                let (trap, site, prio) = if cn.op == Opcode::IfException {
                    let ins = &cn.inputs;
                    let site = ins.get(1).copied().unwrap_or(NONE);
                    let prio = ins
                        .get(2)
                        .and_then(|&p| match gvn.node(p).op {
                            Opcode::ConI(v) => Some(v),
                            _ => None,
                        })
                        .unwrap_or(0);
                    (ins.first().copied().unwrap_or(NONE), site, prio)
                } else {
                    (e.ctrl, NONE, 0)
                };
                if trap != NONE {
                    exc_by_trap.entry(trap).or_default().push(ExcDispatch {
                        block: e.block,
                        ctrl: e.ctrl,
                        site,
                        prio,
                    });
                }
            }
        }
        for v in exc_by_trap.values_mut() {
            v.sort_by_key(|d| d.prio);
        }
        Self {
            buf,
            asm: X64Asm::new(buf),
            gvn,
            schedule,
            alloc,
            options,
            order,
            block_offsets: HashMap::new(),
            patches: Vec::new(),
            pc_descs: Vec::new(),
            temp_sp: 0,
            param_kinds,
            exc_by_trap,
        }
    }

    pub fn emit(&mut self) -> Result<EmitResult, BackendError> {
        self.emit_prologue()?;
        for &b in &self.order.clone() {
            self.block_offsets.insert(b, self.buf.position());
            if self.schedule.blocks[b].loop_header && self.options.emit_safepoints {
                self.emit_safepoint_poll()?;
            }
            for &n in &self.schedule.blocks[b].instrs {
                self.emit_effect(n)?;
                self.emit_exc_dispatch(b, n)?;
            }
            self.emit_terminator(b)?;
        }
        self.patch_branches()?;
        // Unreachable epilogue: guarantees every nmethod contains the
        // frame-restore sequence `jit_unwind` scans for, including
        // methods whose only exits are athrow/unwind.
        self.emit_epilogue()?;
        self.buf.write(&[0x0f, 0x0b])?; // ud2 trap for unreachable tail
        Ok(EmitResult {
            code: self.buf.code_slice().to_vec(),
            frame_size: self.alloc.frame_size,
            pc_descs: self.pc_descs.clone(),
            block_offsets: self.block_offsets.clone(),
        })
    }

    fn pos(&self) -> usize {
        self.buf.position()
    }

    fn saved_count(&self) -> usize {
        self.alloc.saved_regs.len()
    }

    fn slot_disp(&self, slot: usize) -> i32 {
        -(8i32 + 8 * self.saved_count() as i32 + 8 * slot as i32)
    }

    fn spill_slot_disp(&self, ord: usize) -> i32 {
        self.slot_disp(self.alloc.param_slots + ord)
    }

    fn temp_disp(&self, idx: usize) -> i32 {
        self.slot_disp(self.alloc.param_slots + self.alloc.spill_count + XMM_SAVE_SLOTS + idx)
    }

    fn alloc_temp(&mut self) -> Result<usize, BackendError> {
        if self.temp_sp >= TEMP_SLOTS {
            return Err(BackendError::Unsupported(
                "operand temp stack overflow".into(),
            ));
        }
        let i = self.temp_sp;
        self.temp_sp += 1;
        Ok(i)
    }

    fn free_temp(&mut self) {
        self.temp_sp = self.temp_sp.saturating_sub(1);
    }

    // ---- prologue / epilogue ---------------------------------------------

    fn emit_prologue(&mut self) -> Result<(), BackendError> {
        self.buf.write_byte(0x55)?; // push rbp
        self.asm.emit_rex(true, false, false, false);
        self.buf.write(&[0x89, 0xe5])?; // mov rbp,rsp
        for &r in &self.alloc.saved_regs {
            self.asm.emit_push(r)?;
        }
        if self.alloc.frame_size > 0 {
            self.asm.emit_rex(true, false, false, false);
            self.buf.write(&[0x81, 0xec])?;
            self.buf.write_i32(self.alloc.frame_size as i32)?; // sub rsp,imm32
        }
        self.emit_param_moves()?;
        if self.options.osr {
            // rcx holds the marshal-buffer pointer on OSR entry.
            self.asm.mov_mem_reg(
                Some(Register::FP),
                None,
                Scale::S1,
                self.slot_disp(0),
                Register::RCX,
                true,
            )?;
        }
        Ok(())
    }

    fn emit_param_moves(&mut self) -> Result<(), BackendError> {
        for (i, p) in self.options.params.iter().enumerate() {
            let disp = self.slot_disp(p.local_slot);
            if p.kind.is_xmm() {
                // Win64 unified slots: float/double args arrive in xmm[i].
                if i >= 4 {
                    return Err(BackendError::Unsupported(
                        "floating-point stack parameter is outside the executable subset".into(),
                    ));
                }
                let xmm = XMMRegister::from_encoding(i as u8)
                    .ok_or_else(|| BackendError::Unsupported("bad xmm param".into()))?;
                self.asm
                    .sse_store_mem(xmm, Register::FP, disp, p.kind == ParamKind::F64)?;
                continue;
            }
            if i < ARG_REGS.len() {
                self.asm.mov_mem_reg(
                    Some(Register::FP),
                    None,
                    Scale::S1,
                    disp,
                    ARG_REGS[i],
                    true,
                )?;
            } else {
                let stack_off = 48 + ((i - ARG_REGS.len()) * 8) as i32;
                self.asm.mov_reg_mem(
                    Register::RAX,
                    Some(Register::FP),
                    None,
                    Scale::S1,
                    stack_off,
                    true,
                )?;
                self.asm.mov_mem_reg(
                    Some(Register::FP),
                    None,
                    Scale::S1,
                    disp,
                    Register::RAX,
                    true,
                )?;
            }
        }
        Ok(())
    }

    fn emit_epilogue(&mut self) -> Result<(), BackendError> {
        let k = self.saved_count();
        // lea rsp,[rbp-8*k] discards the frame body and points at the
        // callee-saved register area (empty when k == 0).
        self.asm.emit_rex(true, false, false, false);
        self.buf.write(&[0x8d, 0x65])?;
        self.buf.write_byte((-(8i32 * k as i32)) as i8 as u8)?;
        for &r in self.alloc.saved_regs.iter().rev() {
            self.asm.emit_pop(r)?;
        }
        self.buf.write_byte(0x5d)?; // pop rbp
        self.buf.write_byte(0xc3)?; // ret
        Ok(())
    }

    // ---- blocks / terminators ---------------------------------------------

    fn emit_terminator(&mut self, b: usize) -> Result<(), BackendError> {
        let block = &self.schedule.blocks[b];
        match &block.term {
            Some(t) => match t.kind {
                TermKind::If => self.emit_if(t.node, b),
                TermKind::Return => {
                    let inputs = self.gvn.node(t.node).inputs.clone();
                    if inputs.len() > 2 && inputs[2] != NONE {
                        self.emit_value(inputs[2])?;
                    }
                    self.emit_epilogue()
                }
                TermKind::Athrow => {
                    let inputs = self.gvn.node(t.node).inputs.clone();
                    let obj = inputs.get(2).copied().unwrap_or(NONE);
                    self.emit_gp_to(obj, Register::RCX)?;
                    let addr = self.stub_addr_for(t.node, RuntimeStub::Throw)?;
                    self.asm.emit_abs_call(addr)?;
                    self.emit_exc_dispatch(b, t.node)?;
                    self.buf.write(&[0x0f, 0x0b])?;
                    Ok(())
                }
                TermKind::Halt => {
                    self.buf.write(&[0x0f, 0x0b])?;
                    Ok(())
                }
                TermKind::Jump => self.emit_jump_edges(b),
            },
            None => self.emit_jump_edges(b),
        }
    }

    fn emit_jump_edges(&mut self, b: usize) -> Result<(), BackendError> {
        let succs = self.schedule.blocks[b].succs.clone();
        if succs.is_empty() {
            self.buf.write(&[0x0f, 0x0b])?;
            return Ok(());
        }
        for e in &succs {
            self.emit_phi_moves(b, e.block, e.ctrl)?;
        }
        self.emit_jmp_block(succs[0].block)
    }

    fn emit_if(&mut self, if_node: NodeId, b: usize) -> Result<(), BackendError> {
        let inputs = self.gvn.node(if_node).inputs.clone();
        let test = inputs.get(1).copied().unwrap_or(NONE);
        let mut true_t = NONE;
        let mut false_t = NONE;
        for e in &self.schedule.blocks[b].succs {
            let entry = self.schedule.blocks[e.block].entry;
            match self.gvn.node(entry).op {
                Opcode::IfTrue => true_t = e.block,
                Opcode::IfFalse => false_t = e.block,
                _ => {}
            }
        }
        if true_t == NONE || false_t == NONE {
            return Err(BackendError::InvalidGraph(
                "If without IfTrue/IfFalse".into(),
            ));
        }
        self.emit_cond_branch(test, true_t, false_t)
    }

    fn emit_cond_branch(
        &mut self,
        test: NodeId,
        true_t: usize,
        false_t: usize,
    ) -> Result<(), BackendError> {
        if test == NONE {
            return Err(BackendError::InvalidGraph("missing branch test".into()));
        }
        if let Some(v) = self.gvn.node(test).as_bool() {
            return if v {
                self.emit_jmp_block(true_t)
                    .and_then(|_| self.emit_jmp_block(false_t))
            } else {
                self.emit_jmp_block(false_t)
                    .and_then(|_| self.emit_jmp_block(true_t))
            };
        }
        if let Opcode::BoolCmp(cond) = self.gvn.node(test).op {
            let inputs = self.gvn.node(test).inputs.clone();
            let (a, c) = (inputs[0], inputs[1]);
            self.emit_bool_cmp_operands(a, c)?;
            let cc = self.xcond_for(cond);
            self.emit_jcc_block(cc, true_t)?;
            return self.emit_jmp_block(false_t);
        }
        self.emit_value(test)?;
        self.asm.emit_rex(false, false, false, false);
        self.buf.write(&[0x85, 0xc0])?; // test eax,eax
        self.emit_jcc_block(XCond::E, false_t)?;
        self.emit_jmp_block(true_t)
    }

    fn emit_bool_cmp_operands(&mut self, a: NodeId, b: NodeId) -> Result<(), BackendError> {
        let wide = self.is_wide_gp(a) || self.is_wide_gp(b);
        let t = self.alloc_temp()?;
        self.emit_gp(a, Register::RAX)?;
        self.store_gp_to_temp(t, Register::RAX, wide)?;
        self.emit_gp_to(b, Register::RCX)?;
        self.load_gp_from_temp(t, Register::RAX, wide)?;
        self.free_temp();
        self.asm
            .cmp_rr(Register::RAX, Register::RCX, wide)
            .map_err(BackendError::from)
    }

    fn xcond_for(&self, c: CmpCondition) -> XCond {
        match c {
            CmpCondition::Eq => XCond::E,
            CmpCondition::Ne => XCond::NE,
            CmpCondition::Lt => XCond::L,
            CmpCondition::Le => XCond::LE,
            CmpCondition::Gt => XCond::G,
            CmpCondition::Ge => XCond::GE,
        }
    }

    // ---- phi copies --------------------------------------------------------

    fn emit_phi_moves(
        &mut self,
        _from: usize,
        to: usize,
        edge_ctrl: NodeId,
    ) -> Result<(), BackendError> {
        let succ_entry = self.schedule.blocks[to].entry;
        if self.gvn.node(succ_entry).op != Opcode::Region {
            return Ok(());
        }
        let Some(pred_idx) = region_pred_index(self.gvn, succ_entry, edge_ctrl) else {
            return Ok(());
        };
        let phis = phis_of_region(self.gvn, succ_entry);
        if phis.is_empty() {
            return Ok(());
        }
        // Two-phase parallel copy through operand temp slots: first read every
        // source, then write every destination.  This resolves register/slot
        // cycles without a permutation solver.  The staging slots are reserved
        // in `temp_sp` so nested operand materialization (which allocates its
        // own temps) cannot clobber values already staged.
        let mut moves: Vec<(NodeId, NodeId)> = Vec::new();
        for phi in phis {
            let inputs = self.gvn.node(phi).inputs.clone();
            let Some(&src) = inputs.get(pred_idx) else {
                continue;
            };
            if src == NONE {
                continue;
            }
            moves.push((src, phi));
        }
        let base = self.temp_sp;
        if base + moves.len() > TEMP_SLOTS {
            return Err(BackendError::Unsupported(format!(
                "{} phi copies exceed temp slots",
                moves.len()
            )));
        }
        self.temp_sp = base + moves.len();
        for (i, &(src, dst)) in moves.iter().enumerate() {
            self.load_move_src(src, dst, base + i)?;
        }
        for (i, &(_, dst)) in moves.iter().enumerate() {
            self.store_move_dst(dst, base + i)?;
        }
        self.temp_sp = base;
        Ok(())
    }

    fn move_kind_xmm(&self, dst: NodeId) -> bool {
        self.is_xmm(dst)
    }

    fn load_move_src(&mut self, src: NodeId, dst: NodeId, slot: usize) -> Result<(), BackendError> {
        let disp = self.temp_disp(slot);
        if self.move_kind_xmm(dst) {
            self.emit_xmm_to(src, XMMRegister::XMM15)?;
            self.asm
                .sse_store_mem(XMMRegister::XMM15, Register::FP, disp, true)
                .map_err(BackendError::from)
        } else {
            self.emit_gp(src, Register::R11)?;
            self.asm
                .mov_mem_reg(
                    Some(Register::FP),
                    None,
                    Scale::S1,
                    disp,
                    Register::R11,
                    true,
                )
                .map_err(BackendError::from)
        }
    }

    fn store_move_dst(&mut self, dst: NodeId, slot: usize) -> Result<(), BackendError> {
        let disp = self.temp_disp(slot);
        if self.move_kind_xmm(dst) {
            self.asm
                .sse_load_mem(XMMRegister::XMM15, Register::FP, disp, true)?;
            self.store_xmm_loc(dst, XMMRegister::XMM15)
        } else {
            self.asm.mov_reg_mem(
                Register::R11,
                Some(Register::FP),
                None,
                Scale::S1,
                disp,
                true,
            )?;
            self.store_gp_loc(dst, Register::R11, self.is_wide_gp(dst))
        }
    }

    fn is_pure(&self, node: NodeId) -> bool {
        if node == NONE {
            return false;
        }
        matches!(
            self.gvn.node(node).op,
            Opcode::AddI
                | Opcode::SubI
                | Opcode::MulI
                | Opcode::AndI
                | Opcode::OrI
                | Opcode::XorI
                | Opcode::LShiftI
                | Opcode::RShiftI
                | Opcode::URShiftI
                | Opcode::NegI
                | Opcode::AddL
                | Opcode::SubL
                | Opcode::MulL
                | Opcode::AndL
                | Opcode::OrL
                | Opcode::XorL
                | Opcode::LShiftL
                | Opcode::RShiftL
                | Opcode::URShiftL
                | Opcode::NegL
                | Opcode::AddF
                | Opcode::SubF
                | Opcode::MulF
                | Opcode::DivF
                | Opcode::RemF
                | Opcode::NegF
                | Opcode::AddD
                | Opcode::SubD
                | Opcode::MulD
                | Opcode::DivD
                | Opcode::RemD
                | Opcode::NegD
                | Opcode::CmpI
                | Opcode::CmpL
                | Opcode::CmpF(_)
                | Opcode::CmpD(_)
                | Opcode::BoolCmp(_)
                | Opcode::I2L
                | Opcode::I2F
                | Opcode::I2D
                | Opcode::L2I
                | Opcode::L2F
                | Opcode::L2D
                | Opcode::F2I
                | Opcode::F2L
                | Opcode::F2D
                | Opcode::D2I
                | Opcode::D2L
                | Opcode::D2F
                | Opcode::I2B
                | Opcode::I2C
                | Opcode::I2S
                | Opcode::ConI(_)
                | Opcode::ConL(_)
                | Opcode::ConF(_)
                | Opcode::ConD(_)
                | Opcode::ConP(_)
                | Opcode::Bool(_)
                | Opcode::Parm(_)
        )
    }

    // ---- scheduled effects -------------------------------------------------

    fn emit_effect(&mut self, node: NodeId) -> Result<(), BackendError> {
        let op = self.gvn.node(node).op.clone();
        match op {
            Opcode::LoadI
            | Opcode::LoadL
            | Opcode::LoadF
            | Opcode::LoadD
            | Opcode::LoadP
            | Opcode::LoadB
            | Opcode::LoadC
            | Opcode::LoadS => self.emit_load(node),
            Opcode::StoreI
            | Opcode::StoreL
            | Opcode::StoreF
            | Opcode::StoreD
            | Opcode::StoreP
            | Opcode::StoreB
            | Opcode::StoreC
            | Opcode::StoreS => self.emit_store(node),
            Opcode::ArrayLength => self.emit_array_length(node),
            Opcode::New(cp) => self.emit_new_instance(node, cp as u32),
            Opcode::NewArray(kind) => self.emit_new_array(node, kind as u32),
            Opcode::NewArrayRef(cp) => self.emit_anew_array(node, cp as u32),
            Opcode::MultianewArray => {
                self.emit_stub_call(node, RuntimeStub::MultianewArray)?;
                self.store_gp_loc(node, Register::RAX, true)
            }
            Opcode::Call(payload) => self.emit_call(node, payload),
            Opcode::CheckCast(cp) => self.emit_cast(node, cp as u32, RuntimeStub::CheckCast, true),
            Opcode::InstanceOf(cp) => {
                self.emit_cast(node, cp as u32, RuntimeStub::InstanceOf, false)
            }
            Opcode::MonitorEnter => self.emit_monitor(node, RuntimeStub::MonitorEnter),
            Opcode::MonitorExit => self.emit_monitor(node, RuntimeStub::MonitorExit),
            Opcode::DivI | Opcode::RemI | Opcode::DivL | Opcode::RemL => {
                // [mem, ctrl, lhs, rhs]; result in rax like emit_pure_gp.
                let inputs = self.gvn.node(node).inputs.clone();
                let wide = self.is_wide_gp(node);
                let t = self.alloc_temp()?;
                self.emit_value(inputs[2])?;
                self.store_gp_to_temp(t, Register::RAX, wide)?;
                self.emit_gp_to(inputs[3], Register::RCX)?;
                self.load_gp_from_temp(t, Register::RAX, wide)?;
                self.free_temp();
                self.emit_div_rem(node, wide)?;
                self.store_gp_loc(node, Register::RAX, wide)
            }
            _ => Err(BackendError::Unsupported(format!(
                "scheduled effect {:?} is outside the executable subset",
                op
            ))),
        }
    }

    fn last_input_is_site(&self, inputs: &[NodeId]) -> bool {
        inputs
            .last()
            .map(|&n| matches!(self.gvn.node(n).op, Opcode::SitePtr(_)))
            .unwrap_or(false)
    }

    fn emit_field_load(&mut self, node: NodeId, inputs: &[NodeId]) -> Result<(), BackendError> {
        let site = *inputs
            .last()
            .ok_or_else(|| BackendError::InvalidGraph("field load missing site".into()))?;
        if inputs.len() == 3 {
            self.emit_gp_to(site, Register::RCX)?;
            self.emit_stub_call(node, RuntimeStub::GetStatic)?;
        } else {
            self.emit_gp_to(inputs[2], Register::RCX)?;
            self.emit_gp_to(site, Register::RDX)?;
            self.emit_stub_call(node, RuntimeStub::GetField)?;
        }
        match self.gvn.node(node).op {
            Opcode::LoadF | Opcode::LoadD => self.store_xmm_loc(node, XMMRegister::XMM0),
            Opcode::LoadL | Opcode::LoadP => self.store_gp_loc(node, Register::RAX, true),
            _ => self.store_gp_loc(node, Register::RAX, false),
        }
    }

    fn stage_field_value(&mut self, value: NodeId) -> Result<usize, BackendError> {
        let t = self.alloc_temp()?;
        if self.is_xmm(value) {
            self.emit_xmm_to(value, XMMRegister::XMM0)?;
            self.asm.movq_xmm_to_gp(Register::RAX, XMMRegister::XMM0)?;
            self.store_gp_to_temp(t, Register::RAX, true)?;
        } else {
            self.emit_gp_to(value, Register::RAX)?;
            self.store_gp_to_temp(t, Register::RAX, self.is_wide_gp(value))?;
        }
        Ok(t)
    }

    fn emit_field_store(
        &mut self,
        node: NodeId,
        inputs: &[NodeId],
    ) -> Result<(), BackendError> {
        let is_static = inputs.len() == 4;
        let site = *inputs
            .last()
            .ok_or_else(|| BackendError::InvalidGraph("field store missing site".into()))?;
        let (obj, value) = if is_static {
            (NONE, inputs[2])
        } else {
            (inputs[2], inputs[4])
        };
        let tv = self.stage_field_value(value)?;
        if is_static {
            self.emit_gp_to(site, Register::RCX)?;
            self.load_gp_from_temp(tv, Register::RDX, true)?;
        } else {
            self.emit_gp_to(obj, Register::RCX)?;
            self.emit_gp_to(site, Register::RDX)?;
            self.load_gp_from_temp(tv, Register::R8, true)?;
        }
        self.free_temp();
        self.emit_stub_call(
            node,
            if is_static {
                RuntimeStub::PutStatic
            } else {
                RuntimeStub::PutField
            },
        )
    }

    fn emit_new_instance(&mut self, node: NodeId, _cp: u32) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let site = inputs.get(2).copied().unwrap_or(NONE);
        self.emit_gp_to(site, Register::RCX)?;
        self.emit_stub_call(node, RuntimeStub::NewInstance)?;
        self.store_gp_loc(node, Register::RAX, true)
    }

    fn emit_new_array(&mut self, node: NodeId, kind: u32) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let size = inputs.get(2).copied().unwrap_or(NONE);
        self.emit_gp_to(size, Register::RCX)?;
        self.asm.mov_imm32(Register::RDX, kind as i32)?;
        self.emit_stub_call(node, RuntimeStub::NewArray)?;
        self.store_gp_loc(node, Register::RAX, true)
    }

    fn emit_anew_array(&mut self, node: NodeId, _cp: u32) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let site = inputs.get(3).copied().unwrap_or(NONE);
        let size = inputs.get(2).copied().unwrap_or(NONE);
        self.emit_gp_to(site, Register::RCX)?;
        self.emit_gp_to(size, Register::RDX)?;
        self.emit_stub_call(node, RuntimeStub::AnewArray)?;
        self.store_gp_loc(node, Register::RAX, true)
    }

    fn emit_cast(
        &mut self,
        node: NodeId,
        _cp: u32,
        stub: RuntimeStub,
        wide: bool,
    ) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let obj = inputs.get(2).copied().unwrap_or(NONE);
        let site = inputs.get(3).copied().unwrap_or(NONE);
        self.emit_gp_to(obj, Register::RCX)?;
        self.emit_gp_to(site, Register::RDX)?;
        self.emit_stub_call(node, stub)?;
        if wide {
            self.store_gp_loc(node, Register::RAX, true)
        } else {
            self.store_gp_loc(node, Register::RAX, false)
        }
    }

    fn emit_monitor(&mut self, node: NodeId, stub: RuntimeStub) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let obj = inputs.get(2).copied().unwrap_or(NONE);
        self.emit_gp_to(obj, Register::RCX)?;
        self.emit_stub_call(node, stub)
    }

    fn emit_call(&mut self, node: NodeId, payload: u64) -> Result<(), BackendError> {
        let (cp, invoke_op, result) = call_unpack(payload);
        // invokedynamic (0xba) resolves its bootstrap through the
        // constant-pool index, which travels in the opcode slot; the
        // InvokeDynamic stub reads it from there.
        let is_indy = invoke_op == 0xba;
        let inputs = self.gvn.node(node).inputs.clone();
        let site = inputs.last().copied().unwrap_or(NONE);
        let end = inputs.len().saturating_sub(1).max(2);
        let words = &inputs[2..end];
        let n = words.len();
        if n > TEMP_SLOTS {
            return Err(BackendError::Unsupported(format!(
                "call with {} arguments exceeds temp slots",
                n
            )));
        }
        let base = self.temp_sp;
        self.temp_sp += n;
        for (i, &w) in words.iter().enumerate() {
            let t = base + (n - 1 - i);
            if self.is_xmm(w) {
                self.emit_xmm_to(w, XMMRegister::XMM0)?;
                self.asm.movq_xmm_to_gp(Register::RAX, XMMRegister::XMM0)?;
                self.store_gp_to_temp(t, Register::RAX, true)?;
            } else {
                self.emit_gp_to(w, Register::RAX)?;
                self.store_gp_to_temp(t, Register::RAX, self.is_wide_gp(w))?;
            }
        }
        self.emit_gp_to(site, Register::RCX)?;
        if n == 0 {
            self.asm.mov_imm32(Register::RDX, 0)?;
        } else {
            let disp = self.temp_disp(base + n - 1);
            self.asm.emit_rex(true, false, false, false);
            self.buf.write(&[0x8d, 0x95])?;
            self.buf.write_i32(disp)?;
        }
        self.asm.mov_imm32(Register::R8, n as i32)?;
        self.asm
            .mov_imm32(Register::R9, if is_indy { cp as i32 } else { invoke_op as i32 })?;
        self.emit_stub_call(
            node,
            if is_indy {
                RuntimeStub::InvokeDynamic
            } else {
                RuntimeStub::UnresolvedCall
            },
        )?;
        self.temp_sp = base;
        match result {
            CallResult::Void => Ok(()),
            CallResult::I32 => self.store_gp_loc(node, Register::RAX, false),
            CallResult::I64 | CallResult::Ref => self.store_gp_loc(node, Register::RAX, true),
            CallResult::F32 | CallResult::F64 => self.store_xmm_loc(node, XMMRegister::XMM0),
        }
    }

    fn emit_runtime_call_addr(&mut self, stub: RuntimeStub) -> Result<(), BackendError> {
        let addr = self
            .options
            .stubs
            .get(&stub)
            .copied()
            .ok_or(BackendError::MissingRuntimeStub(stub))?;
        self.asm.emit_abs_call(addr).map_err(BackendError::from)
    }

    /// Stub address for a call emitted for `node`: nodes with an
    /// exception-handler edge use the non-unwinding trampoline so the
    /// pending outcome can be dispatched inline; everything else keeps
    /// the unwinding trampoline.
    fn stub_addr_for(&self, node: NodeId, stub: RuntimeStub) -> Result<u64, BackendError> {
        if self.exc_by_trap.contains_key(&node)
            && let Some(&addr) = self.options.stubs_no_unwind.get(&stub)
        {
            return Ok(addr);
        }
        self.options
            .stubs
            .get(&stub)
            .copied()
            .ok_or(BackendError::MissingRuntimeStub(stub))
    }

    fn emit_stub_call(&mut self, node: NodeId, stub: RuntimeStub) -> Result<(), BackendError> {
        let addr = self.stub_addr_for(node, stub)?;
        self.asm.emit_abs_call(addr).map_err(BackendError::from)
    }

    /// Pending-outcome dispatch after a non-unwinding stub call.
    /// `ExcCheck` returns 0 when nothing is pending and `tagged + 1`
    /// for a thrown object (a pending deopt re-arms itself and the
    /// trampoline unwinds before this point).  On a throw the tagged
    /// reference is kept in a temp slot and the covering handlers are
    /// tried in exception-table order: catch-all entries dispatch
    /// unconditionally, typed entries first run `InstanceOf` against
    /// the catch class.  If nothing matches, `RethrowExc` re-arms the
    /// pending throw and unwinds to the caller.
    fn emit_exc_dispatch(&mut self, b: usize, node: NodeId) -> Result<(), BackendError> {
        let Some(disp) = self.exc_by_trap.get(&node) else {
            return Ok(());
        };
        let disp = disp.clone();
        self.emit_runtime_call_addr(RuntimeStub::ExcCheck)?;
        self.asm.emit_rex(true, false, false, false);
        self.buf.write(&[0x85, 0xc0])?; // test rax,rax
        let jz = self.emit_jmp_local_cc(XCond::E)?;
        self.buf.write(&[0x48, 0xff, 0xc8])?; // dec rax -> tagged ref
        // Preserve the tagged reference across InstanceOf calls.
        let t = self.alloc_temp()?;
        self.store_gp_to_temp(t, Register::RAX, true)?;
        let instance_of = self
            .options
            .stubs
            .get(&RuntimeStub::InstanceOf)
            .copied()
            .ok_or(BackendError::MissingRuntimeStub(RuntimeStub::InstanceOf))?;
        let rethrow = self
            .options
            .stubs
            .get(&RuntimeStub::RethrowExc)
            .copied()
            .ok_or(BackendError::MissingRuntimeStub(RuntimeStub::RethrowExc))?;
        let mut fell_through = true;
        for d in &disp {
            let site_ptr = match self.gvn.node(d.site).op {
                Opcode::SitePtr(p) if p != 0 => p,
                _ => 0,
            };
            if site_ptr == 0 {
                // catch-all: unconditional dispatch, chain terminates.
                self.load_gp_from_temp(t, Register::RAX, true)?;
                self.store_gp_loc(d.ctrl, Register::RAX, true)?;
                self.emit_phi_moves(b, d.block, d.ctrl)?;
                self.emit_jmp_block(d.block)?;
                fell_through = false;
                break;
            }
            // typed: InstanceOf(exc, catch_class) -> 0/1
            self.load_gp_from_temp(t, Register::RCX, true)?;
            self.asm.mov_imm(Register::RDX, site_ptr as i64)?;
            self.asm.emit_abs_call(instance_of)?;
            self.asm.emit_rex(true, false, false, false);
            self.buf.write(&[0x85, 0xc0])?; // test rax,rax
            let jno = self.emit_jmp_local_cc(XCond::E)?;
            self.load_gp_from_temp(t, Register::RAX, true)?;
            self.store_gp_loc(d.ctrl, Register::RAX, true)?;
            self.emit_phi_moves(b, d.block, d.ctrl)?;
            self.emit_jmp_block(d.block)?;
            let next = self.pos();
            self.patch_local(jno, next)?;
        }
        if fell_through {
            // No handler matched: re-arm pending and unwind.
            self.load_gp_from_temp(t, Register::RCX, true)?;
            self.asm.emit_abs_call(rethrow)?;
            // The trampoline always unwinds on pending; ud2 if not.
            self.buf.write(&[0x0f, 0x0b])?;
        }
        self.free_temp();
        let cont = self.pos();
        self.patch_local(jz, cont)?;
        Ok(())
    }

    fn array_elem(&self, inputs: &[NodeId]) -> Result<ArrayElemKind, BackendError> {
        let ek = *inputs.last().ok_or_else(|| {
            BackendError::InvalidGraph("array access missing element kind".into())
        })?;
        if let Opcode::ConI(code) = self.gvn.node(ek).op {
            if let Some(kind) = ArrayElemKind::from_code(code) {
                return Ok(kind);
            }
        }
        Err(BackendError::InvalidGraph(format!(
            "node {} has no array element kind",
            ek
        )))
    }

    fn stage_gp_to_temp(&mut self, node: NodeId, wide: bool) -> Result<usize, BackendError> {
        let t = self.alloc_temp()?;
        self.emit_gp_to(node, Register::RAX)?;
        self.store_gp_to_temp(t, Register::RAX, wide)?;
        Ok(t)
    }

    fn stage_value_to_temp(
        &mut self,
        value: NodeId,
        ek: ArrayElemKind,
    ) -> Result<usize, BackendError> {
        let t = self.alloc_temp()?;
        if ek.is_xmm() {
            self.emit_xmm_to(value, XMMRegister::XMM0)?;
            self.asm.movq_xmm_to_gp(Register::RAX, XMMRegister::XMM0)?;
            self.store_gp_to_temp(t, Register::RAX, true)?;
        } else {
            self.emit_gp_to(value, Register::RAX)?;
            self.store_gp_to_temp(t, Register::RAX, self.is_wide_gp(value))?;
        }
        Ok(t)
    }

    fn emit_array_load(&mut self, node: NodeId, inputs: &[NodeId]) -> Result<(), BackendError> {
        let (arr, idx) = (inputs[2], inputs[3]);
        let ek = self.array_elem(inputs)?;
        let addr = self.stub_addr_for(node, RuntimeStub::ArrayLoad)?;
        let ti = self.stage_gp_to_temp(idx, false)?;
        self.emit_gp_to(arr, Register::RCX)?;
        self.load_gp_from_temp(ti, Register::RDX, false)?;
        self.free_temp();
        self.asm.mov_imm32(Register::R8, ek.code())?;
        self.asm.emit_abs_call(addr)?;
        match ek {
            ArrayElemKind::Float => {
                self.asm
                    .movd_xmm_from_gp(XMMRegister::XMM0, Register::RAX)?;
                self.store_xmm_loc(node, XMMRegister::XMM0)
            }
            ArrayElemKind::Double => {
                self.asm
                    .movq_xmm_from_gp(XMMRegister::XMM0, Register::RAX)?;
                self.store_xmm_loc(node, XMMRegister::XMM0)
            }
            ArrayElemKind::Long | ArrayElemKind::Ref => {
                self.store_gp_loc(node, Register::RAX, true)
            }
            _ => self.store_gp_loc(node, Register::RAX, false),
        }
    }

    fn emit_array_store(&mut self, node: NodeId, inputs: &[NodeId]) -> Result<(), BackendError> {
        let (arr, idx, value) = (inputs[2], inputs[3], inputs[4]);
        let ek = self.array_elem(inputs)?;
        let addr = self.stub_addr_for(node, RuntimeStub::ArrayStore)?;
        let ti = self.stage_gp_to_temp(idx, false)?;
        let tv = self.stage_value_to_temp(value, ek)?;
        self.emit_gp_to(arr, Register::RCX)?;
        self.load_gp_from_temp(ti, Register::RDX, false)?;
        self.load_gp_from_temp(tv, Register::R8, true)?;
        self.free_temp();
        self.free_temp();
        self.asm.mov_imm32(Register::R9, ek.code())?;
        self.asm.emit_abs_call(addr)?;
        Ok(())
    }

    fn emit_load(&mut self, node: NodeId) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        if self.last_input_is_site(&inputs) {
            return self.emit_field_load(node, &inputs);
        }
        if inputs.len() >= 5 {
            return self.emit_array_load(node, &inputs);
        }
        Err(BackendError::Unsupported(format!(
            "load node {} has no resolved site or element kind",
            node
        )))
    }

    fn emit_store(&mut self, node: NodeId) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        if self.last_input_is_site(&inputs) {
            return self.emit_field_store(node, &inputs);
        }
        if inputs.len() >= 6 {
            return self.emit_array_store(node, &inputs);
        }
        Err(BackendError::Unsupported(format!(
            "store node {} has no resolved site or element kind",
            node
        )))
    }

    fn emit_array_length(&mut self, node: NodeId) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let arr = inputs.get(2).copied().unwrap_or(NONE);
        let addr = self.stub_addr_for(node, RuntimeStub::ArrayLength)?;
        self.emit_gp_to(arr, Register::RCX)?;
        self.asm.emit_abs_call(addr)?;
        self.store_gp_loc(node, Register::RAX, false)
    }

    // ---- value selection ---------------------------------------------------

    fn is_xmm(&self, node: NodeId) -> bool {
        self.is_xmm_deep(node, 0)
    }

    fn is_xmm_deep(&self, node: NodeId, depth: u8) -> bool {
        if node == NONE {
            return false;
        }
        let n = self.gvn.node(node);
        if matches!(n.op, Opcode::Phi) {
            // Phi nodes carry no static type (`compute_type` yields
            // `Type::None`); their kind is that of their value inputs —
            // every input except the trailing Region control.  Follow
            // input chains with a depth cap so phi cycles terminate.
            if depth >= 32 {
                return false;
            }
            let values = &n.inputs[..n.inputs.len().saturating_sub(1)];
            return values
                .iter()
                .any(|&i| i != NONE && self.is_xmm_deep(i, depth + 1));
        }
        if let Opcode::Parm(slot) = n.op {
            return self
                .param_kinds
                .get(&(slot as usize))
                .map(|k| k.is_xmm())
                .unwrap_or(false);
        }
        matches!(n.ty, Type::Float | Type::Double)
            || matches!(
                n.op,
                Opcode::ConF(_)
                    | Opcode::ConD(_)
                    | Opcode::LoadF
                    | Opcode::LoadD
                    | Opcode::AddF
                    | Opcode::SubF
                    | Opcode::MulF
                    | Opcode::DivF
                    | Opcode::NegF
                    | Opcode::AddD
                    | Opcode::SubD
                    | Opcode::MulD
                    | Opcode::DivD
                    | Opcode::NegD
                    | Opcode::I2F
                    | Opcode::I2D
                    | Opcode::L2F
                    | Opcode::L2D
                    | Opcode::F2D
                    | Opcode::D2F
            )
    }

    fn is_wide_gp(&self, node: NodeId) -> bool {
        self.is_wide_gp_deep(node, 0)
    }

    fn is_wide_gp_deep(&self, node: NodeId, depth: u8) -> bool {
        if node == NONE {
            return false;
        }
        let n = self.gvn.node(node);
        if matches!(n.op, Opcode::Phi) {
            if depth >= 32 {
                return false;
            }
            let values = &n.inputs[..n.inputs.len().saturating_sub(1)];
            return values
                .iter()
                .any(|&i| i != NONE && self.is_wide_gp_deep(i, depth + 1));
        }
        if let Opcode::Parm(slot) = n.op {
            return self
                .param_kinds
                .get(&(slot as usize))
                .map(|k| k.is_wide())
                .unwrap_or(false);
        }
        // IfException carries the tagged exception reference across the
        // handler edge; it is always a full 64-bit word.
        matches!(n.ty, Type::Long | Type::Ref)
            || matches!(n.op, Opcode::ConL(_) | Opcode::ConP(_) | Opcode::IfException)
    }

    fn emit_value(&mut self, node: NodeId) -> Result<(), BackendError> {
        if self.is_xmm(node) {
            self.emit_xmm_to(node, XMMRegister::XMM0)
        } else {
            self.emit_gp(node, Register::RAX)
        }
    }

    fn emit_gp(&mut self, node: NodeId, dst: Register) -> Result<(), BackendError> {
        if node == NONE {
            self.asm.mov_imm32(dst, 0)?;
            return Ok(());
        }
        let n = self.gvn.node(node);
        match n.op {
            Opcode::ConI(v) => {
                return self
                    .asm
                    .mov_imm32(dst, v as u32 as i32)
                    .map_err(BackendError::from);
            }
            Opcode::ConL(v) => return self.asm.mov_imm(dst, v).map_err(BackendError::from),
            Opcode::ConP(v) => return self.asm.mov_imm(dst, v as i64).map_err(BackendError::from),
            Opcode::SitePtr(v) => {
                return self.asm.mov_imm(dst, v as i64).map_err(BackendError::from);
            }
            Opcode::Bool(v) => {
                return self
                    .asm
                    .mov_imm32(dst, if v { 1 } else { 0 })
                    .map_err(BackendError::from);
            }
            Opcode::Parm(slot) => return self.emit_parm_gp(slot as usize, dst),
            Opcode::OsrI(idx) => return self.emit_osr_gp(idx, dst, false),
            Opcode::OsrL(idx) | Opcode::OsrP(idx) => {
                return self.emit_osr_gp(idx, dst, true);
            }
            _ => {}
        }
        if self.is_pure(node) {
            self.emit_pure_gp(node)?;
            if dst != Register::RAX {
                self.asm.mov_reg_reg(dst, Register::RAX, true)?;
            }
            return Ok(());
        }
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Gp(r) => {
                if r != dst {
                    self.asm.mov_reg_reg(dst, r, self.is_wide_gp(node))?;
                }
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                self.asm.mov_reg_mem(
                    dst,
                    Some(Register::FP),
                    None,
                    Scale::S1,
                    disp,
                    self.is_wide_gp(node),
                )?;
            }
            Loc::Xmm(_) => return Err(BackendError::InvalidGraph("gp/xmm mismatch".into())),
        }
        Ok(())
    }

    fn emit_gp_to(&mut self, node: NodeId, dst: Register) -> Result<(), BackendError> {
        self.emit_gp(node, dst)
    }

    fn emit_xmm_to(&mut self, node: NodeId, dst: XMMRegister) -> Result<(), BackendError> {
        if node == NONE {
            return Err(BackendError::InvalidGraph("missing xmm operand".into()));
        }
        let n = self.gvn.node(node);
        match n.op {
            Opcode::ConF(v) => {
                self.asm.mov_imm32(Register::RAX, v as i32)?;
                self.asm.movd_xmm_from_gp(dst, Register::RAX)?;
                return Ok(());
            }
            Opcode::ConD(v) => {
                self.asm.mov_imm(Register::RAX, v as i64)?;
                self.asm.movq_xmm_from_gp(dst, Register::RAX)?;
                return Ok(());
            }
            Opcode::OsrF(idx) => return self.emit_osr_xmm(idx, dst, false),
            Opcode::OsrD(idx) => return self.emit_osr_xmm(idx, dst, true),
            _ => {}
        }
        if self.is_pure(node) {
            self.emit_pure_xmm(node)?;
            if dst != XMMRegister::XMM0 {
                self.asm
                    .mov_xmm_xmm(dst, XMMRegister::XMM0, self.is_double_node(node))?;
            }
            return Ok(());
        }
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Xmm(r) => {
                if r != dst {
                    self.asm.mov_xmm_xmm(dst, r, self.is_double_node(node))?;
                }
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                if self.is_double_node(node) {
                    self.asm.sse_load_mem(dst, Register::FP, disp, true)?;
                } else {
                    self.asm.sse_load_mem(dst, Register::FP, disp, false)?;
                }
            }
            Loc::Gp(_) => return Err(BackendError::InvalidGraph("xmm/gp mismatch".into())),
        }
        Ok(())
    }

    /// Loads one marshal-buffer slot: `[rbp + slot_disp(0)]` holds the
    /// buffer pointer written by the OSR prologue.
    fn emit_osr_gp(
        &mut self,
        idx: u32,
        dst: Register,
        wide: bool,
    ) -> Result<(), BackendError> {
        self.asm
            .mov_reg_mem(
                Register::RAX,
                Some(Register::FP),
                None,
                Scale::S1,
                self.slot_disp(0),
                true,
            )
            .map_err(BackendError::from)?;
        self.asm
            .mov_reg_mem(
                dst,
                Some(Register::RAX),
                None,
                Scale::S1,
                (idx as i32) * 8,
                wide,
            )
            .map_err(BackendError::from)
    }

    fn emit_osr_xmm(
        &mut self,
        idx: u32,
        dst: XMMRegister,
        double: bool,
    ) -> Result<(), BackendError> {
        self.asm
            .mov_reg_mem(
                Register::RAX,
                Some(Register::FP),
                None,
                Scale::S1,
                self.slot_disp(0),
                true,
            )
            .map_err(BackendError::from)?;
        self.asm
            .sse_load_mem(dst, Register::RAX, (idx as i32) * 8, double)
            .map_err(BackendError::from)
    }

    fn emit_parm_gp(&mut self, slot: usize, dst: Register) -> Result<(), BackendError> {
        let disp = self.slot_disp(slot);
        let wide = self
            .param_kinds
            .get(&slot)
            .map(|k| k.is_wide())
            .unwrap_or(false);
        self.asm
            .mov_reg_mem(dst, Some(Register::FP), None, Scale::S1, disp, wide)
            .map_err(BackendError::from)
    }

    fn loc_of(&self, node: NodeId) -> Result<Loc, BackendError> {
        self.alloc
            .loc
            .get(&node)
            .copied()
            .ok_or_else(|| BackendError::InvalidGraph(format!("no location for node {}", node)))
    }

    fn store_gp_loc(
        &mut self,
        node: NodeId,
        reg: Register,
        wide: bool,
    ) -> Result<(), BackendError> {
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Gp(r) => {
                if r != reg {
                    self.asm.mov_reg_reg(r, reg, wide)?;
                }
                Ok(())
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                self.asm
                    .mov_mem_reg(Some(Register::FP), None, Scale::S1, disp, reg, wide)
                    .map_err(BackendError::from)
            }
            Loc::Xmm(_) => Err(BackendError::InvalidGraph("gp/xmm mismatch".into())),
        }
    }

    fn store_xmm_loc(&mut self, node: NodeId, xmm: XMMRegister) -> Result<(), BackendError> {
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Xmm(r) => {
                if r != xmm {
                    self.asm.mov_xmm_xmm(r, xmm, self.is_double_node(node))?;
                }
                Ok(())
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                if self.is_double_node(node) {
                    self.asm
                        .sse_store_mem(xmm, Register::FP, disp, true)
                        .map_err(BackendError::from)
                } else {
                    self.asm
                        .sse_store_mem(xmm, Register::FP, disp, false)
                        .map_err(BackendError::from)
                }
            }
            Loc::Gp(_) => Err(BackendError::InvalidGraph("xmm/gp mismatch".into())),
        }
    }

    // ---- pure node selection ----------------------------------------------

    fn is_double_node(&self, node: NodeId) -> bool {
        self.is_double_node_deep(node, 0)
    }

    fn is_double_node_deep(&self, node: NodeId, depth: u8) -> bool {
        if node == NONE {
            return false;
        }
        let n = self.gvn.node(node);
        if matches!(n.op, Opcode::Phi) {
            if depth >= 32 {
                return false;
            }
            let values = &n.inputs[..n.inputs.len().saturating_sub(1)];
            return values
                .iter()
                .any(|&i| i != NONE && self.is_double_node_deep(i, depth + 1));
        }
        n.ty == Type::Double
    }

    fn emit_pure_gp(&mut self, node: NodeId) -> Result<(), BackendError> {
        let op = self.gvn.node(node).op.clone();
        let inputs = self.gvn.node(node).inputs.clone();
        match op {
            Opcode::NegI | Opcode::NegL => {
                let wide = matches!(op, Opcode::NegL);
                self.emit_value(inputs[0])?;
                self.asm.emit_rex(wide, false, false, false);
                self.buf.write(&[0xf7, 0xd8])?; // neg rax/eax
                Ok(())
            }
            Opcode::AddI
            | Opcode::SubI
            | Opcode::MulI
            | Opcode::AndI
            | Opcode::OrI
            | Opcode::XorI
            | Opcode::AddL
            | Opcode::SubL
            | Opcode::MulL
            | Opcode::AndL
            | Opcode::OrL
            | Opcode::XorL
            | Opcode::LShiftI
            | Opcode::RShiftI
            | Opcode::URShiftI
            | Opcode::LShiftL
            | Opcode::RShiftL
            | Opcode::URShiftL
            | Opcode::CmpI
            | Opcode::CmpL
            | Opcode::BoolCmp(_) => {
                let wide = self.is_wide_gp(node)
                    || self.is_wide_gp(inputs[0])
                    || self.is_wide_gp(inputs[1]);
                let t = self.alloc_temp()?;
                self.emit_value(inputs[0])?;
                self.store_gp_to_temp(t, Register::RAX, wide)?;
                self.emit_gp_to(inputs[1], Register::RCX)?;
                self.load_gp_from_temp(t, Register::RAX, wide)?;
                self.free_temp();
                self.emit_gp_op(&op, wide)
            }
            Opcode::I2L => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.asm
                    .movsxd(Register::RAX, Register::RAX)
                    .map_err(BackendError::from)
            }
            Opcode::L2I => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.asm.emit_rex(false, false, false, false);
                self.buf.write(&[0x89, 0xc0])?; // mov eax,eax
                Ok(())
            }
            Opcode::I2B => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.asm.emit_rex(false, false, false, false);
                self.buf.write(&[0x0f, 0xbe, 0xc0])?; // movsx eax,al
                Ok(())
            }
            Opcode::I2C => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.asm.emit_rex(false, false, false, false);
                self.buf.write(&[0x0f, 0xb7, 0xc0])?; // movzx eax,ax
                Ok(())
            }
            Opcode::I2S => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.asm.emit_rex(false, false, false, false);
                self.buf.write(&[0x0f, 0xbf, 0xc0])?; // movsx eax,ax
                Ok(())
            }
            Opcode::F2I | Opcode::F2L | Opcode::D2I | Opcode::D2L => {
                // Float->int needs the SharedRuntime rules (NaN -> 0,
                // saturate on overflow) that raw cvttss2si/cvttsd2si do
                // not implement, so it goes through the FloatConvert
                // stub: rcx = source bits, r8 = conversion kind.
                let kind = match op {
                    Opcode::F2I => 0,
                    Opcode::F2L => 1,
                    Opcode::D2I => 2,
                    _ => 3,
                };
                let double = matches!(op, Opcode::D2I | Opcode::D2L);
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                if double {
                    self.asm.movq_xmm_to_gp(Register::RAX, XMMRegister::XMM0)?;
                } else {
                    self.asm.movd_xmm_to_gp(Register::RAX, XMMRegister::XMM0)?;
                }
                self.asm.mov_reg_reg(Register::RCX, Register::RAX, true)?;
                self.asm.mov_imm32(Register::R8, kind)?;
                self.emit_runtime_call_addr(RuntimeStub::FloatConvert)
            }
            Opcode::CmpF(greater_nan) | Opcode::CmpD(greater_nan) => {
                let double = matches!(op, Opcode::CmpD(_));
                self.emit_fp_cmp_value(inputs[0], inputs[1], double, greater_nan)
            }
            _ => Err(BackendError::Unsupported(format!("pure gp op {:?}", op))),
        }
    }

    fn emit_gp_op(&mut self, op: &Opcode, wide: bool) -> Result<(), BackendError> {
        use Opcode::*;
        match op {
            AddI | AddL => {
                return self
                    .asm
                    .alu_rr(AluOp::Add, Register::RAX, Register::RCX, wide)
                    .map_err(BackendError::from);
            }
            SubI | SubL => {
                return self
                    .asm
                    .alu_rr(AluOp::Sub, Register::RAX, Register::RCX, wide)
                    .map_err(BackendError::from);
            }
            MulI | MulL => {
                return self
                    .asm
                    .alu_rr(AluOp::Mul, Register::RAX, Register::RCX, wide)
                    .map_err(BackendError::from);
            }
            AndI | AndL => {
                return self
                    .asm
                    .alu_rr(AluOp::And, Register::RAX, Register::RCX, wide)
                    .map_err(BackendError::from);
            }
            OrI | OrL => {
                return self
                    .asm
                    .alu_rr(AluOp::Or, Register::RAX, Register::RCX, wide)
                    .map_err(BackendError::from);
            }
            XorI | XorL => {
                return self
                    .asm
                    .alu_rr(AluOp::Xor, Register::RAX, Register::RCX, wide)
                    .map_err(BackendError::from);
            }
            LShiftI | LShiftL => {
                return self
                    .asm
                    .shift_op(ShiftOp::Shl, wide)
                    .map_err(BackendError::from);
            }
            RShiftI | RShiftL => {
                return self
                    .asm
                    .shift_op(ShiftOp::Sar, wide)
                    .map_err(BackendError::from);
            }
            URShiftI | URShiftL => {
                return self
                    .asm
                    .shift_op(ShiftOp::Shr, wide)
                    .map_err(BackendError::from);
            }
            CmpI | CmpL => {
                self.asm.cmp_rr(Register::RAX, Register::RCX, wide)?;
                self.cmp_to_int()
            }
            BoolCmp(cond) => {
                self.asm.cmp_rr(Register::RAX, Register::RCX, wide)?;
                self.setcc_bool(*cond)
            }
            _ => Err(BackendError::Unsupported(format!("gp op {:?}", op))),
        }
    }

    fn emit_div_rem(&mut self, node: NodeId, wide: bool) -> Result<(), BackendError> {
        let is_rem = matches!(
            self.gvn.node(node).op,
            Opcode::RemI | Opcode::RemL
        );
        let stub_addr = self.stub_addr_for(node, RuntimeStub::ArithmeticException)?;
        self.asm.test_rr(Register::RCX, Register::RCX, wide)?;
        let jnz = self.emit_jmp_local_cc(XCond::NE)?;
        self.asm.emit_abs_call(stub_addr)?;
        // With an exception edge the non-unwinding trampoline returns
        // here with the pending outcome recorded; skip the division
        // and let emit_exc_dispatch route to the handler block.
        let jmp_end = self.emit_jmp_local()?;
        let after_zero = self.pos();
        self.patch_local(jnz, after_zero)?;
        self.asm.cmp_imm8(Register::RCX, -1, wide)?;
        let je_neg = self.emit_jmp_local_cc(XCond::E)?;
        self.asm.cqo(wide)?;
        self.asm.idiv(Register::RCX, wide)?;
        if is_rem {
            self.asm.mov_reg_reg(Register::RAX, Register::RDX, wide)?;
        }
        let done = self.emit_jmp_local()?;
        self.patch_local(je_neg, self.pos())?;
        if is_rem {
            self.asm.emit_rex(wide, false, false, false);
            self.buf.write(&[0x31, 0xc0])?;
        } else {
            self.asm.neg(Register::RAX, wide)?;
        }
        self.patch_local(done, self.pos())?;
        self.patch_local(jmp_end, self.pos())?;
        Ok(())
    }

    fn emit_pure_xmm(&mut self, node: NodeId) -> Result<(), BackendError> {
        let op = self.gvn.node(node).op.clone();
        let inputs = self.gvn.node(node).inputs.clone();
        if let Opcode::Parm(slot) = op {
            let disp = self.slot_disp(slot as usize);
            // Parm nodes carry Type::Bottom, so the parameter kind table
            // is the authoritative source for the load width.
            let double = self
                .param_kinds
                .get(&(slot as usize))
                .map(|k| *k == ParamKind::F64)
                .unwrap_or_else(|| self.is_double_node(node));
            self.asm
                .sse_load_mem(XMMRegister::XMM0, Register::FP, disp, double)?;
            return Ok(());
        }
        match op {
            Opcode::AddF
            | Opcode::SubF
            | Opcode::MulF
            | Opcode::DivF
            | Opcode::AddD
            | Opcode::SubD
            | Opcode::MulD
            | Opcode::DivD => {
                let double = self.is_double_node(node);
                let alu = match op {
                    Opcode::AddF | Opcode::AddD => AluOp::Add,
                    Opcode::SubF | Opcode::SubD => AluOp::Sub,
                    Opcode::MulF | Opcode::MulD => AluOp::Mul,
                    _ => AluOp::Div,
                };
                let t = self.alloc_temp()?;
                // xmm0 must end up holding inputs[0] (the left operand):
                // sse_arith computes xmm0 = xmm0 OP xmm1, and sub/div are
                // not commutative.
                self.emit_xmm_to(inputs[1], XMMRegister::XMM0)?;
                self.asm.sse_store_mem(
                    XMMRegister::XMM0,
                    Register::FP,
                    self.temp_disp(t),
                    double,
                )?;
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                self.asm.sse_load_mem(
                    XMMRegister::XMM1,
                    Register::FP,
                    self.temp_disp(t),
                    double,
                )?;
                self.free_temp();
                self.asm.sse_arith(alu, double).map_err(BackendError::from)
            }
            Opcode::NegF | Opcode::NegD => {
                let double = matches!(op, Opcode::NegD);
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                if double {
                    self.asm
                        .mov_imm(Register::RAX, 0x8000_0000_0000_0000u64 as i64)?;
                    self.asm
                        .movq_xmm_from_gp(XMMRegister::XMM1, Register::RAX)?;
                    self.asm.emit_rex(true, false, false, false);
                    self.buf.write(&[0x66, 0x0f, 0x57, 0xc1])?; // xorpd xmm0,xmm1
                } else {
                    self.asm.mov_imm32(Register::RAX, 0x8000_0000u32 as i32)?;
                    self.asm
                        .movd_xmm_from_gp(XMMRegister::XMM1, Register::RAX)?;
                    self.buf.write(&[0x0f, 0x57, 0xc1])?; // xorps xmm0,xmm1
                }
                Ok(())
            }
            Opcode::I2F | Opcode::L2F => {
                let wide = matches!(op, Opcode::L2F);
                self.emit_gp(inputs[0], Register::RAX)?;
                self.asm
                    .cvtsi2ss(XMMRegister::XMM0, Register::RAX, wide)
                    .map_err(BackendError::from)
            }
            Opcode::I2D | Opcode::L2D => {
                let wide = matches!(op, Opcode::L2D);
                self.emit_gp(inputs[0], Register::RAX)?;
                self.asm
                    .cvtsi2sd(XMMRegister::XMM0, Register::RAX, wide)
                    .map_err(BackendError::from)
            }
            Opcode::F2D => {
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                self.asm
                    .cvtss2sd(XMMRegister::XMM0, XMMRegister::XMM0)
                    .map_err(BackendError::from)
            }
            Opcode::D2F => {
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                self.asm
                    .cvtsd2ss(XMMRegister::XMM0, XMMRegister::XMM0)
                    .map_err(BackendError::from)
            }
            Opcode::RemF | Opcode::RemD => self.emit_float_rem(&op, &inputs),
            _ => Err(BackendError::Unsupported(format!("pure xmm op {:?}", op))),
        }
    }

    // ---- float compares -----------------------------------------------------

    fn emit_fp_cmp_value(
        &mut self,
        a: NodeId,
        b: NodeId,
        double: bool,
        greater_nan: bool,
    ) -> Result<(), BackendError> {
        let t = self.alloc_temp()?;
        // ucomiss xmm0,xmm1 compares xmm0 against xmm1, so xmm0 must hold
        // the left operand `a`.
        self.emit_xmm_to(b, XMMRegister::XMM0)?;
        self.asm
            .sse_store_mem(XMMRegister::XMM0, Register::FP, self.temp_disp(t), double)?;
        self.emit_xmm_to(a, XMMRegister::XMM0)?;
        self.asm
            .sse_load_mem(XMMRegister::XMM1, Register::FP, self.temp_disp(t), double)?;
        self.free_temp();
        // ucomiss/ucomisd xmm0,xmm1
        self.asm.emit_rex(double, false, false, false);
        self.buf
            .write(&[if double { 0x66 } else { 0x0f }, 0x2e, 0xc1])?;
        // NaN -> greater_nan ? 1 : -1 ; else CF->-1, ZF->0, else 1
        let start = self.pos();
        let nan_jp = self.emit_jmp_local_cc(XCond::P)?;
        let less_jb = self.emit_jmp_local_cc(XCond::B)?;
        let eq_je = self.emit_jmp_local_cc(XCond::E)?;
        self.asm.mov_imm32(Register::RAX, 1)?;
        let end_jmp = self.emit_jmp_local()?;
        self.patch_local(less_jb, self.pos())?;
        self.asm.mov_imm32(Register::RAX, -1)?;
        let end2 = self.emit_jmp_local()?;
        self.patch_local(eq_je, self.pos())?;
        self.asm.emit_rex(false, false, false, false);
        self.buf.write(&[0x31, 0xc0])?; // xor eax,eax
        let end3 = self.emit_jmp_local()?;
        self.patch_local(nan_jp, self.pos())?;
        self.asm
            .mov_imm32(Register::RAX, if greater_nan { 1 } else { -1 })?;
        self.patch_local(end_jmp, self.pos())?;
        self.patch_local(end2, self.pos())?;
        self.patch_local(end3, self.pos())?;
        let _ = start;
        Ok(())
    }

    /// `frem`/`drem` have no SSE equivalent, so they go through the
    /// FloatRem stub which applies the exact SharedRuntime semantics
    /// (fmod + Win64 infinity rule).  rcx/rdx carry the operand bit
    /// patterns staged through temp slots (evaluating the second
    /// operand may itself emit stub calls), r8 selects f32/f64, and
    /// the result arrives in xmm0 through the stub xmm channel.
    fn emit_float_rem(&mut self, op: &Opcode, inputs: &[NodeId]) -> Result<(), BackendError> {
        let double = matches!(op, Opcode::RemD);
        let kind = if double { 1 } else { 0 };
        let stage = |this: &mut Self, value: NodeId| -> Result<usize, BackendError> {
            let t = this.alloc_temp()?;
            this.emit_xmm_to(value, XMMRegister::XMM0)?;
            if double {
                this.asm.movq_xmm_to_gp(Register::RAX, XMMRegister::XMM0)?;
            } else {
                this.asm.movd_xmm_to_gp(Register::RAX, XMMRegister::XMM0)?;
            }
            this.store_gp_to_temp(t, Register::RAX, true)?;
            Ok(t)
        };
        let ta = stage(self, inputs[0])?;
        let tb = stage(self, inputs[1])?;
        self.load_gp_from_temp(ta, Register::RCX, true)?;
        self.load_gp_from_temp(tb, Register::RDX, true)?;
        self.free_temp();
        self.free_temp();
        self.asm.mov_imm32(Register::R8, kind)?;
        self.emit_runtime_call_addr(RuntimeStub::FloatRem)
    }

    // ---- safepoint ---------------------------------------------------------

    fn emit_safepoint_poll(&mut self) -> Result<(), BackendError> {
        let addr = self
            .options
            .stubs
            .get(&RuntimeStub::SafepointPoll)
            .copied()
            .ok_or(BackendError::MissingRuntimeStub(RuntimeStub::SafepointPoll))?;
        let xmm_locs: Vec<XMMRegister> = self
            .alloc
            .loc
            .values()
            .filter_map(|&l| match l {
                Loc::Xmm(r) => Some(r),
                _ => None,
            })
            .collect();
        for r in &xmm_locs {
            let slot = self.alloc.xmm_save_slot(*r);
            let disp = self.slot_disp(slot);
            self.asm.sse_store_mem(*r, Register::FP, disp, true)?;
        }
        self.asm.emit_abs_call(addr)?;
        self.pc_descs
            .push(PcDesc::new(self.pos() as i32, 0, PcDesc::PCDESC_REEXECUTE));
        for r in &xmm_locs {
            let slot = self.alloc.xmm_save_slot(*r);
            let disp = self.slot_disp(slot);
            self.asm.sse_load_mem(*r, Register::FP, disp, true)?;
        }
        Ok(())
    }

    // ---- branches / local labels -------------------------------------------

    fn emit_jmp_block(&mut self, target: usize) -> Result<(), BackendError> {
        self.buf.write_byte(0xe9)?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        self.patches.push(BranchPatch { disp, target });
        Ok(())
    }

    fn emit_jcc_block(&mut self, cond: XCond, target: usize) -> Result<(), BackendError> {
        self.buf.write(&[0x0f, 0x80 | cond as u8])?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        self.patches.push(BranchPatch { disp, target });
        Ok(())
    }

    fn patch_branches(&mut self) -> Result<(), BackendError> {
        let patches = self.patches.clone();
        for p in patches {
            let Some(&target_pos) = self.block_offsets.get(&p.target) else {
                return Err(BackendError::InvalidGraph(format!(
                    "missing block {}",
                    p.target
                )));
            };
            let rel = target_pos as i64 - (p.disp as i64 + 4);
            self.buf.patch_i32(p.disp, rel as i32)?;
        }
        Ok(())
    }

    fn emit_jmp_local(&mut self) -> Result<usize, BackendError> {
        self.buf.write_byte(0xe9)?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        Ok(disp)
    }

    fn emit_jmp_local_cc(&mut self, cond: XCond) -> Result<usize, BackendError> {
        self.buf.write(&[0x0f, 0x80 | cond as u8])?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        Ok(disp)
    }

    fn patch_local(&mut self, disp: usize, target: usize) -> Result<(), BackendError> {
        let rel = target as i64 - (disp as i64 + 4);
        self.buf.patch_i32(disp, rel as i32).map_err(Into::into)
    }

    fn cmp_to_int(&mut self) -> Result<(), BackendError> {
        // result = (a > b) - (a < b) in {-1,0,1}
        // Zero with mov, not xor: xor would clobber the flags the
        // following setcc instructions read.
        self.asm.mov_imm32(Register::R8, 0)?;
        self.asm.mov_imm32(Register::R9, 0)?;
        self.asm.emit_setcc(XCond::L, Register::R8)?;
        self.asm.emit_setcc(XCond::G, Register::R9)?;
        self.asm.movzx_reg(Register::R8)?;
        self.asm.movzx_reg(Register::R9)?;
        self.asm.mov_reg_reg(Register::RAX, Register::R9, false)?;
        self.asm
            .alu_rr(AluOp::Sub, Register::RAX, Register::R8, false)
            .map_err(BackendError::from)
    }

    fn setcc_bool(&mut self, cond: CmpCondition) -> Result<(), BackendError> {
        // mov eax,0 preserves the flags that setcc reads (xor would
        // clear them and make every comparison look equal).
        self.asm.mov_imm32(Register::RAX, 0)?;
        let cc = self.xcond_for(cond);
        self.asm.emit_setcc(cc, Register::RAX)?;
        self.asm.emit_rex(false, false, false, false);
        self.buf.write(&[0x0f, 0xb6, 0xc0])?; // movzx eax,al
        Ok(())
    }

    fn store_gp_to_temp(
        &mut self,
        idx: usize,
        reg: Register,
        wide: bool,
    ) -> Result<(), BackendError> {
        self.asm
            .mov_mem_reg(
                Some(Register::FP),
                None,
                Scale::S1,
                self.temp_disp(idx),
                reg,
                wide,
            )
            .map_err(BackendError::from)
    }

    fn load_gp_from_temp(
        &mut self,
        idx: usize,
        reg: Register,
        wide: bool,
    ) -> Result<(), BackendError> {
        self.asm
            .mov_reg_mem(
                reg,
                Some(Register::FP),
                None,
                Scale::S1,
                self.temp_disp(idx),
                wide,
            )
            .map_err(BackendError::from)
    }
}

pub fn compile_schedule(
    buf: &CodeBuffer,
    gvn: &PhaseGVN,
    schedule: &Schedule,
    alloc: &Allocation,
    options: &BackendOptions,
) -> Result<EmitResult, BackendError> {
    validate_supported(gvn)?;
    let mut emitter = X86Emitter::new(buf, gvn, schedule, alloc, options);
    emitter.emit()
}

/// Executable-subset gate.  Anything that needs the object heap, runtime
/// stubs other than the safepoint poll, or Java exception semantics is
/// rejected instead of emitting incorrect code.
pub fn validate_supported(gvn: &PhaseGVN) -> Result<(), BackendError> {
    for n in &gvn.nodes {
        let last_is_site = n
            .inputs
            .last()
            .map(|&i| matches!(gvn.node(i).op, Opcode::SitePtr(_)))
            .unwrap_or(false);
        let bad = match n.op {
            Opcode::Proj(_) => true,
            Opcode::LoadI
            | Opcode::LoadL
            | Opcode::LoadF
            | Opcode::LoadD
            | Opcode::LoadP
            | Opcode::LoadB
            | Opcode::LoadC
            | Opcode::LoadS => {
                if last_is_site {
                    !(n.inputs.len() == 3 || n.inputs.len() == 5)
                } else {
                    n.inputs.len() != 5
                }
            }
            Opcode::StoreI
            | Opcode::StoreL
            | Opcode::StoreF
            | Opcode::StoreD
            | Opcode::StoreP
            | Opcode::StoreB
            | Opcode::StoreC
            | Opcode::StoreS => {
                if last_is_site {
                    !(n.inputs.len() == 4 || n.inputs.len() == 6)
                } else {
                    n.inputs.len() != 6
                }
            }
            Opcode::ConP(v) => v != 0,
            _ => false,
        };
        if bad {
            return Err(BackendError::Unsupported(format!(
                "node {} opcode {:?} is outside the executable subset",
                n.id, n.op
            )));
        }
    }
    Ok(())
}

pub fn default_buffer() -> Result<CodeBuffer, CodeBufferError> {
    CodeBuffer::new(DEFAULT_BUFFER_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_arity_load_rejected_array_arity_accepted() {
        let mut gvn = PhaseGVN::new();
        let c = gvn.transform(Opcode::ConI(0), vec![]);
        gvn.transform(Opcode::LoadI, vec![c, c, c, c]);
        assert!(validate_supported(&gvn).is_err());

        let mut gvn2 = PhaseGVN::new();
        let c2 = gvn2.transform(Opcode::ConI(0), vec![]);
        gvn2.transform(Opcode::LoadI, vec![c2, c2, c2, c2, c2]);
        assert!(validate_supported(&gvn2).is_ok());

        let mut gvn3 = PhaseGVN::new();
        let c3 = gvn3.transform(Opcode::ConI(0), vec![]);
        gvn3.transform(Opcode::StoreI, vec![c3, c3, c3, c3, c3]);
        assert!(validate_supported(&gvn3).is_err());

        let mut gvn4 = PhaseGVN::new();
        let c4 = gvn4.transform(Opcode::ConI(0), vec![]);
        gvn4.transform(Opcode::StoreI, vec![c4, c4, c4, c4, c4, c4]);
        assert!(validate_supported(&gvn4).is_ok());
    }
}
