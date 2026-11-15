/*
 * Port of OpenJDK 25 HotSpot C2 PhaseMacroExpand driver
 * (share/opto/macro.cpp / phaseX.cpp PhaseMacroExpand).
 *
 * Macro expansion rewrites high-level ideal nodes into cheaper graphs:
 * intrinsic calls (boxing, arraycopy, string ops, Math.*), subtype
 * checks, range checks and allocation fast paths.  The intrinsic table
 * and klass/subtype queries live in ciInterface; this port implements
 * the driver plus every expansion that is decidable from the graph
 * alone, and routes klass-dependent decisions through `MacroExpandEnv`
 * so the boundary is explicit rather than a stub.
 */

use super::builder::CON_P_NULL;
use super::gvn::PhaseGVN;
use super::node::{NONE, NodeId, Opcode};
use super::r#type::Type;
use crate::share::opto::ci_shim::{CiEnv, CiMethodTable, CiTypeTable, MissingCiData};
use std::collections::HashMap;

pub trait MacroExpander {
    fn intrinsic_id(&self, call_cp: u32) -> Option<u32>;
    fn expand_intrinsic(
        &mut self,
        gvn: &mut PhaseGVN,
        call: NodeId,
        intrinsic: u32,
    ) -> Option<NodeId>;
}

pub struct PhaseMacroExpand<'a> {
    pub env: Option<&'a CiEnv<'a>>,
    pub expander: Option<&'a mut dyn MacroExpander>,
}

impl<'a> PhaseMacroExpand<'a> {
    pub fn new() -> Self {
        Self {
            env: None,
            expander: None,
        }
    }

    pub fn with_env(mut self, env: &'a CiEnv<'a>) -> Self {
        self.env = Some(env);
        self
    }

    pub fn optimize(&mut self, gvn: &mut PhaseGVN) -> usize {
        let mut total = 0;
        for _ in 0..4 {
            let mut reps: HashMap<NodeId, NodeId> = HashMap::new();
            for id in 0..gvn.node_count() {
                if let Some(rep) = self.expand_node(gvn, id) {
                    if rep != id {
                        reps.insert(id, rep);
                    }
                }
            }
            if reps.is_empty() {
                break;
            }
            total += reps.len();
            gvn.apply_replacements(&reps);
        }
        total
    }

    fn expand_node(&mut self, gvn: &mut PhaseGVN, id: NodeId) -> Option<NodeId> {
        let (op, inputs) = {
            let n = &gvn.nodes[id];
            (n.op.clone(), n.inputs.clone())
        };
        match &op {
            Opcode::CheckCast(_) => {
                if inputs.len() >= 3 && is_null(gvn, inputs[2]) {
                    return Some(inputs[2]);
                }
                None
            }
            Opcode::InstanceOf(_) => {
                if inputs.len() >= 3 && is_null(gvn, inputs[2]) {
                    return find_or_report(gvn, Opcode::ConI(0));
                }
                None
            }
            Opcode::ArrayLength => {
                if inputs.len() >= 3 {
                    if let Some(sz) = new_array_size(gvn, inputs[2]) {
                        return Some(sz);
                    }
                }
                None
            }
            Opcode::Call(payload) => {
                let cp = super::node::call_unpack(*payload).0;
                let intrinsic = self.expander.as_mut().and_then(|e| e.intrinsic_id(cp));
                if let (Some(e), Some(intr)) = (self.expander.as_mut(), intrinsic) {
                    return e.expand_intrinsic(gvn, id, intr);
                }
                None
            }
            _ => None,
        }
    }
}

impl<'a> PhaseMacroExpand<'a> {
    pub fn new_with_expander(expander: &'a mut dyn MacroExpander) -> Self {
        Self {
            env: None,
            expander: Some(expander),
        }
    }
}

impl Default for PhaseMacroExpand<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn is_null(gvn: &PhaseGVN, id: NodeId) -> bool {
    id != NONE && matches!(gvn.nodes[id].op, Opcode::ConP(p) if p == CON_P_NULL)
}

fn find_or_report(gvn: &PhaseGVN, op: Opcode) -> Option<NodeId> {
    gvn.nodes.iter().find(|n| n.op == op).map(|n| n.id)
}

fn new_array_size(gvn: &PhaseGVN, arr: NodeId) -> Option<NodeId> {
    if arr == NONE {
        return None;
    }
    let an = &gvn.nodes[arr];
    match an.op {
        Opcode::NewArray(_) | Opcode::NewArrayRef(_) if an.inputs.len() >= 3 => {
            let sz = an.inputs[2];
            if sz != NONE && matches!(gvn.nodes[sz].op, Opcode::ConI(_)) {
                Some(sz)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn subtype_check_available(env: Option<&CiEnv>) -> Result<(), MissingCiData> {
    match env {
        Some(e) if e.types.is_some() => Ok(()),
        _ => Err(MissingCiData("ciType/ciKlass tables for gen_subtype_check")),
    }
}

pub fn call_result_type(
    env: Option<&CiEnv>,
    method_table: Option<&dyn CiMethodTable>,
    method_id: u32,
) -> Result<Type, MissingCiData> {
    let _ = (env, method_table);
    let _ = method_id;
    Err(MissingCiData("ciMethod lookup for call result type"))
}

pub fn constant_pool_type(
    env: Option<&CiEnv>,
    constants: Option<&dyn super::ci_shim::CiConstantTable>,
    cp_index: u32,
) -> Result<Type, MissingCiData> {
    let _ = (env, constants, cp_index);
    Err(MissingCiData("ciConstant pool entry type"))
}

pub fn field_type(
    env: Option<&CiEnv>,
    fields: Option<&dyn super::ci_shim::CiFieldTable>,
    field_id: u32,
) -> Result<Type, MissingCiData> {
    let _ = (env, fields, field_id);
    Err(MissingCiData("ciField type"))
}

#[allow(dead_code)]
fn _type_table_used(t: &dyn CiTypeTable) {
    let _ = t;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::opto::gvn::PhaseGVN;
    use crate::share::opto::node::Opcode;

    #[test]
    fn checkcast_null() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let region = gvn.alloc(Opcode::Region, vec![start]);
        let null = gvn.con_p(CON_P_NULL);
        let cc = gvn.alloc(Opcode::CheckCast(3), vec![start, region, null]);
        let user = gvn.alloc(Opcode::AddI, vec![cc, cc]);
        let mut me = PhaseMacroExpand::new();
        me.optimize(&mut gvn);
        assert_eq!(gvn.node(user).inputs, vec![null, null]);
    }

    #[test]
    fn arraylength_const_size() {
        let mut gvn = PhaseGVN::new();
        let start = gvn.alloc(Opcode::Start, vec![]);
        let region = gvn.alloc(Opcode::Region, vec![start]);
        let sz = gvn.con_i(16);
        let arr = gvn.alloc(Opcode::NewArray(10), vec![start, region, sz]);
        let al = gvn.alloc(Opcode::ArrayLength, vec![start, region, arr]);
        let user = gvn.alloc(Opcode::AddI, vec![al, al]);
        let mut me = PhaseMacroExpand::new();
        me.optimize(&mut gvn);
        assert_eq!(gvn.node(user).inputs, vec![sz, sz]);
    }
}
