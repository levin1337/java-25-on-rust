/*
 * x86_64 instruction encodings for the share-level Assembler.
 * Mirrors cpu/x86/vm/assembler_x86.hpp emit helpers, restricted to the
 * integer/control subset used by MacroAssembler share operations.
 *
 * Emission order is always: [operand-size prefix] [REX] opcode modrm sib disp.
 */

use crate::share::asm::assembler::{Assembler, AssemblerError, LabelId, PatchKind, RelocSpec};
use crate::share::asm::register::{ConditionCode, FloatRegister, NOREG, OperandSize, Register, Scale};
use crate::share::asm::reloc::{RelocType, RelocWidth, RelocValue};

/// Memory operand, mirrors assembler_x86 Address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address {
    pub base: Register,
    pub index: Register,
    pub scale: Scale,
    pub disp: i64,
    pub rip_relative: bool,
}

impl Address {
    pub const EMPTY: Address = Address {
        base: NOREG,
        index: NOREG,
        scale: Scale::S1,
        disp: 0,
        rip_relative: false,
    };

    pub fn base_disp(base: Register, disp: i64) -> Address {
        Address { base, index: NOREG, scale: Scale::S1, disp, rip_relative: false }
    }

    pub fn base_index_scale_disp(base: Register, index: Register, scale: Scale, disp: i64) -> Address {
        Address { base, index, scale, disp, rip_relative: false }
    }

    pub fn absolute(disp: i64) -> Address {
        Address { base: NOREG, index: NOREG, scale: Scale::S1, disp, rip_relative: false }
    }

    pub fn rip(disp: i64) -> Address {
        Address { base: NOREG, index: NOREG, scale: Scale::S1, disp, rip_relative: true }
    }

    pub fn ind(reg: Register) -> Address {
        Address::base_disp(reg, 0)
    }
}

/// Right-hand side of a mov/alu: register or memory.
#[derive(Clone, Copy, Debug)]
pub enum RmOperand {
    Reg(Register),
    Mem(Address),
}

impl RmOperand {
    pub fn reg(r: Register) -> RmOperand {
        RmOperand::Reg(r)
    }
    pub fn mem(a: Address) -> RmOperand {
        RmOperand::Mem(a)
    }
}

#[derive(Clone, Copy)]
struct Rex {
    w: bool,
    r: bool,
    x: bool,
    b: bool,
}

impl Rex {
    fn emit(self, asm: &mut Assembler) {
        if self.w || self.r || self.x || self.b {
            let byte = 0x40 | (self.w as u8) << 3 | (self.r as u8) << 2 | (self.x as u8) << 1 | self.b as u8;
            asm.emit_byte(byte);
        }
    }
}

fn opsize_prefix(size: OperandSize) -> Option<u8> {
    match size {
        OperandSize::Size2 => Some(0x66),
        _ => None,
    }
}

fn w_bit(size: OperandSize) -> bool {
    size.bytes() == 8
}

/// Shift count operand for D1/D3/C1 group.
#[derive(Clone, Copy, Debug)]
pub enum ShiftCount {
    Imm(i8),
    Cl,
    Reg(Register),
}

impl Assembler {
    /// Mark current position as instruction start (used by relocate()).
    pub fn mark_inst(&mut self) {
        self.set_inst_mark();
    }

    fn inst_begin(&mut self) {
        self.set_inst_mark();
    }

    // ---- REX / modrm / sib / disp -----------------------------------------

    fn rex_of_mem(&self, reg_bits: u8, addr: &Address, w: bool) -> Rex {
        if addr.rip_relative {
            return Rex { w, r: (reg_bits & 8) != 0, x: false, b: false };
        }
        let base_enc = if addr.base.is_valid() { Some(addr.base.encoding() as usize) } else { None };
        let index_enc = if addr.index.is_valid() { Some(addr.index.encoding() as usize) } else { None };
        Rex {
            w,
            r: (reg_bits & 8) != 0,
            x: index_enc.map_or(false, |i| i >= 8),
            // SIB base=101 / index=100 with mod=00 mean "absent"; do not set bits.
            b: base_enc.map_or(false, |b| b >= 8),
        }
    }

    fn rex_of_reg(&self, reg_bits: u8, rm: Register, w: bool) -> Rex {
        Rex {
            w,
            r: (reg_bits & 8) != 0,
            x: false,
            b: rm.encoding() >= 8,
        }
    }

    /// Emit modrm + sib + disp for a memory operand (no REX, no opcode).
    fn modrm_mem_tail(&mut self, reg_bits: u8, addr: &Address) -> Result<(), AssemblerError> {
        if addr.rip_relative {
            self.emit_byte(0x05 | ((reg_bits & 7) << 3));
            if !Assembler::is_simm(addr.disp, 32) {
                return Err(AssemblerError::InvalidOperand("rip displacement out of range".into()));
            }
            self.emit_dword(addr.disp as i32 as u32);
            return Ok(());
        }

        let base = addr.base;
        let index = addr.index;
        let base_enc = if base.is_valid() { Some(base.encoding() as usize) } else { None };
        let index_enc = if index.is_valid() { Some(index.encoding() as usize) } else { None };

        let need_sib = index_enc.is_some()
            || base_enc.map_or(true, |b| (b & 7) == 4)
            || (base_enc.is_none() && index_enc.is_none());

        let disp = addr.disp;
        let (mod_bits, disp_size): (u8, usize) = if base_enc.is_none() {
            (0, 4)
        } else {
            let b = base_enc.unwrap();
            if disp == 0 && (b & 7) != 5 {
                (0, 0)
            } else if Assembler::is_simm(disp, 8) {
                (1, 1)
            } else if Assembler::is_simm(disp, 32) {
                (2, 4)
            } else {
                return Err(AssemblerError::InvalidOperand(format!(
                    "displacement {} out of range",
                    disp
                )));
            }
        };

        let rm_bits: u8 = if need_sib { 4 } else { (base_enc.unwrap() & 7) as u8 };
        self.emit_byte((mod_bits << 6) | ((reg_bits & 7) << 3) | rm_bits);

        if need_sib {
            let scale_bits = addr.scale.encoding();
            let index_bits: u8 = index_enc.map_or(4, |i| (i & 7) as u8);
            let base_bits: u8 = base_enc.map_or(5, |b| (b & 7) as u8);
            self.emit_byte((scale_bits << 6) | (index_bits << 3) | base_bits);
        }

        match disp_size {
            0 => {}
            1 => self.emit_byte(disp as i8 as u8),
            4 => self.emit_dword(disp as i32 as u32),
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Emit modrm (mod=11) for register-direct operand (no REX, no opcode).
    fn modrm_reg_tail(&mut self, reg_bits: u8, rm_reg: Register) {
        self.emit_byte(0xC0 | ((reg_bits & 7) << 3) | (rm_reg.encoding() & 7));
    }

    /// Emit REX + opcode bytes + modrm/sib/disp for "op ..., r/m=mem".
    /// `op` is the full opcode sequence after any prefixes (e.g. [0x89] or
    /// [0x0F, 0xAF]).
    fn emit_rm_mem(&mut self, op: &[u8], reg_bits: u8, addr: Address, w: bool) -> Result<(), AssemblerError> {
        self.rex_of_mem(reg_bits, &addr, w).emit(self);
        self.emit_bytes(op);
        self.modrm_mem_tail(reg_bits, &addr)
    }

    /// Emit REX + opcode bytes + modrm (mod=11) for "op ..., r/m=reg".
    fn emit_rm_reg(&mut self, op: &[u8], reg_bits: u8, rm: Register, w: bool) {
        self.rex_of_reg(reg_bits, rm, w).emit(self);
        self.emit_bytes(op);
        self.modrm_reg_tail(reg_bits, rm);
    }

    /// Convenience: pick mem or reg variant.
    fn emit_rm(&mut self, op: &[u8], reg_bits: u8, rm: RmOperand, w: bool) -> Result<(), AssemblerError> {
        match rm {
            RmOperand::Reg(r) => {
                self.emit_rm_reg(op, reg_bits, r, w);
                Ok(())
            }
            RmOperand::Mem(a) => self.emit_rm_mem(op, reg_bits, a, w),
        }
    }

    /// Emit REX prefix for B8+r style (reg in opcode low bits).
    fn emit_rex_b(&mut self, w: bool, reg: Register) {
        let rex = Rex { w, r: false, x: false, b: reg.encoding() >= 8 };
        rex.emit(self);
    }

    /// Emit operand-size prefix (0x66 for 16-bit).
    fn emit_opsz(&mut self, size: OperandSize) {
        if let Some(p) = opsize_prefix(size) {
            self.emit_byte(p);
        }
    }

    // ---- mov family ---------------------------------------------------------

    /// mov r, imm (size-aware).  64-bit uses movabs (REX.W B8+r imm64).
    pub fn mov_imm_reg(&mut self, size: OperandSize, imm: u64, reg: Register) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        match size {
            OperandSize::Size8 => {
                self.emit_rex_b(true, reg);
                self.emit_byte(0xB8 + (reg.encoding() & 7));
                self.emit_qword(imm);
            }
            OperandSize::Size4 => {
                self.emit_rex_b(false, reg);
                self.emit_byte(0xB8 + (reg.encoding() & 7));
                self.emit_dword(imm as u32);
            }
            OperandSize::Size2 => {
                self.emit_rex_b(false, reg);
                self.emit_byte(0xB8 + (reg.encoding() & 7));
                self.emit_word(imm as u16);
            }
            OperandSize::Size1 => {
                let rex = Rex { w: false, r: false, x: false, b: reg.encoding() >= 8 };
                rex.emit(self);
                self.emit_byte(0xB0 + (reg.encoding() & 7));
                self.emit_byte(imm as u8);
            }
        }
        Ok(())
    }

    /// mov dst, src for two registers (88/89 /r).
    pub fn mov_reg_reg(&mut self, size: OperandSize, dst: Register, src: Register) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let op = if size == OperandSize::Size1 { [0x88u8] } else { [0x89u8] };
        self.emit_rm_reg(&op, src.encoding(), dst, w_bit(size));
        Ok(())
    }

    /// mov r64, imm32 sign-extended (REX.W C7 /0).
    pub fn mov_imm_reg_sext(&mut self, size: OperandSize, imm: i32, reg: Register) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        self.emit_rm_reg(&[0xC7], 0, reg, w_bit(size));
        self.emit_dword(imm as u32);
        Ok(())
    }

    /// mov r, r/m (8A/8B /r).
    pub fn mov_reg_rm(&mut self, size: OperandSize, dst: Register, src: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let op = if size == OperandSize::Size1 { [0x8Au8] } else { [0x8Bu8] };
        self.emit_rm(&op, dst.encoding(), src, w_bit(size))
    }

    /// mov r/m, r (88/89 /r).
    pub fn mov_rm_reg(&mut self, size: OperandSize, dst: RmOperand, src: Register) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let op = if size == OperandSize::Size1 { [0x88u8] } else { [0x89u8] };
        self.emit_rm(&op, src.encoding(), dst, w_bit(size))
    }

    /// movq r64, r/m64.
    pub fn movq(&mut self, dst: Register, src: RmOperand) -> Result<(), AssemblerError> {
        self.mov_reg_rm(OperandSize::Size8, dst, src)
    }

    /// movq r/m64, r64.
    pub fn movq_store(&mut self, dst: RmOperand, src: Register) -> Result<(), AssemblerError> {
        self.mov_rm_reg(OperandSize::Size8, dst, src)
    }

    pub fn movl(&mut self, dst: Register, src: RmOperand) -> Result<(), AssemblerError> {
        self.mov_reg_rm(OperandSize::Size4, dst, src)
    }

    pub fn movl_store(&mut self, dst: RmOperand, src: Register) -> Result<(), AssemblerError> {
        self.mov_rm_reg(OperandSize::Size4, dst, src)
    }

    pub fn movw(&mut self, dst: Register, src: RmOperand) -> Result<(), AssemblerError> {
        self.mov_reg_rm(OperandSize::Size2, dst, src)
    }

    pub fn movb(&mut self, dst: Register, src: RmOperand) -> Result<(), AssemblerError> {
        self.mov_reg_rm(OperandSize::Size1, dst, src)
    }

    /// mov m, imm (C6/C7 /0).
    pub fn mov_imm_mem(&mut self, size: OperandSize, dst: RmOperand, imm: i64) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        match size {
            OperandSize::Size1 => {
                self.emit_rm(&[0xC6], 0, dst, false)?;
                match dst {
                    RmOperand::Reg(_) | RmOperand::Mem(_) => self.emit_byte(imm as u8),
                }
            }
            _ => {
                self.emit_rm(&[0xC7], 0, dst, w_bit(size))?;
                self.emit_dword(imm as i32 as u32);
            }
        }
        Ok(())
    }

    /// lea r, m (8D /r, always 64-bit dest).
    pub fn lea(&mut self, dst: Register, addr: Address) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm_mem(&[0x8D], dst.encoding(), addr, true)
    }

    /// movzx: 0F B6 (byte->32), B7 (word->32).
    pub fn movzx(&mut self, dst: Register, src: RmOperand, src_size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        let op = match src_size {
            OperandSize::Size1 => [0x0F, 0xB6u8],
            OperandSize::Size2 => [0x0F, 0xB7u8],
            _ => return Err(AssemblerError::InvalidOperand("movzx src size".into())),
        };
        self.emit_rm(&op, dst.encoding(), src, false)
    }

    /// movzx to 64-bit register (0F BE/BF, REX.W).
    pub fn movzxq(&mut self, dst: Register, src: RmOperand, src_size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        let op = match src_size {
            OperandSize::Size1 => [0x0F, 0xBEu8],
            OperandSize::Size2 => [0x0F, 0xBFu8],
            _ => return Err(AssemblerError::InvalidOperand("movzxq src size".into())),
        };
        self.emit_rm(&op, dst.encoding(), src, true)
    }

    /// movsx: 0F BE/BF with optional REX.W.
    pub fn movsx(&mut self, dst: Register, src: RmOperand, src_size: OperandSize, to64: bool) -> Result<(), AssemblerError> {
        self.inst_begin();
        let op = match src_size {
            OperandSize::Size1 => [0x0F, 0xBEu8],
            OperandSize::Size2 => [0x0F, 0xBFu8],
            _ => return Err(AssemblerError::InvalidOperand("movsx src size".into())),
        };
        self.emit_rm(&op, dst.encoding(), src, to64)
    }

    /// movsxd r64, r/m32 (63 /r, REX.W).
    pub fn movsxd(&mut self, dst: Register, src: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm(&[0x63], dst.encoding(), src, true)
    }

    /// cbw/cwde/cdqe family and cwd/cdq/cqo.
    pub fn cbw(&mut self) { self.inst_begin(); self.emit_byte(0x66); self.emit_byte(0x98); }
    pub fn cwde(&mut self) { self.inst_begin(); self.emit_byte(0x98); }
    pub fn cdqe(&mut self) { self.inst_begin(); self.emit_byte(0x48); self.emit_byte(0x98); }
    pub fn cwd(&mut self) { self.inst_begin(); self.emit_byte(0x66); self.emit_byte(0x99); }
    pub fn cdq(&mut self) { self.inst_begin(); self.emit_byte(0x99); }
    pub fn cqo(&mut self) { self.inst_begin(); self.emit_byte(0x48); self.emit_byte(0x99); }

    /// bswap r (REX.W 0F C8+r).
    pub fn bswap(&mut self, reg: Register) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0x0F);
        let rex = Rex { w: true, r: false, x: false, b: reg.encoding() >= 8 };
        rex.emit(self);
        self.emit_byte(0xC8 + (reg.encoding() & 7));
        Ok(())
    }

    // ---- push / pop ---------------------------------------------------------

    pub fn push(&mut self, reg: Register) -> Result<(), AssemblerError> {
        self.inst_begin();
        let rex = Rex { w: false, r: false, x: false, b: reg.encoding() >= 8 };
        rex.emit(self);
        self.emit_byte(0x50 + (reg.encoding() & 7));
        Ok(())
    }

    pub fn push_imm8(&mut self, imm: i8) {
        self.inst_begin();
        self.emit_byte(0x6A);
        self.emit_byte(imm as u8);
    }

    pub fn push_imm32(&mut self, imm: i32) {
        self.inst_begin();
        self.emit_byte(0x68);
        self.emit_dword(imm as u32);
    }

    pub fn pop(&mut self, reg: Register) -> Result<(), AssemblerError> {
        self.inst_begin();
        let rex = Rex { w: false, r: false, x: false, b: reg.encoding() >= 8 };
        rex.emit(self);
        self.emit_byte(0x58 + (reg.encoding() & 7));
        Ok(())
    }

    pub fn push_mem(&mut self, addr: Address) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm_mem(&[0xFF], 6, addr, false)
    }

    pub fn pop_mem(&mut self, addr: Address) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm_mem(&[0xFF], 0, addr, false)
    }

    pub fn popfq(&mut self) { self.inst_begin(); self.emit_byte(0x9D); }
    pub fn pushfq(&mut self) { self.inst_begin(); self.emit_byte(0x9C); }

    // ---- ALU -----------------------------------------------------------------

    /// rm <- rm OP imm  (80/81/83 /ext).
    fn alu_op_rm_imm(
        &mut self,
        ext: u8,
        dst: RmOperand,
        imm: i64,
        size: OperandSize,
    ) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        if size == OperandSize::Size1 {
            self.emit_rm(&[0x80], ext, dst, w)?;
            self.emit_byte(imm as u8);
            return Ok(());
        }
        if Assembler::is_simm(imm, 8) {
            self.emit_rm(&[0x83], ext, dst, w)?;
            self.emit_byte(imm as i8 as u8);
        } else {
            if !Assembler::is_simm(imm, 32) {
                return Err(AssemblerError::InvalidOperand("imm32 out of range".into()));
            }
            self.emit_rm(&[0x81], ext, dst, w)?;
            self.emit_dword(imm as i32 as u32);
        }
        Ok(())
    }

    /// Generic ALU op: opcode_rm_r = "op r/m, r" form, opcode_r_rm =
    /// "op r, r/m" form.  Byte forms are one less.
    fn alu_generic(
        &mut self,
        opcode_rm_r: u8,
        opcode_r_rm: u8,
        dst: RmOperand,
        src: RmOperand,
        size: OperandSize,
    ) -> Result<(), AssemblerError> {
        let adj = if size == OperandSize::Size1 { 1u8 } else { 0u8 };
        let (op_rm_r, op_r_rm) = (opcode_rm_r - adj, opcode_r_rm - adj);
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        match (dst, src) {
            (RmOperand::Reg(d), RmOperand::Reg(s)) => {
                self.emit_rm_reg(&[op_r_rm], d.encoding(), s, w);
                Ok(())
            }
            (RmOperand::Reg(d), RmOperand::Mem(a)) => {
                self.emit_rm_mem(&[op_r_rm], d.encoding(), a, w)
            }
            (RmOperand::Mem(a), RmOperand::Reg(s)) => {
                self.emit_rm_mem(&[op_rm_r], s.encoding(), a, w)
            }
            (RmOperand::Mem(_), RmOperand::Mem(_)) => {
                Err(AssemblerError::InvalidOperand("mem,mem alu".into()))
            }
        }
    }

    pub fn add(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x01, 0x03, dst, src, size)
    }
    pub fn add_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(0, dst, imm, size)
    }
    pub fn or_op(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x09, 0x0B, dst, src, size)
    }
    pub fn or_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(1, dst, imm, size)
    }
    pub fn adc(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x11, 0x13, dst, src, size)
    }
    pub fn adc_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(2, dst, imm, size)
    }
    pub fn sbb(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x19, 0x1B, dst, src, size)
    }
    pub fn sbb_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(3, dst, imm, size)
    }
    pub fn and_op(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x21, 0x23, dst, src, size)
    }
    pub fn and_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(4, dst, imm, size)
    }
    pub fn sub(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x29, 0x2B, dst, src, size)
    }
    pub fn sub_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(5, dst, imm, size)
    }
    pub fn xor_op(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x31, 0x33, dst, src, size)
    }
    pub fn xor_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(6, dst, imm, size)
    }
    pub fn cmp_op(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_generic(0x39, 0x3B, dst, src, size)
    }
    pub fn cmp_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.alu_op_rm_imm(7, dst, imm, size)
    }

    /// test r/m, r (84/85 /r).
    pub fn test_op(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        let opcode = if size == OperandSize::Size1 { 0x84u8 } else { 0x85u8 };
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        match (dst, src) {
            (RmOperand::Reg(d), RmOperand::Reg(s)) => {
                self.emit_rm_reg(&[opcode], d.encoding(), s, w);
                Ok(())
            }
            (RmOperand::Reg(d), RmOperand::Mem(a)) => self.emit_rm_mem(&[opcode], d.encoding(), a, w),
            (RmOperand::Mem(a), RmOperand::Reg(s)) => self.emit_rm_mem(&[opcode], s.encoding(), a, w),
            (RmOperand::Mem(_), RmOperand::Mem(_)) => {
                Err(AssemblerError::InvalidOperand("mem,mem test".into()))
            }
        }
    }

    /// test r/m, imm (F6/F7 /0).
    pub fn test_imm(&mut self, dst: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        match size {
            OperandSize::Size1 => {
                self.emit_rm(&[0xF6], 0, dst, w)?;
                self.emit_byte(imm as u8);
            }
            _ => {
                self.emit_rm(&[0xF7], 0, dst, w)?;
                self.emit_dword(imm as i32 as u32);
            }
        }
        Ok(())
    }

    // ---- inc/dec/neg/not/mul/div --------------------------------------------

    fn group1(&mut self, ext: u8, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        let op = if size == OperandSize::Size1 { 0xFEu8 } else { 0xFFu8 };
        self.emit_rm(&[op], ext, dst, w)
    }

    pub fn inc(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group1(0, dst, size)
    }
    pub fn dec(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group1(1, dst, size)
    }

    /// call r/m (FF /2).
    pub fn call_rm(&mut self, dst: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm(&[0xFF], 2, dst, false)
    }

    /// jmp r/m (FF /4).
    pub fn jmp_rm(&mut self, dst: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm(&[0xFF], 4, dst, false)
    }

    /// push r/m (FF /6).
    pub fn push_rm(&mut self, dst: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm(&[0xFF], 6, dst, false)
    }

    /// pop r/m (8F /0).
    pub fn pop_rm(&mut self, dst: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm(&[0x8F], 0, dst, false)
    }

    fn group2_f7(&mut self, ext: u8, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        let op = if size == OperandSize::Size1 { 0xF6u8 } else { 0xF7u8 };
        self.emit_rm(&[op], ext, dst, w)
    }

    pub fn neg(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group2_f7(3, dst, size)
    }
    pub fn not(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group2_f7(2, dst, size)
    }
    pub fn mul(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group2_f7(4, dst, size)
    }
    pub fn imul_one(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group2_f7(5, dst, size)
    }
    pub fn div(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group2_f7(6, dst, size)
    }
    pub fn idiv(&mut self, dst: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.group2_f7(7, dst, size)
    }

    /// imul r, r/m (0F AF /r).
    pub fn imul_reg(&mut self, dst: Register, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        self.emit_rm(&[0x0F, 0xAF], dst.encoding(), src, w_bit(size))
    }

    /// imul r, r/m, imm (69 id / 6B ib /r).
    pub fn imul_imm(&mut self, dst: Register, src: RmOperand, imm: i64, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        let use_imm8 = Assembler::is_simm(imm, 8) && size != OperandSize::Size1;
        if use_imm8 {
            self.emit_rm(&[0x6B], dst.encoding(), src, w)?;
            self.emit_byte(imm as i8 as u8);
        } else {
            if !Assembler::is_simm(imm, 32) {
                return Err(AssemblerError::InvalidOperand("imul imm32 out of range".into()));
            }
            self.emit_rm(&[0x69], dst.encoding(), src, w)?;
            self.emit_dword(imm as i32 as u32);
        }
        Ok(())
    }

    // ---- shifts/rotates -------------------------------------------------------

    fn shift(&mut self, ext: u8, dst: RmOperand, count: ShiftCount, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let w = w_bit(size);
        match count {
            ShiftCount::Imm(n) => {
                self.emit_rm(&[0xC1], ext, dst, w)?;
                self.emit_byte(n as u8);
            }
            ShiftCount::Cl => {
                self.emit_rm(&[0xD3], ext, dst, w)?;
            }
            ShiftCount::Reg(_) => {
                return Err(AssemblerError::InvalidOperand("shift by register other than cl".into()));
            }
        }
        Ok(())
    }

    pub fn shl(&mut self, dst: RmOperand, count: ShiftCount, size: OperandSize) -> Result<(), AssemblerError> {
        self.shift(4, dst, count, size)
    }
    pub fn shr(&mut self, dst: RmOperand, count: ShiftCount, size: OperandSize) -> Result<(), AssemblerError> {
        self.shift(5, dst, count, size)
    }
    pub fn sar(&mut self, dst: RmOperand, count: ShiftCount, size: OperandSize) -> Result<(), AssemblerError> {
        self.shift(7, dst, count, size)
    }
    pub fn rol(&mut self, dst: RmOperand, count: ShiftCount, size: OperandSize) -> Result<(), AssemblerError> {
        self.shift(0, dst, count, size)
    }
    pub fn ror(&mut self, dst: RmOperand, count: ShiftCount, size: OperandSize) -> Result<(), AssemblerError> {
        self.shift(1, dst, count, size)
    }

    // ---- branches -------------------------------------------------------------

    /// jmp rel8 (EB cb) to label.
    pub fn jmp_short(&mut self, label: LabelId) -> Result<(), AssemblerError> {
        self.inst_begin();
        if let Some(target) = self.labels.loc(label) {
            let disp_pos = self.pc() + 1;
            let disp = target as i64 - disp_pos as i64 - 1;
            if !(-128..=127).contains(&disp) {
                return Err(AssemblerError::BranchOutOfRange(disp));
            }
            self.emit_byte(0xEB);
            self.emit_byte(disp as i8 as u8);
        } else {
            self.emit_byte(0xEB);
            let disp_off = self.pc();
            self.emit_byte(0);
            self.add_label_patch(label, self.inst_mark(), disp_off, PatchKind::Rel8);
        }
        Ok(())
    }

    /// jmp rel32 (E9 cd) to label.
    pub fn jmp_near(&mut self, label: LabelId) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xE9);
        let disp_off = self.pc();
        self.emit_dword(0);
        if let Some(target) = self.labels.loc(label) {
            let disp = target as i64 - (disp_off as i64 + 4);
            let cur = self.code.current;
            self.code.section(cur)
                .patch(disp_off, &(disp as i32).to_le_bytes())
                .map_err(AssemblerError::CodeBuffer)?;
        } else {
            self.add_label_patch(label, self.inst_mark(), disp_off, PatchKind::Rel32);
        }
        Ok(())
    }

    /// jmp to label choosing short form when the label is bound and fits.
    pub fn jmp(&mut self, label: LabelId) -> Result<(), AssemblerError> {
        if let Some(target) = self.labels.loc(label) {
            let disp_pos = self.pc() + 2;
            let disp = target as i64 - disp_pos as i64;
            if (-128..=127).contains(&disp) {
                return self.jmp_short(label);
            }
        }
        self.jmp_near(label)
    }

    /// jcc rel8 (70+cc cb) to bound label.
    pub fn jcc_short(&mut self, cond: ConditionCode, label: LabelId) -> Result<(), AssemblerError> {
        self.inst_begin();
        if let Some(target) = self.labels.loc(label) {
            let disp_pos = self.pc() + 2;
            let disp = target as i64 - disp_pos as i64;
            if !(-128..=127).contains(&disp) {
                return Err(AssemblerError::BranchOutOfRange(disp));
            }
            self.emit_byte(0x70 | (cond.encoding() & 0xF));
            self.emit_byte(disp as i8 as u8);
        } else {
            self.emit_byte(0x70 | (cond.encoding() & 0xF));
            let disp_off = self.pc();
            self.emit_byte(0);
            self.add_label_patch(label, self.inst_mark(), disp_off, PatchKind::Rel8);
        }
        Ok(())
    }

    /// jcc near (0F 80+cc cd) to label.
    pub fn jcc_near(&mut self, cond: ConditionCode, label: LabelId) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0x0F);
        self.emit_byte(0x80 | (cond.encoding() & 0xF));
        let disp_off = self.pc();
        self.emit_dword(0);
        if let Some(target) = self.labels.loc(label) {
            let disp = target as i64 - (disp_off as i64 + 4);
            let cur = self.code.current;
            self.code.section(cur)
                .patch(disp_off, &(disp as i32).to_le_bytes())
                .map_err(AssemblerError::CodeBuffer)?;
        } else {
            self.add_label_patch(label, self.inst_mark(), disp_off, PatchKind::Rel32);
        }
        Ok(())
    }

    /// jcc to label: short if bound and fits, else near.
    pub fn jcc(&mut self, cond: ConditionCode, label: LabelId) -> Result<(), AssemblerError> {
        if self.labels.is_bound(label) {
            let target = self.labels.loc(label).unwrap();
            let disp_pos = self.pc() + 2;
            let disp = target as i64 - disp_pos as i64;
            if (-128..=127).contains(&disp) {
                return self.jcc_short(cond, label);
            }
        }
        self.jcc_near(cond, label)
    }

    fn loop_jcc(&mut self, opcode: u8, label: LabelId) -> Result<(), AssemblerError> {
        self.inst_begin();
        if let Some(target) = self.labels.loc(label) {
            let disp_pos = self.pc() + 1;
            let disp = target as i64 - disp_pos as i64 - 1;
            if !(-128..=127).contains(&disp) {
                return Err(AssemblerError::BranchOutOfRange(disp));
            }
            self.emit_byte(opcode);
            self.emit_byte(disp as i8 as u8);
        } else {
            self.emit_byte(opcode);
            let disp_off = self.pc();
            self.emit_byte(0);
            self.add_label_patch(label, self.inst_mark(), disp_off, PatchKind::Rel8);
        }
        Ok(())
    }

    pub fn loop_(&mut self, label: LabelId) -> Result<(), AssemblerError> { self.loop_jcc(0xE2, label) }
    pub fn loope(&mut self, label: LabelId) -> Result<(), AssemblerError> { self.loop_jcc(0xE0, label) }
    pub fn loopne(&mut self, label: LabelId) -> Result<(), AssemblerError> { self.loop_jcc(0xE1, label) }
    pub fn jrcxz(&mut self, label: LabelId) -> Result<(), AssemblerError> { self.loop_jcc(0xE3, label) }

    // ---- calls / ret -----------------------------------------------------------

    /// call rel32 (E8 cd) to bound or unbound label (intra-blob).
    pub fn call_label(&mut self, label: LabelId) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xE8);
        let disp_off = self.pc();
        self.emit_dword(0);
        if let Some(target) = self.labels.loc(label) {
            let disp = target as i64 - (disp_off as i64 + 4);
            let cur = self.code.current;
            self.code.section(cur)
                .patch(disp_off, &(disp as i32).to_le_bytes())
                .map_err(AssemblerError::CodeBuffer)?;
        } else {
            self.add_label_patch(label, self.inst_mark(), disp_off, PatchKind::Rel32);
        }
        Ok(())
    }

    /// call r/m (FF /2).
    pub fn call(&mut self, dst: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rm(&[0xFF], 2, dst, false)
    }

    /// ret [imm16].
    pub fn ret(&mut self) {
        self.inst_begin();
        self.emit_byte(0xC3);
    }

    pub fn ret_imm(&mut self, imm: u16) {
        self.inst_begin();
        self.emit_byte(0xC3);
        self.emit_word(imm);
    }

    // ---- misc ------------------------------------------------------------------

    pub fn nop(&mut self) { self.inst_begin(); self.emit_byte(0x90); }
    pub fn pause(&mut self) { self.inst_begin(); self.emit_byte(0xF3); self.emit_byte(0x90); }
    pub fn int3(&mut self) { self.inst_begin(); self.emit_byte(0xCC); }
    /// os::breakpoint equivalent at asm level.
    pub fn breakpoint(&mut self) { self.int3(); }
    pub fn int_n(&mut self, n: u8) { self.inst_begin(); self.emit_byte(0xCD); self.emit_byte(n); }
    pub fn ud2(&mut self) { self.inst_begin(); self.emit_byte(0x0F); self.emit_byte(0x0B); }
    pub fn syscall(&mut self) { self.inst_begin(); self.emit_byte(0x0F); self.emit_byte(0x05); }
    pub fn cpuid(&mut self) { self.inst_begin(); self.emit_byte(0x0F); self.emit_byte(0xA2); }
    pub fn rdtsc(&mut self) { self.inst_begin(); self.emit_byte(0x0F); self.emit_byte(0x31); }
    pub fn lfence(&mut self) { self.inst_begin(); self.emit_byte(0x0F); self.emit_byte(0xAE); self.emit_byte(0xE8); }
    pub fn mfence(&mut self) { self.inst_begin(); self.emit_byte(0x0F); self.emit_byte(0xAE); self.emit_byte(0xF0); }
    pub fn sfence(&mut self) { self.inst_begin(); self.emit_byte(0x0F); self.emit_byte(0xAE); self.emit_byte(0xF8); }
    pub fn leave(&mut self) { self.inst_begin(); self.emit_byte(0xC9); }
    pub fn enter(&mut self, frame_size: u16, nesting: u8) {
        self.inst_begin();
        self.emit_byte(0xC8);
        self.emit_word(frame_size);
        self.emit_byte(nesting);
    }

    /// setcc: 0F 90+cc /r on r/m8 (modrm.reg unused = 0).
    pub fn setcc(&mut self, cond: ConditionCode, dst: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        let op = [0x0F, 0x90u8 | (cond.encoding() & 0xF)];
        self.emit_rm(&op, 0, dst, false)
    }

    /// cmovcc: 0F 40+cc /r, reg field = dst.
    pub fn cmov(&mut self, cond: ConditionCode, dst: Register, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let op = [0x0F, 0x40u8 | (cond.encoding() & 0xF)];
        self.emit_rm(&op, dst.encoding(), src, w_bit(size))
    }

    /// xchg r/m, r (87 /r) and xchg rax, r (90+r).
    pub fn xchg(&mut self, a: RmOperand, b: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        if let (RmOperand::Reg(d), RmOperand::Reg(s)) = (a, b) {
            if d == Register::RAX || s == Register::RAX {
                self.inst_begin();
                self.emit_opsz(size);
                let other = if d == Register::RAX { s } else { d };
                let rex = Rex { w: w_bit(size), r: false, x: false, b: other.encoding() >= 8 };
                rex.emit(self);
                self.emit_byte(0x90 + (other.encoding() & 7));
                return Ok(());
            }
        }
        self.inst_begin();
        self.emit_opsz(size);
        match (a, b) {
            (RmOperand::Reg(d), RmOperand::Reg(s)) => {
                self.emit_rm_reg(&[0x87], d.encoding(), s, w_bit(size));
                Ok(())
            }
            (RmOperand::Mem(m), RmOperand::Reg(s)) => self.emit_rm_mem(&[0x87], s.encoding(), m, w_bit(size)),
            (RmOperand::Reg(d), RmOperand::Mem(m)) => self.emit_rm_mem(&[0x87], d.encoding(), m, w_bit(size)),
            (RmOperand::Mem(_), RmOperand::Mem(_)) => {
                Err(AssemblerError::InvalidOperand("xchg mem,mem".into()))
            }
        }
    }

    /// cmpxchg r/m, r (0F B0/B1).
    pub fn cmpxchg(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let op = [0x0F, if size == OperandSize::Size1 { 0xB0u8 } else { 0xB1u8 }];
        match (dst, src) {
            (RmOperand::Reg(d), RmOperand::Reg(s)) => {
                self.emit_rm_reg(&op, d.encoding(), s, w_bit(size));
                Ok(())
            }
            (RmOperand::Mem(m), RmOperand::Reg(s)) => self.emit_rm_mem(&op, s.encoding(), m, w_bit(size)),
            _ => Err(AssemblerError::InvalidOperand("cmpxchg operands".into())),
        }
    }

    /// xadd r/m, r (0F C0/C1).
    pub fn xadd(&mut self, dst: RmOperand, src: RmOperand, size: OperandSize) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_opsz(size);
        let op = [0x0F, if size == OperandSize::Size1 { 0xC0u8 } else { 0xC1u8 }];
        match (dst, src) {
            (RmOperand::Reg(d), RmOperand::Reg(s)) => {
                self.emit_rm_reg(&op, d.encoding(), s, w_bit(size));
                Ok(())
            }
            (RmOperand::Mem(m), RmOperand::Reg(s)) => self.emit_rm_mem(&op, s.encoding(), m, w_bit(size)),
            _ => Err(AssemblerError::InvalidOperand("xadd operands".into())),
        }
    }

    // ---- SSE moves (minimal) ---------------------------------------------------

    /// movsd xmm, xmm/m64 (F2 0F 10 /r).
    pub fn movsd(&mut self, dst: FloatRegister, src: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xF2);
        self.emit_rm(&[0x0F, 0x10], dst.encoding(), src, false)
    }

    /// movsd xmm/m64, xmm (F2 0F 11 /r, reg field = src xmm).
    pub fn movsd_store(&mut self, dst: RmOperand, src: FloatRegister) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xF2);
        self.emit_rm(&[0x0F, 0x11], src.encoding(), dst, false)
    }

    /// movss xmm, xmm/m32 (F3 0F 10 /r).
    pub fn movss(&mut self, dst: FloatRegister, src: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xF3);
        self.emit_rm(&[0x0F, 0x10], dst.encoding(), src, false)
    }

    /// movss xmm/m32, xmm (F3 0F 11 /r).
    pub fn movss_store(&mut self, dst: RmOperand, src: FloatRegister) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xF3);
        self.emit_rm(&[0x0F, 0x11], src.encoding(), dst, false)
    }

    /// movq xmm, r/m64 (66 REX.W 0F 6E /r).
    pub fn movq_xmm_gpr(&mut self, xmm: FloatRegister, src: RmOperand) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0x66);
        self.emit_rm_w(&[0x0F, 0x6E], xmm.encoding(), src)
    }

    /// movq r/m64, xmm (66 REX.W 0F 7E /r).
    pub fn movq_gpr_xmm(&mut self, dst: RmOperand, xmm: FloatRegister) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0x66);
        self.emit_rm_w(&[0x0F, 0x7E], xmm.encoding(), dst)
    }

    /// emit_rm variant with REX.W forced (for movq xmm<->gpr).
    fn emit_rm_w(&mut self, op: &[u8], reg_bits: u8, rm: RmOperand) -> Result<(), AssemblerError> {
        match rm {
            RmOperand::Reg(r) => {
                self.rex_of_reg(reg_bits, r, true).emit(self);
                self.emit_bytes(op);
                self.modrm_reg_tail(reg_bits, r);
                Ok(())
            }
            RmOperand::Mem(a) => self.emit_rm_mem_w(op, reg_bits, a),
        }
    }

    fn emit_rm_mem_w(&mut self, op: &[u8], reg_bits: u8, addr: Address) -> Result<(), AssemblerError> {
        self.rex_of_mem(reg_bits, &addr, true).emit(self);
        self.emit_bytes(op);
        self.modrm_mem_tail(reg_bits, &addr)
    }

    // ---- reloc-recording call helpers -----------------------------------------

    /// call rel32 with external_word relocation (fixed runtime address).
    pub fn call_external(&mut self, name: &str, addend: i32) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xE8);
        let disp_off = self.pc();
        self.emit_dword(0);
        let inst = self.inst_mark();
        self.relocate_at(
            inst,
            RelocType::RuntimeCall,
            RelocWidth::Word32,
            RelocValue::External { name: name.to_string(), addend },
            (disp_off - inst) as u32,
        );
        Ok(())
    }

    /// call rel32 with internal_word relocation (target inside blob).
    pub fn call_internal(&mut self, target_pc: u64, addend: i32) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xE8);
        let disp_off = self.pc();
        self.emit_dword(0);
        let inst = self.inst_mark();
        self.relocate_at(
            inst,
            RelocType::InternalWord,
            RelocWidth::Word32,
            RelocValue::Internal { target: target_pc, addend },
            (disp_off - inst) as u32,
        );
        Ok(())
    }

    /// jmp rel32 with internal_word relocation.
    pub fn jmp_internal(&mut self, target_pc: u64, addend: i32) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_byte(0xE9);
        let disp_off = self.pc();
        self.emit_dword(0);
        let inst = self.inst_mark();
        self.relocate_at(
            inst,
            RelocType::InternalWord,
            RelocWidth::Word32,
            RelocValue::Internal { target: target_pc, addend },
            (disp_off - inst) as u32,
        );
        Ok(())
    }

    /// mov r64, imm64 absolute with external_word reloc (movabs).
    pub fn mov_imm64_external(&mut self, reg: Register, name: &str, addend: i32) -> Result<(), AssemblerError> {
        self.inst_begin();
        self.emit_rex_b(true, reg);
        self.emit_byte(0xB8 + (reg.encoding() & 7));
        let imm_off = self.pc();
        self.emit_qword(0);
        let inst = self.inst_mark();
        self.relocate_at(
            inst,
            RelocType::ExternalWord,
            RelocWidth::Word64,
            RelocValue::External { name: name.to_string(), addend },
            (imm_off - inst) as u32,
        );
        Ok(())
    }

    /// Record an oop reloc for a 32-bit embedded field just emitted.
    pub fn record_oop(&mut self, oop: u64) {
        self.relocate_here(RelocType::Oop, RelocWidth::Word32, RelocValue::Oop(oop));
    }

    /// Record a metadata reloc for a 32-bit embedded field just emitted.
    pub fn record_metadata(&mut self, meta: u64) {
        self.relocate_here(RelocType::Metadata, RelocWidth::Word32, RelocValue::Metadata(meta));
    }

    /// Record an external word reloc for the just-emitted field.
    pub fn record_external_word(&mut self, name: &str, addend: i32, width: RelocWidth) {
        self.relocate_here(
            RelocType::ExternalWord,
            width,
            RelocValue::External { name: name.to_string(), addend },
        );
    }

    /// Record an internal word reloc for the just-emitted field.
    pub fn record_internal_word(&mut self, target_pc: u64, addend: i32, width: RelocWidth) {
        self.relocate_here(
            RelocType::InternalWord,
            width,
            RelocValue::Internal { target: target_pc, addend },
        );
    }

    /// Patch a previously emitted field with a label target.
    pub fn patch_label_abs32(&mut self, label: LabelId, _inst_offset: usize, field_offset: usize) -> Result<(), AssemblerError> {
        self.patch_label(label, _inst_offset, field_offset, PatchKind::Abs32)
    }

    /// RelocSpec helper for label-carried relocs.
    pub fn reloc_spec(rtype: RelocType, width: RelocWidth, value: RelocValue) -> RelocSpec {
        RelocSpec { rtype, width, value }
    }
}

/// Microsoft x64 calling convention helpers (windows_abi).
pub mod win64 {
    use crate::share::asm::register::{FloatRegister, Register};

    /// Shadow space (home space) reserved by the caller, 32 bytes.
    pub const SHADOW_SPACE: usize = 32;
    /// Maximum number of integer/pointer register arguments.
    pub const MAX_INT_REG_ARGS: usize = 4;
    /// Maximum number of vector register arguments.
    pub const MAX_XMM_REG_ARGS: usize = 4;

    /// Integer argument registers in order: rcx, rdx, r8, r9.
    pub const ARG_REGS: [Register; 4] = [
        Register::RCX,
        Register::RDX,
        Register::R8,
        Register::R9,
    ];

    /// Vector argument registers in order: xmm0..xmm3.
    pub const XMM_ARG_REGS: [FloatRegister; 4] = [
        FloatRegister::XMM0,
        FloatRegister::XMM1,
        FloatRegister::XMM2,
        FloatRegister::XMM3,
    ];

    /// Return address slot size pushed by `call`.
    pub const RETURN_SLOT: usize = 8;

    /// Offset (from callee rsp at entry) of the home space for arg i.
    pub fn home_offset(i: usize) -> usize {
        RETURN_SLOT + 8 * i
    }

    /// Offset (from caller rsp before `call`) of stack arg i (i >= 4).
    pub fn stack_arg_offset(i: usize) -> usize {
        SHADOW_SPACE + 8 * (i - MAX_INT_REG_ARGS)
    }

    /// Offset (from callee rsp at entry) of stack arg i (i >= 4).
    pub fn stack_arg_offset_callee(i: usize) -> usize {
        RETURN_SLOT + SHADOW_SPACE + 8 * (i - MAX_INT_REG_ARGS)
    }

    /// Register holding integer arg i, or None when it goes on the stack.
    pub fn int_arg_reg(i: usize) -> Option<Register> {
        ARG_REGS.get(i).copied()
    }

    /// Register holding vector arg i, or None when on the stack.
    pub fn xmm_arg_reg(i: usize) -> Option<FloatRegister> {
        XMM_ARG_REGS.get(i).copied()
    }

    /// Number of 16-byte slots of shadow space the caller must allocate.
    pub const SHADOW_SPACE_SLOTS: usize = SHADOW_SPACE / 16;
}
