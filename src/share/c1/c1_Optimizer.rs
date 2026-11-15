use super::c1_IR::ControlFlowGraph;
use super::c1_Instruction::{
    ArrayElemKind, CallInfo, CompareKind, Condition, ConvertKind, HirInstruction, ValueKind,
};
use std::collections::HashMap;

pub struct Optimizer;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum FrameKey {
    Local(usize),
    Stack(usize),
}

impl Optimizer {
    pub fn optimize(instr: HirInstruction) -> HirInstruction {
        match instr {
            HirInstruction::Add(l, r) => Self::const_arith(*l, *r, Arith::Add),
            HirInstruction::Sub(l, r) => Self::const_arith(*l, *r, Arith::Sub),
            HirInstruction::Mul(l, r) => Self::const_arith(*l, *r, Arith::Mul),
            HirInstruction::Div(l, r) => Self::const_arith(*l, *r, Arith::Div),
            HirInstruction::Rem(l, r) => Self::const_arith(*l, *r, Arith::Rem),
            HirInstruction::And(l, r) => Self::const_arith(*l, *r, Arith::And),
            HirInstruction::Or(l, r) => Self::const_arith(*l, *r, Arith::Or),
            HirInstruction::Xor(l, r) => Self::const_arith(*l, *r, Arith::Xor),
            HirInstruction::Shl(l, r) => Self::const_arith(*l, *r, Arith::Shl),
            HirInstruction::Shr(l, r) => Self::const_arith(*l, *r, Arith::Shr),
            HirInstruction::Ushr(l, r) => Self::const_arith(*l, *r, Arith::Ushr),

            HirInstruction::Neg(v) => {
                let v = Self::optimize(*v);
                match v {
                    HirInstruction::ConstantInt(x) => HirInstruction::ConstantInt(x.wrapping_neg()),
                    HirInstruction::ConstantLong(x) => HirInstruction::ConstantLong(x.wrapping_neg()),
                    HirInstruction::ConstantFloat(x) => HirInstruction::ConstantFloat((-f32::from_bits(x)).to_bits()),
                    HirInstruction::ConstantDouble(x) => HirInstruction::ConstantDouble((-f64::from_bits(x)).to_bits()),
                    HirInstruction::Neg(inner) => *inner,
                    other => HirInstruction::Neg(Box::new(other)),
                }
            }

            HirInstruction::Convert { kind, value } => {
                let value = Self::optimize(*value);
                Self::fold_convert(kind, value)
            }

            HirInstruction::Compare { kind, left, right } => {
                let left = Self::optimize(*left);
                let right = Self::optimize(*right);
                Self::fold_compare(kind, left, right)
            }

            HirInstruction::If { left, cond, right, true_block, false_block } => {
                let left = Self::optimize(*left);
                let right = Self::optimize(*right);
                match (&left, &right) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        if cond.evaluate_i32(*a, *b) { HirInstruction::Goto(true_block) } else { HirInstruction::Goto(false_block) }
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        if cond.evaluate_i64(*a, *b) { HirInstruction::Goto(true_block) } else { HirInstruction::Goto(false_block) }
                    }
                    (HirInstruction::ConstantNull, HirInstruction::ConstantNull) => {
                        match cond {
                            Condition::Eq | Condition::Le | Condition::Ge => HirInstruction::Goto(true_block),
                            Condition::Ne | Condition::Lt | Condition::Gt => HirInstruction::Goto(false_block),
                        }
                    }
                    _ if left == right && matches!(cond, Condition::Eq | Condition::Ne) => {
                        match left.value_kind() {
                            ValueKind::Float32 | ValueKind::Float64 => HirInstruction::If {
                                left: Box::new(left), cond, right: Box::new(right), true_block, false_block,
                            },
                            _ => {
                                if cond == Condition::Eq { HirInstruction::Goto(true_block) } else { HirInstruction::Goto(false_block) }
                            }
                        }
                    }
                    _ => HirInstruction::If { left: Box::new(left), cond, right: Box::new(right), true_block, false_block },
                }
            }

            HirInstruction::StoreLocal { slot, kind, value } => {
                let value = Self::optimize(*value);
                if matches!(&value, HirInstruction::LoadLocal { slot: s, kind: k } if *s == slot && *k == kind) {
                    return HirInstruction::StoreLocal { slot, kind, value: Box::new(value) };
                }
                HirInstruction::StoreLocal { slot, kind, value: Box::new(value) }
            }
            HirInstruction::StoreStack { slot, kind, value } => {
                let value = Self::optimize(*value);
                HirInstruction::StoreStack { slot, kind, value: Box::new(value) }
            }
            HirInstruction::LoadIndexed { array, index, elem } => HirInstruction::LoadIndexed {
                array: Box::new(Self::optimize(*array)),
                index: Box::new(Self::optimize(*index)),
                elem,
            },
            HirInstruction::StoreIndexed { array, index, value, elem } => HirInstruction::StoreIndexed {
                array: Box::new(Self::optimize(*array)),
                index: Box::new(Self::optimize(*index)),
                value: Box::new(Self::optimize(*value)),
                elem,
            },
            HirInstruction::NewArray { elem, length } => HirInstruction::NewArray {
                elem,
                length: Box::new(Self::optimize(*length)),
            },
            HirInstruction::NewObject { cp_index, name } => HirInstruction::NewObject { cp_index, name },
            HirInstruction::LoadField { obj, cp_index, offset, kind, name } => HirInstruction::LoadField {
                obj: Box::new(Self::optimize(*obj)), cp_index, offset, kind, name,
            },
            HirInstruction::StoreField { obj, cp_index, offset, value, kind, name } => HirInstruction::StoreField {
                obj: Box::new(Self::optimize(*obj)), cp_index, offset, value: Box::new(Self::optimize(*value)), kind, name,
            },
            HirInstruction::GetStatic { cp_index, offset, kind, name } => HirInstruction::GetStatic { cp_index, offset, kind, name },
            HirInstruction::PutStatic { cp_index, offset, value, kind, name } => HirInstruction::PutStatic {
                cp_index, offset, value: Box::new(Self::optimize(*value)), kind, name,
            },
            HirInstruction::Call { info, operands } => HirInstruction::Call {
                info: Self::optimize_call_info(info),
                operands: operands.into_iter().map(|o| Box::new(Self::optimize(*o))).collect(),
            },
            HirInstruction::CheckCast { value, cp_index, name } => HirInstruction::CheckCast {
                value: Box::new(Self::optimize(*value)), cp_index, name,
            },
            HirInstruction::InstanceOf { value, cp_index, name } => HirInstruction::InstanceOf {
                value: Box::new(Self::optimize(*value)), cp_index, name,
            },
            HirInstruction::MonitorEnter(v) => HirInstruction::MonitorEnter(Box::new(Self::optimize(*v))),
            HirInstruction::MonitorExit(v) => HirInstruction::MonitorExit(Box::new(Self::optimize(*v))),
            HirInstruction::ArrayLength(a) => HirInstruction::ArrayLength(Box::new(Self::optimize(*a))),
            HirInstruction::Return(v) => HirInstruction::Return(v.map(|x| Box::new(Self::optimize(*x)))),
            HirInstruction::Throw(v) => HirInstruction::Throw(Box::new(Self::optimize(*v))),
            HirInstruction::TableSwitch { index, low, high, default_block, targets } => HirInstruction::TableSwitch {
                index: Box::new(Self::optimize(*index)), low, high, default_block, targets,
            },
            HirInstruction::LookupSwitch { key, pairs, default_block } => HirInstruction::LookupSwitch {
                key: Box::new(Self::optimize(*key)), pairs, default_block,
            },
            HirInstruction::Goto(t) => HirInstruction::Goto(t),
            other => other,
        }
    }

    fn optimize_call_info(info: CallInfo) -> CallInfo {
        info
    }

    fn const_arith(l: HirInstruction, r: HirInstruction, op: Arith) -> HirInstruction {
        if let Some(v) = Self::fold_constants(&l, &r, op) {
            return v;
        }
        let lz = Self::is_zero(&r);
        let rz = Self::is_zero(&l);
        match op {
            Arith::Add => {
                if lz { return r; }
                if rz { return l; }
            }
            Arith::Sub => {
                if lz { return r; }
            }
            Arith::Mul => {
                if Self::is_one(&r) { return l; }
                if Self::is_one(&l) { return r; }
                if Self::is_zero(&r) || Self::is_zero(&l) { return Self::zero_like(&l, &r); }
            }
            Arith::Div => {
                if Self::is_one(&r) { return l; }
            }
            Arith::Rem => {
                if Self::is_one(&r) { return Self::zero_like(&l, &r); }
            }
            Arith::And => {
                if Self::is_all_ones(&r) { return l; }
                if Self::is_all_ones(&l) { return r; }
                if Self::is_zero(&r) || Self::is_zero(&l) { return Self::zero_like(&l, &r); }
                if l == r { return l; }
            }
            Arith::Or => {
                if Self::is_zero(&r) { return l; }
                if Self::is_zero(&l) { return r; }
                if Self::is_all_ones(&r) || Self::is_all_ones(&l) { return Self::all_ones_like(&l, &r); }
                if l == r { return l; }
            }
            Arith::Xor => {
                if Self::is_zero(&r) { return l; }
                if Self::is_zero(&l) { return r; }
                if l == r && matches!(l.value_kind(), ValueKind::Int32 | ValueKind::Int64) { return Self::zero_like(&l, &r); }
            }
            Arith::Shl | Arith::Shr | Arith::Ushr => {
                if Self::is_zero(&r) { return l; }
            }
        }
        let l = Box::new(l);
        let r = Box::new(r);
        match op {
            Arith::Add => HirInstruction::Add(l, r),
            Arith::Sub => HirInstruction::Sub(l, r),
            Arith::Mul => HirInstruction::Mul(l, r),
            Arith::Div => HirInstruction::Div(l, r),
            Arith::Rem => HirInstruction::Rem(l, r),
            Arith::And => HirInstruction::And(l, r),
            Arith::Or => HirInstruction::Or(l, r),
            Arith::Xor => HirInstruction::Xor(l, r),
            Arith::Shl => HirInstruction::Shl(l, r),
            Arith::Shr => HirInstruction::Shr(l, r),
            Arith::Ushr => HirInstruction::Ushr(l, r),
        }
    }

    fn fold_constants(l: &HirInstruction, r: &HirInstruction, op: Arith) -> Option<HirInstruction> {
        match (l, r) {
            (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => Self::int_const(*a, *b, op).map(HirInstruction::ConstantInt),
            (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => Self::long_const(*a, *b, op).map(HirInstruction::ConstantLong),
            (HirInstruction::ConstantFloat(a), HirInstruction::ConstantFloat(b)) => Self::float_const(*a, *b, op).map(HirInstruction::ConstantFloat),
            (HirInstruction::ConstantDouble(a), HirInstruction::ConstantDouble(b)) => Self::double_const(*a, *b, op).map(HirInstruction::ConstantDouble),
            _ => None,
        }
    }

    fn int_const(a: i32, b: i32, op: Arith) -> Option<i32> {
        match op {
            Arith::Add => Some(a.wrapping_add(b)),
            Arith::Sub => Some(a.wrapping_sub(b)),
            Arith::Mul => Some(a.wrapping_mul(b)),
            Arith::Div => {
                if b == 0 || (a == i32::MIN && b == -1) { None } else { Some(a.wrapping_div(b)) }
            }
            Arith::Rem => {
                if b == 0 || (a == i32::MIN && b == -1) { None } else { Some(a.wrapping_rem(b)) }
            }
            Arith::And => Some(a & b),
            Arith::Or => Some(a | b),
            Arith::Xor => Some(a ^ b),
            Arith::Shl => Some(a.wrapping_shl((b & 31) as u32)),
            Arith::Shr => Some(a.wrapping_shr((b & 31) as u32)),
            Arith::Ushr => Some(((a as u32) >> (b & 31)) as i32),
        }
    }

    fn long_const(a: i64, b: i64, op: Arith) -> Option<i64> {
        match op {
            Arith::Add => Some(a.wrapping_add(b)),
            Arith::Sub => Some(a.wrapping_sub(b)),
            Arith::Mul => Some(a.wrapping_mul(b)),
            Arith::Div => {
                if b == 0 || (a == i64::MIN && b == -1) { None } else { Some(a.wrapping_div(b)) }
            }
            Arith::Rem => {
                if b == 0 || (a == i64::MIN && b == -1) { None } else { Some(a.wrapping_rem(b)) }
            }
            Arith::And => Some(a & b),
            Arith::Or => Some(a | b),
            Arith::Xor => Some(a ^ b),
            Arith::Shl => Some(a.wrapping_shl((b & 63) as u32)),
            Arith::Shr => Some(a.wrapping_shr((b & 63) as u32)),
            Arith::Ushr => Some(((a as u64) >> (b & 63)) as i64),
        }
    }

    fn float_const(a: u32, b: u32, op: Arith) -> Option<u32> {
        let (a, b) = (f32::from_bits(a), f32::from_bits(b));
        let v = match op {
            Arith::Add => a + b,
            Arith::Sub => a - b,
            Arith::Mul => a * b,
            Arith::Div => a / b,
            Arith::Rem => a % b,
            Arith::And | Arith::Or | Arith::Xor | Arith::Shl | Arith::Shr | Arith::Ushr => return None,
        };
        Some(v.to_bits())
    }

    fn double_const(a: u64, b: u64, op: Arith) -> Option<u64> {
        let (a, b) = (f64::from_bits(a), f64::from_bits(b));
        let v = match op {
            Arith::Add => a + b,
            Arith::Sub => a - b,
            Arith::Mul => a * b,
            Arith::Div => a / b,
            Arith::Rem => a % b,
            Arith::And | Arith::Or | Arith::Xor | Arith::Shl | Arith::Shr | Arith::Ushr => return None,
        };
        Some(v.to_bits())
    }

    fn is_zero(v: &HirInstruction) -> bool {
        match v {
            HirInstruction::ConstantInt(x) => *x == 0,
            HirInstruction::ConstantLong(x) => *x == 0,
            HirInstruction::ConstantFloat(x) => *x == 0,
            HirInstruction::ConstantDouble(x) => *x == 0,
            _ => false,
        }
    }

    fn is_one(v: &HirInstruction) -> bool {
        match v {
            HirInstruction::ConstantInt(x) => *x == 1,
            HirInstruction::ConstantLong(x) => *x == 1,
            HirInstruction::ConstantFloat(x) => *x == 0x3f80_0000,
            HirInstruction::ConstantDouble(x) => *x == 0x3ff0_0000_0000_0000,
            _ => false,
        }
    }

    fn is_all_ones(v: &HirInstruction) -> bool {
        matches!(v, HirInstruction::ConstantInt(-1) | HirInstruction::ConstantLong(-1))
    }

    fn zero_like(l: &HirInstruction, r: &HirInstruction) -> HirInstruction {
        let kind = if l.value_kind() == ValueKind::Float64 || r.value_kind() == ValueKind::Float64 {
            ValueKind::Float64
        } else if l.value_kind() == ValueKind::Float32 || r.value_kind() == ValueKind::Float32 {
            ValueKind::Float32
        } else if l.value_kind() == ValueKind::Int64 || r.value_kind() == ValueKind::Int64 {
            ValueKind::Int64
        } else {
            ValueKind::Int32
        };
        match kind {
            ValueKind::Int64 => HirInstruction::ConstantLong(0),
            ValueKind::Float32 => HirInstruction::ConstantFloat(0),
            ValueKind::Float64 => HirInstruction::ConstantDouble(0),
            _ => HirInstruction::ConstantInt(0),
        }
    }

    fn all_ones_like(l: &HirInstruction, r: &HirInstruction) -> HirInstruction {
        if l.value_kind() == ValueKind::Int64 || r.value_kind() == ValueKind::Int64 {
            HirInstruction::ConstantLong(-1)
        } else {
            HirInstruction::ConstantInt(-1)
        }
    }

    fn fold_convert(kind: ConvertKind, value: HirInstruction) -> HirInstruction {
        match (kind, &value) {
            (ConvertKind::I2L, HirInstruction::ConstantInt(v)) => HirInstruction::ConstantLong(*v as i64),
            (ConvertKind::L2I, HirInstruction::ConstantLong(v)) => HirInstruction::ConstantInt(*v as i32),
            (ConvertKind::I2F, HirInstruction::ConstantInt(v)) => HirInstruction::ConstantFloat((*v as f32).to_bits()),
            (ConvertKind::I2D, HirInstruction::ConstantInt(v)) => HirInstruction::ConstantDouble((*v as f64).to_bits()),
            (ConvertKind::L2F, HirInstruction::ConstantLong(v)) => HirInstruction::ConstantFloat((*v as f32).to_bits()),
            (ConvertKind::L2D, HirInstruction::ConstantLong(v)) => HirInstruction::ConstantDouble((*v as f64).to_bits()),
            (ConvertKind::F2I, HirInstruction::ConstantFloat(v)) => HirInstruction::ConstantInt(f32::from_bits(*v) as i32),
            (ConvertKind::F2L, HirInstruction::ConstantFloat(v)) => HirInstruction::ConstantLong(f32::from_bits(*v) as i64),
            (ConvertKind::F2D, HirInstruction::ConstantFloat(v)) => HirInstruction::ConstantDouble((f32::from_bits(*v) as f64).to_bits()),
            (ConvertKind::D2I, HirInstruction::ConstantDouble(v)) => HirInstruction::ConstantInt(f64::from_bits(*v) as i32),
            (ConvertKind::D2L, HirInstruction::ConstantDouble(v)) => HirInstruction::ConstantLong(f64::from_bits(*v) as i64),
            (ConvertKind::D2F, HirInstruction::ConstantDouble(v)) => HirInstruction::ConstantFloat((f64::from_bits(*v) as f32).to_bits()),
            (ConvertKind::I2B, HirInstruction::ConstantInt(v)) => HirInstruction::ConstantInt((*v as i8) as i32),
            (ConvertKind::I2C, HirInstruction::ConstantInt(v)) => HirInstruction::ConstantInt((*v as u16) as i32),
            (ConvertKind::I2S, HirInstruction::ConstantInt(v)) => HirInstruction::ConstantInt((*v as i16) as i32),
            _ => HirInstruction::Convert { kind, value: Box::new(value) },
        }
    }

    fn fold_compare(kind: CompareKind, left: HirInstruction, right: HirInstruction) -> HirInstruction {
        let folded = match (kind, &left, &right) {
            (CompareKind::Int64, HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => Some(cmp_i64(*a, *b)),
            (CompareKind::Int32, HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => Some(cmp_i32(*a, *b)),
            (CompareKind::Ref, HirInstruction::ConstantNull, HirInstruction::ConstantNull) => Some(0),
            (CompareKind::Float32NanLow, HirInstruction::ConstantFloat(a), HirInstruction::ConstantFloat(b)) => Some(cmp_f32(f32::from_bits(*a), f32::from_bits(*b), -1)),
            (CompareKind::Float32NanHigh, HirInstruction::ConstantFloat(a), HirInstruction::ConstantFloat(b)) => Some(cmp_f32(f32::from_bits(*a), f32::from_bits(*b), 1)),
            (CompareKind::Float64NanLow, HirInstruction::ConstantDouble(a), HirInstruction::ConstantDouble(b)) => Some(cmp_f64(f64::from_bits(*a), f64::from_bits(*b), -1)),
            (CompareKind::Float64NanHigh, HirInstruction::ConstantDouble(a), HirInstruction::ConstantDouble(b)) => Some(cmp_f64(f64::from_bits(*a), f64::from_bits(*b), 1)),
            _ => None,
        };
        if let Some(v) = folded { HirInstruction::ConstantInt(v) } else { HirInstruction::Compare { kind, left: Box::new(left), right: Box::new(right) } }
    }

    pub fn optimize_cfg(cfg: &mut ControlFlowGraph) {
        for block in &mut cfg.blocks {
            let mut instructions = std::mem::take(&mut block.instructions);
            if let Some(pos) = instructions.iter().position(|i| i.is_block_end()) {
                instructions.truncate(pos + 1);
            }
            let mut out = Vec::with_capacity(instructions.len());
            let mut locals: HashMap<FrameKey, HirInstruction> = HashMap::new();
            for inst in instructions {
                let inst = Self::optimize(inst);
                match &inst {
                    HirInstruction::LoadLocal { slot, kind } => {
                        let key = FrameKey::Local(*slot);
                        if let Some(v) = locals.get(&key) {
                            if matches!(v, HirInstruction::ConstantInt(_) | HirInstruction::ConstantLong(_) | HirInstruction::ConstantFloat(_) | HirInstruction::ConstantDouble(_) | HirInstruction::ConstantNull | HirInstruction::LoadLocal { .. } | HirInstruction::LoadStack { .. }) {
                                out.push(v.clone());
                                continue;
                            }
                        }
                        let _ = kind;
                    }
                    HirInstruction::LoadStack { slot, kind } => {
                        let key = FrameKey::Stack(*slot);
                        if let Some(v) = locals.get(&key) {
                            if matches!(v, HirInstruction::ConstantInt(_) | HirInstruction::ConstantLong(_) | HirInstruction::ConstantFloat(_) | HirInstruction::ConstantDouble(_) | HirInstruction::ConstantNull | HirInstruction::LoadLocal { .. } | HirInstruction::LoadStack { .. }) {
                                out.push(v.clone());
                                continue;
                            }
                        }
                        let _ = kind;
                    }
                    HirInstruction::StoreLocal { slot, value, .. } => {
                        let key = FrameKey::Local(*slot);
                        if Self::propagatable(value) { locals.insert(key, (**value).clone()); } else { locals.remove(&key); }
                    }
                    HirInstruction::StoreStack { slot, value, .. } => {
                        let key = FrameKey::Stack(*slot);
                        if Self::propagatable(value) { locals.insert(key, (**value).clone()); } else { locals.remove(&key); }
                    }
                    _ => {}
                }
                out.push(inst);
            }
            block.instructions = out;
            if let Some(last) = block.instructions.last() {
                if last.is_block_end() {
                    block.successors = last.successors();
                    block.begin.successors = block.successors.clone();
                }
            }
        }
        cfg.eliminate_dead_blocks();
    }

    fn propagatable(v: &HirInstruction) -> bool {
        matches!(
            v,
            HirInstruction::ConstantInt(_)
                | HirInstruction::ConstantLong(_)
                | HirInstruction::ConstantFloat(_)
                | HirInstruction::ConstantDouble(_)
                | HirInstruction::ConstantNull
                | HirInstruction::LoadLocal { .. }
                | HirInstruction::LoadStack { .. }
        )
    }
}

#[derive(Clone, Copy)]
enum Arith {
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

fn cmp_i32(a: i32, b: i32) -> i32 {
    if a < b { -1 } else if a > b { 1 } else { 0 }
}

fn cmp_i64(a: i64, b: i64) -> i32 {
    if a < b { -1 } else if a > b { 1 } else { 0 }
}

fn cmp_f32(a: f32, b: f32, nan: i32) -> i32 {
    if a.is_nan() || b.is_nan() { nan } else { cmp_f64(a as f64, b as f64, nan) }
}

fn cmp_f64(a: f64, b: f64, nan: i32) -> i32 {
    if a.is_nan() || b.is_nan() { nan } else if a < b { -1 } else if a > b { 1 } else { 0 }
}
