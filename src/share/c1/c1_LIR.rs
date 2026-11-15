use super::c1_IR::ControlFlowGraph;
use super::c1_Instruction::{
    ArrayElemKind, CallInfo, CompareKind, Condition, ConvertKind, HirInstruction, ValueKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LirKind {
    Int32,
    Int64,
    Float32,
    Float64,
    Ref,
    Void,
}

impl LirKind {
    pub fn from_value_kind(kind: ValueKind) -> Self {
        match kind {
            ValueKind::Int32 => Self::Int32,
            ValueKind::Int64 => Self::Int64,
            ValueKind::Float32 => Self::Float32,
            ValueKind::Float64 => Self::Float64,
            ValueKind::Ref => Self::Ref,
            ValueKind::Void => Self::Void,
        }
    }

    pub fn to_value_kind(self) -> ValueKind {
        match self {
            Self::Int32 => ValueKind::Int32,
            Self::Int64 => ValueKind::Int64,
            Self::Float32 => ValueKind::Float32,
            Self::Float64 => ValueKind::Float64,
            Self::Ref => ValueKind::Ref,
            Self::Void => ValueKind::Void,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LirOperand {
    ConstI32(i32),
    ConstI64(i64),
    ConstF32(u32),
    ConstF64(u64),
    Null,
    Local { slot: usize, kind: LirKind },
    Stack { slot: usize, kind: LirKind },
    Temp { slot: usize, kind: LirKind },
}

impl LirOperand {
    pub fn kind(&self) -> LirKind {
        match self {
            Self::ConstI32(_) => LirKind::Int32,
            Self::ConstI64(_) => LirKind::Int64,
            Self::ConstF32(_) => LirKind::Float32,
            Self::ConstF64(_) => LirKind::Float64,
            Self::Null => LirKind::Ref,
            Self::Local { kind, .. } | Self::Stack { kind, .. } | Self::Temp { kind, .. } => *kind,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimeStub {
    SafepointPoll,
    Deoptimize,
    ArithmeticException,
    NullPointerException,
    ArrayIndexOutOfBounds,
    ArrayStoreException,
    NewInstance,
    NewArray,
    AnewArray,
    MultianewArray,
    CheckCast,
    InstanceOf,
    MonitorEnter,
    MonitorExit,
    Throw,
    UnresolvedField,
    UnresolvedCall,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LirOp {
    Nop,
    LoadLocal { dst: LirOperand, slot: usize, kind: LirKind },
    StoreLocal { slot: usize, src: LirOperand, kind: LirKind },
    LoadStack { dst: LirOperand, slot: usize, kind: LirKind },
    StoreStack { slot: usize, src: LirOperand, kind: LirKind },
    Move { dst: LirOperand, src: LirOperand, kind: LirKind },
    Add { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Sub { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Mul { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Div { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Rem { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Neg { dst: LirOperand, src: LirOperand, kind: LirKind },
    And { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Or { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Xor { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Shl { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Shr { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Ushr { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    Convert { dst: LirOperand, src: LirOperand, kind: ConvertKind },
    CompareValue { dst: LirOperand, lhs: LirOperand, rhs: LirOperand, kind: CompareKind },
    Compare { lhs: LirOperand, rhs: LirOperand, kind: LirKind },
    LoadField { dst: LirOperand, obj: LirOperand, offset: usize, kind: LirKind },
    StoreField { obj: LirOperand, offset: usize, value: LirOperand, kind: LirKind },
    LoadIndexed { dst: LirOperand, array: LirOperand, index: LirOperand, elem: ArrayElemKind },
    StoreIndexed { array: LirOperand, index: LirOperand, value: LirOperand, elem: ArrayElemKind },
    ArrayLength { dst: LirOperand, array: LirOperand },
    Call { dst: Option<LirOperand>, operands: Vec<LirOperand>, info: CallInfo },
    Return { value: Option<LirOperand>, kind: LirKind },
    Goto { target: usize },
    Branch { cond: Condition, target_true: usize, target_false: usize },
    TableSwitch { index: LirOperand, low: i32, high: i32, default_block: usize, targets: Vec<usize> },
    LookupSwitch { key: LirOperand, pairs: Vec<(i32, usize)>, default_block: usize },
    Safepoint { bci: usize },
    CallRuntime { stub: RuntimeStub, operands: Vec<LirOperand>, dst: Option<LirOperand>, bci: usize },
    Unsupported { opcode: u8, bci: usize, reason: String },
}

impl LirOp {
    pub fn bci(&self) -> Option<usize> {
        match self {
            Self::CallRuntime { bci, .. } | Self::Unsupported { bci, .. } | Self::Safepoint { bci } => Some(*bci),
            _ => None,
        }
    }

    pub fn is_safepoint(&self) -> bool {
        matches!(self, Self::Safepoint { .. } | Self::Call { .. } | Self::CallRuntime { stub: RuntimeStub::SafepointPoll, .. })
    }
}

#[derive(Clone, Debug)]
pub struct LirBlock {
    pub id: usize,
    pub bci: usize,
    pub instructions: Vec<LirOp>,
}

#[derive(Clone, Debug)]
pub struct LirProgram {
    pub blocks: Vec<LirBlock>,
    pub entry_block: usize,
    pub local_count: usize,
    pub stack_count: usize,
    pub temp_count: usize,
    pub frame_slots: usize,
}

pub struct LirGenerator {
    temp_count: usize,
    max_temps: usize,
    local_count: usize,
}

impl LirGenerator {
    pub fn new() -> Self {
        Self { temp_count: 0, max_temps: 0, local_count: 0 }
    }

    pub fn generate(cfg: &ControlFlowGraph) -> LirProgram {
        let mut generator = Self::new();
        generator.local_count = scan_local_count(cfg).max(cfg.max_locals);
        let order = cfg.reverse_postorder();
        let mut blocks = Vec::with_capacity(order.len());
        for id in order {
            if let Some(hir) = cfg.block(id) {
                let mut lir = LirBlock { id: hir.id, bci: hir.begin.bci, instructions: Vec::with_capacity(hir.instructions.len() * 2) };
                if id == cfg.entry_block || hir.begin.is_exception_entry {
                    lir.instructions.push(LirOp::Safepoint { bci: hir.begin.bci });
                }
                for inst in &hir.instructions {
                    generator.temp_count = 0;
                    generator.emit_instruction(&mut lir.instructions, inst);
                }
                blocks.push(lir);
            }
        }
        if blocks.is_empty() && !cfg.blocks.is_empty() {
            for hir in &cfg.blocks {
                let mut lir = LirBlock { id: hir.id, bci: hir.begin.bci, instructions: Vec::new() };
                for inst in &hir.instructions {
                    generator.temp_count = 0;
                    generator.emit_instruction(&mut lir.instructions, inst);
                }
                blocks.push(lir);
            }
        }
        let stack_count = cfg.max_stack_entries;
        let temp_count = generator.max_temps;
        let frame_slots = generator.local_count + stack_count + temp_count;
        LirProgram { blocks, entry_block: cfg.entry_block, local_count: generator.local_count, stack_count, temp_count, frame_slots }
    }

    fn alloc_temp(&mut self, kind: LirKind) -> LirOperand {
        let slot = self.temp_count;
        self.temp_count += 1;
        self.max_temps = self.max_temps.max(self.temp_count);
        LirOperand::Temp { slot, kind }
    }

    fn emit_instruction(&mut self, out: &mut Vec<LirOp>, inst: &HirInstruction) {
        match inst {
            HirInstruction::StoreLocal { slot, kind, value } => {
                let src = self.emit_expr(out, value);
                out.push(LirOp::StoreLocal { slot: *slot, src, kind: LirKind::from_value_kind(*kind) });
            }
            HirInstruction::StoreStack { slot, kind, value } => {
                let src = self.emit_expr(out, value);
                out.push(LirOp::StoreStack { slot: *slot, src, kind: LirKind::from_value_kind(*kind) });
            }
            HirInstruction::Call { info, operands } => {
                if info.return_kind == ValueKind::Void {
                    let ops = self.emit_operands(out, operands);
                    out.push(LirOp::Call { dst: None, operands: ops, info: info.clone() });
                } else {
                    let _ = self.emit_expr(out, inst);
                }
            }
            HirInstruction::MonitorEnter(v) => {
                let obj = self.emit_expr(out, v);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::MonitorEnter, operands: vec![obj], dst: None, bci: 0 });
            }
            HirInstruction::MonitorExit(v) => {
                let obj = self.emit_expr(out, v);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::MonitorExit, operands: vec![obj], dst: None, bci: 0 });
            }
            HirInstruction::Throw(v) => {
                let obj = self.emit_expr(out, v);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::Throw, operands: vec![obj], dst: None, bci: 0 });
            }
            HirInstruction::CheckCast { value, cp_index, .. } => {
                let v = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::CheckCast, operands: vec![v, LirOperand::ConstI64(*cp_index as i64)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::InstanceOf { value, cp_index, .. } => {
                let v = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::InstanceOf, operands: vec![v, LirOperand::ConstI64(*cp_index as i64)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::NewArray { elem, length } => {
                let len = self.emit_expr(out, length);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewArray, operands: vec![len, LirOperand::ConstI32(elem_code(*elem))], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::NewObject { cp_index, .. } => {
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewInstance, operands: vec![LirOperand::ConstI64(*cp_index as i64)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::LoadField { obj, offset, kind, .. } => {
                if let Some(offset) = offset {
                    let obj = self.emit_expr(out, obj);
                    let dst = self.alloc_temp(LirKind::from_value_kind(*kind));
                    out.push(LirOp::LoadField { dst, obj, offset: *offset, kind: LirKind::from_value_kind(*kind) });
                } else {
                    let obj_op = self.emit_expr(out, obj);
                    out.push(LirOp::CallRuntime { stub: RuntimeStub::UnresolvedField, operands: vec![obj_op], dst: None, bci: 0 });
                }
            }
            HirInstruction::StoreField { obj, offset, value, kind, .. } => {
                if let Some(offset) = offset {
                    let obj = self.emit_expr(out, obj);
                    let value = self.emit_expr(out, value);
                    out.push(LirOp::StoreField { obj, offset: *offset, value, kind: LirKind::from_value_kind(*kind) });
                } else {
                    let _ = self.emit_expr(out, obj);
                    let _ = self.emit_expr(out, value);
                    out.push(LirOp::CallRuntime { stub: RuntimeStub::UnresolvedField, operands: Vec::new(), dst: None, bci: 0 });
                }
            }
            HirInstruction::GetStatic { .. } | HirInstruction::PutStatic { .. } => {
                out.push(LirOp::CallRuntime { stub: RuntimeStub::UnresolvedField, operands: Vec::new(), dst: None, bci: 0 });
            }
            HirInstruction::LoadIndexed { array, index, elem } => {
                let array = self.emit_expr(out, array);
                let index = self.emit_expr(out, index);
                let dst = self.alloc_temp(LirKind::from_value_kind(elem.value_kind()));
                out.push(LirOp::LoadIndexed { dst, array, index, elem: *elem });
            }
            HirInstruction::StoreIndexed { array, index, value, elem } => {
                let array = self.emit_expr(out, array);
                let index = self.emit_expr(out, index);
                let value = self.emit_expr(out, value);
                out.push(LirOp::StoreIndexed { array, index, value, elem: *elem });
            }
            HirInstruction::ArrayLength(array) => {
                let array = self.emit_expr(out, array);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::ArrayLength { dst, array });
            }
            HirInstruction::Return(value) => {
                let value = value.as_ref().map(|v| self.emit_expr(out, v));
                let kind = value.as_ref().map(|v| v.kind()).unwrap_or(LirKind::Void);
                out.push(LirOp::Return { value, kind });
            }
            HirInstruction::Goto(target) => {
                out.push(LirOp::Goto { target: *target });
            }
            HirInstruction::If { left, cond, right, true_block, false_block } => {
                let lhs = self.emit_expr(out, left);
                let rhs = self.emit_expr(out, right);
                let kind = compare_kind_for(&lhs, &rhs);
                out.push(LirOp::Compare { lhs, rhs, kind });
                out.push(LirOp::Branch { cond: *cond, target_true: *true_block, target_false: *false_block });
            }
            HirInstruction::TableSwitch { index, low, high, default_block, targets } => {
                let index = self.emit_expr(out, index);
                out.push(LirOp::TableSwitch { index, low: *low, high: *high, default_block: *default_block, targets: targets.clone() });
            }
            HirInstruction::LookupSwitch { key, pairs, default_block } => {
                let key = self.emit_expr(out, key);
                out.push(LirOp::LookupSwitch { key, pairs: pairs.clone(), default_block: *default_block });
            }
            HirInstruction::Unsupported { opcode, bci, reason } => {
                out.push(LirOp::Unsupported { opcode: *opcode, bci: *bci, reason: reason.clone() });
            }
            _ => {
                let _ = self.emit_expr(out, inst);
            }
        }
    }

    fn emit_operands(&mut self, out: &mut Vec<LirOp>, operands: &[Box<HirInstruction>]) -> Vec<LirOperand> {
        operands.iter().map(|o| self.emit_expr(out, o)).collect()
    }

    fn emit_expr(&mut self, out: &mut Vec<LirOp>, expr: &HirInstruction) -> LirOperand {
        match expr {
            HirInstruction::ConstantInt(v) => LirOperand::ConstI32(*v),
            HirInstruction::ConstantLong(v) => LirOperand::ConstI64(*v),
            HirInstruction::ConstantFloat(v) => LirOperand::ConstF32(*v),
            HirInstruction::ConstantDouble(v) => LirOperand::ConstF64(*v),
            HirInstruction::ConstantNull => LirOperand::Null,
            HirInstruction::LoadLocal { slot, kind } => LirOperand::Local { slot: *slot, kind: LirKind::from_value_kind(*kind) },
            HirInstruction::LoadStack { slot, kind } => LirOperand::Stack { slot: *slot, kind: LirKind::from_value_kind(*kind) },
            HirInstruction::Neg(v) => {
                let src = self.emit_expr(out, v);
                let kind = src.kind();
                let dst = self.alloc_temp(kind);
                out.push(LirOp::Neg { dst: dst.clone(), src, kind });
                dst
            }
            HirInstruction::Convert { kind, value } => {
                let src = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::from_value_kind(convert_result(*kind)));
                out.push(LirOp::Convert { dst: dst.clone(), src, kind: *kind });
                dst
            }
            HirInstruction::Compare { kind, left, right } => {
                let lhs = self.emit_expr(out, left);
                let rhs = self.emit_expr(out, right);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::CompareValue { dst: dst.clone(), lhs, rhs, kind: *kind });
                dst
            }
            HirInstruction::Call { info, operands } => {
                let ops = self.emit_operands(out, operands);
                if info.return_kind == ValueKind::Void {
                    out.push(LirOp::Call { dst: None, operands: ops, info: info.clone() });
                    LirOperand::ConstI32(0)
                } else {
                    let dst = self.alloc_temp(LirKind::from_value_kind(info.return_kind));
                    out.push(LirOp::Call { dst: Some(dst.clone()), operands: ops, info: info.clone() });
                    dst
                }
            }
            HirInstruction::CheckCast { value, cp_index, .. } => {
                let v = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::CheckCast, operands: vec![v, LirOperand::ConstI64(*cp_index as i64)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::InstanceOf { value, cp_index, .. } => {
                let v = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::InstanceOf, operands: vec![v, LirOperand::ConstI64(*cp_index as i64)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::NewArray { elem, length } => {
                let len = self.emit_expr(out, length);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewArray, operands: vec![len, LirOperand::ConstI32(elem_code(*elem))], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::NewObject { cp_index, .. } => {
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewInstance, operands: vec![LirOperand::ConstI64(*cp_index as i64)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::LoadField { obj, offset, kind, .. } => {
                if let Some(offset) = offset {
                    let obj = self.emit_expr(out, obj);
                    let dst = self.alloc_temp(LirKind::from_value_kind(*kind));
                    out.push(LirOp::LoadField { dst: dst.clone(), obj, offset: *offset, kind: LirKind::from_value_kind(*kind) });
                    dst
                } else {
                    let obj_op = self.emit_expr(out, obj);
                    out.push(LirOp::CallRuntime { stub: RuntimeStub::UnresolvedField, operands: vec![obj_op], dst: None, bci: 0 });
                    LirOperand::ConstI32(0)
                }
            }
            HirInstruction::LoadIndexed { array, index, elem } => {
                let array = self.emit_expr(out, array);
                let index = self.emit_expr(out, index);
                let dst = self.alloc_temp(LirKind::from_value_kind(elem.value_kind()));
                out.push(LirOp::LoadIndexed { dst: dst.clone(), array, index, elem: *elem });
                dst
            }
            HirInstruction::ArrayLength(array) => {
                let array = self.emit_expr(out, array);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::ArrayLength { dst: dst.clone(), array });
                dst
            }
            HirInstruction::Add(l, r) => self.emit_binary(out, expr, l, r, Binary::Add),
            HirInstruction::Sub(l, r) => self.emit_binary(out, expr, l, r, Binary::Sub),
            HirInstruction::Mul(l, r) => self.emit_binary(out, expr, l, r, Binary::Mul),
            HirInstruction::Div(l, r) => self.emit_binary(out, expr, l, r, Binary::Div),
            HirInstruction::Rem(l, r) => self.emit_binary(out, expr, l, r, Binary::Rem),
            HirInstruction::And(l, r) => self.emit_binary(out, expr, l, r, Binary::And),
            HirInstruction::Or(l, r) => self.emit_binary(out, expr, l, r, Binary::Or),
            HirInstruction::Xor(l, r) => self.emit_binary(out, expr, l, r, Binary::Xor),
            HirInstruction::Shl(l, r) => self.emit_binary(out, expr, l, r, Binary::Shl),
            HirInstruction::Shr(l, r) => self.emit_binary(out, expr, l, r, Binary::Shr),
            HirInstruction::Ushr(l, r) => self.emit_binary(out, expr, l, r, Binary::Ushr),
            HirInstruction::Unsupported { opcode, bci, reason } => {
                out.push(LirOp::Unsupported { opcode: *opcode, bci: *bci, reason: reason.clone() });
                LirOperand::ConstI32(0)
            }
            _ => {
                out.push(LirOp::Unsupported { opcode: 0, bci: 0, reason: "expression statement".to_string() });
                LirOperand::ConstI32(0)
            }
        }
    }

    fn emit_binary(&mut self, out: &mut Vec<LirOp>, expr: &HirInstruction, l: &Box<HirInstruction>, r: &Box<HirInstruction>, op: Binary) -> LirOperand {
        let lhs = self.emit_expr(out, l);
        let rhs = self.emit_expr(out, r);
        let kind = binary_kind(expr.value_kind());
        let dst = self.alloc_temp(kind);
        let (lhs, rhs) = normalize_operands(lhs, rhs, kind);
        match op {
            Binary::Add => out.push(LirOp::Add { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Sub => out.push(LirOp::Sub { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Mul => out.push(LirOp::Mul { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Div => out.push(LirOp::Div { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Rem => out.push(LirOp::Rem { dst: dst.clone(), lhs, rhs, kind }),
            Binary::And => out.push(LirOp::And { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Or => out.push(LirOp::Or { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Xor => out.push(LirOp::Xor { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Shl => out.push(LirOp::Shl { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Shr => out.push(LirOp::Shr { dst: dst.clone(), lhs, rhs, kind }),
            Binary::Ushr => out.push(LirOp::Ushr { dst: dst.clone(), lhs, rhs, kind }),
        }
        dst
    }
}

#[derive(Clone, Copy)]
enum Binary {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Ushr,
}

fn scan_local_count(cfg: &ControlFlowGraph) -> usize {
    let mut max = 0;
    for block in &cfg.blocks {
        for inst in &block.instructions {
            scan_instr(inst, &mut max);
        }
    }
    max
}

fn scan_instr(inst: &HirInstruction, max: &mut usize) {
    match inst {
        HirInstruction::LoadLocal { slot, .. } => *max = (*max).max(*slot + 1),
        HirInstruction::StoreLocal { slot, value, .. } => {
            *max = (*max).max(*slot + 1);
            scan_instr(value, max);
        }
        HirInstruction::StoreStack { value, .. } => scan_instr(value, max),
        HirInstruction::Add(l, r) | HirInstruction::Sub(l, r) | HirInstruction::Mul(l, r) | HirInstruction::Div(l, r)
        | HirInstruction::Rem(l, r) | HirInstruction::And(l, r) | HirInstruction::Or(l, r) | HirInstruction::Xor(l, r)
        | HirInstruction::Shl(l, r) | HirInstruction::Shr(l, r) | HirInstruction::Ushr(l, r) => {
            scan_instr(l, max);
            scan_instr(r, max);
        }
        HirInstruction::Neg(v) | HirInstruction::Convert { value: v, .. } | HirInstruction::ArrayLength(v)
        | HirInstruction::CheckCast { value: v, .. } | HirInstruction::InstanceOf { value: v, .. }
        | HirInstruction::MonitorEnter(v) | HirInstruction::MonitorExit(v) | HirInstruction::Throw(v) => scan_instr(v, max),
        HirInstruction::Compare { left, right, .. } | HirInstruction::If { left, right, .. } => {
            scan_instr(left, max);
            scan_instr(right, max);
        }
        HirInstruction::LoadIndexed { array, index, .. } => { scan_instr(array, max); scan_instr(index, max); }
        HirInstruction::StoreIndexed { array, index, value, .. } => { scan_instr(array, max); scan_instr(index, max); scan_instr(value, max); }
        HirInstruction::NewArray { length, .. } => scan_instr(length, max),
        HirInstruction::LoadField { obj, .. } => scan_instr(obj, max),
        HirInstruction::StoreField { obj, value, .. } => { scan_instr(obj, max); scan_instr(value, max); }
        HirInstruction::PutStatic { value, .. } => scan_instr(value, max),
        HirInstruction::Call { operands, .. } => for o in operands { scan_instr(o, max); },
        HirInstruction::Return(v) => if let Some(v) = v { scan_instr(v, max); },
        HirInstruction::TableSwitch { index, .. } => scan_instr(index, max),
        HirInstruction::LookupSwitch { key, .. } => scan_instr(key, max),
        _ => {}
    }
}

fn binary_kind(kind: ValueKind) -> LirKind {
    match kind {
        ValueKind::Int64 => LirKind::Int64,
        ValueKind::Float32 => LirKind::Float32,
        ValueKind::Float64 => LirKind::Float64,
        ValueKind::Ref => LirKind::Ref,
        _ => LirKind::Int32,
    }
}

fn normalize_operands(lhs: LirOperand, rhs: LirOperand, kind: LirKind) -> (LirOperand, LirOperand) {
    let lhs = coerce_operand(lhs, kind);
    let rhs = coerce_operand(rhs, kind);
    (lhs, rhs)
}

fn coerce_operand(op: LirOperand, kind: LirKind) -> LirOperand {
    match (op, kind) {
        (LirOperand::ConstI32(v), LirKind::Int64) => LirOperand::ConstI64(v as i64),
        (LirOperand::ConstI64(v), LirKind::Int32) => LirOperand::ConstI32(v as i32),
        (LirOperand::Null, _) => LirOperand::Null,
        (o, _) => o,
    }
}

fn compare_kind_for(lhs: &LirOperand, rhs: &LirOperand) -> LirKind {
    match (lhs.kind(), rhs.kind()) {
        (LirKind::Int64, _) | (_, LirKind::Int64) => LirKind::Int64,
        (LirKind::Ref, _) | (_, LirKind::Ref) => LirKind::Ref,
        _ => LirKind::Int32,
    }
}

fn convert_result(kind: ConvertKind) -> ValueKind {
    match kind {
        ConvertKind::I2L | ConvertKind::F2L | ConvertKind::D2L => ValueKind::Int64,
        ConvertKind::I2F | ConvertKind::L2F | ConvertKind::D2F => ValueKind::Float32,
        ConvertKind::I2D | ConvertKind::L2D | ConvertKind::F2D => ValueKind::Float64,
        _ => ValueKind::Int32,
    }
}

fn elem_code(elem: ArrayElemKind) -> i32 {
    match elem {
        ArrayElemKind::Boolean => 4,
        ArrayElemKind::Byte => 5,
        ArrayElemKind::Char => 6,
        ArrayElemKind::Short => 7,
        ArrayElemKind::Int => 8,
        ArrayElemKind::Long => 9,
        ArrayElemKind::Float => 10,
        ArrayElemKind::Double => 11,
        ArrayElemKind::Ref => 12,
    }
}
