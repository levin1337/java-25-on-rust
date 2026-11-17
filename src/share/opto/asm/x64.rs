/*
 * Port of OpenJDK 25 HotSpot C2 x86-64 instruction selector backend
 * (share/opto/x86/instr_x86.cpp, macroAssembler_x86.cpp) reduced to the
 * executable primitive subset: a byte-level assembler over CodeBuffer.
 *
 * This is the C2-owned emitter: it does not share c1/asm/x86.rs.  It
 * provides the raw encodings (REX/modrm/SIB, mov/alu/shift/setcc, SSE,
 * branches, calls) that the ideal-graph matcher in codegen.rs composes
 * into scheduled basic blocks.
 */

use super::code_buffer::{CodeBuffer, CodeBufferError};
use super::register::{Register, Scale, XMMRegister};

#[derive(Clone, Copy)]
pub enum AluOp {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy)]
pub enum ShiftOp {
    Shl,
    Shr,
    Sar,
}

pub struct X64Asm<'a> {
    pub buf: &'a CodeBuffer,
}

impl<'a> X64Asm<'a> {
    pub fn new(buf: &'a CodeBuffer) -> Self {
        Self { buf }
    }

    pub fn pos(&self) -> usize {
        self.buf.position()
    }

    pub fn emit_rex(&mut self, w: bool, r: bool, x: bool, b: bool) {
        let bits = ((w as u8) << 3) | ((r as u8) << 2) | ((x as u8) << 1) | b as u8;
        if bits != 0 {
            let _ = self.buf.write_byte(0x40 | bits);
        }
    }

    pub fn rex_bits(&self, reg: Register, rm: Register, w: bool) -> (bool, bool, bool, bool) {
        (w, reg.needs_rex(), false, rm.needs_rex())
    }

    pub fn emit_modrm_sib(
        &mut self,
        reg: u8,
        base: Option<Register>,
        index: Option<Register>,
        scale: Scale,
        disp: i32,
    ) -> Result<(), CodeBufferError> {
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

    pub fn rex_mem(
        &self,
        reg: u8,
        base: Option<Register>,
        index: Option<Register>,
        w: bool,
    ) -> (bool, bool, bool) {
        (
            w,
            reg >= 8,
            base.map(|b| b.needs_rex()).unwrap_or(false)
                || index.map(|i| i.needs_rex()).unwrap_or(false),
        )
    }

    pub fn mov_imm(&mut self, reg: Register, imm: i64) -> Result<(), CodeBufferError> {
        self.emit_rex(true, false, false, reg.needs_rex());
        self.buf.write_byte(0xb8 + (reg.encoding() & 7))?;
        self.buf.write_i64(imm)?;
        Ok(())
    }

    pub fn mov_imm32(&mut self, reg: Register, imm: i32) -> Result<(), CodeBufferError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write_byte(0xb8 + (reg.encoding() & 7))?;
        self.buf.write_i32(imm)?;
        Ok(())
    }

    pub fn mov_reg_reg(&mut self, dst: Register, src: Register, wide: bool) -> Result<(), CodeBufferError> {
        let (w, r, _, b) = self.rex_bits(dst, src, wide);
        self.emit_rex(w, r, false, b);
        self.buf.write(&[0x8b, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    pub fn mov_reg_reg8(&mut self, dst: Register, src: Register) -> Result<(), CodeBufferError> {
        self.emit_rex(false, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0x8a, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    pub fn movsxd(&mut self, dst: Register, src: Register) -> Result<(), CodeBufferError> {
        self.emit_rex(true, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0x63, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    pub fn mov_reg_mem(
        &mut self,
        reg: Register,
        base: Option<Register>,
        index: Option<Register>,
        scale: Scale,
        disp: i32,
        wide: bool,
    ) -> Result<(), CodeBufferError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), base, index, wide);
        self.emit_rex(w, r, false, b);
        self.buf.write_byte(0x8b)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    pub fn mov_mem_reg(
        &mut self,
        base: Option<Register>,
        index: Option<Register>,
        scale: Scale,
        disp: i32,
        reg: Register,
        wide: bool,
    ) -> Result<(), CodeBufferError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), base, index, wide);
        self.emit_rex(w, r, false, b);
        self.buf.write_byte(0x89)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    pub fn mov_mem_reg8(
        &mut self,
        base: Option<Register>,
        index: Option<Register>,
        scale: Scale,
        disp: i32,
        reg: Register,
    ) -> Result<(), CodeBufferError> {
        let (_, r, b) = self.rex_mem(reg.encoding(), base, index, false);
        self.emit_rex(false, r, false, b);
        self.buf.write_byte(0x88)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    pub fn mov_mem_reg16(
        &mut self,
        base: Option<Register>,
        index: Option<Register>,
        scale: Scale,
        disp: i32,
        reg: Register,
    ) -> Result<(), CodeBufferError> {
        let (_, r, b) = self.rex_mem(reg.encoding(), base, index, false);
        self.buf.write_byte(0x66)?;
        self.emit_rex(false, r, false, b);
        self.buf.write_byte(0x89)?;
        self.emit_modrm_sib(reg.encoding() & 7, base, index, scale, disp)
    }

    pub fn movsx_mem_reg(
        &mut self,
        reg: Register,
        base: Register,
        disp: i32,
        word: bool,
    ) -> Result<(), CodeBufferError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), Some(base), None, true);
        self.emit_rex(w, r, false, b);
        self.buf.write(&[0x0f, if word { 0xbf } else { 0xbe }])?;
        self.emit_modrm_sib(reg.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    pub fn movzx_mem_reg(
        &mut self,
        reg: Register,
        base: Register,
        disp: i32,
        word: bool,
    ) -> Result<(), CodeBufferError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), Some(base), None, true);
        self.emit_rex(w, r, false, b);
        self.buf.write(&[0x0f, if word { 0xb7 } else { 0xb6 }])?;
        self.emit_modrm_sib(reg.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    pub fn cmp_rr(&mut self, a: Register, b: Register, wide: bool) -> Result<(), CodeBufferError> {
        let (w, r, _, bb) = self.rex_bits(b, a, wide);
        self.emit_rex(w, r, false, bb);
        self.buf.write(&[0x39, 0xc0 | ((b.encoding() & 7) << 3) | (a.encoding() & 7)])?;
        Ok(())
    }

    pub fn test_rr(&mut self, a: Register, b: Register, wide: bool) -> Result<(), CodeBufferError> {
        let (w, r, _, bb) = self.rex_bits(a, b, wide);
        self.emit_rex(w, r, false, bb);
        self.buf.write(&[0x85, 0xc0 | ((b.encoding() & 7) << 3) | (a.encoding() & 7)])?;
        Ok(())
    }

    pub fn cmp_imm8(&mut self, reg: Register, imm: i8, wide: bool) -> Result<(), CodeBufferError> {
        self.emit_rex(wide, false, false, reg.needs_rex());
        self.buf.write(&[0x83, 0xf8 | (reg.encoding() & 7), imm as u8])?;
        Ok(())
    }

    pub fn cmp_imm32(&mut self, reg: Register, imm: i32) -> Result<(), CodeBufferError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write(&[0x81, 0xf8 | (reg.encoding() & 7)])?;
        self.buf.write_i32(imm)?;
        Ok(())
    }

    pub fn cmp_eax_imm32(&mut self, imm: i32) -> Result<(), CodeBufferError> {
        self.emit_rex(false, false, false, false);
        self.buf.write(&[0x3d])?;
        self.buf.write_i32(imm)?;
        Ok(())
    }

    pub fn alu_imm32(&mut self, alu: AluOp, reg: Register, imm: i32, wide: bool) -> Result<(), CodeBufferError> {
        let ext = match alu {
            AluOp::Add => 0,
            AluOp::Sub => 5,
            AluOp::And => 4,
            AluOp::Or => 1,
            AluOp::Xor => 6,
            AluOp::Mul | AluOp::Div => return Err(CodeBufferError::UnsupportedOp("alu_imm32 op")),
        };
        self.emit_rex(wide, false, false, reg.needs_rex());
        self.buf.write(&[0x81, 0xc0 | (ext << 3) | (reg.encoding() & 7)])?;
        self.buf.write_i32(imm)?;
        Ok(())
    }

    pub fn alu_rr(&mut self, alu: AluOp, dst: Register, src: Register, wide: bool) -> Result<(), CodeBufferError> {
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

    pub fn shift_op(&mut self, shift: ShiftOp, wide: bool) -> Result<(), CodeBufferError> {
        let ext = match shift {
            ShiftOp::Shl => 4,
            ShiftOp::Shr => 5,
            ShiftOp::Sar => 7,
        };
        self.emit_rex(wide, false, false, false);
        self.buf.write(&[0xd3, 0xc0 | (ext << 3) | (Register::RAX.encoding() & 7)])?;
        Ok(())
    }

    pub fn shift_imm(&mut self, reg: Register, imm: u8, shift: ShiftOp, wide: bool) -> Result<(), CodeBufferError> {
        let ext = match shift {
            ShiftOp::Shl => 4,
            ShiftOp::Shr => 5,
            ShiftOp::Sar => 7,
        };
        self.emit_rex(wide, false, false, reg.needs_rex());
        self.buf.write(&[0xc1, 0xc0 | (ext << 3) | (reg.encoding() & 7), imm])?;
        Ok(())
    }

    pub fn cqo(&mut self, wide: bool) -> Result<(), CodeBufferError> {
        if wide {
            self.emit_rex(true, false, false, false);
        }
        self.buf.write_byte(0x99)?;
        Ok(())
    }

    pub fn idiv(&mut self, reg: Register, wide: bool) -> Result<(), CodeBufferError> {
        self.emit_rex(wide, false, false, reg.needs_rex());
        self.buf.write(&[0xf7, 0xf8 | (reg.encoding() & 7)])?;
        Ok(())
    }

    pub fn neg(&mut self, reg: Register, wide: bool) -> Result<(), CodeBufferError> {
        self.emit_rex(wide, false, false, reg.needs_rex());
        self.buf.write(&[0xf7, 0xd8 | (reg.encoding() & 7)])?;
        Ok(())
    }

    pub fn emit_setcc(&mut self, cond: super::register::Condition, reg: Register) -> Result<(), CodeBufferError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write(&[0x0f, 0x90 | cond as u8, 0xc0 | (reg.encoding() & 7)])?;
        Ok(())
    }

    pub fn movzx_reg(&mut self, reg: Register) -> Result<(), CodeBufferError> {
        self.emit_rex(false, reg.needs_rex(), false, reg.needs_rex());
        self.buf.write(&[0x0f, 0xb6, 0xc0 | (reg.encoding() & 7)])?;
        Ok(())
    }

    pub fn movzx_reg16(&mut self, reg: Register) -> Result<(), CodeBufferError> {
        self.emit_rex(false, reg.needs_rex(), false, reg.needs_rex());
        self.buf.write(&[0x0f, 0xb7, 0xc0 | (reg.encoding() & 7)])?;
        Ok(())
    }

    pub fn emit_abs_call(&mut self, addr: u64) -> Result<(), CodeBufferError> {
        self.mov_imm(Register::RAX, addr as i64)?;
        self.buf.write(&[0xff, 0xd0])?;
        Ok(())
    }

    pub fn emit_push(&mut self, reg: Register) -> Result<(), CodeBufferError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write_byte(0x50 + (reg.encoding() & 7))?;
        Ok(())
    }

    pub fn emit_pop(&mut self, reg: Register) -> Result<(), CodeBufferError> {
        self.emit_rex(false, false, false, reg.needs_rex());
        self.buf.write_byte(0x58 + (reg.encoding() & 7))?;
        Ok(())
    }

    pub fn mov_xmm_xmm(&mut self, dst: XMMRegister, src: XMMRegister, double: bool) -> Result<(), CodeBufferError> {
        self.buf.write_byte(if double { 0xf2 } else { 0xf3 })?;
        self.emit_rex(double, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0x0f, 0x10, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    pub fn movd_xmm_from_gp(&mut self, xmm: XMMRegister, gp: Register) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0x66)?;
        self.emit_rex(false, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x0f, 0x6e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    pub fn movq_xmm_from_gp(&mut self, xmm: XMMRegister, gp: Register) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0x66)?;
        self.emit_rex(true, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x0f, 0x6e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    pub fn movq_xmm_to_gp(&mut self, gp: Register, xmm: XMMRegister) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0x66)?;
        self.emit_rex(true, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x0f, 0x7e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    pub fn movd_xmm_to_gp(&mut self, gp: Register, xmm: XMMRegister) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0x66)?;
        self.emit_rex(false, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x0f, 0x7e, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    pub fn cvtsi2ss(&mut self, xmm: XMMRegister, gp: Register, wide: bool) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0xf3)?;
        self.emit_rex(wide, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x0f, 0x2a, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    pub fn cvtsi2sd(&mut self, xmm: XMMRegister, gp: Register, wide: bool) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0xf2)?;
        self.emit_rex(wide, xmm.needs_rex(), false, gp.needs_rex());
        self.buf.write(&[0x0f, 0x2a, 0xc0 | ((xmm.encoding() & 7) << 3) | (gp.encoding() & 7)])?;
        Ok(())
    }

    pub fn cvtss2sd(&mut self, dst: XMMRegister, src: XMMRegister) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0xf3)?;
        self.emit_rex(false, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0x0f, 0x5a, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    pub fn cvtsd2ss(&mut self, dst: XMMRegister, src: XMMRegister) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0xf2)?;
        self.emit_rex(false, dst.needs_rex(), false, src.needs_rex());
        self.buf.write(&[0x0f, 0x5a, 0xc0 | ((dst.encoding() & 7) << 3) | (src.encoding() & 7)])?;
        Ok(())
    }

    pub fn cvttsd2si(&mut self, gp: Register, xmm: XMMRegister, wide: bool) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0xf2)?;
        self.emit_rex(wide, gp.needs_rex(), false, xmm.needs_rex());
        self.buf.write(&[0x0f, 0x2c, 0xc0 | ((gp.encoding() & 7) << 3) | (xmm.encoding() & 7)])?;
        Ok(())
    }

    pub fn cvttss2si(&mut self, gp: Register, xmm: XMMRegister, wide: bool) -> Result<(), CodeBufferError> {
        self.buf.write_byte(0xf3)?;
        self.emit_rex(wide, gp.needs_rex(), false, xmm.needs_rex());
        self.buf.write(&[0x0f, 0x2c, 0xc0 | ((gp.encoding() & 7) << 3) | (xmm.encoding() & 7)])?;
        Ok(())
    }

    pub fn sse_load_mem(&mut self, xmm: XMMRegister, base: Register, disp: i32, double: bool) -> Result<(), CodeBufferError> {
        self.buf.write_byte(if double { 0xf2 } else { 0xf3 })?;
        self.emit_rex(double, xmm.needs_rex(), false, base.needs_rex());
        self.buf.write(&[0x0f, 0x10])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    pub fn sse_store_mem(&mut self, xmm: XMMRegister, base: Register, disp: i32, double: bool) -> Result<(), CodeBufferError> {
        self.buf.write_byte(if double { 0xf2 } else { 0xf3 })?;
        self.emit_rex(double, xmm.needs_rex(), false, base.needs_rex());
        self.buf.write(&[0x0f, 0x11])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), None, Scale::S1, disp)
    }

    pub fn sse_load_mem_idx(
        &mut self,
        xmm: XMMRegister,
        base: Register,
        index: Register,
        scale: Scale,
        disp: i32,
        double: bool,
    ) -> Result<(), CodeBufferError> {
        self.buf.write_byte(if double { 0xf2 } else { 0xf3 })?;
        self.emit_rex(double, xmm.needs_rex(), true, base.needs_rex());
        self.buf.write(&[0x0f, 0x10])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), Some(index), scale, disp)
    }

    pub fn sse_store_mem_idx(
        &mut self,
        xmm: XMMRegister,
        base: Register,
        index: Register,
        scale: Scale,
        disp: i32,
        double: bool,
    ) -> Result<(), CodeBufferError> {
        self.buf.write_byte(if double { 0xf2 } else { 0xf3 })?;
        self.emit_rex(double, xmm.needs_rex(), true, base.needs_rex());
        self.buf.write(&[0x0f, 0x11])?;
        self.emit_modrm_sib(xmm.encoding() & 7, Some(base), Some(index), scale, disp)
    }

    pub fn mov_mem_reg_idx(
        &mut self,
        base: Register,
        index: Register,
        scale: Scale,
        disp: i32,
        reg: Register,
        wide: bool,
    ) -> Result<(), CodeBufferError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), Some(base), Some(index), wide);
        self.emit_rex(w, r, true, b);
        self.buf.write_byte(0x89)?;
        self.emit_modrm_sib(reg.encoding() & 7, Some(base), Some(index), scale, disp)
    }

    pub fn mov_reg_mem_idx(
        &mut self,
        reg: Register,
        base: Register,
        index: Register,
        scale: Scale,
        disp: i32,
        wide: bool,
    ) -> Result<(), CodeBufferError> {
        let (w, r, b) = self.rex_mem(reg.encoding(), Some(base), Some(index), wide);
        self.emit_rex(w, r, true, b);
        self.buf.write_byte(0x8b)?;
        self.emit_modrm_sib(reg.encoding() & 7, Some(base), Some(index), scale, disp)
    }

    pub fn sse_arith(&mut self, alu: AluOp, double: bool) -> Result<(), CodeBufferError> {
        let (prefix, op) = match (alu, double) {
            (AluOp::Add, false) => (0xf3u8, 0x58u8),
            (AluOp::Add, true) => (0xf2, 0x58),
            (AluOp::Sub, false) => (0xf3, 0x5c),
            (AluOp::Sub, true) => (0xf2, 0x5c),
            (AluOp::Mul, false) => (0xf3, 0x59),
            (AluOp::Mul, true) => (0xf2, 0x59),
            (AluOp::Div, false) => (0xf3, 0x5e),
            (AluOp::Div, true) => (0xf2, 0x5e),
            _ => return Err(CodeBufferError::UnsupportedOp("sse_arith op")),
        };
        self.buf.write_byte(prefix)?;
        self.emit_rex(double, false, false, XMMRegister::XMM1.needs_rex());
        self.buf.write(&[0x0f, op, 0xc1])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::code_buffer::CodeBuffer;
    use super::*;

    fn emit(f: impl FnOnce(&mut X64Asm)) -> Vec<u8> {
        let buf = CodeBuffer::new(4096).unwrap();
        {
            let mut asm = X64Asm::new(&buf);
            f(&mut asm);
        }
        buf.code_slice().to_vec()
    }

    #[test]
    fn mov_imm32_encoding() {
        assert_eq!(
            emit(|a| a.mov_imm32(Register::RAX, 0x11223344).unwrap()),
            vec![0xb8, 0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(
            emit(|a| a.mov_imm32(Register::R8, 7).unwrap()),
            vec![0x41, 0xb8, 0x07, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn mov_imm64_encoding() {
        assert_eq!(
            emit(|a| a.mov_imm(Register::RAX, 0x1122334455667788).unwrap()),
            vec![0x48, 0xb8, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
        );
    }

    #[test]
    fn alu_add_encoding() {
        assert_eq!(
            emit(|a| a.alu_rr(AluOp::Add, Register::RAX, Register::RCX, true).unwrap()),
            vec![0x48, 0x03, 0xc1]
        );
        assert_eq!(
            emit(|a| a.alu_rr(AluOp::Add, Register::R8, Register::RCX, true).unwrap()),
            vec![0x4c, 0x03, 0xc1]
        );
        assert_eq!(
            emit(|a| a.alu_rr(AluOp::Xor, Register::RAX, Register::RAX, false).unwrap()),
            vec![0x33, 0xc0]
        );
    }

    #[test]
    fn shift_and_idiv_encoding() {
        assert_eq!(
            emit(|a| a.shift_imm(Register::RDX, 2, ShiftOp::Shl, true).unwrap()),
            vec![0x48, 0xc1, 0xe2, 0x02]
        );
        assert_eq!(
            emit(|a| a.idiv(Register::RCX, false).unwrap()),
            vec![0xf7, 0xf9]
        );
        assert_eq!(
            emit(|a| a.cqo(true).unwrap()),
            vec![0x48, 0x99]
        );
        assert_eq!(
            emit(|a| a.cqo(false).unwrap()),
            vec![0x99]
        );
    }

    #[test]
    fn test_and_cmp_encoding() {
        assert_eq!(
            emit(|a| a.test_rr(Register::RAX, Register::RAX, true).unwrap()),
            vec![0x48, 0x85, 0xc0]
        );
        assert_eq!(
            emit(|a| a.cmp_imm8(Register::RCX, -1, true).unwrap()),
            vec![0x48, 0x83, 0xf9, 0xff]
        );
        assert_eq!(
            emit(|a| a.cmp_rr(Register::RCX, Register::RDX, false).unwrap()),
            vec![0x39, 0xd1]
        );
    }

    #[test]
    fn mov_mem_reg_encoding() {
        assert_eq!(
            emit(|a| {
                a.mov_mem_reg(Some(Register::FP), None, Scale::S1, -16, Register::RCX, true).unwrap();
            }),
            vec![0x48, 0x89, 0x4d, 0xf0]
        );
    }

    #[test]
    fn sse_move_encoding() {
        assert_eq!(
            emit(|a| a.movq_xmm_from_gp(XMMRegister::XMM0, Register::RAX).unwrap()),
            vec![0x66, 0x48, 0x0f, 0x6e, 0xc0]
        );
    }
}
