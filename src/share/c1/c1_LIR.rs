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
    GetField,
    PutField,
    GetStatic,
    PutStatic,
    ArrayLength,
    ArrayLoad,
    ArrayStore,
    FloatRem,
    FloatConvert,
    UnresolvedField,
    UnresolvedCall,
}

/// Resolved constant-pool site passed to JIT runtime stubs.  One
/// instance is leaked per heap-access site at compile time; the
/// pointer travels through the generated code as a `ConstI64`
/// operand and the stub handlers dereference it to recover the
/// owner class / member name / descriptor.
#[repr(C)]
pub struct StubSite {
    pub class: *const u8,
    pub class_len: usize,
    pub name: *const u8,
    pub name_len: usize,
    pub desc: *const u8,
    pub desc_len: usize,
}

impl StubSite {
    /// Borrows the site back as byte slices.  Unsafe: the pointer must
    /// come from `stub_site_ptr` (a leaked, immutable allocation).
    pub unsafe fn as_slices(&self) -> (&'static [u8], &'static [u8], &'static [u8]) {
        (
            unsafe { std::slice::from_raw_parts(self.class, self.class_len) },
            unsafe { std::slice::from_raw_parts(self.name, self.name_len) },
            unsafe { std::slice::from_raw_parts(self.desc, self.desc_len) },
        )
    }
}

fn leak_bytes(bytes: &[u8]) -> *const u8 {
    let leaked: &'static mut [u8] = Box::leak(bytes.to_vec().into_boxed_slice());
    leaked.as_ptr()
}

/// Registers `info` in the leaked-site table and returns its stable
/// pointer as a `u64` operand.  Returns 0 when the constant-pool
/// index was not resolved — stubs treat site 0 as "cannot service"
/// and deoptimize, which is always safe.
pub fn stub_site_ptr(info: Option<&super::c1_GraphBuilder::SiteInfo>) -> u64 {
    let Some(info) = info else { return 0 };
    let site = Box::leak(Box::new(StubSite {
        class: leak_bytes(info.class.as_bytes()),
        class_len: info.class.len(),
        name: leak_bytes(info.name.as_bytes()),
        name_len: info.name.len(),
        desc: leak_bytes(info.desc.as_bytes()),
        desc_len: info.desc.len(),
    }));
    site as *const StubSite as u64
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

pub struct LirGenerator<'a> {
    temp_count: usize,
    max_temps: usize,
    local_count: usize,
    metadata: &'a super::c1_GraphBuilder::MethodMetadata,
}

impl<'a> LirGenerator<'a> {
    pub fn new(metadata: &'a super::c1_GraphBuilder::MethodMetadata) -> Self {
        Self { temp_count: 0, max_temps: 0, local_count: 0, metadata }
    }

    /// Resolves a constant-pool index to a leaked `StubSite` pointer
    /// operand for `CallRuntime` stubs (0 = unresolved -> deopt).
    fn site_operand(&self, cp_index: u32) -> LirOperand {
        LirOperand::ConstI64(stub_site_ptr(self.metadata.sites.get(&cp_index)) as i64)
    }

    /// Raw leaked `StubSite` pointer for invoke call sites; carried on
    /// `CallInfo.site` so the x86 backend can feed the CallMethod stub
    /// without another metadata lookup.
    fn call_site_ptr(&self, cp_index: u32) -> u64 {
        stub_site_ptr(self.metadata.sites.get(&cp_index))
    }

    fn resolved_call_info(&self, info: &CallInfo) -> CallInfo {
        let mut info = info.clone();
        info.site = self.call_site_ptr(info.cp_index);
        info
    }

    pub fn generate(cfg: &ControlFlowGraph, metadata: &'a super::c1_GraphBuilder::MethodMetadata) -> LirProgram {
        let mut generator = Self::new(metadata);
        generator.local_count = scan_local_count(cfg).max(cfg.max_locals);
        let order = cfg.reverse_postorder();
        // Loop headers: blocks targeted by a backward edge (a successor
        // that appears no later in reverse postorder). Compiled code has
        // no interpreter dispatch loop, so these headers are the only
        // places a long-running compiled loop can reach a GC safepoint —
        // a `Safepoint` LIR op is emitted at their entry just like
        // HotSpot's loop safepoint polls.
        let pos: std::collections::HashMap<usize, usize> = order
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();
        let mut loop_headers: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &id in &order {
            let Some(hir) = cfg.block(id) else { continue };
            let my = pos.get(&id).copied().unwrap_or(usize::MAX);
            for inst in &hir.instructions {
                for target in inst.successors() {
                    if pos.get(&target).map_or(false, |&p| p <= my) {
                        loop_headers.insert(target);
                    }
                }
            }
        }
        let mut blocks = Vec::with_capacity(order.len());
        for id in order {
            if let Some(hir) = cfg.block(id) {
                let mut lir = LirBlock { id: hir.id, bci: hir.begin.bci, instructions: Vec::with_capacity(hir.instructions.len() * 2) };
                if id == cfg.entry_block || hir.begin.is_exception_entry || loop_headers.contains(&id) {
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
                    let info = self.resolved_call_info(info);
                    let ops = self.emit_operands(out, operands);
                    out.push(LirOp::Call { dst: None, operands: ops, info });
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
                out.push(LirOp::CallRuntime { stub: RuntimeStub::CheckCast, operands: vec![v, self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::InstanceOf { value, cp_index, .. } => {
                let v = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::InstanceOf, operands: vec![v, self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::NewArray { elem, length } => {
                let len = self.emit_expr(out, length);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewArray, operands: vec![len, LirOperand::ConstI32(elem_code(*elem))], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::AnewArray { cp_index, length } => {
                let site = self.site_operand(*cp_index);
                let len = self.emit_expr(out, length);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::AnewArray, operands: vec![site, len], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::NewObject { cp_index, .. } => {
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewInstance, operands: vec![self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::LoadField { obj, cp_index, kind, .. } => {
                let obj = self.emit_expr(out, obj);
                let dst = self.alloc_temp(LirKind::from_value_kind(*kind));
                out.push(LirOp::CallRuntime { stub: RuntimeStub::GetField, operands: vec![obj, self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::StoreField { obj, cp_index, value, .. } => {
                let obj = self.emit_expr(out, obj);
                let value = self.emit_expr(out, value);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::PutField, operands: vec![obj, self.site_operand(*cp_index), value], dst: None, bci: 0 });
            }
            HirInstruction::GetStatic { cp_index, kind, .. } => {
                let dst = self.alloc_temp(LirKind::from_value_kind(*kind));
                out.push(LirOp::CallRuntime { stub: RuntimeStub::GetStatic, operands: vec![self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::PutStatic { cp_index, value, .. } => {
                let value = self.emit_expr(out, value);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::PutStatic, operands: vec![self.site_operand(*cp_index), value], dst: None, bci: 0 });
            }
            HirInstruction::LoadIndexed { array, index, elem } => {
                let array = self.emit_expr(out, array);
                let index = self.emit_expr(out, index);
                let dst = self.alloc_temp(LirKind::from_value_kind(elem.value_kind()));
                out.push(LirOp::CallRuntime { stub: RuntimeStub::ArrayLoad, operands: vec![array, index, LirOperand::ConstI32(array_load_kind(*elem))], dst: Some(dst.clone()), bci: 0 });
            }
            HirInstruction::StoreIndexed { array, index, value, elem } => {
                let array = self.emit_expr(out, array);
                let index = self.emit_expr(out, index);
                let value = self.emit_expr(out, value);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::ArrayStore, operands: vec![array, index, value, LirOperand::ConstI32(array_load_kind(*elem))], dst: None, bci: 0 });
            }
            HirInstruction::ArrayLength(array) => {
                let array = self.emit_expr(out, array);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::ArrayLength, operands: vec![array], dst: Some(dst.clone()), bci: 0 });
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
                let info = self.resolved_call_info(info);
                let ops = self.emit_operands(out, operands);
                if info.return_kind == ValueKind::Void {
                    out.push(LirOp::Call { dst: None, operands: ops, info });
                    LirOperand::ConstI32(0)
                } else {
                    let dst = self.alloc_temp(LirKind::from_value_kind(info.return_kind));
                    out.push(LirOp::Call { dst: Some(dst.clone()), operands: ops, info });
                    dst
                }
            }
            HirInstruction::CheckCast { value, cp_index, .. } => {
                let v = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::CheckCast, operands: vec![v, self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::InstanceOf { value, cp_index, .. } => {
                let v = self.emit_expr(out, value);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::InstanceOf, operands: vec![v, self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::NewArray { elem, length } => {
                let len = self.emit_expr(out, length);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewArray, operands: vec![len, LirOperand::ConstI32(elem_code(*elem))], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::AnewArray { cp_index, length } => {
                let site = self.site_operand(*cp_index);
                let len = self.emit_expr(out, length);
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::AnewArray, operands: vec![site, len], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::NewObject { cp_index, .. } => {
                let dst = self.alloc_temp(LirKind::Ref);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::NewInstance, operands: vec![self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::LoadField { obj, cp_index, kind, .. } => {
                let obj = self.emit_expr(out, obj);
                let dst = self.alloc_temp(LirKind::from_value_kind(*kind));
                out.push(LirOp::CallRuntime { stub: RuntimeStub::GetField, operands: vec![obj, self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::GetStatic { cp_index, kind, .. } => {
                let dst = self.alloc_temp(LirKind::from_value_kind(*kind));
                out.push(LirOp::CallRuntime { stub: RuntimeStub::GetStatic, operands: vec![self.site_operand(*cp_index)], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::LoadIndexed { array, index, elem } => {
                let array = self.emit_expr(out, array);
                let index = self.emit_expr(out, index);
                let dst = self.alloc_temp(LirKind::from_value_kind(elem.value_kind()));
                out.push(LirOp::CallRuntime { stub: RuntimeStub::ArrayLoad, operands: vec![array, index, LirOperand::ConstI32(array_load_kind(*elem))], dst: Some(dst.clone()), bci: 0 });
                dst
            }
            HirInstruction::ArrayLength(array) => {
                let array = self.emit_expr(out, array);
                let dst = self.alloc_temp(LirKind::Int32);
                out.push(LirOp::CallRuntime { stub: RuntimeStub::ArrayLength, operands: vec![array], dst: Some(dst.clone()), bci: 0 });
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
        HirInstruction::NewArray { length, .. } | HirInstruction::AnewArray { length, .. } => scan_instr(length, max),
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

/// Element discriminator passed to `ArrayLoad`/`ArrayStore` stubs.
/// Matches the interpreter's `opcode - 0x2e` / `opcode - 0x4f` kind
/// codes consumed by `TypeArrayOop::load`/`store`; `Ref` arrays are
/// recognized by the object-array tag and ignore this value.
fn array_load_kind(elem: ArrayElemKind) -> i32 {
    match elem {
        ArrayElemKind::Int => 0,
        ArrayElemKind::Long => 1,
        ArrayElemKind::Float => 2,
        ArrayElemKind::Double => 3,
        ArrayElemKind::Boolean | ArrayElemKind::Byte => 5,
        ArrayElemKind::Char => 6,
        ArrayElemKind::Short => 7,
        ArrayElemKind::Ref => 12,
    }
}

/// `newarray` element code: the JVM typecode consumed by
/// `TypeArrayOop::new` (4=boolean, 5=char, 6=float, 7=double,
/// 8=byte, 9=short, 10=int, 11=long).  `Ref` never reaches
/// `newarray` (that is `anewarray`); 12 is kept as a sentinel.
fn elem_code(elem: ArrayElemKind) -> i32 {
    match elem {
        ArrayElemKind::Boolean => 4,
        ArrayElemKind::Char => 5,
        ArrayElemKind::Float => 6,
        ArrayElemKind::Double => 7,
        ArrayElemKind::Byte => 8,
        ArrayElemKind::Short => 9,
        ArrayElemKind::Int => 10,
        ArrayElemKind::Long => 11,
        ArrayElemKind::Ref => 12,
    }
}
