/*
 * Port of OpenJDK 25 HotSpot BytecodeUtils
 * (share/interpreter/bytecodeUtils.cpp / bytecodeUtils.hpp)
 *
 * Provides JVMS instruction operand decoding (which operand bytes are
 * used by each opcode, including `wide` expansion and the variable
 * length of tableswitch/lookupswitch), plus the operand-stack
 * simulation used to build precise NullPointerException messages
 * (`ExceptionMessageBuilder`, `get_NPE_message_at`).
 */

use super::bytecodes::{self as bytes, BasicType, Code};
use crate::share::classfile::classFileParser::Constant;
use crate::share::oops::constMethod::LocalVariableTableElement;
use crate::share::oops::method::ExceptionTableEntry;
use crate::share::runtime::signature;

// ---------------------------------------------------------------------------
// Instruction operand decoding (JVMS operand layout)
// ---------------------------------------------------------------------------

/// Decoded operands of a single instruction.  Fields are `None` when the
/// opcode does not carry that kind of operand.  All multi-byte operands
/// are big-endian (JVMS); `wide` expands local indices and `iinc`
/// constants to 16 bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Operands {
    /// Opcode after `wide` expansion.
    pub code: Code,
    /// True when the instruction was prefixed by `wide`.
    pub is_wide: bool,
    /// Bytecode index of the instruction (the `wide` prefix if present).
    pub bci: usize,
    /// Constant pool index operand (ldc*, *load/store field, *invoke*,
    /// new, anewarray, checkcast, instanceof, multianewarray).
    pub cp_index: Option<u32>,
    /// Local variable index operand (*load, *store, iinc, ret).
    pub local_index: Option<u32>,
    /// `iinc` increment operand.
    pub const_value: Option<i64>,
    /// `bipush`/`sipush` constant operand.
    pub push_const: Option<i64>,
    /// Branch target as an absolute bytecode index (if*, goto*, jsr*,
    /// *switch default and targets are exposed via `switch_targets`).
    pub branch_target: Option<isize>,
    /// `newarray` element type code.
    pub array_type: Option<u8>,
    /// `multianewarray` dimension count.
    pub dimensions: Option<u8>,
    /// `invokeinterface` argument-slot/count byte and trailing zero byte.
    pub invoke_count: Option<u8>,
}

fn be_u1(code: &[u8], at: usize) -> Option<u8> {
    code.get(at).copied()
}

fn be_u2(code: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = code.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(bytes))
}

fn be_u4(code: &[u8], at: usize) -> Option<u32> {
    let bytes: [u8; 4] = code.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn be_i2(code: &[u8], at: usize) -> Option<i16> {
    be_u2(code, at).map(|v| v as i16)
}

fn be_i4(code: &[u8], at: usize) -> Option<i32> {
    be_u4(code, at).map(|v| v as i32)
}

/// Operand byte ranges (offsets relative to the opcode byte, exclusive of
/// the opcode itself) for each operand kind.  Used by verifiers and
/// debuggers that need to know which bytes of an instruction are
/// significant.  Returns `(start, end)` half-open ranges into `code`.
pub fn operand_ranges(code: &[u8], bci: usize) -> Vec<(usize, usize)> {
    let Some(op) = code.get(bci).copied() else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    let mut push = |start: usize, end: usize| ranges.push((bci + start, bci + end));
    match op {
        0x10 => push(1, 2),          // bipush: i8
        0x11 => push(1, 3),          // sipush: i16
        0x12 => push(1, 2),          // ldc: u1 cp index
        0x13 | 0x14 => push(1, 3),   // ldc_w, ldc2_w: u2
        0x15..=0x19 | 0x36..=0x3a | 0xa9 => push(1, 2), // load/store/ret: u1
        0x84 => {
            push(1, 2);
            push(2, 3);              // iinc: u1 index, i8 const
        }
        0x99..=0xa8 | 0xc6 | 0xc7 => push(1, 3), // if*, goto, jsr: i16 offset
        0xa9 => push(1, 2),          // ret: u1
        0xaa => {
            // tableswitch: padding to 4-byte boundary then default,
            // low, high and (high-low+1) offsets.
            let aligned = (bci + 1 + 3) & !3;
            push(1, aligned - bci);
            if let (Some(lo), Some(hi)) = (
                be_i4(code, aligned + 4),
                be_i4(code, aligned + 8),
            ) && lo <= hi
                && hi - lo < 0x1fff_ffff
            {
                let n = (hi - lo + 1) as usize;
                push(aligned - bci, aligned - bci + 12 + 4 * n);
            }
        }
        0xab => {
            // lookupswitch: padding, default, npairs, then npairs
            // (match, offset) pairs.
            let aligned = (bci + 1 + 3) & !3;
            push(1, aligned - bci);
            if let Some(n) = be_i4(code, aligned + 4)
                && n >= 0
                && n <= 0x1fff_ffff
            {
                let n = n as usize;
                push(aligned - bci, aligned - bci + 8 + 8 * n);
            }
        }
        0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => push(1, 3), // u2 cp/class index
        0xb9 => {
            push(1, 5);              // invokeinterface: u2 cp, u1 count, u1 zero
        }
        0xba => push(1, 5),         // invokedynamic: u4 cp index
        0xbc => push(1, 2),         // newarray: u1 atype
        0xc5 => {
            push(1, 3);
            push(3, 4);              // multianewarray: u2 cp, u1 dims
        }
        0xc8 | 0xc9 => push(1, 5),  // goto_w, jsr_w: i32 offset
        0xc4 => {
            // wide: prefix byte plus the widened instruction operands.
            push(1, 2);
            if let Some(&sub) = code.get(bci + 1) {
                match sub {
                    0x15..=0x19 | 0x36..=0x3a | 0xa9 => push(2, 6), // u2 index
                    0x84 => {
                        push(2, 6);
                        push(6, 8);  // wide iinc: u2 index, i16 const
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    ranges
}

/// Decode all operands of the instruction at `bci` in `code`.
/// Returns `None` when the instruction is truncated.
pub fn decode_operands(code: &[u8], bci: usize) -> Option<Operands> {
    let opcode = *code.get(bci)?;
    let mut operands = Operands {
        code: bytes::code_from_u8(opcode)?,
        bci,
        ..Operands::default()
    };
    let mut pos = bci + 1;
    let mut effective = operands.code;

    if operands.code == Code::Wide {
        let sub = *code.get(pos)?;
        effective = bytes::code_from_u8(sub)?;
        operands.is_wide = true;
        operands.code = effective;
        pos += 1;
    }

    let load_store = |op: Code| {
        matches!(
            op as i32,
            0x15..=0x19 | 0x36..=0x3a | 0xa9 // iload..astore, ret
        )
    };

    match effective {
        Code::Bipush => {
            operands.push_const = be_u1(code, pos).map(|v| v as i8 as i64);
        }
        Code::Sipush => {
            operands.push_const = be_i2(code, pos).map(i64::from);
        }
        Code::Ldc => {
            operands.cp_index = be_u1(code, pos).map(u32::from);
        }
        Code::LdcW | Code::Ldc2W => {
            operands.cp_index = be_u2(code, pos).map(u32::from);
        }
        Code::Iinc => {
            if operands.is_wide {
                operands.local_index = be_u2(code, pos).map(u32::from);
                operands.const_value = be_i2(code, pos + 2).map(i64::from);
            } else {
                operands.local_index = be_u1(code, pos).map(u32::from);
                operands.const_value = be_u1(code, pos + 1).map(|v| v as i8 as i64);
            }
        }
        _ if load_store(effective) => {
            operands.local_index = if operands.is_wide {
                be_u2(code, pos).map(u32::from)
            } else {
                be_u1(code, pos).map(u32::from)
            };
        }
        Code::Goto | Code::Jsr | Code::Ifeq | Code::Ifne | Code::Iflt | Code::Ifge
        | Code::Ifgt | Code::Ifle | Code::IfIcmpeq | Code::IfIcmpne | Code::IfIcmplt
        | Code::IfIcmpge | Code::IfIcmpgt | Code::IfIcmple | Code::IfAcmpeq | Code::IfAcmpne
        | Code::Ifnull | Code::Ifnonnull => {
            operands.branch_target = be_i2(code, pos).map(|off| bci as isize + off as isize);
        }
        Code::GotoW | Code::JsrW => {
            operands.branch_target = be_i4(code, pos).map(|off| bci as isize + off as isize);
        }
        Code::Tableswitch => {
            let aligned = (pos + 3) & !3;
            operands.branch_target =
                be_i4(code, aligned).map(|off| bci as isize + off as isize);
        }
        Code::Lookupswitch => {
            let aligned = (pos + 3) & !3;
            operands.branch_target =
                be_i4(code, aligned).map(|off| bci as isize + off as isize);
        }
        Code::Getstatic | Code::Putstatic | Code::Getfield | Code::Putfield
        | Code::Invokevirtual | Code::Invokespecial | Code::Invokestatic | Code::New
        | Code::Anewarray | Code::Checkcast | Code::Instanceof => {
            operands.cp_index = be_u2(code, pos).map(u32::from);
        }
        Code::Invokeinterface => {
            operands.cp_index = be_u2(code, pos).map(u32::from);
            operands.invoke_count = be_u1(code, pos + 2);
        }
        Code::Invokedynamic | Code::Invokehandle => {
            operands.cp_index = be_u4(code, pos);
        }
        Code::Newarray => {
            operands.array_type = be_u1(code, pos);
        }
        Code::Multianewarray => {
            operands.cp_index = be_u2(code, pos).map(u32::from);
            operands.dimensions = be_u1(code, pos + 2);
        }
        _ => {}
    }
    Some(operands)
}

/// Branch targets of a tableswitch/lookupswitch instruction, including
/// the default target first.
pub fn switch_targets(code: &[u8], bci: usize) -> Option<Vec<isize>> {
    let op = *code.get(bci)?;
    let aligned = ((bci + 1) + 3) & !3;
    let mut targets = Vec::new();
    match op {
        0xaa => {
            let default = be_i4(code, aligned)?;
            let low = be_i4(code, aligned + 4)?;
            let high = be_i4(code, aligned + 8)?;
            targets.push(bci as isize + default as isize);
            if high >= low {
                for i in low..=high {
                    let off = be_i4(code, aligned + 12 + 4 * (i - low) as usize)?;
                    targets.push(bci as isize + off as isize);
                }
            }
        }
        0xab => {
            let default = be_i4(code, aligned)?;
            let npairs = be_i4(code, aligned + 4)?;
            targets.push(bci as isize + default as isize);
            for i in 0..npairs.max(0) as usize {
                let off = be_i4(code, aligned + 12 + 8 * i)?;
                targets.push(bci as isize + off as isize);
            }
        }
        _ => return None,
    }
    Some(targets)
}

// ---------------------------------------------------------------------------
// Constant pool access helpers over `&[Constant]`
// ---------------------------------------------------------------------------

fn utf8<'a>(constants: &'a [Constant<'a>], index: u16) -> Option<&'a [u8]> {
    match constants.get(usize::from(index))? {
        Constant::Utf8(bytes) => Some(bytes),
        _ => None,
    }
}

fn name_and_type_index(
    constants: &[Constant<'_>],
    cp_index: u32,
) -> Option<u16> {
    match constants.get(cp_index as usize)? {
        Constant::FieldRef(_, nt)
        | Constant::MethodRef(_, nt)
        | Constant::InterfaceMethodRef(_, nt)
        | Constant::InvokeDynamic(_, nt)
        | Constant::Dynamic(_, nt) => Some(*nt),
        _ => None,
    }
}

fn name_ref_index(constants: &[Constant<'_>], cp_index: u32) -> Option<u16> {
    let nt = name_and_type_index(constants, cp_index)?;
    match constants.get(usize::from(nt))? {
        Constant::NameAndType(name, _) => Some(*name),
        _ => None,
    }
}

fn signature_ref_index(constants: &[Constant<'_>], cp_index: u32) -> Option<u16> {
    let nt = name_and_type_index(constants, cp_index)?;
    match constants.get(usize::from(nt))? {
        Constant::NameAndType(_, desc) => Some(*desc),
        _ => None,
    }
}

fn signature_at<'a>(constants: &'a [Constant<'a>], cp_index: u32) -> Option<&'a [u8]> {
    let desc = signature_ref_index(constants, cp_index)?;
    utf8(constants, desc)
}

fn name_at<'a>(constants: &'a [Constant<'a>], cp_index: u32) -> Option<&'a [u8]> {
    let name = name_ref_index(constants, cp_index)?;
    utf8(constants, name)
}

fn class_name_at<'a>(constants: &'a [Constant<'a>], cp_index: u32) -> Option<&'a [u8]> {
    // For field/method refs the class is the first u2; for checkcast /
    // instanceof / anewarray / multianewarray the cp entry is a Class.
    match constants.get(cp_index as usize)? {
        Constant::FieldRef(class, _)
        | Constant::MethodRef(class, _)
        | Constant::InterfaceMethodRef(class, _) => {
            class_name_of(constants, *class)
        }
        Constant::Class(name_index) => utf8(constants, *name_index),
        _ => None,
    }
}

fn class_name_of<'a>(constants: &'a [Constant<'a>], class_index: u16) -> Option<&'a [u8]> {
    match constants.get(usize::from(class_index))? {
        Constant::Class(name_index) => utf8(constants, *name_index),
        _ => None,
    }
}

/// `Signature::basic_type`: the BasicType of a field/method descriptor.
fn signature_basic_type(descriptor: &[u8]) -> BasicType {
    match descriptor.first().copied() {
        Some(b'B' | b'C' | b'I' | b'S' | b'Z') => BasicType::Int,
        Some(b'J') => BasicType::Long,
        Some(b'F') => BasicType::Float,
        Some(b'D') => BasicType::Double,
        Some(b'L' | b'[') => BasicType::Object,
        Some(b'V') => BasicType::Void,
        _ => BasicType::Illegal,
    }
}

/// `ArgumentSizeComputer`: number of local/stack slots consumed by the
/// parameters of a method descriptor.
fn argument_size(descriptor: &[u8]) -> usize {
    let Some(body) = descriptor.strip_prefix(b"(") else {
        return 0;
    };
    let mut size = 0usize;
    let mut i = 0usize;
    while i < body.len() && body[i] != b')' {
        let mut dims = 0usize;
        while i < body.len() && body[i] == b'[' {
            dims += 1;
            i += 1;
        }
        if i >= body.len() {
            break;
        }
        let tag = body[i];
        i += 1;
        if tag == b'L' {
            while i < body.len() && body[i] != b';' {
                i += 1;
            }
            i += 1; // skip ';'
        }
        if dims == 0 {
            size += match tag {
                b'J' | b'D' => 2,
                b'B' | b'C' | b'I' | b'S' | b'Z' | b'F' | b'L' => 1,
                _ => 0,
            };
        } else {
            size += 1;
        }
    }
    size
}

/// `ResultTypeFinder`: result type of a method descriptor.
fn result_type_of(descriptor: &[u8]) -> BasicType {
    if let Some(close) = descriptor.iter().position(|&b| b == b')') {
        signature_basic_type(&descriptor[close + 1..])
    } else {
        BasicType::Illegal
    }
}

// ---------------------------------------------------------------------------
// SimulatedOperandStack / StackSlotAnalysisData
// ---------------------------------------------------------------------------

const INVALID_BCI: u32 = 0x1FFFF;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_CAUSE_DETAIL: usize = 5;

/// Analysis information for one operand-stack slot: the bci that pushed
/// the value and its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackSlotAnalysisData {
    bci: u32,
    basic_type: BasicType,
}

impl StackSlotAnalysisData {
    pub fn new(basic_type: BasicType) -> Self {
        Self {
            bci: INVALID_BCI,
            basic_type,
        }
    }

    pub fn with_bci(bci: u32, basic_type: BasicType) -> Self {
        Self { bci, basic_type }
    }

    pub fn bci(&self) -> u32 {
        self.bci
    }

    pub fn has_bci(&self) -> bool {
        self.bci != INVALID_BCI
    }

    pub fn basic_type(&self) -> BasicType {
        self.basic_type
    }

    fn merge(self, other: Self) -> Self {
        if self.basic_type != other.basic_type {
            let ref_or_array = |t| matches!(t, BasicType::Object | BasicType::Array);
            if ref_or_array(self.basic_type) && ref_or_array(other.basic_type) {
                if self.bci == other.bci {
                    return Self::with_bci(self.bci, BasicType::Object);
                }
                return Self::new(BasicType::Object);
            }
            return Self::new(BasicType::Conflict);
        }
        if self.bci == other.bci {
            self
        } else {
            Self::new(self.basic_type)
        }
    }
}

/// `SimulatedOperandStack`: per-bytecode operand-stack model plus the
/// set of local slots written so far (used to decide whether a local
/// still denotes a parameter/`this`).
#[derive(Debug, Clone)]
pub struct SimulatedOperandStack {
    stack: Vec<StackSlotAnalysisData>,
    written_local_slots: u64,
}

impl SimulatedOperandStack {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            written_local_slots: 0,
        }
    }

    fn push_raw(&mut self, slot: StackSlotAnalysisData) {
        if slot.basic_type == BasicType::Void {
            return;
        }
        self.stack.push(slot);
    }

    fn push(&mut self, slot: StackSlotAnalysisData) {
        if bytes::type2size(slot.basic_type) == 2 {
            self.push_raw(slot);
            self.push_raw(slot);
        } else {
            self.push_raw(slot);
        }
    }

    fn push_bci(&mut self, bci: u32, basic_type: BasicType) {
        self.push(StackSlotAnalysisData::with_bci(bci, basic_type));
    }

    fn pop(&mut self, slots: usize) {
        for _ in 0..slots {
            self.stack.pop();
        }
    }

    fn merge(&mut self, other: &SimulatedOperandStack) {
        for i in (0..self.stack.len()).rev() {
            if let Some(&other_slot) = other.stack.get(i) {
                self.stack[i] = self.stack[i].merge(other_slot);
            }
        }
        self.written_local_slots |= other.written_local_slots;
    }

    pub fn size(&self) -> usize {
        self.stack.len()
    }

    /// Slot 0 is the top of stack.
    pub fn slot_data(&self, slot: usize) -> Option<StackSlotAnalysisData> {
        self.stack
            .len()
            .checked_sub(slot + 1)
            .map(|i| self.stack[i])
    }

    fn set_local_slot_written(&mut self, index: usize) {
        if index > 63 {
            return;
        }
        self.written_local_slots |= 1u64 << index;
    }

    fn local_slot_was_written(&self, index: usize) -> bool {
        if index > 63 {
            return true;
        }
        (self.written_local_slots & (1u64 << index)) != 0
    }
}

// ---------------------------------------------------------------------------
// Method view: everything ExceptionMessageBuilder needs from a Method
// ---------------------------------------------------------------------------

/// View over the parts of a method needed by the NPE message analysis:
/// code bytes, exception table, constant pool, method flags and the
/// optional local variable table.
pub struct MethodView<'a> {
    pub code: &'a [u8],
    pub exception_table: &'a [ExceptionTableEntry],
    pub constants: &'a [Constant<'a>],
    pub is_static: bool,
    pub name: &'a [u8],
    pub descriptor: &'a [u8],
    pub local_variable_table: &'a [LocalVariableTableElement],
}

impl<'a> MethodView<'a> {
    pub fn new(
        code: &'a [u8],
        exception_table: &'a [ExceptionTableEntry],
        constants: &'a [Constant<'a>],
        is_static: bool,
        name: &'a [u8],
        descriptor: &'a [u8],
        local_variable_table: &'a [LocalVariableTableElement],
    ) -> Self {
        Self {
            code,
            exception_table,
            constants,
            is_static,
            name,
            descriptor,
            local_variable_table,
        }
    }

    /// Parameter slots (`size_of_parameters`): `this` excluded.
    pub fn parameter_slots(&self) -> usize {
        argument_size(self.descriptor)
    }
}

// ---------------------------------------------------------------------------
// ExceptionMessageBuilder
// ---------------------------------------------------------------------------

/// `ExceptionMessageBuilder`: computes, for each bytecode, a simulated
/// operand stack so that a precise NPE message can be assembled.
pub struct ExceptionMessageBuilder<'a> {
    stacks: Vec<Option<SimulatedOperandStack>>,
    method: &'a MethodView<'a>,
    nr_of_entries: usize,
    added_one: bool,
    all_processed: bool,
}

impl<'a> ExceptionMessageBuilder<'a> {
    /// Builds the analysis, stopping early once the stack at `bci`
    /// (when `Some`) is known.
    pub fn new(method: &'a MethodView<'a>, bci: Option<usize>) -> Self {
        let len = method.code.len();
        let mut stacks = vec![None; len + 1];
        stacks[0] = Some(SimulatedOperandStack::new());
        for entry in method.exception_table {
            let index = entry.handler_pc;
            if index <= len && stacks[index].is_none() {
                let mut stack = SimulatedOperandStack::new();
                stack.push_bci(index as u32, BasicType::Object);
                stacks[index] = Some(stack);
            }
        }
        let mut builder = Self {
            stacks,
            method,
            nr_of_entries: 0,
            added_one: true,
            all_processed: false,
        };
        builder.run(bci);
        builder
    }

    fn run(&mut self, stop_at: Option<usize>) {
        while !self.all_processed && self.added_one {
            self.all_processed = true;
            self.added_one = false;
            let mut i = 0usize;
            while i < self.method.code.len() {
                i += self.do_instruction(i);
                if let Some(target) = stop_at
                    && i == target
                    && self.stacks.get(i).is_some_and(|s| s.is_some())
                {
                    self.all_processed = true;
                    break;
                }
                if self.nr_of_entries > MAX_ENTRIES {
                    return;
                }
            }
        }
    }

    fn merge(&mut self, bci: usize, stack: &SimulatedOperandStack) {
        match &mut self.stacks[bci] {
            Some(existing) => {
                let mut merged = stack.clone();
                merged.merge(existing);
                *existing = merged;
            }
            slot @ None => {
                self.nr_of_entries += stack.size();
                *slot = Some(stack.clone());
            }
        }
    }

    /// Processes the instruction at `bci`; returns its length.
    fn do_instruction(&mut self, bci: usize) -> usize {
        let len = bytes::java_length_at(self.method.code, bci);
        if len <= 0 {
            self.all_processed = false;
            self.added_one = false;
            return 1;
        }
        let len = len as usize;
        let Some(stack) = self.stacks[bci].clone() else {
            self.all_processed = false;
            return len;
        };
        let mut stack = stack;
        let mut dest_bci: Option<usize> = None;
        let mut dests: Vec<usize> = Vec::new();
        let mut flow_ended = false;

        let mut pos = bci + 1;
        let mut is_wide = false;
        let mut code = bytes::java_code_at(self.method.code, bci).unwrap_or(Code::Illegal);
        if code == Code::Wide {
            is_wide = true;
            pos += 1;
            code = bytes::java_code_at(self.method.code, bci + 1).unwrap_or(Code::Illegal);
        }

        let depth = bytes::depth(code);
        let result = bytes::result_type(code);

        match code {
            Code::Nop | Code::AconstNull | Code::IconstM1 | Code::Iconst0 | Code::Iconst1
            | Code::Iconst2 | Code::Iconst3 | Code::Iconst4 | Code::Iconst5 | Code::Lconst0
            | Code::Lconst1 | Code::Fconst0 | Code::Fconst1 | Code::Fconst2 | Code::Dconst0
            | Code::Dconst1 | Code::Bipush | Code::Sipush | Code::Iload | Code::Lload
            | Code::Fload | Code::Dload | Code::Aload | Code::Iload0 | Code::Iload1
            | Code::Iload2 | Code::Iload3 | Code::Lload0 | Code::Lload1 | Code::Lload2
            | Code::Lload3 | Code::Fload0 | Code::Fload1 | Code::Fload2 | Code::Fload3
            | Code::Dload0 | Code::Dload1 | Code::Dload2 | Code::Dload3 | Code::Aload0
            | Code::Aload1 | Code::Aload2 | Code::Aload3 | Code::Iinc | Code::New => {
                stack.push_bci(bci as u32, result);
            }
            Code::Ldc | Code::LdcW | Code::Ldc2W => {
                let cp_index = if code == Code::Ldc {
                    be_u1(self.method.code, pos).map(u32::from)
                } else {
                    be_u2(self.method.code, pos).map(u32::from)
                };
                let basic = match cp_index.and_then(|idx| ldc_type(self.method.constants, idx)) {
                    Some(b) => b,
                    None => {
                        self.all_processed = false;
                        self.added_one = false;
                        return len;
                    }
                };
                stack.push_bci(bci as u32, basic);
            }
            Code::Iaload | Code::Faload | Code::Aaload | Code::Baload | Code::Caload
            | Code::Saload | Code::Laload | Code::Daload => {
                stack.pop(2);
                stack.push_bci(bci as u32, result);
            }
            Code::Istore | Code::Lstore | Code::Fstore | Code::Dstore | Code::Astore => {
                let index = if is_wide {
                    be_u2(self.method.code, bci + 2).map(u32::from)
                } else {
                    be_u1(self.method.code, bci + 1).map(u32::from)
                };
                if let Some(index) = index {
                    stack.set_local_slot_written(index as usize);
                }
                stack.pop((-depth).max(0) as usize);
            }
            Code::Istore0 | Code::Lstore0 | Code::Fstore0 | Code::Dstore0 | Code::Astore0 => {
                stack.set_local_slot_written(0);
                stack.pop((-depth).max(0) as usize);
            }
            Code::Istore1 | Code::Lstore1 | Code::Fstore1 | Code::Dstore1 | Code::Astore1 => {
                stack.set_local_slot_written(1);
                stack.pop((-depth).max(0) as usize);
            }
            Code::Istore2 | Code::Lstore2 | Code::Fstore2 | Code::Dstore2 | Code::Astore2 => {
                stack.set_local_slot_written(2);
                stack.pop((-depth).max(0) as usize);
            }
            Code::Istore3 | Code::Lstore3 | Code::Fstore3 | Code::Dstore3 | Code::Astore3 => {
                stack.set_local_slot_written(3);
                stack.pop((-depth).max(0) as usize);
            }
            Code::Iastore | Code::Lastore | Code::Fastore | Code::Dastore | Code::Aastore
            | Code::Bastore | Code::Castore | Code::Sastore | Code::Pop | Code::Pop2
            | Code::Monitorenter | Code::Monitorexit | Code::Breakpoint => {
                stack.pop((-depth).max(0) as usize);
            }
            Code::Dup => {
                if let Some(top) = stack.slot_data(0) {
                    stack.push_raw(top);
                }
            }
            Code::DupX1 => {
                let (Some(top1), Some(top2)) = (stack.slot_data(0), stack.slot_data(1)) else {
                    self.all_processed = false;
                    self.added_one = false;
                    return len;
                };
                stack.pop(2);
                stack.push_raw(top1);
                stack.push_raw(top2);
                stack.push_raw(top1);
            }
            Code::DupX2 => {
                let (Some(top1), Some(top2), Some(top3)) = (
                    stack.slot_data(0),
                    stack.slot_data(1),
                    stack.slot_data(2),
                ) else {
                    self.all_processed = false;
                    self.added_one = false;
                    return len;
                };
                stack.pop(3);
                stack.push_raw(top1);
                stack.push_raw(top3);
                stack.push_raw(top2);
                stack.push_raw(top1);
            }
            Code::Dup2 => {
                if let Some(second) = stack.slot_data(1) {
                    stack.push_raw(second);
                    if let Some(new_second) = stack.slot_data(1) {
                        stack.push_raw(new_second);
                    }
                }
            }
            Code::Dup2X1 => {
                let (Some(top1), Some(top2), Some(top3)) = (
                    stack.slot_data(0),
                    stack.slot_data(1),
                    stack.slot_data(2),
                ) else {
                    self.all_processed = false;
                    self.added_one = false;
                    return len;
                };
                stack.pop(3);
                stack.push_raw(top2);
                stack.push_raw(top1);
                stack.push_raw(top3);
                stack.push_raw(top2);
                stack.push_raw(top1);
            }
            Code::Dup2X2 => {
                let (Some(top1), Some(top2), Some(top3), Some(top4)) = (
                    stack.slot_data(0),
                    stack.slot_data(1),
                    stack.slot_data(2),
                    stack.slot_data(3),
                ) else {
                    self.all_processed = false;
                    self.added_one = false;
                    return len;
                };
                stack.pop(4);
                stack.push_raw(top2);
                stack.push_raw(top1);
                stack.push_raw(top4);
                stack.push_raw(top3);
                stack.push_raw(top2);
                stack.push_raw(top1);
            }
            Code::Swap => {
                let (Some(top1), Some(top2)) = (stack.slot_data(0), stack.slot_data(1)) else {
                    self.all_processed = false;
                    self.added_one = false;
                    return len;
                };
                stack.pop(2);
                stack.push_raw(top1);
                stack.push_raw(top2);
            }
            // iadd..drem, iand..lxor: binary operators.
            c if matches!(c as i32, 96..=115 | 126..=131) => {
                stack.pop(2 * bytes::type2size(result));
                stack.push_bci(bci as u32, result);
            }
            // ineg..dneg.
            c if (116..=119).contains(&(c as i32)) => {
                stack.pop(bytes::type2size(result));
                stack.push_bci(bci as u32, result);
            }
            // Conversions i2l..i2s and l2i..d2f.
            c if (133..=147).contains(&(c as i32)) => {
                let popped = match c as i32 {
                    136..=144 => 2, // l2*/d2* pop a 2-slot value
                    _ => 1,
                };
                stack.pop(popped);
                stack.push_bci(bci as u32, result);
            }
            // lcmp, fcmpl, fcmpg, dcmpl, dcmpg.
            c if (148..=152).contains(&(c as i32)) => {
                stack.pop((1 - depth).max(0) as usize);
                stack.push_bci(bci as u32, BasicType::Int);
            }
            // if*, if_icmp*, if_acmp*, ifnull, ifnonnull.
            c if matches!(c as i32, 153..=166 | 198 | 199) => {
                stack.pop((-depth).max(0) as usize);
                if let Some(off) = be_i2(self.method.code, pos) {
                    dest_bci = bci.checked_add_signed(off as isize);
                }
            }
            Code::Jsr => {
                stack.push_bci(bci as u32, BasicType::Address);
                if let Some(off) = be_i2(self.method.code, pos) {
                    dest_bci = bci.checked_add_signed(off as isize);
                }
                flow_ended = true;
            }
            Code::JsrW => {
                stack.push_bci(bci as u32, BasicType::Address);
                if let Some(off) = be_i4(self.method.code, pos) {
                    dest_bci = bci.checked_add_signed(off as isize);
                }
                flow_ended = true;
            }
            Code::Ret => {
                flow_ended = true;
            }
            Code::Tableswitch => {
                stack.pop(1);
                let aligned = (pos + 3) & !3;
                if let Some(off) = be_i4(self.method.code, aligned) {
                    dest_bci = bci.checked_add_signed(off as isize);
                }
                if let (Some(low), Some(high)) = (
                    be_i4(self.method.code, aligned + 4),
                    be_i4(self.method.code, aligned + 8),
                ) {
                    let mut key = low;
                    while key <= high {
                        if let Some(off) = be_i4(
                            self.method.code,
                            aligned + 12 + 4 * (key - low) as usize,
                        ) {
                            dests.push(bci.wrapping_add_signed(off as isize));
                        }
                        key += 1;
                    }
                }
            }
            Code::Lookupswitch => {
                stack.pop(1);
                let aligned = (pos + 3) & !3;
                if let Some(off) = be_i4(self.method.code, aligned) {
                    dest_bci = bci.checked_add_signed(off as isize);
                }
                if let Some(npairs) = be_i4(self.method.code, aligned + 4) {
                    for p in 0..npairs.max(0) as usize {
                        if let Some(off) =
                            be_i4(self.method.code, aligned + 12 + 8 * p + 4)
                        {
                            dests.push(bci.wrapping_add_signed(off as isize));
                        }
                    }
                }
            }
            Code::Ireturn | Code::Lreturn | Code::Freturn | Code::Dreturn | Code::Areturn
            | Code::Return | Code::Athrow => {
                stack.pop((-depth).max(0) as usize);
                flow_ended = true;
            }
            Code::Getstatic | Code::Getfield => {
                let cp_index = be_u2(self.method.code, pos).map(u32::from);
                let field_type = cp_index
                    .and_then(|idx| field_type_at(self.method.constants, idx))
                    .unwrap_or(BasicType::Illegal);
                stack.pop((1 - depth).max(0) as usize);
                stack.push_bci(bci as u32, field_type);
            }
            Code::Putstatic | Code::Putfield => {
                let cp_index = be_u2(self.method.code, pos).map(u32::from);
                let field_type = cp_index
                    .and_then(|idx| field_type_at(self.method.constants, idx))
                    .unwrap_or(BasicType::Illegal);
                stack.pop(
                    (bytes::type2size(field_type) as i32 - depth - 1).max(0) as usize,
                );
            }
            Code::Invokevirtual | Code::Invokespecial | Code::Invokestatic
            | Code::Invokeinterface | Code::Invokedynamic => {
                let cp_index = if code == Code::Invokedynamic {
                    be_u4(self.method.code, pos)
                } else {
                    be_u2(self.method.code, pos).map(u32::from)
                };
                let descriptor = cp_index
                    .and_then(|idx| signature_at(self.method.constants, idx))
                    .unwrap_or(b"");
                if !matches!(code, Code::Invokestatic | Code::Invokedynamic) {
                    stack.pop(1); // receiver
                }
                stack.pop(argument_size(descriptor));
                stack.push_bci(bci as u32, result_type_of(descriptor));
            }
            Code::Newarray | Code::Anewarray | Code::Instanceof => {
                stack.pop(1);
                stack.push_bci(bci as u32, result);
            }
            Code::Arraylength => {
                stack.pop(1);
                stack.push_bci(bci as u32, BasicType::Int);
            }
            Code::Checkcast => {}
            Code::Multianewarray => {
                let dims = be_u1(self.method.code, pos + 2).unwrap_or(0);
                stack.pop(dims as usize);
                stack.push_bci(bci as u32, BasicType::Object);
            }
            Code::Goto => {
                stack.pop((-depth).max(0) as usize);
                if let Some(off) = be_i2(self.method.code, pos) {
                    dest_bci = bci.checked_add_signed(off as isize);
                }
                flow_ended = true;
            }
            Code::GotoW => {
                stack.pop((-depth).max(0) as usize);
                if let Some(off) = be_i4(self.method.code, pos) {
                    dest_bci = bci.checked_add_signed(off as isize);
                }
                flow_ended = true;
            }
            _ => {
                self.all_processed = false;
                self.added_one = false;
                return len;
            }
        }

        if !flow_ended {
            let next = bci + len;
            if next < self.stacks.len() && self.stacks[next].is_none() {
                self.added_one = true;
            }
            if next < self.stacks.len() {
                self.merge(next, &stack);
            }
        }
        if let Some(target) = dest_bci {
            if target < self.stacks.len() {
                if self.stacks[target].is_none() {
                    self.added_one = true;
                }
                self.merge(target, &stack);
            }
        }
        for target in dests {
            if target < self.stacks.len() {
                if self.stacks[target].is_none() {
                    self.added_one = true;
                }
                self.merge(target, &stack);
            }
        }
        len
    }

    /// Assuming an NPE was thrown at `bci`, returns the operand-stack
    /// slot (0 = TOS) holding the null reference, or a sentinel:
    /// `INVALID_SLOT` for bytecodes that cannot throw NPE,
    /// `EXPLICIT_CONSTRUCTED` for explicitly constructed NPEs.
    pub fn npe_null_slot(&self, bci: usize) -> i32 {
        let mut code = bytes::java_code_at(self.method.code, bci).unwrap_or(Code::Illegal);
        let mut pos = bci + 1;
        if code == Code::Wide {
            code = bytes::java_code_at(self.method.code, bci + 1).unwrap_or(Code::Illegal);
            pos += 1;
        }
        match code {
            Code::Getfield | Code::Arraylength | Code::Athrow | Code::Monitorenter
            | Code::Monitorexit => 0,
            Code::Iaload | Code::Faload | Code::Aaload | Code::Baload | Code::Caload
            | Code::Saload | Code::Laload | Code::Daload => 1,
            Code::Iastore | Code::Fastore | Code::Aastore | Code::Bastore | Code::Castore
            | Code::Sastore => 2,
            Code::Lastore | Code::Dastore => 3,
            Code::Putfield => {
                let cp_index = be_u2(self.method.code, pos).map(u32::from);
                let field_type = cp_index
                    .and_then(|idx| field_type_at(self.method.constants, idx))
                    .unwrap_or(BasicType::Illegal);
                bytes::type2size(field_type) as i32
            }
            Code::Invokevirtual | Code::Invokespecial | Code::Invokeinterface => {
                let cp_index = be_u2(self.method.code, pos).map(u32::from);
                let name = cp_index.and_then(|idx| name_at(self.method.constants, idx));
                if name == Some(b"<init>") {
                    return EXPLICIT_CONSTRUCTED;
                }
                let descriptor = cp_index
                    .and_then(|idx| signature_at(self.method.constants, idx))
                    .unwrap_or(b"");
                argument_size(descriptor) as i32
            }
            _ => INVALID_SLOT,
        }
    }

    /// Prints (into `out`) the expression that evaluated to null, e.g.
    /// `x.field` or `foo()[i]`.  Returns false when nothing could be
    /// determined.
    pub fn print_npe_cause(&self, out: &mut String, bci: usize, slot: usize) -> bool {
        if self.print_npe_cause0(out, bci, slot, MAX_CAUSE_DETAIL, false, " because \"") {
            out.push_str("\" is null");
            return true;
        }
        false
    }

    fn print_npe_cause0(
        &self,
        out: &mut String,
        bci: usize,
        slot: usize,
        max_detail: usize,
        inner_expr: bool,
        prefix: &str,
    ) -> bool {
        if max_detail == 0 || bci >= self.stacks.len() {
            return false;
        }
        let Some(stack) = &self.stacks[bci] else {
            return false;
        };
        let Some(slot_data) = stack.slot_data(slot) else {
            return false;
        };
        if !slot_data.has_bci() {
            return false;
        }
        let source_bci = slot_data.bci() as usize;
        let mut code = bytes::java_code_at(self.method.code, source_bci).unwrap_or(Code::Illegal);
        let mut pos = source_bci + 1;
        let mut is_wide = false;
        if code == Code::Wide {
            is_wide = true;
            code = bytes::java_code_at(self.method.code, source_bci + 1)
                .unwrap_or(Code::Illegal);
            pos += 1;
        }

        if max_detail == MAX_CAUSE_DETAIL
            && !prefix.is_empty()
            && !matches!(
                code,
                Code::Invokevirtual | Code::Invokespecial | Code::Invokestatic | Code::Invokeinterface
            )
        {
            out.push_str(prefix);
        }

        match code {
            Code::Iload0 | Code::Aload0 => {
                self.print_local_var(out, source_bci, 0, !stack.local_slot_was_written(0));
                return true;
            }
            Code::Iload1 | Code::Aload1 => {
                self.print_local_var(out, source_bci, 1, !stack.local_slot_was_written(1));
                return true;
            }
            Code::Iload2 | Code::Aload2 => {
                self.print_local_var(out, source_bci, 2, !stack.local_slot_was_written(2));
                return true;
            }
            Code::Iload3 | Code::Aload3 => {
                self.print_local_var(out, source_bci, 3, !stack.local_slot_was_written(3));
                return true;
            }
            Code::Iload | Code::Aload => {
                let index = if is_wide {
                    be_u2(self.method.code, source_bci + 2).map(u32::from)
                } else {
                    be_u1(self.method.code, source_bci + 1).map(u32::from)
                };
                if let Some(index) = index {
                    self.print_local_var(
                        out,
                        source_bci,
                        index as usize,
                        !stack.local_slot_was_written(index as usize),
                    );
                    return true;
                }
                return false;
            }
            Code::AconstNull => {
                out.push_str("null");
                return true;
            }
            Code::IconstM1 => {
                out.push_str("-1");
                return true;
            }
            Code::Iconst0 | Code::Iconst1 | Code::Iconst2 | Code::Iconst3 | Code::Iconst4
            | Code::Iconst5 => {
                out.push_str(&(code as i32 - Code::Iconst0 as i32).to_string());
                return true;
            }
            _ => {}
        }
        match code {
            Code::Bipush => {
                if let Some(v) = be_u1(self.method.code, source_bci + 1) {
                    out.push_str(&(v as i8).to_string());
                    return true;
                }
            }
            Code::Sipush => {
                if let Some(v) = be_i2(self.method.code, source_bci + 1) {
                    out.push_str(&v.to_string());
                    return true;
                }
            }
            Code::Iaload | Code::Aaload => {
                if !self.print_npe_cause0(out, source_bci, 1, max_detail - 1, inner_expr, "") {
                    out.push_str("<array>");
                }
                out.push('[');
                if !self.print_npe_cause0(out, source_bci, 0, max_detail, true, "") {
                    out.push_str("...");
                }
                out.push(']');
                return true;
            }
            Code::Getstatic => {
                if let Some(cp_index) = be_u2(self.method.code, pos).map(u32::from) {
                    self.print_field_and_class(out, cp_index);
                    return true;
                }
            }
            Code::Getfield => {
                if self.print_npe_cause0(out, source_bci, 0, max_detail - 1, inner_expr, "") {
                    out.push('.');
                }
                if let Some(cp_index) = be_u2(self.method.code, pos).map(u32::from)
                    && let Some(name) = name_at(self.method.constants, cp_index)
                {
                    out.push_str(&String::from_utf8_lossy(name));
                    return true;
                }
            }
            Code::Invokevirtual | Code::Invokespecial | Code::Invokestatic
            | Code::Invokeinterface => {
                if let Some(cp_index) = be_u2(self.method.code, pos).map(u32::from) {
                    if max_detail == MAX_CAUSE_DETAIL && !inner_expr {
                        out.push_str(" because the return value of \"");
                    }
                    self.print_method_name(out, cp_index);
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    /// Prints a description of the action that failed because of the
    /// null reference, e.g. `Cannot invoke "..."`.
    pub fn print_npe_failed_action(&self, out: &mut String, bci: usize) {
        let mut code = bytes::java_code_at(self.method.code, bci).unwrap_or(Code::Illegal);
        let mut pos = bci + 1;
        if code == Code::Wide {
            code = bytes::java_code_at(self.method.code, bci + 1).unwrap_or(Code::Illegal);
            pos += 1;
        }
        match code {
            Code::Iaload => out.push_str("Cannot load from int array"),
            Code::Faload => out.push_str("Cannot load from float array"),
            Code::Aaload => out.push_str("Cannot load from object array"),
            Code::Baload => out.push_str("Cannot load from byte/boolean array"),
            Code::Caload => out.push_str("Cannot load from char array"),
            Code::Saload => out.push_str("Cannot load from short array"),
            Code::Laload => out.push_str("Cannot load from long array"),
            Code::Daload => out.push_str("Cannot load from double array"),
            Code::Iastore => out.push_str("Cannot store to int array"),
            Code::Fastore => out.push_str("Cannot store to float array"),
            Code::Aastore => out.push_str("Cannot store to object array"),
            Code::Bastore => out.push_str("Cannot store to byte/boolean array"),
            Code::Castore => out.push_str("Cannot store to char array"),
            Code::Sastore => out.push_str("Cannot store to short array"),
            Code::Lastore => out.push_str("Cannot store to long array"),
            Code::Dastore => out.push_str("Cannot store to double array"),
            Code::Arraylength => out.push_str("Cannot read the array length"),
            Code::Athrow => out.push_str("Cannot throw exception"),
            Code::Monitorenter => out.push_str("Cannot enter synchronized block"),
            Code::Monitorexit => out.push_str("Cannot exit synchronized block"),
            Code::Getfield => {
                if let Some(cp_index) = be_u2(self.method.code, pos).map(u32::from)
                    && let Some(name) = name_at(self.method.constants, cp_index)
                {
                    out.push_str("Cannot read field \"");
                    out.push_str(&String::from_utf8_lossy(name));
                    out.push('"');
                }
            }
            Code::Putfield => {
                if let Some(cp_index) = be_u2(self.method.code, pos).map(u32::from)
                    && let Some(name) = name_at(self.method.constants, cp_index)
                {
                    out.push_str("Cannot assign field \"");
                    out.push_str(&String::from_utf8_lossy(name));
                    out.push('"');
                }
            }
            Code::Invokevirtual | Code::Invokespecial | Code::Invokestatic
            | Code::Invokeinterface => {
                if let Some(cp_index) = be_u2(self.method.code, pos).map(u32::from) {
                    out.push_str("Cannot invoke \"");
                    self.print_method_name(out, cp_index);
                    out.push('"');
                }
            }
            _ => {}
        }
    }

    fn print_local_var(&self, out: &mut String, bci: usize, slot: usize, is_parameter: bool) {
        for elem in self.method.local_variable_table {
            let start = usize::from(elem.start_pc);
            let end = start + usize::from(elem.length);
            if (start..end).contains(&bci) && usize::from(elem.slot) == slot {
                if let Some(name) = utf8(self.method.constants, elem.name_cp_index) {
                    out.push_str(&String::from_utf8_lossy(name));
                }
                return;
            }
        }
        if !self.method.is_static && slot == 0 && is_parameter {
            out.push_str("this");
            return;
        }
        let first = if self.method.is_static { 0 } else { 1 };
        let mut slot_cursor = first;
        let mut param_index = 1usize;
        let mut found = false;
        if let Ok((params, _)) = signature::method(self.method.descriptor) {
            for param in params.flatten() {
                let size = match param {
                    1 => 2, // J
                    3 => 2, // D
                    _ => 1,
                };
                if (slot >= slot_cursor) && (slot < slot_cursor + size) {
                    found = true;
                    break;
                }
                param_index += 1;
                slot_cursor += size;
            }
        }
        if found && is_parameter {
            out.push_str(&format!("<parameter{param_index}>"));
        } else {
            out.push_str(&format!("<local{slot}>"));
        }
    }

    fn print_method_name(&self, out: &mut String, cp_index: u32) {
        if let Some(class) = class_name_at(self.method.constants, cp_index) {
            push_dotted_class_name(out, class);
        }
        out.push('.');
        if let Some(name) = name_at(self.method.constants, cp_index) {
            out.push_str(&String::from_utf8_lossy(name));
        }
        out.push('(');
        if let Some(desc) = signature_at(self.method.constants, cp_index) {
            push_external_parameters(out, desc);
        }
        out.push_str(")");
    }

    fn print_field_and_class(&self, out: &mut String, cp_index: u32) {
        if let Some(class) = class_name_at(self.method.constants, cp_index) {
            push_dotted_class_name(out, class);
        }
        out.push('.');
        if let Some(name) = name_at(self.method.constants, cp_index) {
            out.push_str(&String::from_utf8_lossy(name));
        }
    }
}

pub const INVALID_SLOT: i32 = -1;
pub const EXPLICIT_CONSTRUCTED: i32 = -2;

fn ldc_type(constants: &[Constant<'_>], cp_index: u32) -> Option<BasicType> {
    match constants.get(cp_index as usize)? {
        Constant::Class(_)
        | Constant::String(_)
        | Constant::FieldRef(..)
        | Constant::MethodRef(..)
        | Constant::InterfaceMethodRef(..)
        | Constant::MethodHandle(..)
        | Constant::MethodType(_)
        | Constant::Dynamic(..)
        | Constant::InvokeDynamic(..) => Some(BasicType::Object),
        Constant::Integer(_) => Some(BasicType::Int),
        Constant::Long(_) => Some(BasicType::Long),
        Constant::Float(_) => Some(BasicType::Float),
        Constant::Double(_) => Some(BasicType::Double),
        _ => None,
    }
}

fn field_type_at(constants: &[Constant<'_>], cp_index: u32) -> Option<BasicType> {
    let desc_index = signature_ref_index(constants, cp_index)?;
    let desc = utf8(constants, desc_index)?;
    Some(signature_basic_type(desc))
}

/// Converts an internal class name (`java/lang/Object`) to the dotted
/// external form with well-known names shortened, as
/// `print_klass_name` does.
fn push_dotted_class_name(out: &mut String, internal: &[u8]) {
    let dotted = String::from_utf8_lossy(internal).replace('/', ".");
    match dotted.as_str() {
        "java.lang.Object" => out.push_str("Object"),
        "java.lang.String" => out.push_str("String"),
        _ => out.push_str(&dotted),
    }
}

/// Prints a method descriptor's parameter types in external form,
/// trimming `java.lang.` prefixes for Object/String like
/// `trim_well_known_class_names_from_signature`.
fn push_external_parameters(out: &mut String, descriptor: &[u8]) {
    let Some(close) = descriptor.iter().position(|&b| b == b')') else {
        return;
    };
    let params = &descriptor[1..close];
    let mut i = 0usize;
    while i < params.len() {
        let start = i;
        while i < params.len() && params[i] == b'[' {
            i += 1;
        }
        if i >= params.len() {
            break;
        }
        let tag = params[i];
        i += 1;
        if tag == b'L' {
            while i < params.len() && params[i] != b';' {
                i += 1;
            }
            i += 1;
        }
        let raw = &params[start..i];
        if raw.first() == Some(&b'L') {
            let inner = &raw[1..raw.len().saturating_sub(1)];
            let dotted = String::from_utf8_lossy(inner).replace('/', ".");
            let trimmed = match dotted.as_str() {
                "java.lang.Object" => "Object".to_string(),
                "java.lang.String" => "String".to_string(),
                _ => dotted,
            };
            for _ in 0..raw.iter().take_while(|&&b| b == b'[').count() {
                out.push('[');
            }
            out.push_str(&trimmed);
            out.push(';');
        } else {
            out.push_str(&String::from_utf8_lossy(raw));
        }
    }
}

// ---------------------------------------------------------------------------
// Public API: BytecodeUtils::get_NPE_message_at
// ---------------------------------------------------------------------------

/// `BytecodeUtils::get_NPE_message_at`: assuming an NPE was thrown at
/// `bci`, returns the precise message (e.g. `Cannot invoke "..." because
/// "x" is null`) or `None` when no message can be produced.
pub fn get_npe_message_at(method: &MethodView<'_>, bci: usize) -> Option<String> {
    if bci >= method.code.len() {
        return None;
    }
    let builder = ExceptionMessageBuilder::new(method, Some(bci));
    let slot = builder.npe_null_slot(bci);
    if slot == EXPLICIT_CONSTRUCTED {
        return None;
    }
    if slot == INVALID_SLOT {
        return None;
    }
    let mut message = String::new();
    builder.print_npe_failed_action(&mut message, bci);
    builder.print_npe_cause(&mut message, bci, slot as usize);
    if message.is_empty() { None } else { Some(message) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operand_decode_covers_jvms_forms() {
        // bipush 5, sipush 0x1234, ldc 0x01, ldc_w 0x0203, ldc2_w 0x0405
        let code = [
            0x10, 0x05, 0x11, 0x12, 0x34, 0x12, 0x01, 0x13, 0x02, 0x03, 0x14, 0x04, 0x05,
        ];
        let b = decode_operands(&code, 0).unwrap();
        assert_eq!(b.push_const, Some(5));
        assert_eq!(decode_operands(&code, 2).unwrap().push_const, Some(0x1234));
        assert_eq!(decode_operands(&code, 5).unwrap().cp_index, Some(1));
        assert_eq!(decode_operands(&code, 7).unwrap().cp_index, Some(0x0203));
        assert_eq!(decode_operands(&code, 10).unwrap().cp_index, Some(0x0405));
    }

    #[test]
    fn wide_expands_local_index() {
        // wide iload 0x0102 ; wide iinc 0x0102 +5 ; wide ret 0x0102
        let code = [
            0xc4, 0x15, 0x01, 0x02, 0xc4, 0x84, 0x01, 0x02, 0x00, 0x05, 0xc4, 0xa9, 0x01, 0x02,
        ];
        let b = decode_operands(&code, 0).unwrap();
        assert!(b.is_wide);
        assert_eq!(b.code, Code::Iload);
        assert_eq!(b.local_index, Some(0x0102));
        let b = decode_operands(&code, 4).unwrap();
        assert_eq!(b.code, Code::Iinc);
        assert_eq!(b.local_index, Some(0x0102));
        assert_eq!(b.const_value, Some(5));
        let b = decode_operands(&code, 10).unwrap();
        assert_eq!(b.code, Code::Ret);
        assert_eq!(b.local_index, Some(0x0102));
    }

    #[test]
    fn tableswitch_and_lookupswitch_lengths() {
        // tableswitch at bci 0: pad 3 bytes then default,low,high,1 offset.
        let mut code = vec![0xaa, 0, 0, 0];
        code.extend_from_slice(&0i32.to_be_bytes()); // default -> 0
        code.extend_from_slice(&5i32.to_be_bytes()); // low
        code.extend_from_slice(&5i32.to_be_bytes()); // high
        code.extend_from_slice(&7i32.to_be_bytes()); // offset
        assert_eq!(bytes::special_length_at(&code, 0), 20);
        // lookupswitch: 1 pair -> 8+8+8=24 plus padding.
        let mut code = vec![0xab, 0, 0, 0];
        code.extend_from_slice(&0i32.to_be_bytes());
        code.extend_from_slice(&1i32.to_be_bytes()); // npairs
        code.extend_from_slice(&9i32.to_be_bytes());
        code.extend_from_slice(&3i32.to_be_bytes());
        assert_eq!(bytes::special_length_at(&code, 0), 20);
        let targets = switch_targets(&code, 0).unwrap();
        assert_eq!(targets, vec![0, 3]);
    }

    #[test]
    fn operand_ranges_mark_used_bytes() {
        // getstatic 0x0007 ; invokeinterface cp=1 count=2 zero=0 ; multianewarray
        let code = [
            0xb2, 0x00, 0x07, 0xb9, 0x00, 0x01, 0x02, 0x00, 0xc5, 0x00, 0x02, 0x03,
        ];
        assert_eq!(operand_ranges(&code, 0), vec![(1, 3)]);
        assert_eq!(operand_ranges(&code, 3), vec![(4, 8)]);
        assert_eq!(operand_ranges(&code, 8), vec![(9, 11), (11, 12)]);
    }

    #[test]
    fn npe_message_for_invokevirtual_on_null() {
        use crate::share::classfile::classFileParser::Constant;
        // CP: 1=Utf8 java/lang/Object, 2=Class(1), 3=Utf8 "foo",
        // 4=Utf8 "()V", 5=NameAndType(3,4), 6=MethodRef(2,5)
        let utf8_obj: &[u8] = b"java/lang/Object";
        let utf8_foo: &[u8] = b"foo";
        let utf8_sig: &[u8] = b"()V";
        let constants = [
            Constant::Unusable,
            Constant::Utf8(utf8_obj),
            Constant::Class(1),
            Constant::Utf8(utf8_foo),
            Constant::Utf8(utf8_sig),
            Constant::NameAndType(3, 4),
            Constant::MethodRef(2, 5),
        ];
        // aload_0 ; invokevirtual #6 ; return
        let code = [0x2a, 0xb6, 0x00, 0x06, 0xb1];
        let method = MethodView::new(&code, &[], &constants, false, b"m", b"()V", &[]);
        let msg = get_npe_message_at(&method, 1).expect("message");
        assert!(msg.contains("Cannot invoke"), "{msg}");
        assert!(msg.contains("Object.foo()"), "{msg}");
    }
}
