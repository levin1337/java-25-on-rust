#[derive(Clone, Debug)]
pub enum FormatArg {
    Str(&'static str),
    String(std::borrow::Cow<'static, str>),
    Char(char),
    I64(i64),
    U64(u64),
    F64(f64),
    Ptr(usize),
}

impl FormatArg {
    pub fn str(s: &'static str) -> Self {
        Self::Str(s)
    }

    pub fn string(s: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::String(s.into())
    }

    pub fn char(c: char) -> Self {
        Self::Char(c)
    }

    pub fn i64(v: i64) -> Self {
        Self::I64(v)
    }

    pub fn u64(v: u64) -> Self {
        Self::U64(v)
    }

    pub fn isize(v: isize) -> Self {
        Self::I64(v as i64)
    }

    pub fn usize(v: usize) -> Self {
        Self::U64(v as u64)
    }

    pub fn f64(v: f64) -> Self {
        Self::F64(v)
    }

    pub fn ptr(v: usize) -> Self {
        Self::Ptr(v)
    }
}

#[derive(Clone, Copy, Default)]
struct Spec {
    minus: bool,
    plus: bool,
    space: bool,
    hash: bool,
    zero: bool,
    width: usize,
    width_star: bool,
    precision: Option<usize>,
    precision_star: bool,
    conv: char,
}

fn numeric_value(arg: Option<FormatArg>) -> i128 {
    match arg {
        Some(FormatArg::I64(v)) => v as i128,
        Some(FormatArg::U64(v)) => v as i128,
        Some(FormatArg::Ptr(p)) => p as i128,
        Some(FormatArg::F64(v)) => v as i128,
        _ => 0,
    }
}

fn float_value(arg: Option<FormatArg>) -> f64 {
    match arg {
        Some(FormatArg::F64(v)) => v,
        Some(FormatArg::I64(v)) => v as f64,
        Some(FormatArg::U64(v)) => v as f64,
        Some(FormatArg::Ptr(p)) => p as f64,
        _ => 0.0,
    }
}

fn pad(out: &mut String, body: &str, spec: &Spec, zero_pad_ok: bool) {
    let width = spec.width;
    if body.len() >= width {
        out.push_str(body);
        return;
    }
    let pad_len = width - body.len();
    let zero_pad = spec.zero && zero_pad_ok && !spec.minus;
    if spec.minus {
        out.push_str(body);
        for _ in 0..pad_len {
            out.push(if zero_pad { '0' } else { ' ' });
        }
    } else if zero_pad {
        let sign_len = if body.starts_with('-') || body.starts_with('+') || body.starts_with(' ') {
            1
        } else if body.starts_with("0x") || body.starts_with("0X") {
            2
        } else {
            0
        };
        out.push_str(&body[..sign_len]);
        for _ in 0..pad_len {
            out.push('0');
        }
        out.push_str(&body[sign_len..]);
    } else {
        for _ in 0..pad_len {
            out.push(' ');
        }
        out.push_str(body);
    }
}

fn format_integer(value: i128, conv: char, spec: &Spec) -> String {
    let negative = value < 0;
    let mag: u128 = value.unsigned_abs();
    let (digits, alt_prefix) = match conv {
        'd' | 'i' | 'u' => (mag.to_string(), String::new()),
        'o' => {
            let d = format!("{:o}", mag);
            let prefix = if spec.hash && !d.starts_with('0') {
                "0".to_string()
            } else {
                String::new()
            };
            (d, prefix)
        }
        'x' => {
            let d = format!("{:x}", mag);
            let prefix = if spec.hash && mag != 0 { "0x".to_string() } else { String::new() };
            (d, prefix)
        }
        'X' => {
            let d = format!("{:X}", mag);
            let prefix = if spec.hash && mag != 0 { "0X".to_string() } else { String::new() };
            (d, prefix)
        }
        _ => (mag.to_string(), String::new()),
    };
    let precision = spec.precision.unwrap_or(0);
    let mut digits = digits;
    if precision > digits.len() {
        digits = format!("{}{}", "0".repeat(precision - digits.len()), digits);
    }
    if digits == "0" && spec.precision == Some(0) {
        digits.clear();
    }
    let mut body = alt_prefix;
    body.push_str(&digits);
    if negative {
        body.insert(0, '-');
    } else if spec.plus {
        body.insert(0, '+');
    } else if spec.space {
        body.insert(0, ' ');
    }
    let mut out = String::new();
    pad(&mut out, &body, spec, spec.precision.is_none());
    out
}

fn c_exponent(exp: i32, upper: bool) -> String {
    let e = if upper { 'E' } else { 'e' };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{}{}{:02}", e, sign, exp.abs())
}

fn format_float_fixed(value: f64, precision: usize, upper: bool) -> String {
    let s = format!("{:.*}", precision, value.abs());
    if upper {
        s.to_uppercase()
    } else {
        s
    }
}

fn decimal_exponent(value: f64) -> i32 {
    if value == 0.0 {
        return 0;
    }
    value.abs().log10().floor() as i32
}

fn format_float_exp(value: f64, precision: usize, upper: bool) -> String {
    let exp = decimal_exponent(value);
    let mant = value.abs() / 10f64.powi(exp);
    let mut s = format!("{:.*}", precision, mant);
    if precision > 0 {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    let mut out = s;
    out.push_str(&c_exponent(exp, upper));
    if upper {
        out.to_uppercase()
    } else {
        out
    }
}

fn format_float_g(value: f64, precision_in: usize, upper: bool, hash: bool) -> String {
    let precision = if precision_in == 0 { 1 } else { precision_in };
    if value == 0.0 {
        return if hash {
            format!("{:.*}", precision - 1, 0.0f64)
        } else {
            "0".to_string()
        };
    }
    let exp = decimal_exponent(value);
    let use_fixed = exp >= -4 && exp < precision as i32;
    let mut body = if use_fixed {
        let prec = (precision as i32 - 1 - exp).max(0) as usize;
        format_float_fixed(value.abs(), prec, upper)
    } else {
        format_float_exp(value.abs(), precision - 1, upper)
    };
    if !hash {
        if use_fixed {
            if body.contains('.') {
                body = body.trim_end_matches('0').trim_end_matches('.').to_string();
            }
        } else if let Some(epos) = body.find(['e', 'E']) {
            let mut mant = body[..epos].to_string();
            if mant.contains('.') {
                mant = mant.trim_end_matches('0').trim_end_matches('.').to_string();
            }
            body = format!("{}{}", mant, &body[epos..]);
        }
    }
    if upper {
        body.to_uppercase()
    } else {
        body
    }
}

fn format_float_hex(value: f64, precision: usize, upper: bool) -> String {
    let bits = value.to_bits();
    let negative = (bits >> 63) != 0;
    let exp_field = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    if exp_field == 0x7ff {
        return if frac == 0 {
            format!("{}inf", if negative { "-" } else { "" })
        } else {
            format!("{}nan", if negative { "-" } else { "" })
        };
    }
    let (exp, mant_bits) = if exp_field == 0 {
        (-1022, frac)
    } else {
        (exp_field - 1023, frac | (1u64 << 52))
    };
    let int_part = mant_bits >> 52;
    let frac_part = mant_bits & ((1u64 << 52) - 1);
    let mut frac_hex = format!("{:013x}", frac_part);
    if precision < frac_hex.len() {
        frac_hex.truncate(precision);
        while frac_hex.ends_with('0') {
            frac_hex.pop();
        }
    }
    let sign = if negative { "-" } else { "" };
    let esign = if exp < 0 { '-' } else { '+' };
    let body = if frac_hex.is_empty() {
        format!("{}0x{:x}p{}{}", sign, int_part, esign, exp.abs())
    } else {
        format!("{}0x{:x}.{}p{}{}", sign, int_part, frac_hex, esign, exp.abs())
    };
    if upper {
        body.to_uppercase()
    } else {
        body
    }
}

fn format_float(value: f64, spec: &Spec) -> String {
    let precision = spec.precision.unwrap_or(6);
    let upper = spec.conv.is_ascii_uppercase();
    let body = match spec.conv.to_ascii_lowercase() {
        'f' => format_float_fixed(value, precision, upper),
        'e' => format_float_exp(value, precision, upper),
        'g' => format_float_g(value, precision, upper, spec.hash),
        'a' => format_float_hex(value, precision, upper),
        _ => return String::new(),
    };
    let negative = value.is_sign_negative() && !body.starts_with('-');
    let mut body = body;
    if negative {
        body.insert(0, '-');
    } else if value >= 0.0 || value == 0.0 {
        if spec.plus {
            body.insert(0, '+');
        } else if spec.space {
            body.insert(0, ' ');
        }
    }
    let mut out = String::new();
    pad(&mut out, &body, spec, true);
    out
}

fn star_value(args: &[FormatArg], argi: usize) -> i64 {
    match args.get(argi) {
        Some(FormatArg::I64(v)) => *v,
        Some(FormatArg::U64(v)) => *v as i64,
        _ => 0,
    }
}

fn parse_spec(bytes: &[u8], i: &mut usize) -> Option<Spec> {
    let mut spec = Spec::default();
    while *i < bytes.len() {
        match bytes[*i] {
            b'-' => spec.minus = true,
            b'+' => spec.plus = true,
            b' ' => spec.space = true,
            b'#' => spec.hash = true,
            b'0' => spec.zero = true,
            _ => break,
        }
        *i += 1;
    }
    if *i < bytes.len() && bytes[*i] == b'*' {
        spec.width_star = true;
        *i += 1;
    } else {
        let mut w = 0usize;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            w = w.saturating_mul(10).saturating_add((bytes[*i] - b'0') as usize);
            *i += 1;
        }
        spec.width = w;
    }
    if *i < bytes.len() && bytes[*i] == b'.' {
        *i += 1;
        if *i < bytes.len() && bytes[*i] == b'*' {
            spec.precision_star = true;
            *i += 1;
        } else {
            let mut p = 0usize;
            while *i < bytes.len() && bytes[*i].is_ascii_digit() {
                p = p.saturating_mul(10).saturating_add((bytes[*i] - b'0') as usize);
                *i += 1;
            }
            spec.precision = Some(p);
        }
    }
    while *i < bytes.len() && matches!(bytes[*i], b'h' | b'l' | b'j' | b'z' | b't' | b'L' | b'q') {
        *i += 1;
    }
    if *i >= bytes.len() {
        return None;
    }
    spec.conv = bytes[*i] as char;
    *i += 1;
    Some(spec)
}

pub fn format(fmt: &str, args: &[FormatArg]) -> String {
    let bytes = fmt.as_bytes();
    let mut out = String::new();
    let mut i = 0usize;
    let mut argi = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'%' {
                i += 1;
            }
            out.push_str(&fmt[start..i]);
            continue;
        }
        i += 1;
        if i < bytes.len() && bytes[i] == b'%' {
            out.push('%');
            i += 1;
            continue;
        }
        let spec_start = i;
        let Some(mut spec) = parse_spec(bytes, &mut i) else {
            out.push_str(&fmt[i - 1..]);
            break;
        };
        if spec.width_star {
            spec.width = star_value(args, argi).max(0) as usize;
            argi += 1;
        }
        if spec.precision_star {
            spec.precision = Some(star_value(args, argi).max(0) as usize);
            argi += 1;
        }
        let arg = args.get(argi).cloned();
        argi += 1;
        match spec.conv {
            's' => {
                let s: String = match arg {
                    Some(FormatArg::Str(s)) => s.to_string(),
                    Some(FormatArg::String(s)) => s.into_owned(),
                    Some(FormatArg::Char(c)) => c.to_string(),
                    Some(FormatArg::Ptr(p)) => format!("{:p}", p as *const u8),
                    Some(FormatArg::I64(v)) => v.to_string(),
                    Some(FormatArg::U64(v)) => v.to_string(),
                    Some(FormatArg::F64(v)) => v.to_string(),
                    None => String::new(),
                };
                let prec = spec.precision.unwrap_or(usize::MAX);
                let truncated: String = s.chars().take(prec).collect();
                let mut tmp = String::new();
                pad(&mut tmp, &truncated, &spec, false);
                out.push_str(&tmp);
            }
            'c' => {
                let c = match arg {
                    Some(FormatArg::Char(c)) => c,
                    Some(FormatArg::I64(v)) => char::from_u32(v as u32 & 0xff).unwrap_or('\0'),
                    Some(FormatArg::U64(v)) => char::from_u32(v as u32 & 0xff).unwrap_or('\0'),
                    _ => '\0',
                };
                let s = c.to_string();
                let mut tmp = String::new();
                pad(&mut tmp, &s, &spec, false);
                out.push_str(&tmp);
            }
            'p' => {
                let v = match arg {
                    Some(FormatArg::Ptr(p)) => p,
                    Some(FormatArg::U64(v)) => v as usize,
                    Some(FormatArg::I64(v)) => v as usize,
                    _ => 0,
                };
                let s = format!("0x{:x}", v);
                let mut tmp = String::new();
                pad(&mut tmp, &s, &spec, false);
                out.push_str(&tmp);
            }
            'd' | 'i' | 'u' | 'o' | 'x' | 'X' => {
                out.push_str(&format_integer(numeric_value(arg), spec.conv, &spec));
            }
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'a' | 'A' => {
                out.push_str(&format_float(float_value(arg), &spec));
            }
            _ => {
                out.push('%');
                out.push_str(&fmt[spec_start..i]);
            }
        }
    }
    out
}

pub struct FormatBuffer {
    buf: String,
}

impl FormatBuffer {
    pub const BUFFER_SIZE: usize = 256;

    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    pub fn with_format(fmt: &str, args: &[FormatArg]) -> Self {
        let mut b = Self::new();
        b.print(fmt, args);
        b
    }

    pub fn print(&mut self, fmt: &str, args: &[FormatArg]) {
        self.buf.push_str(&format(fmt, args));
        if self.buf.len() > Self::BUFFER_SIZE {
            self.buf.truncate(Self::BUFFER_SIZE.saturating_sub(1));
        }
    }

    pub fn append(&mut self, fmt: &str, args: &[FormatArg]) {
        self.print(fmt, args);
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    pub fn into_string(self) -> String {
        self.buf
    }
}

impl Default for FormatBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FormatBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.buf)
    }
}

pub fn err_msg(fmt: &str, args: &[FormatArg]) -> String {
    let s = format(fmt, args);
    if s.len() > FormatBuffer::BUFFER_SIZE {
        s[..FormatBuffer::BUFFER_SIZE - 1].to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_formats() {
        assert_eq!(format("hello %s", &[FormatArg::str("world")]), "hello world");
        assert_eq!(format("%d %i", &[FormatArg::i64(42), FormatArg::i64(-7)]), "42 -7");
        assert_eq!(
            format("%u %x %X %o", &[FormatArg::u64(255), FormatArg::u64(255), FormatArg::u64(255), FormatArg::u64(8)]),
            "255 ff FF 10"
        );
        assert_eq!(format("%5d|%-5d|%05d", &[FormatArg::i64(42), FormatArg::i64(42), FormatArg::i64(42)]), "   42|42   |00042");
        assert_eq!(format("%.3d", &[FormatArg::i64(5)]), "005");
        assert_eq!(format("%+d % d", &[FormatArg::i64(5), FormatArg::i64(5)]), "+5  5");
        assert_eq!(format("%c%c", &[FormatArg::char('a'), FormatArg::char('b')]), "ab");
        assert_eq!(format("%%"), "%");
        assert_eq!(format("%p", &[FormatArg::ptr(0xdead)]), "0xdead");
    }

    #[test]
    fn string_precision() {
        assert_eq!(format("%.3s", &[FormatArg::str("abcdef")]), "abc");
        assert_eq!(format("%8.3s", &[FormatArg::str("abcdef")]), "     abc");
    }

    #[test]
    fn floats() {
        assert_eq!(format("%f", &[FormatArg::f64(1.5)]), "1.500000");
        assert_eq!(format("%.2f", &[FormatArg::f64(1.5)]), "1.50");
        assert_eq!(format("%e", &[FormatArg::f64(12345.0)]), "1.234500e+04");
        assert_eq!(format("%g", &[FormatArg::f64(0.0001)]), "0.0001");
        assert_eq!(format("%g", &[FormatArg::f64(1000000.0)]), "1e+06");
        assert_eq!(format("%g", &[FormatArg::f64(1.2300)]), "1.23");
        assert_eq!(format("%a", &[FormatArg::f64(1.0)]), "0x1p+0");
        assert_eq!(format("%08.2f", &[FormatArg::f64(-1.5)]), "-0001.50");
    }

    #[test]
    fn star_width() {
        assert_eq!(format("%*d", &[FormatArg::u64(6), FormatArg::i64(42)]), "    42");
        assert_eq!(format("%.*f", &[FormatArg::u64(2), FormatArg::f64(1.23456)]), "1.23");
    }

    #[test]
    fn buffer() {
        let b = FormatBuffer::with_format("%s=%d", &[FormatArg::str("x"), FormatArg::i64(3)]);
        assert_eq!(b.as_str(), "x=3");
        assert_eq!(err_msg("%d", &[FormatArg::i64(9)]), "9");
    }
}
