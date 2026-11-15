use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonType {
    None,
    ObjectBegin,
    ObjectEnd,
    ArrayBegin,
    ArrayEnd,
    Key,
    String,
    NumberInt,
    NumberFloat,
    True,
    False,
    Null,
}

#[derive(Clone, Copy, Debug)]
pub enum JsonVal<'a> {
    Int(i64),
    UInt(u64),
    Double(f64),
    Str(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonError {
    Internal,
    Syntax,
    Key,
    Value,
}

impl JsonError {
    pub fn strerror(&self) -> &'static str {
        match self {
            Self::Syntax => "Syntax error",
            Self::Internal => "Internal error",
            Self::Key => "Key error",
            Self::Value => "Value error",
        }
    }
}

pub trait JsonCallback<'a> {
    fn callback(&mut self, t: JsonType, v: Option<JsonVal<'a>>, level: u32) -> bool;
}

pub struct JsonParser<'a, 'b, C: JsonCallback<'a>> {
    text: &'a str,
    pos: usize,
    mark: usize,
    level: u32,
    line: u32,
    column: u32,
    silent: bool,
    valid: bool,
    errors: Vec<String>,
    callback: &'b mut C,
    st: Option<&'b mut dyn Write>,
}

impl<'a, 'b, C: JsonCallback<'a>> JsonParser<'a, 'b, C> {
    pub fn new(text: &'a str, silent: bool, callback: &'b mut C) -> Self {
        Self {
            text,
            pos: 0,
            mark: 0,
            level: 0,
            line: 1,
            column: 0,
            silent,
            valid: true,
            errors: Vec::new(),
            callback,
            st: None,
        }
    }

    pub fn with_stream(mut self, st: &'b mut dyn Write) -> Self {
        self.st = Some(st);
        self
    }

    pub fn parse(&mut self) -> bool {
        self.valid = self.parse_json_value();
        self.valid
    }

    pub fn valid(&self) -> bool {
        self.valid
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    fn bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    fn error(&mut self, e: JsonError, message: String) {
        self.valid = false;
        let full = format!("{} on line {} byte {}: {}", e.strerror(), self.line, self.column + 1, message);
        self.errors.push(full.clone());
        if !self.silent {
            if let Some(st) = self.st.as_mut() {
                let _ = writeln!(*st, "{}", full);
            }
        }
    }

    fn errorf(&mut self, e: JsonError, fmt: &str) {
        self.error(e, fmt.to_string());
    }

    fn parse_json_value(&mut self) -> bool {
        let c = self.skip_to_token();
        if c == 0 {
            return false;
        }
        if self.level == 0 {
            match c {
                x if x == b'{' as i32 => {
                    if !self.parse_json_object() {
                        return false;
                    }
                    let c = self.skip_to_token();
                    if c > 0 {
                        self.mark_pos();
                        self.errorf(JsonError::Syntax, "Only one top level object/array is allowed.");
                        return false;
                    } else if c < 0 {
                        return false;
                    }
                    return true;
                }
                x if x == b'[' as i32 => {
                    if !self.parse_json_array() {
                        return false;
                    }
                    let c = self.skip_to_token();
                    if c > 0 {
                        self.mark_pos();
                        self.errorf(JsonError::Syntax, "Only one top level object/array is allowed.");
                        return false;
                    } else if c < 0 {
                        return false;
                    }
                    return true;
                }
                0 => {
                    self.errorf(JsonError::Syntax, "EOS was encountered before any json declarations");
                    return false;
                }
                _ => {
                    self.errorf(JsonError::Syntax, "Json must start with an object or an array.");
                    return false;
                }
            }
        }
        match c {
            x if x == b'{' as i32 => self.parse_json_object(),
            x if x == b'[' as i32 => self.parse_json_array(),
            x if x == b'"' as i32 => self.parse_json_string(false),
            x if x == b'-' as i32 || (b'0' as i32..=b'9' as i32).contains(&x) => self.parse_json_number(),
            x if x == b't' as i32 => self.parse_json_symbol("true", JsonType::True),
            x if x == b'f' as i32 => self.parse_json_symbol("false", JsonType::False),
            x if x == b'n' as i32 => self.parse_json_symbol("null", JsonType::Null),
            0 => {
                self.errorf(JsonError::Syntax, "EOS was encountered when expecting a json value.");
                false
            }
            _ => {
                self.errorf(JsonError::Syntax, "Could not parse as a json value (did you forget to quote your strings?).");
                false
            }
        }
    }

    fn parse_json_object(&mut self) -> bool {
        self.mark_pos();
        if self.expect_any("{", "object start", JsonError::Internal) <= 0 {
            return false;
        }
        if !self.callback.callback(JsonType::ObjectBegin, None, {
            self.level += 1;
            self.level - 1
        }) {
            return false;
        }

        loop {
            self.mark_pos();
            let c = self.skip_to_token();
            if c == 0 {
                self.errorf(JsonError::Syntax, "EOS when expecting an object key or object end");
                return false;
            } else if c < 0 {
                return false;
            } else if c == b'}' as i32 {
                self.next();
                break;
            }

            if !self.parse_json_key() {
                return false;
            }

            self.skip_to_token();
            self.mark_pos();
            if self.expect_any(":", "object key-value separator", JsonError::Syntax) <= 0 {
                return false;
            }

            self.skip_to_token();
            self.mark_pos();
            if !self.parse_json_value() {
                return false;
            }

            let c = self.skip_to_token();
            self.mark_pos();
            if self.expect_any(",}", "value separator or object end", JsonError::Syntax) <= 0 {
                return false;
            }
            if c == b'}' as i32 {
                break;
            }
        }

        self.level -= 1;
        self.callback.callback(JsonType::ObjectEnd, None, self.level)
    }

    fn parse_json_array(&mut self) -> bool {
        self.mark_pos();
        if self.expect_any("[", "array start character", JsonError::Internal) <= 0 {
            return false;
        }
        if !self.callback.callback(JsonType::ArrayBegin, None, {
            self.level += 1;
            self.level - 1
        }) {
            return false;
        }

        loop {
            self.mark_pos();
            let c = self.skip_to_token();
            if c == 0 {
                self.errorf(JsonError::Syntax, "EOS when expecting a json value or array end");
                return false;
            } else if c < 0 {
                return false;
            } else if c == b']' as i32 {
                self.next();
                break;
            }

            self.mark_pos();
            if !self.parse_json_value() {
                return false;
            }

            let c = self.skip_to_token();
            self.mark_pos();
            if self.expect_any(",]", "value separator or array end", JsonError::Syntax) <= 0 {
                return false;
            }
            if c == b']' as i32 {
                break;
            }
        }

        self.level -= 1;
        self.callback.callback(JsonType::ArrayEnd, None, self.level)
    }

    fn parse_json_string(&mut self, key: bool) -> bool {
        self.mark_pos();
        if self.expect_any("\"", "string start character", JsonError::Internal) <= 0 {
            return false;
        }

        let start = self.pos;
        let bytes = self.bytes();
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'"' {
            end += 1;
        }
        if end >= bytes.len() {
            self.errorf(JsonError::Syntax, "String started here never ended. Expected '\"' before EOS.");
            return false;
        }

        let v = JsonVal::Str(&self.text[start..end]);
        self.skip(end - self.pos);

        if self.expect_any("\"", "string end character", JsonError::Internal) <= 0 {
            return false;
        }

        let t = if key { JsonType::Key } else { JsonType::String };
        self.callback.callback(t, Some(v), self.level)
    }

    fn is_alpha(c: u8) -> bool {
        c.is_ascii_alphabetic()
    }

    fn is_numeric(c: u8) -> bool {
        c.is_ascii_digit()
    }

    fn is_word(c: u8) -> bool {
        c == b'_' || Self::is_alpha(c) || Self::is_numeric(c)
    }

    fn parse_json_key(&mut self) -> bool {
        self.mark_pos();
        let c = self.peek();
        if c == b'"' as i32 {
            return self.parse_json_string(true);
        }

        let begin = self.pos;
        let c = self.peek();
        if c == 0 {
            self.errorf(JsonError::Syntax, "Got EOS when expecting an object key.");
            return false;
        } else if !Self::is_word(c as u8) {
            self.errorf(JsonError::Syntax, "Expected an object key, which can be a double-quoted (\") string or a simple string (only alphanumeric characters and underscore, separated by whitespace) that doesn't need to be quoted.");
            return false;
        }

        loop {
            let c = self.peek();
            if c <= b' ' as i32 || c == b':' as i32 {
                break;
            } else if !Self::is_word(c as u8) {
                self.errorf(JsonError::Syntax, "Object key need to be quoted, or consist entirely of alphanumeric characters and underscores.");
                return false;
            }
            self.next();
        }

        let v = JsonVal::Str(&self.text[begin..self.pos]);
        self.callback.callback(JsonType::Key, Some(v), self.level)
    }

    fn parse_json_number(&mut self) -> bool {
        self.mark_pos();

        let bytes = self.bytes();
        let start = self.pos;
        let mut end = start;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' || c == b'e' || c == b'E' {
                end += 1;
            } else {
                break;
            }
        }
        let token = &self.text[start..end];
        match token.parse::<f64>() {
            Ok(double_value) => {
                if double_value.fract() == 0.0 && double_value >= i64::MIN as f64 && double_value <= i64::MAX as f64 {
                    let v = JsonVal::Int(double_value as i64);
                    if !self.callback.callback(JsonType::NumberInt, Some(v), self.level) {
                        return false;
                    }
                } else {
                    let v = JsonVal::Double(double_value);
                    if !self.callback.callback(JsonType::NumberFloat, Some(v), self.level) {
                        return false;
                    }
                }
                self.skip(end - self.pos);
                true
            }
            Err(_) => {
                self.errorf(JsonError::Syntax, "Couldn't parse json number (note that exponents are not supported).");
                false
            }
        }
    }

    fn parse_json_symbol(&mut self, name: &str, symbol: JsonType) -> bool {
        if !self.expect_string(name, "maybe you forgot to quote your strings?", JsonError::Syntax) {
            self.mark_pos();
            return false;
        }
        self.callback.callback(symbol, None, self.level)
    }

    fn mark_pos(&mut self) {
        while self.mark < self.pos {
            let c = self.bytes()[self.mark];
            self.mark += 1;
            self.column += 1;
            if c == b'\n' {
                self.line += 1;
                self.column = 0;
            }
        }
    }

    fn next(&mut self) -> i32 {
        let bytes = self.bytes();
        if self.pos >= bytes.len() {
            return 0;
        }
        let c = bytes[self.pos];
        self.pos += 1;
        c as i32
    }

    fn peek(&self) -> i32 {
        let bytes = self.bytes();
        if self.pos >= bytes.len() {
            0
        } else {
            bytes[self.pos] as i32
        }
    }

    fn peek_at(&self, i: usize) -> i32 {
        let bytes = self.bytes();
        let p = self.pos + i;
        if p >= bytes.len() {
            0
        } else {
            bytes[p] as i32
        }
    }

    fn expect_any(&mut self, valid_chars: &str, error_msg: &str, e: JsonError) -> i32 {
        let c = self.peek();
        if c == 0 {
            self.error(e, format!("Got EOS when expecting {} ('{}').", error_msg, valid_chars));
            return 0;
        }
        for &vc in valid_chars.as_bytes() {
            if c == vc as i32 {
                return self.next();
            }
        }
        self.error(e, format!("Expected {} ('{}').", error_msg, valid_chars));
        -1
    }

    fn expect_string(&mut self, expected: &str, error_msg: &str, e: JsonError) -> bool {
        let bytes = self.bytes();
        for (i, &ec) in expected.as_bytes().iter().enumerate() {
            let c = self.peek_at(i);
            if c == 0 {
                self.error(e, format!("EOS encountered when expecting {} (\"{}\")", error_msg, expected));
                return false;
            } else if c != ec as i32 {
                self.error(e, format!("Expected \"{}\" ({})", expected, error_msg));
                return false;
            }
        }
        let _ = bytes;
        self.skip(expected.len());
        true
    }

    fn skip(&mut self, i: usize) -> usize {
        let mut j = i;
        while self.peek() != 0 && j > 0 {
            self.next();
            j -= 1;
        }
        i - j
    }

    fn skip_to_token(&mut self) -> i32 {
        loop {
            let mut c = self.peek();
            if c == b'/' as i32 {
                let c2 = self.peek_at(1);
                if c2 == b'/' as i32 {
                    c = self.skip_line_comment() as i32;
                } else if c2 == b'*' as i32 {
                    c = self.skip_block_comment();
                    if c < 0 {
                        return -1;
                    }
                }
            }
            if c == 0 || c > b' ' as i32 {
                return c;
            }
            self.next();
        }
    }

    fn skip_line_comment(&mut self) -> u8 {
        self.expect_any("/", "line comment start", JsonError::Internal);
        self.expect_any("/", "line comment start", JsonError::Internal);
        loop {
            let c = self.peek();
            if c == 0 {
                return 0;
            }
            if c == b'\n' as i32 {
                self.next();
                return self.peek() as u8;
            }
            self.next();
        }
    }

    fn skip_block_comment(&mut self) -> i32 {
        if self.peek() != b'/' as i32 || self.peek_at(1) != b'*' as i32 {
            self.expect_string("/*", "block comment start", JsonError::Internal);
            return 0;
        }
        let bytes = self.bytes();
        let mut current = self.pos;
        loop {
            while current < bytes.len() && bytes[current] != b'*' {
                current += 1;
            }
            if current >= bytes.len() || current + 1 >= bytes.len() {
                self.mark_pos();
                self.errorf(JsonError::Syntax, "Block comment started here never ended. Expected \"*/\" before EOS.");
                return -1;
            }
            if bytes[current + 1] == b'/' {
                self.pos = current;
                if !self.expect_string("*/", "block comment end", JsonError::Internal) {
                    return -1;
                }
                return self.peek();
            }
            current += 1;
        }
    }
}

pub struct JsonCollector<'a> {
    pub events: Vec<(JsonType, Option<JsonVal<'a>>, u32)>,
}

impl<'a> JsonCollector<'a> {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }
}

impl<'a> Default for JsonCollector<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> JsonCallback<'a> for JsonCollector<'a> {
    fn callback(&mut self, t: JsonType, v: Option<JsonVal<'a>>, level: u32) -> bool {
        self.events.push((t, v, level));
        true
    }
}

pub fn parse_json<'a, C: JsonCallback<'a>>(text: &'a str, callback: &mut C) -> bool {
    let mut p = JsonParser::new(text, true, callback);
    p.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_object() {
        let mut c = JsonCollector::new();
        assert!(parse_json(r#"{"a":1,"b":[true,null,"x",2.5]}"#, &mut c));
        let types: Vec<JsonType> = c.events.iter().map(|e| e.0).collect();
        assert_eq!(
            types,
            vec![
                JsonType::ObjectBegin,
                JsonType::Key,
                JsonType::NumberInt,
                JsonType::Key,
                JsonType::ArrayBegin,
                JsonType::True,
                JsonType::Null,
                JsonType::String,
                JsonType::NumberFloat,
                JsonType::ArrayEnd,
                JsonType::ObjectEnd,
            ]
        );
        match c.events[1].1 {
            Some(JsonVal::Str("a")) => {}
            other => panic!("bad key {:?}", other),
        }
        match c.events[2].1 {
            Some(JsonVal::Int(1)) => {}
            other => panic!("bad int {:?}", other),
        }
    }

    #[test]
    fn unquoted_keys_and_comments() {
        let mut c = JsonCollector::new();
        assert!(parse_json("{key1: // comment\n [1,2,] }", &mut c));
        let keys: Vec<&str> = c
            .events
            .iter()
            .filter(|e| e.0 == JsonType::Key)
            .filter_map(|e| match e.1 {
                Some(JsonVal::Str(s)) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(keys, vec!["key1"]);
    }

    #[test]
    fn trailing_comma() {
        let mut c = JsonCollector::new();
        assert!(parse_json("[1,2,]", &mut c));
    }

    #[test]
    fn errors() {
        let mut c = JsonCollector::new();
        let mut p = JsonParser::new("{bad", true, &mut c);
        assert!(!p.parse());
        let mut c2 = JsonCollector::new();
        let mut p2 = JsonParser::new("42", true, &mut c2);
        assert!(!p2.parse());
        assert!(!p2.errors().is_empty());
    }

    #[test]
    fn block_comment() {
        let mut c = JsonCollector::new();
        assert!(parse_json("/* hi */ {\"x\": /* inner */ [1]}", &mut c));
    }
}
