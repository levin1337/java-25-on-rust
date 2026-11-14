// C1 HIR Optimizer ported from HotSpot c1_Optimizer.hpp / .cpp and c1_Canonicalizer.cpp

use super::c1_IR::ControlFlowGraph;
use super::c1_Instruction::HirInstruction;

pub struct Optimizer;

impl Optimizer {
    pub fn optimize(instr: HirInstruction) -> HirInstruction {
        match instr {
            HirInstruction::Add(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a.wrapping_add(*b))
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        HirInstruction::ConstantLong(a.wrapping_add(*b))
                    }
                    (other, HirInstruction::ConstantInt(0))
                    | (HirInstruction::ConstantInt(0), other) => other.clone(),
                    (other, HirInstruction::ConstantLong(0))
                    | (HirInstruction::ConstantLong(0), other) => other.clone(),
                    _ => HirInstruction::Add(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Sub(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a.wrapping_sub(*b))
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        HirInstruction::ConstantLong(a.wrapping_sub(*b))
                    }
                    (other, HirInstruction::ConstantInt(0)) => other.clone(),
                    (other, HirInstruction::ConstantLong(0)) => other.clone(),
                    (a, b) if a == b => HirInstruction::ConstantInt(0),
                    _ => HirInstruction::Sub(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Mul(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a.wrapping_mul(*b))
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        HirInstruction::ConstantLong(a.wrapping_mul(*b))
                    }
                    (_, HirInstruction::ConstantInt(0)) | (HirInstruction::ConstantInt(0), _) => {
                        HirInstruction::ConstantInt(0)
                    }
                    (_, HirInstruction::ConstantLong(0)) | (HirInstruction::ConstantLong(0), _) => {
                        HirInstruction::ConstantLong(0)
                    }
                    (other, HirInstruction::ConstantInt(1))
                    | (HirInstruction::ConstantInt(1), other) => other.clone(),
                    (other, HirInstruction::ConstantLong(1))
                    | (HirInstruction::ConstantLong(1), other) => other.clone(),
                    (other, HirInstruction::ConstantInt(-1))
                    | (HirInstruction::ConstantInt(-1), other) => {
                        HirInstruction::Neg(Box::new(other.clone()))
                    }
                    _ => HirInstruction::Mul(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Div(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) if *b != 0 => {
                        if *a == i32::MIN && *b == -1 {
                            HirInstruction::ConstantInt(i32::MIN)
                        } else {
                            HirInstruction::ConstantInt(a.wrapping_div(*b))
                        }
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b))
                        if *b != 0 =>
                    {
                        if *a == i64::MIN && *b == -1 {
                            HirInstruction::ConstantLong(i64::MIN)
                        } else {
                            HirInstruction::ConstantLong(a.wrapping_div(*b))
                        }
                    }
                    (other, HirInstruction::ConstantInt(1)) => other.clone(),
                    (other, HirInstruction::ConstantLong(1)) => other.clone(),
                    _ => HirInstruction::Div(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Rem(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) if *b != 0 => {
                        if *a == i32::MIN && *b == -1 {
                            HirInstruction::ConstantInt(0)
                        } else {
                            HirInstruction::ConstantInt(a.wrapping_rem(*b))
                        }
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b))
                        if *b != 0 =>
                    {
                        if *a == i64::MIN && *b == -1 {
                            HirInstruction::ConstantLong(0)
                        } else {
                            HirInstruction::ConstantLong(a.wrapping_rem(*b))
                        }
                    }
                    (_, HirInstruction::ConstantInt(1)) => HirInstruction::ConstantInt(0),
                    (_, HirInstruction::ConstantLong(1)) => HirInstruction::ConstantLong(0),
                    _ => HirInstruction::Rem(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Neg(val) => {
                let v = Self::optimize(*val);
                match v {
                    HirInstruction::ConstantInt(x) => HirInstruction::ConstantInt(x.wrapping_neg()),
                    HirInstruction::ConstantLong(x) => {
                        HirInstruction::ConstantLong(x.wrapping_neg())
                    }
                    HirInstruction::Neg(inner) => *inner,
                    other => HirInstruction::Neg(Box::new(other)),
                }
            }

            HirInstruction::And(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a & b)
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        HirInstruction::ConstantLong(a & b)
                    }
                    (_, HirInstruction::ConstantInt(0)) | (HirInstruction::ConstantInt(0), _) => {
                        HirInstruction::ConstantInt(0)
                    }
                    (_, HirInstruction::ConstantLong(0)) | (HirInstruction::ConstantLong(0), _) => {
                        HirInstruction::ConstantLong(0)
                    }
                    (other, HirInstruction::ConstantInt(-1))
                    | (HirInstruction::ConstantInt(-1), other) => other.clone(),
                    (other, HirInstruction::ConstantLong(-1))
                    | (HirInstruction::ConstantLong(-1), other) => other.clone(),
                    (a, b) if a == b => a.clone(),
                    _ => HirInstruction::And(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Or(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a | b)
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        HirInstruction::ConstantLong(a | b)
                    }
                    (other, HirInstruction::ConstantInt(0))
                    | (HirInstruction::ConstantInt(0), other) => other.clone(),
                    (other, HirInstruction::ConstantLong(0))
                    | (HirInstruction::ConstantLong(0), other) => other.clone(),
                    (_, HirInstruction::ConstantInt(-1)) | (HirInstruction::ConstantInt(-1), _) => {
                        HirInstruction::ConstantInt(-1)
                    }
                    (_, HirInstruction::ConstantLong(-1))
                    | (HirInstruction::ConstantLong(-1), _) => HirInstruction::ConstantLong(-1),
                    (a, b) if a == b => a.clone(),
                    _ => HirInstruction::Or(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Xor(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a ^ b)
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        HirInstruction::ConstantLong(a ^ b)
                    }
                    (other, HirInstruction::ConstantInt(0))
                    | (HirInstruction::ConstantInt(0), other) => other.clone(),
                    (other, HirInstruction::ConstantLong(0))
                    | (HirInstruction::ConstantLong(0), other) => other.clone(),
                    (a, b) if a == b => HirInstruction::ConstantInt(0),
                    _ => HirInstruction::Xor(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Shl(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a.wrapping_shl((*b & 31) as u32))
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantLong(a.wrapping_shl((*b & 63) as u32))
                    }
                    (other, HirInstruction::ConstantInt(0)) => other.clone(),
                    (HirInstruction::ConstantInt(0), _) => HirInstruction::ConstantInt(0),
                    (HirInstruction::ConstantLong(0), _) => HirInstruction::ConstantLong(0),
                    _ => HirInstruction::Shl(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Shr(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantInt(a.wrapping_shr((*b & 31) as u32))
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantInt(b)) => {
                        HirInstruction::ConstantLong(a.wrapping_shr((*b & 63) as u32))
                    }
                    (other, HirInstruction::ConstantInt(0)) => other.clone(),
                    (HirInstruction::ConstantInt(0), _) => HirInstruction::ConstantInt(0),
                    (HirInstruction::ConstantLong(0), _) => HirInstruction::ConstantLong(0),
                    _ => HirInstruction::Shr(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::Ushr(left, right) => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        let shift = (*b & 31) as u32;
                        HirInstruction::ConstantInt((*a as u32 >> shift) as i32)
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantInt(b)) => {
                        let shift = (*b & 63) as u32;
                        HirInstruction::ConstantLong((*a as u64 >> shift) as i64)
                    }
                    (other, HirInstruction::ConstantInt(0)) => other.clone(),
                    (HirInstruction::ConstantInt(0), _) => HirInstruction::ConstantInt(0),
                    (HirInstruction::ConstantLong(0), _) => HirInstruction::ConstantLong(0),
                    _ => HirInstruction::Ushr(Box::new(l), Box::new(r)),
                }
            }

            HirInstruction::If {
                left,
                cond,
                right,
                true_block,
                false_block,
            } => {
                let l = Self::optimize(*left);
                let r = Self::optimize(*right);
                match (&l, &r) {
                    (HirInstruction::ConstantInt(a), HirInstruction::ConstantInt(b)) => {
                        if cond.evaluate_i32(*a, *b) {
                            HirInstruction::Goto(true_block)
                        } else {
                            HirInstruction::Goto(false_block)
                        }
                    }
                    (HirInstruction::ConstantLong(a), HirInstruction::ConstantLong(b)) => {
                        if cond.evaluate_i64(*a, *b) {
                            HirInstruction::Goto(true_block)
                        } else {
                            HirInstruction::Goto(false_block)
                        }
                    }
                    _ => HirInstruction::If {
                        left: Box::new(l),
                        cond,
                        right: Box::new(r),
                        true_block,
                        false_block,
                    },
                }
            }

            HirInstruction::StoreLocal(slot, val) => {
                HirInstruction::StoreLocal(slot, Box::new(Self::optimize(*val)))
            }
            HirInstruction::StoreField {
                obj,
                offset,
                value,
                field_name,
            } => HirInstruction::StoreField {
                obj: Box::new(Self::optimize(*obj)),
                offset,
                value: Box::new(Self::optimize(*value)),
                field_name,
            },
            HirInstruction::LoadField {
                obj,
                offset,
                field_name,
            } => HirInstruction::LoadField {
                obj: Box::new(Self::optimize(*obj)),
                offset,
                field_name,
            },
            HirInstruction::ArrayLength(array) => {
                HirInstruction::ArrayLength(Box::new(Self::optimize(*array)))
            }
            HirInstruction::Return(val) => {
                HirInstruction::Return(val.map(|v| Box::new(Self::optimize(*v))))
            }
            HirInstruction::Throw(val) => HirInstruction::Throw(Box::new(Self::optimize(*val))),
            other => other,
        }
    }

    pub fn optimize_cfg(cfg: &mut ControlFlowGraph) {
        for block in &mut cfg.blocks {
            let mut opt_insts = Vec::with_capacity(block.instructions.len());
            for inst in block.instructions.drain(..) {
                opt_insts.push(Self::optimize(inst));
            }
            block.instructions = opt_insts;
            if let Some(last) = block.instructions.last() {
                if last.is_block_end() {
                    block.successors = last.successors();
                    block.begin.successors = block.successors.clone();
                }
            }
        }
        cfg.eliminate_dead_blocks();
    }
}
