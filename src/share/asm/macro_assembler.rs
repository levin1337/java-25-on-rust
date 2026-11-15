/*
 * Port of OpenJDK 25 HotSpot share/asm/macroAssembler.hpp
 * (+ cpu/x86/macroAssembler_x86.hpp high-level operations used at the
 *  share level: stack allocation, branches, calls, null checks,
 *  card-table barriers, alignment, breakpoints).
 */

use std::ops::{Deref, DerefMut};

use crate::share::asm::assembler::{Assembler, AssemblerError, LabelId};
use crate::share::asm::code_buffer::{CodeBuffer, CodeBlobType};
use crate::share::asm::register::{ConditionCode, OperandSize, Register};
use crate::share::asm::reloc::{RelocType, RelocWidth, RelocValue};
use crate::share::asm::x86_64::{Address, RmOperand, ShiftCount};

/// Target of a runtime/stub call (HotSpot `address` entry point or a
/// named external symbol resolved at link time).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallTarget {
    /// Absolute address (u64) of the entry point.
    Address(u64),
    /// Named external symbol.
    Symbol(String),
}

/// Parameters of the G1 card-table write barrier fast path.
#[derive(Clone, Copy, Debug)]
pub struct CardBarrierParams {
    /// Card table base address (byte_map_base).
    pub card_table_base: u64,
    /// Card shift (address >> shift indexes the card table).
    pub card_shift: u32,
    /// Register used as scratch for the card address computation.
    pub scratch: Register,
    /// Second scratch register.
    pub scratch2: Register,
}

/// High-level assembler, mirrors MacroAssembler (Assembler subclass).
pub struct MacroAssembler {
    pub asm: Assembler,
}

impl Deref for MacroAssembler {
    type Target = Assembler;
    fn deref(&self) -> &Assembler {
        &self.asm
    }
}

impl DerefMut for MacroAssembler {
    fn deref_mut(&mut self) -> &mut Assembler {
        &mut self.asm
    }
}

impl MacroAssembler {
    pub fn new(code: CodeBuffer) -> Self {
        Self { asm: Assembler::new(code) }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { asm: Assembler::with_capacity(capacity) }
    }

    // ---- stack ------------------------------------------------------------

    /// allocate_stack(size): sub rsp, size (HotSpot aligns to 8/16 first).
    pub fn allocate_stack(&mut self, size: usize) -> Result<(), AssemblerError> {
        let aligned = (size + 7) & !7;
        if aligned == 0 {
            return Ok(());
        }
        self.asm.sub_imm(
            RmOperand::Reg(Register::RSP),
            aligned as i64,
            OperandSize::Size8,
        )
    }

    /// free_stack(size): add rsp, size.
    pub fn free_stack(&mut self, size: usize) -> Result<(), AssemblerError> {
        let aligned = (size + 7) & !7;
        if aligned == 0 {
            return Ok(());
        }
        self.asm.add_imm(
            RmOperand::Reg(Register::RSP),
            aligned as i64,
            OperandSize::Size8,
        )
    }

    /// push rax-style convenience (delegates to Assembler::push).
    pub fn push_reg(&mut self, reg: Register) -> Result<(), AssemblerError> {
        self.asm.push(reg)
    }

    pub fn pop_reg(&mut self, reg: Register) -> Result<(), AssemblerError> {
        self.asm.pop(reg)
    }

    /// build_frame / remove_frame helpers used by prologues.
    pub fn build_frame(&mut self, frame_size: usize) -> Result<(), AssemblerError> {
        // push rbp; mov rbp, rsp; sub rsp, frame_size
        self.asm.push(Register::RBP)?;
        self.asm.movq(Register::RBP, RmOperand::Reg(Register::RSP))?;
        self.allocate_stack(frame_size)
    }

    pub fn remove_frame(&mut self) -> Result<(), AssemblerError> {
        self.asm.movq(Register::RSP, RmOperand::Reg(Register::RBP))?;
        self.asm.pop(Register::RBP)
    }

    // ---- compressed oops ----------------------------------------------------

    /// unpack_oop: decompress a 32-bit compressed oop in `src` (memory or
    /// register low half) into `dst` using `base` as heap base.
    /// Encoding: movl dst32, src32 (zero-extend) then add dst, base.
    pub fn unpack_oop(&mut self, dst: Register, src: RmOperand, base: Register) -> Result<(), AssemblerError> {
        match src {
            RmOperand::Reg(s) => {
                self.asm.movl(dst, RmOperand::Reg(s))?;
            }
            RmOperand::Mem(a) => {
                self.asm.movl(dst, RmOperand::Mem(a))?;
            }
        }
        self.asm.add(
            RmOperand::Reg(dst),
            RmOperand::Reg(base),
            OperandSize::Size8,
        )
    }

    /// pack_oop: compress `src` oop into `dst` low 32 bits (store or reg).
    /// Emits movl dst32, src32 (the high bits are dropped).
    pub fn pack_oop(&mut self, dst: RmOperand, src: Register) -> Result<(), AssemblerError> {
        match dst {
            RmOperand::Reg(d) => self.asm.movl(d, RmOperand::Reg(src)),
            RmOperand::Mem(a) => self.asm.mov_rm_reg(OperandSize::Size4, RmOperand::Mem(a), src),
        }
    }

    // ---- branches -----------------------------------------------------------

    /// compare_and_branch: cmp a, b; jcc cond, label.
    pub fn compare_and_branch(
        &mut self,
        a: Register,
        b: Register,
        cond: ConditionCode,
        label: LabelId,
    ) -> Result<(), AssemblerError> {
        self.asm.cmp_op(RmOperand::Reg(a), RmOperand::Reg(b), OperandSize::Size8)?;
        self.asm.jcc(cond, label)
    }

    /// compare_and_branch with immediate.
    pub fn compare_imm_and_branch(
        &mut self,
        a: Register,
        imm: i64,
        cond: ConditionCode,
        label: LabelId,
    ) -> Result<(), AssemblerError> {
        self.asm.cmp_imm(RmOperand::Reg(a), imm, OperandSize::Size8)?;
        self.asm.jcc(cond, label)
    }

    /// test_and_branch: test a, mask; jcc cond, label.
    pub fn test_and_branch(
        &mut self,
        a: Register,
        mask: Register,
        cond: ConditionCode,
        label: LabelId,
    ) -> Result<(), AssemblerError> {
        self.asm.test_op(RmOperand::Reg(a), RmOperand::Reg(mask), OperandSize::Size8)?;
        self.asm.jcc(cond, label)
    }

    /// test_imm_and_branch: test a, imm; jcc cond, label.
    pub fn test_imm_and_branch(
        &mut self,
        a: Register,
        imm: i64,
        cond: ConditionCode,
        label: LabelId,
    ) -> Result<(), AssemblerError> {
        self.asm.test_imm(RmOperand::Reg(a), imm, OperandSize::Size8)?;
        self.asm.jcc(cond, label)
    }

    /// jump to label (Alias for Assembler::jmp).
    pub fn jump(&mut self, label: LabelId) -> Result<(), AssemblerError> {
        self.asm.jmp(label)
    }

    pub fn jump_cc(&mut self, cond: ConditionCode, label: LabelId) -> Result<(), AssemblerError> {
        self.asm.jcc(cond, label)
    }

    // ---- calls ---------------------------------------------------------------

    /// call_VM_leaf equivalent: call a runtime leaf entry point.
    /// Args must already be in the C-ABI argument registers.
    pub fn call_vm_leaf(&mut self, target: CallTarget) -> Result<(), AssemblerError> {
        self.call_target(target)
    }

    /// call a runtime stub / leaf by target.
    pub fn call_stub(&mut self, target: CallTarget) -> Result<(), AssemblerError> {
        self.call_target(target)
    }

    /// Emit call rel32 with runtime_call relocation.
    fn call_target(&mut self, target: CallTarget) -> Result<(), AssemblerError> {
        self.asm.mark_inst();
        self.asm.emit_byte(0xE8);
        let disp_off = self.pc();
        self.asm.emit_dword(0);
        let inst = self.asm.inst_mark();
        let value = match target {
            CallTarget::Address(a) => RelocValue::Address(a),
            CallTarget::Symbol(s) => RelocValue::External { name: s, addend: 0 },
        };
        self.asm.relocate_at(
            inst,
            RelocType::RuntimeCall,
            RelocWidth::Word32,
            value,
            (disp_off - inst) as u32,
        );
        Ok(())
    }

    /// call a C function by symbol name (external_word reloc on the call).
    pub fn call_c_function(&mut self, name: &str) -> Result<(), AssemblerError> {
        self.asm.call_external(name, 0)
    }

    /// call an absolute address via movabs + call reg (for targets that
    /// may be out of rel32 range).
    pub fn call_absolute(&mut self, addr: u64, scratch: Register) -> Result<(), AssemblerError> {
        self.asm.mov_imm_reg(OperandSize::Size8, addr, scratch)?;
        self.asm.call(RmOperand::Reg(scratch))
    }

    /// ret [imm].
    pub fn return_from(&mut self, stack_adjust: usize) -> Result<(), AssemblerError> {
        if stack_adjust != 0 {
            self.asm.ret_imm(stack_adjust as u16);
        } else {
            self.asm.ret();
        }
        Ok(())
    }

    // ---- null checks -----------------------------------------------------------

    /// Explicit null check: test obj, obj; jcc ok; call throw_null_exception;
    /// bind ok.  Mirrors MacroAssembler::null_check with explicit_null_check.
    pub fn null_check_explicit(
        &mut self,
        obj: Register,
        throw_stub: CallTarget,
    ) -> Result<(), AssemblerError> {
        let ok = self.asm.new_label();
        self.asm.test_op(RmOperand::Reg(obj), RmOperand::Reg(obj), OperandSize::Size8)?;
        self.asm.jcc_near(ConditionCode::NE, ok)?;
        self.call_target(throw_stub)?;
        self.asm.bind(ok)?;
        Ok(())
    }

    /// Implicit null check: emit a probe load that faults on a null oop.
    /// `scratch` receives the loaded value (the access is the real load the
    /// VM's implicit-null-check handler expects).
    pub fn null_check_implicit(
        &mut self,
        obj: Register,
        offset: i32,
        scratch: Register,
    ) -> Result<(), AssemblerError> {
        self.asm.movl(
            scratch,
            RmOperand::Mem(Address::base_disp(obj, offset as i64)),
        )
    }

    /// null_check: dispatch between implicit (offset access) and explicit.
    /// When `explicit` is Some(label), emits explicit sequence branching to
    /// the label's stub; otherwise emits the implicit probe load.
    pub fn null_check(
        &mut self,
        obj: Register,
        offset: i32,
        scratch: Register,
        explicit: Option<CallTarget>,
    ) -> Result<(), AssemblerError> {
        match explicit {
            Some(stub) => {
                let ok = self.asm.new_label();
                self.asm.cmp_imm(RmOperand::Reg(obj), 0, OperandSize::Size8)?;
                self.asm.jcc_near(ConditionCode::NE, ok)?;
                self.call_target(stub)?;
                self.asm.bind(ok)?;
                Ok(())
            }
            None => self.null_check_implicit(obj, offset, scratch),
        }
    }

    // ---- alignment -------------------------------------------------------------

    /// align(align): pad with multi-byte NOPs to `align` boundary.
    pub fn align(&mut self, align: usize) -> Result<(), AssemblerError> {
        if align <= 1 {
            return Ok(());
        }
        let pad = self.pc().div_ceil(align) * align - self.pc();
        self.emit_nop_fill(pad)
    }

    /// align_to boundary relative to insts start.
    pub fn align_code_entry(&mut self) -> Result<(), AssemblerError> {
        self.align(crate::share::asm::code_buffer::CODE_ENTRY_ALIGNMENT)
    }

    /// Emit `count` bytes of NOP padding using multi-byte NOP forms.
    pub fn emit_nop_fill(&mut self, mut count: usize) -> Result<(), AssemblerError> {
        const NOPS: [&[u8]; 9] = [
            &[0x90],
            &[0x66, 0x90],
            &[0x0F, 0x1F, 0x00],
            &[0x0F, 0x1F, 0x40, 0x00],
            &[0x0F, 0x1F, 0x44, 0x00, 0x00],
            &[0x66, 0x0F, 0x1F, 0x44, 0x00, 0x00],
            &[0x0F, 0x1F, 0x80, 0x00, 0x00, 0x00, 0x00],
            &[0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
            &[0x66, 0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
        ];
        while count > 0 {
            if count >= 9 {
                self.asm.emit_bytes(NOPS[8]);
                count -= 9;
            } else {
                self.asm.emit_bytes(NOPS[count - 1]);
                count = 0;
            }
        }
        Ok(())
    }

    /// breakpoint(): int3 (os::breakpoint at asm level).
    pub fn breakpoint(&mut self) {
        self.asm.breakpoint();
    }

    /// os::breakpoint with a message is a runtime function; asm level emits int3.
    pub fn breakpoint_message(&mut self) {
        self.asm.breakpoint();
    }

    // ---- card table / GC barriers -------------------------------------------------

    /// G1 card-mark fast path: compute card byte address for `obj_addr`,
    /// compare against the "clean" sentinel (0), call `slow_path` when the
    /// card needs marking, else skip.
    ///
    /// Sequence:
    ///   mov scratch2, obj_addr
    ///   shr scratch2, card_shift
    ///   mov scratch, card_table_base
    ///   add scratch2, scratch
    ///   cmpb [scratch2], 0
    ///   je  done
    ///   call slow_path
    ///   done:
    pub fn card_mark_check(
        &mut self,
        obj_addr: Register,
        params: CardBarrierParams,
        slow_path: LabelId,
    ) -> Result<(), AssemblerError> {
        let done = self.asm.new_label();
        self.asm.movq(params.scratch2, RmOperand::Reg(obj_addr))?;
        self.asm.shr(
            RmOperand::Reg(params.scratch2),
            ShiftCount::Imm(params.card_shift.min(63) as i8),
            OperandSize::Size8,
        )?;
        self.asm.mov_imm_reg(OperandSize::Size8, params.card_table_base, params.scratch)?;
        self.asm.add(
            RmOperand::Reg(params.scratch2),
            RmOperand::Reg(params.scratch),
            OperandSize::Size8,
        )?;
        self.asm.cmp_imm(
            RmOperand::Mem(Address::ind(params.scratch2)),
            0,
            OperandSize::Size1,
        )?;
        self.asm.jcc(ConditionCode::E, done)?;
        self.asm.call_label(slow_path)?;
        self.asm.bind(done)?;
        Ok(())
    }

    /// G1 post-write barrier entry point: full fast-path check + slow-path
    /// call.  `store_addr` is the address of the field being written,
    /// `value` the oop being stored (used by some barriers; here it only
    /// participates in the card computation via store_addr).
    pub fn g1_write_barrier(
        &mut self,
        store_addr: Register,
        value: Register,
        params: CardBarrierParams,
        slow_path: LabelId,
    ) -> Result<(), AssemblerError> {
        let _ = value;
        self.card_mark_check(store_addr, params, slow_path)
    }

    /// G1 pre-write barrier stub variant (marks card before overwrite).
    pub fn g1_pre_write_barrier(
        &mut self,
        store_addr: Register,
        params: CardBarrierParams,
        slow_path: LabelId,
    ) -> Result<(), AssemblerError> {
        self.card_mark_check(store_addr, params, slow_path)
    }

    /// Record a safepoint poll: emit test of the poll page and record a
    /// poll_type relocation.  `poll_addr` is the address operand.
    pub fn safepoint_poll(&mut self, poll_addr: Address) -> Result<(), AssemblerError> {
        self.asm.test_imm(RmOperand::Mem(poll_addr), 0, OperandSize::Size1)?;
        self.asm.relocate_here(
            RelocType::Poll,
            RelocWidth::Word32,
            RelocValue::Poll { data: 0 },
        );
        Ok(())
    }

    // ---- oop maps / scope descs -----------------------------------------------------

    /// Record an oop map at current pc (OopMapSet::add_gc_map).
    pub fn record_oopmap(&mut self, oops: &[i32], derived_oops: &[i32]) {
        let pc = self.pc() as u32;
        self.code_buffer_mut().record_oopmap(pc, oops, derived_oops);
    }

    /// add_safepoint: record poll + oopmap at a call site.
    pub fn add_safepoint(&mut self, oops: &[i32], derived_oops: &[i32]) {
        self.record_oopmap(oops, derived_oops);
    }

    // ---- finalize ---------------------------------------------------------------------

    /// flush + copy into a CodeBlob.
    pub fn copy_codes(&self, blob_type: CodeBlobType, name: &str) -> Result<crate::share::asm::code_buffer::CodeBlob, AssemblerError> {
        self.code.copy_codes(blob_type, name).map_err(AssemblerError::CodeBuffer)
    }
}
