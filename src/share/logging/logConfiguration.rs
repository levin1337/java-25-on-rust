/*
 * Port of OpenJDK 25 HotSpot LogConfiguration (share/logging/logConfiguration.hpp)
 *
 * LogConfiguration configures log output destinations, tag selectors, and log
 * levels (-Xlog). Supports the HotSpot selection syntax:
 *
 *   -Xlog:what[.what...][=level][,...][:output-options]
 *
 *   what      — tag selectors: gc+init=debug, class*, all=warning
 *   output    — file=path | output=stderr|stdout|file | decorations=list
 *
 * Examples:
 *   gc+init=info:file=vm.log:time,uptime
 *   all=warning
 *   class+load=debug,os=trace:output=stderr:uptime,level,tags
 */

use super::logDecorations::Decorations;
use super::logLevel::LogLevel;
use super::logOutput::{FileOutput, LogOutput, NullOutput, StderrOutput, StdoutOutput};
use super::logTag::LogTag;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Where a configured output writes.
enum OutputKind {
    Stderr,
    Stdout,
    File(PathBuf),
    Null,
}
use std::path::PathBuf;

struct OutputEntry {
    kind: OutputKind,
    decorations: Decorations,
}

pub struct LogConfiguration {
    tag_levels: Mutex<HashMap<LogTag, LogLevel>>,
    default_level: LogLevel,
    /// Configured outputs; empty means "default stderr".
    outputs: Mutex<Vec<OutputEntry>>,
    /// Last parse errors (surfaced via describe()).
    errors: Mutex<Vec<String>>,
}

static GLOBAL_LOG_CONFIG: OnceLock<LogConfiguration> = OnceLock::new();

impl LogConfiguration {
    pub fn new() -> Self {
        Self {
            tag_levels: Mutex::new(HashMap::new()),
            default_level: LogLevel::Info,
            outputs: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
        }
    }

    pub fn global() -> &'static LogConfiguration {
        GLOBAL_LOG_CONFIG.get_or_init(Self::new)
    }

    /// Backwards-compatible simple parser: "gc=info,class+load=debug".
    pub fn parse_log_argument(&self, arg: &str) -> Result<(), String> {
        for part in arg.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (tag_str, level_str) = if let Some(eq_pos) = part.find('=') {
                (&part[..eq_pos], &part[eq_pos + 1..])
            } else {
                (part, "info")
            };
            let level = LogLevel::from_str(level_str)
                .ok_or_else(|| format!("Invalid log level '{}'", level_str))?;
            for t_str in tag_str.split('+') {
                let tag = LogTag::from_str(t_str)
                    .ok_or_else(|| format!("Invalid log tag '{}'", t_str))?;
                self.set_level(tag, level);
            }
        }
        Ok(())
    }

    /// Full -Xlog parser: selectors[:output-options].
    /// `arg` is the text after "-Xlog:" (or "Xlog:" prefix already stripped).
    pub fn parse_xlog(&self, arg: &str) -> Result<(), String> {
        self.errors.lock().unwrap().clear();
        // Split selector section from output options on the LAST ':' that is
        // followed by output-looking content. HotSpot uses ':' to separate
        // output options; selectors never contain ':'.
        let (selectors, options) = match arg.find(':') {
            Some(pos) => (&arg[..pos], Some(&arg[pos + 1..])),
            None => (arg, None),
        };

        // Parse tag selectors.
        for part in selectors.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (tag_str, level_str) = if let Some(eq_pos) = part.find('=') {
                (&part[..eq_pos], &part[eq_pos + 1..])
            } else {
                (part, "info")
            };
            let level = match level_str {
                "" => LogLevel::Info,
                s => LogLevel::from_str(s)
                    .ok_or_else(|| format!("Invalid log level '{}'", s))?,
            };
            for t_str in tag_str.split('+') {
                // Support wildcard suffix "class*".
                let t_str = t_str.trim();
                if t_str.is_empty() {
                    continue;
                }
                let tag = if let Some(prefix) = t_str.strip_suffix('*') {
                    LogTag::from_prefix(prefix)
                        .ok_or_else(|| format!("Invalid log tag '{}'", t_str))?
                } else {
                    LogTag::from_str(t_str)
                        .ok_or_else(|| format!("Invalid log tag '{}'", t_str))?
                };
                self.set_level(tag, level);
            }
        }

        // Parse output options.
        if let Some(opts) = options {
            let mut kind = OutputKind::Stderr;
            let mut decorations = Decorations::none();
            let mut saw_output = false;
            for opt in opts.split(',') {
                let opt = opt.trim();
                if opt.is_empty() {
                    continue;
                }
                if let Some((k, v)) = opt.split_once('=') {
                    match k.trim().to_ascii_lowercase().as_str() {
                        "file" => {
                            kind = OutputKind::File(PathBuf::from(v.trim()));
                            saw_output = true;
                        }
                        "output" => {
                            kind = match v.trim().to_ascii_lowercase().as_str() {
                                "stderr" | "tty" => OutputKind::Stderr,
                                "stdout" => OutputKind::Stdout,
                                "file" | "file=" => OutputKind::Stderr, // needs file=
                                "null" => OutputKind::Null,
                                other => {
                                    self.errors.lock().unwrap().push(format!(
                                        "unknown output '{}'",
                                        other
                                    ));
                                    OutputKind::Stderr
                                }
                            };
                            saw_output = true;
                        }
                        "decorations" => {
                            decorations = Decorations::parse(v);
                        }
                        _ => {
                            self.errors.lock().unwrap().push(format!(
                                "unknown output option '{}'",
                                k
                            ));
                        }
                    }
                } else {
                    // Bare decoration names: "time,uptime,level,tags".
                    decorations = Decorations::parse(opt);
                }
            }
            if saw_output || !decorations_eq_none(&decorations) {
                self.outputs.lock().unwrap().push(OutputEntry {
                    kind,
                    decorations,
                });
            }
        }
        Ok(())
    }

    pub fn set_level(&self, tag: LogTag, level: LogLevel) {
        self.tag_levels.lock().unwrap().insert(tag, level);
    }

    pub fn is_level(&self, tag: LogTag, level: LogLevel) -> bool {
        let guard = self.tag_levels.lock().unwrap();
        let target_level = guard
            .get(&tag)
            .or_else(|| guard.get(&LogTag::All))
            .copied()
            .unwrap_or(self.default_level);

        level <= target_level && target_level != LogLevel::Off
    }

    /// All configured outputs; defaults to stderr when none configured.
    /// Returns owned trait objects (File outputs are opened lazily on first
    /// write via OnceLock inside each entry).
    pub fn outputs(&self) -> Vec<Box<dyn LogOutput>> {
        let entries = self.outputs.lock().unwrap();
        if entries.is_empty() {
            return vec![Box::new(StderrOutput)];
        }
        entries
            .iter()
            .map(|e| match &e.kind {
                OutputKind::Stderr => Box::new(StderrOutput) as Box<dyn LogOutput>,
                OutputKind::Stdout => Box::new(StdoutOutput),
                OutputKind::File(p) => match FileOutput::create(p) {
                    Ok(f) => Box::new(f) as Box<dyn LogOutput>,
                    Err(_) => Box::new(NullOutput),
                },
                OutputKind::Null => Box::new(NullOutput),
            })
            .collect()
    }

    /// Decorations for the i-th configured output (stderr default when none).
    pub fn decorations_for(&self, index: usize) -> Decorations {
        let entries = self.outputs.lock().unwrap();
        if entries.is_empty() {
            return Decorations::default_stderr();
        }
        entries
            .get(index)
            .map(|e| e.decorations)
            .unwrap_or_else(Decorations::default_stderr)
    }

    /// Number of configured outputs (1 = implicit stderr default).
    pub fn output_count(&self) -> usize {
        let n = self.outputs.lock().unwrap().len();
        if n == 0 {
            1
        } else {
            n
        }
    }

    /// Parse errors from the last parse_xlog call.
    pub fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }

    pub fn reset(&self) {
        self.tag_levels.lock().unwrap().clear();
        self.outputs.lock().unwrap().clear();
        self.errors.lock().unwrap().clear();
    }
}

fn decorations_eq_none(d: &Decorations) -> bool {
    !(d.time || d.uptime || d.level || d.tags || d.pid || d.thread)
}

impl Default for LogConfiguration {
    fn default() -> Self {
        Self::new()
    }
}
