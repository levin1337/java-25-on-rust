/*
 * Port of OpenJDK 25 HotSpot diagnosticArgument (share/services/diagnosticArgument.hpp/.cpp)
 *
 * Typed argument values used by the diagnostic command parser:
 * jlong, bool, char* (STRING/FILE), NanoTimeArgument, MemorySizeArgument
 * and StringArrayArgument. Values are stored inside DCmdArgument<T>
 * and exposed to commands through the GenDCmdArgument interface.
 */

use std::any::Any;

pub const MAX_PRINT: usize = 64;

pub type DcmdResult<T> = Result<T, String>;

fn illegal_argument(msg: String) -> String {
    msg
}

pub trait DcmdValue: Clone + 'static {
    fn default_value() -> Self;
    fn parse_value(s: Option<&str>, name: &str) -> DcmdResult<Self>
    where
        Self: Sized;
    fn to_display(&self) -> String;
    fn allow_multiple_default() -> bool {
        false
    }
    fn merge_value(existing: &mut Self, s: Option<&str>, name: &str) -> DcmdResult<()> {
        *existing = Self::parse_value(s, name)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct StringArrayArgument {
    pub array: Vec<String>,
}

impl StringArrayArgument {
    pub fn new() -> Self {
        Self { array: Vec::new() }
    }

    pub fn add(&mut self, s: &str) {
        self.array.push(s.to_string());
    }
}

impl Default for StringArrayArgument {
    fn default() -> Self {
        Self::new()
    }
}

impl DcmdValue for StringArrayArgument {
    fn default_value() -> Self { Self::default() }
    fn allow_multiple_default() -> bool {
        true
    }
    fn parse_value(s: Option<&str>, _name: &str) -> DcmdResult<Self> {
        let mut arr = StringArrayArgument::new();
        if let Some(v) = s {
            arr.add(v);
        }
        Ok(arr)
    }

    fn to_display(&self) -> String {
        self.array.join(",")
    }

    fn merge_value(existing: &mut Self, s: Option<&str>, _name: &str) -> DcmdResult<()> {
        if let Some(v) = s {
            existing.add(v);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct NanoTimeArgument {
    pub nanotime: i64,
    pub time: i64,
    pub unit: String,
}

impl Default for NanoTimeArgument {
    fn default() -> Self {
        Self {
            nanotime: 0,
            time: 0,
            unit: "ns".to_string(),
        }
    }
}

impl DcmdValue for NanoTimeArgument {
    fn default_value() -> Self { Self::default() }
    fn parse_value(s: Option<&str>, name: &str) -> DcmdResult<Self> {
        let str_ = s.ok_or_else(|| {
            illegal_argument(
                "Integer parsing error nanotime value: syntax error, value is null\n".to_string(),
            )
        })?;

        let digit_len = str_.chars().take_while(|c| c.is_ascii_digit()).count();
        let time_part = &str_[..digit_len];
        let unit_part = &str_[digit_len..];

        let time: i64 = if time_part.is_empty() {
            return Err(illegal_argument(
                "Integer parsing error nanotime value: syntax error\n".to_string(),
            ));
        } else {
            time_part.parse::<i64>().map_err(|_| {
                illegal_argument("Integer parsing error nanotime value: syntax error\n".to_string())
            })?
        };

        if unit_part.is_empty() {
            if time != 0 {
                return Err(illegal_argument(
                    "Integer parsing error nanotime value: unit required\n".to_string(),
                ));
            }
            return Ok(NanoTimeArgument {
                nanotime: 0,
                time,
                unit: "ns".to_string(),
            });
        }
        if unit_part.len() > 2 {
            return Err(illegal_argument(
                "Integer parsing error nanotime value: illegal unit\n".to_string(),
            ));
        }

        let multiplier: i64 = match unit_part {
            "ns" => 1,
            "us" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60 * 1_000_000_000,
            "h" => 60 * 60 * 1_000_000_000,
            "d" => 24 * 60 * 60 * 1_000_000_000,
            _ => {
                return Err(illegal_argument(
                    "Integer parsing error nanotime value: illegal unit\n".to_string(),
                ))
            }
        };

        let _ = name;
        Ok(NanoTimeArgument {
            nanotime: time.saturating_mul(multiplier),
            time,
            unit: unit_part.to_string(),
        })
    }

    fn to_display(&self) -> String {
        format!("{}", self.nanotime)
    }
}

#[derive(Clone)]
pub struct MemorySizeArgument {
    pub size: u64,
    pub val: u64,
    pub multiplier: char,
}

impl Default for MemorySizeArgument {
    fn default() -> Self {
        Self {
            size: 0,
            val: 0,
            multiplier: ' ',
        }
    }
}

impl DcmdValue for MemorySizeArgument {
    fn default_value() -> Self { Self::default() }
    fn parse_value(s: Option<&str>, _name: &str) -> DcmdResult<Self> {
        let str_ = s.ok_or_else(|| {
            illegal_argument(
                "Parsing error memory size value: syntax error, value is null\n".to_string(),
            )
        })?;
        if str_.starts_with('-') {
            return Err(illegal_argument(
                "Parsing error memory size value: negative values not allowed\n".to_string(),
            ));
        }
        let bytes = str_.as_bytes();
        let mut idx = 0;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        let val: u64 = if idx == 0 {
            return Err(illegal_argument(
                "Parsing error memory size value: invalid value\n".to_string(),
            ));
        } else {
            str_[..idx].parse::<u64>().map_err(|_| {
                illegal_argument(
                    "Parsing error memory size value: invalid value\n".to_string(),
                )
            })?
        };
        let mut arg = MemorySizeArgument {
            size: val,
            val,
            multiplier: ' ',
        };
        if idx < bytes.len() {
            let mult = bytes[idx] as char;
            match mult {
                'k' | 'K' => arg.size = val.saturating_mul(1024),
                'm' | 'M' => arg.size = val.saturating_mul(1024 * 1024),
                'g' | 'G' => arg.size = val.saturating_mul(1024 * 1024 * 1024),
                _ => {
                    arg.multiplier = ' ';
                }
            }
            if matches!(mult, 'k' | 'K' | 'm' | 'M' | 'g' | 'G') {
                arg.multiplier = mult;
            }
        }
        Ok(arg)
    }

    fn to_display(&self) -> String {
        format!("{}", self.size)
    }
}

impl DcmdValue for i64 {
    fn default_value() -> Self { 0 }
    fn parse_value(s: Option<&str>, name: &str) -> DcmdResult<Self> {
        let str_ = s.ok_or_else(|| {
            illegal_argument(format!(
                "Integer parsing error in command argument '{}'. Could not parse: <null>.\n",
                truncate(name)
            ))
        })?;
        let trimmed = str_.trim();
        if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit() || b == b'-' || b == b'+')
        {
            return Err(illegal_argument(format!(
                "Integer parsing error in command argument '{}'. Could not parse: {}{}.\n",
                truncate(name),
                truncate_value(trimmed),
                if trimmed.len() > MAX_PRINT { "..." } else { "" }
            )));
        }
        trimmed.parse::<i64>().map_err(|_| {
            illegal_argument(format!(
                "Integer parsing error in command argument '{}'. Could not parse: {}{}.\n",
                truncate(name),
                truncate_value(trimmed),
                if trimmed.len() > MAX_PRINT { "..." } else { "" }
            ))
        })
    }

    fn to_display(&self) -> String {
        format!("{}", self)
    }
}

impl DcmdValue for bool {
    fn default_value() -> Self { false }
    fn parse_value(s: Option<&str>, name: &str) -> DcmdResult<Self> {
        match s {
            None => Ok(true),
            Some("") => Ok(true),
            Some(v) => {
                if v.eq_ignore_ascii_case("true") {
                    Ok(true)
                } else if v.eq_ignore_ascii_case("false") {
                    Ok(false)
                } else {
                    Err(illegal_argument(format!(
                        "Boolean parsing error in command argument '{}'. Could not parse: {}.\n",
                        truncate(name),
                        truncate_value(v)
                    )))
                }
            }
        }
    }

    fn to_display(&self) -> String {
        if *self { "true" } else { "false" }.to_string()
    }
}

impl DcmdValue for String {
    fn default_value() -> Self { String::new() }
    fn parse_value(s: Option<&str>, _name: &str) -> DcmdResult<Self> {
        Ok(s.unwrap_or("").to_string())
    }

    fn to_display(&self) -> String {
        self.clone()
    }
}

fn truncate(s: &str) -> String {
    if s.len() > MAX_PRINT {
        s[..MAX_PRINT].to_string()
    } else {
        s.to_string()
    }
}

fn truncate_value(s: &str) -> String {
    truncate(s)
}

pub struct DCmdArgument<T: DcmdValue> {
    pub name: &'static str,
    pub description: &'static str,
    pub type_name: &'static str,
    pub default_string: Option<&'static str>,
    pub is_set: bool,
    pub is_mandatory: bool,
    pub allow_multiple: bool,
    pub value: T,
}

impl<T: DcmdValue> DCmdArgument<T> {
    pub fn new(name: &'static str, description: &'static str, type_name: &'static str, mandatory: bool) -> Self {
        Self {
            name,
            description,
            type_name,
            default_string: None,
            is_set: false,
            is_mandatory: mandatory,
            allow_multiple: false,
            value: T::default_value(),
        }
    }

    pub fn with_default(
        name: &'static str,
        description: &'static str,
        type_name: &'static str,
        mandatory: bool,
        default: &'static str,
    ) -> Self {
        let mut arg = Self::new(name, description, type_name, mandatory);
        arg.default_string = Some(default);
        arg
    }

    pub fn init_value(&mut self) -> DcmdResult<()> {
        if let Some(d) = self.default_string {
            let parsed = T::parse_value(Some(d), self.name)?;
            self.value = parsed;
        } else {
            self.value = T::default_value();
        }
        self.allow_multiple = T::allow_multiple_default();
        Ok(())
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }

    pub fn read_typed(&mut self, s: Option<&str>) -> DcmdResult<()> {
        if self.is_set && !self.allow_multiple {
            return Err("Duplicates in diagnostic command arguments\n".to_string());
        }
        T::merge_value(&mut self.value, s, self.name)?;
        self.is_set = true;
        Ok(())
    }
}

unsafe impl<T: DcmdValue + Send> Send for DCmdArgument<T> {}

impl<T: DcmdValue + Send> GenDCmdArgument for DCmdArgument<T> {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn type_name(&self) -> &'static str {
        self.type_name
    }
    fn default_string(&self) -> Option<&'static str> {
        self.default_string
    }
    fn is_set(&self) -> bool {
        self.is_set
    }
    fn is_mandatory(&self) -> bool {
        self.is_mandatory
    }
    fn allow_multiple(&self) -> bool {
        self.allow_multiple
    }
    fn read_value(&mut self, value: Option<&str>) -> DcmdResult<()> {
        self.read_typed(value)
    }
    fn value_as_str(&self) -> String {
        self.value.to_display()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub trait GenDCmdArgument: Send {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn type_name(&self) -> &'static str;
    fn default_string(&self) -> Option<&'static str>;
    fn is_set(&self) -> bool;
    fn is_mandatory(&self) -> bool;
    fn allow_multiple(&self) -> bool;
    fn has_value(&self) -> bool {
        self.is_set() || self.default_string().is_some()
    }
    fn has_default(&self) -> bool {
        self.default_string().is_some()
    }
    fn read_value(&mut self, value: Option<&str>) -> DcmdResult<()>;
    fn value_as_str(&self) -> String;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub fn get_argument<T: DcmdValue + Default>(arg: &dyn GenDCmdArgument) -> Option<T> {
    arg.as_any()
        .downcast_ref::<DCmdArgument<T>>()
        .map(|a| a.value.clone())
}
