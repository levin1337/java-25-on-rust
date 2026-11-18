use super::codeBuffer::{CodeBuffer, CodeBufferError};
use super::register::{Condition as x86Condition, Register, Scale, XMMRegister};
use crate::share::c1::c1_IR::ControlFlowGraph;
use crate::share::c1::c1_LIR::{LirKind, LirOperand, LirOp, LirProgram, RuntimeStub};
use crate::share::c1::c1_Instruction::{ArrayElemKind, CompareKind, Condition, ConvertKind, ValueKind};
use crate::share::oops::objLayout::ObjLayoutMode;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abi {
    SystemV,
    WindowsX64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamInfo {
    pub local_slot: usize,
    pub kind: LirKind,
}

#[derive(Clone, Debug)]
pub struct BackendOptions {
    pub abi: Abi,
    pub stubs: BTreeMap<RuntimeStub, u64>,
    pub emit_safepoints: bool,
    pub params: Vec<ParamInfo>,
    pub obj_layout: ObjLayoutMode,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self { abi: Abi::SystemV, stubs: BTreeMap::new(), emit_safepoints: false, params: Vec::new(), obj_layout: ObjLayoutMode::Uncompressed }
    }
}

#[derive(Debug)]
pub enum BackendError {
    CodeBuffer(CodeBufferError),
    Unsupported(String),
    MissingRuntimeStub(RuntimeStub),
    MissingCallTarget,
    InvalidLir,
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodeBuffer(e) => write!(f, "code buffer: {}", e),
            Self::Unsupported(s) => write!(f, "unsupported: {}", s),
            Self::MissingRuntimeStub(s) => write!(f, "missing runtime stub {:?}", s),
            Self::MissingCallTarget => write!(f, "missing call target"),
            Self::InvalidLir => write!(f, "invalid LIR"),
        }
    }
}

impl std::error::Error for BackendError {}
impl From<CodeBufferError> for BackendError {
    fn from(e: CodeBufferError) -> Self { Self::CodeBuffer(e) }
}

#[derive(Clone, Debug)]
pub struct PcDescEntry {
    pub pc_offset: usize,
    pub bci: i32,
    pub is_safepoint: bool,
}

#[derive(Clone, Debug)]
pub struct EmitResult {
    pub pc_descs: Vec<PcDescEntry>,
    pub frame_size: usize,
    pub frame_complete_offset: usize,
    pub code_size: usize,
    pub block_offsets: BTreeMap<usize, usize>,
}

#[derive(Clone, Copy)]
enum Patch {
    Jmp { disp: usize, target: usize },
    Jcc { disp: usize, cond: x86Condition, target: usize },
    LocalJmp { disp: usize, target_marker: usize },
}

pub struct X86Emitter<'a> {
    buf: &'a CodeBuffer,
    program: &'a LirProgram,
    options: &'a BackendOptions,
    patches: Vec<Patch>,
    local_patches: Vec<(usize, usize)>,
    table_patches: Vec<(usize, usize, usize)>,
    table_lea: Vec<(usize, usize)>,
    block_offsets: BTreeMap<usize, usize>,
    pc_descs: Vec<PcDescEntry>,
    frame_size: usize,
}

impl<'a> X86Emitter<'a> {
    pub fn new(buf: &'a CodeBuffer, program: &'a LirProgram, options: &'a BackendOptions) -> Self {
        let frame_bytes = program.frame_slots * 8;
        let frame_size = frame_bytes.div_ceil(16) * 16;
        Self {
            buf,
            program,
            options,
            patches: Vec::new(),
            local_patches: Vec::new(),
            table_patches: Vec::new(),
            table_lea: Vec::new(),
            block_offsets: BTreeMap::new(),
            pc_descs: Vec::new(),
            frame_size,
        }
    }

    pub fn emit(&mut self) -> Result<EmitResult, BackendError> {
        self.emit_prologue()?;
        let frame_complete_offset = self.buf.position();
        for block in &self.program.blocks {
            self.block_offsets.insert(block.id, self.buf.position());
            for inst in &block.instructions {
                self.emit_lir(inst)?;
            }
        }
        self.buf.write(&[0x0f, 0x0b])?;
        self.patch_all()?;
        Ok(EmitResult {
            pc_descs: self.pc_descs.clone(),
            frame_size: self.frame_size,
            frame_complete_offset,
            code_size: self.buf.position(),
            block_offsets: self.block_offsets.clone(),
        })
    }

    fn pos(&self) -> usize { self.buf.position() }

    fn emit_prologue(&mut self) -> Result<(), BackendError> {
        self.buf.write_byte(0x55)?;
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x89, 0xe5])?;
        if self.frame_size > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x81, 0xec])?;
            self.buf.write_i32(self.frame_size as i32)?;
        }
        self.emit_params()?;
        Ok(())
    }

    fn emit_params(&mut self) -> Result<(), BackendError> {
        // Windows x64: argument *position* i selects the register —
        // xmm0-3 for float/double, rcx/rdx/r8/r9 for everything else —
        // and positions >= 4 spill to the stack above the 32-byte
        // shadow space at [rbp+48].  System V keeps separate gp/xmm
        // counters (rdi/rsi/rdx/rcx/r8/r9 and xmm0-7) with stack args
        // at [rbp+16] in declaration order.
        let win64 = self.options.abi == Abi::WindowsX64;
        let gp_regs: &[Register] = if win64 {
            &[Register::RCX, Register::RDX, Register::R8, Register::R9]
        } else {
            &[Register::RDI, Register::RSI, Register::RDX, Register::RCX, Register::R8, Register::R9]
        };
        let stack_base: i32 = if win64 { 48 } else { 16 };
        let (mut gp_idx, mut xmm_idx, mut stack_idx) = (0usize, 0usize, 0usize);
        for (i, param) in self.options.params.iter().enumerate() {
            let off = self.local_offset(param.local_slot);
            let is_float = matches!(param.kind, LirKind::Float32 | LirKind::Float64);
            let is_double = param.kind == LirKind::Float64;
            if win64 {
                if i < 4 {
                    if is_float {
                        let xmm = XMMRegister::from_encoding(i as u8)
                            .ok_or(BackendError::InvalidLir)?;
                        self.sse_store_mem(xmm, Register::FP, -(off as i32), is_double)?;
                        if param.kind == LirKind::Float32 {
                            self.zero_slot_upper(off)?;
                        }
                    } else {
                        self.mov_mem_reg(Some(Register::FP), None, Scale::S1, -(off as i32), gp_regs[i], true)?;
                    }
                } else {
                    // Stack-passed argument: raw 8-byte slot, bit pattern
                    // preserved for both integer and floating-point args.
                    let stack_off = stack_base + ((i - 4) * 8) as i32;
                    self.mov_reg_mem(Register::RAX, Some(Register::FP), None, Scale::S1, stack_off, true)?;
                    self.mov_mem_reg(Some(Register::FP), None, Scale::S1, -(off as i32), Register::RAX, true)?;
                    if param.kind == LirKind::Float32 {
                        self.zero_slot_upper(off)?;
                    }
                }
            } else {
                if is_float {
                    if xmm_idx < 8 {
                        let xmm = XMMRegister::from_encoding(xmm_idx as u8)
                            .ok_or(BackendError::InvalidLir)?;
                        self.sse_store_mem(xmm, Register::FP, -(off as i32), is_double)?;
                        if param.kind == LirKind::Float32 {
                            self.zero_slot_upper(off)?;
                        }
                    } else {
                        let stack_off = stack_base + (stack_idx * 8) as i32;
                        self.mov_reg_mem(Register::RAX, Some(Register::FP), None, Scale::S1, stack_off, true)?;
                        self.mov_mem_reg(Some(Register::FP), None, Scale::S1, -(off as i32), Register::RAX, true)?;
                        if param.kind == LirKind::Float32 {
                            self.zero_slot_upper(off)?;
                        }
                    }
                    xmm_idx += 1;
                } else {
                    if gp_idx < 6 {
                        self.mov_mem_reg(Some(Register::FP), None, Scale::S1, -(off as i32), gp_regs[gp_idx], true)?;
                    } else {
                        let stack_off = stack_base + (stack_idx * 8) as i32;
                        self.mov_reg_mem(Register::RAX, Some(Register::FP), None, Scale::S1, stack_off, true)?;
                        self.mov_mem_reg(Some(Register::FP), None, Scale::S1, -(off as i32), Register::RAX, true)?;
                    }
                    gp_idx += 1;
                }
                if (is_float && xmm_idx > 8) || (!is_float && gp_idx > 6) {
                    stack_idx += 1;
                }
            }
        }
        Ok(())
    }

    /// Zeroes the upper 4 bytes of an 8-byte frame slot that holds a
    /// Float32 value (slot layout: low 4 bytes = value, high 4 = pad).
    fn zero_slot_upper(&mut self, off: usize) -> Result<(), BackendError> {
        let disp = -(off as i32) + 4;
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0xc7, 0x85])?; // mov dword [rbp+disp32], imm32
        self.buf.write_i32(disp)?;
        self.buf.write_i32(0)?;
        Ok(())
    }

    fn local_offset(&self, slot: usize) -> usize {
        8 + slot * 8
    }

    fn stack_offset(&self, slot: usize) -> usize {
        8 + (self.program.local_count + slot) * 8
    }

    fn temp_offset(&self, slot: usize) -> usize {
        8 + (self.program.local_count + self.program.stack_count + slot) * 8
    }

    fn operand_offset(&self, op: &LirOperand) -> Option<(bool, usize)> {
        match op {
            LirOperand::Local { slot, .. } => Some((true, self.local_offset(*slot))),
            LirOperand::Stack { slot, .. } => Some((true, self.stack_offset(*slot))),
            LirOperand::Temp { slot, .. } => Some((true, self.temp_offset(*slot))),
            _ => None,
        }
    }

    fn frame_addr(&self, op: &LirOperand) -> Option<(usize, LirKind)> {
        match op {
            LirOperand::Local { slot, kind } => Some((self.local_offset(*slot), *kind)),
            LirOperand::Stack { slot, kind } => Some((self.stack_offset(*slot), *kind)),
            LirOperand::Temp { slot, kind } => Some((self.temp_offset(*slot), *kind)),
            _ => None,
        }
    }

    fn emit_lir(&mut self, inst: &LirOp) -> Result<(), BackendError> {
        match inst {
            LirOp::Nop => {}
            LirOp::LoadLocal { dst, slot, kind } | LirOp::LoadStack { dst, slot, kind } => {
                let _ = (dst, slot, kind);
            }
            LirOp::StoreLocal { slot, src, kind } => self.emit_store_frame(true, *slot, src, *kind)?,
            LirOp::StoreStack { slot, src, kind } => self.emit_store_frame(false, *slot, src, *kind)?,
            LirOp::Move { dst, src, kind } => {
                if let Some((off, k)) = self.frame_addr(dst) {
                    self.emit_operand_to_reg(src, Register::RAX)?;
                    self.store_gp_at(off, k, Register::RAX)?;
                }
            }
            LirOp::Add { dst, lhs, rhs, kind } => self.emit_int_binary(*kind, lhs, rhs, Alu::Add, dst)?,
            LirOp::Sub { dst, lhs, rhs, kind } => self.emit_int_binary(*kind, lhs, rhs, Alu::Sub, dst)?,
            LirOp::Mul { dst, lhs, rhs, kind } => self.emit_int_binary(*kind, lhs, rhs, Alu::Mul, dst)?,
            LirOp::Div { dst, lhs, rhs, kind } => self.emit_div_rem(*kind, lhs, rhs, dst, true)?,
            LirOp::Rem { dst, lhs, rhs, kind } => self.emit_div_rem(*kind, lhs, rhs, dst, false)?,
            LirOp::And { dst, lhs, rhs, kind } => self.emit_int_binary(*kind, lhs, rhs, Alu::And, dst)?,
            LirOp::Or { dst, lhs, rhs, kind } => self.emit_int_binary(*kind, lhs, rhs, Alu::Or, dst)?,
            LirOp::Xor { dst, lhs, rhs, kind } => self.emit_int_binary(*kind, lhs, rhs, Alu::Xor, dst)?,
            LirOp::Shl { dst, lhs, rhs, kind } => self.emit_shift(*kind, lhs, rhs, Shift::Shl, dst)?,
            LirOp::Shr { dst, lhs, rhs, kind } => self.emit_shift(*kind, lhs, rhs, Shift::Sar, dst)?,
            LirOp::Ushr { dst, lhs, rhs, kind } => self.emit_shift(*kind, lhs, rhs, Shift::Shr, dst)?,
            LirOp::Neg { dst, src, kind } => self.emit_neg(*kind, src, dst)?,
            LirOp::Convert { dst, src, kind } => self.emit_convert(*kind, src, dst)?,
            LirOp::CompareValue { dst, lhs, rhs, kind } => self.emit_compare_value(*kind, lhs, rhs, dst)?,
            LirOp::Compare { lhs, rhs, kind } => self.emit_compare(*kind, lhs, rhs)?,
            LirOp::LoadField { dst, obj, offset, kind } => self.emit_load_field(obj, *offset, *kind, dst)?,
            LirOp::StoreField { obj, offset, value, kind } => self.emit_store_field(obj, *offset, value, *kind)?,
            LirOp::LoadIndexed { dst, array, index, elem } => self.emit_load_indexed(array, index, *elem, dst)?,
            LirOp::StoreIndexed { array, index, value, elem } => self.emit_store_indexed(array, index, value, *elem)?,
            LirOp::ArrayLength { dst, array } => self.emit_array_length(array, dst)?,
            LirOp::Call { dst, operands, info } => self.emit_call(dst.as_ref(), operands, info)?,
            LirOp::Return { value, kind } => self.emit_return(value.as_ref(), *kind)?,
            LirOp::Goto { target } => self.emit_jmp(*target)?,
            LirOp::Branch { cond, target_true, target_false } => self.emit_branch(*cond, *target_true, *target_false)?,
            LirOp::TableSwitch { index, low, high, default_block, targets } => self.emit_table_switch(index, *low, *high, *default_block, targets)?,
            LirOp::LookupSwitch { key, pairs, default_block } => self.emit_lookup_switch(key, pairs, *default_block)?,
            LirOp::Safepoint { bci } => self.emit_safepoint(*bci)?,
            LirOp::CallRuntime { stub, operands, dst, bci } => self.emit_call_runtime(*stub, operands, dst.as_ref(), *bci)?,
            LirOp::Unsupported { opcode, bci, reason } => {
                return Err(BackendError::Unsupported(format!("bytecode {:02x} at {}: {}", opcode, bci, reason)));
            }
        }
        Ok(())
    }

    fn emit_int_binary(&mut self, kind: LirKind, lhs: &LirOperand, rhs: &LirOperand, alu: Alu, dst: &LirOperand) -> Result<(), BackendError> {
        if matches!(kind, LirKind::Float32 | LirKind::Float64) {
            return self.emit_float_binary(kind, lhs, rhs, alu, dst);
        }
        let wide = kind == LirKind::Int64 || kind == LirKind::Ref;
        self.emit_operand_to_reg(lhs, Register::RAX)?;
        self.emit_operand_to_reg(rhs, Register::RCX)?;
        if wide {
            self.alu_rr(alu, Register::RAX, Register::RCX, true)?;
        } else {
            self.alu_rr(alu, Register::RAX, Register::RCX, false)?;
        }
        self.store_gp_result(dst, Register::RAX)
    }

    fn emit_float_binary(&mut self, kind: LirKind, lhs: &LirOperand, rhs: &LirOperand, alu: Alu, dst: &LirOperand) -> Result<(), BackendError> {
        if !matches!(alu, Alu::Add | Alu::Sub | Alu::Mul | Alu::Div) {
            return Err(BackendError::Unsupported("float rem/logic".to_string()));
        }
        self.emit_operand_to_xmm(lhs, XMMRegister::XMM0)?;
        self.emit_operand_to_xmm(rhs, XMMRegister::XMM1)?;
        let double = kind == LirKind::Float64;
        self.sse_arith(alu, double)?;
        self.store_xmm_result(dst, XMMRegister::XMM0)
    }

    fn emit_div_rem(&mut self, kind: LirKind, lhs: &LirOperand, rhs: &LirOperand, dst: &LirOperand, is_div: bool) -> Result<(), BackendError> {
        match kind {
            LirKind::Float32 | LirKind::Float64 => {
                if !is_div { return Err(BackendError::Unsupported("float rem".to_string())); }
                return self.emit_float_binary(kind, lhs, rhs, Alu::Div, dst);
            }
            _ => {}
        }
        let wide = kind == LirKind::Int64 || kind == LirKind::Ref;
        self.emit_operand_to_reg(lhs, Register::RAX)?;
        self.emit_operand_to_reg(rhs, Register::RCX)?;
        self.emit_runtime_check_zero(Register::RCX)?;
        self.emit_div_overflow_check(wide)?;
        if wide { self.buf.write(&[0x48, 0x99])?; } else { self.buf.write(&[0x99])?; }
        self.buf.write(&[0xf7, 0xf9])?;
        if is_div {
            self.store_gp_result(dst, Register::RAX)
        } else {
            self.store_gp_result(dst, Register::RDX)
        }
    }

    fn emit_shift(&mut self, kind: LirKind, lhs: &LirOperand, rhs: &LirOperand, shift: Shift, dst: &LirOperand) -> Result<(), BackendError> {
        let wide = kind == LirKind::Int64;
        self.emit_operand_to_reg(lhs, Register::RAX)?;
        self.emit_operand_to_reg(rhs, Register::RCX)?;
        self.shift_rr(shift, Register::RAX, true, wide)?;
        self.store_gp_result(dst, Register::RAX)
    }

    fn emit_neg(&mut self, kind: LirKind, src: &LirOperand, dst: &LirOperand) -> Result<(), BackendError> {
        match kind {
            LirKind::Float32 => {
                self.emit_operand_to_xmm(src, XMMRegister::XMM0)?;
                self.mov_reg_imm(Register::RAX, 0x8000_0000, false)?;
                self.movd_xmm_from_gp(XMMRegister::XMM1, Register::RAX, false)?;
                self.buf.write(&[0x0f, 0x57, 0xc1])?;
                return self.store_xmm_result(dst, XMMRegister::XMM0);
            }
            LirKind::Float64 => {
                self.emit_operand_to_xmm(src, XMMRegister::XMM0)?;
                self.mov_reg_imm(Register::RAX, 0x8000_0000_0000_0000, true)?;
                self.movq_xmm_from_gp(XMMRegister::XMM1, Register::RAX)?;
                self.buf.write(&[0x66, 0x0f, 0x57, 0xc1])?;
                return self.store_xmm_result(dst, XMMRegister::XMM0);
            }
            _ => {
                let wide = kind == LirKind::Int64;
                self.emit_operand_to_reg(src, Register::RAX)?;
                self.emit_rex(wide, false, false, false);
                self.buf.write(&[0xf7, 0xd8])?;
                return self.store_gp_result(dst, Register::RAX);
            }
        }
    }

    fn emit_convert(&mut self, kind: ConvertKind, src: &LirOperand, dst: &LirOperand) -> Result<(), BackendError> {
        let src_kind = src.kind();
        let dst_kind = dst.kind();
        // The source register file is chosen by the SOURCE kind, not the
        // destination: float->int conversions (F2I/D2L/...) must load the
        // operand into xmm0 even though the result goes to rax.
        let src_is_float = matches!(src_kind, LirKind::Float32 | LirKind::Float64);
        if src_is_float {
            self.emit_operand_to_xmm(src, XMMRegister::XMM0)?;
        } else {
            self.emit_operand_to_reg(src, Register::RAX)?;
        }
        match kind {
            ConvertKind::I2L => { self.movsxd(Register::RAX, Register::RAX)?; self.store_gp_result(dst, Register::RAX)?; }
            ConvertKind::L2I => { self.mov_reg_reg(Register::RAX, Register::RAX, false)?; self.store_gp_result(dst, Register::RAX)?; }
            ConvertKind::I2F => { self.cvtsi2ss(XMMRegister::XMM0, Register::RAX, false)?; self.store_xmm_result(dst, XMMRegister::XMM0)?; }
            ConvertKind::I2D => { self.cvtsi2sd(XMMRegister::XMM0, Register::RAX, false)?; self.store_xmm_result(dst, XMMRegister::XMM0)?; }
            ConvertKind::L2F => { self.cvtsi2ss(XMMRegister::XMM0, Register::RAX, true)?; self.store_xmm_result(dst, XMMRegister::XMM0)?; }
            ConvertKind::L2D => { self.cvtsi2sd(XMMRegister::XMM0, Register::RAX, true)?; self.store_xmm_result(dst, XMMRegister::XMM0)?; }
            ConvertKind::F2I => { self.cvttss2si(Register::RAX, XMMRegister::XMM0, false)?; self.store_gp_result(dst, Register::RAX)?; }
            ConvertKind::F2L => { self.cvttss2si(Register::RAX, XMMRegister::XMM0, true)?; self.store_gp_result(dst, Register::RAX)?; }
            ConvertKind::F2D => { self.cvtss2sd(XMMRegister::XMM0, XMMRegister::XMM0)?; self.store_xmm_result(dst, XMMRegister::XMM0)?; }
            ConvertKind::D2I => { self.cvttsd2si(Register::RAX, XMMRegister::XMM0, false)?; self.store_gp_result(dst, Register::RAX)?; }
            ConvertKind::D2L => { self.cvttsd2si(Register::RAX, XMMRegister::XMM0, true)?; self.store_gp_result(dst, Register::RAX)?; }
            ConvertKind::D2F => { self.cvtsd2ss(XMMRegister::XMM0, XMMRegister::XMM0)?; self.store_xmm_result(dst, XMMRegister::XMM0)?; }
            ConvertKind::I2B => { self.emit_rex(false, false, false, false); self.buf.write(&[0x0f, 0xbe, 0xc0])?; self.store_gp_result(dst, Register::RAX)?; }
            ConvertKind::I2C => { self.emit_rex(false, false, false, false); self.buf.write(&[0x0f, 0xb7, 0xc0])?; self.store_gp_result(dst, Register::RAX)?; }
            _ => { self.emit_rex(false, false, false, false); self.buf.write(&[0x0f, 0xbf, 0xc0])?; self.store_gp_result(dst, Register::RAX)?; }
        }
        Ok(())
    }

    fn emit_compare_value(&mut self, kind: CompareKind, lhs: &LirOperand, rhs: &LirOperand, dst: &LirOperand) -> Result<(), BackendError> {
        match kind {
            CompareKind::Float32NanLow | CompareKind::Float32NanHigh | CompareKind::Float64NanLow | CompareKind::Float64NanHigh => {
                let double = matches!(kind, CompareKind::Float64NanLow | CompareKind::Float64NanHigh);
                let nan = if matches!(kind, CompareKind::Float32NanHigh | CompareKind::Float64NanHigh) { 1 } else { -1 };
                self.emit_operand_to_xmm(lhs, XMMRegister::XMM0)?;
                self.emit_operand_to_xmm(rhs, XMMRegister::XMM1)?;
                if double { self.buf.write(&[0x66, 0x0f, 0x2e, 0xc1])?; } else { self.buf.write(&[0x0f, 0x2e, 0xc1])?; }
                let jp = self.emit_jcc_local(x86Condition::P)?;
                let je = self.emit_jcc_local(x86Condition::E)?;
                let jb = self.emit_jcc_local(x86Condition::B)?;
                // Fallthrough = above: +1
                self.mov_reg_imm(Register::RAX, 1, false)?;
                let jmp_above = self.emit_jmp_local()?;
                // Unordered (NaN): dcmpg -> -1, dcmpl -> +1
                let nan_pos = self.pos();
                self.patch_local(jp, nan_pos)?;
                self.mov_reg_imm(Register::RAX, nan as i32 as u32 as u64, false)?;
                let jmp_nan = self.emit_jmp_local()?;
                // Below (CF=1, strictly less): -1
                let less_pos = self.pos();
                self.patch_local(jb, less_pos)?;
                self.mov_reg_imm(Register::RAX, (-1i32) as u32 as u64, false)?;
                let jmp_less = self.emit_jmp_local()?;
                // Equal (ZF=1): 0
                let equal_pos = self.pos();
                self.patch_local(je, equal_pos)?;
                self.mov_reg_imm(Register::RAX, 0, false)?;
                let end_pos = self.pos();
                for d in [jmp_above, jmp_nan, jmp_less] { self.patch_local(d, end_pos)?; }
                self.store_gp_result(dst, Register::RAX)
            }
            _ => {
                let wide = matches!(kind, CompareKind::Int64 | CompareKind::Ref);
                // Zero the setcc target registers BEFORE the compare:
                // `xor` clobbers flags, so doing it after `cmp` would
                // destroy the very condition codes setcc reads (this
                // made every lcmp/fcmp produce 0).
                self.emit_rex(false, false, false, false);
                self.buf.write(&[0x45, 0x31, 0xc0])?; // xor r8d, r8d
                self.emit_rex(false, false, false, false);
                self.buf.write(&[0x45, 0x31, 0xc9])?; // xor r9d, r9d
                self.emit_operand_to_reg(lhs, Register::RAX)?;
                self.emit_operand_to_reg(rhs, Register::RCX)?;
                self.cmp_rr(Register::RAX, Register::RCX, wide)?;
                self.emit_cmp_to_int(wide, dst)
            }
        }
    }

    fn emit_compare(&mut self, kind: LirKind, lhs: &LirOperand, rhs: &LirOperand) -> Result<(), BackendError> {
        let wide = kind == LirKind::Int64 || kind == LirKind::Ref;
        self.emit_operand_to_reg(lhs, Register::RAX)?;
        self.emit_operand_to_reg(rhs, Register::RCX)?;
        self.cmp_rr(Register::RAX, Register::RCX, wide)
    }

    fn emit_cmp_to_int(&mut self, wide: bool, dst: &LirOperand) -> Result<(), BackendError> {
        // r8/r9 were zeroed before the cmp; setcc writes only their low
        // bytes so the upper bits are already clear.
        let _ = wide;
        self.emit_setcc(x86Condition::L, Register::R8)?;
        self.emit_setcc(x86Condition::G, Register::R9)?;
        self.movzx_same(Register::R8)?;
        self.movzx_same(Register::R9)?;
        self.mov_reg_reg(Register::RAX, Register::R9, false)?;
        self.alu_rr(Alu::Sub, Register::RAX, Register::R8, false)?;
        self.store_gp_result(dst, Register::RAX)
    }

    fn emit_load_field(&mut self, obj: &LirOperand, offset: usize, kind: LirKind, dst: &LirOperand) -> Result<(), BackendError> {
        self.emit_operand_to_reg(obj, Register::RAX)?;
        let null_jmp = self.emit_null_check(Register::RAX)?;
        match kind {
            LirKind::Float32 => self.sse_load_mem(XMMRegister::XMM0, Register::RAX, offset as i32, false)?,
            LirKind::Float64 => self.sse_load_mem(XMMRegister::XMM0, Register::RAX, offset as i32, true)?,
            _ => {
                let wide = kind == LirKind::Int64 || kind == LirKind::Ref;
                self.mov_reg_mem(Register::RAX, Some(Register::RAX), None, Scale::S1, offset as i32, wide)?;
            }
        }
        let over = self.jmp_over_stub()?;
        let end = self.emit_exception_stub(RuntimeStub::NullPointerException, null_jmp)?;
        self.patch_local(over, end)?;
        match kind {
            LirKind::Float32 | LirKind::Float64 => self.store_xmm_result(dst, XMMRegister::XMM0),
            _ => self.store_gp_result(dst, Register::RAX),
        }
    }

    fn emit_store_field(&mut self, obj: &LirOperand, offset: usize, value: &LirOperand, kind: LirKind) -> Result<(), BackendError> {
        self.emit_operand_to_reg(obj, Register::RAX)?;
        let null_jmp = self.emit_null_check(Register::RAX)?;
        match kind {
            LirKind::Float32 | LirKind::Float64 => {
                self.emit_operand_to_xmm(value, XMMRegister::XMM0)?;
                self.sse_store_mem(XMMRegister::XMM0, Register::RAX, offset as i32, kind == LirKind::Float64)?;
            }
            _ => {
                self.emit_operand_to_reg(value, Register::RCX)?;
                let wide = kind == LirKind::Int64 || kind == LirKind::Ref;
                self.mov_mem_reg(Some(Register::RAX), None, Scale::S1, offset as i32, Register::RCX, wide)?;
            }
        }
        let over = self.jmp_over_stub()?;
        let end = self.emit_exception_stub(RuntimeStub::NullPointerException, null_jmp)?;
        self.patch_local(over, end)
    }

    fn emit_load_indexed(&mut self, array: &LirOperand, index: &LirOperand, elem: ArrayElemKind, dst: &LirOperand) -> Result<(), BackendError> {
        self.emit_operand_to_reg(array, Register::RAX)?;
        let null_jmp = self.emit_null_check(Register::RAX)?;
        self.emit_operand_to_reg(index, Register::RCX)?;
        let bounds_jmp = self.emit_bounds_check(Register::RAX, Register::RCX)?;
        let addr = self.compute_element_addr(Register::RAX, Register::RCX, elem)?;
        self.emit_array_load(elem, addr)?;
        let over = self.jmp_over_stub()?;
        self.emit_exception_stub(RuntimeStub::ArrayIndexOutOfBounds, bounds_jmp)?;
        let end = self.emit_exception_stub(RuntimeStub::NullPointerException, null_jmp)?;
        self.patch_local(over, end)?;
        self.store_result_for_elem(dst, elem, Register::RAX, XMMRegister::XMM0)
    }

    fn emit_store_indexed(&mut self, array: &LirOperand, index: &LirOperand, value: &LirOperand, elem: ArrayElemKind) -> Result<(), BackendError> {
        self.emit_operand_to_reg(array, Register::RAX)?;
        let null_jmp = self.emit_null_check(Register::RAX)?;
        self.emit_operand_to_reg(index, Register::RCX)?;
        let bounds_jmp = self.emit_bounds_check(Register::RAX, Register::RCX)?;
        let addr = self.compute_element_addr(Register::RAX, Register::RCX, elem)?;
        match elem.value_kind() {
            ValueKind::Float32 | ValueKind::Float64 => {
                self.emit_operand_to_xmm(value, XMMRegister::XMM0)?;
                self.sse_store_mem(XMMRegister::XMM0, addr, 0, elem.value_kind() == ValueKind::Float64)?;
            }
            _ => {
                self.emit_operand_to_reg(value, Register::RDX)?;
                self.store_array_element(elem, addr, Register::RDX)?;
            }
        }
        let over = self.jmp_over_stub()?;
        self.emit_exception_stub(RuntimeStub::ArrayIndexOutOfBounds, bounds_jmp)?;
        let end = self.emit_exception_stub(RuntimeStub::NullPointerException, null_jmp)?;
        self.patch_local(over, end)
    }

    fn emit_array_length(&mut self, array: &LirOperand, dst: &LirOperand) -> Result<(), BackendError> {
        self.emit_operand_to_reg(array, Register::RAX)?;
        let null_jmp = self.emit_null_check(Register::RAX)?;
        let len_off = self.obj_layout().array_length_offset() as i32;
        self.mov_reg_mem(Register::RAX, Some(Register::RAX), None, Scale::S1, len_off, false)?;
        let over = self.jmp_over_stub()?;
        let end = self.emit_exception_stub(RuntimeStub::NullPointerException, null_jmp)?;
        self.patch_local(over, end)?;
        self.store_gp_result(dst, Register::RAX)
    }

    fn obj_layout(&self) -> ObjLayoutMode { self.options.obj_layout }

    fn array_length_offset(&self) -> i32 { self.obj_layout().array_length_offset() as i32 }
    fn array_base_offset(&self, elem: ArrayElemKind) -> i32 {
        self.obj_layout().array_base_offset(elem.element_size(), elem.element_align()) as i32
    }

    fn compute_element_addr(&mut self, array: Register, index: Register, elem: ArrayElemKind) -> Result<Register, BackendError> {
        let size = elem.element_size();
        self.mov_reg_reg(Register::RDX, index, false)?;
        if size == 1 {
        } else if size == 2 {
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x48, 0xc1, 0xe2, 0x01])?;
        } else if size == 4 {
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x48, 0xc1, 0xe2, 0x02])?;
        } else {
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x48, 0xc1, 0xe2, 0x03])?;
        }
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x48, 0x01, 0xc2])?;
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x48, 0x81, 0xc2])?;
        self.buf.write_i32(self.array_base_offset(elem))?;
        Ok(Register::RDX)
    }

    fn emit_array_load(&mut self, elem: ArrayElemKind, addr: Register) -> Result<(), BackendError> {
        match elem {
            ArrayElemKind::Boolean | ArrayElemKind::Byte => { self.emit_rex(false, false, false, false); self.buf.write(&[0x0f, 0xbe, 0x02])?; }
            ArrayElemKind::Char => { self.emit_rex(false, false, false, false); self.buf.write(&[0x0f, 0xb7, 0x02])?; }
            ArrayElemKind::Short => { self.emit_rex(false, false, false, false); self.buf.write(&[0x0f, 0xbf, 0x02])?; }
            ArrayElemKind::Int | ArrayElemKind::Ref => { self.mov_reg_mem(Register::RAX, Some(addr), None, Scale::S1, 0, elem == ArrayElemKind::Long || elem == ArrayElemKind::Double || elem == ArrayElemKind::Ref)?; }
            ArrayElemKind::Long => { self.mov_reg_mem(Register::RAX, Some(addr), None, Scale::S1, 0, true)?; }
            ArrayElemKind::Float => { self.sse_load_mem(XMMRegister::XMM0, addr, 0, false)?; }
            ArrayElemKind::Double => { self.sse_load_mem(XMMRegister::XMM0, addr, 0, true)?; }
        }
        Ok(())
    }

    fn store_array_element(&mut self, elem: ArrayElemKind, addr: Register, src: Register) -> Result<(), BackendError> {
        match elem {
            ArrayElemKind::Boolean | ArrayElemKind::Byte => { self.emit_rex(false, false, false, false); self.buf.write(&[0x88, 0x02])?; }
            ArrayElemKind::Char | ArrayElemKind::Short => { self.emit_rex(false, false, false, false); self.buf.write(&[0x66, 0x89, 0x02])?; }
            ArrayElemKind::Int => { self.emit_rex(false, false, false, false); self.buf.write(&[0x89, 0x02])?; }
            ArrayElemKind::Long | ArrayElemKind::Ref => { self.mov_mem_reg(Some(addr), None, Scale::S1, 0, src, true)?; }
            _ => return Err(BackendError::InvalidLir),
        }
        Ok(())
    }

    fn store_result_for_elem(&mut self, dst: &LirOperand, elem: ArrayElemKind, gp: Register, xmm: XMMRegister) -> Result<(), BackendError> {
        match elem.value_kind() {
            ValueKind::Float32 | ValueKind::Float64 => self.store_xmm_result(dst, xmm),
            _ => self.store_gp_result(dst, gp),
        }
    }

    fn emit_call(&mut self, dst: Option<&LirOperand>, operands: &[LirOperand], info: &crate::share::c1::c1_Instruction::CallInfo) -> Result<(), BackendError> {
        let Some(target) = info.target else {
            return self.emit_call_stub(dst, operands, info);
        };
        let win64 = self.options.abi == Abi::WindowsX64;
        let mut gp = 0usize;
        let mut xmm = 0usize;
        let mut slot = 0usize;
        let mut stack_args: Vec<(LirOperand, LirKind)> = Vec::new();
        // Windows x64: four unified argument slots — integer args go to
        // rcx/rdx/r8/r9, float args to xmm0..3; slot index is shared.
        // System V: six integer registers + eight xmm registers.
        let gp_win64 = [Register::RCX, Register::RDX, Register::R8, Register::R9];
        let gp_sysv = [Register::RDI, Register::RSI, Register::RDX, Register::RCX, Register::R8, Register::R9];
        let xmm_regs = [XMMRegister::XMM0, XMMRegister::XMM1, XMMRegister::XMM2, XMMRegister::XMM3, XMMRegister::XMM4, XMMRegister::XMM5, XMMRegister::XMM6, XMMRegister::XMM7];
        for op in operands {
            let kind = op.kind();
            let is_sse = matches!(kind, LirKind::Float32 | LirKind::Float64);
            if win64 {
                if slot < 4 {
                    if is_sse {
                        self.emit_operand_to_xmm(op, xmm_regs[slot])?;
                    } else {
                        self.emit_operand_to_reg(op, gp_win64[slot])?;
                    }
                    slot += 1;
                } else {
                    stack_args.push((op.clone(), kind));
                }
            } else if is_sse {
                if xmm < xmm_regs.len() {
                    self.emit_operand_to_xmm(op, xmm_regs[xmm])?;
                    xmm += 1;
                } else {
                    stack_args.push((op.clone(), kind));
                }
            } else if gp < gp_sysv.len() {
                self.emit_operand_to_reg(op, gp_sysv[gp])?;
                gp += 1;
            } else {
                stack_args.push((op.clone(), kind));
            }
        }
        let _ = (gp, xmm);
        let stack_size = if stack_args.is_empty() { 0 } else { stack_args.len() * 8 };
        let frame_adj = if self.options.abi == Abi::WindowsX64 { (32 + stack_size).div_ceil(16) * 16 } else { stack_size.div_ceil(16) * 16 };
        if frame_adj > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x48, 0x81, 0xec])?;
            self.buf.write_i32(frame_adj as i32)?;
        }
        for (i, (op, kind)) in stack_args.iter().enumerate() {
            let base = if self.options.abi == Abi::WindowsX64 { 32 } else { 0 };
            let off = (base + i * 8) as i32;
            if matches!(kind, LirKind::Float32 | LirKind::Float64) {
                self.emit_operand_to_xmm(op, XMMRegister::XMM0)?;
                self.sse_store_mem(XMMRegister::XMM0, Register::SP, off, *kind == LirKind::Float64)?;
            } else {
                self.emit_operand_to_reg(op, Register::RAX)?;
                self.mov_mem_reg(Some(Register::SP), None, Scale::S1, off, Register::RAX, true)?;
            }
        }
        self.emit_abs_call(target)?;
        self.pc_descs.push(PcDescEntry { pc_offset: self.pos(), bci: 0, is_safepoint: true });
        if frame_adj > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x48, 0x81, 0xc4])?;
            self.buf.write_i32(frame_adj as i32)?;
        }
        if let Some(dst) = dst {
            match dst.kind() {
                LirKind::Float32 | LirKind::Float64 => self.store_xmm_result(dst, XMMRegister::XMM0)?,
                _ => self.store_gp_result(dst, Register::RAX)?,
            }
        }
        Ok(())
    }

    /// Lowers an unresolved invoke to the `UnresolvedCall` runtime
    /// stub.  The JIT builds a flat array of raw 64-bit argument
    /// words on its own stack (receiver first for instance methods,
    /// then the declared parameters — the same order `try_invoke`
    /// marshals), then calls the trampoline with:
    ///   rcx = leaked StubSite (owner/name/descriptor)
    ///   rdx = pointer to the argument word array
    ///   r8  = word count
    ///   r9  = invoke opcode
    /// The handler resolves the target through `LinkResolver`, runs an
    /// installed nmethod via `try_invoke` when present and otherwise
    /// executes the method in `bytecodeInterpreter`.  Integer results
    /// return in rax; float/double results return in xmm0.
    fn emit_call_stub(&mut self, dst: Option<&LirOperand>, operands: &[LirOperand], info: &crate::share::c1::c1_Instruction::CallInfo) -> Result<(), BackendError> {
        let addr = self
            .options
            .stubs
            .get(&RuntimeStub::UnresolvedCall)
            .copied()
            .ok_or(BackendError::MissingRuntimeStub(RuntimeStub::UnresolvedCall))?;
        let n = operands.len();
        if n > 64 {
            return Err(BackendError::Unsupported(format!(
                "invoke with {} arguments",
                n
            )));
        }
        let total = (n * 8).div_ceil(16) * 16;
        if total > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x48, 0x81, 0xec])?;
            self.buf.write_i32(total as i32)?;
        }
        for (i, op) in operands.iter().enumerate() {
            self.emit_operand_raw_to_reg(op, Register::RAX)?;
            self.mov_mem_reg(Some(Register::SP), None, Scale::S1, (i * 8) as i32, Register::RAX, true)?;
        }
        self.mov_reg_imm(Register::RCX, info.site, true)?;
        self.mov_reg_reg(Register::RDX, Register::SP, true)?;
        self.mov_reg_imm(Register::R8, n as u64, false)?;
        self.mov_reg_imm(Register::R9, info.opcode as u64, false)?;
        self.emit_abs_call(addr)?;
        if total > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x48, 0x81, 0xc4])?;
            self.buf.write_i32(total as i32)?;
        }
        self.pc_descs.push(PcDescEntry { pc_offset: self.pos(), bci: 0, is_safepoint: true });
        if let Some(dst) = dst {
            match dst.kind() {
                LirKind::Float32 | LirKind::Float64 => self.store_xmm_result(dst, XMMRegister::XMM0)?,
                _ => self.store_gp_result(dst, Register::RAX)?,
            }
        }
        Ok(())
    }

    /// Loads an operand's raw 64-bit representation (bit pattern for
    /// floats, tagged word for refs) into a GP register — unlike
    /// `emit_operand_to_reg` this accepts the float constant kinds the
    /// CallMethod argument array needs.
    fn emit_operand_raw_to_reg(&mut self, op: &LirOperand, reg: Register) -> Result<(), BackendError> {
        match op {
            LirOperand::ConstI32(v) => self.mov_reg_imm(reg, *v as i32 as u64, false)?,
            LirOperand::ConstI64(v) => self.mov_reg_imm(reg, *v as u64, true)?,
            LirOperand::ConstF32(v) => self.mov_reg_imm(reg, *v as u64, false)?,
            LirOperand::ConstF64(v) => self.mov_reg_imm(reg, *v, true)?,
            LirOperand::Null => self.mov_reg_imm(reg, 0, false)?,
            LirOperand::Local { slot, kind }
            | LirOperand::Stack { slot, kind }
            | LirOperand::Temp { slot, kind } => {
                let off = match op {
                    LirOperand::Local { .. } => self.local_offset(*slot),
                    LirOperand::Stack { .. } => self.stack_offset(*slot),
                    _ => self.temp_offset(*slot),
                };
                let wide = matches!(kind, LirKind::Int64 | LirKind::Ref | LirKind::Float64);
                self.mov_reg_mem(reg, Some(Register::FP), None, Scale::S1, -(off as i32), wide)?;
            }
        }
        Ok(())
    }

    fn emit_return(&mut self, value: Option<&LirOperand>, kind: LirKind) -> Result<(), BackendError> {
        if let Some(v) = value {
            match kind {
                LirKind::Float32 | LirKind::Float64 => self.emit_operand_to_xmm(v, XMMRegister::XMM0)?,
                _ => self.emit_operand_to_reg(v, Register::RAX)?,
            }
        }
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x89, 0xec])?;
        self.buf.write_byte(0x5d)?;
        self.buf.write_byte(0xc3)?;
        Ok(())
    }

    fn emit_safepoint(&mut self, bci: usize) -> Result<(), BackendError> {
        if !self.options.emit_safepoints { return Ok(()); }
        let addr = self.options.stubs.get(&RuntimeStub::SafepointPoll).copied().ok_or(BackendError::MissingRuntimeStub(RuntimeStub::SafepointPoll))?;
        self.emit_abs_call(addr)?;
        self.pc_descs.push(PcDescEntry { pc_offset: self.pos(), bci: bci as i32, is_safepoint: true });
        Ok(())
    }

    fn emit_call_runtime(&mut self, stub: RuntimeStub, operands: &[LirOperand], dst: Option<&LirOperand>, _bci: usize) -> Result<(), BackendError> {
        // Handlers receive at most the four Win64 argument registers;
        // the trampoline does not forward stack arguments.
        if operands.len() > 4 {
            return Err(BackendError::Unsupported(format!(
                "runtime stub {:?} with {} operands",
                stub,
                operands.len()
            )));
        }
        let addr = self.options.stubs.get(&stub).copied().ok_or(BackendError::MissingRuntimeStub(stub))?;
        // Runtime stubs are real `extern "C"` Rust functions: Windows x64
        // takes args in rcx/rdx/r8/r9 with the rest on the stack above a
        // 32-byte shadow space.  System V uses rdi/rsi/rdx/rcx/r8/r9.
        let win64 = self.options.abi == Abi::WindowsX64;
        let regs: &[Register] = if win64 {
            &[Register::RCX, Register::RDX, Register::R8, Register::R9]
        } else {
            &[Register::RDI, Register::RSI, Register::RDX, Register::RCX, Register::R8, Register::R9]
        };
        let mut stack_ops: Vec<LirOperand> = Vec::new();
        for (i, op) in operands.iter().enumerate() {
            if i < regs.len() {
                self.emit_operand_to_reg(op, regs[i])?;
            } else {
                stack_ops.push(op.clone());
            }
        }
        let stack_size = stack_ops.len() * 8;
        let frame_adj = if win64 {
            (32 + stack_size).div_ceil(16) * 16
        } else {
            stack_size.div_ceil(16) * 16
        };
        if frame_adj > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x48, 0x81, 0xec])?;
            self.buf.write_i32(frame_adj as i32)?;
        }
        let stack_base: i32 = if win64 { 32 } else { 0 };
        for (i, op) in stack_ops.iter().enumerate() {
            let off = stack_base + (i * 8) as i32;
            self.emit_operand_to_reg(op, Register::RAX)?;
            self.mov_mem_reg(Some(Register::SP), None, Scale::S1, off, Register::RAX, true)?;
        }
        self.emit_abs_call(addr)?;
        if frame_adj > 0 {
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x48, 0x81, 0xc4])?;
            self.buf.write_i32(frame_adj as i32)?;
        }
        self.pc_descs.push(PcDescEntry { pc_offset: self.pos(), bci: 0, is_safepoint: stub == RuntimeStub::SafepointPoll });
        if let Some(dst) = dst {
            self.store_gp_result(dst, Register::RAX)?;
        }
        Ok(())
    }

    fn emit_jmp(&mut self, target: usize) -> Result<(), BackendError> {
        self.buf.write_byte(0xe9)?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        self.patches.push(Patch::Jmp { disp, target });
        Ok(())
    }

    fn emit_jcc(&mut self, cond: x86Condition, target: usize) -> Result<(), BackendError> {
        self.buf.write(&[0x0f, 0x80 | cond as u8])?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        self.patches.push(Patch::Jcc { disp, cond, target });
        Ok(())
    }

    fn emit_branch(&mut self, cond: Condition, t: usize, f: usize) -> Result<(), BackendError> {
        let cc = match cond { Condition::Eq => x86Condition::E, Condition::Ne => x86Condition::NE, Condition::Lt => x86Condition::L, Condition::Le => x86Condition::LE, Condition::Gt => x86Condition::G, Condition::Ge => x86Condition::GE };
        self.emit_jcc(cc, t)?;
        self.emit_jmp(f)
    }

    fn emit_table_switch(&mut self, index: &LirOperand, low: i32, high: i32, default_block: usize, targets: &[usize]) -> Result<(), BackendError> {
        self.emit_operand_to_reg(index, Register::RAX)?;
        if low != 0 {
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x3d])?;
            self.buf.write_i32(low)?;
            self.emit_jcc(x86Condition::L, default_block)?;
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x3d])?;
            self.buf.write_i32(high)?;
            self.emit_jcc(x86Condition::G, default_block)?;
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x2d])?;
            self.buf.write_i32(low)?;
        } else {
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x85, 0xc0])?;
            self.emit_jcc(x86Condition::S, default_block)?;
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x3d])?;
            self.buf.write_i32(high)?;
            self.emit_jcc(x86Condition::G, default_block)?;
        }
        self.mov_reg_reg(Register::RDX, Register::RAX, false)?;
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x48, 0x8d, 0x0d])?;
        let lea_disp = self.pos();
        self.buf.write_i32(0)?;
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x48, 0x63, 0x14, 0x91])?;
        self.emit_rex(true, false, false, false);
        self.buf.write(&[0x48, 0x03, 0xd1])?;
        self.buf.write(&[0xff, 0xe2])?;
        let table_base = self.pos();
        self.table_lea.push((lea_disp, table_base));
        for target in targets {
            let entry = self.pos();
            self.buf.write_i32(0)?;
            self.table_patches.push((entry, table_base, *target));
        }
        Ok(())
    }

    fn emit_lookup_switch(&mut self, key: &LirOperand, pairs: &[(i32, usize)], default_block: usize) -> Result<(), BackendError> {
        self.emit_operand_to_reg(key, Register::RAX)?;
        for (k, target) in pairs {
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x3d])?;
            self.buf.write_i32(*k)?;
            self.emit_jcc(x86Condition::E, *target)?;
        }
        self.emit_jmp(default_block)
    }

    fn patch_all(&mut self) -> Result<(), BackendError> {
        for p in self.patches.clone() {
            let (disp, target) = match p {
                Patch::Jmp { disp, target } | Patch::Jcc { disp, target, .. } => (disp, target),
                Patch::LocalJmp { .. } => continue,
            };
            let target_off = *self.block_offsets.get(&target).ok_or(BackendError::InvalidLir)?;
            self.patch_rel32(disp, target_off)?;
        }
        for (disp, table_base) in self.table_lea.clone() {
            self.patch_i32(disp, (table_base as i32) - (disp as i32) - 4)?;
        }
        for (entry, table_base, target) in self.table_patches.clone() {
            let target_off = *self.block_offsets.get(&target).ok_or(BackendError::InvalidLir)?;
            self.patch_i32(entry, (target_off as i32) - (table_base as i32))?;
        }
        for (disp, marker) in self.local_patches.clone() {
            self.patch_local(disp, marker)?;
        }
        Ok(())
    }

    fn patch_rel32(&self, disp_off: usize, target_off: usize) -> Result<(), BackendError> {
        let disp = target_off as i32 - disp_off as i32 - 4;
        self.buf.patch_i32(disp_off, disp)?;
        Ok(())
    }

    fn patch_i32(&self, off: usize, val: i32) -> Result<(), BackendError> {
        self.buf.patch_i32(off, val)?;
        Ok(())
    }

    fn emit_jcc_local(&mut self, cond: x86Condition) -> Result<usize, BackendError> {
        self.buf.write(&[0x0f, 0x80 | cond as u8])?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        Ok(disp)
    }

    fn emit_jmp_local(&mut self) -> Result<usize, BackendError> {
        self.buf.write_byte(0xe9)?;
        let disp = self.pos();
        self.buf.write_i32(0)?;
        Ok(disp)
    }

    fn patch_local(&mut self, disp_off: usize, target_off: usize) -> Result<(), BackendError> {
        self.patch_rel32(disp_off, target_off)
    }

    fn jmp_over_stub(&mut self) -> Result<usize, BackendError> {
        self.emit_jmp_local()
    }

    fn emit_exception_stub(&mut self, stub: RuntimeStub, jump: Option<usize>) -> Result<usize, BackendError> {
        let stub_pos = self.pos();
        if let Some(d) = jump { self.patch_local(d, stub_pos)?; }
        self.emit_runtime_call_addr(stub)?;
        self.buf.write(&[0x0f, 0x0b])?;
        Ok(self.pos())
    }

    fn emit_null_check(&mut self, reg: Register) -> Result<Option<usize>, BackendError> {
        if !self.options.stubs.contains_key(&RuntimeStub::NullPointerException) {
            return Err(BackendError::MissingRuntimeStub(RuntimeStub::NullPointerException));
        }
        self.test_rr(reg, reg, true)?;
        let disp = self.emit_jcc_local(x86Condition::E)?;
        Ok(Some(disp))
    }

    fn emit_bounds_check(&mut self, array: Register, index: Register) -> Result<Option<usize>, BackendError> {
        if !self.options.stubs.contains_key(&RuntimeStub::ArrayIndexOutOfBounds) {
            return Err(BackendError::MissingRuntimeStub(RuntimeStub::ArrayIndexOutOfBounds));
        }
        let len_off = self.array_length_offset();
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x8b, 0x90])?;
        self.buf.write_i32(len_off)?;
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x3b, 0xca])?;
        let disp = self.emit_jcc_local(x86Condition::AE)?;
        Ok(Some(disp))
    }

    /// Emits the `idiv` overflow guard: division overflows only for
    /// `INT_MIN / -1`.  Jumps to `end` (skipping the stub) whenever
    /// the operands cannot overflow; falls through to the
    /// ArithmeticException stub otherwise.
    fn emit_div_overflow_check(&mut self, wide: bool) -> Result<(), BackendError> {
        let stub = RuntimeStub::ArithmeticException;
        if !self.options.stubs.contains_key(&stub) {
            return Err(BackendError::MissingRuntimeStub(stub));
        }
        let mut skips: Vec<usize> = Vec::new();
        if wide {
            // dividend (rax) == i64::MIN ?
            self.mov_reg_imm(Register::RDX, 0x8000_0000_0000_0000, true)?;
            self.cmp_rr(Register::RAX, Register::RDX, true)?;
            skips.push(self.emit_jcc_local(x86Condition::NE)?);
            // divisor (rcx) == -1 ?  -> cmp rcx, imm8(-1)
            self.emit_rex(true, false, false, false);
            self.buf.write(&[0x83, 0xf9, 0xff])?;
            skips.push(self.emit_jcc_local(x86Condition::NE)?);
        } else {
            // dividend (eax) == i32::MIN ? -> cmp eax, imm32
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x3d])?;
            self.buf.write_i32(i32::MIN)?;
            skips.push(self.emit_jcc_local(x86Condition::NE)?);
            // divisor (ecx) == -1 ? -> cmp ecx, imm8(-1)
            self.emit_rex(false, false, false, false);
            self.buf.write(&[0x83, 0xf9, 0xff])?;
            skips.push(self.emit_jcc_local(x86Condition::NE)?);
        }
        self.emit_runtime_call_addr(stub)?;
        self.buf.write(&[0x0f, 0x0b])?;
        let end = self.pos();
        for d in skips {
            self.patch_local(d, end)?;
        }
        Ok(())
    }

    /// Emits the divide-by-zero guard: a zero divisor falls through to
    /// the ArithmeticException stub; a nonzero divisor jumps over it.
    fn emit_runtime_check_zero(&mut self, reg: Register) -> Result<(), BackendError> {
        let stub = RuntimeStub::ArithmeticException;
        if !self.options.stubs.contains_key(&stub) {
            return Err(BackendError::MissingRuntimeStub(stub));
        }
        self.test_rr(reg, reg, reg == Register::RCX || reg == Register::RAX)?;
        let jnz = self.emit_jcc_local(x86Condition::NE)?;
        self.emit_runtime_call_addr(stub)?;
        self.buf.write(&[0x0f, 0x0b])?;
        let end = self.pos();
        self.patch_local(jnz, end)?;
        Ok(())
    }

    fn emit_runtime_call_addr(&mut self, stub: RuntimeStub) -> Result<(), BackendError> {
        let addr = self.options.stubs.get(&stub).copied().ok_or(BackendError::MissingRuntimeStub(stub))?;
        self.emit_abs_call(addr)
    }

    fn emit_abs_call(&mut self, addr: u64) -> Result<(), BackendError> {
        self.mov_reg_imm(Register::RAX, addr, true)?;
        self.buf.write(&[0xff, 0xd0])?;
        Ok(())
    }

    fn store_gp_result(&mut self, dst: &LirOperand, reg: Register) -> Result<(), BackendError> {
        let (off, kind) = self.frame_addr(dst).ok_or(BackendError::InvalidLir)?;
        self.store_gp_at(off, kind, reg)
    }

    fn store_gp_at(&mut self, off: usize, kind: LirKind, reg: Register) -> Result<(), BackendError> {
        let wide = kind == LirKind::Int64 || kind == LirKind::Ref;
        self.mov_mem_reg(Some(Register::FP), None, Scale::S1, -(off as i32), reg, wide)
    }

    fn store_xmm_result(&mut self, dst: &LirOperand, xmm: XMMRegister) -> Result<(), BackendError> {
        let (off, kind) = self.frame_addr(dst).ok_or(BackendError::InvalidLir)?;
        let disp = -(off as i32);
        if kind == LirKind::Float32 {
            self.sse_store_mem(xmm, Register::FP, disp, false)?;
            self.zero_slot_upper(off)?;
        } else {
            self.sse_store_mem(xmm, Register::FP, disp, true)?;
        }
        Ok(())
    }

    fn emit_store_frame(&mut self, local: bool, slot: usize, src: &LirOperand, kind: LirKind) -> Result<(), BackendError> {
        match kind {
            LirKind::Float32 | LirKind::Float64 => {
                self.emit_operand_to_xmm(src, XMMRegister::XMM0)?;
                let off = if local { self.local_offset(slot) } else { self.stack_offset(slot) };
                let disp = -(off as i32);
                self.sse_store_mem(XMMRegister::XMM0, Register::FP, disp, kind == LirKind::Float64)?;
                if kind == LirKind::Float32 {
                    self.zero_slot_upper(off)?;
                }
            }
            _ => {
                self.emit_operand_to_reg(src, Register::RAX)?;
                let off = if local { self.local_offset(slot) } else { self.stack_offset(slot) };
                let wide = kind == LirKind::Int64 || kind == LirKind::Ref;
                self.mov_mem_reg(Some(Register::FP), None, Scale::S1, -(off as i32), Register::RAX, wide)?;
            }
        }
        Ok(())
    }

    fn emit_operand_to_reg(&mut self, op: &LirOperand, reg: Register) -> Result<(), BackendError> {
        match op {
            LirOperand::ConstI32(v) => self.mov_reg_imm(reg, *v as i32 as u64, false)?,
            LirOperand::ConstI64(v) => self.mov_reg_imm(reg, *v as u64, true)?,
            LirOperand::Null => self.mov_reg_imm(reg, 0, false)?,
            LirOperand::Local { slot, kind } | LirOperand::Stack { slot, kind } | LirOperand::Temp { slot, kind } => {
                let off = match op { LirOperand::Local { .. } => self.local_offset(*slot), LirOperand::Stack { .. } => self.stack_offset(*slot), _ => self.temp_offset(*slot) };
                let wide = *kind == LirKind::Int64 || *kind == LirKind::Ref;
                self.mov_reg_mem(reg, Some(Register::FP), None, Scale::S1, -(off as i32), wide)?;
            }
            _ => return Err(BackendError::InvalidLir),
        }
        Ok(())
    }

    fn emit_operand_to_xmm(&mut self, op: &LirOperand, xmm: XMMRegister) -> Result<(), BackendError> {
        match op {
            LirOperand::ConstF32(v) => {
                self.mov_reg_imm(Register::RAX, *v as u64, false)?;
                self.movd_xmm_from_gp(xmm, Register::RAX, false)?;
            }
            LirOperand::ConstF64(v) => {
                self.mov_reg_imm(Register::RAX, *v, true)?;
                self.movq_xmm_from_gp(xmm, Register::RAX)?;
            }
            LirOperand::Local { slot, kind } | LirOperand::Stack { slot, kind } | LirOperand::Temp { slot, kind } => {
                let off = match op { LirOperand::Local { .. } => self.local_offset(*slot), LirOperand::Stack { .. } => self.stack_offset(*slot), _ => self.temp_offset(*slot) };
                let disp = -(off as i32);
                if *kind == LirKind::Float64 { self.sse_load_mem(xmm, Register::FP, disp, true)?; } else { self.sse_load_mem(xmm, Register::FP, disp, false)?; }
            }
            LirOperand::ConstI32(v) => { self.mov_reg_imm(Register::RAX, *v as i32 as u64, false)?; self.cvtsi2ss(xmm, Register::RAX, false)?; }
            LirOperand::ConstI64(v) => { self.mov_reg_imm(Register::RAX, *v as u64, true)?; self.cvtsi2ss(xmm, Register::RAX, true)?; }
            LirOperand::Null => { self.emit_rex(false, false, false, false); self.buf.write(&[0x31, 0xc0])?; self.cvtsi2ss(xmm, Register::RAX, false)?; }
            _ => return Err(BackendError::InvalidLir),
        }
        Ok(())
    }

    fn emit_rex(&mut self, w: bool, r: bool, x: bool, b: bool) {
        let bits = (w as u8) << 3 | (r as u8) << 2 | (x as u8) << 1 | b as u8;
        if w || r || x || b { let _ = self.buf.write_byte(0x40 | bits); }
    }

    fn rex_bits(&self, reg: Register, rm: Register, w: bool) -> (bool, bool, bool, bool) {
        (w, reg.needs_rex(), false, rm.needs_rex())
    }

    fn emit_modrm_sib(&mut self, reg: u8, base: Option<Register>, index: Option<Register>, scale: Scale, disp: i32) -> Result<(), BackendError> {
        let base_enc = base.map(|b| b.encoding() & 7).unwrap_or(5);
        let index_enc = index.map(|i| i.encoding() & 7).unwrap_or(4);
        let need_sib = index.is_some() || matches!(base, Some(Register::RSP) | Some(Register::R12)) || base.is_none();
        let use_disp32 = base.is_none() || disp < -128 || disp > 127 || matches!(base, Some(Register::RBP) | Some(Register::R13)) && disp == 0;
        let mod_bits = if use_disp32 { 2 } else if disp == 0 && !matches!(base, Some(Register::RBP) | Some(Register::R13)) { 0 } else { 1 };
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

    fn emit_mem_modrm(&mut self, reg: u8, base: Option<Register>, index: Option<Register>, scale: Scale, disp: i32) -> Result<(), BackendError> {
        let rex_r = reg >= 8;
        let rex_b = base.map(|b| b.needs_rex()).unwrap_or(false) || index.map(|i| i.needs_rex()).unwrap_or(false);
        self.emit_rex(false, rex_r, false, rex_b);
        self.emit_modrm_sib(reg, base, index, scale, disp)
    }

    fn emit_mem_modrm_w(&mut self, reg: u8, base: Option<Register>, index: Option<Register>, scale: Scale, disp: i32) -> Result<(), BackendError> {
        let rex_r = reg >= 8;
        let rex_b = base.map(|b| b.needs_rex()).unwrap_or(false) || index.map(|i| i.needs_rex()).unwrap_or(false);
        self.emit_rex(true, rex_r, false, rex_b);
        self.emit_modrm_sib(reg, base, index, scale, disp)
    }

    fn mov_reg_imm(&mut self, reg: Register, imm: u64, wide: bool) -> Result<(), BackendError> {
        self.emit_rex(wide, false, false, reg.needs_rex());
        self.buf.write_byte(0xb8 + (reg.encoding() & 7))?;
        if wide { self.buf.write_u64(imm)?; } else { self.buf.write_u32(imm as u32)?; }
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
        if wide { self.emit_rex(true, reg.needs_rex(), false, base.map(|b| b.needs_rex()).unwrap_or(false) || index.map(|i| i.needs_rex()).unwrap_or(false)); }
        else { self.emit_rex(false, reg.needs_rex(), false, base.map(|b| b.needs_rex()).unwrap_or(false) || index.map(|i| i.needs_rex()).unwrap_or(false)); }
        self.buf.write_byte(0x8b)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    fn mov_mem_reg(&mut self, base: Option<Register>, index: Option<Register>, scale: Scale, disp: i32, reg: Register, wide: bool) -> Result<(), BackendError> {
        if wide { self.emit_rex(true, reg.needs_rex(), false, base.map(|b| b.needs_rex()).unwrap_or(false) || index.map(|i| i.needs_rex()).unwrap_or(false)); }
        else { self.emit_rex(false, reg.needs_rex(), false, base.map(|b| b.needs_rex()).unwrap_or(false) || index.map(|i| i.needs_rex()).unwrap_or(false)); }
        self.buf.write_byte(0x89)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    fn test_rr(&mut self, a: Register, b: Register, wide: bool) -> Result<(), BackendError> {
        let (w, r, _, bb) = self.rex_bits(a, b, wide);
        self.emit_rex(w, r, false, bb);
        self.buf.write(&[0x85, 0xc0 | ((a.encoding() & 7) << 3) | (b.encoding() & 7)])?;
        Ok(())
    }

    fn cmp_rr(&mut self, a: Register, b: Register, wide: bool) -> Result<(), BackendError> {
        let (w, r, _, bb) = self.rex_bits(a, b, wide);
        self.emit_rex(w, r, false, bb);
        self.buf.write(&[0x39, 0xc0 | ((b.encoding() & 7) << 3) | (a.encoding() & 7)])?;
        Ok(())
    }

    fn alu_rr(&mut self, alu: Alu, dst: Register, src: Register, wide: bool) -> Result<(), BackendError> {
        let (w, r, _, b) = self.rex_bits(dst, src, wide);
        self.emit_rex(w, r, false, b);
        let (op, op2) = match alu {
            Alu::Add => (0x03u8, None),
            Alu::Sub => (0x2b, None),
            Alu::And => (0x23, None),
            Alu::Or => (0x0b, None),
            Alu::Xor => (0x33, None),
            Alu::Div => return Err(BackendError::InvalidLir),
            Alu::Mul => (0, Some(0xaf)),
        };
        if let Some(op2) = op2 {
            self.buf.write(&[0x0f, op2, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        } else {
            self.buf.write(&[op, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        }
        Ok(())
    }

    fn shift_rr(&mut self, shift: Shift, reg: Register, by_cl: bool, wide: bool) -> Result<(), BackendError> {
        let ext = match shift { Shift::Shl => 4, Shift::Shr => 5, Shift::Sar => 7 };
        if by_cl {
            self.emit_rex(wide, false, false, reg.needs_rex());
            self.buf.write(&[0xd3, 0xc0 | (ext << 3) | (reg.encoding() & 7)])?;
        } else {
            self.emit_rex(wide, false, false, reg.needs_rex());
            self.buf.write(&[0xc1, 0xc0 | (ext << 3) | (reg.encoding() & 7), 1])?;
        }
        Ok(())
    }

    fn emit_setcc(&mut self, cond: x86Condition, reg: Register) -> Result<(), BackendError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write(&[0x0f, 0x90 | cond as u8, 0xc0 | (reg.encoding() & 7)])?;
        Ok(())
    }

    fn movzx_same(&mut self, reg: Register) -> Result<(), BackendError> {
        // movzx r32, r8 of the same register — the destination is the
        // modrm.reg field, which must encode `reg` too (a hardcoded 0
        // silently wrote every result into r8d).
        let enc = reg.encoding() & 7;
        self.emit_rex(false, reg.needs_rex(), false, reg.needs_rex());
        self.buf.write(&[0x0f, 0xb6, 0xc0 | (enc << 3) | enc])?;
        Ok(())
    }

    /// SSE ops need the legacy prefix (0x66/0xF2/0xF3) emitted BEFORE
    /// the REX byte — the canonical order is `66 F2 F3 REX opcode`.
    /// Emitting REX first produces encodings some cores decode as a
    /// different instruction (this silently zeroed xmm results).
    fn emit_sse_rex(&mut self, prefix: u8, w: bool, r: bool, x: bool, b: bool) -> Result<(), BackendError> {
        self.buf.write(&[prefix])?;
        self.emit_rex(w, r, x, b);
        Ok(())
    }

    fn sse_load_mem(&mut self, xmm: XMMRegister, base: Register, disp: i32, double: bool) -> Result<(), BackendError> {
        self.emit_sse_rex(if double { 0xf2 } else { 0xf3 }, double, xmm.needs_rex(), false, base.needs_rex())?;
        self.buf.write(&[0x0f, 0x10])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    fn sse_store_mem(&mut self, xmm: XMMRegister, base: Register, disp: i32, double: bool) -> Result<(), BackendError> {
        self.emit_sse_rex(if double { 0xf2 } else { 0xf3 }, double, xmm.needs_rex(), false, base.needs_rex())?;
        self.buf.write(&[0x0f, 0x11])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    fn movd_xmm_from_gp(&mut self, xmm: XMMRegister, gp: Register, _wide: bool) -> Result<(), BackendError> {
        self.emit_sse_rex(0x66, false, xmm.needs_rex(), false, gp.needs_rex())?;
        self.buf.write(&[0x0f, 0x6e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn movq_xmm_from_gp(&mut self, xmm: XMMRegister, gp: Register) -> Result<(), BackendError> {
        self.emit_sse_rex(0x66, true, xmm.needs_rex(), false, gp.needs_rex())?;
        self.buf.write(&[0x0f, 0x6e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn cvtsi2ss(&mut self, xmm: XMMRegister, gp: Register, wide: bool) -> Result<(), BackendError> {
        self.emit_sse_rex(0xf3, wide, xmm.needs_rex(), false, gp.needs_rex())?;
        self.buf.write(&[0x0f, 0x2a, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn cvtsi2sd(&mut self, xmm: XMMRegister, gp: Register, wide: bool) -> Result<(), BackendError> {
        self.emit_sse_rex(0xf2, wide, xmm.needs_rex(), false, gp.needs_rex())?;
        self.buf.write(&[0x0f, 0x2a, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    fn cvttss2si(&mut self, gp: Register, xmm: XMMRegister, wide: bool) -> Result<(), BackendError> {
        self.emit_sse_rex(0xf3, wide, gp.needs_rex(), false, xmm.needs_rex())?;
        self.buf.write(&[0x0f, 0x2c, 0xc0 | ((gp.encoding() & 7) << 3) | (xmm.encoding() & 7)])?;
        Ok(())
    }

    fn cvttsd2si(&mut self, gp: Register, xmm: XMMRegister, wide: bool) -> Result<(), BackendError> {
        self.emit_sse_rex(0xf2, wide, gp.needs_rex(), false, xmm.needs_rex())?;
        self.buf.write(&[0x0f, 0x2c, 0xc0 | ((gp.encoding() & 7) << 3) | (xmm.encoding() & 7)])?;
        Ok(())
    }

    fn cvtss2sd(&mut self, dst: XMMRegister, src: XMMRegister) -> Result<(), BackendError> {
        self.emit_sse_rex(0xf3, false, dst.needs_rex(), false, src.needs_rex())?;
        self.buf.write(&[0x0f, 0x5a, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    fn cvtsd2ss(&mut self, dst: XMMRegister, src: XMMRegister) -> Result<(), BackendError> {
        self.emit_sse_rex(0xf2, false, dst.needs_rex(), false, src.needs_rex())?;
        self.buf.write(&[0x0f, 0x5a, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    fn sse_arith(&mut self, alu: Alu, double: bool) -> Result<(), BackendError> {
        let (prefix, op) = match (alu, double) {
            (Alu::Add, false) => (0xf3, 0x58),
            (Alu::Add, true) => (0xf2, 0x58),
            (Alu::Sub, false) => (0xf3, 0x5c),
            (Alu::Sub, true) => (0xf2, 0x5c),
            (Alu::Mul, false) => (0xf3, 0x59),
            (Alu::Mul, true) => (0xf2, 0x59),
            (Alu::Div, false) => (0xf3, 0x5e),
            (Alu::Div, true) => (0xf2, 0x5e),
            _ => return Err(BackendError::Unsupported("sse arith".to_string())),
        };
        self.emit_sse_rex(prefix, double, false, false, XMMRegister::XMM1.needs_rex())?;
        self.buf.write(&[0x0f, op, 0xc1])?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Alu {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy)]
enum Shift {
    Shl,
    Shr,
    Sar,
}

fn of_placeholder() -> u8 { 0x0f }

pub fn emit_program(buf: &CodeBuffer, program: &LirProgram, options: &BackendOptions) -> Result<EmitResult, BackendError> {
    let mut emitter = X86Emitter::new(buf, program, options);
    emitter.emit()
}

pub fn compile_cfg_to_lir(cfg: &ControlFlowGraph) -> LirProgram {
    crate::share::c1::c1_LIR::LirGenerator::generate(
        cfg,
        &crate::share::c1::c1_GraphBuilder::MethodMetadata::default(),
    )
}
