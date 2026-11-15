/*
 * Port of OpenJDK 25 HotSpot CompilerOracle (share/compiler/compilerOracle.hpp/.cpp)
 *
 * Implements -XX:CompileCommand=<option>,<method pattern>[,<value>] rules:
 * per-method control over inlining, exclusion, compilation, logging and
 * other compiler flags, with wildcard method patterns and typed option
 * values.  A global singleton is provided via `CompilerOracle::global()`.
 */

use std::sync::{OnceLock, RwLock};

/// `CompileCommandEnum`: every supported -XX:CompileCommand option.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum CompileCommandEnum {
    Help = 0,
    Quiet,
    Log,
    Print,
    Inline,
    DontInline,
    Blackhole,
    CompileOnly,
    Exclude,
    Break,
    BreakAtExecute,
    BreakAtCompile,
    MemLimit,
    MemStat,
    PrintAssembly,
    PrintCompilation,
    PrintInlining,
    PrintIntrinsics,
    PrintNMethods,
    PrintOptoAssembly,
    PrintDebugInfo,
    PrintRelocations,
    PrintDependencies,
    BackgroundCompilation,
    RepeatCompilation,
    ReplayInline,
    DumpReplay,
    DumpInline,
    CompileThresholdScaling,
    ControlIntrinsic,
    DisableIntrinsic,
    BlockLayoutByFrequency,
    TraceOptoPipelining,
    TraceOptoOutput,
    TraceSpilling,
    Vectorize,
    CloneMapDebug,
    IncrementalInlineForceCleanup,
    MaxNodeLimit,
    Option,
    Unknown,
    Count,
}

impl CompileCommandEnum {
    pub const ALL: &'static [(&'static str, CompileCommandEnum, OptionType)] = &[
        ("help", Self::Help, OptionType::Unknown),
        ("quiet", Self::Quiet, OptionType::Unknown),
        ("log", Self::Log, OptionType::Bool),
        ("print", Self::Print, OptionType::Bool),
        ("inline", Self::Inline, OptionType::Bool),
        ("dontinline", Self::DontInline, OptionType::Bool),
        ("blackhole", Self::Blackhole, OptionType::Bool),
        ("compileonly", Self::CompileOnly, OptionType::Bool),
        ("exclude", Self::Exclude, OptionType::Bool),
        ("break", Self::Break, OptionType::Bool),
        ("breakatexecute", Self::BreakAtExecute, OptionType::Bool),
        ("breakatcompile", Self::BreakAtCompile, OptionType::Bool),
        ("memlimit", Self::MemLimit, OptionType::Intx),
        ("memstat", Self::MemStat, OptionType::Uintx),
        ("printassembly", Self::PrintAssembly, OptionType::Bool),
        ("printcompilation", Self::PrintCompilation, OptionType::Bool),
        ("printinlining", Self::PrintInlining, OptionType::Bool),
        ("printintrinsics", Self::PrintIntrinsics, OptionType::Bool),
        ("printnmethods", Self::PrintNMethods, OptionType::Bool),
        ("printoptoassembly", Self::PrintOptoAssembly, OptionType::Bool),
        ("printdebuginfo", Self::PrintDebugInfo, OptionType::Bool),
        ("printrelocations", Self::PrintRelocations, OptionType::Bool),
        ("printdependencies", Self::PrintDependencies, OptionType::Bool),
        ("backgroundcompilation", Self::BackgroundCompilation, OptionType::Bool),
        ("repeatcompilation", Self::RepeatCompilation, OptionType::Intx),
        ("replayinline", Self::ReplayInline, OptionType::Bool),
        ("dumpreplay", Self::DumpReplay, OptionType::Bool),
        ("dumpinline", Self::DumpInline, OptionType::Bool),
        ("compilethresholdscaling", Self::CompileThresholdScaling, OptionType::Double),
        ("controlintrinsic", Self::ControlIntrinsic, OptionType::CcstrList),
        ("disableintrinsic", Self::DisableIntrinsic, OptionType::CcstrList),
        ("blocklayoutbyfrequency", Self::BlockLayoutByFrequency, OptionType::Bool),
        ("traceoptopipelining", Self::TraceOptoPipelining, OptionType::Bool),
        ("traceoptooutput", Self::TraceOptoOutput, OptionType::Bool),
        ("tracespilling", Self::TraceSpilling, OptionType::Bool),
        ("vectorize", Self::Vectorize, OptionType::Bool),
        ("clonemapdebug", Self::CloneMapDebug, OptionType::Bool),
        (
            "incrementalinlineforcecleanup",
            Self::IncrementalInlineForceCleanup,
            OptionType::Bool,
        ),
        ("maxnodelimit", Self::MaxNodeLimit, OptionType::Intx),
        ("option", Self::Option, OptionType::Unknown),
        ("unknown", Self::Unknown, OptionType::Unknown),
    ];

    pub fn name(&self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, e, _)| e == self)
            .map(|(n, _, _)| *n)
            .unwrap_or("unknown")
    }

    pub fn option_type(&self) -> OptionType {
        Self::ALL
            .iter()
            .find(|(_, e, _)| e == self)
            .map(|(_, _, t)| *t)
            .unwrap_or(OptionType::Unknown)
    }

    /// Case-insensitive name -> option (`string_to_option`).
    pub fn from_name(name: &str) -> Self {
        Self::ALL
            .iter()
            .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, e, _)| *e)
            .unwrap_or(Self::Unknown)
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OptionType {
    Intx,
    Uintx,
    Bool,
    Ccstr,
    CcstrList,
    Double,
    Unknown,
}

impl OptionType {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Intx => "intx",
            Self::Uintx => "uintx",
            Self::Bool => "bool",
            Self::Ccstr => "ccstr",
            Self::CcstrList => "ccstrlist",
            Self::Double => "double",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "intx" => Self::Intx,
            "uintx" => Self::Uintx,
            "bool" => Self::Bool,
            "ccstr" => Self::Ccstr,
            "ccstrlist" => Self::CcstrList,
            "double" => Self::Double,
            _ => Self::Unknown,
        }
    }
}

/// Typed option value carried by a CompileCommand rule.
#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Str(String),
    Double(f64),
}

impl OptionValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Int(v) => Some(*v != 0),
            Self::UInt(v) => Some(*v != 0),
            Self::Str(s) => match s.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            },
            Self::Double(v) => Some(*v != 0.0),
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::UInt(v) => i64::try_from(*v).ok(),
            Self::Bool(b) => Some(*b as i64),
            Self::Str(s) => s.trim().parse::<i64>().ok(),
            Self::Double(v) => Some(*v as i64),
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::UInt(v) => Some(*v),
            Self::Int(v) if *v >= 0 => Some(*v as u64),
            Self::Bool(b) => Some(*b as u64),
            Self::Str(s) => s.trim().parse::<u64>().ok(),
            Self::Double(v) if *v >= 0.0 => Some(*v as u64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Double(v) => Some(*v),
            Self::Int(v) => Some(*v as f64),
            Self::UInt(v) => Some(*v as f64),
            Self::Bool(b) => Some(*b as i64 as f64),
            Self::Str(s) => s.trim().parse::<f64>().ok(),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            Self::Bool(b) => None,
            _ => None,
        }
    }
}

/// One `-XX:CompileCommand` rule.
#[derive(Clone, Debug)]
pub struct OracleRule {
    pub option: CompileCommandEnum,
    pub value: Option<OptionValue>,
    /// Method pattern in HotSpot syntax, e.g. `java/lang/String.charAt` or
    /// `pkg/Class.method (signature)`; wildcards `*` allowed.
    pub method_pattern: String,
    /// Precompiled matcher.
    pub matcher: MethodMatcher,
}

impl OracleRule {
    pub fn matches(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        self.matcher.matches(class_name, method_name, signature)
    }
}

/// `MethodMatcher` analogue: matches `package/Class.method(signature)`
/// patterns with `*` wildcards on any component.
#[derive(Clone, Debug)]
pub struct MethodMatcher {
    /// Pattern split into (class part, method part, optional signature).
    class_pattern: String,
    method_pattern: String,
    /// None = don't check signature; Some(pattern) with possible `*`.
    signature_pattern: Option<String>,
    /// Whether the pattern had an explicit signature.
    has_signature: bool,
}

impl MethodMatcher {
    /// Parse a pattern like `java/lang/String.charAt`,
    /// `java/lang/String.charAt()I`, `*.*`, `java/util/*.* (I)I`.
    pub fn new(pattern: &str) -> Self {
        let p = pattern.trim();
        // Normalize `::` to `.` (HotSpot accepts both).
        let p = p.replace("::", ".");

        // Extract optional signature in parentheses at the end.
        let (body, signature_pattern, has_signature) = if p.ends_with(')') {
            if let Some(open) = p.rfind('(') {
                let sig = &p[open + 1..p.len() - 1];
                (&p[..open], Some(sig.to_string()), true)
            } else {
                (p.as_str(), None, false)
            }
        } else {
            (p.as_str(), None, false)
        };

        // Split class.method at the last '.' not inside signature.
        let (class_pattern, method_pattern) = match body.rfind('.') {
            Some(idx) => (body[..idx].to_string(), body[idx + 1..].to_string()),
            None => (String::new(), body.to_string()),
        };

        Self {
            class_pattern,
            method_pattern,
            signature_pattern,
            has_signature,
        }
    }

    fn wildcard_match(pat: &str, text: &str) -> bool {
        // HotSpot MethodMatcher semantics: '*' matches any sequence;
        // components without '*' must match exactly.
        if pat == "*" || pat.is_empty() {
            return true;
        }
        if !pat.contains('*') {
            return pat == text;
        }
        // Recursive wildcard match (handles multiple '*').
        fn rec(p: &[u8], t: &[u8]) -> bool {
            if p.is_empty() {
                return t.is_empty();
            }
            if p[0] == b'*' {
                // collapse consecutive '*'
                let mut i = 0;
                while i < p.len() && p[i] == b'*' {
                    i += 1;
                }
                if i == p.len() {
                    return true;
                }
                for j in 0..=t.len() {
                    if rec(&p[i..], &t[j..]) {
                        return true;
                    }
                }
                false
            } else {
                !t.is_empty() && p[0] == t[0] && rec(&p[1..], &t[1..])
            }
        }
        rec(pat.as_bytes(), text.as_bytes())
    }

    pub fn matches(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        if !Self::wildcard_match(&self.class_pattern, class_name) {
            return false;
        }
        if !Self::wildcard_match(&self.method_pattern, method_name) {
            return false;
        }
        if self.has_signature {
            match &self.signature_pattern {
                Some(sig_pat) => Self::wildcard_match(sig_pat, signature),
                None => true,
            }
        } else {
            true
        }
    }
}

/// What the oracle decided for an inline candidate
/// (`what_to_inline` analogue used by C1/C2 inlining).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineDecision {
    /// No directive: normal inlining heuristics apply.
    Normal,
    /// `inline` option matched.
    Inline,
    /// `dontinline` (or `exclude`) matched: never inline.
    DontInline,
    /// `print` matched: inline but log the decision.
    Print,
    /// `break` matched: inline then break into debugger.
    Break,
}

pub struct CompilerOracle {
    rules: Vec<OracleRule>,
    quiet: bool,
    any_set: bool,
}

impl CompilerOracle {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            quiet: false,
            any_set: false,
        }
    }

    pub fn quiet(&self) -> bool {
        self.quiet
    }

    pub fn set_quiet(&mut self, quiet: bool) {
        self.quiet = quiet;
    }

    pub fn has_any_command_set(&self) -> bool {
        self.any_set
    }

    /// Register a typed option rule.
    pub fn register(
        &mut self,
        option: CompileCommandEnum,
        method_pattern: &str,
        value: Option<OptionValue>,
    ) {
        self.rules.push(OracleRule {
            option,
            value,
            method_pattern: method_pattern.to_string(),
            matcher: MethodMatcher::new(method_pattern),
        });
        self.any_set = true;
    }

    /// Boolean shorthand (`-XX:CompileCommand=exclude,foo.Bar.baz`).
    pub fn register_bool(&mut self, option: CompileCommandEnum, method_pattern: &str, value: bool) {
        self.register(option, method_pattern, Some(OptionValue::Bool(value)));
    }

    /// Parse `-XX:CompileCommand=<option>,<method pattern>[,<value>]`
    /// or a `.hotspot_compiler` file line `<option> <method pattern>[, <value>]`.
    pub fn parse_compile_command(&mut self, spec: &str) -> Result<(), String> {
        let spec = spec.trim();
        if spec.is_empty() || spec.starts_with('#') {
            return Ok(());
        }

        // File format: "option pattern[, value]" (space separated).
        // Flag format: "option,pattern[,value]" (comma separated).
        let (option_str, rest) = if let Some((o, r)) = spec.split_once(',') {
            (o.trim(), r.trim())
        } else {
            let mut it = spec.splitn(2, char::is_whitespace);
            let o = it.next().unwrap_or("");
            let r = it.next().unwrap_or("").trim();
            (o, r)
        };

        if option_str.eq_ignore_ascii_case("help") {
            return Err("help".to_string());
        }
        if option_str.eq_ignore_ascii_case("quiet") {
            self.quiet = true;
            return Ok(());
        }

        let option = CompileCommandEnum::from_name(option_str);
        if option.is_unknown() {
            return Err(format!("Unrecognized option '{}'", option_str));
        }

        // Split pattern and optional value: pattern may contain commas
        // inside signature? No — signatures use parens, commas separate
        // value.  HotSpot splits at the LAST comma for typed options.
        let opt_type = option.option_type();
        let (pattern, value_str) = match opt_type {
            OptionType::Bool | OptionType::Unknown => {
                // Boolean options: value optional trailing bool.
                if let Some((p, v)) = rest.rsplit_once(',') {
                    let p = p.trim();
                    let v = v.trim();
                    if v.eq_ignore_ascii_case("true")
                        || v.eq_ignore_ascii_case("false")
                        || v == "1"
                        || v == "0"
                    {
                        (p, Some(v.to_string()))
                    } else {
                        (rest, None)
                    }
                } else {
                    (rest, None)
                }
            }
            _ => {
                // Typed options: last comma separates the value.
                match rest.rsplit_once(',') {
                    Some((p, v)) => (p.trim(), Some(v.trim().to_string())),
                    None => (rest, None),
                }
            }
        };

        if pattern.is_empty() {
            return Err("empty method pattern".to_string());
        }

        let value = match value_str {
            Some(v) => Some(parse_option_value(opt_type, &v)?),
            None => {
                if opt_type == OptionType::Bool || opt_type == OptionType::Unknown {
                    Some(OptionValue::Bool(true))
                } else {
                    None
                }
            }
        };

        self.register(option, pattern, value);
        Ok(())
    }

    /// All rules matching a method, in registration order (later rules
    /// override earlier ones for the same option, like HotSpot).
    fn matching_rules<'a>(
        &'a self,
        class_name: &str,
        method_name: &str,
        signature: &str,
        option: CompileCommandEnum,
    ) -> Vec<&'a OracleRule> {
        self.rules
            .iter()
            .filter(|r| r.option == option && r.matches(class_name, method_name, signature))
            .collect()
    }

    /// `CompilerOracle::has_option_value` for bool options.
    pub fn has_option(&self, class_name: &str, method_name: &str, signature: &str, option: CompileCommandEnum) -> bool {
        self.matching_rules(class_name, method_name, signature, option)
            .last()
            .and_then(|r| r.value.as_ref().and_then(|v| v.as_bool()))
            .unwrap_or(false)
    }

    pub fn has_option_value_bool(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
        option: CompileCommandEnum,
    ) -> Option<bool> {
        self.matching_rules(class_name, method_name, signature, option)
            .last()
            .and_then(|r| r.value.as_ref().and_then(|v| v.as_bool()))
    }

    pub fn option_value_f64(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
        option: CompileCommandEnum,
    ) -> Option<f64> {
        self.matching_rules(class_name, method_name, signature, option)
            .last()
            .and_then(|r| r.value.as_ref().and_then(|v| v.as_f64()))
    }

    pub fn option_value_i64(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
        option: CompileCommandEnum,
    ) -> Option<i64> {
        self.matching_rules(class_name, method_name, signature, option)
            .last()
            .and_then(|r| r.value.as_ref().and_then(|v| v.as_i64()))
    }

    pub fn option_value_str<'a>(
        &'a self,
        class_name: &str,
        method_name: &str,
        signature: &str,
        option: CompileCommandEnum,
    ) -> Option<&'a str> {
        self.matching_rules(class_name, method_name, signature, option)
            .last()
            .and_then(|r| r.value.as_ref().and_then(|v| v.as_str()))
    }

    /// Whether any `compileonly` command exists at all
    /// (`has_command(CompileOnly)`).
    pub fn has_compile_only(&self) -> bool {
        self.rules.iter().any(|r| r.option == CompileCommandEnum::CompileOnly)
    }

    /// `CompilerOracle::should_exclude`.
    pub fn should_exclude(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        if self.has_option(class_name, method_name, signature, CompileCommandEnum::Exclude) {
            return true;
        }
        if self.has_compile_only()
            && !self.has_option(class_name, method_name, signature, CompileCommandEnum::CompileOnly)
        {
            return true;
        }
        false
    }

    /// `resolve_inlining_predicate` + `should_inline` /
    /// `should_not_inline`: later rules win for inline/dontinline;
    /// exclude also blocks inlining.
    pub fn inline_decision(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
    ) -> InlineDecision {
        // Scan in order; the last matching directive wins (HotSpot
        // processes commands in order, later overriding earlier).
        let mut decision = InlineDecision::Normal;
        for r in &self.rules {
            if !r.matches(class_name, method_name, signature) {
                continue;
            }
            match r.option {
                CompileCommandEnum::Inline => {
                    decision = match r.value.as_ref().and_then(|v| v.as_bool()) {
                        Some(false) => InlineDecision::Normal,
                        _ => InlineDecision::Inline,
                    };
                }
                CompileCommandEnum::DontInline => {
                    decision = match r.value.as_ref().and_then(|v| v.as_bool()) {
                        Some(false) => InlineDecision::Normal,
                        _ => InlineDecision::DontInline,
                    };
                }
                CompileCommandEnum::Exclude => {
                    if r.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(true) {
                        decision = InlineDecision::DontInline;
                    }
                }
                CompileCommandEnum::Print => {
                    if decision == InlineDecision::Normal
                        && r.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(true)
                    {
                        decision = InlineDecision::Print;
                    }
                }
                CompileCommandEnum::Break => {
                    if decision == InlineDecision::Normal
                        && r.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(true)
                    {
                        decision = InlineDecision::Break;
                    }
                }
                _ => {}
            }
        }
        decision
    }

    pub fn should_inline(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        matches!(self.inline_decision(class_name, method_name, signature), InlineDecision::Inline)
    }

    pub fn should_not_inline(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        matches!(
            self.inline_decision(class_name, method_name, signature),
            InlineDecision::DontInline
        )
    }

    pub fn should_print(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        self.has_option(class_name, method_name, signature, CompileCommandEnum::Print)
            || matches!(
                self.inline_decision(class_name, method_name, signature),
                InlineDecision::Print
            )
    }

    pub fn should_log(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        // -XX:+LogCompilation default: log everything unless `log` rules
        // restrict it.
        let has_log_cmd = self
            .rules
            .iter()
            .any(|r| r.option == CompileCommandEnum::Log);
        if !has_log_cmd {
            return true;
        }
        self.has_option(class_name, method_name, signature, CompileCommandEnum::Log)
    }

    pub fn should_break_at(&self, class_name: &str, method_name: &str, signature: &str) -> bool {
        self.has_option(class_name, method_name, signature, CompileCommandEnum::Break)
            || self.has_option(
                class_name,
                method_name,
                signature,
                CompileCommandEnum::BreakAtCompile,
            )
    }

    /// `CompileThresholdScaling` option value for threshold scaling.
    pub fn compile_threshold_scaling(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
    ) -> Option<f64> {
        self.option_value_f64(
            class_name,
            method_name,
            signature,
            CompileCommandEnum::CompileThresholdScaling,
        )
    }

    /// Print all registered commands (`print_commands` analogue).
    pub fn print_commands(&self, f: &mut dyn std::fmt::Write) {
        for r in &self.rules {
            let val = match &r.value {
                Some(v) => format!(",{:?}", v),
                None => String::new(),
            };
            let _ = writeln!(f, "{} {}{}", r.option.name(), r.method_pattern, val);
        }
    }
}

fn parse_option_value(t: OptionType, v: &str) -> Result<OptionValue, String> {
    match t {
        OptionType::Bool | OptionType::Unknown => {
            let b = match v.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => return Err(format!("invalid bool value '{}'", v)),
            };
            Ok(OptionValue::Bool(b))
        }
        OptionType::Intx => v
            .trim()
            .parse::<i64>()
            .map(OptionValue::Int)
            .map_err(|_| format!("invalid intx value '{}'", v)),
        OptionType::Uintx => v
            .trim()
            .parse::<u64>()
            .map(OptionValue::UInt)
            .map_err(|_| format!("invalid uintx value '{}'", v)),
        OptionType::Double => v
            .trim()
            .parse::<f64>()
            .map(OptionValue::Double)
            .map_err(|_| format!("invalid double value '{}'", v)),
        OptionType::Ccstr | OptionType::CcstrList => Ok(OptionValue::Str(v.to_string())),
    }
}

impl Default for CompilerOracle {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_ORACLE: OnceLock<RwLock<CompilerOracle>> = OnceLock::new();

impl CompilerOracle {
    /// Process-wide oracle instance, populated from
    /// `-XX:CompileCommand` / `.hotspot_compiler` at startup.
    pub fn global() -> &'static RwLock<CompilerOracle> {
        GLOBAL_ORACLE.get_or_init(|| RwLock::new(CompilerOracle::new()))
    }

    /// Convenience: evaluate `should_exclude` on the global oracle.
    pub fn global_should_exclude(class_name: &str, method_name: &str, signature: &str) -> bool {
        Self::global()
            .read()
            .unwrap()
            .should_exclude(class_name, method_name, signature)
    }

    /// Convenience: `inline_decision` on the global oracle.
    pub fn global_inline_decision(
        class_name: &str,
        method_name: &str,
        signature: &str,
    ) -> InlineDecision {
        Self::global()
            .read()
            .unwrap()
            .inline_decision(class_name, method_name, signature)
    }
}
