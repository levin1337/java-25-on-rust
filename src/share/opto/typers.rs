/*
 * Port of OpenJDK 25 HotSpot C2 node type rules (Ideal::Value / Node::Value
 * implementations scattered over the opto sources) plus the PhaseTyper
 * driver that materializes the type of every node in the ideal graph.
 *
 * In HotSpot each node subclass computes its type on demand through
 * `virtual const Type* Value(PhaseGVN*)`.  Here the same rules are
 * centralized in `node_value_type`, dispatched on `Opcode`, and
 * `PhaseTyper` evaluates them bottom-up over `PhaseGVN::nodes` until a
 * fixpoint is reached (matching the optimistic dataflow iteration C2
 * performs inside PhaseCCP/PhaseIterGVN).
 */

use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode, NONE};
use super::r#type::{self, Type};
use crate::share::opto::builder::CON_P_NULL;

pub fn node_value_type(gvn: &PhaseGVN, id: NodeId, types: &[Type]) -> Type {
    let node = match gvn.nodes.get(id) {
        Some(n) => n,
        None => return Type::Top,
    };
    let t = |i: usize| -> Type {
        node.inputs
            .get(i)
            .and_then(|&inp| {
                if inp == NONE || inp >= types.len() {
                    None
                } else {
                    Some(types[inp].clone())
                }
            })
            .unwrap_or(Type::Top)
    };
    match &node.op {
        Opcode::Root | Opcode::Start | Opcode::Halt => Type::Control,
        Opcode::Return | Opcode::Athrow => Type::Top,
        Opcode::If | Opcode::IfTrue | Opcode::IfFalse | Opcode::IfException => Type::Control,
        Opcode::Region => Type::Control,
        Opcode::MergeMem => Type::Memory,
        Opcode::Phi => {
            let n = node.inputs.len();
            if n <= 1 {
                return Type::Top;
            }
            let mut acc = Type::Top;
            for i in 0..n - 1 {
                let ti = t(i);
                if ti.kind() == r#type::TypeKind::Control {
                    continue;
                }
                acc = acc.meet(&ti, &r#type::EmptyKlassDb);
            }
            acc
        }
        Opcode::Proj(_) => {
            if node.inputs.is_empty() {
                Type::Top
            } else {
                t(0)
            }
        }
        Opcode::ConI(v) => Type::int_const(*v),
        Opcode::ConL(v) => Type::long_const(*v),
        Opcode::ConF(v) => Type::float_const(f32::from_bits(*v)),
        Opcode::ConD(v) => Type::double_const(f64::from_bits(*v)),
        Opcode::ConP(p) => {
            if *p == CON_P_NULL {
                Type::null_ptr()
            } else {
                Type::inst_ptr(None)
            }
        }
        Opcode::Bool(b) => Type::int_const(*b as i32),
        Opcode::Parm(_) => Type::Bottom,
        Opcode::I2L => Type::long(),
        Opcode::I2F => Type::float(),
        Opcode::I2D => Type::double(),
        Opcode::L2I => Type::int(),
        Opcode::L2F => Type::float(),
        Opcode::L2D => Type::double(),
        Opcode::F2I => Type::int(),
        Opcode::F2L => Type::long(),
        Opcode::F2D => Type::double(),
        Opcode::D2I => Type::int(),
        Opcode::D2L => Type::long(),
        Opcode::D2F => Type::float(),
        Opcode::I2B => Type::byte_(),
        Opcode::I2C => Type::char_(),
        Opcode::I2S => Type::short_(),
        Opcode::AddI | Opcode::SubI | Opcode::MulI | Opcode::AndI | Opcode::OrI
        | Opcode::XorI | Opcode::LShiftI | Opcode::RShiftI | Opcode::URShiftI => {
            int_arith_type(&t(0), &t(1), node.op.clone())
        }
        Opcode::NegI => int_arith_type(&t(0), &t(0), node.op.clone()),
        Opcode::DivI | Opcode::RemI => int_arith_type(&t(0), &t(1), node.op.clone()),
        Opcode::AddL | Opcode::SubL | Opcode::MulL | Opcode::AndL | Opcode::OrL
        | Opcode::XorL | Opcode::LShiftL | Opcode::RShiftL | Opcode::URShiftL => {
            long_arith_type(&t(0), &t(1), node.op.clone())
        }
        Opcode::NegL => long_arith_type(&t(0), &t(0), node.op.clone()),
        Opcode::DivL | Opcode::RemL => long_arith_type(&t(0), &t(1), node.op.clone()),
        Opcode::AddF | Opcode::SubF | Opcode::MulF | Opcode::DivF | Opcode::RemF | Opcode::NegF => {
            Type::float()
        }
        Opcode::AddD | Opcode::SubD | Opcode::MulD | Opcode::DivD | Opcode::RemD | Opcode::NegD => {
            Type::double()
        }
        Opcode::CmpI | Opcode::CmpL | Opcode::CmpF(_) | Opcode::CmpD(_) => Type::Int(r#type::I32Range::new(-1, 1, r#type::WIDEN_MIN)),
        Opcode::BoolCmp(_) => Type::bool_(),
        Opcode::LoadI => Type::int(),
        Opcode::LoadL => Type::long(),
        Opcode::LoadF => Type::float(),
        Opcode::LoadD => Type::double(),
        Opcode::LoadP => Type::inst_ptr(None),
        Opcode::LoadB => Type::byte_(),
        Opcode::LoadC => Type::char_(),
        Opcode::LoadS => Type::short_(),
        Opcode::StoreI
        | Opcode::StoreL
        | Opcode::StoreF
        | Opcode::StoreD
        | Opcode::StoreP
        | Opcode::StoreB
        | Opcode::StoreC
        | Opcode::StoreS => Type::Memory,
        Opcode::New(_) => Type::inst_ptr(None),
        Opcode::NewArray(_) | Opcode::NewArrayRef(_) => Type::inst_ptr(None),
        Opcode::ArrayLength => Type::pos(),
        Opcode::CheckCast(_) => t(2),
        Opcode::InstanceOf(_) => Type::bool_(),
        Opcode::MonitorEnter | Opcode::MonitorExit => Type::Control,
        Opcode::Call(_) => Type::Bottom,
    }
}

fn int_arith_type(a: &Type, b: &Type, op: Opcode) -> Type {
    let (ra, rb) = match (a.is_int(), b.is_int()) {
        (Some(x), Some(y)) => (x, y),
        _ => return Type::int(),
    };
    if ra.is_con() && rb.is_con() {
        let (x, y) = (ra.get_con(), rb.get_con());
        let r = match op {
            Opcode::AddI => x.wrapping_add(y),
            Opcode::SubI => x.wrapping_sub(y),
            Opcode::MulI => x.wrapping_mul(y),
            Opcode::DivI => {
                if y == 0 {
                    return Type::int();
                }
                x.wrapping_div(y)
            }
            Opcode::RemI => {
                if y == 0 {
                    return Type::int();
                }
                x.wrapping_rem(y)
            }
            Opcode::NegI => x.wrapping_neg(),
            Opcode::AndI => x & y,
            Opcode::OrI => x | y,
            Opcode::XorI => x ^ y,
            _ => return Type::int(),
        };
        return Type::int_const(r);
    }
    if ra.is_con() && matches!(op, Opcode::NegI) {
        return Type::int_const(ra.get_con().wrapping_neg());
    }
    let (lo, hi) = match op {
        Opcode::AddI => (
            ra.lo as i64 + rb.lo as i64,
            ra.hi as i64 + rb.hi as i64,
        ),
        Opcode::SubI => (
            ra.lo as i64 - rb.hi as i64,
            ra.hi as i64 - rb.lo as i64,
        ),
        Opcode::MulI => {
            let vals = [
                ra.lo as i64 * rb.lo as i64,
                ra.lo as i64 * rb.hi as i64,
                ra.hi as i64 * rb.lo as i64,
                ra.hi as i64 * rb.hi as i64,
            ];
            (*vals.iter().min().unwrap(), *vals.iter().max().unwrap())
        }
        Opcode::AndI => {
            if ra.lo >= 0 && rb.lo >= 0 {
                (0, ra.hi.min(rb.hi) as i64)
            } else {
                (i32::MIN as i64, i32::MAX as i64)
            }
        }
        Opcode::OrI | Opcode::XorI => {
            if ra.lo >= 0 && rb.lo >= 0 {
                (0, (ra.hi | rb.hi) as i64)
            } else {
                (i32::MIN as i64, i32::MAX as i64)
            }
        }
        Opcode::LShiftI | Opcode::RShiftI | Opcode::URShiftI => {
            (i32::MIN as i64, i32::MAX as i64)
        }
        Opcode::DivI | Opcode::RemI | Opcode::NegI => (i32::MIN as i64, i32::MAX as i64),
        _ => return Type::int(),
    };
    let lo = lo.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    let hi = hi.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    if lo > hi {
        return Type::Top;
    }
    Type::Int(r#type::I32Range::new(lo, hi, r#type::WIDEN_MIN))
}

fn long_arith_type(a: &Type, b: &Type, op: Opcode) -> Type {
    let (ra, rb) = match (a.is_long(), b.is_long()) {
        (Some(x), Some(y)) => (x, y),
        _ => return Type::long(),
    };
    if ra.is_con() && rb.is_con() {
        let (x, y) = (ra.get_con(), rb.get_con());
        let r = match op {
            Opcode::AddL => x.wrapping_add(y),
            Opcode::SubL => x.wrapping_sub(y),
            Opcode::MulL => x.wrapping_mul(y),
            Opcode::DivL => {
                if y == 0 {
                    return Type::long();
                }
                x.wrapping_div(y)
            }
            Opcode::RemL => {
                if y == 0 {
                    return Type::long();
                }
                x.wrapping_rem(y)
            }
            Opcode::AndL => x & y,
            Opcode::OrL => x | y,
            Opcode::XorL => x ^ y,
            _ => return Type::long(),
        };
        return Type::long_const(r);
    }
    if ra.is_con() && matches!(op, Opcode::NegL) {
        return Type::long_const(ra.get_con().wrapping_neg());
    }
    Type::Long(r#type::I64Range::new(
        ra.lo.min(rb.lo),
        ra.hi.max(rb.hi),
        r#type::WIDEN_MIN,
    ))
}

pub struct PhaseTyper<'a> {
    gvn: &'a PhaseGVN,
    pub types: Vec<Type>,
    max_iterations: usize,
}

impl<'a> PhaseTyper<'a> {
    pub fn new(gvn: &'a PhaseGVN) -> Self {
        Self {
            gvn,
            types: vec![Type::Top; gvn.node_count()],
            max_iterations: 16,
        }
    }

    pub fn with_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn type_of(&self, id: NodeId) -> Type {
        self.types.get(id).cloned().unwrap_or(Type::Top)
    }

    pub fn optimize(&mut self) -> usize {
        let mut rounds = 0;
        loop {
            let mut changed = false;
            for id in 0..self.gvn.node_count() {
                let new_t = node_value_type(self.gvn, id, &self.types);
                let old = self.types[id].clone();
                let merged = if id_is_phi_like(self.gvn, id) {
                    old.meet(&new_t, &r#type::EmptyKlassDb)
                } else {
                    new_t
                };
                let merged = if merged != old {
                    merged.widen(&old, &Type::int())
                } else {
                    merged
                };
                if merged != old {
                    self.types[id] = merged;
                    changed = true;
                }
            }
            rounds += 1;
            if !changed || rounds >= self.max_iterations {
                break;
            }
        }
        rounds
    }
}

fn id_is_phi_like(gvn: &PhaseGVN, id: NodeId) -> bool {
    matches!(gvn.nodes[id].op, Opcode::Phi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::opto::gvn::PhaseGVN;
    use crate::share::opto::node::Opcode;

    #[test]
    fn const_types() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let c = gvn.transform(Opcode::ConI(42), vec![]);
        let add = gvn.transform(Opcode::AddI, vec![c, c]);
        let mut typer = PhaseTyper::new(&gvn);
        typer.optimize();
        assert_eq!(typer.type_of(c), Type::int_const(42));
        assert_eq!(typer.type_of(start), Type::Control);
        assert!(typer.type_of(add).is_int().is_some());
    }

    #[test]
    fn phi_meet_type() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let region = gvn.alloc(Opcode::Region, vec![start, start]);
        let a = gvn.transform(Opcode::ConI(1), vec![]);
        let b = gvn.transform(Opcode::ConI(100), vec![]);
        let phi = gvn.alloc(Opcode::Phi, vec![a, b, region]);
        let mut typer = PhaseTyper::new(&gvn);
        typer.optimize();
        match typer.type_of(phi) {
            Type::Int(r) => {
                assert_eq!(r.lo, 1);
                assert_eq!(r.hi, 100);
            }
            other => panic!("expected int range, got {:?}", other),
        }
    }

    #[test]
    fn conversion_types() {
        let mut gvn = PhaseGVN::new();
        let c = gvn.transform(Opcode::ConI(7), vec![]);
        let l = gvn.transform(Opcode::I2L, vec![c]);
        let mut typer = PhaseTyper::new(&gvn);
        typer.optimize();
        assert!(typer.type_of(l).is_long().is_some());
    }
}
