/*
 * OpenJDK 25 HotSpot adlc/ port
 *
 * Post-parse inspection phase: builds component lists for operands and
 * instructions, constructs chain rules and generates the DFA match lists
 * (ArchDesc::generateMatchLists / inspectOperands / inspectInstructions /
 * buildMatchList / buildMList / build_chain_rule in the C++ adlc).
 */

use std::collections::HashMap;

use super::model::*;
use super::opcodes::LAST_OPCODE;

// Root operand types that never get match lists (C++ inspectOperands).
fn is_special_root_op(root_op: &str) -> bool {
    matches!(root_op,
        "Universe" | "label" | "sRegI" | "sRegP" | "sRegF" | "sRegD" | "sRegL")
}

fn attr_int_val(val: &str) -> i64 {
    // Simplified Attribute::int_val: the attribute default/value must be an
    // integer constant.  Parenthesized expressions are unwrapped; anything
    // else yields 0 (the C++ version reports a semantic error through the
    // expression evaluator).
    let mut t = val.trim();
    loop {
        if let Some(inner) = t.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            t = inner.trim();
        } else {
            break;
        }
    }
    let neg = t.starts_with('-');
    let digits = t.strip_prefix('-').unwrap_or(t);
    if !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) {
        digits.parse::<i64>().map(|v| if neg { -v } else { v }).unwrap_or(0)
    } else {
        0
    }
}

impl ArchDesc {
    /// Port of ArchDesc::generateMatchLists: call after parsing completes.
    pub fn generate_match_lists(&mut self) {
        let globals = self.global_names.clone();
        let ideal_index = self.ideal_index.clone();
        self.inspect_operands(&globals, &ideal_index);
        self.inspect_instructions(&globals, &ideal_index);
    }

    //------------------------------inspectOperands-----------------------------
    fn inspect_operands(&mut self,
                        globals: &HashMap<String, FormRef>,
                        ideal_index: &HashMap<String, usize>) {
        let default_op_cost = globals.get(OP_COST)
            .and_then(|f| f.borrow().code.clone());
        for op in self.operands.clone() {
            self.build_components(&op, globals);
            let (has_mrule, root_op, pred, cost_attr) = {
                let b = op.borrow();
                (b.matrule.is_some(), b.ident.clone(),
                 b.predicate.clone(), b.op_cost())
            };
            if !has_mrule {
                continue;
            }
            self.build_chain_rule(&op, globals);
            if is_special_root_op(&root_op) {
                continue;
            }
            let cost = cost_attr.or_else(|| default_op_cost.clone()).unwrap_or_default();
            let result = root_op.clone();
            let mut rule = op.borrow().matrule.clone();
            while let Some(r) = rule {
                r.borrow_mut().mach_type = Some(root_op.clone());
                self.build_match_list(&r, &result, &root_op,
                                      pred.as_deref(), &cost, globals, ideal_index);
                rule = r.borrow().next.clone();
            }
        }
    }

    //------------------------------inspectInstructions------------------------
    fn inspect_instructions(&mut self,
                             globals: &HashMap<String, FormRef>,
                             ideal_index: &HashMap<String, usize>) {
        let default_ins_cost = globals.get(INS_COST)
            .and_then(|f| f.borrow().code.clone());
        for instr in self.instructions.clone() {
            {
                let b = instr.borrow();
                if b.ideal_only {
                    continue;
                }
            }
            self.build_components(&instr, globals);
            let (has_mrule, root_op, cost_attr, linenum, is_short) = {
                let b = instr.borrow();
                (b.matrule.is_some(), b.ident.clone(),
                 b.ins_cost(), b.linenum, b.is_short_branch)
            };
            if !has_mrule {
                continue;
            }
            let cost = cost_attr.or_else(|| default_ins_cost.clone()).unwrap_or_default();
            let result = {
                let b = instr.borrow();
                b.reduce_result()
            };
            // label position check
            let label_pos = {
                let b = instr.borrow();
                b.components.label_position()
            };
            let is_branch = {
                let b = instr.borrow();
                b.is_ideal_branch()
            };
            if (is_branch && label_pos == -1) || (!is_branch && label_pos != -1) {
                self.parse_err(SEMERR, linenum,
                    &format!("{}: Only branches to a label are supported\n", root_op));
            }
            // attributes: ins_short_branch / ins_alignment
            let mut set_short = false;
            let mut set_align: Option<u32> = None;
            {
                let b = instr.borrow();
                let mut cur = b.attribs.clone();
                while let Some(a) = cur {
                    if a.ident == "ins_short_branch" && attr_int_val(&a.val) != 0 {
                        if !is_branch || label_pos == -1 {
                            self.parse_err(SEMERR, linenum,
                                &format!("{}: Only short branch to a label is supported\n", root_op));
                        }
                        set_short = true;
                    } else if a.ident == "ins_alignment" {
                        let v = attr_int_val(&a.val);
                        if v != 0 {
                            set_align = Some(v as u32);
                        }
                    }
                    cur = a.next.clone();
                }
            }
            if set_short {
                instr.borrow_mut().is_short_branch = true;
            }
            if let Some(al) = set_align {
                instr.borrow_mut().alignment = al;
            }
            let do_build = !instr.borrow().is_short_branch && !is_short;
            if do_build {
                let mr = instr.borrow().matrule.clone().unwrap();
                mr.borrow_mut().mach_type = Some(root_op.clone());
                self.build_match_list(&mr, &result, &root_op,
                                      instr.borrow().predicate.as_deref(),
                                      &cost, globals, ideal_index);
            }
        }
    }

    //------------------------------build_components----------------------------
    // Port of InstructForm::build_components / OperandForm::build_components.
    fn build_components(&mut self, form: &FormRef,
                        globals: &HashMap<String, FormRef>) {
        let kind = form.borrow().kind;
        match kind {
            FormKind::Operand => self.build_operand_components(form, globals),
            FormKind::Instruct => self.build_instruct_components(form, globals),
            _ => {}
        }
    }

    fn build_operand_components(&mut self, form: &FormRef,
                                globals: &HashMap<String, FormRef>) {
        {
            let mut b = form.borrow_mut();
            b.components = ComponentList::default();
            let mr = b.matrule.clone();
            if let Some(mr) = mr {
                mr.borrow().m.append_components(globals, &mut b.components, false);
            }
        }
        let params = form.borrow().parameters.clone();
        for name in params {
            let type_ident = form.borrow().local_names.get(&name)
                .map(|f| f.borrow().ident.clone())
                .unwrap_or_default();
            let mut b = form.borrow_mut();
            if b.components.operand_position(&name) == -1 {
                b.components.insert(&name, &type_ident, INVALID_UDEF, false);
            }
        }
    }

    fn build_instruct_components(&mut self, form: &FormRef,
                                 globals: &HashMap<String, FormRef>) {
        let (ident, linenum) = {
            let b = form.borrow();
            (b.ident.clone(), b.linenum)
        };
        let mut has_temp = false;
        let mut kill_name: Option<String> = None;
        {
            let mut b = form.borrow_mut();
            b.components = ComponentList::default();
            let mr = b.matrule.clone();
            if let Some(mr) = mr {
                mr.borrow().m.append_components(globals, &mut b.components, false);
            }
        }
        let params = form.borrow().parameters.clone();
        for name in params {
            let type_ident = form.borrow().local_names.get(&name)
                .map(|f| f.borrow().ident.clone())
                .unwrap_or_default();
            let e_use_def = form.borrow().effects.get(&name)
                .map(|ef| ef.borrow().use_def);
            if let Some(ud) = e_use_def {
                has_temp |= (ud & TEMP) == TEMP;
                // KILLs must be declared after any TEMPs (C++ ordering check)
                if kill_name.is_some() && (ud & TEMP) == TEMP && (ud & DEF) == 0 {
                    self.parse_err(SYNERR, linenum,
                        &format!("{}: {} {} must be at the end of the argument list\n",
                                 ident, type_ident, name));
                } else if (ud & KILL) != 0 && (ud & USE) == 0 {
                    kill_name = Some(name.clone());
                }
            }
            let mut b = form.borrow_mut();
            let pos = b.components.search(&name);
            match pos {
                None => {
                    let ud = e_use_def.unwrap_or(INVALID_UDEF);
                    b.components.insert(&name, &type_ident, ud, false);
                }
                Some(idx) => {
                    if let Some(ud) = e_use_def {
                        let comp_isa_use = b.components.components[idx].isa(USE);
                        if !comp_isa_use && (ud & USE) != 0 {
                            // Adding 'USE' to a component that is not yet one:
                            // insert an extra component (C++ behaviour).
                            b.components.insert(&name, &type_ident, ud, false);
                        } else {
                            b.components.components[idx].usedef |= ud;
                        }
                    }
                }
            }
        }
        let has_mrule = form.borrow().matrule.is_some();
        if !has_mrule && has_temp {
            self.parse_err(SEMERR, linenum,
                &format!("{}: TEMPs without match rule isn't supported\n", ident));
        }
    }

    //------------------------------build_chain_rule-----------------------------
    // Port of ArchDesc::build_chain_rule (operand chain rules).
    fn build_chain_rule(&mut self, oper: &FormRef,
                        globals: &HashMap<String, FormRef>) {
        let mut rules_to_add: Vec<(String, String, String)> = Vec::new(); // (src, cost, result)
        {
            let b = oper.borrow();
            let Some(mr) = &b.matrule else { return };
            let mr = mr.borrow();
            let is_leaf_rule = mr.m.l_child.is_none() && mr.m.r_child.is_none();
            if is_leaf_rule {
                if let Some(form) = globals.get(&mr.m.op_type) {
                    let f = form.borrow();
                    if f.kind == FormKind::Operand && !f.ideal_only() {
                        rules_to_add.push((mr.m.op_type.clone(),
                            b.op_cost().unwrap_or_default(), b.ident.clone()));
                    }
                }
                // additional chain rules
                let mut next = mr.next.clone();
                while let Some(r) = next {
                    let rb = r.borrow();
                    if let Some(form) = globals.get(&rb.m.op_type) {
                        let f = form.borrow();
                        if f.kind == FormKind::Operand && !f.ideal_only() {
                            rules_to_add.push((rb.m.op_type.clone(),
                                b.op_cost().unwrap_or_default(), b.ident.clone()));
                        }
                    }
                    next = rb.next.clone();
                }
            } else if mr.next.is_some() {
                let mut next = mr.next.clone();
                while let Some(r) = next {
                    let rb = r.borrow();
                    if let Some(form) = globals.get(&rb.m.op_type) {
                        let f = form.borrow();
                        if f.kind == FormKind::Operand && !f.ideal_only() {
                            rules_to_add.push((rb.m.op_type.clone(),
                                b.op_cost().unwrap_or_default(), b.ident.clone()));
                        }
                    }
                    next = rb.next.clone();
                }
            }
        }
        for (src, cost, result) in rules_to_add {
            self.add_chain_rule_entry(&src, &cost, &result, globals);
        }
    }

    fn add_chain_rule_entry(&mut self, src: &str, cost: &str, result: &str,
                            globals: &HashMap<String, FormRef>) {
        let default_cost = globals.get(OP_COST)
            .and_then(|f| f.borrow().code.clone())
            .unwrap_or_default();
        let cost = if cost.is_empty() { default_cost.as_str() } else { cost };
        let entry = self.chain_rules.entry(src.to_string()).or_default();
        if !entry.search(result) {
            entry.insert(result, cost, result);
        }
    }

    //------------------------------buildMatchList-------------------------------
    fn build_match_list(&mut self, mrule: &MatchRuleRef, result_str: &str,
                        root_op: &str, pred: Option<&str>, cost: &str,
                        globals: &HashMap<String, FormRef>,
                        ideal_index: &HashMap<String, usize>) {
        {
            let mr = mrule.borrow();
            if mr.is_chain_rule(globals) {
                return;
            }
            let index_str = if mr.sets_result() {
                mr.m.r_child.as_ref().map(|c| c.op_type.clone())
            } else {
                Some(mr.m.op_type.clone())
            };
            let Some(index_str) = index_str else { return };
            let Some(&index) = ideal_index.get(&index_str) else {
                eprintln!("Ideal node missing: {}", index_str);
                return;
            };
            if index >= LAST_OPCODE {
                eprintln!("Invalid match rule {} <-- ( {} )",
                          result_str, root_op);
                return;
            }
            if mr.m.op_type == "Set" {
                if let Some(rc) = &mr.m.r_child {
                    // Recurse on the child node of the Set rule
                    let rc = (**rc).clone();
                    drop(mr);
                    self.build_mlist_node(&rc, Some(root_op), Some(result_str),
                                          pred, Some(cost), globals, ideal_index);
                    return;
                }
                return;
            }
            // Non-Set: build children then insert this level's entry
            let mut leftstr: Option<String> = None;
            let mut rightstr: Option<String> = None;
            if let Some(l) = &mr.m.l_child {
                let l = (**l).clone();
                self.build_mlist_node(&l, None, None, None, None, globals, ideal_index);
                leftstr = Some(l.internalop.clone().unwrap_or_else(|| l.op_type.clone()));
            }
            if let Some(r) = &mr.m.r_child {
                let r = (**r).clone();
                self.build_mlist_node(&r, None, None, None, None, globals, ideal_index);
                rightstr = Some(r.internalop.clone().unwrap_or_else(|| r.op_type.clone()));
            }
            drop(mr);
            self.insert_mlist(index, root_op, result_str,
                              leftstr.as_deref(), rightstr.as_deref(),
                              pred, cost);
        }
    }

    //------------------------------buildMList-----------------------------------
    // Recursive helper operating on an owned MatchNode clone.
    fn build_mlist_node(&mut self, node: &MatchNode,
                        root_op: Option<&str>, result_op: Option<&str>,
                        pred: Option<&str>, cost: Option<&str>,
                        globals: &HashMap<String, FormRef>,
                        ideal_index: &HashMap<String, usize>) {
        // Do not process leaves of the Match Tree if they are not ideal
        if node.l_child.is_none() && node.r_child.is_none() {
            if let Some(form) = globals.get(&node.op_type) {
                let f = form.borrow();
                if !f.ideal_only() {
                    return;
                }
            }
        }
        let index_str = node.op_type.clone();
        let Some(&index) = ideal_index.get(&index_str) else {
            eprintln!("error: operand \"{}\" not found", index_str);
            return;
        };
        let mut leftstr: Option<String> = None;
        let mut rightstr: Option<String> = None;
        if let Some(l) = &node.l_child {
            self.build_mlist_node(l, None, None, None, None, globals, ideal_index);
            leftstr = Some(l.internalop.clone().unwrap_or_else(|| l.op_type.clone()));
        }
        if let Some(r) = &node.r_child {
            self.build_mlist_node(r, None, None, None, None, globals, ideal_index);
            rightstr = Some(r.internalop.clone().unwrap_or_else(|| r.op_type.clone()));
        }
        let opcode = root_op.map(|s| s.to_string())
            .or_else(|| node.internalop.clone())
            .unwrap_or_else(|| node.op_type.clone());
        let resultop = result_op.map(|s| s.to_string())
            .or_else(|| node.internalop.clone())
            .unwrap_or_else(|| node.op_type.clone());
        self.insert_mlist_opt(index, &opcode, &resultop,
                              leftstr.as_deref(), rightstr.as_deref(),
                              pred, cost);
    }

    fn insert_mlist(&mut self, index: usize, opcode: &str, result_str: &str,
                    lchild: Option<&str>, rchild: Option<&str>,
                    pred: Option<&str>, cost: &str) {
        self.insert_mlist_opt(index, opcode, result_str, lchild, rchild, pred, Some(cost));
    }

    fn insert_mlist_opt(&mut self, index: usize, opcode: &str, result_str: &str,
                        lchild: Option<&str>, rchild: Option<&str>,
                        pred: Option<&str>, cost: Option<&str>) {
        if index >= self.mlistab.len() {
            return;
        }
        let entry = MatchListEntry {
            opcode: opcode.to_string(),
            result_str: result_str.to_string(),
            lchild: lchild.map(|s| s.to_string()),
            rchild: rchild.map(|s| s.to_string()),
            pred: pred.map(|s| s.to_string()),
            cost: cost.unwrap_or("0").to_string(),
        };
        let list = &mut self.mlistab[index];
        if !list.iter().any(|e| e.matches(&entry)) {
            list.insert(0, entry);
        }
    }
}

impl MatchListEntry {
    // Port of MatchList::search (equality on all fields).
    fn matches(&self, other: &MatchListEntry) -> bool {
        self.opcode == other.opcode
            && self.result_str == other.result_str
            && self.lchild == other.lchild
            && self.rchild == other.rchild
            && self.pred == other.pred
            && self.cost == other.cost
    }
}

//------------------------------Form helpers------------------------------------
impl Form {
    /// Port of InstructForm::reduce_result / OperandForm::reduce_result.
    pub fn reduce_result(&self) -> String {
        match self.kind {
            FormKind::Operand => return self.ident.clone(),
            FormKind::Instruct => {}
            _ => return self.ident.clone(),
        }
        let mut result = "Universe".to_string();
        if let Some(first) = self.components.components.first() {
            if first.isa(DEF) {
                result = first.type_name.clone();
                // Override this if the rule is a store operation:
                if let Some(mr) = &self.matrule {
                    let mrb = mr.borrow();
                    if let Some(r) = &mrb.m.r_child {
                        if is_store_to_memory(&r.op_type) != DataType::None {
                            result = "Universe".to_string();
                        }
                    }
                }
            }
        }
        result
    }

    /// Raw predicate text for match-list entries.  The C++ build_predicate
    /// additionally synthesizes equality checks for operand names that appear
    /// more than once; those are derivable from the component list, so the
    /// raw predicate is stored here.
    pub fn build_predicate_text(&self) -> Option<String> {
        self.predicate.clone()
    }
}
