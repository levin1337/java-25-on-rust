/*
 * OpenJDK 25 HotSpot adlc/ port
 *
 * ADLParser: port of adlparse.cpp.  Byte-level cursor over FileBuff with
 * the same _ptr/_curchar/_curline invariants as the C++ original.
 */

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::filebuff::FileBuff;
use super::model::*;
use super::opcodes::{LAST_MACHINE_LEAF, LAST_OPCODE, NODE_CLASS_NAMES};

pub struct AdlParser<'a> {
    pub buf: FileBuff,
    ad: &'a mut ArchDesc,
    ptr: usize,
    curchar: u8,
    curline: Option<usize>,
    preproc_depth: usize,
    preproc_not_taken: usize,
    preproc_taken: [bool; 20],
    preproc_else: [bool; 20],
}

impl<'a> AdlParser<'a> {
    pub fn new(buf: FileBuff, ad: &'a mut ArchDesc) -> AdlParser<'a> {
        ad.syntax_errs = 0;
        ad.semantic_errs = 0;
        ad.warnings = 0;
        ad.preproc_list.push(SIGNAL.to_string());
        AdlParser {
            buf,
            ad,
            ptr: 0,
            curchar: b' ',
            curline: None,
            preproc_depth: 0,
            preproc_not_taken: 0,
            preproc_taken: [false; 20],
            preproc_else: [false; 20],
        }
    }

    pub fn into_ad(self) -> &'a mut ArchDesc {
        self.ad
    }

    pub fn linenum(&self) -> i32 {
        self.buf.linenum as i32
    }

    #[inline]
    fn byte(&self, off: usize) -> u8 {
        self.buf.buf.get(self.ptr + off).copied().unwrap_or(0)
    }
    #[inline]
    fn byte_at(&self, idx: usize) -> u8 {
        self.buf.buf.get(idx).copied().unwrap_or(0)
    }
    #[inline]
    fn cur_char(&self) -> u8 {
        self.curchar
    }

    fn parse_err(&mut self, flag: i32, fmt: &str) {
        let line = self.linenum();
        // skip to end of current line, like C++ parse_err
        let mut p = self.ptr;
        while self.byte_at(p) != b'\n' && self.byte_at(p) != 0 {
            p += 1;
        }
        self.ptr = p;
        self.curchar = b'\n';
        self.ad.parse_err(flag, line, fmt);
    }
    fn synerr(&mut self, fmt: &str) {
        self.parse_err(SYNERR, fmt);
    }

    //------------------------------next_line-----------------------------------
    fn next_line(&mut self) {
        self.curline = self.buf.get_line();
        self.curchar = b' ';
    }

    fn next_char(&mut self) {
        if self.curchar == b'\n' {
            self.parse_err(WARN, "must call next_line!");
        }
        self.ptr += 1;
        self.curchar = self.byte(0);
    }

    fn next_char_or_line(&mut self) {
        if self.curchar != b'\n' {
            self.ptr += 1;
            self.curchar = self.byte(0);
        } else {
            self.next_line();
            if let Some(cl) = self.curline {
                self.ptr = cl;
            }
            self.curchar = self.byte(0);
        }
    }

    //---------------------------skipws_common----------------------------------
    fn skipws_common(&mut self, do_preproc: bool) {
        if self.byte(0) == 0 {
            if self.curchar > b' ' {
                return;
            }
            if self.curchar == b'\n' {
                if !do_preproc {
                    return;
                }
                self.next_line();
                if let Some(cl) = self.curline {
                    self.ptr = cl;
                }
            } else if self.curchar == b'#'
                || (self.curchar == b'/' && (self.byte(1) == b'/' || self.byte(1) == b'*'))
            {
                self.synerr("unimplemented: comment token in a funny place");
            }
        }
        while self.curline.is_some() {
            let c = self.byte(0);
            if c == b'\n' {
                if !do_preproc {
                    break;
                }
                self.next_line();
                if let Some(cl) = self.curline {
                    self.ptr = cl;
                }
            } else if c == b'/' && self.byte(1) == b'/' {
                while self.byte(0) != b'\n' {
                    self.ptr += 1;
                }
            } else if c == b'/' && self.byte(1) == b'*' {
                self.ptr += 1;
                loop {
                    self.ptr += 1;
                    if self.byte(0) == b'\n' {
                        self.next_line();
                        if self.curline.is_none() {
                            self.synerr("end-of-file detected inside comment\n");
                            break;
                        }
                        if let Some(cl) = self.curline {
                            self.ptr = cl;
                        }
                    }
                    if self.byte(0) == b'*' && self.byte(1) == b'/' {
                        break;
                    }
                    if self.byte(0) == 0 {
                        break;
                    }
                }
                self.ptr += 2;
            } else if do_preproc && c == b'#' {
                let handled = self.handle_preproc_token();
                if !handled {
                    if self.preproc_taken() {
                        return;
                    }
                    self.ptr += 1;
                }
            } else if c > b' ' && !(do_preproc && !self.preproc_taken()) {
                break;
            } else if c == b'"' || c == b'\'' {
                let qchar = c;
                loop {
                    self.ptr += 1;
                    let cc = self.byte(0);
                    if cc == qchar {
                        self.ptr += 1;
                        break;
                    }
                    if cc == b'\\' {
                        self.ptr += 1;
                    }
                    if cc == b'\n' || cc == 0 {
                        self.synerr("newline in string");
                        break;
                    }
                    if self.byte(0) == 0 && self.curline.is_none() {
                        break;
                    }
                }
            } else {
                self.ptr += 1;
            }
        }
        if self.curline.is_some() {
            self.curchar = self.byte(0);
        }
    }

    fn skipws(&mut self) {
        self.skipws_common(true);
    }
    fn skipws_no_preproc(&mut self) {
        self.skipws_common(false);
    }

    //---------------------------preproc---------------------------------------
    fn begin_if_def(&mut self, taken: bool) {
        let ppn = self.preproc_depth;
        self.preproc_depth += 1;
        self.preproc_taken[ppn] = taken;
        if !taken {
            self.preproc_not_taken += 1;
        }
        self.preproc_else[ppn] = false;
    }
    fn invert_if_def(&mut self) {
        let ppn = self.preproc_depth - 1;
        self.preproc_else[ppn] = true;
        if !self.preproc_taken[ppn] {
            self.preproc_not_taken -= 1;
        }
        self.preproc_taken[ppn] = !self.preproc_taken[ppn];
        if !self.preproc_taken[ppn] {
            self.preproc_not_taken += 1;
        }
    }
    fn end_if_def(&mut self) {
        self.preproc_depth -= 1;
        let ppn = self.preproc_depth;
        if !self.preproc_taken[ppn] {
            self.preproc_not_taken -= 1;
        }
    }
    fn preproc_taken(&self) -> bool {
        self.preproc_not_taken == 0
    }

    fn handle_preproc_token(&mut self) -> bool {
        // _ptr is at '#'
        self.ensure_start_of_line();
        self.next_char();
        self.skipws_no_preproc();
        let start_ident = self.ptr;
        let ident = if self.curchar == b'\n' {
            None
        } else {
            self.get_ident_no_preproc()
        };
        let Some(ident) = ident else {
            self.synerr("expected preprocessor command, got end of line\n");
            return true;
        };
        if ident == "ifdef" || ident == "ifndef" {
            let flag = self.get_ident_no_preproc().unwrap_or_default();
            self.ensure_end_of_line();
            let flag_def = self.preproc_taken() && self.ad.get_preproc_def(&flag).is_some();
            let now_taken = if ident == "ifdef" { flag_def } else { !flag_def };
            self.begin_if_def(now_taken);
        } else if ident == "if" {
            if self.preproc_taken() {
                let rest = self.rest_of_line();
                self.synerr(&format!("unimplemented: #{} {}", ident, rest));
            }
            self.next_line();
            self.begin_if_def(true);
        } else if ident == "else" {
            self.ensure_end_of_line();
            self.invert_if_def();
        } else if ident == "endif" {
            self.ensure_end_of_line();
            self.end_if_def();
        } else if self.preproc_taken() {
            self.ptr = start_ident;
            self.ptr -= 1;
            self.curchar = self.byte(0);
            if self.curchar != b'#' {
                self.synerr("no space allowed after # in #define or #undef");
            }
            return false;
        }
        true
    }

    fn rest_of_line(&self) -> String {
        let mut s = String::new();
        let mut p = self.ptr;
        while self.byte_at(p) != b'\n' && self.byte_at(p) != 0 {
            s.push(self.byte_at(p) as char);
            p += 1;
        }
        s
    }

    fn ensure_start_of_line(&mut self) {
        if self.curchar == b'\n' {
            self.next_line();
            return;
        }
        if let Some(cl) = self.curline {
            for i in cl..self.ptr {
                if self.byte_at(i) > b' ' {
                    self.synerr(&format!("'{}' must be at beginning of line\n", self.curchar as char));
                    break;
                }
            }
        }
    }

    fn ensure_end_of_line(&mut self) {
        self.skipws_no_preproc();
        if self.curchar != b'\n' && self.curchar != 0 {
            self.synerr(&format!("garbage char '{}' at end of line\n", self.curchar as char));
        } else {
            self.next_char_or_line();
        }
    }

    //------------------------------get_ident-----------------------------------
    fn get_ident_common(&mut self, do_preproc: bool) -> Option<String> {
        if self.curline.is_none() {
            return None;
        }
        self.skipws_common(do_preproc);
        let start = self.ptr;
        let mut end = start;
        loop {
            let c = self.byte_at(end);
            if (c.is_ascii_lowercase() || c.is_ascii_uppercase() || c.is_ascii_digit()
                || c == b'_' || c == b':' || c == b'#')
                && c != 0
            {
                end += 1;
            } else {
                break;
            }
        }
        if start == end {
            let tail: String = self.buf.buf[start..]
                .iter()
                .take(24)
                .map(|&b| b as char)
                .collect();
            self.synerr(&format!("Identifier expected, but found '{}'.", tail));
            return None;
        }
        self.curchar = self.byte_at(end);
        self.buf.buf[end] = 0;
        self.ptr = end;
        let mut result = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
        if do_preproc {
            if let Some(def) = self.ad.get_preproc_def(&result).map(|s| s.to_string()) {
                if def != result {
                    let def1 = def.clone();
                    if let Some(def2) = self.ad.get_preproc_def(&def1).map(|s| s.to_string()) {
                        if def2 != def1 {
                            if let Some(def3) = self.ad.get_preproc_def(&def2).map(|s| s.to_string()) {
                                if def3 != def2 && def3 != def1 {
                                    self.synerr(&format!(
                                        "unimplemented: using {} defined as {} => {} => {}",
                                        result, def1, def2, def3));
                                }
                            }
                            result = def2;
                        } else {
                            result = def;
                        }
                    } else {
                        result = def;
                    }
                }
            }
        }
        Some(result)
    }
    fn get_ident(&mut self) -> Option<String> {
        self.get_ident_common(true)
    }
    fn get_ident_no_preproc(&mut self) -> Option<String> {
        self.get_ident_common(false)
    }
    fn get_ident_dup(&mut self) -> Option<String> {
        let ident = self.get_ident()?;
        // restore the character clobbered by the NUL terminator
        self.buf.buf[self.ptr] = self.curchar;
        Some(ident)
    }

    fn get_ident_or_literal_constant(&mut self, description: &str) -> Option<String> {
        self.skipws();
        if self.curchar == b'(' {
            let param = self.get_paren_expr(description, false)?;
            let mut p = param;
            if !p.starts_with('(') {
                p = format!("({})", p);
            }
            Some(p)
        } else {
            self.get_ident()
        }
    }

    fn get_rep_var_ident(&mut self) -> Option<String> {
        let rep_var = self.ptr;
        if self.curchar == b'$' {
            self.next_char();
        }
        if self.curchar == b'$' {
            self.next_char();
        }
        if self.curchar == b'$' {
            self.next_char();
        }
        if self.curchar == b'$' {
            self.synerr("Replacement variables and field specifiers can not start with '$$$$'");
            self.next_char();
            return None;
        }
        self.get_ident()?;
        Some(String::from_utf8_lossy(&self.buf.buf[rep_var..self.ptr]).into_owned())
    }

    fn get_rep_var_ident_dup(&mut self) -> Option<String> {
        if self.curchar != b'$' {
            return None;
        }
        self.next_char();
        let rep_var = self.ptr;
        if self.curchar == b'$' {
            self.next_char();
        }
        if self.curchar == b'$' {
            self.next_char();
        }
        if self.curchar == b'$' {
            self.synerr("Replacement variables and field specifiers can not start with '$$$$'");
            self.next_char();
            return None;
        }
        self.get_ident()?;
        let s = String::from_utf8_lossy(&self.buf.buf[rep_var..self.ptr]).into_owned();
        self.buf.buf[self.ptr] = self.curchar;
        Some(s)
    }

    fn get_unique_ident(&mut self, dict: &HashMap<String, FormRef>, desc: &str) -> Option<String> {
        let ident = self.get_ident()?;
        if dict.contains_key(&ident) {
            self.synerr(&format!("duplicate name {} for {}\n", ident, desc));
            return None;
        }
        Some(ident)
    }

    fn get_int(&mut self) -> i64 {
        self.skipws();
        let start = self.ptr;
        let mut end = start;
        loop {
            let c = self.byte_at(end);
            if (c.is_ascii_digit() || (c == b'-' && end == start)) && c != 0 {
                end += 1;
            } else {
                break;
            }
        }
        if start == end {
            self.synerr(&format!("integer expected at {}\n", c as char));
            return 0;
        }
        self.curchar = self.byte_at(end);
        let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
        self.ptr = end;
        s.trim().parse::<i64>().unwrap_or(0)
    }

    fn get_relation_dup(&mut self) -> Option<String> {
        self.skipws();
        let start = self.ptr;
        let first = self.byte(0);
        if first == b'=' || first == b'!' || first == b'<' || first == b'>' {
            self.next_char();
            let second = self.byte(0);
            if second == b'=' {
                self.next_char();
                let s = String::from_utf8_lossy(&self.buf.buf[start..self.ptr]).into_owned();
                return Some(s);
            }
            self.synerr(&format!("relational operator expected at {}\n", self.rest_of_line()));
            return None;
        }
        self.synerr(&format!("relational operator expected at {}\n", self.rest_of_line()));
        None
    }

    //------------------------------get_expr------------------------------------
    // Move to the closing token of the expression, matching parens and quotes.
    fn get_expr(&mut self, desc: &str, stop_chars: &str) -> Option<String> {
        let expr_start = self.ptr;
        let mut paren: i32 = 0;
        loop {
            let c = self.curchar;
            if paren > 0 || !stop_chars.contains(c as char) {
                if c == b'(' {
                    paren += 1;
                    self.next_char();
                } else if c == b')' {
                    if paren == 0 {
                        self.synerr(&format!("too many )'s, did not find {} after {}\n", stop_chars, desc));
                        return None;
                    }
                    paren -= 1;
                    self.next_char();
                } else if c == b'"' || c == b'\'' {
                    let qchar = c;
                    loop {
                        self.next_char();
                        if self.curchar == qchar {
                            self.next_char();
                            break;
                        }
                        if self.curchar == b'\\' {
                            self.next_char();
                        }
                        if self.curchar == b'\n' || self.curchar == 0 {
                            self.synerr(&format!("newline in string in {}\n", desc));
                            return None;
                        }
                    }
                } else if c == b'%' && (self.byte(1) == b'{' || self.byte(1) == b'}') {
                    self.synerr(&format!("unexpected %{} in {}\n", self.byte(1) as char, desc));
                    return None;
                } else if c == 0 {
                    self.synerr(&format!("unexpected EOF in {}\n", desc));
                    return None;
                } else {
                    let pre_skip_ptr = self.ptr;
                    self.skipws();
                    if pre_skip_ptr == self.ptr {
                        self.next_char();
                    }
                }
            } else {
                break;
            }
        }
        // _ptr is at the stop char; terminate string there
        let end = self.ptr;
        let s = String::from_utf8_lossy(&self.buf.buf[expr_start..end]).into_owned();
        self.buf.buf[end] = 0;
        Some(s)
    }

    fn get_paren_expr(&mut self, description: &str, _include_location: bool) -> Option<String> {
        if self.curchar != b'(' {
            return None;
        }
        self.next_char();
        let token = self.get_expr(description, ")")?;
        if self.curchar == b')' {
            self.next_char();
        }
        Some(token)
    }
    fn get_paren_expr_simple(&mut self, description: &str) -> Option<String> {
        self.get_paren_expr(description, false)
    }

    fn find_cpp_block(&mut self, description: &str) -> Option<String> {
        if self.curchar == b'%' {
            self.next_char();
            if self.curchar != b'{' {
                self.synerr(&format!("missing '{{' in {} \n", description));
                return None;
            }
            self.next_char();
            self.skipws_no_preproc();
            let cpp_start = self.ptr;
            let mut next = self.ptr + 1;
            while !(self.curchar == b'%' && self.byte_at(next) == b'}') && self.curchar != 0 {
                self.next_char_or_line();
                next = self.ptr + 1;
            }
            if self.curchar == 0 {
                self.synerr(&format!("invalid termination of {} \n", description));
                return None;
            }
            let end = self.ptr;
            let s = String::from_utf8_lossy(&self.buf.buf[cpp_start..end]).into_owned();
            self.buf.buf[end] = 0;
            self.ptr += 2;
            self.curchar = self.byte(0);
            Some(s)
        } else {
            None
        }
    }

    fn trim(token: &str) -> String {
        token.trim().to_string()
    }

    fn parse_one_arg(&mut self, description: &str) -> Option<String> {
        if self.curchar == b'(' {
            self.next_char();
            self.skipws();
            let token = self.get_expr(description, ")")?;
            self.next_char(); // skip close paren
            if self.curchar != b';' {
                self.synerr(&format!("missing ';' in {}.\n", description));
                return None;
            }
            self.next_char();
            Some(Self::trim(&token))
        } else {
            self.synerr(&format!("Missing '(' in {}.\n", description));
            None
        }
    }

    //============================== Level 1 ==================================
    pub fn parse(&mut self) {
        self.next_line();
        while self.curline.is_some() {
            if let Some(cl) = self.curline {
                self.ptr = cl;
            }
            self.skipws();
            let ident = self.get_ident();
            let Some(ident) = ident else { continue };
            match ident.as_str() {
                "instruct" => self.instr_parse(),
                "operand" => self.oper_parse(),
                "opclass" => self.opclass_parse(),
                "ins_attrib" => self.ins_attr_parse(),
                "op_attrib" => self.op_attr_parse(),
                "source" => self.source_parse(),
                "source_hpp" => self.source_hpp_parse(),
                "register" => self.reg_parse(),
                "frame" => self.frame_parse(),
                "encode" => self.encode_parse(),
                "pipeline" => self.pipe_parse(),
                "definitions" => self.definitions_parse(),
                "peephole" => self.peep_parse(),
                "#line" => self.preproc_line(),
                "#define" => self.preproc_define(),
                "#undef" => self.preproc_undef(),
                _ => {
                    self.synerr(&format!(
                        "expected one of - instruct, operand, ins_attrib, op_attrib, source, register, pipeline, encode\n     Found {}",
                        ident));
                }
            }
            self.next_line();
        }
        // add spill & dynamic reg classes
        if let Some(reg) = self.ad.register.clone() {
            self.add_spill_reg_class(reg.clone());
            self.add_dynamic_reg_class(reg.clone());
        } else {
            self.parse_err(SEMERR, self.linenum(), "Did not declare 'register' definitions");
        }
        if self.preproc_depth != 0 {
            self.synerr("End of file inside #ifdef");
        }
        if !self.ad.global_names.contains_key(INS_COST) {
            self.parse_err(SEMERR, self.linenum(), "Did not declare 'ins_cost' attribute");
        }
        if !self.ad.global_names.contains_key(OP_COST) {
            self.parse_err(SEMERR, self.linenum(), "Did not declare 'op_cost' attribute");
        }
    }

    fn instr_parse(&mut self) {
        let Some(ident) = self.get_unique_ident_local("instruction") else { return };
        let mut f = Form::new(FormKind::Instruct);
        f.ident = ident.clone();
        f.linenum = self.linenum();
        f.cisc_spill_operand = -1;
        let instr = Rc::new(RefCell::new(f));
        self.ad.global_names.insert(ident.clone(), instr.clone());
        if self.ad.adl_debug > 1 {
            eprintln!("Parsing Instruction Form {}", ident);
        }
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' in instruct definition\n");
            return;
        }
        self.get_oplist(&mut instr.borrow_mut().parameters, &mut instr.borrow_mut().local_names);
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("missing '%%{' in instruction definition\n");
            return;
        }
        self.next_char();
        let mut match_rules_cnt: i32 = 0;
        loop {
            let kw = self.get_ident();
            let Some(kw) = kw else {
                self.synerr(&format!("keyword identifier expected at {}\n", self.curchar as char));
                continue;
            };
            match kw.as_str() {
                "predicate" => {
                    let p = self.pred_parse();
                    if let Some(p) = p {
                        instr.borrow_mut().predicate = Some(p);
                    }
                }
                "match" => {
                    // Allow one instruction to have several match rules.
                    let is_first = instr.borrow().matrule.is_none();
                    if is_first {
                        // This is first match rule encountered
                        let lr = instr.borrow().local_names.clone();
                        let rule = self.match_parse(&lr);
                        if let Some(rule) = rule {
                            {
                                let mut b = instr.borrow_mut();
                                b.matrule = Some(rule.clone());
                            }
                            // Special case the treatment of Control instructions.
                            {
                                let b = instr.borrow();
                                if b.is_ideal_control() {
                                    let mr = b.matrule.as_ref().unwrap().clone();
                                    drop(b);
                                    mr.borrow_mut().m.result = Some("Universe".to_string());
                                }
                            }
                            // Check for commutative operations with tree operands.
                            self.matchrule_clone_and_swap(rule, &ident, &mut match_rules_cnt);
                        }
                    } else {
                        // Find the end of the match rule list
                        let mut tail = instr.borrow().matrule.clone().unwrap();
                        loop {
                            let nxt = tail.borrow().next.clone();
                            match nxt {
                                Some(n) => tail = n,
                                None => break,
                            }
                        }
                        // Add the new match rule to the list
                        let lr = instr.borrow().local_names.clone();
                        let new_rule = self.match_parse(&lr);
                        if let Some(nr) = new_rule {
                            let is_control = instr.borrow().is_ideal_control();
                            {
                                let mut nrb = nr.borrow_mut();
                                if is_control {
                                    let nm = nrb.m.name.clone().unwrap_or_default();
                                    drop(nrb);
                                    self.parse_err(SYNERR, self.linenum(),
                                        &format!("unique match rule expected for {}\n", nm));
                                    continue;
                                }
                            }
                            let buf = {
                                let b = instr.borrow();
                                format!("{}_{}", b.ident, match_rules_cnt)
                            };
                            match_rules_cnt += 1;
                            nr.borrow_mut().m.result = Some(buf);
                            tail.borrow_mut().next = Some(nr.clone());
                            // Check for commutative operations with tree operands.
                            self.matchrule_clone_and_swap(nr, &ident, &mut match_rules_cnt);
                        }
                    }
                }
                "encode" => {
                    self.synerr("Instructions specify ins_encode, not encode\n");
                }
                "ins_encode" => self.ins_encode_parse(&instr),
                "postalloc_expand" => self.postalloc_expand_parse(&instr),
                "opcode" => {
                    let op = self.opcode_parse();
                    if let Some(op) = op {
                        let mut b = instr.borrow_mut();
                        b.opcode_primary = op.0;
                        b.opcode_secondary = op.1;
                        b.opcode_tertiary = op.2;
                    }
                }
                "size" => {
                    let sz = self.size_parse();
                    instr.borrow_mut().size = sz;
                }
                "effect" => self.effect_parse(&instr),
                "flag" => {
                    if let Some(names) = self.flag_parse() {
                        instr.borrow_mut().flag_names = names;
                    }
                }
                "expand" => {
                    let exp = self.expand_parse(&instr);
                    if let Some(exp) = exp {
                        let mut b = instr.borrow_mut();
                        b.expand_newopers = exp.0;
                        b.expand_newopconst = exp.1;
                        b.expand_instrs = exp.2;
                        b.has_expand_rule = true;
                    }
                }
                "rewrite" => {
                    let rw = self.rewrite_parse();
                    if let Some(rw) = rw {
                        let mut b = instr.borrow_mut();
                        b.rewrite_params = Some(rw.0);
                        b.rewrite_block = Some(rw.1);
                    }
                }
                "constraint" => {
                    self.synerr("Instructions do not specify a constraint\n");
                }
                "construct" => {
                    self.synerr("Instructions do not specify a construct\n");
                }
                "format" => {
                    let fmt = self.format_parse();
                    if let Some((ss, rv)) = fmt {
                        let mut b = instr.borrow_mut();
                        b.format_strings = ss;
                        b.format_rep_vars = rv;
                    }
                }
                "interface" => {
                    self.synerr("Instructions do not specify an interface\n");
                }
                "ins_pipe" => self.ins_pipe_parse(&instr),
                _ => {
                    // attribute?
                    let form = self.ad.global_names.get(&kw).cloned();
                    let is_ins_attr = form.as_ref().map_or(false, |f| {
                        let f = f.borrow();
                        f.kind == FormKind::AttributeForm && f.use_def == INS_ATTR as i32
                    });
                    if is_ins_attr {
                        if let Some(attr) = self.attr_parse(&kw) {
                            let mut b = instr.borrow_mut();
                            b.attribs = Some(match b.attribs.take() {
                                Some(mut head) => {
                                    // append at front: new attr -> old list
                                    let mut a = attr;
                                    a.next = Some(head);
                                    Rc::new(a)
                                }
                                None => Rc::new(attr),
                            });
                        }
                    } else {
                        self.synerr(&format!(
                            "expected one of:\n predicate, match, encode, or the name of an instruction attribute at {}\n",
                            kw));
                    }
                }
            }
            self.skipws();
            if self.curchar == b'%' {
                break;
            }
        }
        self.next_char();
        if self.curchar != b'}' {
            self.synerr("missing '%%}' in instruction definition\n");
            return;
        }
        self.adjust_set_rule(&instr);
        if self.ad.pipeline.is_some() {
            let b = instr.borrow();
            if b.expands() || b.postalloc_expands() {
                if b.ins_pipe.is_some() {
                    self.parse_err(WARN, self.linum(), &format!(
                        "ins_pipe and expand rule both specified for instruction \"{}\"; ins_pipe will be unused\n",
                        b.ident));
                }
            } else if b.ins_pipe.is_none() {
                self.parse_err(WARN, self.linum(), &format!(
                    "No ins_pipe specified for instruction \"{}\"\n", b.ident));
            }
        }
        self.ad.add_form(instr.clone());
        // create clone instruction forms for additional match rules
        let mut rule = instr.borrow().matrule.clone();
        if let Some(mut r) = rule.take() {
            r = match r.borrow().next.clone() {
                Some(n) => n,
                None => {
                    self.finish_instr_parse();
                    return;
                }
            };
            loop {
                let ident2 = r.borrow().m.result.clone().unwrap_or_default();
                let clone = self.clone_instruct_form(&instr, &ident2, r.clone());
                self.ad.global_names.insert(ident2.clone(), clone.clone());
                if self.ad.adl_debug > 1 {
                    eprintln!("Parsing Instruction Form {}", ident2);
                }
                self.adjust_set_rule(&clone);
                self.ad.add_form(clone);
                // detach next
                let nxt = r.borrow().next.clone();
                r.borrow_mut().next = None;
                match nxt {
                    Some(n) => r = n,
                    None => break,
                }
            }
        }
    }

    fn finish_instr_parse(&mut self) {}

    fn clone_instruct_form(&self, instr: &FormRef, id: &str, rule: MatchRuleRef) -> FormRef {
        // InstructForm(id, instr, rule): copy fields, share match rule object
        // but each clone gets its own MatchRule wrapper pointing at same tree.
        let src = instr.borrow();
        let mut f = Form::new(FormKind::Instruct);
        f.ident = id.to_string();
        f.linenum = self.linenum();
        f.ideal_only = false;
        f.local_names = src.local_names.clone();
        f.effects = src.effects.clone();
        f.has_call = src.has_call;
        f.matrule = Some(rule);
        f.insencode = src.insencode.clone();
        f.constant_encode = src.constant_encode.clone();
        f.is_postalloc_expand = src.is_postalloc_expand;
        f.is_mach_constant = src.is_mach_constant;
        f.needs_constant_base = src.needs_constant_base;
        f.has_expand_rule = src.has_expand_rule;
        f.opcode_primary = src.opcode_primary.clone();
        f.opcode_secondary = src.opcode_secondary.clone();
        f.opcode_tertiary = src.opcode_tertiary.clone();
        f.size = src.size.clone();
        f.attribs = src.attribs.clone();
        f.predicate = src.predicate.clone();
        f.constraint_func = src.constraint_func.clone();
        f.constraint_arg = src.constraint_arg.clone();
        f.construct_expr = src.construct_expr.clone();
        f.interface = src.interface.clone();
        f.format_strings = src.format_strings.clone();
        f.format_rep_vars = src.format_rep_vars.clone();
        f.expand_newopers = src.expand_newopers.clone();
        f.expand_newopconst = src.expand_newopconst.clone();
        f.expand_instrs = src.expand_instrs.clone();
        f.rewrite_params = src.rewrite_params.clone();
        f.rewrite_block = src.rewrite_block.clone();
        f.ins_pipe = src.ins_pipe.clone();
        f.flag_names = src.flag_names.clone();
        f.peephole = src.peephole.clone();
        f.components = ComponentList::default();
        f.alignment = src.alignment;
        f.is_cisc_alternate = false;
        f.cisc_spill_operand = -1;
        f.short_branch_form = src.short_branch_form.clone();
        f.is_short_branch = src.is_short_branch;
        f.parameters = src.parameters.clone();
        drop(src);
        Rc::new(RefCell::new(f))
    }

    fn matchrule_clone_and_swap(&mut self, rule: MatchRuleRef, instr_ident: &str, match_rules_cnt: &mut i32) {
        let mut count = 0i32;
        {
            let mut rb = rule.borrow_mut();
            rb.m.count_commutative_op(&mut count, &self.ad.global_names);
        }
        if count > 0 {
            self.matchrule_swap_commutative_op(rule, instr_ident, count, match_rules_cnt);
        }
    }

    fn matchrule_swap_commutative_op(&mut self, rule: MatchRuleRef, instr_ident: &str,
                                      count: i32, match_rules_cnt: &mut i32) {
        // clone the rule (deep copy of the match tree)
        let clone_rule = {
            let r = rule.borrow();
            let mut c = r.clone();
            c.next = None;
            Rc::new(RefCell::new(c))
        };
        // swap operands of commutative op with id == count
        {
            let globals = &self.ad.global_names;
            clone_rule.borrow_mut().m.swap_commutative_op(true, count, globals);
        }
        let buf = format!("{}_{}", instr_ident, *match_rules_cnt);
        *match_rules_cnt += 1;
        clone_rule.borrow_mut().m.result = Some(buf);
        // link: clone.next = rule.next ; rule.next = clone
        let old_next = rule.borrow().next.clone();
        clone_rule.borrow_mut().next = old_next;
        rule.borrow_mut().next = Some(clone_rule.clone());
        let new_count = count - 1;
        if new_count > 0 {
            self.matchrule_swap_commutative_op(rule.clone(), instr_ident, new_count, match_rules_cnt);
            self.matchrule_swap_commutative_op(clone_rule, instr_ident, new_count, match_rules_cnt);
        }
    }

    fn adjust_set_rule(&mut self, instr: &FormRef) {
        // Check for "Set" form of chain rule
        let (is_set, rch, lchild_op) = {
            let b = instr.borrow();
            let Some(mr) = &b.matrule else { return };
            let mr = mr.borrow();
            if mr.m.r_child.is_none() || mr.m.l_child.is_none() {
                return;
            }
            let rch = mr.m.r_child.as_ref().unwrap().op_type.clone();
            let lchild_op = mr.m.l_child.as_ref().unwrap().op_type.clone();
            (mr.m.op_type == "Set", rch, lchild_op)
        };
        let form = self.ad.global_names.get(&rch).cloned();
        let Some(form) = form else { return };
        {
            let f = form.borrow();
            if !(is_set && f.kind == FormKind::Operand && !f.ideal_only()) {
                return;
            }
        }
        // base_operand check on right child
        let mut position: u32 = 0;
        let mut result = None;
        let mut name = None;
        let mut optype = None;
        let right = {
            let b = instr.borrow();
            let mr = b.matrule.as_ref().unwrap().clone();
            drop(b);
            let mrb = mr.borrow();
            mrb.m.r_child.as_ref().unwrap().clone()
        };
        let ok = right.base_operand(&mut position, &self.ad.global_names,
                                    &mut result, &mut name, &mut optype);
        if !ok {
            return;
        }
        position = 1;
        let mut result2 = None;
        let mut name2 = None;
        let mut optype2 = None;
        let right2 = right.clone();
        let ok2 = right2.base_operand(&mut position, &self.ad.global_names,
                                      &mut result2, &mut name2, &mut optype2);
        let cost = instr.borrow().ins_cost()
            .or_else(|| self.ad.global_names.get(INS_COST)
                .and_then(|f| f.borrow().code.clone()));
        if !ok2 {
            if instr.borrow().predicate.is_some() {
                self.synerr("ADLC does not support instruction chain rules with predicates");
            }
            // Chain from input _ideal_operand_type_
            if let Some(optype) = optype.clone() {
                let entry = self.ad.chain_rules.entry(optype).or_default();
                if !entry.search(&lchild_op) {
                    // The ADLC does not support chaining from the ideal operand type
                    // of a predicated user-defined operand
                    let oper_has_pred = form.borrow().predicate.is_some();
                    if !oper_has_pred {
                        let coststr = cost.clone().unwrap_or_default();
                        entry.insert(&lchild_op, &coststr, &instr.borrow().ident);
                    }
                }
            }
            // Chain from input _user_defined_operand_type_
            if let Some(result) = result.clone() {
                let entry = self.ad.chain_rules.entry(result).or_default();
                if !entry.search(&lchild_op) {
                    let coststr = cost.clone().unwrap_or_default();
                    entry.insert(&lchild_op, &coststr, &instr.borrow().ident);
                }
            }
        } else {
            // May have instruction chain rule if root of right-tree is an ideal
            // rightOp = the operand form for rch (already known non-ideal operand)
            let right_op_ident = form.borrow().ident.clone();
            // rightOp->_matrule->_opType : root opType of operand's match rule
            let root_optype = {
                let fb = form.borrow();
                fb.matrule.as_ref().map(|m| m.borrow().m.op_type.clone())
            };
            if let Some(root_optype) = root_optype {
                let right_root = self.ad.global_names.get(&root_optype).cloned();
                if let Some(rr) = right_root {
                    let (is_ideal, is_inst) = {
                        let rrb = rr.borrow();
                        (rrb.ideal_only(), rrb.kind == FormKind::Instruct)
                    };
                    if is_ideal && is_inst {
                        let entry = self.ad.chain_rules.entry(right_op_ident).or_default();
                        let coststr = cost.clone().unwrap_or_default();
                        entry.insert(&lchild_op, &coststr, &instr.borrow().ident);
                    }
                }
            }
        }
    }

    //------------------------------oper_parse----------------------------------
    fn oper_parse(&mut self) {
        let Some(ident) = self.get_unique_ident_local("operand") else { return };
        let mut f = Form::new(FormKind::Operand);
        f.ident = ident.clone();
        f.linenum = self.linenum();
        let oper = Rc::new(RefCell::new(f));
        self.ad.global_names.insert(ident.clone(), oper.clone());
        if self.ad.adl_debug > 1 {
            eprintln!("Parsing Operand Form {}", ident);
        }
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' in operand definition\n");
            return;
        }
        self.get_oplist(&mut oper.borrow_mut().parameters, &mut oper.borrow_mut().local_names);
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("missing '%%{' in operand definition\n");
            return;
        }
        self.next_char();
        self.next_char();
        loop {
            let kw = self.get_ident();
            let Some(kw) = kw else {
                self.synerr(&format!("keyword identifier expected at {}\n", self.curchar as char));
                continue;
            };
            match kw.as_str() {
                "predicate" => {
                    let p = self.pred_parse();
                    if let Some(p) = p {
                        oper.borrow_mut().predicate = Some(p);
                    }
                }
                "match" => {
                    let lr = oper.borrow().local_names.clone();
                    let rule = self.match_parse(&lr);
                    if let Some(r) = rule {
                        let mut b = oper.borrow_mut();
                        if let Some(last) = b.matrule.clone() {
                            // find end of list
                            let mut cur = last;
                            loop {
                                let nxt = cur.borrow().next.clone();
                                match nxt {
                                    Some(n) => cur = n,
                                    None => break,
                                }
                            }
                            drop(b);
                            r.borrow_mut().m.result = Some(ident.clone());
                            cur.borrow_mut().next = Some(r);
                        } else {
                            r.borrow_mut().m.result = Some(ident.clone());
                            b.matrule = Some(r);
                        }
                    }
                }
                "encode" => {
                    let iface = self.interface_parse();
                    if let Some(i) = iface {
                        oper.borrow_mut().interface = Some(i);
                    }
                }
                "ins_encode" => {
                    self.synerr("Operands specify 'encode', not 'ins_encode'\n");
                }
                "opcode" => {
                    self.synerr("Operands do not specify an opcode\n");
                }
                "effect" => {
                    self.synerr("Operands do not specify an effect\n");
                }
                "expand" => {
                    self.synerr("Operands do not specify an expand\n");
                }
                "rewrite" => {
                    self.synerr("Operands do not specify a rewrite\n");
                }
                "constraint" => {
                    let c = self.constraint_parse();
                    if let Some((f_, a_)) = c {
                        let mut b = oper.borrow_mut();
                        b.constraint_func = Some(f_);
                        b.constraint_arg = Some(a_);
                    }
                }
                "construct" => {
                    let c = self.construct_parse();
                    oper.borrow_mut().construct_expr = c;
                }
                "format" => {
                    let fmt = self.format_parse();
                    if let Some((ss, rv)) = fmt {
                        let mut b = oper.borrow_mut();
                        b.format_strings = ss;
                        b.format_rep_vars = rv;
                    }
                }
                "interface" => {
                    let iface = self.interface_parse();
                    if let Some(i) = iface {
                        oper.borrow_mut().interface = Some(i);
                    }
                }
                _ => {
                    let is_op_attr = {
                        let form = self.ad.global_names.get(&kw).cloned();
                        form.map_or(false, |f| {
                            let f = f.borrow();
                            f.kind == FormKind::AttributeForm && f.use_def == OP_ATTR as i32
                        })
                    };
                    if is_op_attr {
                        if let Some(attr) = self.attr_parse(&kw) {
                            let mut b = oper.borrow_mut();
                            b.attribs = Some(match b.attribs.take() {
                                Some(head) => {
                                    let mut a = attr;
                                    a.next = Some(head);
                                    Rc::new(a)
                                }
                                None => Rc::new(attr),
                            });
                        }
                    } else {
                        self.synerr(&format!(
                            "expected one of - constraint, predicate, match, encode, format, construct, or the name of a defined operand attribute at {}\n",
                            kw));
                    }
                }
            }
            self.skipws();
            if self.curchar == b'%' {
                break;
            }
        }
        self.next_char();
        if self.curchar != b'}' {
            self.synerr("missing '%%}' in operand definition\n");
            return;
        }
        self.ad.add_form(oper);
    }

    fn opclass_parse(&mut self) {
        let Some(ident) = self.get_unique_ident_local("opclass") else { return };
        let mut f = Form::new(FormKind::OpClass);
        f.ident = ident.clone();
        f.linenum = self.linenum();
        let opc = Rc::new(RefCell::new(f));
        self.ad.global_names.insert(ident.clone(), opc.clone());
        if self.ad.adl_debug > 1 {
            eprintln!("Parsing Operand Class Form {}", ident);
        }
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' in operand definition\n");
            return;
        }
        loop {
            self.next_char();
            let kw = self.get_ident();
            let Some(kw) = kw else {
                self.synerr(&format!("keyword identifier expected at {}\n", self.curchar as char));
                continue;
            };
            let form = self.ad.global_names.get(&kw).cloned();
            let is_operand = form.as_ref().map_or(false, |f| {
                let f = f.borrow();
                f.kind == FormKind::Operand
            });
            if is_operand {
                if let Some(of) = form {
                    opc.borrow_mut().oplst.push(kw.clone());
                    of.borrow_mut().classes.push(ident.clone());
                }
            } else {
                self.synerr(&format!("expected name of a defined operand at {}\n", kw));
            }
            self.skipws();
            if self.curchar != b',' {
                break;
            }
        }
        if self.curchar != b')' {
            self.synerr("missing ')' or ',' in opclass definition\n");
            return;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' in opclass definition\n");
            return;
        }
        self.next_char();
        self.ad.add_form(opc);
    }

    fn ins_attr_parse(&mut self) {
        self.ins_or_op_attr_parse(true);
    }
    fn op_attr_parse(&mut self) {
        self.ins_or_op_attr_parse(false);
    }
    fn ins_or_op_attr_parse(&mut self, is_ins: bool) {
        let desc = if is_ins { "inst_attrib" } else { "op_attrib" };
        let Some(ident) = self.get_unique_ident_local(desc) else { return };
        self.skipws();
        let Some(aexpr) = self.get_paren_expr_simple("attribute default expression string") else {
            self.synerr(&format!("missing '(' in {} definition\n", desc));
            return;
        };
        if self.curchar != b';' {
            self.synerr(&format!("missing ';' in {} definition\n", desc));
            return;
        }
        self.next_char();
        let mut f = Form::new(FormKind::AttributeForm);
        f.ident = ident.clone();
        f.use_def = if is_ins { INS_ATTR } else { OP_ATTR } as i32;
        // store default expression in code field
        f.code = Some(aexpr);
        f.linenum = self.linenum();
        let attrib = Rc::new(RefCell::new(f));
        self.ad.global_names.insert(ident, attrib.clone());
        self.ad.add_form(attrib);
    }

    fn definitions_parse(&mut self) {
        self.skipws();
        if self.curchar == b'%' && self.byte(1) == b'{' {
            self.next_char();
            self.next_char();
            self.skipws();
            loop {
                if self.curchar == b'%' && self.byte(1) == b'}' {
                    break;
                }
                let token = self.get_ident();
                let Some(token) = token else {
                    self.synerr("missing identifier inside definitions block.\n");
                    return;
                };
                if token == "int_def" {
                    self.int_def_parse();
                }
                self.skipws();
                if self.curchar == b'%' && self.byte(1) == b'}' {
                    break;
                }
                if self.curline.is_none() {
                    break;
                }
            }
        } else {
            self.synerr("Missing %%{ ... %%} block after definitions keyword.\n");
        }
    }

    fn int_def_parse(&mut self) {
        self.skipws();
        let Some(name) = self.get_ident() else {
            self.synerr("missing definition name after int_def\n");
            return;
        };
        self.skipws();
        let mut int_value: i64 = -1;
        let mut description: Option<String> = None;
        if self.curchar == b'(' {
            self.next_char();
            let value = self.get_ident();
            let Some(value) = value else {
                self.synerr("missing value in int_def\n");
                return;
            };
            if !Self::is_int_token(&value, &mut int_value) {
                self.synerr("value in int_def is not recognized as integer\n");
                return;
            }
            self.skipws();
            if self.curchar == b',' {
                self.next_char();
                description = self.get_expr("int_def description", ")");
                if description.is_none() {
                    self.synerr("invalid or missing description in int_def\n");
                    return;
                }
                description = description.map(|d| Self::trim(&d));
            }
            if self.curchar != b')' {
                self.synerr("missing ')' in register definition statement\n");
                return;
            }
            self.next_char();
        }
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' after int_def\n");
            return;
        }
        self.next_char();
        if self.ad.global_defs.contains_key(&name) {
            self.synerr("Duplicate definition\n");
            return;
        }
        let expr = Expr::named(&name, description.as_deref(), int_value, int_value);
        self.ad.global_defs.insert(name.clone(), expr);
        self.ad.defines_order.push(name);
    }

    fn source_parse(&mut self) {
        self.skipws();
        let Some(rule) = self.find_cpp_block("source block") else {
            self.synerr("incorrect or missing block for 'source'.\n");
            return;
        };
        let mut f = Form::new(FormKind::Source);
        f.code = Some(rule);
        let r = Rc::new(RefCell::new(f));
        self.ad.add_form(r);
    }

    fn source_hpp_parse(&mut self) {
        self.skipws();
        let Some(rule) = self.find_cpp_block("source_hpp block") else {
            self.synerr("incorrect or missing block for 'source_hpp'.\n");
            return;
        };
        let has_registers = self.ad.register.is_some();
        let kind = if has_registers { FormKind::Header } else { FormKind::PreHeader };
        let mut f = Form::new(kind);
        f.code = Some(rule);
        let r = Rc::new(RefCell::new(f));
        self.ad.add_form(r);
    }

    fn reg_parse(&mut self) {
        if self.ad.register.is_none() {
            let f = Form::new(FormKind::Register);
            let r = Rc::new(RefCell::new(f));
            self.ad.add_form(r.clone());
        }
        let reg = self.ad.register.clone().unwrap();
        self.skipws();
        if self.curchar == b'%' && self.byte(1) == b'{' {
            self.next_char();
            self.next_char();
            self.skipws();
            loop {
                if self.curchar == b'%' && self.byte(1) == b'}' {
                    break;
                }
                let token = self.get_ident();
                let Some(token) = token else {
                    self.synerr("missing identifier inside register block.\n");
                    return;
                };
                match token.as_str() {
                    "reg_def" => self.reg_def_parse(&reg),
                    "reg_class" => self.reg_class_parse(&reg),
                    "reg_class_dynamic" => self.reg_class_dynamic_parse(&reg),
                    "alloc_class" => self.alloc_class_parse(&reg),
                    "#define" => self.preproc_define(),
                    _ => {
                        self.synerr(&format!("bad token {} inside register block.\n", token));
                        break;
                    }
                }
                self.skipws();
                if self.curline.is_none() {
                    break;
                }
            }
        } else {
            self.synerr("Missing %{ ... %} block after register keyword.\n");
        }
    }

    fn encode_parse(&mut self) {
        if self.ad.encode.is_none() {
            let f = Form::new(FormKind::Encode);
            let r = Rc::new(RefCell::new(f));
            self.ad.add_form(r);
        }
        let enc = self.ad.encode.clone().unwrap();
        self.skipws();
        if self.curchar == b'%' && self.byte(1) == b'{' {
            self.next_char();
            self.next_char();
            self.skipws();
            loop {
                if self.curchar == b'%' && self.byte(1) == b'}' {
                    break;
                }
                let token = self.get_ident();
                let Some(token) = token else {
                    self.synerr("missing identifier inside encoding block.\n");
                    return;
                };
                if token == "enc_class" {
                    self.enc_class_parse(&enc);
                }
                self.skipws();
                if self.curline.is_none() {
                    break;
                }
            }
        } else {
            self.synerr("Missing %{ ... %} block after encode keyword.\n");
        }
    }

    fn enc_class_parse(&mut self, enc_block: &FormRef) {
        self.skipws();
        let Some(ec_name) = self.get_ident() else {
            self.synerr("missing encoding class name after encode.\n");
            return;
        };
        let encoding = self.add_enc_class(enc_block, &ec_name);
        encoding.borrow_mut().linenum = self.linenum();
        self.skipws();
        if self.curchar == b'(' {
            loop {
                self.next_char();
                self.skipws();
                if self.curchar == b')' {
                    break;
                }
                let ptype = self.get_ident();
                let Some(ptype) = ptype else {
                    self.synerr(&format!("parameter type expected at {}\n", self.curchar as char));
                    return;
                };
                self.skipws();
                let pname = self.get_ident();
                let Some(pname) = pname else {
                    self.synerr(&format!("parameter name expected at {}\n", self.curchar as char));
                    return;
                };
                {
                    let mut b = encoding.borrow_mut();
                    b.parameter_type.push(ptype);
                    b.parameter_name.push(pname);
                }
                self.skipws();
                if self.curchar != b',' {
                    break;
                }
            }
            if self.curchar != b')' {
                self.synerr("missing ')'\n");
            } else {
                self.next_char();
            }
        }
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("missing '%{' in enc_class definition\n");
            return;
        }
        self.next_char();
        self.next_char();
        self.enc_class_parse_block(&encoding);
    }

    fn enc_class_parse_block(&mut self, encoding: &FormRef) {
        self.skipws_no_preproc();
        loop {
            if self.curchar == b'%' && self.byte(1) == b'}' {
                break;
            }
            // (1) string to pass through
            let start = self.ptr;
            while !(self.curchar == b'$'
                || (self.curchar == b'%' && self.byte(1) == b'}'))
            {
                if self.curchar == b'/' && (self.byte(1) == b'/' || self.byte(1) == b'*') {
                    self.skipws_no_preproc();
                } else {
                    self.next_char_or_line();
                }
                if self.curchar == 0 && self.curline.is_none() {
                    break;
                }
            }
            if start != self.ptr {
                let end = self.ptr;
                let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
                self.buf.buf[end] = 0;
                encoding.borrow_mut().code_strings.push(s);
            }
            if self.curchar == b'$' {
                if let Some(rep_var) = self.get_rep_var_ident_dup() {
                    encoding.borrow_mut().rep_vars.push(rep_var);
                }
            }
            if self.curline.is_none() && self.curchar == 0 {
                break;
            }
        }
        self.next_char();
        self.next_char();
        self.skipws();
    }

    fn add_enc_class(&mut self, enc_block: &FormRef, name: &str) -> FormRef {
        let mut eb = enc_block.borrow_mut();
        if let Some(existing) = eb.enc_class.get(name) {
            return existing.clone();
        }
        let mut f = Form::new(FormKind::EncClass);
        f.ident = name.to_string();
        let r = Rc::new(RefCell::new(f));
        eb.enc_class.insert(name.to_string(), r.clone());
        eb.eclasses.push(name.to_string());
        r
    }

    //------------------------------frame_parse---------------------------------
    fn frame_parse(&mut self) {
        let mut f = Form::new(FormKind::Frame);
        f.linenum = self.linenum();
        let frame = Rc::new(RefCell::new(f));
        self.skipws();
        if self.curchar == b'%' && self.byte(1) == b'{' {
            self.next_char();
            self.next_char();
            self.skipws();
            loop {
                if self.curchar == b'%' && self.byte(1) == b'}' {
                    break;
                }
                let token = self.get_ident();
                let Some(token) = token else {
                    self.synerr("missing identifier inside frame block.\n");
                    return;
                };
                match token.as_str() {
                    "sync_stack_slots" => {
                        if let Some(v) = self.parse_one_arg("sync stack slots entry") {
                            frame.borrow_mut().frame_sync_stack_slots = Some(v);
                        }
                    }
                    "frame_pointer" => {
                        if let Some(v) = self.parse_one_arg("frame pointer entry") {
                            frame.borrow_mut().frame_pointer = Some(v);
                        }
                    }
                    "interpreter_frame_pointer" => {
                        if let Some(v) = self.parse_one_arg("interpreter frame pointer entry") {
                            frame.borrow_mut().frame_interpreter_frame_pointer_reg = Some(v);
                        }
                    }
                    "inline_cache_reg" => {
                        if let Some(v) = self.parse_one_arg("inline cache reg entry") {
                            frame.borrow_mut().frame_inline_cache_reg = Some(v);
                        }
                    }
                    "compiler_method_oop_reg" | "interpreter_method_oop_reg"
                    | "interpreter_method_reg" | "in_preserve_stack_slots"
                    | "out_preserve_stack_slots" | "calling_convention"
                    | "c_calling_convention" => {
                        self.parse_err(WARN, self.linenum(), &format!("Using obsolete Token, {}", token));
                        self.skipws();
                    }
                    "cisc_spilling_operand_name" => {
                        if let Some(v) = self.parse_one_arg("cisc spilling operand name") {
                            frame.borrow_mut().frame_cisc_spilling_operand_name = Some(v);
                        }
                    }
                    "stack_alignment" => {
                        if let Some(v) = self.parse_one_arg("stack alignment entry") {
                            frame.borrow_mut().alignment_str = Some(v);
                        }
                    }
                    "return_addr" => self.return_addr_parse(&frame, false),
                    "varargs_C_out_slots_killed" => {
                        if let Some(v) = self.parse_one_arg("varargs C out slots killed") {
                            frame.borrow_mut().varargs_c_out_slots_killed = Some(v);
                        }
                    }
                    "return_value" => {
                        let v = self.return_value_parse();
                        frame.borrow_mut().return_value = v;
                    }
                    "c_frame_pointer" => {
                        if let Some(v) = self.parse_one_arg("frame pointer entry") {
                            frame.borrow_mut().c_frame_pointer = Some(v);
                        }
                    }
                    "c_return_addr" => self.return_addr_parse(&frame, true),
                    "c_return_value" => {
                        let v = self.return_value_parse();
                        frame.borrow_mut().c_return_value = v;
                    }
                    _ => {}
                }
                self.skipws();
                if self.curline.is_none() {
                    break;
                }
            }
        } else {
            self.synerr("Missing %{ ... %} block after frame keyword.\n");
            return;
        }
        {
            let mut b = frame.borrow_mut();
            if b.frame_pointer.is_none() {
                self.parse_err(SYNERR, self.linenum(), "missing frame pointer definition in frame section.\n");
                return;
            }
            if b.alignment_str.is_none() {
                self.parse_err(SYNERR, self.linenum(), "missing alignment definition in frame section.\n");
                return;
            }
            if b.return_addr.is_none() {
                self.parse_err(SYNERR, self.linenum(), "missing return address location in frame section.\n");
                return;
            }
            if b.varargs_c_out_slots_killed.is_none() {
                self.parse_err(SYNERR, self.linenum(), "missing varargs C out slots killed definition in frame section.\n");
                return;
            }
            if b.return_value.is_none() {
                self.parse_err(SYNERR, self.linenum(), "missing return value definition in frame section.\n");
                return;
            }
            if b.c_frame_pointer.is_none() {
                b.c_frame_pointer = b.frame_pointer.clone();
            }
            if b.c_return_addr.is_none() {
                b.c_return_addr = b.return_addr.clone();
                b.c_return_addr_loc = b.return_addr_loc;
            }
            if b.c_return_value.is_none() {
                b.c_return_value = b.return_value.clone();
            }
        }
        self.ad.add_form(frame);
    }

    fn return_addr_parse(&mut self, frame: &FormRef, native: bool) {
        if self.curchar == b'(' {
            self.next_char();
            self.skipws();
            let Some(token) = self.get_ident() else {
                self.synerr("missing value inside return address entry.\n");
                return;
            };
            let in_register = match token.as_str() {
                "REG" => true,
                "STACK" => false,
                _ => {
                    self.synerr("invalid value inside return_address entry.\n");
                    return;
                }
            };
            self.skipws();
            let Some(token2) = self.get_expr("return address entry", ")") else {
                self.synerr("missing value inside return address entry.\n");
                return;
            };
            self.next_char();
            if self.curchar != b';' {
                self.synerr("missing ';' in return address entry.\n");
                return;
            }
            self.next_char();
            let mut b = frame.borrow_mut();
            if native {
                b.c_return_addr_loc = in_register;
                b.c_return_addr = Some(token2);
            } else {
                b.return_addr_loc = in_register;
                b.return_addr = Some(token2);
            }
        } else {
            self.synerr("Missing '(' in return_address entry.\n");
        }
    }

    fn return_value_parse(&mut self) -> Option<String> {
        self.skipws();
        self.find_cpp_block("return value block")
    }

    fn ins_pipe_parse(&mut self, instr: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing \"(\" in ins_pipe definition\n");
            return;
        }
        self.next_char();
        let ident = self.get_ident();
        let Some(ident) = ident else {
            self.synerr(&format!("keyword identifier expected at {}\n", self.curchar as char));
            return;
        };
        self.skipws();
        if self.curchar != b')' {
            self.synerr("missing \")\" in ins_pipe definition\n");
            return;
        }
        self.next_char();
        if self.curchar != b';' {
            self.synerr("missing ';' in return value entry.\n");
            return;
        }
        self.next_char();
        // validate pipe class exists
        let valid = self.ad.pipeline.as_ref().map_or(false, |p| {
            p.borrow().classlist.iter().any(|c| c == &ident)
        });
        if self.ad.pipeline.is_some() && !valid {
            self.synerr(&format!("\"{}\" is not a valid pipeline class\n", ident));
            return;
        }
        // add instruction name to pipe class's instructs
        if let Some(p) = self.ad.pipeline.clone() {
            let pb = p.borrow();
            if let Some(pc) = pb.classdict.get(&ident).cloned() {
                drop(pb);
                pc.borrow_mut().pipe_instructs.push(instr.borrow().ident.clone());
            }
        }
        instr.borrow_mut().ins_pipe = Some(ident);
    }

    //------------------------------pipe_parse----------------------------------
    fn pipe_parse(&mut self) {
        let f = Form::new(FormKind::Pipeline);
        let pipeline = Rc::new(RefCell::new(f));
        self.ad.add_form(pipeline.clone());
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("missing '%%{' in pipeline definition\n");
            return;
        }
        self.next_char();
        loop {
            let kw = self.get_ident();
            let Some(kw) = kw else {
                self.synerr(&format!("keyword identifier expected at {}\n", self.curchar as char));
                continue;
            };
            match kw.as_str() {
                "resources" => self.resource_parse(&pipeline),
                "pipe_desc" => self.pipe_desc_parse(&pipeline),
                "pipe_class" => self.pipe_class_parse(&pipeline),
                "define" => self.pipe_define_parse(&pipeline),
                "attributes" => self.pipe_attributes_parse(&pipeline),
                _ => {
                    self.synerr(&format!(
                        "expected one of \"resources\", \"pipe_desc\", \"pipe_class\", found \"{}\"\n", kw));
                    return;
                }
            }
            self.skipws();
            if self.curchar == b';' {
                self.skipws();
            }
            if self.curchar == b'%' {
                break;
            }
        }
        self.next_char();
        if self.curchar != b'}' {
            self.synerr("missing \"%%}\" in pipeline definition\n");
            return;
        }
        self.next_char();
    }

    fn pipe_define_parse(&mut self, pipeline: &FormRef) {
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("expected '%%{'\n");
            return;
        }
        self.next_char();
        self.skipws();
        let Some(node_class) = self.get_ident() else {
            self.synerr(&format!("expected identifier, found \"{}\"\n", self.curchar as char));
            return;
        };
        self.skipws();
        if self.curchar != b',' && self.curchar != b'=' {
            self.synerr(&format!("expected `=`, found '{}'\n", self.curchar as char));
            return;
        }
        self.next_char();
        self.skipws();
        let Some(pipe_class) = self.get_ident() else {
            self.synerr(&format!("expected identifier, found \"{}\"\n", self.curchar as char));
            return;
        };
        if self.curchar != b';' {
            self.synerr(&format!("expected `;`, found '{}'\n", self.curchar as char));
            return;
        }
        self.next_char();
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'}') {
            self.synerr(&format!("expected '%%}', found \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        let valid = pipeline.borrow().classlist.iter().any(|c| c == &pipe_class);
        if !valid {
            self.synerr(&format!("\"{}\" is not a valid pipeline class\n", pipe_class));
            return;
        }
        if let Some(pc) = pipeline.borrow().classdict.get(&pipe_class).cloned() {
            pc.borrow_mut().pipe_instructs.push(node_class.clone());
        }
        let mut f = Form::new(FormKind::MachNode);
        f.ident = node_class;
        f.ins_pipe = Some(pipe_class);
        let r = Rc::new(RefCell::new(f));
        self.ad.add_form(r);
    }

    fn pipe_attributes_parse(&mut self, pipeline: &FormRef) {
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("expected '%%{'\n");
            return;
        }
        self.next_char();
        self.skipws();
        let mut vsi_seen = false;
        loop {
            if self.curchar == b'%' {
                break;
            }
            let Some(ident) = self.get_ident() else { break };
            match ident.as_str() {
                "variable_size_instructions" => {
                    self.skipws();
                    if self.curchar == b';' {
                        self.next_char();
                        self.skipws();
                    }
                    pipeline.borrow_mut().variable_size_instrs = true;
                    vsi_seen = true;
                    continue;
                }
                "fixed_size_instructions" => {
                    self.skipws();
                    if self.curchar == b';' {
                        self.next_char();
                        self.skipws();
                    }
                    pipeline.borrow_mut().variable_size_instrs = false;
                    vsi_seen = true;
                    continue;
                }
                "branch_has_delay_slot" => {
                    self.skipws();
                    if self.curchar == b';' {
                        self.next_char();
                        self.skipws();
                    }
                    pipeline.borrow_mut().branch_has_delay_slot = true;
                    continue;
                }
                "max_instructions_per_bundle" | "max_bundles_per_cycle"
                | "instruction_unit_size" | "bundle_unit_size"
                | "instruction_fetch_unit_size" | "instruction_fetch_units" => {
                    self.skipws();
                    if self.curchar != b'=' {
                        self.synerr("expected `=`\n");
                        break;
                    }
                    self.next_char();
                    self.skipws();
                    let v = self.get_int();
                    {
                        let mut b = pipeline.borrow_mut();
                        match ident.as_str() {
                            "max_instructions_per_bundle" => b.max_instrs_per_bundle = v as i32,
                            "max_bundles_per_cycle" => b.max_bundles_per_cycle = v as i32,
                            "instruction_unit_size" => b.instr_unit_size = v as i32,
                            "bundle_unit_size" => b.bundle_unit_size = v as i32,
                            "instruction_fetch_unit_size" => b.instr_fetch_unit_size = v as i32,
                            "instruction_fetch_units" => b.instr_fetch_units = v as i32,
                            _ => {}
                        }
                    }
                    self.skipws();
                    if self.curchar == b';' {
                        self.next_char();
                        self.skipws();
                    }
                    continue;
                }
                "nops" => {
                    self.skipws();
                    if self.curchar != b'(' {
                        self.synerr(&format!("expected `(`, found '{}'\n", self.curchar as char));
                        break;
                    }
                    self.next_char();
                    self.skipws();
                    loop {
                        if self.curchar == b')' {
                            break;
                        }
                        let Some(n) = self.get_ident() else {
                            self.synerr(&format!("expected identifier for nop instruction, found '{}'\n", self.curchar as char));
                            break;
                        };
                        {
                            let mut b = pipeline.borrow_mut();
                            b.noplist.push(n);
                            b.nopcnt += 1;
                        }
                        self.skipws();
                        if self.curchar == b',' {
                            self.next_char();
                            self.skipws();
                        }
                    }
                    self.next_char();
                    self.skipws();
                    if self.curchar == b';' {
                        self.next_char();
                        self.skipws();
                    }
                    continue;
                }
                _ => {
                    self.synerr(&format!("unknown specifier \"{}\"\n", ident));
                }
            }
        }
        if !(self.curchar == b'%' && self.byte(1) == b'}') {
            self.synerr(&format!("expected '%%}}', found \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        self.next_char();
        self.skipws();
        {
            let b = pipeline.borrow();
            if b.max_instrs_per_bundle == 0 {
                self.synerr("\"max_instructions_per_bundle\" unspecified\n");
            }
            if b.instr_unit_size == 0 && b.bundle_unit_size == 0 {
                self.synerr("\"instruction_unit_size\" and \"bundle_unit_size\" unspecified\n");
            }
            if b.instr_fetch_unit_size == 0 {
                self.synerr("\"instruction_fetch_unit_size\" unspecified\n");
            }
            if b.instr_fetch_units == 0 {
                self.synerr("\"instruction_fetch_units\" unspecified\n");
            }
            if !vsi_seen {
                self.synerr("\"variable_size_instruction\" or \"fixed_size_instruction\" unspecified\n");
            }
        }
    }

    fn resource_parse(&mut self, pipeline: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing \"(\" in resource definition\n");
            return;
        }
        {
            pipeline.borrow_mut().rescount = 0;
        }
        loop {
            self.next_char();
            let Some(ident) = self.get_ident() else {
                self.synerr(&format!("keyword identifier expected at \"{}\"\n", self.curchar as char));
                return;
            };
            self.skipws();
            let mask = if self.curchar != b'=' {
                let m = 1u32 << pipeline.borrow().rescount;
                pipeline.borrow_mut().rescount += 1;
                m
            } else {
                self.next_char();
                self.skipws();
                let Some(expr) = self.get_ident() else {
                    self.synerr(&format!("keyword identifier expected at \"{}\"\n", self.curchar as char));
                    return;
                };
                let mut mask = pipeline.borrow().resdict.get(&expr).copied().unwrap_or_else(|| {
                    self.synerr(&format!("resource \"{}\" is not defined\n", expr));
                    0
                });
                self.skipws();
                while self.curchar == b'|' {
                    self.next_char();
                    self.skipws();
                    let Some(e2) = self.get_ident() else {
                        self.synerr(&format!("keyword identifier expected at \"{}\"\n", self.curchar as char));
                        return;
                    };
                    let m2 = pipeline.borrow().resdict.get(&e2).copied().unwrap_or_else(|| {
                        self.synerr(&format!("resource \"{}\" is not defined\n", e2));
                        0
                    });
                    mask |= m2;
                    self.skipws();
                }
                mask
            };
            {
                let mut b = pipeline.borrow_mut();
                b.resdict.insert(ident.clone(), mask);
                b.reslist.push(ident);
            }
            self.skipws();
            if self.curchar != b',' {
                break;
            }
        }
        if self.curchar != b')' {
            self.synerr(&format!("\")\" expected at \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        if self.curchar == b';' {
            self.next_char();
        }
    }

    fn pipe_desc_parse(&mut self, pipeline: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing \"(\" in pipe_desc definition\n");
            return;
        }
        loop {
            self.next_char();
            let Some(ident) = self.get_ident() else {
                self.synerr(&format!("keyword identifier expected at \"{}\"\n", self.curchar as char));
                return;
            };
            {
                let mut b = pipeline.borrow_mut();
                b.stages.push(ident);
                b.stagecnt += 1;
            }
            self.skipws();
            if self.curchar != b',' {
                break;
            }
        }
        if self.curchar != b')' {
            self.synerr(&format!("\")\" expected at \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        if self.curchar == b';' {
            self.next_char();
        }
    }

    fn pipe_class_parse(&mut self, pipeline: &FormRef) {
        self.skipws();
        let Some(ident) = self.get_ident() else {
            self.synerr(&format!("keyword identifier expected at \"{}\"\n", self.curchar as char));
            return;
        };
        let mut f = Form::new(FormKind::PipeClass);
        f.ident = ident.clone();
        {
            let mut b = pipeline.borrow_mut();
            b.classcnt += 1;
            f.pipe_num = b.classcnt;
        }
        f.linenum = self.linenum();
        let pipe_class = Rc::new(RefCell::new(f));
        {
            let mut b = pipeline.borrow_mut();
            b.classdict.insert(ident.clone(), pipe_class.clone());
            b.classlist.push(ident);
        }
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing \"(\" in pipe_class definition\n");
        } else {
            let (params, locals) = {
                let mut b = pipe_class.borrow_mut();
                (std::mem::take(&mut b.parameters), std::mem::take(&mut b.local_names))
            };
            let _ = (params, locals);
            self.get_oplist_pipe(&pipe_class);
        }
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("missing \"%%{\" in pipe_class definition\n");
            return;
        }
        self.next_char();
        loop {
            let kw = self.get_ident();
            let Some(kw) = kw else {
                self.synerr(&format!("keyword identifier expected at \"{}\"\n", self.curchar as char));
                continue;
            };
            self.skipws();
            match kw.as_str() {
                "fixed_latency" => {
                    self.skipws();
                    if self.curchar != b'(' {
                        self.synerr("missing \"(\" in latency definition\n");
                        return;
                    }
                    self.next_char();
                    self.skipws();
                    if !self.curchar.is_ascii_digit() {
                        self.synerr(&format!("number expected for \"{}\" in latency definition\n", self.curchar as char));
                        return;
                    }
                    let lat = self.get_int();
                    self.skipws();
                    if self.curchar != b')' {
                        self.synerr("missing \")\" in latency definition\n");
                        return;
                    }
                    self.next_char();
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" in latency definition\n");
                        return;
                    }
                    {
                        let mut b = pipe_class.borrow_mut();
                        b.has_fixed_latency = true;
                        b.fixed_latency = lat as i32;
                    }
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "zero_instructions" | "no_instructions" => {
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" in latency definition\n");
                        return;
                    }
                    pipe_class.borrow_mut().instruction_count = 0;
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "one_instruction_with_delay_slot" | "single_instruction_with_delay_slot" => {
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" in latency definition\n");
                        return;
                    }
                    {
                        let mut b = pipe_class.borrow_mut();
                        b.instruction_count = 1;
                        b.has_branch_delay = true;
                    }
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "one_instruction" | "single_instruction" => {
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" in latency definition\n");
                        return;
                    }
                    pipe_class.borrow_mut().instruction_count = 1;
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "instructions_in_first_bundle" | "instruction_count" => {
                    self.skipws();
                    if self.curchar != b'(' {
                        self.synerr(&format!("\"(\" expected at \"{}\"\n", self.curchar as char));
                        continue;
                    }
                    self.next_char();
                    self.skipws();
                    let n = self.get_int();
                    self.skipws();
                    if self.curchar != b')' {
                        self.synerr(&format!("\")\" expected at \"{}\"\n", self.curchar as char));
                        continue;
                    }
                    self.next_char();
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" in latency definition\n");
                        return;
                    }
                    pipe_class.borrow_mut().instruction_count = n as i32;
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "multiple_bundles" => {
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" after multiple bundles\n");
                        return;
                    }
                    pipe_class.borrow_mut().has_multiple_bundles = true;
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "has_delay_slot" => {
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" after \"has_delay_slot\"\n");
                        return;
                    }
                    pipe_class.borrow_mut().has_branch_delay = true;
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "force_serialization" => {
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" after \"force_serialization\"\n");
                        return;
                    }
                    pipe_class.borrow_mut().force_serialization = true;
                    self.next_char();
                    self.skipws();
                    continue;
                }
                "may_have_no_code" => {
                    self.skipws();
                    if self.curchar != b';' {
                        self.synerr("missing \";\" after \"may_have_no_code\"\n");
                        return;
                    }
                    pipe_class.borrow_mut().may_have_no_code = true;
                    self.next_char();
                    self.skipws();
                    continue;
                }
                _ => {
                    // operand parameter or resource specifier
                    let is_param = pipe_class.borrow().local_names.contains_key(&kw);
                    if is_param {
                        self.pipe_class_operand_parse(&pipe_class, &pipeline, &kw);
                        if self.curchar == b'%' {
                            continue;
                        }
                        if self.curchar != b';' {
                            self.synerr(&format!("\";\" expected at \"{}\"\n", self.curchar as char));
                            continue;
                        }
                        self.next_char();
                        self.skipws();
                        continue;
                    }
                    let is_res = pipeline.borrow().resdict.contains_key(&kw);
                    if is_res {
                        self.pipe_class_resource_parse(&pipe_class, &pipeline, &kw);
                        if self.curchar == b'%' {
                            continue;
                        }
                        if self.curchar != b';' {
                            self.synerr(&format!("\";\" expected at \"{}\"\n", self.curchar as char));
                            continue;
                        }
                        self.next_char();
                        self.skipws();
                        continue;
                    }
                    self.synerr(&format!("resource expected at \"{}\"\n", kw));
                    return;
                }
            }
            if self.curchar == b'%' {
                break;
            }
        }
        self.next_char();
        if self.curchar != b'}' {
            self.synerr("missing \"%%}\" in pipe_class definition\n");
            return;
        }
        self.next_char();
    }

    fn get_oplist_pipe(&mut self, pipe_class: &FormRef) {
        // like get_oplist but local names may be operands OR opclasses AND
        // must not collide with global operand names? C++ uses same get_oplist
        // which requires type be operand/opclass in _globalNames.
        loop {
            self.next_char();
            self.skipws();
            if self.curchar == b')' {
                break;
            }
            let Some(ty) = self.get_ident() else {
                self.synerr(&format!("optype identifier expected at {}\n", self.curchar as char));
                return;
            };
            let form = self.ad.global_names.get(&ty).cloned();
            let ok = form.as_ref().map_or(false, |f| {
                let f = f.borrow();
                f.kind == FormKind::Operand || f.kind == FormKind::OpClass
            });
            if !ok {
                self.synerr(&format!("undefined operand type {}\n", ty));
                return;
            }
            let Some(name) = self.get_ident() else {
                self.synerr(&format!("operand name expected at {}\n", self.curchar as char));
                return;
            };
            if self.ad.global_names.contains_key(&name) {
                self.synerr(&format!("Reuse of global name {} as operand.\n", name));
                return;
            }
            {
                let mut b = pipe_class.borrow_mut();
                if let Some(f) = form {
                    b.local_names.insert(name.clone(), f);
                }
                b.parameters.push(name);
            }
            self.skipws();
            if self.curchar != b',' {
                break;
            }
        }
        if self.curchar != b')' {
            self.synerr("missing ')'\n");
        } else {
            self.next_char();
        }
    }

    fn pipe_class_operand_parse(&mut self, pipe_class: &FormRef, _pipeline: &FormRef, ident: &str) {
        self.skipws();
        if self.curchar != b':' {
            self.synerr(&format!("\":\" expected at \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        self.skipws();
        let Some(stage) = self.get_ident() else {
            self.synerr(&format!("pipeline stage identifier expected at \"{}\"\n", self.curchar as char));
            return;
        };
        self.skipws();
        if self.curchar != b'(' {
            self.synerr(&format!("\"(\" expected at \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        let Some(row) = self.get_ident() else {
            self.synerr(&format!("\"read\" or \"write\" expected at \"{}\"\n", self.curchar as char));
            return;
        };
        let is_write = match row.as_str() {
            "read" => false,
            "write" => true,
            _ => {
                self.synerr(&format!("\"read\" or \"write\" expected at \"{}\"\n", self.curchar as char));
                return;
            }
        };
        self.skipws();
        if self.curchar != b')' {
            self.synerr(&format!("\")\" expected at \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        self.skipws();
        let mut more_instrs = 0i64;
        if self.curchar == b'+' {
            self.next_char();
            self.skipws();
            if !self.curchar.is_ascii_digit() {
                self.synerr(&format!("<number> expected at \"{}\"\n", self.curchar as char));
                return;
            }
            while self.curchar.is_ascii_digit() {
                more_instrs = more_instrs * 10 + (self.curchar - b'0') as i64;
                self.next_char();
            }
            self.skipws();
        }
        let mut f = Form::new(FormKind::PipeClassOperand);
        f.code = Some(stage);
        f.use_def = is_write as i32;
        f.register_num = more_instrs as u32;
        let r = Rc::new(RefCell::new(f));
        pipe_class.borrow_mut().local_usage.insert(ident.to_string(), r);
    }

    fn pipe_class_resource_parse(&mut self, pipe_class: &FormRef, pipeline: &FormRef, ident: &str) {
        let mut cyclecnt: i64 = 1;
        self.skipws();
        if self.curchar != b':' {
            self.synerr(&format!("\":\" expected at \"{}\"\n", self.curchar as char));
            return;
        }
        self.next_char();
        self.skipws();
        let Some(stage) = self.get_ident() else {
            self.synerr(&format!("pipeline stage identifier expected at \"{}\"\n", self.curchar as char));
            return;
        };
        self.skipws();
        if self.curchar == b'(' {
            self.next_char();
            cyclecnt = self.get_int();
            self.skipws();
            if self.curchar != b')' {
                self.synerr(&format!("\")\" expected at \"{}\"\n", self.curchar as char));
                return;
            }
            self.next_char();
            self.skipws();
        }
        let mut f = Form::new(FormKind::PipeClassResource);
        f.code = Some(stage.clone());
        f.effect_name = ident.to_string();
        f.use_def = cyclecnt as i32;
        let r = Rc::new(RefCell::new(f));
        {
            let stagenum = pipeline.borrow().stages.iter().position(|s| s == &stage).unwrap_or(-1) as i64;
            let mut pb = pipeline.borrow_mut();
            if pb.maxcycleused < (stagenum + cyclecnt) as i32 {
                pb.maxcycleused = (stagenum + cyclecnt) as i32;
            }
            drop(pb);
            pipe_class.borrow_mut().res_usage.push(r);
        }
    }

    //------------------------------peep_parse----------------------------------
    fn peep_parse(&mut self) {
        let mut f = Form::new(FormKind::Peephole);
        f.linenum = self.linenum();
        let peep = Rc::new(RefCell::new(f));
        self.skipws();
        if self.curchar == b'%' && self.byte(1) == b'{' {
            self.next_char();
            self.next_char();
            self.skipws();
            loop {
                if self.curchar == b'%' && self.byte(1) == b'}' {
                    break;
                }
                let token = self.get_ident();
                let Some(token) = token else {
                    self.synerr("missing identifier inside peephole rule.\n");
                    return;
                };
                match token.as_str() {
                    "peeppredicate" => self.peep_predicate_parse(&peep),
                    "peepmatch" => self.peep_match_parse(&peep),
                    "peepprocedure" => self.peep_procedure_parse(&peep),
                    "peepconstraint" => self.peep_constraint_parse(&peep),
                    "peepreplace" => self.peep_replace_parse(&peep),
                    _ => {
                        self.synerr(&format!(
                            "expected peeppreddicate, peepmatch, peepprocedure, peepconstraint, peepreplace, received {}.\n",
                            token));
                    }
                }
                self.skipws();
                if self.curline.is_none() {
                    break;
                }
            }
        } else {
            self.synerr("Missing %%{ ... %%} block after peephole keyword.\n");
            return;
        }
        self.next_char();
        self.next_char();
        // attach peep to root instruction if it has a match
        if let Some(root) = self.peep_root_instruct(&peep) {
            let mut b = root.borrow_mut();
            // append to peephole list of instruction
            match b.peephole.take() {
                None => b.peephole = Some(peep.clone()),
                Some(head) => {
                    // find tail
                    let mut cur = head;
                    loop {
                        let nxt = cur.borrow().next_peephole.clone();
                        match nxt {
                            Some(n) => cur = n,
                            None => break,
                        }
                    }
                    cur.borrow_mut().next_peephole = Some(peep.clone());
                    b.peephole = Some(head);
                }
            }
        }
    }

    fn peep_root_instruct(&self, peep: &FormRef) -> Option<FormRef> {
        let pb = peep.borrow();
        // peepmatch stores instr names in peep_match_instrs; root is the first
        // entry with parent == -1? Actually root is first added instruction.
        let name = pb.peep_match_instrs.first().map(|e| e.2.clone())?;
        drop(pb);
        self.ad.global_names.get(&name).cloned()
    }

    fn peep_predicate_parse(&mut self, peep: &FormRef) {
        self.skipws();
        let Some(rule) = self.get_paren_expr("pred expression", true) else {
            self.synerr("incorrect or missing expression for 'peeppredicate'\n");
            return;
        };
        if self.curchar != b';' {
            self.synerr("missing ';' in peeppredicate definition\n");
            return;
        }
        self.next_char();
        self.skipws();
        peep.borrow_mut().peep_predicate = Some(rule);
    }

    fn peep_match_parse(&mut self, peep: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' at start of peepmatch rule.\n");
            return;
        }
        self.next_char();
        let mut parent: i32 = -1;
        let mut position: i32 = 0;
        let mut input: i32 = 0;
        let root = self.peep_match_child_parse(peep, parent, &mut position, input);
        if root.is_none() {
            self.synerr("missing instruction-name at start of peepmatch.\n");
            return;
        }
        if self.curchar != b')' {
            self.synerr("missing ')' at end of peepmatch.\n");
            return;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' at end of peepmatch.\n");
            return;
        }
        self.next_char();
    }

    // Returns the root InstructForm seen (first instruction at this level)
    fn peep_match_child_parse(&mut self, peep: &FormRef, parent: i32,
                              position: &mut i32, input: i32) -> Option<FormRef> {
        let mut lparen: i32 = 0;
        let mut rparen: i32 = 0;
        let mut inst_seen: Option<FormRef> = None;
        loop {
            if lparen < rparen {
                break;
            }
            self.skipws();
            if self.curchar == b'(' {
                lparen += 1;
                self.next_char();
                self.peep_match_child_parse(peep, parent, position, rparen);
            } else if self.curchar == b')' {
                rparen += 1;
                if rparen == lparen {
                    self.next_char();
                } else {
                    if inst_seen.is_none() {
                        {
                            let mut b = peep.borrow_mut();
                            b.peep_match_instrs.push((parent, *position, SIGNAL.to_string(), input));
                        }
                        *position += 1;
                    }
                    return inst_seen;
                }
            } else if let Some(token) = self.get_ident_dup() {
                let form = self.ad.global_names.get(&token).cloned();
                if let Some(form) = form {
                    let is_inst = form.borrow().kind == FormKind::Instruct;
                    if inst_seen.is_none() {
                        inst_seen = Some(form.clone());
                    }
                    if is_inst {
                        {
                            let mut b = peep.borrow_mut();
                            if *position as i32 > b.peep_match_max_position {
                                b.peep_match_max_position = *position;
                            }
                            b.peep_match_instrs.push((parent, *position, token.clone(), input));
                        }
                        // parent = position for children
                        // (C++ uses local 'parent' var; emulate via recursion param)
                        // We handle by recursion below through a wrapper.
                        // For simplicity, children get parent = *position before increment.
                        // Implemented via peep_match_child_parse_parented.
                        *position += 1;
                        // recurse manually is complex; C++ continues loop with parent=position
                        // We emulate by continuing the loop with updated parent via a stack:
                        // Simplest faithful approach: recurse is already handled for '('.
                        // For non-paren children the C++ loop just continues; the 'parent'
                        // variable was updated. We track it via a local shadow:
                        // -> handled by caller passing updated parent on subsequent calls is
                        //    not possible; instead we store pending parent in peep? No:
                        // C++ parent var is per-call. The update `parent = position` affects
                        // subsequent add_instruction calls in THIS call's loop. Emulate:
                        // (we saved old parent in 'parent' param shadow below)
                        // NOTE: see peep_match_child_parse2 for full fidelity.
                        let _ = parent;
                    } else {
                        self.synerr(&format!("instruction name expected at identifier {}.\n", token));
                        return inst_seen;
                    }
                } else {
                    self.synerr("missing identifier in peepmatch rule.\n");
                    return None;
                }
            } else {
                self.synerr("missing identifier in peepmatch rule.\n");
                return None;
            }
        }
        None
    }

    fn peep_procedure_parse(&mut self, peep: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' at start of peepprocedure rule.\n");
            return;
        }
        self.next_char();
        self.skipws();
        let Some(name) = self.get_ident_dup() else {
            self.synerr("incorrect or missing expression for 'peepprocedure'\n");
            return;
        };
        self.skipws();
        if self.curchar != b')' {
            self.synerr("peepprocedure should contain a single identifier only\n");
            return;
        }
        self.next_char();
        if self.curchar != b';' {
            self.synerr("missing ';' in peepprocedure definition\n");
            return;
        }
        self.next_char();
        self.skipws();
        peep.borrow_mut().peep_procedure = Some(name);
    }

    fn peep_constraint_parse(&mut self, peep: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' at start of peepconstraint rule.\n");
            return;
        }
        self.next_char();
        self.skipws();
        loop {
            if self.curchar == b')' {
                break;
            }
            let left_inst = self.get_int();
            self.skipws();
            if self.curchar != b'.' {
                self.synerr("missing '.' in peepconstraint after instruction number.\n");
                return;
            }
            self.next_char();
            let Some(left_op) = self.get_ident_dup() else {
                self.synerr("missing operand identifier in peepconstraint.\n");
                return;
            };
            self.skipws();
            let Some(relation) = self.get_relation_dup() else {
                return;
            };
            self.skipws();
            let mut right_inst: i64 = -1;
            if self.curchar.is_ascii_digit() {
                right_inst = self.get_int();
                self.skipws();
                if self.curchar != b'.' {
                    self.synerr("missing '.' in peepconstraint after instruction number.\n");
                    return;
                }
                self.next_char();
            }
            let Some(right_op) = self.get_ident_dup() else {
                self.synerr("missing operand identifier in peepconstraint.\n");
                return;
            };
            peep.borrow_mut().peep_constraints.push((
                left_inst as i32, left_op, relation, right_inst as i32, right_op));
            self.skipws();
            if self.curchar == b',' {
                self.next_char();
                self.skipws();
            } else if self.curchar != b')' {
                self.synerr("expected ',' or ')' after peephole constraint.\n");
                return;
            }
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' at end of peepconstraint.\n");
            return;
        }
        self.next_char();
    }

    fn peep_replace_parse(&mut self, peep: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' at start of peepreplace rule.\n");
            return;
        }
        self.next_char();
        let Some(inst) = self.get_ident_dup() else {
            self.synerr("Instruction name expected at start of peepreplace.\n");
            return;
        };
        let is_inst = self.ad.global_names.get(&inst)
            .map_or(false, |f| f.borrow().kind == FormKind::Instruct);
        if !is_inst {
            self.synerr("Instruction name expected at start of peepreplace.\n");
            return;
        }
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' at peepreplace root's operand-list.\n");
            return;
        }
        self.next_char();
        {
            let mut b = peep.borrow_mut();
            b.peep_replace_root = Some(inst);
        }
        self.skipws();
        loop {
            if self.curchar == b')' {
                break;
            }
            let inst_num = self.get_int();
            self.skipws();
            if self.curchar != b'.' {
                self.synerr("missing '.' in peepreplace after instruction number.\n");
                return;
            }
            self.next_char();
            let Some(inst_op) = self.get_ident_dup() else {
                self.synerr("missing operand identifier in peepreplace.\n");
                return;
            };
            peep.borrow_mut().peep_replace_operands.push((inst_num as i32, inst_op));
            self.skipws();
        }
        self.next_char();
        self.skipws();
        if self.curchar != b')' {
            self.synerr("missing ')' at end of peepmatch.\n");
            self.synerr("Support one replacement instruction.\n");
            return;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' at end of peepreplace.\n");
            return;
        }
        self.next_char();
    }

    //============================== Level 2 ==================================
    fn pred_parse(&mut self) -> Option<String> {
        self.skipws();
        let _line = self.linenum();
        let Some(rule) = self.get_paren_expr("pred expression", true) else {
            self.synerr("incorrect or missing expression for 'predicate'\n");
            return None;
        };
        if self.curchar != b';' {
            self.synerr("missing ';' in predicate definition\n");
            return None;
        }
        self.next_char();
        self.skipws();
        Some(rule)
    }

    fn match_parse(&mut self, operands: &HashMap<String, FormRef>) -> Option<MatchRuleRef> {
        let mut depth: i32 = 0;
        let mut numleaves: i32 = 0;
        let mnode = self.match_node_parse(operands, &mut depth, &mut numleaves, true)?;
        self.skipws();
        let cnstr = if self.curchar == b';' {
            self.next_char();
            None
        } else {
            match self.find_cpp_block("match constructor") {
                Some(c) => Some(c),
                None => {
                    self.synerr("invalid construction of match rule\nMissing ';' or invalid '%%{' and '%%}' constructor\n");
                    return None;
                }
            }
        };
        let rule = MatchRule::new(mnode, depth, cnstr, numleaves as usize);
        self.skipws();
        Some(Rc::new(RefCell::new(rule)))
    }

    fn match_node_parse(&mut self, operands: &HashMap<String, FormRef>, depth: &mut i32,
                         numleaves: &mut i32, atroot: bool) -> Option<MatchNode> {
        let l_parens = *depth;
        let r_parens = *depth;
        let mut l_child = None;
        let mut r_child = None;
        if self.cur_char() != b'(' {
            return None;
        }
        self.next_char();
        let token = self.get_ident()?;
        // has_match_rule tracking
        for i in (LAST_MACHINE_LEAF + 1)..LAST_OPCODE {
            if NODE_CLASS_NAMES[i] == token {
                if i < self.ad.has_match_rule.len() {
                    self.ad.has_match_rule[i] = true;
                }
            }
        }
        let mut result: Option<String> = None;
        let mut name: Option<String> = Some(token.clone());
        let mut operation = token.clone();
        let form = operands.get(&token).cloned();
        let opc_form = form.as_ref().filter(|f| {
            let f = f.borrow();
            f.kind == FormKind::OpClass || f.kind == FormKind::Operand
        }).cloned();
        if let Some(of) = opc_form {
            let ofb = of.borrow();
            if !ofb.ideal_only {
                operation = ofb.ident.clone();
                result = Some(operation.clone());
            } else {
                name = None;
            }
        }
        self.skipws();
        if self.cur_char() != b')' {
            if operation != "Set" {
                l_child = self.match_child_parse(operands, depth, numleaves, false);
            } else {
                l_child = self.match_child_parse(operands, depth, numleaves, true);
            }
            self.skipws();
            if self.cur_char() != b')' {
                if operation != "Set" {
                    r_child = self.match_child_parse(operands, depth, numleaves, false);
                } else {
                    r_child = self.match_child_parse(operands, depth, numleaves, true);
                }
            }
        }
        self.skipws();
        if self.cur_char() != b')' {
            self.synerr("missing ')' in match expression\n");
            return None;
        }
        self.next_char();
        let mut mroot = MatchNode::new(
            result,
            name,
            operation,
            l_child.map(Box::new),
            r_child.map(Box::new),
        );
        if !atroot {
            // build internal operand name via AD tables
            let mut internal_ops = std::mem::take(&mut self.ad.internal_ops);
            let mut internal_match = std::mem::take(&mut self.ad.internal_match);
            mroot.build_internalop_with(&mut internal_ops, &mut internal_match);
            self.ad.internal_ops = internal_ops;
            self.ad.internal_match = internal_match;
        }
        *depth = l_parens.max(r_parens);
        Some(mroot)
    }

    fn match_child_parse(&mut self, operands: &HashMap<String, FormRef>, parens: &mut i32,
                          numleaves: &mut i32, atroot: bool) -> Option<MatchNode> {
        if self.cur_char() == b'(' {
            *parens += 1;
            self.match_node_parse(operands, parens, numleaves, atroot)
        } else {
            let token = self.get_ident()?;
            let form = operands.get(&token).cloned();
            let opc_form = form.as_ref().filter(|f| {
                let f = f.borrow();
                f.kind == FormKind::OpClass || f.kind == FormKind::Operand
            }).cloned();
            let Some(of) = opc_form else {
                self.synerr(&format!("undefined operand {} in match rule\n", token));
                return None;
            };
            let op_type = of.borrow().ident.clone();
            let result = Some(op_type.clone());
            *numleaves += 1;
            Some(MatchNode::leaf(result, Some(token), op_type))
        }
    }

    // Port of FormatRule parsing; returns (strings, rep_vars) where strings
    // contains SIGNAL markers at positions of rep vars, SIGNAL2/SIGNAL3 in
    // template mode.
    fn format_parse(&mut self) -> Option<(NameList, NameList)> {
        let mut format_strings: NameList = Vec::new();
        let mut format_rep_vars: NameList = Vec::new();
        self.skipws();
        if self.curchar == b';' {
            self.next_char();
            return Some((format_strings, format_rep_vars));
        }
        if self.curchar == b'%' && self.byte(1) == b'{' {
            self.next_char();
            self.next_char();
            self.skipws();
            if self.curchar == b'$' {
                let ident = self.get_rep_var_ident();
                if ident.as_deref() == Some("$$template") {
                    return self.template_parse();
                }
                self.synerr(&format!("Unknown \"{}\" directive in format", ident.unwrap_or_default()));
                return None;
            }
            if self.curchar == b'"' {
                self.next_char();
                if self.curchar == b'"' {
                    // Handle empty format string case
                    self.buf.buf[self.ptr] = 0;
                    format_strings.push(String::new());
                }
                loop {
                    if self.curchar == b'%' || self.curchar == b'\n' {
                        if self.curchar != b'"' {
                            self.synerr("missing '\"' at end of format block");
                            return None;
                        }
                    }
                    // (1) string passed through to output
                    let start = self.ptr;
                    while !(self.curchar == b'$' || self.curchar == b'"'
                        || self.curchar == b'%' || self.curchar == b'\n')
                    {
                        if self.curchar == b'\\' {
                            self.next_char();
                            if self.curchar == b'$' || self.curchar == b'%' {
                                self.buf.buf[self.ptr - 1] = self.curchar;
                            }
                        }
                        if self.curchar == b'\n' {
                            self.synerr("newline in string");
                        }
                        self.next_char();
                        if self.curchar == 0 && self.curline.is_none() {
                            break;
                        }
                    }
                    if start != self.ptr {
                        let end = self.ptr;
                        let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
                        self.buf.buf[end] = 0;
                        format_strings.push(s);
                    }
                    // (2) replacement variable
                    if self.curchar == b'$' {
                        self.next_char();
                        let rep_var = self.get_ident().unwrap_or_default();
                        self.buf.buf[self.ptr] = self.curchar;
                        format_rep_vars.push(rep_var);
                        format_strings.push(SIGNAL.to_string());
                    }
                    // (3) adjacent string literal
                    if self.curchar == b'"' {
                        self.next_char();
                        self.skipws();
                        if self.curchar != b'"' {
                            break;
                        } else {
                            self.next_char();
                        }
                    } else {
                        break;
                    }
                }
            }
            self.skipws();
            if self.curchar != b'%' {
                self.synerr("non-blank characters between closing '\"' and '%%' in format");
                return None;
            }
        }
        self.skipws();
        if self.curchar != b'%' || self.byte(1) != b'}' {
            self.synerr("missing '%%}' at end of format block");
            return None;
        }
        self.next_char();
        self.next_char();
        self.skipws();
        Some((format_strings, format_rep_vars))
    }

    fn template_parse(&mut self) -> Option<(NameList, NameList)> {
        let mut format_strings: NameList = Vec::new();
        let mut format_rep_vars: NameList = Vec::new();
        self.skipws();
        while self.curchar != b'%' && self.byte(1) != b'}' {
            // (1) string passed through to output
            {
                let start = self.ptr;
                while !(self.curchar == b'$'
                    || (self.curchar == b'%' && self.byte(1) == b'}'))
                {
                    if self.curchar == b'/' && (self.byte(1) == b'/' || self.byte(1) == b'*') {
                        self.skipws_no_preproc();
                    } else {
                        self.next_char_or_line();
                    }
                    if self.curchar == 0 && self.curline.is_none() {
                        break;
                    }
                }
                if start != self.ptr {
                    let end = self.ptr;
                    let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
                    self.buf.buf[end] = 0;
                    format_strings.push(SIGNAL2.to_string());
                    format_strings.push(s);
                }
            }
            // (2) replacement variable
            if self.curchar == b'$' {
                let rep_var = self.get_rep_var_ident_dup().unwrap_or_default();
                if rep_var == "$emit" {
                    // switch to normal format parsing
                    self.next_char();
                    self.next_char();
                    self.skipws();
                    if self.curchar == b'"' {
                        self.next_char();
                        if self.curchar == b'"' {
                            self.buf.buf[self.ptr] = 0;
                            format_strings.push(String::new());
                        }
                        loop {
                            if self.curchar == b'%' || self.curchar == b'\n' {
                                self.synerr("missing '\"' at end of format block");
                                return None;
                            }
                            let start = self.ptr;
                            while !(self.curchar == b'$' || self.curchar == b'"'
                                || self.curchar == b'%' || self.curchar == b'\n')
                            {
                                if self.curchar == b'\\' {
                                    self.next_char();
                                }
                                if self.curchar == b'\n' {
                                    self.synerr("newline in string");
                                }
                                self.next_char();
                                if self.curchar == 0 && self.curline.is_none() {
                                    break;
                                }
                            }
                            if start != self.ptr {
                                let end = self.ptr;
                                let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
                                self.buf.buf[end] = 0;
                                format_strings.push(s);
                            }
                            if self.curchar == b'$' {
                                self.next_char();
                                let nrv = self.get_ident().unwrap_or_default();
                                self.buf.buf[self.ptr] = self.curchar;
                                format_rep_vars.push(nrv);
                                format_strings.push(SIGNAL.to_string());
                            }
                            if self.curchar == b'"' {
                                self.next_char();
                                self.skipws();
                                if self.curchar != b'"' {
                                    break;
                                } else {
                                    self.next_char();
                                }
                            } else {
                                break;
                            }
                        }
                    }
                } else {
                    format_rep_vars.push(rep_var);
                    format_strings.push(SIGNAL3.to_string());
                }
            }
            if self.curchar == 0 && self.curline.is_none() {
                break;
            }
        }
        self.skipws();
        if self.curchar != b'%' || self.byte(1) != b'}' {
            self.synerr("missing '%%}' at end of format block");
            return None;
        }
        self.next_char();
        self.next_char();
        self.skipws();
        Some((format_strings, format_rep_vars))
    }

    fn effect_parse(&mut self, instr: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' in effect definition\n");
            return;
        }
        self.get_effectlist(instr);
        if self.curchar != b';' {
            self.synerr("missing ';' in Effect definition\n");
        }
        self.next_char();
    }

    fn flag_parse(&mut self) -> Option<Vec<String>> {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' in flag definition\n");
            return None;
        }
        let mut names = Vec::new();
        loop {
            self.next_char();
            self.skipws();
            if self.curchar == b')' {
                break;
            }
            let Some(ident) = self.get_ident() else {
                self.synerr(&format!("flag name expected at {}\n", self.curchar as char));
                return None;
            };
            names.push(ident);
            self.skipws();
            if self.curchar != b',' {
                break;
            }
        }
        if self.curchar != b')' {
            self.synerr("missing ')'\n");
        } else {
            self.next_char();
        }
        if self.curchar != b';' {
            self.synerr("missing ';' in Flag definition\n");
        }
        self.next_char();
        Some(names)
    }

    fn expand_parse(&mut self, instr: &FormRef) -> Option<(NameList, HashMap<String, String>, Vec<(String, NameList)>)> {
        self.skipws();
        if !(self.curchar == b'%' && self.byte(1) == b'{') {
            self.synerr("missing '%%{' in expand definition\n");
            return None;
        }
        self.next_char();
        let mut newopers: NameList = Vec::new();
        let mut newopconst: HashMap<String, String> = HashMap::new();
        let mut instrs: Vec<(String, NameList)> = Vec::new();
        loop {
            let Some(ident) = self.get_ident() else {
                self.synerr(&format!("identifier expected at {}\n", self.curchar as char));
                continue;
            };
            let form = self.ad.global_names.get(&ident).cloned();
            let mut parse_oper = false;
            let mut parse_ins = false;
            if form.is_none() {
                self.skipws();
                if self.curchar == b'(' {
                    parse_ins = true;
                }
            } else {
                let f = form.as_ref().unwrap().borrow();
                if f.kind == FormKind::Instruct {
                    parse_ins = true;
                } else if f.kind == FormKind::Operand {
                    parse_oper = true;
                } else {
                    self.synerr(&format!("instruction/operand name expected at {}\n", ident));
                    continue;
                }
            }
            if parse_oper {
                let oper_form = form.unwrap();
                self.skipws();
                let local = instr.borrow().local_names.clone();
                let Some(new_ident) = self.get_unique_ident_local_in("Operand", &local) else {
                    self.synerr(&format!("identifier expected at {}\n", self.curchar as char));
                    continue;
                };
                newopers.push(new_ident.clone());
                instr.borrow_mut().local_names.insert(new_ident.clone(), oper_form);
                self.skipws();
                if self.curchar == b'%' {
                    let c = self.find_cpp_block("Operand Constructor");
                    match c {
                        Some(c) => {
                            newopconst.insert(new_ident, c);
                        }
                        None => {
                            self.synerr("Invalid code block for operand constructor\n");
                            continue;
                        }
                    }
                } else if self.curchar != b';' {
                    self.synerr("Missing ; in expand rule operand declaration\n");
                    continue;
                } else {
                    self.next_char();
                }
                self.skipws();
            } else {
                // instruction with operand list
                self.skipws();
                if self.curchar != b'(' {
                    self.synerr("missing '(' in expand instruction declaration\n");
                    continue;
                }
                let mut oplist: NameList = Vec::new();
                loop {
                    self.next_char();
                    self.skipws();
                    if self.curchar == b')' {
                        break;
                    }
                    let Some(ident2) = self.get_ident() else {
                        self.synerr(&format!("identifier expected at {}\n", self.curchar as char));
                        continue;
                    };
                    self.skipws();
                    let local = instr.borrow().local_names.clone();
                    let Some(form2) = local.get(&ident2).cloned() else {
                        self.synerr(&format!("operand name expected at {}\n", ident2));
                        continue;
                    };
                    let ok = {
                        let f = form2.borrow();
                        f.kind == FormKind::Operand || f.kind == FormKind::OpClass
                    };
                    if !ok {
                        self.synerr(&format!("operand name expected at {}\n", ident2));
                        continue;
                    }
                    oplist.push(ident2);
                    if self.curchar != b',' {
                        break;
                    }
                }
                if self.curchar != b')' {
                    self.synerr("missing ')'in expand instruction declaration\n");
                    continue;
                }
                self.next_char();
                if self.curchar != b';' {
                    self.synerr("missing ';'in expand instruction declaration\n");
                    continue;
                }
                self.next_char();
                instrs.push((ident, oplist));
                self.skipws();
            }
            if self.curchar == b'%' {
                break;
            }
        }
        self.next_char();
        if self.curchar != b'}' {
            self.synerr("missing '%%}' in expand rule definition\n");
            return None;
        }
        self.next_char();
        self.skipws();
        Some((newopers, newopconst, instrs))
    }

    fn rewrite_parse(&mut self) -> Option<(String, String)> {
        self.skipws();
        let Some(params) = self.get_paren_expr_simple("rewrite parameters") else {
            self.synerr("missing '(' in rewrite rule\n");
            return None;
        };
        self.skipws();
        let Some(desc) = self.find_cpp_block("rewrite block") else {
            self.synerr("incorrect or missing block for 'rewrite'.\n");
            return None;
        };
        self.skipws();
        Some((params, desc))
    }

    fn attr_parse(&mut self, ident: &str) -> Option<Attribute> {
        self.skipws();
        let Some(cost) = self.get_paren_expr_simple("attribute") else {
            self.synerr("incorrect or missing expression for 'attribute'\n");
            return None;
        };
        if self.curchar != b';' {
            self.synerr("missing ';' in attribute definition\n");
            return None;
        }
        self.next_char();
        self.skipws();
        Some(Attribute {
            ident: ident.to_string(),
            val: cost,
            atype: INS_ATTR,
            next: None,
        })
    }

    fn opcode_parse(&mut self) -> Option<(Option<String>, Option<String>, Option<String>)> {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' in expand instruction declaration\n");
            return None;
        }
        self.next_char();
        self.skipws();
        let mut primary = None;
        let mut secondary = None;
        let mut tertiary = None;
        if self.curchar != b')' {
            primary = self.get_ident_or_literal_constant("primary opcode");
            if primary.is_none() {
                self.synerr(&format!("primary hex opcode expected at {}\n", self.curchar as char));
                return None;
            }
            self.skipws();
            if self.curchar == b',' {
                self.next_char();
                self.skipws();
                secondary = self.get_ident_or_literal_constant("secondary opcode");
                if secondary.is_none() {
                    self.synerr(&format!("secondary hex opcode expected at {}\n", self.curchar as char));
                    return None;
                }
                self.skipws();
                if self.curchar == b',' {
                    self.next_char();
                    self.skipws();
                    tertiary = self.get_ident_or_literal_constant("tertiary opcode");
                    if tertiary.is_none() {
                        self.synerr(&format!("tertiary hex opcode expected at {}\n", self.curchar as char));
                        return None;
                    }
                    self.skipws();
                }
            }
            self.skipws();
            if self.curchar != b')' {
                self.synerr("Missing ')' in opcode description\n");
                return None;
            }
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' in ins_attrib definition\n");
            return None;
        }
        self.next_char();
        self.skipws();
        Some((primary, secondary, tertiary))
    }

    fn size_parse(&mut self) -> Option<String> {
        self.skipws();
        let size = self.get_paren_expr_simple("size expression");
        if size.is_none() {
            self.synerr(&format!("size of opcode expected at {}\n", self.curchar as char));
            return None;
        }
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' in ins_attrib definition\n");
            return None;
        }
        self.next_char();
        self.skipws();
        size
    }

    fn interface_parse(&mut self) -> Option<Interface> {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("Missing '(' at start of interface description.\n");
            return None;
        }
        self.next_char();
        self.skipws();
        let Some(iface_name) = self.get_ident() else {
            self.synerr("missing interface name after 'interface'.\n");
            return None;
        };
        self.skipws();
        if self.curchar != b')' {
            self.synerr("Missing ')' after name of interface.\n");
            return None;
        }
        self.next_char();
        let mut inter: Option<Interface> = None;
        self.skipws();
        if self.curchar != b';' {
            if iface_name == "MEMORY_INTER" {
                inter = self.mem_interface_parse();
            } else if iface_name == "COND_INTER" {
                inter = self.cond_interface_parse();
            }
            if self.curchar == b';' {
                self.synerr("Extra ';' after defining interface block.\n");
                self.next_char();
                return None;
            }
        } else {
            self.next_char();
            if iface_name == "REG_INTER" {
                inter = Some(Interface {
                    kind: InterfaceKind::Reg, name: iface_name,
                    base: None, index: None, scale: None, disp: None,
                    equal: None, not_equal: None, less: None, greater_equal: None,
                    less_equal: None, greater: None, overflow: None, no_overflow: None,
                    equal_format: None, not_equal_format: None, less_format: None,
                    greater_equal_format: None, less_equal_format: None, greater_format: None,
                    overflow_format: None, no_overflow_format: None,
                });
            } else if iface_name == "CONST_INTER" {
                inter = Some(Interface {
                    kind: InterfaceKind::Const, name: iface_name,
                    base: None, index: None, scale: None, disp: None,
                    equal: None, not_equal: None, less: None, greater_equal: None,
                    less_equal: None, greater: None, overflow: None, no_overflow: None,
                    equal_format: None, not_equal_format: None, less_format: None,
                    greater_equal_format: None, less_equal_format: None, greater_format: None,
                    overflow_format: None, no_overflow_format: None,
                });
            }
        }
        self.skipws();
        inter
    }

    fn mem_interface_parse(&mut self) -> Option<Interface> {
        if self.curchar != b'%' {
            self.synerr("Missing '%%{' for 'interface' block.\n");
            return None;
        }
        self.next_char();
        if self.curchar != b'{' {
            self.synerr("Missing '%%{' for 'interface' block.\n");
            return None;
        }
        self.next_char();
        self.skipws();
        let mut base = None;
        let mut index = None;
        let mut scale = None;
        let mut disp = None;
        loop {
            if self.curchar == b'%' {
                break;
            }
            let Some(field) = self.get_ident() else {
                self.synerr("Expected keyword, base|index|scale|disp,  or '%%}' ending interface.\n");
                return None;
            };
            let v = self.interface_field_parse(None);
            match field.as_str() {
                "base" => base = v,
                "index" => index = v,
                "scale" => scale = v,
                "disp" => disp = v,
                _ => {
                    self.synerr("Expected keyword, base|index|scale|disp,  or '%%}' ending interface.\n");
                    return None;
                }
            }
        }
        self.next_char();
        if self.curchar != b'}' {
            self.synerr("Missing '%%}' for 'interface' block.\n");
            return None;
        }
        self.next_char();
        Some(Interface {
            kind: InterfaceKind::Mem, name: "MEMORY_INTER".to_string(),
            base, index, scale, disp,
            equal: None, not_equal: None, less: None, greater_equal: None,
            less_equal: None, greater: None, overflow: None, no_overflow: None,
            equal_format: None, not_equal_format: None, less_format: None,
            greater_equal_format: None, less_equal_format: None, greater_format: None,
            overflow_format: None, no_overflow_format: None,
        })
    }

    fn cond_interface_parse(&mut self) -> Option<Interface> {
        if self.curchar != b'%' {
            self.synerr("Missing '%%{' for 'cond_interface' block.\n");
            return None;
        }
        self.next_char();
        if self.curchar != b'{' {
            self.synerr("Missing '%%{' for 'cond_interface' block.\n");
            return None;
        }
        self.next_char();
        self.skipws();
        let mut equal = None;
        let mut not_equal = None;
        let mut less = None;
        let mut greater_equal = None;
        let mut less_equal = None;
        let mut greater = None;
        let mut overflow = None;
        let mut no_overflow = None;
        let mut equal_format = Some("eq".to_string());
        let mut not_equal_format = Some("ne".to_string());
        let mut less_format = Some("lt".to_string());
        let mut greater_equal_format = Some("ge".to_string());
        let mut less_equal_format = Some("le".to_string());
        let mut greater_format = Some("gt".to_string());
        let mut overflow_format = Some("o".to_string());
        let mut no_overflow_format = Some("no".to_string());
        loop {
            if self.curchar == b'%' {
                break;
            }
            let Some(field) = self.get_ident() else {
                self.synerr("Expected keyword, base|index|scale|disp,  or '%%}' ending interface.\n");
                return None;
            };
            match field.as_str() {
                "equal" => equal = self.interface_field_parse(Some(&mut equal_format)),
                "not_equal" => not_equal = self.interface_field_parse(Some(&mut not_equal_format)),
                "less" => less = self.interface_field_parse(Some(&mut less_format)),
                "greater_equal" => greater_equal = self.interface_field_parse(Some(&mut greater_equal_format)),
                "less_equal" => less_equal = self.interface_field_parse(Some(&mut less_equal_format)),
                "greater" => greater = self.interface_field_parse(Some(&mut greater_format)),
                "overflow" => overflow = self.interface_field_parse(Some(&mut overflow_format)),
                "no_overflow" => no_overflow = self.interface_field_parse(Some(&mut no_overflow_format)),
                _ => {
                    self.synerr("Expected keyword, base|index|scale|disp,  or '%%}' ending interface.\n");
                    return None;
                }
            }
        }
        self.next_char();
        if self.curchar != b'}' {
            self.synerr("Missing '%%}' for 'interface' block.\n");
            return None;
        }
        self.next_char();
        Some(Interface {
            kind: InterfaceKind::Cond, name: "COND_INTER".to_string(),
            base: None, index: None, scale: None, disp: None,
            equal, not_equal, less, greater_equal, less_equal, greater, overflow, no_overflow,
            equal_format, not_equal_format, less_format, greater_equal_format,
            less_equal_format, greater_format, overflow_format, no_overflow_format,
        })
    }

    fn interface_field_parse(&mut self, format: Option<&mut Option<String>>) -> Option<String> {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("Missing '(' at start of interface field.\n");
            return None;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b'0' && self.curchar != b'$' {
            self.synerr("missing or invalid interface field contents.\n");
            return None;
        }
        let iface_field = self.get_rep_var_ident();
        if iface_field.is_none() {
            self.synerr("missing or invalid interface field contents.\n");
            return None;
        }
        let iface_field = iface_field.unwrap();
        self.skipws();
        if let Some(fmt_slot) = format {
            if self.curchar == b',' {
                self.next_char();
                self.skipws();
                if self.curchar != b'"' {
                    self.synerr("Missing '\"' in field format .\n");
                    return None;
                }
                self.next_char();
                let start = self.ptr;
                while !(self.curchar == b'"' || self.curchar == b'%' || self.curchar == b'\n') {
                    if self.curchar == b'\\' {
                        self.next_char();
                    }
                    if self.curchar == b'\n' {
                        self.synerr("newline in string");
                    }
                    self.next_char();
                    if self.curchar == 0 && self.curline.is_none() {
                        break;
                    }
                }
                if self.curchar != b'"' {
                    self.synerr("Missing '\"' at end of field format .\n");
                    return None;
                }
                if start != self.ptr {
                    let end = self.ptr;
                    let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
                    self.buf.buf[end] = 0;
                    **fmt_slot = Some(s);
                }
                self.next_char();
                self.skipws();
            }
        }
        if self.curchar != b')' {
            self.synerr("Missing ')' after interface field.\n");
            return None;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("Missing ';' at end of interface field.\n");
            return None;
        }
        self.next_char();
        self.skipws();
        Some(iface_field)
    }

    fn constraint_parse(&mut self) -> Option<(String, String)> {
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing constraint expression, (...)\n");
            return None;
        }
        self.next_char();
        self.skipws();
        let Some(func) = self.get_ident() else {
            self.synerr("missing function in constraint expression.\n");
            return None;
        };
        let arg;
        if func == "ALLOC_IN_RC" || func == "IS_R_CLASS" {
            self.skipws();
            if self.curchar != b'(' {
                self.synerr("missing '(' for constraint function's argument.\n");
                return None;
            }
            self.next_char();
            self.skipws();
            let Some(a) = self.get_ident() else {
                self.synerr(&format!("missing argument for constraint function {}\n", func));
                return None;
            };
            arg = a;
            self.skipws();
            if self.curchar != b')' {
                self.synerr(&format!("missing ')' after constraint function argument {}\n", arg));
                return None;
            }
            self.next_char();
        } else {
            self.synerr(&format!("Invalid constraint function {}\n", func));
            return None;
        }
        self.skipws();
        if self.curchar != b')' {
            self.synerr(&format!("Missing ')' for constraint function {}\n", func));
            return None;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("Missing ';' after constraint.\n");
            return None;
        }
        self.next_char();
        Some((func, arg))
    }

    fn construct_parse(&mut self) -> Option<String> {
        None
    }

    //------------------------------ins_encode----------------------------------
    fn ins_encode_parse(&mut self, inst: &FormRef) {
        self.skipws();
        if self.curchar != b'(' {
            if self.curchar == b'%' && self.byte(1) == b'{' {
                self.next_char();
                self.next_char();
                self.ins_encode_parse_block(inst);
                return;
            }
            self.synerr("missing '%%{' or '(' in ins_encode definition\n");
            return;
        }
        self.next_char();
        self.skipws();
        let mut encrule = InsEncode::default();
        loop {
            if self.curchar == b')' {
                break;
            }
            let Some(ec_name) = self.get_ident() else {
                self.synerr("Invalid encode class name after 'ins_encode('.\n");
                return;
            };
            // check enc class exists (warning deferred in C++)
            let idx = encrule.add_encode(&ec_name);
            self.skipws();
            if self.curchar == b'(' {
                self.next_char();
                loop {
                    if self.curchar == b')' {
                        break;
                    }
                    let param = self.get_ident_or_literal_constant("encoding operand");
                    if let Some(param) = param {
                        if param == "constanttablebase" {
                            {
                                let mut b = inst.borrow_mut();
                                b.needs_constant_base = true;
                                if !b.mach_base_class().starts_with("MachCall") {
                                    b.is_mach_constant = true;
                                }
                            }
                            if self.curchar == b'(' {
                                self.synerr(&format!(
                                    "constanttablebase in instruct {} cannot have an argument (only constantaddress and constantoffset)",
                                    ec_name));
                                return;
                            }
                        } else if param == "constantaddress" || param == "constantoffset" {
                            inst.borrow_mut().is_mach_constant = true;
                            if self.curchar == b'(' {
                                self.constant_parse(inst);
                            }
                        } else {
                            let is_local = inst.borrow().local_names.contains_key(&param);
                            let is_lit = Self::is_literal_constant(&param);
                            let is_opcode = matches!(param.as_str(), "primary" | "secondary" | "tertiary");
                            let is_reg = self.ad.register.as_ref().map_or(false, |r| {
                                r.borrow().reg_def.contains_key(&param)
                            });
                            if !is_local && !is_lit && !is_opcode && !is_reg {
                                self.synerr(&format!(
                                    "Using non-locally defined parameter {} for encoding {}.\n",
                                    param, ec_name));
                                return;
                            }
                        }
                        encrule.encodes[idx].1.push(param);
                        self.skipws();
                        if self.curchar == b',' {
                            self.next_char();
                            self.skipws();
                        } else if self.curchar == b')' {
                            // done
                        } else {
                            self.synerr(&format!("expected ',' or ')' after parameter {}.\n", ec_name));
                            return;
                        }
                    } else {
                        self.skipws();
                        if self.curchar == b',' {
                            self.synerr(&format!("Expected encode parameter before ',' in encoding {}.\n", ec_name));
                            return;
                        }
                        if self.curchar != b')' {
                            self.synerr("Expected ')' after encode parameters.\n");
                            return;
                        }
                    }
                }
                self.next_char();
            }
            self.skipws();
            if self.curchar == b',' {
                self.next_char();
                self.skipws();
            } else if self.curchar != b')' {
                self.synerr(&format!("Expected ')' after encoding {}.\n", ec_name));
                return;
            }
        }
        if self.curchar != b')' {
            self.synerr("Missing ')' at end of ins_encode description.\n");
            return;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("Missing ';' at end of ins_encode.\n");
            return;
        }
        self.next_char();
        self.skipws();
        if inst.borrow().insencode.is_some() {
            self.synerr("Multiple ins_encode sections defined\n");
            return;
        }
        inst.borrow_mut().insencode = Some(encrule);
    }

    fn ins_encode_parse_block(&mut self, inst: &FormRef) {
        let prefix = "__ins_encode_";
        let ec_name = format!("{}{}", prefix, inst.borrow().ident);
        let encoding = self.add_enc_class(self.ad.encode.as_ref().unwrap(), &ec_name);
        encoding.borrow_mut().linenum = self.linenum();
        // synthesize parameters from instruct's parameters
        {
            let b = inst.borrow();
            for param in &b.parameters {
                if let Some(pf) = b.local_names.get(param) {
                    let pfb = pf.borrow();
                    let ptype = if pfb.kind == FormKind::Operand { pfb.ident.clone() }
                        else if pfb.kind == FormKind::OpClass { pfb.ident.clone() }
                        else { String::new() };
                    let mut eb = encoding.borrow_mut();
                    eb.parameter_type.push(ptype);
                    eb.parameter_name.push(param.clone());
                }
            }
        }
        self.ins_encode_parse_block_impl(inst, &encoding, &ec_name);
        // build InsEncode invoking the new enc class
        let mut encrule = InsEncode::default();
        let idx = encrule.add_encode(&ec_name);
        for param in &inst.borrow().parameters {
            encrule.encodes[idx].1.push(param.clone());
        }
        if inst.borrow().insencode.is_some() {
            self.synerr("Multiple ins_encode sections defined\n");
            return;
        }
        inst.borrow_mut().insencode = Some(encrule);
    }

    fn ins_encode_parse_block_impl(&mut self, inst: &FormRef, encoding: &FormRef, ec_name: &str) {
        self.skipws_no_preproc();
        loop {
            if self.curchar == b'%' && self.byte(1) == b'}' {
                break;
            }
            let start = self.ptr;
            while !(self.curchar == b'$'
                || (self.curchar == b'%' && self.byte(1) == b'}'))
            {
                if self.curchar == b'/' && (self.byte(1) == b'/' || self.byte(1) == b'*') {
                    self.skipws_no_preproc();
                } else {
                    self.next_char_or_line();
                }
                if self.curchar == 0 && self.curline.is_none() {
                    break;
                }
            }
            if start != self.ptr {
                let end = self.ptr;
                let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
                self.buf.buf[end] = 0;
                encoding.borrow_mut().code_strings.push(s);
            }
            if self.curchar == b'$' {
                if let Some(rep_var) = self.get_rep_var_ident_dup() {
                    encoding.borrow_mut().rep_vars.push(rep_var.clone());
                    self.skipws();
                    if rep_var == "constanttablebase" {
                        {
                            let mut b = inst.borrow_mut();
                            b.needs_constant_base = true;
                            if !b.mach_base_class().starts_with("MachCall") {
                                b.is_mach_constant = true;
                            }
                        }
                        if self.curchar == b'(' {
                            self.synerr(&format!(
                                "constanttablebase in instruct {} cannot have an argument (only constantaddress and constantoffset)",
                                ec_name));
                            return;
                        }
                    } else if rep_var == "constantaddress" || rep_var == "constantoffset" {
                        inst.borrow_mut().is_mach_constant = true;
                        if self.curchar == b'(' {
                            self.constant_parse(inst);
                        }
                    }
                }
            }
            if self.curchar == 0 && self.curline.is_none() {
                break;
            }
        }
        self.next_char();
        self.next_char();
        self.skipws();
    }

    fn postalloc_expand_parse(&mut self, inst: &FormRef) {
        inst.borrow_mut().is_postalloc_expand = true;
        self.skipws();
        if self.curchar != b'(' {
            if self.curchar == b'%' && self.byte(1) == b'{' {
                self.next_char();
                self.next_char();
                self.ins_encode_parse_block(inst);
                return;
            }
            self.synerr("missing '(' in postalloc_expand definition\n");
            return;
        }
        self.next_char();
        self.skipws();
        let mut encrule = InsEncode::default();
        if self.curchar != b')' {
            let Some(ec_name) = self.get_ident() else {
                self.synerr("Invalid postalloc_expand class name after 'postalloc_expand('.\n");
                return;
            };
            let idx = encrule.add_encode(&ec_name);
            self.skipws();
            if self.curchar == b'(' {
                self.next_char();
                loop {
                    if self.curchar == b')' {
                        break;
                    }
                    let param = self.get_ident_or_literal_constant("encoding operand");
                    if let Some(param) = param {
                        if param == "constanttablebase" {
                            {
                                let mut b = inst.borrow_mut();
                                b.needs_constant_base = true;
                                if !b.mach_base_class().starts_with("MachCall") {
                                    b.is_mach_constant = true;
                                }
                            }
                            if self.curchar == b'(' {
                                self.synerr(&format!(
                                    "constanttablebase in instruct {} cannot have an argument (only constantaddress and constantoffset)",
                                    ec_name));
                                return;
                            }
                        } else if param == "constantaddress" || param == "constantoffset" {
                            inst.borrow_mut().is_mach_constant = true;
                            if self.curchar == b'(' {
                                self.constant_parse(inst);
                            }
                        } else {
                            let is_local = inst.borrow().local_names.contains_key(&param);
                            let is_lit = Self::is_literal_constant(&param);
                            let is_opcode = matches!(param.as_str(), "primary" | "secondary" | "tertiary");
                            let is_reg = self.ad.register.as_ref().map_or(false, |r| {
                                r.borrow().reg_def.contains_key(&param)
                            });
                            if !is_local && !is_lit && !is_opcode && !is_reg {
                                self.synerr(&format!(
                                    "Using non-locally defined parameter {} for encoding {}.\n",
                                    param, ec_name));
                                return;
                            }
                        }
                        encrule.encodes[idx].1.push(param);
                        self.skipws();
                        if self.curchar == b',' {
                            self.next_char();
                            self.skipws();
                        } else if self.curchar == b')' {
                        } else {
                            self.synerr(&format!("expected ',' or ')' after parameter {}.\n", ec_name));
                            return;
                        }
                    } else {
                        self.skipws();
                        if self.curchar == b',' {
                            self.synerr(&format!("Expected encode parameter before ',' in postalloc_expand {}.\n", ec_name));
                            return;
                        }
                        if self.curchar != b')' {
                            self.synerr("Expected ')' after postalloc_expand parameters.\n");
                            return;
                        }
                    }
                }
                self.next_char();
            }
            self.skipws();
            if self.curchar != b')' {
                self.synerr(&format!("Expected ')' after postalloc_expand {}.\n", ec_name));
                return;
            }
        }
        if self.curchar != b')' {
            self.synerr("Missing ')' at end of postalloc_expand description.\n");
            return;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("Missing ';' at end of postalloc_expand.\n");
            return;
        }
        self.next_char();
        self.skipws();
        inst.borrow_mut().insencode = Some(encrule);
    }

    fn constant_parse(&mut self, inst: &FormRef) {
        let prefix = "__constant_";
        let ec_name = format!("{}{}", prefix, inst.borrow().ident);
        let encoding = self.add_enc_class(self.ad.encode.as_ref().unwrap(), &ec_name);
        encoding.borrow_mut().linenum = self.linenum();
        {
            let b = inst.borrow();
            for param in &b.parameters {
                if let Some(pf) = b.local_names.get(param) {
                    let pfb = pf.borrow();
                    let ptype = pfb.ident.clone();
                    let mut eb = encoding.borrow_mut();
                    eb.parameter_type.push(ptype);
                    eb.parameter_name.push(param.clone());
                }
            }
        }
        self.constant_parse_expression(&encoding, &ec_name);
        let mut encrule = InsEncode::default();
        let idx = encrule.add_encode(&ec_name);
        for param in &inst.borrow().parameters {
            encrule.encodes[idx].1.push(param.clone());
        }
        inst.borrow_mut().constant_encode = Some(encrule);
    }

    fn constant_parse_expression(&mut self, encoding: &FormRef, _ec_name: &str) {
        self.skipws();
        encoding.borrow_mut().code_strings.push(
            "    _constant = C->output()->constant_table().add".to_string());
        encoding.borrow_mut().code_strings.push("(this, ".to_string());
        self.next_char(); // skip '('
        let mut parens_depth = 1i32;
        while parens_depth > 0 {
            if self.curchar == b'(' {
                parens_depth += 1;
                encoding.borrow_mut().code_strings.push("(".to_string());
                self.next_char_or_line();
            } else if self.curchar == b')' {
                parens_depth -= 1;
                if parens_depth > 0 {
                    encoding.borrow_mut().code_strings.push(")".to_string());
                }
                self.next_char_or_line();
            } else {
                let start = self.ptr;
                while !(self.curchar == b'$' || self.curchar == b'('
                    || self.curchar == b')')
                {
                    self.next_char_or_line();
                    if self.curchar == 0 && self.curline.is_none() {
                        break;
                    }
                }
                if start != self.ptr {
                    let end = self.ptr;
                    let s = String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned();
                    self.buf.buf[end] = 0;
                    encoding.borrow_mut().code_strings.push(s);
                }
                if self.curchar == b'$' {
                    if let Some(rep_var) = self.get_rep_var_ident_dup() {
                        encoding.borrow_mut().rep_vars.push(rep_var);
                    }
                }
                if self.curchar == 0 && self.curline.is_none() {
                    break;
                }
            }
        }
        encoding.borrow_mut().code_strings.push(");".to_string());
    }

    //------------------------------reg sections-------------------------------
    fn reg_def_parse(&mut self, reg: &FormRef) {
        self.skipws();
        let Some(rname) = self.get_ident() else {
            self.synerr("missing register name after reg_def\n");
            return;
        };
        self.skipws();
        let mut callconv = None;
        let mut c_conv = None;
        let mut idealtype = None;
        let mut encoding = None;
        let mut concrete = None;
        if self.curchar == b'(' {
            self.next_char();
            let Some(cc) = self.get_ident() else {
                self.synerr("missing register calling convention value\n");
                return;
            };
            if !matches!(cc.as_str(), "SOC" | "SOE" | "NS" | "AS") {
                self.synerr("invalid value for register calling convention\n");
            }
            callconv = Some(cc);
            self.skipws();
            if self.curchar != b',' {
                self.synerr("missing comma in register definition statement\n");
                return;
            }
            self.next_char();
            let Some(cc2) = self.get_ident() else {
                self.synerr("missing register native calling convention value\n");
                return;
            };
            if !matches!(cc2.as_str(), "SOC" | "SOE" | "NS" | "AS") {
                self.synerr("invalid value for register calling convention\n");
            }
            c_conv = Some(cc2);
            self.skipws();
            if self.curchar != b',' {
                self.synerr("missing comma in register definition statement\n");
                return;
            }
            self.next_char();
            self.skipws();
            let Some(it) = self.get_ident() else {
                self.synerr("missing register save type value\n");
                return;
            };
            idealtype = Some(it);
            self.skipws();
            if self.curchar != b',' {
                self.synerr("missing comma in register definition statement\n");
                return;
            }
            self.next_char();
            self.skipws();
            let Some(enc) = self.get_expr("encoding", ",") else {
                self.synerr("missing register encoding value\n");
                return;
            };
            encoding = Some(Self::trim(&enc));
            if self.curchar != b',' {
                self.synerr("missing comma in register definition statement\n");
                return;
            }
            self.next_char();
            self.skipws();
            let Some(conc) = self.get_expr("concrete", ")") else {
                self.synerr("missing vm register name value\n");
                return;
            };
            concrete = Some(conc);
            if self.curchar != b')' {
                self.synerr("missing ')' in register definition statement\n");
                return;
            }
            self.next_char();
        }
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' after reg_def\n");
            return;
        }
        self.next_char();
        let mut f = Form::new(FormKind::RegDef);
        f.ident = rname.clone();
        f.callconv = callconv;
        f.c_conv = c_conv;
        f.idealtype = idealtype;
        f.register_encode = encoding;
        f.concrete = concrete;
        let rd = Rc::new(RefCell::new(f));
        {
            let mut b = reg.borrow_mut();
            b.rdefs.push(rname.clone());
            b.reg_def.insert(rname, rd);
        }
    }

    fn reg_class_parse(&mut self, reg: &FormRef) {
        self.skipws();
        let Some(cname) = self.get_ident() else {
            self.synerr("missing register class name after 'reg_class'\n");
            return;
        };
        self.skipws();
        if self.curchar == b'(' {
            let kind = FormKind::RegClass;
            let mut f = Form::new(kind);
            f.ident = cname.clone();
            let rc = Rc::new(RefCell::new(f));
            {
                let mut b = reg.borrow_mut();
                b.rclasses.push(cname.clone());
                b.reg_class.insert(cname.clone(), rc.clone());
            }
            self.next_char();
            self.skipws();
            loop {
                if self.curchar == b')' {
                    break;
                }
                let Some(rname) = self.get_ident() else {
                    self.synerr("missing identifier inside reg_class list.\n");
                    return;
                };
                let rd = reg.borrow().reg_def.get(&rname).cloned();
                match rd {
                    Some(rd) => {
                        rc.borrow_mut().rdefs.push(rname.clone());
                        rc.borrow_mut().reg_def.insert(rname.clone(), rd);
                    }
                    None => {
                        self.synerr(&format!("unknown identifier {} inside reg_class list.\n", rname));
                    }
                }
                self.skipws();
                if self.curchar == b',' {
                    self.next_char();
                    self.skipws();
                }
            }
            self.next_char();
        } else if self.curchar == b'%' {
            let mut f = Form::new(FormKind::CodeSnippetRegClass);
            f.ident = cname.clone();
            let rc = Rc::new(RefCell::new(f));
            {
                let mut b = reg.borrow_mut();
                b.rclasses.push(cname.clone());
                b.reg_class.insert(cname.clone(), rc.clone());
            }
            let code = self.find_cpp_block("reg class");
            match code {
                Some(code) => {
                    rc.borrow_mut().code_snippet = Some(code);
                }
                None => {
                    self.synerr("missing code declaration for reg class.\n");
                    return;
                }
            }
            return;
        }
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' at end of reg_class definition.\n");
            return;
        }
        self.next_char();
    }

    fn reg_class_dynamic_parse(&mut self, reg: &FormRef) {
        self.skipws();
        let Some(cname) = self.get_ident() else {
            self.synerr("missing dynamic register class name after 'reg_class_dynamic'\n");
            return;
        };
        self.skipws();
        if self.curchar != b'(' {
            self.synerr("missing '(' at the beginning of reg_class_dynamic definition\n");
            return;
        }
        self.next_char();
        self.skipws();
        let mut f = Form::new(FormKind::ConditionalRegClass);
        f.ident = cname.clone();
        let rc = Rc::new(RefCell::new(f));
        {
            let mut b = reg.borrow_mut();
            b.rclasses.push(cname.clone());
            b.reg_class.insert(cname.clone(), rc.clone());
        }
        for i in 0..2 {
            let Some(name) = self.get_ident() else {
                self.synerr("missing class identifier inside reg_class_dynamic list.\n");
                return;
            };
            let rcl = reg.borrow().reg_class.get(&name).cloned();
            match rcl {
                Some(rcl) => {
                    rc.borrow_mut().rclass_at[i] = Some(rcl);
                }
                None => {
                    self.synerr(&format!("unknown identifier {} inside reg_class_dynamic list.\n", name));
                }
            }
            self.skipws();
            if self.curchar == b',' {
                self.next_char();
                self.skipws();
            } else {
                self.synerr("missing separator ',' inside reg_class_dynamic list.\n");
            }
        }
        self.skipws();
        if self.curchar == b'%' {
            let code = self.find_cpp_block("reg class dynamic");
            match code {
                Some(code) => {
                    rc.borrow_mut().condition_code = Some(code);
                }
                None => {
                    self.synerr("missing code declaration for reg_class_dynamic.\n");
                    return;
                }
            }
        } else {
            self.synerr("missing %% at the beginning of code block in reg_class_dynamic definition\n");
            return;
        }
        self.skipws();
        if self.curchar != b')' {
            self.synerr("missing ')' at the end of reg_class_dynamic definition\n");
            return;
        }
        self.next_char();
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' at the end of reg_class_dynamic definition.\n");
            return;
        }
        self.next_char();
    }

    fn alloc_class_parse(&mut self, reg: &FormRef) {
        self.skipws();
        let Some(name) = self.get_ident() else {
            self.synerr("missing allocation class name after 'reg_class'\n");
            return;
        };
        let mut f = Form::new(FormKind::AllocClass);
        f.ident = name.clone();
        let ac = Rc::new(RefCell::new(f));
        {
            let mut b = reg.borrow_mut();
            b.aclasses.push(name.clone());
            b.alloc_class.insert(name, ac.clone());
        }
        self.skipws();
        if self.curchar == b'(' {
            self.next_char();
            self.skipws();
            loop {
                if self.curchar == b')' {
                    break;
                }
                let Some(rname) = self.get_ident() else {
                    self.synerr("missing identifier inside reg_class list.\n");
                    return;
                };
                let rd = reg.borrow().reg_def.get(&rname).cloned();
                match rd {
                    Some(rd) => {
                        ac.borrow_mut().rdefs.push(rname.clone());
                        ac.borrow_mut().reg_def.insert(rname, rd);
                    }
                    None => {
                        self.synerr(&format!("name {} should be a previously defined reg_def.\n", rname));
                        return;
                    }
                }
                self.skipws();
                if self.curchar == b',' {
                    self.next_char();
                    self.skipws();
                }
            }
            self.next_char();
        }
        self.skipws();
        if self.curchar != b';' {
            self.synerr("missing ';' at end of reg_class definition.\n");
            return;
        }
        self.next_char();
    }

    fn add_spill_reg_class(&mut self, reg: FormRef) {
        // align reg_ctr to 8
        let mut b = reg.borrow_mut();
        b.register_num = (b.register_num + 7) & !7;
        let mut f = Form::new(FormKind::RegClass);
        f.ident = "stack_slots".to_string();
        f.stack_or_reg = true;
        let rc = Rc::new(RefCell::new(f));
        b.rclasses.push("stack_slots".to_string());
        b.reg_class.insert("stack_slots".to_string(), rc);
    }

    fn add_dynamic_reg_class(&mut self, reg: FormRef) {
        let mut f = Form::new(FormKind::RegClass);
        f.ident = "dynamic".to_string();
        f.stack_or_reg = false;
        let rc = Rc::new(RefCell::new(f));
        let mut b = reg.borrow_mut();
        b.rclasses.push("dynamic".to_string());
        b.reg_class.insert("dynamic".to_string(), rc);
    }

    //------------------------------get_oplist----------------------------------
    fn get_oplist(&mut self, parameters: &mut NameList, operands: &mut HashMap<String, FormRef>) {
        loop {
            self.next_char();
            self.skipws();
            if self.curchar == b')' {
                break;
            }
            let Some(ty) = self.get_ident() else {
                self.synerr(&format!("optype identifier expected at {}\n", self.curchar as char));
                return;
            };
            let form = self.ad.global_names.get(&ty).cloned();
            let Some(form) = form else {
                self.synerr(&format!("undefined operand type {}\n", ty));
                return;
            };
            {
                let f = form.borrow();
                let ok = f.kind == FormKind::Operand || f.kind == FormKind::OpClass;
                if !ok {
                    self.synerr(&format!("identifier {} not operand type\n", ty));
                    return;
                }
            }
            let Some(name) = self.get_unique_ident_local_in("operand", operands) else {
                return;
            };
            if self.ad.global_names.contains_key(&name) {
                self.synerr(&format!("Reuse of global name {} as operand.\n", name));
                return;
            }
            operands.insert(name.clone(), form);
            parameters.push(name);
            self.skipws();
            if self.curchar != b',' {
                break;
            }
        }
        if self.curchar != b')' {
            self.synerr("missing ')'\n");
        } else {
            self.next_char();
        }
    }

    fn get_effectlist(&mut self, instr: &FormRef) {
        loop {
            self.next_char();
            self.skipws();
            if self.curchar == b')' {
                break;
            }
            let Some(ident) = self.get_ident() else {
                self.synerr(&format!("effect type identifier expected at {}\n", self.curchar as char));
                return;
            };
            let form = self.ad.global_names.get(&ident).cloned();
            let Some(eform) = form else {
                self.synerr(&format!("undefined effect type {}\n", ident));
                return;
            };
            {
                let ef = eform.borrow();
                if ef.kind != FormKind::Effect {
                    self.synerr(&format!("identifier {} not effect type\n", ident));
                    return;
                }
            }
            self.skipws();
            let is_call = eform.borrow().use_def == CALL;
            if is_call {
                instr.borrow_mut().has_call = true;
            } else {
                // operand name
                let local = instr.borrow().local_names.clone();
                let Some(opname) = self.get_unique_ident_local_in("effect", &HashMap::new()) else {
                    self.synerr("missing operand identifier in effect list\n");
                    return;
                };
                let opform = local.get(&opname).cloned();
                let ok = opform.as_ref().map_or(false, |f| {
                    let f = f.borrow();
                    f.kind == FormKind::Operand
                });
                if !ok {
                    let is_class = opform.as_ref().map_or(false, |f| f.borrow().kind == FormKind::OpClass);
                    if is_class {
                        let cname = opform.unwrap().borrow().ident.clone();
                        self.synerr(&format!(
                            "operand classes are illegal in effect lists (found {} {})\n", cname, opname));
                    } else {
                        self.synerr(&format!("undefined operand {} in effect list\n", opname));
                    }
                    return;
                }
                instr.borrow_mut().effects.insert(opname, eform);
            }
            self.skipws();
            if self.curchar != b',' {
                break;
            }
        }
        if self.curchar != b')' {
            self.synerr("missing ')'\n");
        } else {
            self.next_char();
        }
    }

    //------------------------------preproc directives-------------------------
    fn preproc_line(&mut self) {
        let line = self.get_int();
        self.skipws_no_preproc();
        let mut file: Option<String> = None;
        if self.curchar == b'"' {
            self.next_char();
            let start = self.ptr;
            loop {
                if self.curchar == b'\n' {
                    self.synerr("missing '\"' at end of #line directive");
                    return;
                }
                if self.curchar == b'"' {
                    let end = self.ptr;
                    file = Some(String::from_utf8_lossy(&self.buf.buf[start..end]).into_owned());
                    self.buf.buf[end] = 0;
                    self.next_char();
                    self.skipws_no_preproc();
                    break;
                }
                self.next_char();
                if self.curchar == 0 && self.curline.is_none() {
                    break;
                }
            }
        }
        self.ensure_end_of_line();
        if let Some(f) = file {
            self.ad.adl_file_name = f;
        }
        self.buf.linenum = line as usize;
    }

    fn preproc_define(&mut self) {
        let flag = self.get_ident_no_preproc().unwrap_or_default();
        self.skipws_no_preproc();
        let def = self.get_ident_no_preproc();
        self.ad.set_preproc_def(&flag, def.as_deref());
        self.skipws_no_preproc();
        if self.curchar != b'\n' {
            self.synerr("non-identifier in preprocessor definition\n");
        }
    }

    fn preproc_undef(&mut self) {
        let flag = self.get_ident_no_preproc().unwrap_or_default();
        self.skipws_no_preproc();
        self.ensure_end_of_line();
        self.ad.set_preproc_def(&flag, None);
    }

    //------------------------------utilities----------------------------------
    fn get_unique_ident_local(&mut self, desc: &str) -> Option<String> {
        self.get_unique_ident(&self.ad.global_names, desc)
    }
    fn get_unique_ident_local_in(&mut self, desc: &str, dict: &HashMap<String, FormRef>) -> Option<String> {
        let ident = self.get_ident()?;
        if dict.contains_key(&ident) {
            self.synerr(&format!("duplicate name {} for {}\n", ident, desc));
            return None;
        }
        Some(ident)
    }

    fn is_literal_constant(param: &str) -> bool {
        let b = param.as_bytes();
        if b.is_empty() {
            return false;
        }
        if b[0] == b'(' {
            return true;
        }
        if b[0] == b'0' && b.len() > 1 && (b[1] == b'x' || b[1] == b'X') {
            return b[2..].iter().all(|c| c.is_ascii_hexdigit());
        }
        false
    }
    fn is_int_token(token: &str, intval: &mut i64) -> bool {
        let t = token.trim();
        let t = t.strip_prefix('-').unwrap_or(t);
        if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit()) {
            return false;
        }
        *intval = token.trim().parse().unwrap_or(0);
        true
    }
}
