/*
 * Port of OpenJDK 25 HotSpot diagnosticFramework (share/services/diagnosticFramework.hpp/.cpp)
 *
 * Implements the DCmd diagnostic command framework: CmdLine/DCmdIter command
 * line splitting, DCmdArgIter option/argument tokenizer, DCmdParser, the
 * DCmdFactory registry and the DCmd::Executor entry points used by jcmd and
 * the attach listener.
 */

use crate::share::services::diagnosticArgument::{
    DcmdResult, DcmdValue, DCmdArgument, GenDCmdArgument, MAX_PRINT,
};
use std::io::Write;
use std::sync::{Mutex, OnceLock, RwLock};

pub const DCMD_SOURCE_INTERNAL: u32 = 0x01;
pub const DCMD_SOURCE_ATTACH_API: u32 = 0x02;
pub const DCMD_SOURCE_MBEAN: u32 = 0x04;
pub const FULL_EXPORT: u32 =
    DCMD_SOURCE_INTERNAL | DCMD_SOURCE_ATTACH_API | DCMD_SOURCE_MBEAN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DCmdSource {
    Internal,
    AttachApi,
    MBean,
}

impl DCmdSource {
    pub fn flag(self) -> u32 {
        match self {
            Self::Internal => DCMD_SOURCE_INTERNAL,
            Self::AttachApi => DCMD_SOURCE_ATTACH_API,
            Self::MBean => DCMD_SOURCE_MBEAN,
        }
    }
}

// CmdLine is the class used to handle a command line containing a single
// diagnostic command and its arguments.
pub struct CmdLine {
    cmd: String,
    args: String,
    no_command_name: bool,
}

impl CmdLine {
    pub fn new(line: &str, no_command_name: bool) -> Self {
        let trimmed = line.trim_start();
        if no_command_name {
            return Self {
                cmd: String::new(),
                args: trimmed.to_string(),
                no_command_name: true,
            };
        }
        let cmd_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        Self {
            cmd: trimmed[..cmd_end].to_string(),
            args: trimmed[cmd_end..].to_string(),
            no_command_name: false,
        }
    }

    pub fn args(&self) -> &str {
        &self.args
    }

    pub fn cmd(&self) -> &str {
        &self.cmd
    }

    pub fn is_empty(&self) -> bool {
        self.cmd.is_empty()
    }

    pub fn is_executable(&self) -> bool {
        !self.cmd.is_empty() && !self.cmd.starts_with('#')
    }

    pub fn is_stop(&self) -> bool {
        self.cmd == "stop"
    }
}

pub struct DCmdIter<'a> {
    str_: &'a str,
    cursor: usize,
}

impl<'a> DCmdIter<'a> {
    pub fn new(s: &'a str, _delim: char) -> Self {
        Self { str_: s, cursor: 0 }
    }

    pub fn has_next(&self) -> bool {
        self.cursor < self.str_.len()
    }

    pub fn next(&mut self) -> CmdLine {
        let bytes = self.str_.as_bytes();
        let mut n = self.cursor;
        while n < bytes.len() && bytes[n] != b'\n' {
            n += 1;
        }
        let line = &self.str_[self.cursor..n];
        self.cursor = n + 1;
        CmdLine::new(line, false)
    }
}

// DCmdArgIter iterates over diagnostic command arguments of the form
// [key[=value]] separated by a delimiter, with single/double quote support.
pub struct DCmdArgIter<'a> {
    buffer: &'a [u8],
    cursor: usize,
    key: String,
    value: Option<String>,
    delim: u8,
}

impl<'a> DCmdArgIter<'a> {
    pub fn new(buf: &'a str, delim: char) -> Self {
        Self {
            buffer: buf.as_bytes(),
            cursor: 0,
            key: String::new(),
            value: None,
            delim: delim as u8,
        }
    }

    fn extract_quoted(&mut self) -> DcmdResult<String> {
        let quote = self.buffer[self.cursor];
        self.cursor += 1;
        let start = self.cursor;
        while self.cursor < self.buffer.len() {
            if self.buffer[self.cursor] == quote
                && (self.cursor == 0 || self.buffer[self.cursor - 1] != b'\\')
            {
                let out = String::from_utf8_lossy(&self.buffer[start..self.cursor]).into_owned();
                self.cursor += 1;
                return Ok(out);
            }
            self.cursor += 1;
        }
        Err("Format error in diagnostic command arguments".to_string())
    }

    pub fn next(&mut self) -> DcmdResult<Option<(String, Option<String>)>> {
        let len = self.buffer.len();
        if len == 0 {
            return Ok(None);
        }
        while self.cursor < len.saturating_sub(1) && self.buffer[self.cursor] == self.delim {
            self.cursor += 1;
        }
        if self.cursor == len - 1 && self.buffer[self.cursor] == self.delim {
            self.key = String::new();
            self.value = None;
            return Ok(None);
        }
        if self.cursor >= len {
            return Ok(None);
        }

        let mut key = String::new();
        while self.cursor < len
            && self.buffer[self.cursor] != b'='
            && self.buffer[self.cursor] != self.delim
        {
            if self.buffer[self.cursor] == b'"' || self.buffer[self.cursor] == b'\'' {
                let q = self.extract_quoted()?;
                key.push_str(&q);
                break;
            }
            key.push(self.buffer[self.cursor] as char);
            self.cursor += 1;
        }

        let mut value: Option<String> = None;
        if self.cursor < len && self.buffer[self.cursor] == b'=' {
            self.cursor += 1;
            if self.cursor < len
                && (self.buffer[self.cursor] == b'"' || self.buffer[self.cursor] == b'\'')
            {
                value = Some(self.extract_quoted()?);
            } else {
                let start = self.cursor;
                while self.cursor < len && self.buffer[self.cursor] != self.delim {
                    self.cursor += 1;
                }
                value = Some(String::from_utf8_lossy(&self.buffer[start..self.cursor]).into_owned());
            }
        }

        self.key = key;
        self.value = value;
        if self.key.is_empty() {
            return Ok(None);
        }
        Ok(Some((self.key.clone(), self.value.clone())))
    }
}

#[derive(Clone)]
pub struct DCmdInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub impact: &'static str,
    pub num_arguments: usize,
    pub is_enabled: bool,
}

#[derive(Clone)]
pub struct DCmdArgumentInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub type_name: &'static str,
    pub default_string: Option<&'static str>,
    pub is_mandatory: bool,
    pub is_option: bool,
    pub allow_multiple: bool,
    pub position: i32,
}

pub struct DCmdParser {
    options: Vec<Box<dyn GenDCmdArgument>>,
    arguments_list: Vec<Box<dyn GenDCmdArgument>>,
}

impl Default for DCmdParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DCmdParser {
    pub fn new() -> Self {
        Self {
            options: Vec::new(),
            arguments_list: Vec::new(),
        }
    }

    pub fn add_dcmd_option<T: DcmdValue + Send>(&mut self, arg: DCmdArgument<T>) {
        self.add_option_boxed(Box::new(arg));
    }

    pub fn add_dcmd_argument<T: DcmdValue + Send>(&mut self, arg: DCmdArgument<T>) {
        self.add_argument_boxed(Box::new(arg));
    }

    fn init_boxed(arg: &mut dyn GenDCmdArgument) {
        macro_rules! init_t {
            ($t:ty) => {
                if let Some(a) = arg.as_any_mut().downcast_mut::<DCmdArgument<$t>>() {
                    if let Err(e) = a.init_value() {
                        panic!("Initialization must be successful: {}", e);
                    }
                    return;
                }
            };
        }
        init_t!(i64);
        init_t!(bool);
        init_t!(String);
        init_t!(crate::share::services::diagnosticArgument::NanoTimeArgument);
        init_t!(crate::share::services::diagnosticArgument::MemorySizeArgument);
        init_t!(crate::share::services::diagnosticArgument::StringArrayArgument);
    }

    pub fn add_option_boxed(&mut self, arg: Box<dyn GenDCmdArgument>) {
        let mut a = arg;
        Self::init_boxed(a.as_mut());
        self.options.push(a);
    }

    pub fn add_argument_boxed(&mut self, arg: Box<dyn GenDCmdArgument>) {
        let mut a = arg;
        Self::init_boxed(a.as_mut());
        self.arguments_list.push(a);
    }

    pub fn lookup_dcmd_option(&self, name: &str) -> Option<usize> {
        self.options.iter().position(|a| a.name() == name)
    }

    pub fn parse(&mut self, line: &CmdLine, delim: char) -> DcmdResult<()> {
        let mut next_argument = 0usize;
        let mut iter = DCmdArgIter::new(line.args(), delim);
        while let Some((key, value)) = iter.next()? {
            if let Some(idx) = self.lookup_dcmd_option(&key) {
                self.options[idx].read_value(value.as_deref())?;
            } else if next_argument < self.arguments_list.len() {
                let idx = next_argument;
                next_argument += 1;
                self.arguments_list[idx].read_value(Some(&key))?;
            } else {
                let truncated: String = key.chars().take(MAX_PRINT).collect();
                return Err(format!(
                    "Unknown argument '{}' in diagnostic command.",
                    truncated
                ));
            }
        }
        self.check()
    }

    pub fn check(&self) -> DcmdResult<()> {
        for arg in &self.arguments_list {
            if arg.is_mandatory() && !arg.has_value() {
                return Err(format!("The argument '{}' is mandatory.", arg.name()));
            }
        }
        for arg in &self.options {
            if arg.is_mandatory() && !arg.has_value() {
                return Err(format!("The option '{}' is mandatory.", arg.name()));
            }
        }
        Ok(())
    }

    pub fn print_help(&self, out: &mut dyn Write, cmd_name: &str) -> std::io::Result<()> {
        let options_part = if self.options.is_empty() {
            ""
        } else {
            " [options]"
        };
        write!(out, "Syntax : {} {}", cmd_name, options_part)?;
        for arg in &self.arguments_list {
            if arg.is_mandatory() {
                write!(out, " <{}>", arg.name())?;
            } else {
                write!(out, " [<{}>]", arg.name())?;
            }
        }
        writeln!(out)?;
        if !self.arguments_list.is_empty() {
            writeln!(out, "\nArguments:")?;
            for arg in &self.arguments_list {
                writeln!(
                    out,
                    "\t{} : {} {} ({}, {})",
                    arg.name(),
                    if arg.is_mandatory() { "" } else { "[optional] " },
                    arg.description(),
                    arg.type_name(),
                    if arg.has_default() {
                        arg.default_string().unwrap_or("").to_string()
                    } else {
                        "no default value".to_string()
                    }
                )?;
            }
        }
        if !self.options.is_empty() {
            writeln!(
                out,
                "\nOptions: (options must be specified using the <key> or <key>=<value> syntax)"
            )?;
            for arg in &self.options {
                writeln!(
                    out,
                    "\t{} : {} {} ({}, {})",
                    arg.name(),
                    if arg.is_mandatory() { "" } else { "[optional] " },
                    arg.description(),
                    arg.type_name(),
                    if arg.has_default() {
                        arg.default_string().unwrap_or("").to_string()
                    } else {
                        "no default value".to_string()
                    }
                )?;
            }
        }
        Ok(())
    }

    pub fn reset(&mut self) -> DcmdResult<()> {
        for arg in self.arguments_list.iter_mut().chain(self.options.iter_mut()) {
            Self::reset_boxed(arg.as_mut())?;
        }
        Ok(())
    }

    fn reset_boxed(arg: &mut dyn GenDCmdArgument) -> DcmdResult<()> {
        macro_rules! reset_t {
            ($t:ty) => {
                if let Some(a) = arg.as_any_mut().downcast_mut::<DCmdArgument<$t>>() {
                    a.is_set = false;
                    a.value = <$t as DcmdValue>::default_value();
                    if let Some(d) = a.default_string {
                        a.value = <$t as DcmdValue>::parse_value(Some(d), a.name)?;
                    }
                    a.allow_multiple = <$t as DcmdValue>::allow_multiple_default();
                    return Ok(());
                }
            };
        }
        reset_t!(i64);
        reset_t!(bool);
        reset_t!(String);
        reset_t!(crate::share::services::diagnosticArgument::NanoTimeArgument);
        reset_t!(crate::share::services::diagnosticArgument::MemorySizeArgument);
        reset_t!(crate::share::services::diagnosticArgument::StringArrayArgument);
        Ok(())
    }

    pub fn cleanup(&mut self) {}

    pub fn num_arguments(&self) -> usize {
        self.arguments_list.len() + self.options.len()
    }

    pub fn argument_name_array(&self) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = self
            .arguments_list
            .iter()
            .map(|a| a.name())
            .collect();
        v.extend(self.options.iter().map(|a| a.name()));
        v
    }

    pub fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        let mut out = Vec::new();
        let mut idx = 0i32;
        for arg in &self.arguments_list {
            out.push(DCmdArgumentInfo {
                name: arg.name(),
                description: arg.description(),
                type_name: arg.type_name(),
                default_string: arg.default_string(),
                is_mandatory: arg.is_mandatory(),
                is_option: false,
                allow_multiple: arg.allow_multiple(),
                position: idx,
            });
            idx += 1;
        }
        for arg in &self.options {
            out.push(DCmdArgumentInfo {
                name: arg.name(),
                description: arg.description(),
                type_name: arg.type_name(),
                default_string: arg.default_string(),
                is_mandatory: arg.is_mandatory(),
                is_option: true,
                allow_multiple: arg.allow_multiple(),
                position: -1,
            });
        }
        out
    }

    pub fn option<T: DcmdValue + Clone>(&self, name: &str) -> Option<T> {
        let idx = self.lookup_dcmd_option(name)?;
        self.options[idx]
            .as_any()
            .downcast_ref::<DCmdArgument<T>>()
            .map(|a| a.value.clone())
    }

    pub fn argument<T: DcmdValue + Clone>(&self, index: usize) -> Option<T> {
        self.arguments_list
            .get(index)?
            .as_any()
            .downcast_ref::<DCmdArgument<T>>()
            .map(|a| a.value.clone())
    }

    pub fn argument_is_set(&self, index: usize) -> bool {
        self.arguments_list
            .get(index)
            .map(|a| a.is_set())
            .unwrap_or(false)
    }

    pub fn option_is_set(&self, name: &str) -> bool {
        self.lookup_dcmd_option(name)
            .and_then(|i| self.options.get(i))
            .map(|a| a.is_set())
            .unwrap_or(false)
    }
}

// The DCmd instance interface — one instance per command invocation.
pub trait DCmdInstance: Send {
    fn parse(&mut self, line: &CmdLine, delim: char) -> DcmdResult<()>;
    fn execute(&mut self, source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()>;
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()>;
    fn reset(&mut self) -> DcmdResult<()>;
    fn cleanup(&mut self);
    fn argument_name_array(&self) -> Vec<&'static str>;
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo>;
}

pub trait DCmdFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn impact(&self) -> &'static str;
    fn disabled_message(&self) -> &'static str;
    fn num_arguments(&self) -> usize;
    fn export_flags(&self) -> u32;
    fn is_enabled(&self) -> bool;
    fn is_hidden(&self) -> bool;
    fn create_instance(&self) -> Box<dyn DCmdInstance>;
}

pub struct DCmdFactoryImpl {
    name: &'static str,
    description: &'static str,
    impact: &'static str,
    disabled_message: &'static str,
    num_arguments: usize,
    export_flags: u32,
    enabled: bool,
    hidden: bool,
    ctor: fn() -> Box<dyn DCmdInstance>,
}

impl DCmdFactoryImpl {
    pub fn new(
        name: &'static str,
        description: &'static str,
        impact: &'static str,
        num_arguments: usize,
        export_flags: u32,
        enabled: bool,
        hidden: bool,
        ctor: fn() -> Box<dyn DCmdInstance>,
    ) -> Self {
        Self {
            name,
            description,
            impact,
            disabled_message: "Diagnostic command currently disabled",
            num_arguments,
            export_flags,
            enabled,
            hidden,
            ctor,
        }
    }
}

impl DCmdFactory for DCmdFactoryImpl {
    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn impact(&self) -> &'static str {
        self.impact
    }
    fn disabled_message(&self) -> &'static str {
        self.disabled_message
    }
    fn num_arguments(&self) -> usize {
        self.num_arguments
    }
    fn export_flags(&self) -> u32 {
        self.export_flags
    }
    fn is_enabled(&self) -> bool {
        self.enabled
    }
    fn is_hidden(&self) -> bool {
        self.hidden
    }
    fn create_instance(&self) -> Box<dyn DCmdInstance> {
        (self.ctor)()
    }
}

static DCMD_FACTORY_LIST: OnceLock<RwLock<Vec<Box<dyn DCmdFactory>>>> = OnceLock::new();

fn factories() -> &'static RwLock<Vec<Box<dyn DCmdFactory>>> {
    DCMD_FACTORY_LIST.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn register_dcmd_factory(factory: Box<dyn DCmdFactory>) -> usize {
    let mut list = factories().write().unwrap();
    list.push(factory);
    list.len() - 1
}

pub fn dcmd_list(source: DCmdSource) -> Vec<&'static str> {
    let list = factories().read().unwrap();
    list.iter()
        .filter(|f| !f.is_hidden() && (f.export_flags() & source.flag()) != 0)
        .map(|f| f.name())
        .collect()
}

pub fn dcmd_info_list(source: DCmdSource) -> Vec<DCmdInfo> {
    let list = factories().read().unwrap();
    list.iter()
        .filter(|f| !f.is_hidden() && (f.export_flags() & source.flag()) != 0)
        .map(|f| DCmdInfo {
            name: f.name(),
            description: f.description(),
            impact: f.impact(),
            num_arguments: f.num_arguments(),
            is_enabled: f.is_enabled(),
        })
        .collect()
}

// Returns a snapshot of factory metadata for a command name (used by help).
pub fn factory_snapshot(name: &str) -> Option<(String, String, String, bool, usize)> {
    let list = factories().read().unwrap();
    list.iter()
        .find(|f| f.name() == name)
        .map(|f| {
            (
                f.name().to_string(),
                f.description().to_string(),
                f.impact().to_string(),
                f.is_enabled(),
                f.num_arguments(),
            )
        })
}

pub fn create_instance_for(name: &str) -> Option<Box<dyn DCmdInstance>> {
    let list = factories().read().unwrap();
    list.iter()
        .find(|f| f.name() == name)
        .map(|f| f.create_instance())
}

pub fn factory_is_enabled(name: &str) -> Option<bool> {
    let list = factories().read().unwrap();
    list.iter().find(|f| f.name() == name).map(|f| f.is_enabled())
}

pub fn factory_impact(name: &str) -> Option<&'static str> {
    let list = factories().read().unwrap();
    list.iter().find(|f| f.name() == name).map(|f| f.impact())
}

pub fn factory_num_arguments(name: &str) -> Option<usize> {
    let list = factories().read().unwrap();
    list.iter()
        .find(|f| f.name() == name)
        .map(|f| f.num_arguments())
}

pub fn factory_names_sorted(source: DCmdSource) -> Vec<String> {
    let mut v = dcmd_list(source).into_iter().map(|s| s.to_string()).collect::<Vec<_>>();
    v.sort();
    v
}

pub fn factory_meta(name: &str) -> Option<(String, String, bool)> {
    let list = factories().read().unwrap();
    list.iter()
        .find(|f| f.name() == name)
        .map(|f| (f.description().to_string(), f.impact().to_string(), f.is_enabled()))
}

pub fn with_factory<R>(name: &str, f: impl FnOnce(&dyn DCmdFactory) -> R) -> Option<R> {
    let list = factories().read().unwrap();
    list.iter().find(|fac| fac.name() == name).map(|fac| f(fac.as_ref()))
}

// Mutex kept for API parity with DCmdFactory_lock.
static DCMD_FACTORY_LOCK: Mutex<()> = Mutex::new(());

pub fn with_factory_lock<R>(f: impl FnOnce() -> R) -> R {
    let _g = DCMD_FACTORY_LOCK.lock().unwrap();
    f()
}

// Helper: substitute help options "<cmd> -h|-help|--help" for "help <cmd>".
pub fn reorder_help_cmd(line: &CmdLine) -> Option<String> {
    for token in line.args().split_whitespace() {
        if token == "-h" || token == "--help" || token == "-help" {
            return Some(format!("help {}", line.cmd()));
        }
    }
    None
}

// DCmd::Executor — parses and executes commands from a command line.
pub struct Executor<'a> {
    pub source: DCmdSource,
    pub out: &'a mut dyn Write,
    pub allow_streaming: bool,
    pub pre_execute: Option<Box<dyn FnMut() + Send + 'a>>,
}

impl<'a> Executor<'a> {
    pub fn new(source: DCmdSource, out: &'a mut dyn Write) -> Self {
        Self {
            source,
            out,
            allow_streaming: true,
            pre_execute: None,
        }
    }

    pub fn parse_and_execute(&mut self, cmdline: &str, delim: char) -> DcmdResult<()> {
        if cmdline.is_empty() {
            return Ok(());
        }
        let mut iter = DCmdIter::new(cmdline, '\n');
        let mut count = 0;
        while iter.has_next() {
            if self.source == DCmdSource::MBean && count > 0 {
                return Err("Invalid syntax".to_string());
            }
            let mut line = iter.next();
            if line.is_stop() {
                break;
            }
            if line.is_executable() {
                if let Some(updated) = reorder_help_cmd(&line) {
                    line = CmdLine::new(&updated, false);
                }
                let cmd_name = line.cmd().to_string();
                let mut command = create_local_dcmd(self.source, &cmd_name)?;
                command.parse(&line, delim)?;
                self.execute(command.as_mut())?;
            }
            count += 1;
        }
        Ok(())
    }

    fn execute(&mut self, command: &mut dyn DCmdInstance) -> DcmdResult<()> {
        if let Some(hook) = self.pre_execute.as_mut() {
            hook();
        }
        command.execute(self.source, self.out)
    }
}

pub fn parse_and_execute(
    source: DCmdSource,
    out: &mut dyn Write,
    cmdline: &str,
    delim: char,
) -> DcmdResult<()> {
    Executor::new(source, out).parse_and_execute(cmdline, delim)
}

pub fn create_local_dcmd(
    source: DCmdSource,
    cmd_name: &str,
) -> DcmdResult<Box<dyn DCmdInstance>> {
    let list = factories().read().unwrap();
    for f in list.iter() {
        if f.name() == cmd_name {
            if f.export_flags() & source.flag() == 0 {
                return Err("Unknown diagnostic command".to_string());
            }
            if !f.is_enabled() {
                return Err(f.disabled_message().to_string());
            }
            return Ok(f.create_instance());
        }
    }
    Err("Unknown diagnostic command".to_string())
}
