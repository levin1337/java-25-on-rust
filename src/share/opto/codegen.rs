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

use super::gvn::PhaseGVN;
use super::node::{CmpCondition, NodeId, Opcode, Type, NONE};
use super::regalloc::{Allocation, Loc, TEMP_SLOTS};
use super::scheduler::{phis_of_region, region_pred_index, Schedule, TermKind};
use crate::share::c1::asm::codeBuffer::{CodeBuffer, CodeBufferError, DEFAULT_BUFFER_SIZE};
use crate::share::c1::asm::register::{Condition as XCond, Register, Scale, XMMRegister};
use crate::share::c1::c1_LIR::RuntimeStub;
use crate::share::code::pcDesc::PcDesc;
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
    /// Incoming arguments in ABI order with their JVM local slot.
    pub params: Vec<ParamSlot>,
    pub emit_safepoints: bool,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            stubs: BTreeMap::new(),
            params: Vec::new(),
            emit_safepoints: true,
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
        Self {
            buf,
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
            }
            self.emit_terminator(b)?;
        }
        self.patch_branches()?;
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
            return Err(BackendError::Unsupported("operand temp stack overflow".into()));
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
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x89, 0xe5])?; // mov rbp,rsp
        for &r in &self.alloc.saved_regs {
            self.emit_push(r)?;
        }
        if self.alloc.frame_size > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x81, 0xec])?;
            self.buf.write_i32(self.alloc.frame_size as i32)?; // sub rsp,imm32
        }
        self.emit_param_moves()?;
        Ok(())
    }

    fn emit_param_moves(&mut self) -> Result<(), BackendError> {
        for (i, p) in self.options.params.iter().enumerate() {
            if p.kind.is_xmm() {
                return Err(BackendError::Unsupported(
                    "floating-point parameters are outside the executable subset".into(),
                ));
            }
            let disp = self.slot_disp(p.local_slot);
            if i < ARG_REGS.len() {
                self.mov_mem_reg(
                    Some(Register::FP),
                    None,
                    Scale::S1,
                    disp,
                    ARG_REGS[i],
                    true,
                )?;
            } else {
                let stack_off = 48 + ((i - ARG_REGS.len()) * 8) as i32;
                self.mov_reg_mem(
                    Register::RAX,
                    Some(Register::FP),
                    None,
                    Scale::S1,
                    stack_off,
                    true,
                )?;
                self.mov_mem_reg(
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
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x8d, 0x65])?;
        self.buf.write_byte((-(8i32 * k as i32)) as i8 as u8)?;
        for &r in self.alloc.saved_regs.iter().rev() {
            self.emit_pop(r)?;
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
                TermKind::Athrow => Err(BackendError::Unsupported("athrow".into())),
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
            return Err(BackendError::InvalidGraph("If without IfTrue/IfFalse".into()));
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
                self.emit_jmp_block(true_t).and_then(|_| self.emit_jmp_block(false_t))
            } else {
                self.emit_jmp_block(false_t).and_then(|_| self.emit_jmp_block(true_t))
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
        self.emit_rex(false, false, false, false);
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
        self.cmp_rr(Register::RAX, Register::RCX, wide)
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

    fn emit_phi_moves(&mut self, _from: usize, to: usize, edge_ctrl: NodeId) -> Result<(), BackendError> {
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
        // cycles without a permutation solver.
        let mut moves: Vec<(NodeId, NodeId)> = Vec::new();
        for phi in phis {
            let inputs = self.gvn.node(phi).inputs.clone();
            let Some(&src) = inputs.get(pred_idx) else { continue };
            if src == NONE {
                continue;
            }
            moves.push((src, phi));
        }
        if moves.len() > TEMP_SLOTS {
            return Err(BackendError::Unsupported(format!(
                "{} phi copies exceed temp slots",
                moves.len()
            )));
        }
        for (i, &(src, dst)) in moves.iter().enumerate() {
            self.load_move_src(src, dst, i)?;
        }
        for (i, &(_, dst)) in moves.iter().enumerate() {
            self.store_move_dst(dst, i)?;
        }
        Ok(())
    }

    fn move_kind_xmm(&self, dst: NodeId) -> bool {
        self.is_xmm(dst)
    }

    fn load_move_src(&mut self, src: NodeId, dst: NodeId, slot: usize) -> Result<(), BackendError> {
        let disp = self.temp_disp(slot);
        if self.move_kind_xmm(dst) {
            self.emit_xmm_to(src, XMMRegister::XMM15)?;
            self.sse_store_mem(XMMRegister::XMM15, Register::FP, disp, true)
        } else {
            self.emit_gp(src, Register::R11)?;
            self.mov_mem_reg(Some(Register::FP), None, Scale::S1, disp, Register::R11, true)
        }
    }

    fn store_move_dst(&mut self, dst: NodeId, slot: usize) -> Result<(), BackendError> {
        let disp = self.temp_disp(slot);
        if self.move_kind_xmm(dst) {
            self.sse_load_mem(XMMRegister::XMM15, Register::FP, disp, true)?;
            self.store_xmm_loc(dst, XMMRegister::XMM15)
        } else {
            self.mov_reg_mem(Register::R11, Some(Register::FP), None, Scale::S1, disp, true)?;
            self.store_gp_loc(dst, Register::R11, self.is_wide_gp(dst))
        }
    }

    fn is_pure(&self, node: NodeId) -> bool {
        if node == NONE {
            return false;
        }
        matches!(
            self.gvn.node(node).op,
            Opcode::AddI | Opcode::SubI | Opcode::MulI | Opcode::AndI | Opcode::OrI | Opcode::XorI
                | Opcode::LShiftI | Opcode::RShiftI | Opcode::URShiftI | Opcode::NegI
                | Opcode::AddL | Opcode::SubL | Opcode::MulL | Opcode::AndL | Opcode::OrL
                | Opcode::XorL | Opcode::LShiftL | Opcode::RShiftL | Opcode::URShiftL | Opcode::NegL
                | Opcode::AddF | Opcode::SubF | Opcode::MulF | Opcode::DivF | Opcode::NegF
                | Opcode::AddD | Opcode::SubD | Opcode::MulD | Opcode::DivD | Opcode::NegD
                | Opcode::CmpI | Opcode::CmpL | Opcode::CmpF(_) | Opcode::CmpD(_)
                | Opcode::BoolCmp(_)
                | Opcode::I2L | Opcode::I2F | Opcode::I2D | Opcode::L2I | Opcode::L2F | Opcode::L2D
                | Opcode::F2I | Opcode::F2L | Opcode::F2D | Opcode::D2I | Opcode::D2L | Opcode::D2F
                | Opcode::I2B | Opcode::I2C | Opcode::I2S
                | Opcode::ConI(_) | Opcode::ConL(_) | Opcode::ConF(_) | Opcode::ConD(_)
                | Opcode::ConP(_) | Opcode::Bool(_) | Opcode::Parm(_)
        )
    }

    // ---- scheduled effects -------------------------------------------------

    fn emit_effect(&mut self, node: NodeId) -> Result<(), BackendError> {
        let op = self.gvn.node(node).op.clone();
        match op {
            Opcode::LoadI | Opcode::LoadL | Opcode::LoadF | Opcode::LoadD | Opcode::LoadB
            | Opcode::LoadC | Opcode::LoadS => self.emit_load(node),
            Opcode::StoreI | Opcode::StoreL | Opcode::StoreF | Opcode::StoreD => self.emit_store(node),
            _ => Err(BackendError::Unsupported(format!(
                "scheduled effect {:?} is outside the executable subset",
                op
            ))),
        }
    }

    fn emit_load(&mut self, node: NodeId) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let (base, off) = (inputs[2], inputs[3]);
        let disp = self.const_i32(off)?;
        self.emit_gp_to(base, Register::RAX)?;
        match self.gvn.node(node).op {
            Opcode::LoadF | Opcode::LoadD => {
                let double = matches!(self.gvn.node(node).op, Opcode::LoadD);
                self.sse_load_mem(XMMRegister::XMM0, Register::RAX, disp, double)?;
                self.store_xmm_loc(node, XMMRegister::XMM0)
            }
            Opcode::LoadB => {
                self.movsx_mem_reg(Register::RAX, Register::RAX, disp, false)?;
                self.store_gp_loc(node, Register::RAX, false)
            }
            Opcode::LoadC => {
                self.movzx_mem_reg(Register::RAX, Register::RAX, disp, true)?;
                self.store_gp_loc(node, Register::RAX, false)
            }
            Opcode::LoadS => {
                self.movsx_mem_reg(Register::RAX, Register::RAX, disp, true)?;
                self.store_gp_loc(node, Register::RAX, false)
            }
            _ => {
                let wide = matches!(self.gvn.node(node).op, Opcode::LoadL);
                self.mov_reg_mem(Register::RAX, Some(Register::RAX), None, Scale::S1, disp, wide)?;
                self.store_gp_loc(node, Register::RAX, wide)
            }
        }
    }

    fn emit_store(&mut self, node: NodeId) -> Result<(), BackendError> {
        let inputs = self.gvn.node(node).inputs.clone();
        let (base, off, value) = (inputs[2], inputs[3], inputs[4]);
        let disp = self.const_i32(off)?;
        let op = self.gvn.node(node).op.clone();
        let t = self.alloc_temp()?;
        self.emit_gp_to(base, Register::RAX)?;
        self.store_gp_to_temp(t, Register::RAX, true)?;
        match op {
            Opcode::StoreF | Opcode::StoreD => {
                let double = matches!(op, Opcode::StoreD);
                self.emit_xmm_to(value, XMMRegister::XMM0)?;
                self.load_gp_from_temp(t, Register::RAX, true)?;
                self.free_temp();
                self.sse_store_mem(XMMRegister::XMM0, Register::RAX, disp, double)
            }
            _ => {
                self.emit_gp_to(value, Register::RCX)?;
                self.load_gp_from_temp(t, Register::RAX, true)?;
                self.free_temp();
                let wide = matches!(op, Opcode::StoreL);
                self.mov_mem_reg(Some(Register::RAX), None, Scale::S1, disp, Register::RCX, wide)
            }
        }
    }

    // ---- value selection ---------------------------------------------------

    fn is_xmm(&self, node: NodeId) -> bool {
        if node == NONE {
            return false;
        }
        let n = self.gvn.node(node);
        matches!(n.ty, Type::Float | Type::Double)
            || matches!(
                n.op,
                Opcode::ConF(_) | Opcode::ConD(_) | Opcode::LoadF | Opcode::LoadD
                    | Opcode::AddF | Opcode::SubF | Opcode::MulF | Opcode::DivF | Opcode::NegF
                    | Opcode::AddD | Opcode::SubD | Opcode::MulD | Opcode::DivD | Opcode::NegD
                    | Opcode::I2F | Opcode::I2D | Opcode::L2F | Opcode::L2D | Opcode::F2D | Opcode::D2F
            )
    }

    fn is_wide_gp(&self, node: NodeId) -> bool {
        if node == NONE {
            return false;
        }
        let n = self.gvn.node(node);
        if let Opcode::Parm(slot) = n.op {
            return self
                .param_kinds
                .get(&(slot as usize))
                .map(|k| k.is_wide())
                .unwrap_or(false);
        }
        matches!(n.ty, Type::Long | Type::Ref)
            || matches!(n.op, Opcode::ConL(_) | Opcode::ConP(_))
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
            self.mov_imm32(dst, 0)?;
            return Ok(());
        }
        let n = self.gvn.node(node);
        match n.op {
            Opcode::ConI(v) => return self.mov_imm32(dst, v as u32 as i32),
            Opcode::ConL(v) => return self.mov_imm(dst, v),
            Opcode::ConP(v) => return self.mov_imm(dst, v as i64),
            Opcode::Bool(v) => return self.mov_imm32(dst, if v { 1 } else { 0 }),
            Opcode::Parm(slot) => return self.emit_parm_gp(slot as usize, dst),
            _ => {}
        }
        if self.is_pure(node) {
            self.emit_pure_gp(node)?;
            if dst != Register::RAX {
                self.mov_reg_reg(dst, Register::RAX, true)?;
            }
            return Ok(());
        }
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Gp(r) => {
                if r != dst {
                    self.mov_reg_reg(dst, r, self.is_wide_gp(node))?;
                }
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                self.mov_reg_mem(dst, Some(Register::FP), None, Scale::S1, disp, self.is_wide_gp(node))?;
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
                self.mov_imm32(Register::RAX, v as i32)?;
                self.movd_xmm_from_gp(dst, Register::RAX)?;
                return Ok(());
            }
            Opcode::ConD(v) => {
                self.mov_imm(Register::RAX, v as i64)?;
                self.movq_xmm_from_gp(dst, Register::RAX)?;
                return Ok(());
            }
            _ => {}
        }
        if self.is_pure(node) {
            self.emit_pure_xmm(node)?;
            if dst != XMMRegister::XMM0 {
                self.mov_xmm_xmm(dst, XMMRegister::XMM0, self.is_double_node(node))?;
            }
            return Ok(());
        }
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Xmm(r) => {
                if r != dst {
                    self.mov_xmm_xmm(dst, r, self.is_double_node(node))?;
                }
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                if self.is_double_node(node) {
                    self.sse_load_mem(dst, Register::FP, disp, true)?;
                } else {
                    self.sse_load_mem(dst, Register::FP, disp, false)?;
                }
            }
            Loc::Gp(_) => return Err(BackendError::InvalidGraph("xmm/gp mismatch".into())),
        }
        Ok(())
    }

    fn emit_parm_gp(&mut self, slot: usize, dst: Register) -> Result<(), BackendError> {
        let disp = self.slot_disp(slot);
        let wide = self
            .param_kinds
            .get(&slot)
            .map(|k| k.is_wide())
            .unwrap_or(false);
        self.mov_reg_mem(dst, Some(Register::FP), None, Scale::S1, disp, wide)
    }

    fn loc_of(&self, node: NodeId) -> Result<Loc, BackendError> {
        self.alloc
            .loc
            .get(&node)
            .copied()
            .ok_or_else(|| BackendError::InvalidGraph(format!("no location for node {}", node)))
    }

    fn store_gp_loc(&mut self, node: NodeId, reg: Register, wide: bool) -> Result<(), BackendError> {
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Gp(r) => {
                if r != reg {
                    self.mov_reg_reg(r, reg, wide)?;
                }
                Ok(())
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                self.mov_mem_reg(Some(Register::FP), None, Scale::S1, disp, reg, wide)
            }
            Loc::Xmm(_) => Err(BackendError::InvalidGraph("gp/xmm mismatch".into())),
        }
    }

    fn store_xmm_loc(&mut self, node: NodeId, xmm: XMMRegister) -> Result<(), BackendError> {
        let loc = self.loc_of(node)?;
        match loc {
            Loc::Xmm(r) => {
                if r != xmm {
                    self.mov_xmm_xmm(r, xmm, self.is_double_node(node))?;
                }
                Ok(())
            }
            Loc::Spill(ord) => {
                let disp = self.spill_slot_disp(ord);
                if self.is_double_node(node) {
                    self.sse_store_mem(xmm, Register::FP, disp, true)
                } else {
                    self.sse_store_mem(xmm, Register::FP, disp, false)
                }
            }
            Loc::Gp(_) => Err(BackendError::InvalidGraph("xmm/gp mismatch".into())),
        }
    }

    // ---- pure node selection ----------------------------------------------

    fn is_double_node(&self, node: NodeId) -> bool {
        self.gvn.node(node).ty == Type::Double
    }

    fn emit_pure_gp(&mut self, node: NodeId) -> Result<(), BackendError> {
        let op = self.gvn.node(node).op.clone();
        let inputs = self.gvn.node(node).inputs.clone();
        match op {
            Opcode::NegI | Opcode::NegL => {
                let wide = matches!(op, Opcode::NegL);
                self.emit_value(inputs[0])?;
                self.emit_rex(wide, false, false, false);
                self.buf.write(&[0xf7, 0xd8])?; // neg rax/eax
                Ok(())
            }
            Opcode::AddI | Opcode::SubI | Opcode::MulI | Opcode::AndI | Opcode::OrI | Opcode::XorI
            | Opcode::AddL | Opcode::SubL | Opcode::MulL | Opcode::AndL | Opcode::OrL
            | Opcode::XorL | Opcode::LShiftI | Opcode::RShiftI | Opcode::URShiftI
            | Opcode::LShiftL | Opcode::RShiftL | Opcode::URShiftL
            | Opcode::CmpI | Opcode::CmpL | Opcode::BoolCmp(_) => {
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
                self.movsxd(Register::RAX, Register::RAX)
            }
            Opcode::L2I => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.emit_rex(false, false, false, false);
                self.buf.write(&[0x89, 0xc0])?; // mov eax,eax
                Ok(())
            }
            Opcode::I2B => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.emit_rex(false, false, false, false);
                self.buf.write(&[0x0f, 0xbe, 0xc0])?; // movsx eax,al
                Ok(())
            }
            Opcode::I2C => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.emit_rex(false, false, false, false);
                self.buf.write(&[0x0f, 0xb7, 0xc0])?; // movzx eax,ax
                Ok(())
            }
            Opcode::I2S => {
                self.emit_gp(inputs[0], Register::RAX)?;
                self.emit_rex(false, false, false, false);
                self.buf.write(&[0x0f, 0xbf, 0xc0])?; // movsx eax,ax
                Ok(())
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
            AddI | AddL => self.alu_rr(AluOp::Add, Register::RAX, Register::RCX, wide),
            SubI | SubL => self.alu_rr(AluOp::Sub, Register::RAX, Register::RCX, wide),
            MulI | MulL => self.alu_rr(AluOp::Mul, Register::RAX, Register::RCX, wide),
            AndI | AndL => self.alu_rr(AluOp::And, Register::RAX, Register::RCX, wide),
            OrI | OrL => self.alu_rr(AluOp::Or, Register::RAX, Register::RCX, wide),
            XorI | XorL => self.alu_rr(AluOp::Xor, Register::RAX, Register::RCX, wide),
            LShiftI | LShiftL => self.shift_op(ShiftOp::Shl, wide),
            RShiftI | RShiftL => self.shift_op(ShiftOp::Sar, wide),
            URShiftI | URShiftL => self.shift_op(ShiftOp::Shr, wide),
            CmpI | CmpL => {
                self.cmp_rr(Register::RAX, Register::RCX, wide)?;
                self.cmp_to_int()
            }
            BoolCmp(cond) => {
                self.cmp_rr(Register::RAX, Register::RCX, wide)?;
                self.setcc_bool(*cond)
            }
            _ => Err(BackendError::Unsupported(format!("gp op {:?}", op))),
        }
    }

    fn emit_pure_xmm(&mut self, node: NodeId) -> Result<(), BackendError> {
        let op = self.gvn.node(node).op.clone();
        let inputs = self.gvn.node(node).inputs.clone();
        match op {
            Opcode::AddF | Opcode::SubF | Opcode::MulF | Opcode::DivF
            | Opcode::AddD | Opcode::SubD | Opcode::MulD | Opcode::DivD => {
                let double = self.is_double_node(node);
                let alu = match op {
                    Opcode::AddF | Opcode::AddD => AluOp::Add,
                    Opcode::SubF | Opcode::SubD => AluOp::Sub,
                    Opcode::MulF | Opcode::MulD => AluOp::Mul,
                    _ => AluOp::Div,
                };
                let t = self.alloc_temp()?;
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                self.sse_store_mem(XMMRegister::XMM0, Register::FP, self.temp_disp(t), double)?;
                self.emit_xmm_to(inputs[1], XMMRegister::XMM0)?;
                self.sse_load_mem(XMMRegister::XMM1, Register::FP, self.temp_disp(t), double)?;
                self.free_temp();
                self.sse_arith(alu, double)
            }
            Opcode::NegF | Opcode::NegD => {
                let double = matches!(op, Opcode::NegD);
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                if double {
                    self.mov_imm(Register::RAX, 0x8000_0000_0000_0000u64 as i64)?;
                    self.movq_xmm_from_gp(XMMRegister::XMM1, Register::RAX)?;
                    self.emit_rex(true, false, false, false);
                    self.buf.write(&[0x66, 0x0f, 0x57, 0xc1])?; // xorpd xmm0,xmm1
                } else {
                    self.mov_imm32(Register::RAX, 0x8000_0000u32 as i32)?;
                    self.movd_xmm_from_gp(XMMRegister::XMM1, Register::RAX)?;
                    self.buf.write(&[0x0f, 0x57, 0xc1])?; // xorps xmm0,xmm1
                }
                Ok(())
            }
            Opcode::I2F | Opcode::L2F => {
                let wide = matches!(op, Opcode::L2F);
                self.emit_gp(inputs[0], Register::RAX)?;
                self.cvtsi2ss(XMMRegister::XMM0, Register::RAX, wide)
            }
            Opcode::I2D | Opcode::L2D => {
                let wide = matches!(op, Opcode::L2D);
                self.emit_gp(inputs[0], Register::RAX)?;
                self.cvtsi2sd(XMMRegister::XMM0, Register::RAX, wide)
            }
            Opcode::F2D => {
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                self.cvtss2sd(XMMRegister::XMM0, XMMRegister::XMM0)
            }
            Opcode::D2F => {
                self.emit_xmm_to(inputs[0], XMMRegister::XMM0)?;
                self.cvtsd2ss(XMMRegister::XMM0, XMMRegister::XMM0)
            }
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
        self.emit_xmm_to(a, XMMRegister::XMM0)?;
        self.sse_store_mem(XMMRegister::XMM0, Register::FP, self.temp_disp(t), double)?;
        self.emit_xmm_to(b, XMMRegister::XMM0)?;
        self.sse_load_mem(XMMRegister::XMM1, Register::FP, self.temp_disp(t), double)?;
        self.free_temp();
        // ucomiss/ucomisd xmm0,xmm1
        self.emit_rex(double, false, false, false);
        self.buf.write(&[if double { 0x66 } else { 0x0f }, 0x2e, 0xc1])?;
        // NaN -> greater_nan ? 1 : -1 ; else CF->-1, ZF->0, else 1
        let start = self.pos();
        let nan_jp = self.emit_jmp_local_cc(XCond::P)?;
        let less_jb = self.emit_jmp_local_cc(XCond::B)?;
        let eq_je = self.emit_jmp_local_cc(XCond::E)?;
        self.mov_imm32(Register::RAX, 1)?;
        let end_jmp = self.emit_jmp_local()?;
        self.patch_local(less_jb, self.pos())?;
        self.mov_imm32(Register::RAX, -1)?;
        let end2 = self.emit_jmp_local()?;
        self.patch_local(eq_je, self.pos())?;
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x31, 0xc0])?; // xor eax,eax
        let end3 = self.emit_jmp_local()?;
        self.patch_local(nan_jp, self.pos())?;
        self.mov_imm32(Register::RAX, if greater_nan { 1 } else { -1 })?;
        self.patch_local(end_jmp, self.pos())?;
        self.patch_local(end2, self.pos())?;
        self.patch_local(end3, self.pos())?;
        let _ = start;
        Ok(())
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
            self.sse_store_mem(*r, Register::FP, disp, true)?;
        }
        self.emit_abs_call(addr)?;
        self.pc_descs.push(PcDesc::new(self.pos() as i32, 0, PcDesc::PCDESC_REEXECUTE));
        for r in &xmm_locs {
            let slot = self.alloc.xmm_save_slot(*r);
            let disp = self.slot_disp(slot);
            self.sse_load_mem(*r, Register::FP, disp, true)?;
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
                return Err(BackendError::InvalidGraph(format!("missing block {}", p.target)));
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

    // ---- low-level encoders --------------------------------------------------

    fn emit_rex(&mut self, w: bool, r: bool, x: bool, b: bool) {
        let bits = ((w as u8) << 3) | ((r as u8) << 2) | ((x as u8) << 1) | b as u8;
        if bits != 0 {
            let _ = self.buf.write_byte(0x40 | bits);
        }
    }

    fn rex_bits(&self, reg: Register, rm: Register, w: bool) -> (bool, bool, bool, bool) {
        (w, reg.needs_rex(), false, rm.needs_rex())
    }

    fn emit_modrm_sib(
        &mut self,
        reg: u8,
        base: Option<Register>,
        index: Option<Register>,
        scale: Scale,
        disp: i32,
    ) -> Result<(), BackendError> {
        let base_enc = base.map(|b| b.encoding() & 7).unwrap_or(5);
        let index_enc = index.map(|i| i.encoding() & 7).unwrap_or(4);
        let need_sib = index.is_some()
            || matches!(base, Some(Register::RSP) | Some(Register::R12))
            || base.is_none();
        let rbp_base = matches!(base, Some(Register::RBP) | Some(Register::R13));
        let use_disp32 = base.is_none() || disp < -128 || disp > 127 || (rbp_base && disp == 0);
        let mod_bits = if use_disp32 {
            2
        } else if disp == 0 && !rbp_base {
            0
        } else {
            1
        };
        let rm = if need_sib { 4 } else { base_enc };
        let modrm = (mod_bits << 6) | ((reg & 7) << 3) | rm;
        self.buf.write_byte(modrm)?;
        if need_sib {
            let sib = (scale.encoding() << 6) | ((index_enc & 7) << 3) | (base_enc & 7);
            self.buf.write_byte(sib)?;
        }
        if use_disp32 {
            self.buf.write_i32(disp)?;
        } else if mod_bits == 1 {
            self.buf.write_byte(disp as i8 as u8)?;
        }
        Ok(())
    }

    fn rex_mem(&self, reg: u8, base: Option<Register>, index: Option<Register>, w: bool) -> (bool, bool, bool) {
        (
            w,
            reg >= 8,
            base.map(|b| b.needs_rex()).unwrap_or(false)
                || index.map(|i| i.needs_rex()).unwrap_or(false),
        )
    }

    fn mov_imm(&mut self, reg: Register, imm: i64) -> Result<(), BackendError> {
        self.emit_rex(true, false, false, reg.needs_rex());
        self.buf.write_byte(0xb8 + (reg.encoding() & 7))?;
        self.buf.write_i64(imm)?;
        Ok(())
    }

    fn mov_imm32(&mut self, reg: Register, imm: i32) -> Result<(), BackendError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write_byte(0xb8 + (reg.encoding() & 7))?;
        self.buf.write_i32(imm)?;
        Ok(())
    }

    fn mov_reg_reg(&mut self, dst: Register, src: Register, wide: bool) -> Result<(), BackendError> {
        let (w, r, _, b) = self.rex_bits(dst, src, wide);
        self.emit_rex(w, r, false, b);
        self.buf.write(&[0x8b, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    fn movsxd(&mut self, dst: Register, src: Register) -> Result<(), BackendError> {
        self.emit_rex(true, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0x63, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    fn mov_reg_mem(&mut self, reg: Register, base: Option<Register>, index: Option<Register>, scale: Scale, disp: i32, wide: bool) -> Result<(), BackendError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), base, index, wide);
        self.emit_rex(w, r, false, b);
        self.buf.write_byte(0x8b)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    fn mov_mem_reg(&mut self, base: Option<Register>, index: Option<Register>, scale: Scale, disp: i32, reg: Register, wide: bool) -> Result<(), BackendError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), base, index, wide);
        self.emit_rex(w, r, false, b);
        self.buf.write_byte(0x89)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    fn movsx_mem_reg(&mut self, reg: Register, base: Register, disp: i32, word: bool) -> Result<(), BackendError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), Some(base), None, true);
        self.emit_rex(w, r, false, b);
        self.buf.write(&[0x0f, if word { 0xbf } else { 0xbe }])?;
        self.emit_modrm_sib(reg.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    fn movzx_mem_reg(&mut self, reg: Register, base: Register, disp: i32, word: bool) -> Result<(), BackendError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), Some(base), None, true);
        self.emit_rex(w, r, false, b);
        self.buf.write(&[0x0f, if word { 0xb7 } else { 0xb6 }])?;
        self.emit_modrm_sib(reg.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    fn cmp_rr(&mut self, a: Register, b: Register, wide: bool) -> Result<(), BackendError> {
        let (w, r, _, bb) = self.rex_bits(b, a, wide);
        self.emit_rex(w, r, false, bb);
        self.buf.write(&[0x39, 0xc0 | ((b.encoding() & 7) << 3) | (a.encoding() & 7)])?;
        Ok(())
    }

    fn alu_rr(&mut self, alu: AluOp, dst: Register, src: Register, wide: bool) -> Result<(), BackendError> {
        let (w, r, _, b) = self.rex_bits(dst, src, wide);
        self.emit_rex(w, r, false, b);
        match alu {
            AluOp::Mul => {
                self.buf.write(&[0x0f, 0xaf, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
            }
            _ => {
                let op = match alu {
                    AluOp::Add => 0x03,
                    AluOp::Sub => 0x2b,
                    AluOp::And => 0x23,
                    AluOp::Or => 0x0b,
                    AluOp::Xor => 0x33,
                    AluOp::Div | AluOp::Mul => unreachable!(),
                };
                self.buf.write(&[op, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
            }
        }
        Ok(())
    }

    fn shift_op(&mut self, shift: ShiftOp, wide: bool) -> Result<(), BackendError> {
        let ext = match shift {
            ShiftOp::Shl => 4,
            ShiftOp::Shr => 5,
            ShiftOp::Sar => 7,
        };
        self.emit_rex(wide, false, false, false);
        self.buf.write(&[0xd3, 0xc0 | (ext << 3) | (Register::RAX.encoding() & 7)])?;
        Ok(())
    }

    fn cmp_to_int(&mut self) -> Result<(), BackendError> {
        // result = (a > b) - (a < b) in {-1,0,1}
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x45, 0x31, 0xc0])?; // xor r8d,r8d
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x45, 0x31, 0xc9])?; // xor r9d,r9d
        self.emit_setcc(XCond::L, Register::R8)?;
        self.emit_setcc(XCond::G, Register::R9)?;
        self.movzx_reg(Register::R8)?;
        self.movzx_reg(Register::R9)?;
        self.mov_reg_reg(Register::RAX, Register::R9, false)?;
        self.alu_rr(AluOp::Sub, Register::RAX, Register::R8, false)
    }

    fn setcc_bool(&mut self, cond: CmpCondition) -> Result<(), BackendError> {
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x31, 0xc0])?; // xor eax,eax
        let cc = self.xcond_for(cond);
        self.emit_setcc(cc, Register::RAX)?;
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x0f, 0xb6, 0xc0])?; // movzx eax,al
        Ok(())
    }

    fn emit_setcc(&mut self, cond: XCond, reg: Register) -> Result<(), BackendError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write(&[0x0f, 0x90 | cond as u8, 0xc0 | (reg.encoding() & 7)])?;
        Ok(())
    }

    fn movzx_reg(&mut self, reg: Register) -> Result<(), BackendError> {
        self.emit_rex(false, reg.needs_rex(), false, reg.needs_rex());
        self.buf.write(&[0x0f, 0xb6, 0xc0 | (reg.encoding() & 7)])?;
        Ok(())
    }

    fn emit_abs_call(&mut self, addr: u64) -> Result<(), BackendError> {
        self.mov_imm(Register::RAX, addr as i64)?;
        self.buf.write(&[0xff, 0xd0])?; // call rax
        Ok(())
    }

    fn emit_push(&mut self, reg: Register) -> Result<(), BackendError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write_byte(0x50 + (reg.encoding() & 7))?;
        Ok(())
    }

    fn emit_pop(&mut self, reg: Register) -> Result<(), BackendError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write_byte(0x58 + (reg.encoding() & 7))?;
        Ok(())
    }

    fn store_gp_to_temp(&mut self, idx: usize, reg: Register, wide: bool) -> Result<(), BackendError> {
        self.mov_mem_reg(Some(Register::FP), None, Scale::S1, self.temp_disp(idx), reg, wide)
    }

    fn load_gp_from_temp(&mut self, idx: usize, reg: Register, wide: bool) -> Result<(), BackendError> {
        self.mov_reg_mem(reg, Some(Register::FP), None, Scale::S1, self.temp_disp(idx), wide)
    }

    fn const_i32(&self, node: NodeId) -> Result<i32, BackendError> {
        match self.gvn.node(node).op {
            Opcode::ConI(v) => Ok(v),
            Opcode::ConL(v) => Ok(v as i32),
            _ => Err(BackendError::Unsupported(format!(
                "node {} is not a constant offset",
                node
            ))),
        }
    }

    // ---- SSE ----------------------------------------------------------------

    fn mov_xmm_xmm(&mut self, dst: XMMRegister, src: XMMRegister, double: bool) -> Result<(), BackendError> {
        self.emit_rex(double, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[
            if double { 0xf2 } else { 0xf3 },
            0x0f,
            0x10,
            0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7),
        ])?;
        Ok(())
    }

    fn movd_xmm_from_gp(&mut self, xmm: XMMRegister, gp: Register) -> Result<(), BackendError> {
        self.emit_rex(false, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x66, 0x0f, 0x6e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn movq_xmm_from_gp(&mut self, xmm: XMMRegister, gp: Register) -> Result<(), BackendError> {
        self.emit_rex(true, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x66, 0x0f, 0x6e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn cvtsi2ss(&mut self, xmm: XMMRegister, gp: Register, wide: bool) -> Result<(), BackendError> {
        self.emit_rex(wide, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0xf3, 0x0f, 0x2a, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn cvtsi2sd(&mut self, xmm: XMMRegister, gp: Register, wide: bool) -> Result<(), BackendError> {
        self.emit_rex(wide, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0xf2, 0x0f, 0x2a, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn cvtss2sd(&mut self, dst: XMMRegister, src: XMMRegister) -> Result<(), BackendError> {
        self.emit_rex(false, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0xf3, 0x0f, 0x5a, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    fn cvtsd2ss(&mut self, dst: XMMRegister, src: XMMRegister) -> Result<(), BackendError> {
        self.emit_rex(false, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0xf2, 0x0f, 0x5a, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    fn sse_load_mem(&mut self, xmm: XMMRegister, base: Register, disp: i32, double: bool) -> Result<(), BackendError> {
        self.emit_rex(double, xmm.needs_rex(), false, base.needs_rex());
        self.buf.write(&[if double { 0xf2 } else { 0xf3 }, 0x0f, 0x10])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    fn sse_store_mem(&mut self, xmm: XMMRegister, base: Register, disp: i32, double: bool) -> Result<(), BackendError> {
        self.emit_rex(double, xmm.needs_rex(), false, base.needs_rex());
        self.buf.write(&[if double { 0xf2 } else { 0xf3 }, 0x0f, 0x11])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    fn sse_arith(&mut self, alu: AluOp, double: bool) -> Result<(), BackendError> {
        let (prefix, op) = match (alu, double) {
            (AluOp::Add, false) => (0xf3u8, 0x58u8),
            (AluOp::Add, true) => (0xf2, 0x58),
            (AluOp::Sub, false) => (0xf3, 0x5c),
            (AluOp::Sub, true) => (0xf2, 0x5c),
            (AluOp::Mul, false) => (0xf3, 0x59),
            (AluOp::Mul, true) => (0xf2, 0x59),
            (AluOp::Div, false) => (0xf3, 0x5e),
            (AluOp::Div, true) => (0xf2, 0x5e),
            _ => return Err(BackendError::Unsupported("sse arith".into())),
        };
        self.emit_rex(double, false, false, XMMRegister::XMM1.needs_rex());
        self.buf.write(&[prefix, 0x0f, op, 0xc1])?; // xmm0 = xmm0 op xmm1
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum AluOp {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy)]
enum ShiftOp {
    Shl,
    Shr,
    Sar,
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
        let bad = match n.op {
            Opcode::LoadP | Opcode::StoreP | Opcode::New(_) | Opcode::NewArray(_)
            | Opcode::NewArrayRef(_) | Opcode::Call(_) | Opcode::CheckCast(_)
            | Opcode::InstanceOf(_) | Opcode::MonitorEnter | Opcode::MonitorExit
            | Opcode::Athrow | Opcode::IfException | Opcode::ArrayLength | Opcode::Proj(_) => true,
            Opcode::LoadI | Opcode::LoadL | Opcode::LoadF | Opcode::LoadD | Opcode::LoadB
            | Opcode::LoadC | Opcode::LoadS | Opcode::StoreI | Opcode::StoreL
            | Opcode::StoreF | Opcode::StoreD | Opcode::StoreB | Opcode::StoreC | Opcode::StoreS => {
                // Field/array accesses require null/bounds checks and oop
                // maps; keep the first executable tier to local computation.
                true
            }
            Opcode::DivI | Opcode::RemI | Opcode::DivL | Opcode::RemL | Opcode::RemF | Opcode::RemD => true,
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
