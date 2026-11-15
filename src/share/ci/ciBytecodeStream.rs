/*
 * Port of OpenJDK 25 HotSpot ciBytecodeStream (share/ci/ciStreams.hpp
 * `ciBytecodeStream` / `BytecodeStream` usage in the C2 parser)
 *
 * A `CiBytecodeStream` iterates the instructions of a `CiMethod` with
 * decoded operands: branch destinations, constant-pool indices,
 * local indices and wide prefixes.  It layers the interpreter's
 * `BytecodeStream` length table with ci-level constant-pool access
 * (`CiConstantPool`) so the parser can resolve `ldc`, field and
 * invoke operands without leaving the ci facade.
 */

use super::ciConstant::CiConstantPool;
use super::ciMethod::CiMethod;
use crate::share::interpreter::bytecode::{BytecodeInvoke, BytecodeLookupswitch, BytecodeTableswitch};
use crate::share::interpreter::bytecodeStream::BytecodeStream;
use crate::share::interpreter::bytecodes::{self, Code};

/// Operand kind decoded at the current instruction — mirrors the
/// `Bytecodes::Code` operand attributes used by the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiOperandKind {
    /// No operands.
    None,
    /// `u1` local index (`iload`, `istore`, ...).
    LocalU1,
    /// `u2` branch offset (`if*`, `goto`).
    BranchS2,
    /// `u4` branch offset (`goto_w`, `jsr_w`).
    BranchS4,
    /// `u1`/`u2` constant-pool index (`ldc`, `ldc_w`, `ldc2_w`).
    ConstantPool,
    /// `u2` field/method constant-pool index (`get*`, `put*`, `invoke*`).
    MemberRef,
    /// `u2` class constant-pool index (`new`, `anewarray`, `checkcast`,
    /// `instanceof`, `multianewarray`).
    ClassRef,
    /// `u1` array type (`newarray`).
    NewArrayType,
    /// `iinc`: local `u1` + const `i8` (or wide `u2` + `i16`).
    Iinc,
    /// `tableswitch`.
    Tableswitch,
    /// `lookupswitch`.
    Lookupswitch,
    /// `wide` prefix — operand decoding delegated to the widened
    /// instruction.
    Wide,
    /// `multianewarray`: class `u2` + dims `u1`.
    MultiANewArray,
    /// `invokeinterface`: member `u2` + count `u1` + 0 `u1`.
    InvokeInterface,
}

/// `ciBytecodeStream`: decoded bytecode iteration for one `CiMethod`.
pub struct CiBytecodeStream<'m, 'a> {
    method: &'m CiMethod<'a>,
    stream: BytecodeStream<'a>,
    /// Constant pool of the method's holder, built lazily.
    pool: Option<CiConstantPool<'a>>,
}

impl<'m, 'a> CiBytecodeStream<'m, 'a> {
    /// `ciBytecodeStream` over a method's Code attribute.
    /// Returns `None` for methods without bytecode.
    pub fn new(method: &'m CiMethod<'a>) -> Option<Self> {
        if !method.has_code() {
            return None;
        }
        Some(Self {
            method,
            stream: BytecodeStream::new(method.bytecodes()),
            pool: None,
        })
    }

    /// `ciBytecodeStream` positioned at `bci` (must be an instruction
    /// boundary; `BytecodeStream::set_interval` does not verify).
    pub fn at_bci(method: &'m CiMethod<'a>, bci: usize) -> Option<Self> {
        let mut s = Self::new(method)?;
        s.stream.set_start(bci.min(method.code_size()));
        Some(s)
    }

    // ---- position ---------------------------------------------------------

    #[inline]
    pub fn bci(&self) -> usize {
        self.stream.bci()
    }

    #[inline]
    pub fn next_bci(&self) -> usize {
        self.stream.next_bci()
    }

    #[inline]
    pub fn is_at_end(&self) -> bool {
        self.stream.is_at_end()
    }

    #[inline]
    pub fn is_last_bytecode(&self) -> bool {
        self.stream.is_last_bytecode()
    }

    #[inline]
    pub fn instruction_size(&self) -> usize {
        self.stream.instruction_size()
    }

    #[inline]
    pub fn is_wide(&self) -> bool {
        self.stream.is_wide()
    }

    /// Advance to the next instruction; returns the opcode byte.
    pub fn next(&mut self) -> Option<u8> {
        self.stream.next()
    }

    /// Reposition to `bci`.
    pub fn set_bci(&mut self, bci: usize) {
        self.stream.set_start(bci);
    }

    /// Restrict iteration to `[beg, end)`.
    pub fn set_interval(&mut self, beg: usize, end: usize) {
        self.stream.set_interval(beg, end);
    }

    // ---- opcode ------------------------------------------------------------

    /// Raw opcode byte (the `wide` prefix is reported as `0xc4`).
    #[inline]
    pub fn raw_code(&self) -> Option<u8> {
        self.stream.raw_code()
    }

    /// Effective opcode: for `wide` instructions the widened opcode.
    pub fn code(&self) -> Option<Code> {
        let op = self.stream.current_opcode()?;
        if op == 0xc4 {
            // wide prefix: next byte is the real opcode.
            return self.method.bytecode_at(self.bci() + 1).and_then(bytecodes::code_from_u8);
        }
        bytecodes::code_from_u8(op)
    }

    /// `ciBytecodeStream::java_code` — `Bytecodes::Code`.
    #[inline]
    pub fn java_code(&self) -> Option<Code> {
        self.code()
    }

    /// Operand kind for the current instruction, decoded by raw
    /// opcode byte (covers the `_N` and `wide` forms uniformly).
    pub fn operand_kind(&self) -> CiOperandKind {
        use CiOperandKind::*;
        let Some(code) = self.code() else {
            return None;
        };
        let op = code as u8;
        if (0x15..=0x4e).contains(&op) || op == 0x84 || op == 0xa9 {
            // iload..astore incl. _N forms, iinc, ret
            return LocalU1;
        }
        match op {
            0x99..=0xa8 | 0xc6 | 0xc7 => BranchS2, // if*, goto, jsr, ifnull, ifnonnull
            0xc8 | 0xc9 => BranchS4,             // goto_w, jsr_w
            0x12..=0x14 => ConstantPool,         // ldc, ldc_w, ldc2_w
            0xb2..=0xb5 | 0xb6..=0xb8 | 0xba => MemberRef, // field/invoke (non-interface)
            0xb9 => InvokeInterface,
            0xbb | 0xbd | 0xc0 | 0xc1 => ClassRef, // new, anewarray, checkcast, instanceof
            0xbc => NewArrayType,
            0xaa => Tableswitch,
            0xab => Lookupswitch,
            0xc5 => MultiANewArray,
            _ => None,
        }
    }

    // ---- operands -----------------------------------------------------------

    /// `u1` operand at byte offset 1 (non-wide forms).
    #[inline]
    pub fn get_index_u1(&self) -> u8 {
        self.stream.get_index_u1()
    }

    /// `u2` operand at byte offset 1.
    #[inline]
    pub fn get_index_u2(&self) -> u16 {
        self.stream.get_index_u2()
    }

    /// Local variable index for load/store instructions, honouring
    /// `wide` (u2 index) and the `_0.._3` shortcut forms.
    pub fn local_index(&self) -> Option<usize> {
        let code = self.code()?;
        use Code::*;
        if self.is_wide() {
            // wide iinc: index is u2 at bci+2; wide load/store: u2 at bci+2.
            let idx = self.u2_at(2)? as usize;
            return Some(idx);
        }
        Some(match code {
            Iload0 | Lload0 | Fload0 | Dload0 | Aload0 | Istore0 | Lstore0 | Fstore0
            | Dstore0 | Astore0 | Ret => 0,
            Iload1 | Lload1 | Fload1 | Dload1 | Aload1 | Istore1 | Lstore1 | Fstore1
            | Dstore1 | Astore1 => 1,
            Iload2 | Lload2 | Fload2 | Dload2 | Aload2 | Istore2 | Lstore2 | Fstore2
            | Dstore2 | Astore2 => 2,
            Iload3 | Lload3 | Fload3 | Dload3 | Aload3 | Istore3 | Lstore3 | Fstore3
            | Dstore3 | Astore3 => 3,
            Iload | Lload | Fload | Dload | Aload | Istore | Lstore | Fstore | Dstore
            | Astore => self.get_index_u1() as usize,
            Iinc => self.get_index_u1() as usize,
            _ => return None,
        })
    }

    /// `iinc` constant operand.
    pub fn iinc_constant(&self) -> Option<i32> {
        if self.is_wide() {
            // wide iinc: opcode(1) sub(1) index(2) const(2)
            let b = self.method.bytecodes();
            let bci = self.bci();
            let bytes: [u8; 2] = b.get(bci + 4..bci + 6)?.try_into().ok()?;
            Some(i16::from_be_bytes(bytes) as i32)
        } else {
            Some(self.method.bytecode_at(self.bci() + 2)? as i8 as i32)
        }
    }

    /// Signed branch offset target bci (`dest()` analogue).
    pub fn branch_dest(&self) -> Option<usize> {
        match self.operand_kind() {
            CiOperandKind::BranchS2 => {
                let off = self.s2_at(1)? as isize;
                Some(self.bci().wrapping_add_signed(off))
            }
            CiOperandKind::BranchS4 => {
                let off = self.s4_at(1)? as isize;
                Some(self.bci().wrapping_add_signed(off))
            }
            _ => None,
        }
    }

    /// `tableswitch` decoded view at the current bci.
    pub fn tableswitch(&self) -> Option<BytecodeTableswitch<'a>> {
        BytecodeTableswitch::new(self.method.bytecodes(), self.bci())
    }

    /// `lookupswitch` decoded view.
    pub fn lookupswitch(&self) -> Option<BytecodeLookupswitch<'a>> {
        BytecodeLookupswitch::new(self.method.bytecodes(), self.bci())
    }

    /// `invoke*` decoded view (`BytecodeInvoke` analogue).
    pub fn invoke(&self) -> Option<BytecodeInvoke<'a>> {
        BytecodeInvoke::new(self.method.bytecodes(), self.bci())
    }

    /// Constant-pool index operand (`ldc`, `ldc_w`, `ldc2_w`,
    /// field/method refs, class refs).
    pub fn constant_pool_index(&self) -> Option<u16> {
        match self.operand_kind() {
            CiOperandKind::ConstantPool | CiOperandKind::MemberRef | CiOperandKind::ClassRef
            | CiOperandKind::InvokeInterface => Some(self.get_index_u2()),
            CiOperandKind::MultiANewArray => {
                // class index u2 at offset 1, dims u1 at offset 3.
                Some(self.get_index_u2())
            }
            _ => None,
        }
    }

    /// `newarray` element type byte.
    pub fn newarray_type(&self) -> Option<u8> {
        if self.code() == Some(Code::Newarray) {
            self.method.bytecode_at(self.bci() + 1)
        } else {
            None
        }
    }

    /// `multianewarray` dimension count.
    pub fn multianewarray_dims(&self) -> Option<u8> {
        if self.code() == Some(Code::Multianewarray) {
            self.method.bytecode_at(self.bci() + 3)
        } else {
            None
        }
    }

    /// `invokeinterface` argument-slot count operand (second u1).
    pub fn invokeinterface_count(&self) -> Option<u8> {
        if self.code() == Some(Code::Invokeinterface) {
            self.method.bytecode_at(self.bci() + 3)
        } else {
            None
        }
    }

    // ---- constant pool --------------------------------------------------------

    /// Lazily built constant pool of the method's holder.
    fn pool(&mut self) -> &CiConstantPool<'a> {
        self.pool.get_or_insert_with(|| {
            CiConstantPool::new(self.method.holder().clone())
        })
    }

    /// `ciBytecodeStream::get_constant` analogue: resolve the current
    /// instruction's pool operand to a `CiConstant`.
    pub fn constant(&mut self) -> Option<super::ciConstant::CiConstant> {
        let index = self.constant_pool_index()?;
        self.pool().constant_at(index)
    }

    /// Member-ref resolution for field/method operands:
    /// `(owner_name, name, descriptor)` of the referenced member.
    pub fn member_ref(&mut self) -> Option<[&'a [u8]; 3]> {
        let index = self.constant_pool_index()?;
        let is_field = matches!(
            self.code(),
            Some(Code::Getstatic | Code::Putstatic | Code::Getfield | Code::Putfield)
        );
        self.pool().member_ref(index, is_field)
    }

    /// Class-name resolution for `new`/`checkcast`/`instanceof`/
    /// `anewarray`/`multianewarray` operands.
    pub fn class_name(&self) -> Option<&'a [u8]> {
        let index = self.constant_pool_index()?;
        self.method.holder().class_name_of(index)
    }

    // ---- raw operand reads -----------------------------------------------------

    fn u2_at(&self, offset: usize) -> Option<u16> {
        let bci = self.bci();
        let bytes: [u8; 2] = self
            .method
            .bytecodes()
            .get(bci + offset..bci + offset + 2)?
            .try_into()
            .ok()?;
        Some(u16::from_be_bytes(bytes))
    }

    fn s2_at(&self, offset: usize) -> Option<i16> {
        self.u2_at(offset).map(|v| v as i16)
    }

    fn s4_at(&self, offset: usize) -> Option<i32> {
        let bci = self.bci();
        let bytes: [u8; 4] = self
            .method
            .bytecodes()
            .get(bci + offset..bci + offset + 4)?
            .try_into()
            .ok()?;
        Some(i32::from_be_bytes(bytes))
    }
}

/// Convenience constructor over raw bytecodes + holder for callers
/// that do not have a `CiMethod` (replay tooling).
pub struct CiBytecodeStreamRaw<'a> {
    stream: BytecodeStream<'a>,
    bytes: &'a [u8],
}

impl<'a> CiBytecodeStreamRaw<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            stream: BytecodeStream::new(bytes),
            bytes,
        }
    }

    #[inline]
    pub fn bci(&self) -> usize {
        self.stream.bci()
    }

    #[inline]
    pub fn next_bci(&self) -> usize {
        self.stream.next_bci()
    }

    #[inline]
    pub fn is_at_end(&self) -> bool {
        self.stream.is_at_end()
    }

    pub fn next(&mut self) -> Option<u8> {
        self.stream.next()
    }

    /// Instruction length in bytes at `bci` (`bytecodes::length_at`).
    #[inline]
    pub fn instruction_size_at(bci: usize, bytes: &'a [u8]) -> usize {
        bytecodes::length_at(bytes, bci)
    }

    /// All instruction start bcis in `[0, len)`.
    pub fn instruction_bcis(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut bci = 0usize;
        while bci < self.bytes.len() {
            out.push(bci);
            bci += bytecodes::length_at(self.bytes, bci);
        }
        out
    }
}
