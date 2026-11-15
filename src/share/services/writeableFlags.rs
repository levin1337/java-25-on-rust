/*
 * Port of OpenJDK 25 HotSpot WriteableFlags (share/services/writeableFlags.hpp/.cpp)
 *
 * Sets writeable JVM flags through the attach/jcmd interface. The backing
 * flag table mirrors the JVMFlag model (runtime/flags/jvmFlag.hpp); until the
 * runtime flag subsystem is ported, the table is seeded with the flags the
 * Rust VM actually honours (the Arguments-parsed subset) and can be extended
 * via JvmFlag::register.
 */

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmFlagOrigin {
    CommandLine,
    Ergonometric,
    EnvironmentVariable,
    ConfigFile,
    Api,
    Management,
    AttachOnDemand,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmFlagType {
    Bool,
    Int,
    Uint,
    Intx,
    Uintx,
    Uint64,
    SizeT,
    Double,
    Ccstr,
}

impl JvmFlagType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Uint => "uint",
            Self::Intx => "intx",
            Self::Uintx => "uintx",
            Self::Uint64 => "uint64_t",
            Self::SizeT => "size_t",
            Self::Double => "double",
            Self::Ccstr => "ccstr",
        }
    }
}

#[derive(Clone)]
pub enum JvmFlagValue {
    Bool(bool),
    Int(i32),
    Uint(u32),
    Intx(i64),
    Uintx(u64),
    Uint64(u64),
    SizeT(usize),
    Double(f64),
    Ccstr(Option<String>),
}

impl JvmFlagValue {
    pub fn as_bool(&self) -> bool {
        matches!(self, Self::Bool(true))
    }
    pub fn as_i64(&self) -> i64 {
        match self {
            Self::Int(v) => *v as i64,
            Self::Uint(v) => *v as i64,
            Self::Intx(v) => *v,
            Self::Uintx(v) => *v as i64,
            Self::Uint64(v) => *v as i64,
            Self::SizeT(v) => *v as i64,
            Self::Double(v) => *v as i64,
            _ => 0,
        }
    }
    pub fn as_f64(&self) -> f64 {
        match self {
            Self::Double(v) => *v,
            Self::Int(v) => *v as f64,
            Self::Uint(v) => *v as f64,
            Self::Intx(v) => *v as f64,
            Self::Uintx(v) => *v as f64,
            Self::Uint64(v) => *v as f64,
            Self::SizeT(v) => *v as f64,
            _ => 0.0,
        }
    }
    pub fn as_str(&self) -> String {
        match self {
            Self::Bool(v) => {
                if *v {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            Self::Int(v) => format!("{}", v),
            Self::Uint(v) => format!("{}", v),
            Self::Intx(v) => format!("{}", v),
            Self::Uintx(v) => format!("{}", v),
            Self::Uint64(v) => format!("{}", v),
            Self::SizeT(v) => format!("{}", v),
            Self::Double(v) => format!("{}", v),
            Self::Ccstr(v) => v.clone().unwrap_or_default(),
        }
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Bool(_))
    }
    pub fn is_int(&self) -> bool {
        matches!(self, Self::Int(_))
    }
    pub fn is_uint(&self) -> bool {
        matches!(self, Self::Uint(_))
    }
    pub fn is_intx(&self) -> bool {
        matches!(self, Self::Intx(_))
    }
    pub fn is_uintx(&self) -> bool {
        matches!(self, Self::Uintx(_))
    }
    pub fn is_uint64_t(&self) -> bool {
        matches!(self, Self::Uint64(_))
    }
    pub fn is_size_t(&self) -> bool {
        matches!(self, Self::SizeT(_))
    }
    pub fn is_double(&self) -> bool {
        matches!(self, Self::Double(_))
    }
    pub fn is_ccstr(&self) -> bool {
        matches!(self, Self::Ccstr(_))
    }
}

#[derive(Clone)]
pub struct JvmFlag {
    pub name: &'static str,
    pub kind: JvmFlagType,
    pub writeable: bool,
    pub value: JvmFlagValue,
    pub origin: JvmFlagOrigin,
    pub min_i64: Option<i64>,
    pub max_i64: Option<i64>,
}

static FLAG_TABLE: OnceLock<Mutex<HashMap<&'static str, JvmFlag>>> = OnceLock::new();

fn table() -> &'static Mutex<HashMap<&'static str, JvmFlag>> {
    FLAG_TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn seed_flags() {
    let mut t = table().lock().unwrap();
    if !t.is_empty() {
        return;
    }
    let mut add = |name: &'static str, kind: JvmFlagType, writeable: bool, value: JvmFlagValue| {
        t.insert(
            name,
            JvmFlag {
                name,
                kind,
                writeable,
                value,
                origin: JvmFlagOrigin::Ergonometric,
                min_i64: None,
                max_i64: None,
            },
        );
    };
    // Flags honoured by the Rust VM (Arguments-parsed subset, defaults from
    // Arguments::default()).
    add("UseG1GC", JvmFlagType::Bool, true, JvmFlagValue::Bool(true));
    add("UseParallelGC", JvmFlagType::Bool, false, JvmFlagValue::Bool(false));
    add("UseSerialGC", JvmFlagType::Bool, false, JvmFlagValue::Bool(false));
    add("UseEpsilonGC", JvmFlagType::Bool, false, JvmFlagValue::Bool(false));
    add("UseZGC", JvmFlagType::Bool, false, JvmFlagValue::Bool(false));
    add("UseShenandoahGC", JvmFlagType::Bool, false, JvmFlagValue::Bool(false));
    add("TieredCompilation", JvmFlagType::Bool, true, JvmFlagValue::Bool(true));
    add(
        "TieredStopAtLevel",
        JvmFlagType::Int,
        true,
        JvmFlagValue::Int(4),
    );
    add(
        "InitialHeapSize",
        JvmFlagType::SizeT,
        false,
        JvmFlagValue::SizeT(128 * 1024 * 1024),
    );
    add(
        "MaxHeapSize",
        JvmFlagType::SizeT,
        false,
        JvmFlagValue::SizeT(512 * 1024 * 1024),
    );
    add(
        "ThreadStackSize",
        JvmFlagType::SizeT,
        false,
        JvmFlagValue::SizeT(1024 * 1024),
    );
    add(
        "MaxMetaspaceSize",
        JvmFlagType::SizeT,
        false,
        JvmFlagValue::SizeT(256 * 1024 * 1024),
    );
    add("NativeMemoryTracking", JvmFlagType::Ccstr, true, JvmFlagValue::Ccstr(None));
    add("StartAttachListener", JvmFlagType::Bool, false, JvmFlagValue::Bool(true));
    add("DisableAttachMechanism", JvmFlagType::Bool, false, JvmFlagValue::Bool(false));
}

impl JvmFlag {
    pub fn seed() {
        seed_flags();
    }

    pub fn find_flag(name: &str) -> Option<JvmFlag> {
        seed_flags();
        table().lock().unwrap().get(name).cloned()
    }

    pub fn register(flag: JvmFlag) {
        seed_flags();
        table().lock().unwrap().insert(flag.name, flag);
    }

    pub fn is_writeable(&self) -> bool {
        self.writeable
    }
    pub fn is_bool(&self) -> bool {
        self.value.is_bool()
    }
    pub fn is_int(&self) -> bool {
        self.value.is_int()
    }
    pub fn is_uint(&self) -> bool {
        self.value.is_uint()
    }
    pub fn is_intx(&self) -> bool {
        self.value.is_intx()
    }
    pub fn is_uintx(&self) -> bool {
        self.value.is_uintx()
    }
    pub fn is_uint64_t(&self) -> bool {
        self.value.is_uint64_t()
    }
    pub fn is_size_t(&self) -> bool {
        self.value.is_size_t()
    }
    pub fn is_double(&self) -> bool {
        self.value.is_double()
    }
    pub fn is_ccstr(&self) -> bool {
        self.value.is_ccstr()
    }

    pub fn set_value(&mut self, value: JvmFlagValue) {
        self.value = value;
    }

    pub fn print_as_flag(&self) -> String {
        if self.is_bool() {
            format!(
                "-XX:{}{}",
                if self.value.as_bool() { "+" } else { "-" },
                self.name
            )
        } else {
            format!("-XX:{}={}", self.name, self.value.as_str())
        }
    }

    pub fn all_flags() -> Vec<JvmFlag> {
        seed_flags();
        let mut v: Vec<JvmFlag> = table().lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(b.name));
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmFlagError {
    Success,
    MissingName,
    MissingValue,
    NonWritable,
    OutOfBounds,
    ViolatesConstraint,
    InvalidFlag,
    WrongFormat,
    ErrOther,
}

impl JvmFlagError {
    pub fn is_success(&self) -> bool {
        *self == Self::Success
    }
}

pub struct WriteableFlags;

fn buffer_concat(buffer: &mut String, src: &str) {
    if buffer.len() < 79 {
        let room = 79 - buffer.len();
        buffer.push_str(&src[..src.len().min(room)]);
    }
}

fn print_flag_error_message_if_needed(
    error: JvmFlagError,
    flag_name: Option<&str>,
    flag: Option<&JvmFlag>,
) -> String {
    if error == JvmFlagError::Success {
        return String::new();
    }
    let mut buffer = String::new();
    match (error, flag_name) {
        (JvmFlagError::MissingName, _) | (_, None) => buffer_concat(&mut buffer, "Error: "),
        _ => {
            buffer_concat(&mut buffer, flag_name.unwrap_or(""));
            buffer_concat(&mut buffer, " error: ");
        }
    }
    match error {
        JvmFlagError::MissingName => buffer_concat(&mut buffer, "flag name is missing."),
        JvmFlagError::MissingValue => {
            buffer_concat(&mut buffer, "parsing the textual form of the value.")
        }
        JvmFlagError::NonWritable => buffer_concat(&mut buffer, "flag is not writeable."),
        JvmFlagError::OutOfBounds => {
            if let Some(f) = flag {
                buffer_concat(&mut buffer, "must have value in range ");
                if let (Some(lo), Some(hi)) = (f.min_i64, f.max_i64) {
                    buffer_concat(&mut buffer, &format!("[{}, {}]", lo, hi));
                }
            }
        }
        JvmFlagError::ViolatesConstraint => {
            buffer_concat(&mut buffer, "value violates its flag's constraint.")
        }
        JvmFlagError::InvalidFlag => {
            buffer_concat(&mut buffer, "there is no flag with the given name.")
        }
        JvmFlagError::ErrOther => {
            buffer_concat(&mut buffer, "other, unspecified error related to setting the flag.")
        }
        _ => {}
    }
    buffer
}

fn check_bounds(flag: &JvmFlag, value: i64) -> JvmFlagError {
    if let Some(lo) = flag.min_i64 {
        if value < lo {
            return JvmFlagError::OutOfBounds;
        }
    }
    if let Some(hi) = flag.max_i64 {
        if value > hi {
            return JvmFlagError::OutOfBounds;
        }
    }
    JvmFlagError::Success
}

impl WriteableFlags {
    fn set_bool_flag(name: &str, arg: &str, origin: JvmFlagOrigin) -> (JvmFlagError, String) {
        let value = if arg.eq_ignore_ascii_case("true") || arg == "1" {
            true
        } else if arg.eq_ignore_ascii_case("false") || arg == "0" {
            false
        } else {
            return (
                JvmFlagError::WrongFormat,
                "flag value must be a boolean (1/0 or true/false)".to_string(),
            );
        };
        WriteableFlags::set_typed(name, JvmFlagValue::Bool(value), origin)
    }

    fn set_int_like<TParse, TApply>(
        name: &str,
        arg: &str,
        origin: JvmFlagOrigin,
        err_msg: &str,
        parse: TParse,
        apply: TApply,
    ) -> (JvmFlagError, String)
    where
        TParse: Fn(&str) -> Option<i64>,
        TApply: Fn(i64) -> JvmFlagValue,
    {
        match parse(arg) {
            Some(v) => WriteableFlags::set_typed(name, apply(v), origin),
            None => (JvmFlagError::WrongFormat, err_msg.to_string()),
        }
    }

    fn set_ccstr_flag(
        name: &str,
        value: &str,
        origin: JvmFlagOrigin,
    ) -> (JvmFlagError, String) {
        WriteableFlags::set_typed(name, JvmFlagValue::Ccstr(Some(value.to_string())), origin)
    }

    fn set_typed(
        name: &str,
        value: JvmFlagValue,
        origin: JvmFlagOrigin,
    ) -> (JvmFlagError, String) {
        let flag = JvmFlag::find_flag(name);
        let Some(mut f) = flag else {
            return (
                JvmFlagError::InvalidFlag,
                print_flag_error_message_if_needed(JvmFlagError::InvalidFlag, Some(name), None),
            );
        };
        if !f.is_writeable() {
            return (
                JvmFlagError::NonWritable,
                print_flag_error_message_if_needed(JvmFlagError::NonWritable, Some(name), Some(&f)),
            );
        }
        let numeric = value.as_i64();
        let err = check_bounds(&f, numeric);
        if err != JvmFlagError::Success {
            return (
                err,
                print_flag_error_message_if_needed(err, Some(name), Some(&f)),
            );
        }
        f.set_value(value);
        f.origin = origin;
        JvmFlag::register(f.clone());
        (JvmFlagError::Success, String::new())
    }

    pub fn set_flag_from_char(
        flag: &JvmFlag,
        value: Option<&str>,
        origin: JvmFlagOrigin,
    ) -> (JvmFlagError, String) {
        let Some(flag_value) = value else {
            return (
                JvmFlagError::MissingValue,
                "flag value is missing".to_string(),
            );
        };
        let name = flag.name;
        if flag.is_bool() {
            return WriteableFlags::set_bool_flag(name, flag_value, origin);
        } else if flag.is_int() {
            return WriteableFlags::set_int_like(
                name,
                flag_value,
                origin,
                "flag value must be an integer",
                |s| s.trim().parse::<i32>().ok().map(|v| v as i64),
                |v| JvmFlagValue::Int(v as i32),
            );
        } else if flag.is_uint() {
            return WriteableFlags::set_int_like(
                name,
                flag_value,
                origin,
                "flag value must be an unsigned integer",
                |s| s.trim().parse::<u32>().ok().map(|v| v as i64),
                |v| JvmFlagValue::Uint(v as u32),
            );
        } else if flag.is_intx() {
            return WriteableFlags::set_int_like(
                name,
                flag_value,
                origin,
                "flag value must be an integer",
                |s| s.trim().parse::<i64>().ok(),
                JvmFlagValue::Intx,
            );
        } else if flag.is_uintx() {
            return WriteableFlags::set_int_like(
                name,
                flag_value,
                origin,
                "flag value must be an unsigned integer",
                |s| s.trim().parse::<u64>().ok().map(|v| v as i64),
                |v| JvmFlagValue::Uintx(v as u64),
            );
        } else if flag.is_uint64_t() {
            return WriteableFlags::set_int_like(
                name,
                flag_value,
                origin,
                "flag value must be an unsigned 64-bit integer",
                |s| s.trim().parse::<u64>().ok().map(|v| v as i64),
                |v| JvmFlagValue::Uint64(v as u64),
            );
        } else if flag.is_size_t() {
            return WriteableFlags::set_int_like(
                name,
                flag_value,
                origin,
                "flag value must be an unsigned integer",
                |s| s.trim().parse::<usize>().ok().map(|v| v as i64),
                |v| JvmFlagValue::SizeT(v as usize),
            );
        } else if flag.is_double() {
            return match flag_value.trim().parse::<f64>() {
                Ok(v) => WriteableFlags::set_typed(name, JvmFlagValue::Double(v), origin),
                Err(_) => (
                    JvmFlagError::WrongFormat,
                    "flag value must be a double".to_string(),
                ),
            };
        } else if flag.is_ccstr() {
            return WriteableFlags::set_ccstr_flag(name, flag_value, origin);
        }
        (JvmFlagError::ErrOther, String::new())
    }

    /// sets a writeable flag to the provided value
    pub fn set_flag(
        flag_name: Option<&str>,
        flag_value: Option<&str>,
        origin: JvmFlagOrigin,
    ) -> (JvmFlagError, String) {
        let Some(name) = flag_name else {
            return (JvmFlagError::MissingName, "flag name is missing".to_string());
        };
        if flag_value.is_none() {
            return (JvmFlagError::MissingValue, "flag value is missing".to_string());
        }
        let Some(f) = JvmFlag::find_flag(name) else {
            return (
                JvmFlagError::InvalidFlag,
                print_flag_error_message_if_needed(JvmFlagError::InvalidFlag, Some(name), None),
            );
        };
        if !f.is_writeable() {
            return (
                JvmFlagError::NonWritable,
                print_flag_error_message_if_needed(JvmFlagError::NonWritable, Some(name), Some(&f)),
            );
        }
        WriteableFlags::set_flag_from_char(&f, flag_value, origin)
    }

    pub fn print_set_flags(out: &mut dyn std::io::Write) -> std::io::Result<()> {
        for f in JvmFlag::all_flags() {
            writeln!(out, "{}", f.print_as_flag())?;
        }
        Ok(())
    }

    pub fn print_flags(out: &mut dyn std::io::Write, all: bool) -> std::io::Result<()> {
        for f in JvmFlag::all_flags() {
            if !all && !f.is_writeable() {
                continue;
            }
            writeln!(
                out,
                "{} = {} {kind}",
                f.name,
                f.value.as_str(),
                kind = f.kind.name()
            )?;
        }
        Ok(())
    }
}
