/*
 * OpenJDK 25 HotSpot adlc/ port
 *
 * Data model mirroring forms.hpp / formsopt.hpp / formssel.hpp /
 * archDesc.hpp.  Forms are shared through Rc<RefCell<Form>> exactly like
 * the C++ pointer graph; the parser mutates them through RefCell.
 */

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::opcodes::{LAST_MACHINE_LEAF, LAST_OPCODE, NODE_CLASS_NAMES};

pub type FormRef = Rc<RefCell<Form>>;
pub type MatchRuleRef = Rc<RefCell<MatchRule>>;

pub const WARN: i32 = 0;
pub const SYNERR: i32 = 1;
pub const SEMERR: i32 = 2;
pub const INTERNAL_ERR: i32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormKind {
    Source,
    Header,
    PreHeader,
    Instruct,
    Operand,
    OpClass,
    AttributeForm,
    Register,
    Pipeline,
    Encode,
    EncClass,
    Frame,
    MachNode,
    Effect,
    ExpandRule,
    Flag,
    RewriteRule,
    Constraint,
    ConstructRule,
    Predicate,
    Interface,
    RegDef,
    RegClass,
    CodeSnippetRegClass,
    ConditionalRegClass,
    AllocClass,
    Resource,
    PipeClass,
    PipeClassOperand,
    PipeClassResource,
    Peephole,
    PeepPredicate,
    PeepMatch,
    PeepProcedure,
    PeepConstraint,
    PeepReplace,
    Opcode,
    InsEncode,
    ExprDef,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InterfaceKind {
    None,
    Reg,
    Const,
    Mem,
    Cond,
}

#[derive(Clone, Debug)]
pub struct Interface {
    pub kind: InterfaceKind,
    pub name: String,
    // MemInterface fields
    pub base: Option<String>,
    pub index: Option<String>,
    pub scale: Option<String>,
    pub disp: Option<String>,
    // CondInterface fields
    pub equal: Option<String>,
    pub not_equal: Option<String>,
    pub less: Option<String>,
    pub greater_equal: Option<String>,
    pub less_equal: Option<String>,
    pub greater: Option<String>,
    pub overflow: Option<String>,
    pub no_overflow: Option<String>,
    pub equal_format: Option<String>,
    pub not_equal_format: Option<String>,
    pub less_format: Option<String>,
    pub greater_equal_format: Option<String>,
    pub less_equal_format: Option<String>,
    pub greater_format: Option<String>,
    pub overflow_format: Option<String>,
    pub no_overflow_format: Option<String>,
}

impl Interface {
    pub fn interface_type(&self) -> InterfaceKind {
        self.kind
    }
}

#[derive(Clone, Debug)]
pub struct Attribute {
    pub ident: String,
    pub val: String,
    pub atype: i32, // INS_ATTR or OP_ATTR
    pub next: Option<Rc<Attribute>>,
}

pub const INS_ATTR: i32 = 0;
pub const OP_ATTR: i32 = 1;

pub const INS_COST: &str = "ins_cost";
pub const OP_COST: &str = "op_cost";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DataType {
    None = 0,
    IdealI = 1,
    IdealP = 2,
    IdealL = 3,
    IdealF = 4,
    IdealD = 5,
    IdealB = 6,
    IdealC = 7,
    IdealS = 8,
    IdealN = 9,
    IdealNKlass = 10,
    IdealV = 11,
    IdealH = 12,
}

impl DataType {
    pub fn name(self) -> &'static str {
        match self {
            DataType::None => "none",
            DataType::IdealI => "idealI",
            DataType::IdealP => "idealP",
            DataType::IdealL => "idealL",
            DataType::IdealF => "idealF",
            DataType::IdealD => "idealD",
            DataType::IdealB => "idealB",
            DataType::IdealC => "idealC",
            DataType::IdealS => "idealS",
            DataType::IdealN => "idealN",
            DataType::IdealNKlass => "idealNKlass",
            DataType::IdealV => "idealV",
            DataType::IdealH => "idealH",
        }
    }
}

pub fn ideal_to_const_type(name: &str) -> DataType {
    match name {
        "ConI" => DataType::IdealI,
        "ConP" => DataType::IdealP,
        "ConN" => DataType::IdealN,
        "ConNKlass" => DataType::IdealNKlass,
        "ConL" => DataType::IdealL,
        "ConF" => DataType::IdealF,
        "ConH" => DataType::IdealH,
        "ConD" => DataType::IdealD,
        "Bool" => DataType::IdealI,
        _ => DataType::None,
    }
}

pub fn ideal_to_sreg_type(name: &str) -> DataType {
    match name {
        "sRegI" => DataType::IdealI,
        "sRegP" => DataType::IdealP,
        "sRegF" => DataType::IdealF,
        "sRegD" => DataType::IdealD,
        "sRegL" => DataType::IdealL,
        _ => DataType::None,
    }
}

pub fn ideal_to_reg_type(name: &str) -> DataType {
    match name {
        "RegI" => DataType::IdealI,
        "RegP" => DataType::IdealP,
        "RegF" => DataType::IdealF,
        "RegD" => DataType::IdealD,
        "RegL" => DataType::IdealL,
        _ => DataType::None,
    }
}

pub fn is_load_from_memory(op_type: &str) -> DataType {
    match op_type {
        "LoadB" | "LoadUB" => DataType::IdealB,
        "LoadUS" => DataType::IdealC,
        "LoadD" | "LoadD_unaligned" => DataType::IdealD,
        "LoadF" => DataType::IdealF,
        "LoadI" | "LoadRange" => DataType::IdealI,
        "LoadKlass" => DataType::IdealP,
        "LoadNKlass" => DataType::IdealNKlass,
        "LoadL" | "LoadL_unaligned" => DataType::IdealL,
        "LoadP" => DataType::IdealP,
        "LoadN" => DataType::IdealN,
        "LoadS" => DataType::IdealS,
        "LoadVector" | "LoadVectorGather" | "LoadVectorGatherMasked"
        | "LoadVectorMasked" => DataType::IdealV,
        _ => DataType::None,
    }
}

pub fn is_store_to_memory(op_type: &str) -> DataType {
    match op_type {
        "StoreB" => DataType::IdealB,
        "StoreC" => DataType::IdealC,
        "StoreD" => DataType::IdealD,
        "StoreF" => DataType::IdealF,
        "StoreI" => DataType::IdealI,
        "StoreL" => DataType::IdealL,
        "StoreP" => DataType::IdealP,
        "StoreN" => DataType::IdealN,
        "StoreNKlass" => DataType::IdealNKlass,
        "StoreVector" | "StoreVectorScatter" | "StoreVectorScatterMasked"
        | "StoreVectorMasked" => DataType::IdealV,
        _ => DataType::None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallType {
    Invalid = 0,
    JavaStatic = 1,
    JavaDynamic = 2,
    JavaCompiled = 3,
    JavaInterp = 4,
    JavaNative = 5,
    JavaRuntime = 6,
    JavaLeaf = 7,
}

// Component::use_def_enum bit values
pub const USE: i32 = 0x1;
pub const DEF: i32 = 0x2;
pub const USE_DEF: i32 = USE | DEF;
pub const KILL: i32 = 0x4;
pub const USE_KILL: i32 = USE | KILL;
pub const SYNTHETIC: i32 = 0x8;
pub const TEMP: i32 = USE | SYNTHETIC;
pub const TEMP_DEF: i32 = TEMP | DEF;
pub const CALL: i32 = 0x10;
pub const INVALID_UDEF: i32 = 0x0;

pub fn effect_lookup(name: &str) -> i32 {
    match name {
        "USE" => USE,
        "DEF" => DEF,
        "USE_DEF" => USE_DEF,
        "KILL" => KILL,
        "USE_KILL" => USE_KILL,
        "TEMP" => TEMP,
        "TEMP_DEF" => TEMP_DEF,
        "INVALID" => INVALID_UDEF,
        "CALL" => CALL,
        _ => INVALID_UDEF,
    }
}

#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    pub type_name: String,
    pub usedef: i32,
}

impl Component {
    pub fn is(&self, flag: i32) -> bool {
        self.usedef & flag == flag && flag != 0
    }
    pub fn isa(&self, flag: i32) -> bool {
        self.usedef & flag != 0
    }
    pub fn usedef_name(&self) -> &'static str {
        match self.usedef {
            INVALID_UDEF => "INVALID",
            USE => "USE",
            USE_DEF => "USE_DEF",
            USE_KILL => "USE_KILL",
            KILL => "KILL",
            TEMP => "TEMP",
            TEMP_DEF => "TEMP_DEF",
            DEF => "DEF",
            CALL => "CALL",
            _ => "Undefined Use/Def info",
        }
    }
    pub fn promote_use_def_info(&mut self, new_use_def: i32) {
        // C++: _usedef = (_usedef & ~USE) | (new_use_def & USE) ... simplified:
        // port of Component::promote_use_def_info
        self.usedef |= new_use_def;
    }
}

#[derive(Clone, Debug, Default)]
pub struct ComponentList {
    pub components: Vec<Component>,
    pub match_count: usize,
}

impl ComponentList {
    pub fn insert(&mut self, name: &str, op_type: &str, usedef: i32, mflag: bool) {
        self.components.push(Component {
            name: name.to_string(),
            type_name: op_type.to_string(),
            usedef,
        });
        if mflag {
            self.match_count += 1;
        }
    }
    pub fn search(&self, name: &str) -> Option<usize> {
        self.components.iter().position(|c| c.name == name)
    }
    pub fn num_operands(&self) -> usize {
        self.components.len()
    }
    pub fn operand_position(&self, name: &str) -> i32 {
        self.operand_position_usedef(name, None)
    }
    // Port of ComponentList::operand_position(name) and (name,usedef,fm);
    // the Form* fm parameter is only used for diagnostics, so it is dropped.
    pub fn operand_position_usedef(&self, name: &str, usedef: Option<i32>) -> i32 {
        const NOT_IN_LIST: i32 = -1;
        let mut position: i32 = 0;
        let num_opnds = self.num_operands() as i32;
        let mut num_opnds_adj = num_opnds;
        let mut preceding_non_use: Option<&Component> = None;
        let mut first_def: Option<&Component> = None;
        for (idx, component) in self.components.iter().enumerate() {
            let mut pos = position + idx as i32;
            if idx == 0 && !component.is(DEF) {
                pos += 1;
                num_opnds_adj += 1;
            }
            if let Some(ud) = usedef {
                if component.name == name && component.is(ud) {
                    if idx == 0 && ud == USE && component.is(DEF) {
                        return pos + 1;
                    }
                    let _ = (preceding_non_use, first_def, num_opnds_adj);
                    return pos;
                }
            } else if component.name == name {
                return pos;
            }
            if component.is(DEF) && component.is(USE) {
                pos += 1;
                if pos != 1 {
                    pos -= 1;
                }
            }
            if component.is(DEF) && first_def.is_none() {
                first_def = Some(component);
            }
            if !component.is(USE) && Some(component) != first_def {
                preceding_non_use = Some(component);
            } else if let Some(p) = preceding_non_use {
                if component.name == p.name {
                    preceding_non_use = None;
                }
            }
        }
        let _ = num_opnds_adj;
        NOT_IN_LIST
    }
    pub fn label_position(&self) -> i32 {
        let mut position = 0i32;
        for (idx, comp) in self.components.iter().enumerate() {
            let mut pos = position + idx as i32;
            if idx == 0 && !comp.is(DEF) {
                pos += 1;
            }
            if comp.type_name == "label" {
                return pos;
            }
            if comp.is(DEF) && comp.is(USE) {
                pos += 1;
                if pos != 1 {
                    pos -= 1;
                }
            }
        }
        -1
    }
    pub fn method_position(&self) -> i32 {
        let mut position = 0i32;
        for (idx, comp) in self.components.iter().enumerate() {
            let mut pos = position + idx as i32;
            if idx == 0 && !comp.is(DEF) {
                pos += 1;
            }
            if comp.type_name == "method" {
                return pos;
            }
            if comp.is(DEF) && comp.is(USE) {
                pos += 1;
                if pos != 1 {
                    pos -= 1;
                }
            }
        }
        -1
    }
}

//------------------------------Expr------------------------------------------
#[derive(Clone, Debug)]
pub struct Expr {
    pub external_name: Option<String>,
    pub expr: String,
    pub min_value: i64,
    pub max_value: i64,
}

impl Expr {
    pub fn new(cost: &str) -> Expr {
        Expr {
            external_name: None,
            expr: cost.to_string(),
            min_value: 0,
            max_value: 0,
        }
    }
    pub fn named(name: &str, expression: Option<&str>, min_value: i64, max_value: i64) -> Expr {
        Expr {
            external_name: Some(name.to_string()),
            expr: expression.unwrap_or("").to_string(),
            min_value,
            max_value,
        }
    }
    pub fn as_string(&self) -> &str {
        self.external_name.as_deref().unwrap_or(&self.expr)
    }
}

//------------------------------NameList---------------------------------------
pub const SIGNAL: &str = "$$SIGNAL$$";
pub const SIGNAL2: &str = "$$SIGNAL2$$";
pub const SIGNAL3: &str = "$$SIGNAL3$$";

pub type NameList = Vec<String>;

pub fn name_list_add_signal(list: &mut NameList) {
    list.push(SIGNAL.to_string());
}

pub fn name_list_search(list: &[String], name: &str) -> bool {
    list.iter().any(|n| n == name)
}

pub fn name_list_index(list: &[String], name: &str) -> i32 {
    list.iter().position(|n| n == name).map(|i| i as i32).unwrap_or(-1)
}

//------------------------------MatchNode / MatchRule--------------------------
#[derive(Clone, Debug)]
pub struct MatchNode {
    pub result: Option<String>,
    pub name: Option<String>,
    pub op_type: String,
    pub l_child: Option<Box<MatchNode>>,
    pub r_child: Option<Box<MatchNode>>,
    pub num_leaves: usize,
    pub internalop: Option<String>,
    pub commutative_id: i32,
}

impl MatchNode {
    pub fn new(result: Option<String>, name: Option<String>, op_type: String,
               l_child: Option<Box<MatchNode>>, r_child: Option<Box<MatchNode>>) -> MatchNode {
        let num_leaves = l_child.as_ref().map_or(0, |c| c.num_leaves)
            + r_child.as_ref().map_or(0, |c| c.num_leaves);
        MatchNode {
            result, name, op_type, l_child, r_child, num_leaves,
            internalop: None, commutative_id: 0,
        }
    }

    pub fn leaf(result: Option<String>, name: Option<String>, op_type: String) -> MatchNode {
        MatchNode {
            result, name, op_type,
            l_child: None, r_child: None, num_leaves: 1,
            internalop: None, commutative_id: 0,
        }
    }

    pub fn sets_result(&self) -> bool {
        self.name.as_deref() == Some("Set")
    }

    pub fn find_type(&self, ty: &str) -> bool {
        if let Some(l) = &self.l_child {
            if l.find_type(ty) {
                return true;
            }
        }
        if let Some(r) = &self.r_child {
            if r.find_type(ty) {
                return true;
            }
        }
        self.op_type == ty
    }

    // Port of MatchNode::append_components
    pub fn append_components(&self, globals: &HashMap<String, FormRef>,
                             components: &mut ComponentList, def_flag: bool) {
        let usedef = if def_flag { DEF } else { USE };
        if self.l_child.is_none() && self.r_child.is_none() {
            if let Some(f) = globals.get(&self.op_type) {
                let f = f.borrow();
                if !f.ideal_only()
                    && (f.kind == FormKind::OpClass || f.kind == FormKind::Operand)
                {
                    if let Some(n) = &self.name {
                        components.insert(n, &self.op_type, usedef, true);
                    }
                }
            }
            return;
        }
        let tmpdef = self.op_type == "Set";
        if let Some(l) = &self.l_child {
            l.append_components(globals, components, tmpdef);
        }
        if let Some(r) = &self.r_child {
            r.append_components(globals, components, false);
        }
    }

    // Port of MatchNode::base_operand (recursive through user-defined match rules)
    pub fn base_operand(&self, position: &mut u32, globals: &HashMap<String, FormRef>,
                        result: &mut Option<String>, name: &mut Option<String>,
                        op_type: &mut Option<String>) -> bool {
        if self.l_child.is_none() && self.r_child.is_none() {
            if self.op_type == "Universe" || self.op_type == "label" {
                if *position == 0 {
                    *result = self.result.clone();
                    *name = self.name.clone();
                    *op_type = Some(self.op_type.clone());
                    return true;
                }
                *position -= 1;
                return false;
            }
            let mut match_node: Option<&MatchNode> = None;
            if let Some(form) = globals.get(&self.op_type) {
                let f = form.borrow();
                match f.kind {
                    FormKind::Operand => {
                        if let Some(mr) = &f.matrule {
                            match_node = Some(&mr.borrow().m);
                        }
                    }
                    FormKind::Instruct => {
                        if let Some(mr) = &f.matrule {
                            match_node = Some(&mr.borrow().m);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(mn) = match_node {
                return mn.base_operand(position, globals, result, name, op_type);
            }
            if *position == 0 {
                *result = self.result.clone();
                *name = self.name.clone();
                *op_type = Some(self.op_type.clone());
                return true;
            }
            *position -= 1;
            return false;
        }
        if let Some(l) = &self.l_child {
            if l.base_operand(position, globals, result, name, op_type) {
                return true;
            }
        }
        if let Some(r) = &self.r_child {
            if r.base_operand(position, globals, result, name, op_type) {
                return true;
            }
        }
        false
    }

    pub fn count_commutative_op(&mut self, count: &mut i32, globals: &HashMap<String, FormRef>) {
        const COMMUT_OP_LIST: &[&str] = &[
            "AddI", "AddL", "AddHF", "AddF", "AddD",
            "AndI", "AndL",
            "MaxI", "MinI", "MaxHF", "MinHF", "MaxF", "MinF", "MaxD", "MinD",
            "MulI", "MulL", "MulHF", "MulF", "MulD",
            "OrI", "OrL",
            "XorI", "XorL",
            "UMax", "UMin",
        ];
        const COMMUT_VECTOR_OP_LIST: &[&str] = &[
            "AddVB", "AddVS", "AddVI", "AddVL", "AddVHF", "AddVF", "AddVD",
            "MulVB", "MulVS", "MulVI", "MulVL", "MulVHF", "MulVF", "MulVD",
            "AndV", "OrV", "XorV",
            "MaxVHF", "MinVHF", "MaxV", "MinV", "UMax", "UMin",
        ];
        if self.l_child.is_some() && self.r_child.is_some()
            && (self.l_child.as_ref().unwrap().l_child.is_some()
                || self.r_child.as_ref().unwrap().l_child.is_some())
        {
            let mut is_const = false;
            {
                let r = self.r_child.as_ref().unwrap();
                if r.l_child.is_none() && r.r_child.is_none() {
                    if let Some(form) = globals.get(&r.op_type) {
                        let f = form.borrow();
                        if f.kind == FormKind::Operand {
                            if let Some(it) = f.interface_type(globals) {
                                if it == InterfaceKind::Const {
                                    is_const = true;
                                }
                            }
                        }
                    }
                }
            }
            if !is_const {
                let l_op = self.l_child.as_ref().unwrap().op_type.clone();
                let r_op = self.r_child.as_ref().unwrap().op_type.clone();
                if COMMUT_VECTOR_OP_LIST.contains(&self.op_type.as_str()) {
                    if l_op != "Binary" && r_op != "Binary" {
                        *count += 1;
                        self.commutative_id = *count;
                    }
                } else if COMMUT_OP_LIST.contains(&self.op_type.as_str()) {
                    *count += 1;
                    self.commutative_id = *count;
                }
            }
        }
        if let Some(l) = &mut self.l_child {
            l.count_commutative_op(count, globals);
        }
        if let Some(r) = &mut self.r_child {
            r.count_commutative_op(count, globals);
        }
    }

    pub fn swap_commutative_op(&mut self, atroot: bool, id: i32, globals: &HashMap<String, FormRef>) {
        if self.commutative_id == id {
            std::mem::swap(&mut self.l_child, &mut self.r_child);
        }
        let is_set = self.op_type == "Set";
        if let Some(l) = &mut self.l_child {
            l.swap_commutative_op(is_set, id, globals);
        }
        if let Some(r) = &mut self.r_child {
            r.swap_commutative_op(is_set, id, globals);
        }
        if !atroot && (self.l_child.is_some() || self.r_child.is_some()) {
            self.build_internalop(globals);
        }
    }

    // Port of MatchNode::build_internalop; needs access to AD internal op tables,
    // passed via globals + a callback-free approach: caller supplies maps.
    pub fn build_internalop_with(&mut self, internal_ops: &mut HashMap<String, String>,
                                 internal_match: &mut HashMap<String, MatchNodeRef>) {
        let lstr = self.l_child.as_ref()
            .map(|c| c.internalop.clone().unwrap_or_else(|| c.op_type.clone()))
            .unwrap_or_default();
        let rstr = self.r_child.as_ref()
            .map(|c| c.internalop.clone().unwrap_or_else(|| c.op_type.clone()))
            .unwrap_or_default();
        let subtree = format!("_{}_{}_{}", self.op_type, lstr, rstr);
        let iop = if let Some(existing) = internal_ops.get(&subtree) {
            existing.clone()
        } else {
            internal_match.insert(subtree.clone(), Rc::new(RefCell::new(self.clone())));
            internal_ops.insert(subtree.clone(), subtree.clone());
            subtree
        };
        self.internalop = Some(iop.clone());
        self.result = Some(iop);
    }

    pub fn build_internalop(&mut self, globals: &HashMap<String, FormRef>) {
        // Fallback without AD tables: generate name and store only locally.
        let lstr = self.l_child.as_ref()
            .map(|c| c.internalop.clone().unwrap_or_else(|| c.op_type.clone()))
            .unwrap_or_default();
        let rstr = self.r_child.as_ref()
            .map(|c| c.internalop.clone().unwrap_or_else(|| c.op_type.clone()))
            .unwrap_or_default();
        let subtree = format!("_{}_{}_{}", self.op_type, lstr, rstr);
        self.internalop = Some(subtree.clone());
        self.result = Some(subtree);
    }

    pub fn num_consts(&self, globals: &HashMap<String, FormRef>) -> usize {
        let mut n = 0;
        self.num_consts_rec(globals, &mut n, DataType::None);
        n
    }

    fn num_consts_rec(&self, globals: &HashMap<String, FormRef>, n: &mut usize, ty: DataType) {
        if self.l_child.is_none() && self.r_child.is_none() {
            if let Some(form) = globals.get(&self.op_type) {
                let f = form.borrow();
                if f.kind == FormKind::Operand {
                    if ty == DataType::None {
                        // count all constants: operand whose interface is Const
                        if let Some(it) = f.interface_type(globals) {
                            if it == InterfaceKind::Const {
                                *n += 1;
                            }
                        }
                    }
                }
            }
            return;
        }
        if let Some(l) = &self.l_child {
            l.num_consts_rec(globals, n, ty);
        }
        if let Some(r) = &self.r_child {
            r.num_consts_rec(globals, n, ty);
        }
    }
}

pub type MatchNodeRef = Rc<RefCell<MatchNode>>;

#[derive(Clone, Debug)]
pub struct MatchRule {
    pub m: MatchNode,
    pub mach_type: Option<String>,
    pub depth: i32,
    pub construct: Option<String>,
    pub numchilds: usize,
    pub next: Option<MatchRuleRef>,
}

impl MatchRule {
    pub fn new(m: MatchNode, depth: i32, construct: Option<String>, num_leaves: usize) -> MatchRule {
        let numchilds = (if m.l_child.is_some() { 1 } else { 0 })
            + (if m.r_child.is_some() { 1 } else { 0 });
        MatchRule {
            m, mach_type: None, depth, construct, numchilds, next: None,
        }
    }

    pub fn sets_result(&self) -> bool {
        self.m.sets_result()
    }

    pub fn is_chain_rule(&self, globals: &HashMap<String, FormRef>) -> bool {
        if self.m.l_child.is_none() && self.m.r_child.is_none() {
            if let Some(form) = globals.get(&self.m.op_type) {
                let f = form.borrow();
                if f.kind == FormKind::Operand && !f.ideal_only() {
                    return true;
                }
            }
        }
        if let Some(r) = &self.m.r_child {
            let rch = r.op_type.clone();
            if self.m.op_type == "Set" {
                if let Some(form) = globals.get(&rch) {
                    let f = form.borrow();
                    if f.kind == FormKind::Operand {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn is_ideal_if(&self) -> bool {
        self.m.op_type == "If" || self.m.op_type == "CountedLoopEnd"
    }
    pub fn is_ideal_fastlock(&self) -> bool {
        self.m.op_type == "Set"
            && self.m.r_child.as_ref().map_or(false, |r| r.op_type == "FastLock")
    }
    pub fn is_ideal_membar(&self) -> bool {
        matches!(self.m.op_type.as_str(),
            "MemBarAcquire" | "MemBarRelease" | "MemBarAcquireLock" | "MemBarReleaseLock"
            | "LoadFence" | "StoreFence" | "StoreStoreFence" | "MemBarVolatile"
            | "MemBarCPUOrder" | "MemBarStoreStore" | "OnSpinWait")
    }
    pub fn is_ideal_loadpc(&self) -> bool {
        self.m.op_type == "Set"
            && self.m.r_child.as_ref().map_or(false, |r| r.op_type == "LoadPC")
    }
    pub fn is_ideal_box(&self) -> bool {
        self.m.op_type == "Set"
            && self.m.r_child.as_ref().map_or(false, |r| r.op_type == "Box")
    }
    pub fn is_ideal_goto(&self) -> bool {
        self.m.op_type == "Goto"
    }
    pub fn is_ideal_jump(&self) -> bool {
        self.m.op_type == "Jump"
    }
    pub fn is_ideal_bool(&self) -> bool {
        self.m.op_type == "Bool"
    }
    pub fn is_ideal_load(&self) -> DataType {
        if self.m.op_type == "Set" {
            if let Some(r) = &self.m.r_child {
                return is_load_from_memory(&r.op_type);
            }
        }
        DataType::None
    }
    pub fn is_ideal_store(&self) -> DataType {
        if self.m.op_type == "Set" {
            if let Some(r) = &self.m.r_child {
                return is_store_to_memory(&r.op_type);
            }
        }
        DataType::None
    }
    pub fn skip_antidep_check(&self) -> bool {
        if self.m.op_type == "Set" {
            if let Some(r) = &self.m.r_child {
                return matches!(r.op_type.as_str(), "LoadKlass" | "LoadNKlass" | "LoadRange");
            }
        }
        false
    }
    pub fn is_vector(&self) -> bool {
        const VECTOR_LIST: &[&str] = &[
            "AddVB","AddVS","AddVI","AddVL","AddVHF","AddVF","AddVD",
            "SubVB","SubVS","SubVI","SubVL","SubVHF","SubVF","SubVD",
            "MulVB","MulVS","MulVI","MulVL","MulVHF","MulVF","MulVD",
            "DivVHF","DivVF","DivVD",
            "AbsVB","AbsVS","AbsVI","AbsVL","AbsVF","AbsVD",
            "NegVF","NegVD","NegVI","NegVL",
            "SqrtVD","SqrtVF","SqrtVHF",
            "AndV","XorV","OrV",
            "MaxV","MinV","MinVHF","MaxVHF","UMinV","UMaxV",
            "CompressV","ExpandV","CompressM","CompressBitsV","ExpandBitsV",
            "AddReductionVI","AddReductionVL","AddReductionVF","AddReductionVD",
            "MulReductionVI","MulReductionVL","MulReductionVF","MulReductionVD",
            "MaxReductionV","MinReductionV","AndReductionV","OrReductionV","XorReductionV",
            "MulAddVS2VI","MacroLogicV","LShiftCntV","RShiftCntV",
            "LShiftVB","LShiftVS","LShiftVI","LShiftVL",
            "RShiftVB","RShiftVS","RShiftVI","RShiftVL",
            "URShiftVB","URShiftVS","URShiftVI","URShiftVL",
            "Replicate","ReverseV","ReverseBytesV",
            "RoundDoubleModeV","RotateLeftV","RotateRightV","LoadVector","StoreVector",
            "LoadVectorGather","StoreVectorScatter","LoadVectorGatherMasked","StoreVectorScatterMasked",
            "SelectFromTwoVector","VectorTest","VectorLoadMask","VectorStoreMask","VectorBlend","VectorInsert",
            "VectorRearrange","VectorLoadShuffle","VectorLoadConst",
            "VectorCastB2X","VectorCastS2X","VectorCastI2X","VectorCastL2X","VectorCastF2X","VectorCastD2X","VectorCastF2HF","VectorCastHF2F",
            "VectorUCastB2X","VectorUCastS2X","VectorUCastI2X",
            "VectorMaskWrapper","VectorMaskCmp","VectorReinterpret","LoadVectorMasked","StoreVectorMasked",
            "FmaVD","FmaVF","FmaVHF","PopCountVI","PopCountVL","PopulateIndex","VectorLongToMask",
            "CountLeadingZerosV","CountTrailingZerosV","SignumVF","SignumVD","SaturatingAddV","SaturatingSubV",
            "MaskAll","AndVMask","OrVMask","XorVMask","VectorMaskCast",
            "RoundVF","RoundVD",
            "PackB","PackS","PackI","PackL","PackF","PackD","Pack2L","Pack2D",
            "ExtractB","ExtractUB","ExtractC","ExtractS","ExtractI","ExtractL","ExtractF","ExtractD",
        ];
        if let Some(r) = &self.m.r_child {
            return VECTOR_LIST.contains(&r.op_type.as_str());
        }
        false
    }
}

//------------------------------The Form---------------------------------------
#[derive(Debug)]
pub struct Form {
    pub kind: FormKind,
    pub linenum: i32,
    pub ident: String,
    pub ideal_only: bool,

    // SourceForm/HeaderForm/PreHeaderForm
    pub code: Option<String>,

    // parameters & local name table (operand lists, pipe class params)
    pub parameters: NameList,
    pub local_names: HashMap<String, FormRef>,

    // match rule (Operand/Instruct)
    pub matrule: Option<MatchRuleRef>,

    // predicate / constraint / construct / format / attribute lists
    pub predicate: Option<String>,
    pub constraint_func: Option<String>,
    pub constraint_arg: Option<String>,
    pub construct_expr: Option<String>,
    pub attribs: Option<Rc<Attribute>>,
    pub effects: HashMap<String, FormRef>, // operand name -> Effect form
    pub has_call: bool,

    // interface (Operand)
    pub interface: Option<Interface>,

    // format rule (strings interleaved with rep vars via SIGNAL markers)
    pub format_strings: NameList,
    pub format_rep_vars: NameList,

    // expand / rewrite
    pub expand_newopers: NameList,
    pub expand_newopconst: HashMap<String, String>,
    pub expand_instrs: Vec<(String, NameList)>,
    pub rewrite_params: Option<String>,
    pub rewrite_block: Option<String>,

    // instruction fields
    pub opcode_primary: Option<String>,
    pub opcode_secondary: Option<String>,
    pub opcode_tertiary: Option<String>,
    pub size: Option<String>,
    pub insencode: Option<InsEncode>,
    pub constant_encode: Option<InsEncode>,
    pub is_postalloc_expand: bool,
    pub is_mach_constant: bool,
    pub needs_constant_base: bool,
    pub has_expand_rule: bool,
    pub ins_pipe: Option<String>,
    pub flag_names: Vec<String>,
    pub peephole: Option<FormRef>, // chain via Form.next_peephole
    pub next_peephole: Option<FormRef>,
    pub components: ComponentList,
    pub alignment: u32,
    pub is_cisc_alternate: bool,
    pub cisc_spill_operand: i32,
    pub short_branch_form: Option<String>,
    pub is_short_branch: bool,

    // operand fields
    pub classes: NameList, // opclasses containing this operand
    pub oplst: NameList,   // OpClassForm operand list
    pub min_code: i32,
    pub max_code: i32,

    // effect
    pub effect_name: String,
    pub use_def: i32,

    // register forms
    pub rdefs: NameList,
    pub reg_def: HashMap<String, FormRef>,
    pub rclasses: NameList,
    pub reg_class: HashMap<String, FormRef>,
    pub aclasses: NameList,
    pub alloc_class: HashMap<String, FormRef>,
    // RegDef fields
    pub callconv: Option<String>,
    pub c_conv: Option<String>,
    pub idealtype: Option<String>,
    pub register_encode: Option<String>,
    pub concrete: Option<String>,
    pub register_num: u32,
    // RegClass fields
    pub stack_or_reg: bool,
    pub code_snippet: Option<String>,
    pub condition_code: Option<String>,
    pub rclass_at: [Option<FormRef>; 2],

    // pipeline
    pub reslist: NameList,
    pub resdict: HashMap<String, u32>,
    pub rescount: i32,
    pub maxcycleused: i32,
    pub stages: NameList,
    pub stagecnt: i32,
    pub classlist: NameList,
    pub classdict: HashMap<String, FormRef>,
    pub classcnt: i32,
    pub noplist: NameList,
    pub nopcnt: i32,
    pub variable_size_instrs: bool,
    pub branch_has_delay_slot: bool,
    pub max_instrs_per_bundle: i32,
    pub max_bundles_per_cycle: i32,
    pub instr_unit_size: i32,
    pub bundle_unit_size: i32,
    pub instr_fetch_unit_size: i32,
    pub instr_fetch_units: i32,
    // PipeClassForm fields
    pub pipe_num: i32,
    pub has_fixed_latency: bool,
    pub fixed_latency: i32,
    pub instruction_count: i32,
    pub has_multiple_bundles: bool,
    pub has_branch_delay: bool,
    pub force_serialization: bool,
    pub may_have_no_code: bool,
    pub pipe_instructs: NameList,
    pub local_usage: HashMap<String, FormRef>, // name -> PipeClassOperandForm
    pub res_usage: Vec<FormRef>,              // PipeClassResourceForm

    // encode
    pub eclasses: NameList,
    pub enc_class: HashMap<String, FormRef>,
    // EncClass fields
    pub parameter_type: NameList,
    pub parameter_name: NameList,
    pub code_strings: NameList,
    pub rep_vars: NameList,

    // frame
    pub frame_sync_stack_slots: Option<String>,
    pub frame_inline_cache_reg: Option<String>,
    pub frame_interpreter_frame_pointer_reg: Option<String>,
    pub frame_cisc_spilling_operand_name: Option<String>,
    pub frame_pointer: Option<String>,
    pub c_frame_pointer: Option<String>,
    pub alignment_str: Option<String>,
    pub return_addr_loc: bool,
    pub c_return_addr_loc: bool,
    pub return_addr: Option<String>,
    pub c_return_addr: Option<String>,
    pub varargs_c_out_slots_killed: Option<String>,
    pub return_value: Option<String>,
    pub c_return_value: Option<String>,

    // peephole
    pub peep_predicate: Option<String>,
    pub peep_match_instrs: Vec<(i32, i32, String, i32)>, // (parent,position,name,input)
    pub peep_match_max_position: i32,
    pub peep_procedure: Option<String>,
    pub peep_constraints: Vec<(i32, String, String, i32, String)>,
    pub peep_replace_root: Option<String>,
    pub peep_replace_operands: Vec<(i32, String)>,
}

impl Form {
    pub fn new(kind: FormKind) -> Form {
        Form {
            kind,
            linenum: 0,
            ident: String::new(),
            ideal_only: false,
            code: None,
            parameters: Vec::new(),
            local_names: HashMap::new(),
            matrule: None,
            predicate: None,
            constraint_func: None,
            constraint_arg: None,
            construct_expr: None,
            attribs: None,
            effects: HashMap::new(),
            has_call: false,
            interface: None,
            format_strings: Vec::new(),
            format_rep_vars: Vec::new(),
            expand_newopers: Vec::new(),
            expand_newopconst: HashMap::new(),
            expand_instrs: Vec::new(),
            rewrite_params: None,
            rewrite_block: None,
            opcode_primary: None,
            opcode_secondary: None,
            opcode_tertiary: None,
            size: None,
            insencode: None,
            constant_encode: None,
            is_postalloc_expand: false,
            is_mach_constant: false,
            needs_constant_base: false,
            has_expand_rule: false,
            ins_pipe: None,
            flag_names: Vec::new(),
            peephole: None,
            next_peephole: None,
            components: ComponentList::default(),
            alignment: 1,
            is_cisc_alternate: false,
            cisc_spill_operand: -1,
            short_branch_form: None,
            is_short_branch: false,
            oplst: Vec::new(),
            min_code: 0,
            max_code: 0,
            effect_name: String::new(),
            use_def: 0,
            rdefs: Vec::new(),
            reg_def: HashMap::new(),
            rclasses: Vec::new(),
            reg_class: HashMap::new(),
            aclasses: Vec::new(),
            alloc_class: HashMap::new(),
            callconv: None,
            c_conv: None,
            idealtype: None,
            register_encode: None,
            concrete: None,
            register_num: 0,
            stack_or_reg: false,
            code_snippet: None,
            condition_code: None,
            rclass_at: [None, None],
            reslist: Vec::new(),
            resdict: HashMap::new(),
            rescount: 0,
            maxcycleused: 0,
            stages: Vec::new(),
            stagecnt: 0,
            classlist: Vec::new(),
            classdict: HashMap::new(),
            classcnt: 0,
            noplist: Vec::new(),
            nopcnt: 0,
            variable_size_instrs: false,
            branch_has_delay_slot: false,
            max_instrs_per_bundle: 0,
            max_bundles_per_cycle: 1,
            instr_unit_size: 0,
            bundle_unit_size: 0,
            instr_fetch_unit_size: 0,
            instr_fetch_units: 0,
            pipe_num: 0,
            has_fixed_latency: false,
            fixed_latency: 0,
            instruction_count: 0,
            has_multiple_bundles: false,
            has_branch_delay: false,
            force_serialization: false,
            may_have_no_code: false,
            pipe_instructs: Vec::new(),
            local_usage: HashMap::new(),
            res_usage: Vec::new(),
            eclasses: Vec::new(),
            enc_class: HashMap::new(),
            parameter_type: Vec::new(),
            parameter_name: Vec::new(),
            code_strings: Vec::new(),
            rep_vars: Vec::new(),
            frame_sync_stack_slots: None,
            frame_inline_cache_reg: None,
            frame_interpreter_frame_pointer_reg: None,
            frame_cisc_spilling_operand_name: None,
            frame_pointer: None,
            c_frame_pointer: None,
            alignment_str: None,
            return_addr_loc: false,
            c_return_addr_loc: false,
            return_addr: None,
            c_return_addr: None,
            varargs_c_out_slots_killed: None,
            return_value: None,
            c_return_value: None,
            peep_predicate: None,
            peep_match_instrs: Vec::new(),
            peep_match_max_position: 0,
            peep_procedure: None,
            peep_constraints: Vec::new(),
            peep_replace_root: None,
            peep_replace_operands: Vec::new(),
        }
    }

    pub fn is_opclass(&self) -> bool {
        self.kind == FormKind::OpClass || self.kind == FormKind::Operand
    }
    pub fn is_operand_kind(&self) -> bool {
        self.kind == FormKind::Operand
    }
    pub fn is_instruction(&self) -> bool {
        self.kind == FormKind::Instruct
    }
    pub fn is_attribute_form(&self) -> bool {
        self.kind == FormKind::AttributeForm
    }
    pub fn is_effect_kind(&self) -> bool {
        self.kind == FormKind::Effect
    }
    pub fn is_pipeclass(&self) -> bool {
        self.kind == FormKind::PipeClass
    }
    pub fn is_resource(&self) -> bool {
        self.kind == FormKind::Resource
    }
    pub fn is_machnode(&self) -> bool {
        self.kind == FormKind::MachNode
    }
    pub fn is_regclass(&self) -> bool {
        matches!(self.kind, FormKind::RegClass | FormKind::CodeSnippetRegClass | FormKind::ConditionalRegClass)
    }

    // interface_type for Operand forms
    pub fn interface_type(&self, _globals: &HashMap<String, FormRef>) -> Option<InterfaceKind> {
        match self.kind {
            FormKind::Operand => self.interface.as_ref().map(|i| i.kind),
            FormKind::OpClass => {
                // union of member operand interface types; simplified: first member's
                for name in &self.oplst {
                    // caller resolves; return None to be resolved by helper
                    let _ = name;
                }
                None
            }
            _ => None,
        }
    }

    // InstructForm predicate helpers
    pub fn is_ideal_control(&self) -> bool {
        self.is_ideal_return() || self.is_ideal_branch()
            || self.matrule.as_ref().map_or(false, |m| m.borrow().m.op_type == "Jump")
            || self.is_ideal_halt()
    }
    pub fn is_ideal_return(&self) -> bool {
        if let Some(m) = &self.matrule {
            let m = m.borrow();
            for ty in ["Return", "Rethrow", "TailCall", "TailJump", "ForwardException"] {
                if m.m.find_type(ty) {
                    return true;
                }
            }
        }
        false
    }
    pub fn is_ideal_halt(&self) -> bool {
        self.matrule.as_ref().map_or(false, |m| m.borrow().m.find_type("Halt"))
    }
    pub fn is_ideal_safepoint(&self) -> bool {
        self.matrule.as_ref().map_or(false, |m| m.borrow().m.find_type("SafePoint"))
    }
    pub fn is_ideal_branch(&self) -> bool {
        if let Some(m) = &self.matrule {
            let m = m.borrow();
            return m.is_ideal_if() || m.is_ideal_goto();
        }
        false
    }
    pub fn is_ideal_jump(&self) -> bool {
        if let Some(m) = &self.matrule {
            return m.borrow().is_ideal_jump();
        }
        false
    }
    pub fn is_ideal_call(&self) -> CallType {
        let Some(m) = &self.matrule else { return CallType::Invalid };
        let m = m.borrow();
        for (ty, ct) in [
            ("CallStaticJava", CallType::JavaStatic),
            ("Lock", CallType::JavaStatic),
            ("Unlock", CallType::JavaStatic),
            ("CallDynamicJava", CallType::JavaDynamic),
            ("CallRuntime", CallType::JavaRuntime),
            ("CallLeaf", CallType::JavaLeaf),
            ("CallLeafNoFP", CallType::JavaLeaf),
            ("CallLeafVector", CallType::JavaLeaf),
        ] {
            if m.m.find_type(ty) {
                return ct;
            }
        }
        CallType::Invalid
    }
    pub fn mach_base_class(&self) -> &'static str {
        match self.is_ideal_call() {
            CallType::JavaStatic => return "MachCallStaticJavaNode",
            CallType::JavaDynamic => return "MachCallDynamicJavaNode",
            CallType::JavaRuntime => return "MachCallRuntimeNode",
            CallType::JavaLeaf => return "MachCallLeafNode",
            _ => {}
        }
        if self.is_ideal_return() {
            return "MachReturnNode";
        }
        if self.is_ideal_halt() {
            return "MachHaltNode";
        }
        if self.is_ideal_safepoint() {
            return "MachSafePointNode";
        }
        if self.is_ideal_branch() && self.matrule.as_ref().map_or(false, |m| m.borrow().is_ideal_if()) {
            return "MachIfNode";
        }
        if self.is_ideal_goto_name() {
            return "MachGotoNode";
        }
        if self.matrule.as_ref().map_or(false, |m| m.borrow().is_ideal_fastlock()) {
            return "MachFastLockNode";
        }
        if self.ident.starts_with("Nop_") {
            return "MachNopNode";
        }
        if self.matrule.as_ref().map_or(false, |m| m.borrow().is_ideal_membar()) {
            return "MachMemBarNode";
        }
        if self.is_ideal_jump() {
            return "MachJumpNode";
        }
        if self.is_mach_constant {
            return "MachConstantNode";
        }
        "MachNode"
    }
    fn is_ideal_goto_name(&self) -> bool {
        self.matrule.as_ref().map_or(false, |m| m.borrow().is_ideal_goto())
    }
    pub fn expands(&self) -> bool {
        !self.expand_newopers.is_empty() || !self.expand_instrs.is_empty()
            || self.rewrite_block.is_some() || self.expand_has_rules()
    }
    fn expand_has_rules(&self) -> bool {
        self.has_expand_rule
    }
    pub fn postalloc_expands(&self) -> bool {
        self.is_postalloc_expand
    }

    pub fn cost(&self, globals: &HashMap<String, FormRef>) -> Option<String> {
        let mut cur = self.attribs.clone();
        while let Some(a) = cur {
            if a.ident == INS_COST {
                return Some(a.val.clone());
            }
            cur = a.next.clone();
        }
        // operand cost uses OP_COST; caller distinguishes
        let _ = globals;
        None
    }
    pub fn op_cost(&self) -> Option<String> {
        let mut cur = self.attribs.clone();
        while let Some(a) = cur {
            if a.ident == OP_COST {
                return Some(a.val.clone());
            }
            cur = a.next.clone();
        }
        None
    }
    pub fn ins_cost(&self) -> Option<String> {
        let mut cur = self.attribs.clone();
        while let Some(a) = cur {
            if a.ident == INS_COST {
                return Some(a.val.clone());
            }
            cur = a.next.clone();
        }
        None
    }
    pub fn ideal_only(&self) -> bool {
        self.ideal_only
    }
}

// InsEncode: list of (encode_class_name, Vec<param>)
#[derive(Clone, Debug, Default)]
pub struct InsEncode {
    pub encodes: Vec<(String, Vec<String>)>,
}

impl InsEncode {
    pub fn add_encode(&mut self, name: &str) -> usize {
        self.encodes.push((name.to_string(), Vec::new()));
        self.encodes.len() - 1
    }
}

//------------------------------ArchDesc---------------------------------------
#[derive(Default)]
pub struct ArchDesc {
    pub global_names: HashMap<String, FormRef>,
    pub global_defs: HashMap<String, Expr>,
    pub defines_order: NameList,
    pub ideal_index: HashMap<String, usize>,
    pub internal_op_counter: usize,

    pub pre_header: Vec<FormRef>,
    pub header: Vec<FormRef>,
    pub source: Vec<FormRef>,
    pub instructions: Vec<FormRef>,
    pub machnodes: Vec<FormRef>,
    pub operands: Vec<FormRef>,
    pub opclass: Vec<FormRef>,
    pub attributes: Vec<FormRef>,
    pub register: Option<FormRef>,
    pub frame: Option<FormRef>,
    pub encode: Option<FormRef>,
    pub pipeline: Option<FormRef>,

    pub has_match_rule: Vec<bool>,
    pub mlistab: Vec<Vec<MatchListEntry>>,

    pub chain_rules: HashMap<String, ChainList>,
    pub internal_ops: HashMap<String, String>,
    pub internal_op_names: NameList,
    // internal_match maps internal name -> serialized match node is complex;
    // store the MatchNode clone.
    pub internal_match: HashMap<String, MatchNode>,

    pub preproc_list: NameList,
    pub preproc_table: HashMap<String, String>,

    pub total_lines: usize,
    pub no_output: bool,
    pub quiet_mode: bool,
    pub disable_warnings: bool,
    pub adl_debug: i32,
    pub adlocation_debug: i32,
    pub cisc_spill_debug: bool,
    pub short_branch_debug: bool,

    pub syntax_errs: i32,
    pub semantic_errs: i32,
    pub warnings: i32,
    pub internal_errs: i32,

    pub adl_file_name: String,
    pub needs_deep_clone_jvms: bool,
}

#[derive(Clone, Debug)]
pub struct MatchListEntry {
    pub opcode: String,
    pub result_str: String,
    pub lchild: String,
    pub rchild: String,
    pub pred: Option<String>,
    pub cost: String,
}

#[derive(Clone, Debug, Default)]
pub struct ChainList {
    pub name: Vec<String>,
    pub cost: Vec<String>,
    pub rule: Vec<String>,
}

impl ChainList {
    pub fn insert(&mut self, name: &str, cost: &str, rule: &str) {
        self.name.push(name.to_string());
        self.cost.push(cost.to_string());
        self.rule.push(rule.to_string());
    }
    pub fn search(&self, name: &str) -> bool {
        self.name.iter().any(|n| n == name)
    }
}

impl ArchDesc {
    pub fn new() -> ArchDesc {
        let mut ad = ArchDesc {
            has_match_rule: vec![false; LAST_OPCODE],
            mlistab: vec![Vec::new(); LAST_OPCODE],
            adl_file_name: String::new(),
            ..Default::default()
        };
        init_keywords(&mut ad.global_names);
        ad.init_base_op_types();
        ad
    }

    pub fn get_preproc_def(&self, flag: &str) -> Option<&str> {
        self.preproc_table.get(flag).map(|s| s.as_str())
    }
    pub fn set_preproc_def(&mut self, flag: &str, def: Option<&str>) {
        if !self.preproc_table.contains_key(flag) {
            self.preproc_list.push(flag.to_string());
        }
        match def {
            Some(d) => {
                self.preproc_table.insert(flag.to_string(), d.to_string());
            }
            None => {
                self.preproc_table.remove(flag);
            }
        }
    }

    pub fn global_names_get(&self, name: &str) -> Option<FormRef> {
        self.global_names.get(name).cloned()
    }

    pub fn add_form(&mut self, form: FormRef) {
        let f = form.borrow();
        match f.kind {
            FormKind::PreHeader => self.pre_header.push(form.clone()),
            FormKind::Header => self.header.push(form.clone()),
            FormKind::Source => self.source.push(form.clone()),
            FormKind::Encode => {
                self.encode = Some(form.clone());
            }
            FormKind::Instruct => self.instructions.push(form.clone()),
            FormKind::MachNode => self.machnodes.push(form.clone()),
            FormKind::Operand => self.operands.push(form.clone()),
            FormKind::OpClass => self.opclass.push(form.clone()),
            FormKind::AttributeForm => self.attributes.push(form.clone()),
            FormKind::Register => self.register = Some(form.clone()),
            FormKind::Frame => self.frame = Some(form.clone()),
            FormKind::Pipeline => self.pipeline = Some(form.clone()),
            _ => {}
        }
    }

    pub fn get_registers(&self) -> Option<FormRef> {
        self.register.clone()
    }

    pub fn construct_operand(&mut self, ident: &str, ideal_only: bool) -> FormRef {
        let mut f = Form::new(FormKind::Operand);
        f.ident = ident.to_string();
        f.ideal_only = ideal_only;
        let r = Rc::new(RefCell::new(f));
        self.global_names.insert(ident.to_string(), r.clone());
        self.add_form(r.clone());
        r
    }

    pub fn init_base_op_types(&mut self) {
        for i in 1..LAST_MACHINE_LEAF {
            let ident = NODE_CLASS_NAMES[i];
            self.construct_operand(ident, true);
        }
        for j in (LAST_MACHINE_LEAF + 1)..LAST_OPCODE {
            let ident = NODE_CLASS_NAMES[j];
            match ident {
                "ConI" | "ConP" | "ConN" | "ConNKlass" | "ConH" | "ConF" | "ConD"
                | "ConL" | "Con" | "Bool" => {
                    self.construct_operand(ident, true);
                }
                "_last_class_name" => {}
                _ => {
                    let mut f = Form::new(FormKind::Instruct);
                    f.ident = ident.to_string();
                    f.ideal_only = true;
                    let r = Rc::new(RefCell::new(f));
                    self.global_names.insert(ident.to_string(), r.clone());
                    self.add_form(r);
                }
            }
        }
        // Universe, label, sReg*, method
        self.construct_operand("Universe", false);
        self.construct_operand("label", false);
        for sreg in ["sRegI", "sRegP", "sRegF", "sRegD", "sRegL"] {
            let r = self.construct_operand(sreg, false);
            r.borrow_mut().constraint_func = Some("ALLOC_IN_RC".to_string());
            r.borrow_mut().constraint_arg = Some("stack_slots".to_string());
        }
        self.construct_operand("method", false);
        // Effects
        for e in ["USE", "DEF", "USE_DEF", "KILL", "USE_KILL", "TEMP", "TEMP_DEF", "CALL"] {
            let mut f = Form::new(FormKind::Effect);
            f.ident = e.to_string();
            f.effect_name = e.to_string();
            f.use_def = effect_lookup(e);
            let r = Rc::new(RefCell::new(f));
            self.global_names.insert(e.to_string(), r);
        }
        // ideal index
        for i in 1..LAST_MACHINE_LEAF {
            self.ideal_index.insert(NODE_CLASS_NAMES[i].to_string(), i);
        }
        for i in (LAST_MACHINE_LEAF + 1)..LAST_OPCODE {
            let name = NODE_CLASS_NAMES[i];
            if name != "_last_class_name" {
                self.ideal_index.insert(name.to_string(), i);
            }
        }
    }

    pub fn operand_form_count(&self) -> usize {
        self.operands.iter().filter(|o| o.borrow().matrule.is_some()).count()
    }
    pub fn opclass_form_count(&self) -> usize {
        self.opclass.len()
    }
    pub fn instruct_form_count(&self) -> usize {
        self.instructions.iter().filter(|o| o.borrow().matrule.is_some()).count()
    }

    pub fn emit_msg(&self, flag: i32, linenum: i32, msg: &str) -> i32 {
        let pref = match flag {
            0 => "Warning: ",
            1 => "Syntax Error: ",
            2 => "Semantic Error: ",
            3 => "Internal Error: ",
            _ => "",
        };
        eprintln!("{}({}) {:<8}: {}", self.adl_file_name, linenum, pref, msg);
        1
    }
    pub fn parse_err(&mut self, flag: i32, linenum: i32, msg: &str) {
        match flag {
            SYNERR => self.syntax_errs += self.emit_msg(flag, linenum, msg),
            SEMERR => self.semantic_errs += self.emit_msg(flag, linenum, msg),
            _ => self.warnings += self.emit_msg(flag, linenum, msg),
        }
        self.no_output = true;
    }
}

fn init_keywords(names: &mut HashMap<String, FormRef>) {
    // Keywords are stored with a null form; we use a sentinel empty Form.
    for kw in [
        "instruct", "operand", "attribute", "source", "register", "pipeline",
        "constraint", "predicate", "encode", "enc_class", "interface", "opcode",
        "ins_encode", "match", "effect", "expand", "rewrite", "reg_def",
        "reg_class", "alloc_class", "resource", "pipe_class", "pipe_desc",
        "postalloc_expand", "size", "ins_pipe", "flag", "format", "peephole",
        "definitions", "source_hpp", "frame", "int_def", "opclass",
        "ins_attrib", "op_attrib", "enc_class", "reg_class_dynamic",
    ] {
        names.insert(kw.to_string(), Rc::new(RefCell::new(Form::new(FormKind::ExprDef))));
    }
}
