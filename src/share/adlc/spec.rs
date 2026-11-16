/*
 * OpenJDK 25 HotSpot adlc/ port
 *
 * Machine-readable matcher specification: serializes ArchDesc to JSON.
 * No external dependencies: a minimal JSON value type, emitter and parser
 * are included so that specs round-trip through `Json`.
 */

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::rc::Rc;

use super::model::*;
use super::opcodes::NODE_CLASS_NAMES;

//------------------------------Json value------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn s(v: &str) -> Json {
        Json::Str(v.to_string())
    }
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        if let Json::Obj(kv) = self {
            kv.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Json::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        if let Json::Arr(a) = self {
            Some(a)
        } else {
            None
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let Json::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }
    pub fn push_obj(&mut self, key: &str, val: Json) {
        if let Json::Obj(kv) = self {
            kv.push((key.to_string(), val));
        }
    }
}

fn json_escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn json_to_string(v: &Json) -> String {
    let mut s = String::new();
    write_json(v, &mut s);
    s
}

fn write_json(v: &Json, out: &mut String) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => {
            let _ = write!(out, "{}", i);
        }
        Json::Num(n) => {
            let _ = write!(out, "{}", n);
        }
        Json::Str(s) => json_escape(s, out),
        Json::Arr(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(it, out);
            }
            out.push(']');
        }
        Json::Obj(kv) => {
            out.push('{');
            for (i, (k, val)) in kv.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json_escape(k, out);
                out.push(':');
                write_json(val, out);
            }
            out.push('}');
        }
    }
}

//------------------------------Json parser-----------------------------------
pub struct JsonParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    pub fn new(src: &'a str) -> JsonParser<'a> {
        JsonParser {
            src: src.as_bytes(),
            pos: 0,
        }
    }
    fn ws(&mut self) {
        while self.pos < self.src.len()
            && matches!(self.src[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at offset {}", c as char, self.pos))
        }
    }
    pub fn parse(&mut self) -> Result<Json, String> {
        self.ws();
        let v = self.value()?;
        self.ws();
        if self.pos != self.src.len() {
            return Err(format!("trailing data at offset {}", self.pos));
        }
        Ok(v)
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(format!("unexpected character at offset {}", self.pos)),
        }
    }
    fn lit(&mut self, s: &str, v: Json) -> Result<Json, String> {
        if self.src[self.pos..].starts_with(s.as_bytes()) {
            self.pos += s.len();
            Ok(v)
        } else {
            Err(format!("bad literal at offset {}", self.pos))
        }
    }
    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut kv = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(kv));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.expect(b':')?;
            let v = self.value()?;
            kv.push((k, v));
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(kv));
                }
                _ => return Err(format!("expected ',' or '}}' at offset {}", self.pos)),
            }
        }
    }
    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("expected ',' or ']' at offset {}", self.pos)),
            }
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            let c = self.peek().ok_or("unterminated string")?;
            self.pos += 1;
            match c {
                b'"' => return Ok(s),
                b'\\' => {
                    let e = self.peek().ok_or("bad escape")?;
                    self.pos += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let hex = self
                                .src
                                .get(self.pos..self.pos + 4)
                                .ok_or("bad unicode escape")?;
                            let code = u32::from_str_radix(
                                std::str::from_utf8(hex).map_err(|_| "bad unicode escape")?,
                                16,
                            )
                            .map_err(|_| "bad unicode escape")?;
                            self.pos += 4;
                            s.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(format!("bad escape at offset {}", self.pos)),
                    }
                }
                _ => {
                    // collect UTF-8 bytes
                    let start = self.pos - 1;
                    let mut end = self.pos;
                    while end < self.src.len() && self.src[end] != b'"' && self.src[end] != b'\\' {
                        end += 1;
                    }
                    s.push_str(std::str::from_utf8(&self.src[start..end]).map_err(|_| "bad utf8")?);
                    self.pos = end;
                }
            }
        }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'.' || c == b'e' || c == b'E' || c == b'+' || c == b'-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).map_err(|_| "bad number")?;
        if let Ok(i) = text.parse::<i64>() {
            return Ok(Json::Int(i));
        }
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number {}", text))
    }
}

pub fn parse_json(src: &str) -> Result<Json, String> {
    JsonParser::new(src).parse()
}

//------------------------------spec emission--------------------------------
fn opt_str(v: &Option<String>) -> Json {
    match v {
        Some(s) => Json::s(s),
        None => Json::Null,
    }
}

fn name_list(v: &[String]) -> Json {
    Json::Arr(v.iter().map(|s| Json::s(s)).collect())
}

fn str_map(m: &BTreeMap<String, String>) -> Json {
    Json::Obj(m.iter().map(|(k, v)| (k.clone(), Json::s(v))).collect())
}

fn match_node_json(m: &MatchNode) -> Json {
    let mut o = Json::obj();
    o.push_obj("result", opt_str(&m.result));
    o.push_obj("name", opt_str(&m.name));
    o.push_obj("op_type", Json::s(&m.op_type));
    o.push_obj("internalop", opt_str(&m.internalop));
    o.push_obj(
        "l_child",
        m.l_child
            .as_ref()
            .map(|c| match_node_json(c))
            .unwrap_or(Json::Null),
    );
    o.push_obj(
        "r_child",
        m.r_child
            .as_ref()
            .map(|c| match_node_json(c))
            .unwrap_or(Json::Null),
    );
    o
}

fn match_rule_json(r: &MatchRule) -> Json {
    let mut o = Json::obj();
    o.push_obj("op_type", Json::s(&r.m.op_type));
    o.push_obj("result", opt_str(&r.m.result));
    o.push_obj("name", opt_str(&r.m.name));
    o.push_obj("internalop", opt_str(&r.m.internalop));
    o.push_obj("depth", Json::Int(r.depth as i64));
    o.push_obj("construct", opt_str(&r.construct));
    o.push_obj("numchilds", Json::Int(r.numchilds as i64));
    o.push_obj(
        "l_child",
        r.m.l_child
            .as_ref()
            .map(|c| match_node_json(c))
            .unwrap_or(Json::Null),
    );
    o.push_obj(
        "r_child",
        r.m.r_child
            .as_ref()
            .map(|c| match_node_json(c))
            .unwrap_or(Json::Null),
    );
    o
}

fn match_rule_list_json(head: &Option<MatchRuleRef>) -> Json {
    let mut rules = Vec::new();
    let mut cur = head.clone();
    while let Some(r) = cur {
        rules.push(match_rule_json(&r.borrow()));
        cur = r.borrow().next.clone();
    }
    Json::Arr(rules)
}

fn interface_json(i: &Interface) -> Json {
    let mut o = Json::obj();
    o.push_obj(
        "kind",
        Json::s(match i.kind {
            InterfaceKind::None => "none",
            InterfaceKind::Reg => "REG_INTER",
            InterfaceKind::Const => "CONST_INTER",
            InterfaceKind::Mem => "MEMORY_INTER",
            InterfaceKind::Cond => "COND_INTER",
        }),
    );
    o.push_obj("name", Json::s(&i.name));
    for (k, v) in [
        ("base", &i.base),
        ("index", &i.index),
        ("scale", &i.scale),
        ("disp", &i.disp),
        ("equal", &i.equal),
        ("not_equal", &i.not_equal),
        ("less", &i.less),
        ("greater_equal", &i.greater_equal),
        ("less_equal", &i.less_equal),
        ("greater", &i.greater),
        ("overflow", &i.overflow),
        ("no_overflow", &i.no_overflow),
        ("equal_format", &i.equal_format),
        ("not_equal_format", &i.not_equal_format),
        ("less_format", &i.less_format),
        ("greater_equal_format", &i.greater_equal_format),
        ("less_equal_format", &i.less_equal_format),
        ("greater_format", &i.greater_format),
        ("overflow_format", &i.overflow_format),
        ("no_overflow_format", &i.no_overflow_format),
    ] {
        o.push_obj(k, opt_str(v));
    }
    o
}

fn attribute_list_json(head: &Option<Rc<super::model::Attribute>>) -> Json {
    let mut items = Vec::new();
    let mut cur = head.clone();
    while let Some(a) = cur {
        let mut o = Json::obj();
        o.push_obj("name", Json::s(&a.ident));
        o.push_obj("value", Json::s(&a.val));
        o.push_obj(
            "type",
            Json::s(if a.atype == INS_ATTR {
                "ins_attrib"
            } else {
                "op_attrib"
            }),
        );
        items.push(o);
        cur = a.next.clone();
    }
    Json::Arr(items)
}

fn effects_json(effects: &std::collections::HashMap<String, FormRef>) -> Json {
    let mut names: Vec<&String> = effects.keys().collect();
    names.sort();
    Json::Arr(
        names
            .iter()
            .map(|n| {
                let mut o = Json::obj();
                o.push_obj("operand", Json::s(*n));
                let ef = effects[*n].borrow();
                o.push_obj("effect", Json::s(&ef.effect_name));
                o.push_obj("use_def", Json::s(ef.usedef_name_static()));
                o
            })
            .collect(),
    )
}

impl Form {
    fn usedef_name_static(&self) -> &'static str {
        // helper for Effect forms
        match self.use_def {
            INVALID_UDEF => "INVALID",
            USE => "USE",
            USE_DEF => "USE_DEF",
            USE_KILL => "USE_KILL",
            KILL => "KILL",
            TEMP => "TEMP",
            TEMP_DEF => "TEMP_DEF",
            DEF => "DEF",
            CALL => "CALL",
            _ => "UNDEFINED",
        }
    }
}

fn ins_encode_json(enc: &Option<InsEncode>) -> Json {
    match enc {
        None => Json::Null,
        Some(e) => Json::Arr(
            e.encodes
                .iter()
                .map(|(n, params)| {
                    let mut o = Json::obj();
                    o.push_obj("enc_class", Json::s(n));
                    o.push_obj("parameters", name_list(params));
                    o
                })
                .collect(),
        ),
    }
}

fn components_json(c: &ComponentList) -> Json {
    Json::Arr(
        c.components
            .iter()
            .map(|comp| {
                let mut o = Json::obj();
                o.push_obj("name", Json::s(&comp.name));
                o.push_obj("type", Json::s(&comp.type_name));
                o.push_obj("usedef", Json::s(comp.usedef_name()));
                o
            })
            .collect(),
    )
}

fn param_types_json(f: &Form) -> Json {
    Json::Arr(
        f.parameters
            .iter()
            .map(|p| {
                let mut o = Json::obj();
                o.push_obj("name", Json::s(p));
                match f.local_names.get(p) {
                    Some(tf) => {
                        let t = tf.borrow();
                        o.push_obj("type", Json::s(&t.ident));
                        o.push_obj(
                            "type_kind",
                            Json::s(match t.kind {
                                FormKind::Operand => "operand",
                                FormKind::OpClass => "opclass",
                                _ => "other",
                            }),
                        );
                    }
                    None => {
                        o.push_obj("type", Json::Null);
                        o.push_obj("type_kind", Json::s("unresolved"));
                    }
                }
                o
            })
            .collect(),
    )
}

fn operand_json(f: &Form) -> Json {
    let mut o = Json::obj();
    o.push_obj("name", Json::s(&f.ident));
    o.push_obj("ideal_only", Json::Bool(f.ideal_only));
    o.push_obj("parameters", param_types_json(f));
    o.push_obj("predicate", opt_str(&f.predicate));
    o.push_obj(
        "constraint",
        Json::Arr(vec![
            opt_str(&f.constraint_func),
            opt_str(&f.constraint_arg),
        ]),
    );
    o.push_obj("construct", opt_str(&f.construct_expr));
    o.push_obj("opclasses", name_list(&f.classes));
    o.push_obj(
        "interface",
        f.interface
            .as_ref()
            .map(interface_json)
            .unwrap_or(Json::Null),
    );
    o.push_obj("format_strings", name_list(&f.format_strings));
    o.push_obj("format_rep_vars", name_list(&f.format_rep_vars));
    o.push_obj("attributes", attribute_list_json(&f.attribs));
    o.push_obj("match_rules", match_rule_list_json(&f.matrule));
    o
}

fn opclass_json(f: &Form) -> Json {
    let mut o = Json::obj();
    o.push_obj("name", Json::s(&f.ident));
    o.push_obj("operands", name_list(&f.oplst));
    o
}

fn instruct_json(f: &Form) -> Json {
    let mut o = Json::obj();
    o.push_obj("name", Json::s(&f.ident));
    o.push_obj("linenum", Json::Int(f.linenum as i64));
    o.push_obj("parameters", param_types_json(f));
    o.push_obj(
        "opcode",
        Json::Arr(vec![
            opt_str(&f.opcode_primary),
            opt_str(&f.opcode_secondary),
            opt_str(&f.opcode_tertiary),
        ]),
    );
    o.push_obj("size", opt_str(&f.size));
    o.push_obj("ins_encode", ins_encode_json(&f.insencode));
    o.push_obj("constant_encode", ins_encode_json(&f.constant_encode));
    o.push_obj("is_postalloc_expand", Json::Bool(f.is_postalloc_expand));
    o.push_obj("is_mach_constant", Json::Bool(f.is_mach_constant));
    o.push_obj("needs_constant_base", Json::Bool(f.needs_constant_base));
    o.push_obj("has_expand_rule", Json::Bool(f.has_expand_rule));
    o.push_obj("ins_pipe", opt_str(&f.ins_pipe));
    o.push_obj("flags", name_list(&f.flag_names));
    o.push_obj("predicate", opt_str(&f.predicate));
    o.push_obj("effects", effects_json(&f.effects));
    o.push_obj("has_call", Json::Bool(f.has_call));
    o.push_obj("format_strings", name_list(&f.format_strings));
    o.push_obj("format_rep_vars", name_list(&f.format_rep_vars));
    o.push_obj("expand_new_operands", name_list(&f.expand_newopers));
    o.push_obj(
        "expand_instructions",
        Json::Arr(
            f.expand_instrs
                .iter()
                .map(|(n, ops)| {
                    let mut e = Json::obj();
                    e.push_obj("instruction", Json::s(n));
                    e.push_obj("operands", name_list(ops));
                    e
                })
                .collect(),
        ),
    );
    o.push_obj("rewrite_parameters", opt_str(&f.rewrite_params));
    o.push_obj("rewrite_block", opt_str(&f.rewrite_block));
    o.push_obj("attributes", attribute_list_json(&f.attribs));
    o.push_obj("mach_base_class", Json::s(f.mach_base_class()));
    o.push_obj("is_ideal_control", Json::Bool(f.is_ideal_control()));
    o.push_obj("match_rules", match_rule_list_json(&f.matrule));
    o.push_obj("components", components_json(&f.components));
    o.push_obj("peepholes", peephole_list_json(&f.peephole));
    o
}

fn peephole_list_json(head: &Option<FormRef>) -> Json {
    let mut items = Vec::new();
    let mut cur = head.clone();
    while let Some(p) = cur {
        let pb = p.borrow();
        let mut o = Json::obj();
        o.push_obj("predicate", opt_str(&pb.peep_predicate));
        o.push_obj(
            "match",
            Json::Arr(
                pb.peep_match_instrs
                    .iter()
                    .map(|(parent, pos, name, input)| {
                        let mut m = Json::obj();
                        m.push_obj("parent", Json::Int(*parent as i64));
                        m.push_obj("position", Json::Int(*pos as i64));
                        m.push_obj("instruction", Json::s(name));
                        m.push_obj("input", Json::Int(*input as i64));
                        m
                    })
                    .collect(),
            ),
        );
        o.push_obj("procedure", opt_str(&pb.peep_procedure));
        o.push_obj(
            "constraints",
            Json::Arr(
                pb.peep_constraints
                    .iter()
                    .map(|(li, lo, rel, ri, ro)| {
                        let mut c = Json::obj();
                        c.push_obj("left_instruction", Json::Int(*li as i64));
                        c.push_obj("left_operand", Json::s(lo));
                        c.push_obj("relation", Json::s(rel));
                        c.push_obj("right_instruction", Json::Int(*ri as i64));
                        c.push_obj("right_operand", Json::s(ro));
                        c
                    })
                    .collect(),
            ),
        );
        o.push_obj("replace_instruction", opt_str(&pb.peep_replace_root));
        o.push_obj(
            "replace_operands",
            Json::Arr(
                pb.peep_replace_operands
                    .iter()
                    .map(|(n, op)| {
                        let mut r = Json::obj();
                        r.push_obj("instruction", Json::Int(*n as i64));
                        r.push_obj("operand", Json::s(op));
                        r
                    })
                    .collect(),
            ),
        );
        items.push(o);
        cur = pb.next_peephole.clone();
    }
    Json::Arr(items)
}

fn register_json(reg: &Form) -> Json {
    let mut o = Json::obj();
    o.push_obj(
        "reg_defs",
        Json::Arr(
            reg.rdefs
                .iter()
                .map(|name| {
                    let rd = reg.reg_def[name].borrow();
                    let mut r = Json::obj();
                    r.push_obj("name", Json::s(name));
                    r.push_obj("callconv", opt_str(&rd.callconv));
                    r.push_obj("c_callconv", opt_str(&rd.c_conv));
                    r.push_obj("idealtype", opt_str(&rd.idealtype));
                    r.push_obj("encode", opt_str(&rd.register_encode));
                    r.push_obj("concrete", opt_str(&rd.concrete));
                    r
                })
                .collect(),
        ),
    );
    o.push_obj(
        "reg_classes",
        Json::Arr(
            reg.rclasses
                .iter()
                .map(|name| {
                    let rc = reg.reg_class[name].borrow();
                    let mut c = Json::obj();
                    c.push_obj("name", Json::s(name));
                    c.push_obj("stack_or_reg", Json::Bool(rc.stack_or_reg));
                    c.push_obj("code_snippet", opt_str(&rc.code_snippet));
                    c.push_obj("condition_code", opt_str(&rc.condition_code));
                    c.push_obj("registers", name_list(&rc.rdefs));
                    let dyn_names: Vec<String> = rc
                        .rclass_at
                        .iter()
                        .filter_map(|r| r.as_ref().map(|f| f.borrow().ident.clone()))
                        .collect();
                    c.push_obj("dynamic_classes", name_list(&dyn_names));
                    c
                })
                .collect(),
        ),
    );
    o.push_obj(
        "alloc_classes",
        Json::Arr(
            reg.aclasses
                .iter()
                .map(|name| {
                    let ac = reg.alloc_class[name].borrow();
                    let mut a = Json::obj();
                    a.push_obj("name", Json::s(name));
                    a.push_obj("registers", name_list(&ac.rdefs));
                    a
                })
                .collect(),
        ),
    );
    o
}

fn encode_json(enc: &Form) -> Json {
    let mut o = Json::obj();
    o.push_obj(
        "enc_classes",
        Json::Arr(
            enc.eclasses
                .iter()
                .map(|name| {
                    let ec = enc.enc_class[name].borrow();
                    let mut e = Json::obj();
                    e.push_obj("name", Json::s(name));
                    e.push_obj("parameter_types", name_list(&ec.parameter_type));
                    e.push_obj("parameter_names", name_list(&ec.parameter_name));
                    e.push_obj(
                        "code",
                        Json::Arr(ec.code_strings.iter().map(|s| Json::s(s)).collect()),
                    );
                    e.push_obj("rep_vars", name_list(&ec.rep_vars));
                    e
                })
                .collect(),
        ),
    );
    o
}

fn pipeline_json(p: &Form) -> Json {
    let mut o = Json::obj();
    o.push_obj(
        "resources",
        Json::Arr(
            p.reslist
                .iter()
                .map(|name| {
                    let mut r = Json::obj();
                    r.push_obj("name", Json::s(name));
                    r.push_obj(
                        "mask",
                        Json::Int(p.resdict.get(name).copied().unwrap_or(0) as i64),
                    );
                    r
                })
                .collect(),
        ),
    );
    o.push_obj("stages", name_list(&p.stages));
    o.push_obj(
        "pipe_classes",
        Json::Arr(
            p.classlist
                .iter()
                .map(|name| {
                    let pc = p.classdict[name].borrow();
                    let mut c = Json::obj();
                    c.push_obj("name", Json::s(name));
                    c.push_obj("number", Json::Int(pc.pipe_num as i64));
                    c.push_obj("parameters", name_list(&pc.parameters));
                    c.push_obj("instructions", name_list(&pc.pipe_instructs));
                    c.push_obj("has_fixed_latency", Json::Bool(pc.has_fixed_latency));
                    c.push_obj("fixed_latency", Json::Int(pc.fixed_latency as i64));
                    c.push_obj("instruction_count", Json::Int(pc.instruction_count as i64));
                    c.push_obj("has_multiple_bundles", Json::Bool(pc.has_multiple_bundles));
                    c.push_obj("has_branch_delay", Json::Bool(pc.has_branch_delay));
                    c.push_obj("force_serialization", Json::Bool(pc.force_serialization));
                    c.push_obj("may_have_no_code", Json::Bool(pc.may_have_no_code));
                    c.push_obj(
                        "operand_usage",
                        Json::Arr(
                            pc.local_usage
                                .iter()
                                .map(|(n, u)| {
                                    let ub = u.borrow();
                                    let mut x = Json::obj();
                                    x.push_obj("operand", Json::s(n));
                                    x.push_obj("stage", opt_str(&ub.code));
                                    x.push_obj("is_write", Json::Bool(ub.use_def != 0));
                                    x.push_obj(
                                        "more_instructions",
                                        Json::Int(ub.register_num as i64),
                                    );
                                    x
                                })
                                .collect(),
                        ),
                    );
                    c.push_obj(
                        "resource_usage",
                        Json::Arr(
                            pc.res_usage
                                .iter()
                                .map(|r| {
                                    let rb = r.borrow();
                                    let mut x = Json::obj();
                                    x.push_obj("resource", Json::s(&rb.effect_name));
                                    x.push_obj("stage", opt_str(&rb.code));
                                    x.push_obj("cycles", Json::Int(rb.use_def as i64));
                                    x
                                })
                                .collect(),
                        ),
                    );
                    c
                })
                .collect(),
        ),
    );
    o.push_obj("nops", name_list(&p.noplist));
    o.push_obj(
        "variable_size_instructions",
        Json::Bool(p.variable_size_instrs),
    );
    o.push_obj("branch_has_delay_slot", Json::Bool(p.branch_has_delay_slot));
    o.push_obj(
        "max_instructions_per_bundle",
        Json::Int(p.max_instrs_per_bundle as i64),
    );
    o.push_obj(
        "max_bundles_per_cycle",
        Json::Int(p.max_bundles_per_cycle as i64),
    );
    o.push_obj("instruction_unit_size", Json::Int(p.instr_unit_size as i64));
    o.push_obj("bundle_unit_size", Json::Int(p.bundle_unit_size as i64));
    o.push_obj(
        "instruction_fetch_unit_size",
        Json::Int(p.instr_fetch_unit_size as i64),
    );
    o.push_obj(
        "instruction_fetch_units",
        Json::Int(p.instr_fetch_units as i64),
    );
    o.push_obj("max_cycle_used", Json::Int(p.maxcycleused as i64));
    o
}

fn frame_json(f: &Form) -> Json {
    let mut o = Json::obj();
    for (k, v) in [
        ("sync_stack_slots", &f.frame_sync_stack_slots),
        ("inline_cache_reg", &f.frame_inline_cache_reg),
        (
            "interpreter_frame_pointer_reg",
            &f.frame_interpreter_frame_pointer_reg,
        ),
        (
            "cisc_spilling_operand_name",
            &f.frame_cisc_spilling_operand_name,
        ),
        ("frame_pointer", &f.frame_pointer),
        ("c_frame_pointer", &f.c_frame_pointer),
        ("stack_alignment", &f.alignment_str),
        ("return_addr", &f.return_addr),
        ("c_return_addr", &f.c_return_addr),
        ("varargs_c_out_slots_killed", &f.varargs_c_out_slots_killed),
        ("return_value", &f.return_value),
        ("c_return_value", &f.c_return_value),
    ] {
        o.push_obj(k, opt_str(v));
    }
    o.push_obj("return_addr_in_register", Json::Bool(f.return_addr_loc));
    o.push_obj("c_return_addr_in_register", Json::Bool(f.c_return_addr_loc));
    o
}

fn source_block_json(f: &Form) -> Json {
    let mut o = Json::obj();
    o.push_obj(
        "kind",
        Json::s(match f.kind {
            FormKind::PreHeader => "pre_header",
            FormKind::Header => "header",
            _ => "source",
        }),
    );
    o.push_obj("code", opt_str(&f.code));
    o
}

/// Serialize the whole architecture description into the matcher spec JSON.
pub fn arch_desc_to_json(ad: &ArchDesc) -> Json {
    let mut root = Json::obj();
    root.push_obj("format", Json::s("adlc-matcher-spec/1"));
    root.push_obj("file", Json::s(&ad.adl_file_name));

    // definitions (int_def)
    let mut defs: BTreeMap<String, String> = BTreeMap::new();
    for name in &ad.defines_order {
        if let Some(e) = ad.global_defs.get(name) {
            defs.insert(name.clone(), e.expr.clone());
        }
    }
    root.push_obj("definitions", str_map(&defs));

    // source blocks
    let mut blocks = Vec::new();
    for f in ad
        .pre_header
        .iter()
        .chain(ad.header.iter())
        .chain(ad.source.iter())
    {
        blocks.push(source_block_json(&f.borrow()));
    }
    root.push_obj("source_blocks", Json::Arr(blocks));

    // attributes
    root.push_obj(
        "attributes",
        Json::Arr(
            ad.attributes
                .iter()
                .map(|f| {
                    let b = f.borrow();
                    let mut a = Json::obj();
                    a.push_obj("name", Json::s(&b.ident));
                    a.push_obj(
                        "kind",
                        Json::s(if b.use_def == INS_ATTR {
                            "ins_attrib"
                        } else {
                            "op_attrib"
                        }),
                    );
                    a.push_obj("default", opt_str(&b.code));
                    a
                })
                .collect(),
        ),
    );

    // operands
    root.push_obj(
        "operands",
        Json::Arr(
            ad.operands
                .iter()
                .map(|f| operand_json(&f.borrow()))
                .collect(),
        ),
    );

    // operand classes
    root.push_obj(
        "opclasses",
        Json::Arr(
            ad.opclass
                .iter()
                .map(|f| opclass_json(&f.borrow()))
                .collect(),
        ),
    );

    // mach nodes (from pipeline define)
    root.push_obj(
        "mach_nodes",
        Json::Arr(
            ad.machnodes
                .iter()
                .map(|f| {
                    let b = f.borrow();
                    let mut m = Json::obj();
                    m.push_obj("name", Json::s(&b.ident));
                    m.push_obj("ins_pipe", opt_str(&b.ins_pipe));
                    m
                })
                .collect(),
        ),
    );

    // instructions (skip ideal-only internal forms)
    let instrs: Vec<&FormRef> = ad
        .instructions
        .iter()
        .filter(|f| !f.borrow().ideal_only)
        .collect();
    root.push_obj(
        "instructions",
        Json::Arr(instrs.iter().map(|f| instruct_json(&f.borrow())).collect()),
    );

    // register / encode / pipeline / frame
    root.push_obj(
        "register",
        ad.register
            .as_ref()
            .map(|r| register_json(&r.borrow()))
            .unwrap_or(Json::Null),
    );
    root.push_obj(
        "encode",
        ad.encode
            .as_ref()
            .map(|e| encode_json(&e.borrow()))
            .unwrap_or(Json::Null),
    );
    root.push_obj(
        "pipeline",
        ad.pipeline
            .as_ref()
            .map(|p| pipeline_json(&p.borrow()))
            .unwrap_or(Json::Null),
    );
    root.push_obj(
        "frame",
        ad.frame
            .as_ref()
            .map(|fr| frame_json(&fr.borrow()))
            .unwrap_or(Json::Null),
    );

    // chain rules
    let mut chains = Json::obj();
    let mut keys: Vec<&String> = ad.chain_rules.keys().collect();
    keys.sort();
    for k in keys {
        let cl = &ad.chain_rules[k];
        let mut entries = Vec::new();
        for i in 0..cl.name.len() {
            let mut e = Json::obj();
            e.push_obj("operand", Json::s(&cl.name[i]));
            e.push_obj("cost", Json::s(&cl.cost[i]));
            e.push_obj("instruction", Json::s(&cl.rule[i]));
            entries.push(e);
        }
        chains.push_obj(k, Json::Arr(entries));
    }
    root.push_obj("chain_rules", chains);

    // internal operations generated for subtree matches
    let mut iop_names: Vec<&String> = ad.internal_match.keys().collect();
    iop_names.sort();
    root.push_obj(
        "internal_operations",
        Json::Arr(
            iop_names
                .iter()
                .map(|n| {
                    let mut o = Json::obj();
                    o.push_obj("name", Json::s(*n));
                    o.push_obj(
                        "match",
                        ad.internal_match
                            .get(*n)
                            .map(|m| match_node_json(&m.borrow()))
                            .unwrap_or(Json::Null),
                    );
                    o
                })
                .collect(),
        ),
    );

    // DFA match lists indexed by ideal operand name
    let mut mlists = Json::obj();
    for (i, list) in ad.mlistab.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let ideal_name = NODE_CLASS_NAMES.get(i).copied().unwrap_or("?");
        let entries: Vec<Json> = list
            .iter()
            .map(|e| {
                let mut o = Json::obj();
                o.push_obj("opcode", Json::s(&e.opcode));
                o.push_obj("result", Json::s(&e.result_str));
                o.push_obj("lchild", opt_str(&e.lchild));
                o.push_obj("rchild", opt_str(&e.rchild));
                o.push_obj("predicate", opt_str(&e.pred));
                o.push_obj("cost", Json::s(&e.cost));
                o
            })
            .collect();
        mlists.push_obj(ideal_name, Json::Arr(entries));
    }
    root.push_obj("match_lists", mlists);

    let mut errs = Json::obj();
    errs.push_obj("syntax", Json::Int(ad.syntax_errs as i64));
    errs.push_obj("semantic", Json::Int(ad.semantic_errs as i64));
    errs.push_obj("warnings", Json::Int(ad.warnings as i64));
    errs.push_obj("internal", Json::Int(ad.internal_errs as i64));
    root.push_obj("errors", errs);

    root
}

/// Serialize ArchDesc to a JSON string.
pub fn arch_desc_to_string(ad: &ArchDesc) -> String {
    json_to_string(&arch_desc_to_json(ad))
}

/// Serialize ArchDesc to a file.
pub fn write_spec(path: &str, ad: &ArchDesc) -> Result<(), String> {
    std::fs::write(path, arch_desc_to_string(ad)).map_err(|e| format!("{}: {}", path, e))
}

/// Parse entry point used by tests: parse JSON back into a value tree.
pub fn spec_from_string(src: &str) -> Result<Json, String> {
    parse_json(src)
}
