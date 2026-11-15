/*
 * Port of OpenJDK 25 HotSpot LogConfiguration
 * (share/logging/logConfiguration.cpp/.hpp)
 *
 * -Xlog grammar (logConfiguration.cpp::parse_command_line_arguments):
 *
 *   -Xlog[:[what]:[output]:[decorators]:[output-options]]
 *
 *   what             — comma-separated LogSelectionList
 *                      (tag1[+tag2...][*][=level][,...], default "all")
 *   output           — stdout | stderr | #<idx> | <name> (implicit file=)
 *   decorators       — LogDecorators list, "none", or empty (defaults)
 *   output-options   — key=value list (filesize, filecount, foldmultilines)
 *
 * Special first arguments: "disable", "help", "async[:drop|:stall]".
 * Outputs #0 (stdout) and #1 (stderr) always exist; additional outputs are
 * appended. Per-tag-set levels are resolved per output and persist across
 * -Xlog occurrences until overwritten (LogLevel::NotMentioned semantics).
 */

use super::logAsyncWriter::AsyncMode;
use super::logDecorations::Decorations;
use super::logLevel::LogLevel;
use super::logOutput::{LogOutput, StreamLogOutput};
use super::logSelection::{LogSelection, LogSelectionList};
use super::logTag::LogTag;
use super::logTagSet::LogTagSet;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// One configured log output (LogOutput + its decorations/selections).
pub struct OutputConfig {
    pub output: Arc<dyn LogOutput>,
    /// Normalized output name: "stdout", "stderr", "#n" or "file=<path>".
    pub name: String,
    pub decorators: Decorations,
    /// Every configure_output call appends its selection list. Because
    /// LogTagSets register lazily in this port (C++ pre-registers all
    /// tag sets at startup), the per-tagset sticky level of
    /// LogTagSet::_output_list is reproduced by replaying this history:
    /// the last non-NotMentioned level across all lists wins.
    pub selection_history: Vec<LogSelectionList>,
    /// Level for tag sets never covered by any selection on this output
    /// (stdout defaults to Warning, all other outputs to Off).
    pub default_level: LogLevel,
    /// Computed config string ("all=<mcl>,<tagset>=<level>,...").
    pub config_string: String,
}

struct ConfigInner {
    outputs: Vec<OutputConfig>,
    async_mode: AsyncMode,
    stdout_options_used: bool,
    stderr_options_used: bool,
}

/// Result of parsing one -Xlog argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseOutcome {
    /// Selections/decorators applied to an output.
    Configured,
    /// "-Xlog:disable" — all logging turned off.
    Disabled,
    /// "-Xlog:help" — help text should be printed.
    Help,
    /// "-Xlog:async..." — async mode changed.
    AsyncConfigured,
}

pub struct LogConfiguration {
    inner: Mutex<ConfigInner>,
}

static GLOBAL_LOG_CONFIG: OnceLock<LogConfiguration> = OnceLock::new();

impl LogConfiguration {
    /// LogConfiguration::initialize — stdout (#0) and stderr (#1).
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ConfigInner {
                outputs: vec![
                    OutputConfig {
                        output: Arc::new(StreamLogOutput::stdout()),
                        name: "stdout".to_string(),
                        decorators: Decorations::default(),
                        selection_history: Vec::new(),
                        default_level: LogLevel::DEFAULT,
                        config_string: "all=warning".to_string(),
                    },
                    OutputConfig {
                        output: Arc::new(StreamLogOutput::stderr()),
                        name: "stderr".to_string(),
                        decorators: Decorations::default(),
                        selection_history: Vec::new(),
                        default_level: LogLevel::Off,
                        config_string: "all=off".to_string(),
                    },
                ],
                async_mode: AsyncMode::Off,
                stdout_options_used: false,
                stderr_options_used: false,
            }),
        }
    }

    pub fn global() -> &'static LogConfiguration {
        GLOBAL_LOG_CONFIG.get_or_init(Self::new)
    }

    // ------------------------------------------------------------------
    // Query API used by Log::write
    // ------------------------------------------------------------------

    /// Effective level of `tagset` on output `idx`
    /// (LogOutputList::level_for + is_level equivalent): replay the
    /// output's selection history, last mentioned level wins; tag sets
    /// never mentioned fall back to the output's default level.
    pub fn level_for(&self, tagset: &LogTagSet, output_idx: usize) -> LogLevel {
        let inner = self.inner.lock().unwrap();
        output_level_for(&inner, tagset, output_idx)
    }

    /// Snapshot of configured outputs for the write path.
    pub fn outputs_snapshot(&self) -> Vec<(Arc<dyn LogOutput>, Decorations)> {
        let inner = self.inner.lock().unwrap();
        inner
            .outputs
            .iter()
            .map(|o| (o.output.clone(), o.decorators))
            .collect()
    }

    pub fn output_count(&self) -> usize {
        self.inner.lock().unwrap().outputs.len()
    }

    pub fn is_async_mode(&self) -> bool {
        self.inner.lock().unwrap().async_mode != AsyncMode::Off
    }

    pub fn async_mode(&self) -> AsyncMode {
        self.inner.lock().unwrap().async_mode
    }

    /// True when the message would be emitted on at least one output.
    pub fn is_level_enabled(&self, tagset: &LogTagSet, level: LogLevel) -> bool {
        if level == LogLevel::Off || level == LogLevel::NotMentioned {
            return false;
        }
        let inner = self.inner.lock().unwrap();
        for (idx, _) in inner.outputs.iter().enumerate() {
            let lvl = output_level_for(&inner, tagset, idx);
            if lvl != LogLevel::Off && level <= lvl {
                return true;
            }
        }
        false
    }

    // ------------------------------------------------------------------
    // Configuration
    // ------------------------------------------------------------------

    /// Backwards-compatible entry point: parses a bare selection list
    /// ("gc=info,class+load=debug") and applies it to stdout.
    pub fn parse_log_argument(&self, arg: &str) -> Result<(), String> {
        match self.parse_xlog(arg)? {
            ParseOutcome::Configured | ParseOutcome::AsyncConfigured => Ok(()),
            ParseOutcome::Disabled | ParseOutcome::Help => Ok(()),
        }
    }

    /// Full -Xlog parser: text after "-Xlog:" (or the whole "-Xlog" arg
    /// for the legacy simple form).
    pub fn parse_xlog(&self, arg: &str) -> Result<ParseOutcome, String> {
        let arg = arg.trim();
        if arg.is_empty() {
            // Bare "-Xlog": all=info on stdout (arguments.rs maps this
            // through parse_log_argument, but stay tolerant here).
            return self.parse_log_arguments(Some(""), Some("all=info"), None, None);
        }
        if arg == "disable" {
            self.disable_logging();
            return Ok(ParseOutcome::Disabled);
        }
        if arg == "help" {
            return Ok(ParseOutcome::Help);
        }
        if arg == "async" || arg == "async:drop" || arg == "async:stall" {
            let mode = if arg.ends_with(":stall") {
                AsyncMode::Stall
            } else {
                AsyncMode::Drop
            };
            self.inner.lock().unwrap().async_mode = mode;
            return Ok(ParseOutcome::AsyncConfigured);
        }

        // Split into what:output:decorators:output-options honoring quotes
        // and Windows drive-letter colons (logConfiguration.cpp).
        let parts = split_xlog_arguments(arg);
        let what = parts.first().map(|s| s.as_str()).unwrap_or("");
        let output = parts.get(1).map(|s| s.as_str());
        let decorators = parts.get(2).map(|s| s.as_str());
        let output_options = parts.get(3).map(|s| s.as_str());

        self.parse_log_arguments(output, Some(what), decorators, output_options)
    }

    /// LogConfiguration::parse_log_arguments — apply selections +
    /// decorators + output options to one output.
    pub fn parse_log_arguments(
        &self,
        outputstr: Option<&str>,
        selectionstr: Option<&str>,
        decoratorstr: Option<&str>,
        output_options: Option<&str>,
    ) -> Result<ParseOutcome, String> {
        let outputstr = match outputstr {
            None | Some("") => "stdout",
            Some(v) => v,
        };

        let selections = LogSelectionList::parse(selectionstr)
            .map_err(|e| format!("Invalid log selection: {}", e.trim()))?;

        // Default decorators: uptime,level,tags — unless every selection
        // covers the undecorated {jit,inlining} tag set.
        let mut decorators = if selections.has_default_decorators() {
            Decorations::default()
        } else {
            Decorations::none()
        };
        if let Some(d) = decoratorstr {
            if !d.is_empty() {
                decorators = Decorations::parse(d)
                    .map_err(|e| format!("Invalid decorator in -Xlog: {}", e))?;
            }
        }

        let mut inner = self.inner.lock().unwrap();

        let idx = if outputstr.starts_with('#') {
            let n: usize = outputstr[1..]
                .parse()
                .map_err(|_| format!("Invalid output index '{}'", outputstr))?;
            if n >= inner.outputs.len() {
                return Err(format!("Invalid output index '{}'", outputstr));
            }
            n
        } else if outputstr == "stdout" {
            0
        } else if outputstr == "stderr" {
            1
        } else {
            // Normalize: strip quotes, add implicit "file=" prefix.
            let normalized = normalize_output_name(outputstr)
                .ok_or_else(|| "Output name has opening quote but is missing a terminating quote.".to_string())?;
            match inner.outputs.iter().position(|o| o.name == normalized) {
                Some(i) => i,
                None => {
                    if !normalized.starts_with("file=") {
                        return Err(format!("Unsupported log output type: {}", normalized));
                    }
                    let output: Arc<dyn LogOutput> = Arc::new(StreamLogOutput::file(
                        &normalized,
                        output_options.unwrap_or(""),
                    ));
                    inner.outputs.push(OutputConfig {
                        output,
                        name: normalized.clone(),
                        decorators,
                        selection_history: Vec::new(),
                        default_level: LogLevel::Off,
                        config_string: String::new(),
                    });
                    inner.outputs.len() - 1
                }
            }
        };

        // Options for pre-existing outputs are ignored (stdout/stderr get
        // one parse_options pass, like the C++ static flags).
        if let Some(opts) = output_options {
            if !opts.is_empty() {
                let is_builtin = idx <= 1;
                let already_used = if idx == 0 {
                    inner.stdout_options_used
                } else {
                    inner.stderr_options_used
                };
                if is_builtin {
                    if !already_used {
                        if idx == 0 {
                            inner.stdout_options_used = true;
                        } else {
                            inner.stderr_options_used = true;
                        }
                        let _ = inner.outputs[idx].output.set_option_str(opts);
                    }
                }
                // Non-builtin existing outputs: ignored with warning.
            }
        }

        // configure_output(idx, selections, decorators): the selection
        // list is appended to the output's history; per-tagset levels
        // are resolved lazily by replaying it (see output_level_for).
        {
            let entry = &mut inner.outputs[idx];
            entry.decorators = decorators;
            entry.selection_history.push(selections);
        }
        let config_string = compute_config_string(&inner, idx);
        inner.outputs[idx].config_string = config_string;

        // selections.verify_selections(errstream): unmatched selections
        // only produce a warning on the error stream in C++; they do not
        // fail the configuration.
        Ok(ParseOutcome::Configured)
    }

    /// LogConfiguration::disable_logging / disable_outputs — appends an
    /// "all=off" selection to every output, matching the sticky
    /// per-tagset levels the C++ code sets explicitly.
    pub fn disable_logging(&self) {
        let mut inner = self.inner.lock().unwrap();
        let off = LogSelectionList {
            selections: vec![LogSelection {
                tags: Vec::new(),
                wildcard: true,
                level: LogLevel::Off,
            }],
        };
        for out in inner.outputs.iter_mut() {
            out.selection_history.push(off.clone());
            out.config_string = "all=off".to_string();
        }
    }

    /// LogConfiguration::rotate_all_outputs — file outputs only (idx >= 2).
    pub fn rotate_all_outputs(&self) {
        let inner = self.inner.lock().unwrap();
        for out in inner.outputs.iter().skip(2) {
            out.output.force_rotate();
        }
    }

    /// VM.log "print"/"describe" — current configuration text.
    pub fn describe_current_configuration(&self) -> String {
        let inner = self.inner.lock().unwrap();
        let mut out = String::from("Log output configuration:\n");
        for (i, o) in inner.outputs.iter().enumerate() {
            out.push_str(&format!(
                " #{}: {} {} {}\n",
                i,
                o.name,
                o.config_string,
                o.decorators.describe()
            ));
        }
        out
    }

    /// LogConfiguration::describe_available.
    pub fn describe_available(&self) -> String {
        let mut out = String::new();
        out.push_str("Available log levels: off,error,warning,info,debug,trace\n");
        out.push_str("Available log decorators: ");
        let decos: Vec<String> = super::logDecorations::DECORATOR_NAMES
            .iter()
            .map(|(n, a)| format!("{} ({})", n, a))
            .collect();
        out.push_str(&decos.join(","));
        out.push_str("\nAvailable log tags: ");
        let tags: Vec<&str> = LogTag::all_tags().iter().map(|t| t.name()).collect();
        out.push_str(&tags.join(","));
        out.push('\n');
        out.push_str(&LogTagSet::describe_tagsets());
        out.push_str(&LogTagSet::list_all());
        out
    }

    /// LogConfiguration::describe — available + current.
    pub fn describe(&self) -> String {
        let mut out = self.describe_available();
        out.push_str(&self.describe_current_configuration());
        out
    }

    /// LogConfiguration::print_command_line_help — -Xlog:help text.
    pub fn print_command_line_help() -> String {
        let mut out = String::new();
        out.push_str("-Xlog Usage: -Xlog[:[selections][:[output][:[decorators][:output-options]]]]\n");
        out.push_str("\t where 'selections' are combinations of tags and levels of the form tag1[+tag2...][*][=level][,...]\n");
        out.push_str("\t NOTE: Unless wildcard (*) is specified, only log messages tagged with exactly the tags specified will be matched.\n\n");
        out.push_str("Available log levels: off,error,warning,info,debug,trace\n\n");
        out.push_str("Available log decorators: ");
        let decos: Vec<String> = super::logDecorations::DECORATOR_NAMES
            .iter()
            .map(|(n, a)| format!("{} ({})", n, a))
            .collect();
        out.push_str(&decos.join(","));
        out.push_str("\n Decorators can also be specified as 'none' for no decoration.\n\n");
        out.push_str("Available log tags: ");
        let tags: Vec<&str> = LogTag::all_tags().iter().map(|t| t.name()).collect();
        out.push_str(&tags.join(","));
        out.push_str("\n Specifying 'all' instead of a tag combination matches all tag combinations.\n\n");
        out.push_str(&LogTagSet::describe_tagsets());
        out.push_str("\nAvailable log outputs:\n stdout/stderr\n file=<filename>\n");
        out.push_str("  If the filename contains %p, %t and/or %hn, they will expand to the JVM's PID, startup timestamp and host name, respectively.\n\n");
        out.push_str("Available log output options:\n foldmultilines=.. - If set to true, a log event that consists of multiple lines will be folded into a single line.\n\n");
        out.push_str("Additional file output options:\n filesize=.. - Target byte size for log rotation (supports K/M/G suffix).\n filecount=.. - Number of files to keep in rotation (not counting the active file).\n\n");
        out.push_str("Asynchronous logging (off by default):\n -Xlog:async[:[mode]]\n  All log messages are written to an intermediate buffer first and will then be flushed to the corresponding log outputs by a standalone thread.\n");
        out
    }

    /// Test/reset hook: clears resolved levels and outputs back to the
    /// initial stdout/stderr pair.
    pub fn reset(&self) {
        *self.inner.lock().unwrap() = ConfigInner {
            outputs: vec![
                OutputConfig {
                    output: Arc::new(StreamLogOutput::stdout()),
                    name: "stdout".to_string(),
                    decorators: Decorations::default(),
                    selection_history: Vec::new(),
                    default_level: LogLevel::DEFAULT,
                    config_string: "all=warning".to_string(),
                },
                OutputConfig {
                    output: Arc::new(StreamLogOutput::stderr()),
                    name: "stderr".to_string(),
                    decorators: Decorations::default(),
                    selection_history: Vec::new(),
                    default_level: LogLevel::Off,
                    config_string: "all=off".to_string(),
                },
            ],
            async_mode: AsyncMode::Off,
            stdout_options_used: false,
            stderr_options_used: false,
        };
    }
}

/// normalize_output_name — quotes handling + implicit "file=" prefix.
fn normalize_output_name(full_name: &str) -> Option<String> {
    let bytes = full_name.as_bytes();
    let start_quote = bytes.iter().position(|&b| b == b'"');
    let quoted = start_quote.is_some();
    let mut equals = full_name.find('=');
    // Ignore '=' inside quotes.
    if quoted && equals.map_or(false, |e| e > start_quote.unwrap()) {
        equals = None;
    }
    let is_stdout_or_stderr = full_name == "stdout" || full_name == "stderr";

    let (prefix, name, name_off) = if let Some(eq) = equals {
        (&full_name[..=eq], &full_name[eq + 1..], eq + 1)
    } else if is_stdout_or_stderr {
        ("", full_name, 0usize)
    } else {
        ("file=", full_name, 0usize)
    };

    let name = if quoted {
        let sq = start_quote.unwrap();
        if sq != name_off {
            return None;
        }
        let end_quote = full_name[name_off + 1..].find('"')? + name_off + 1;
        if end_quote + 1 != full_name.len() {
            return None;
        }
        &full_name[name_off + 1..end_quote]
    } else {
        name
    };

    Some(format!("{}{}", prefix, name))
}

/// Split -Xlog argument on ':' outside quotes; Windows drive-letter colons
/// ("C:\...", "file=C:/...") are skipped like the C++ code does.
fn split_xlog_arguments(arg: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let bytes = arg.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() && parts.len() < 3 {
        match bytes[i] {
            b'"' => {
                // Skip quoted region.
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b':' => {
                #[cfg(windows)]
                {
                    // Skip drive-letter colon: "X:\" or "X:/" at position
                    // start+1, or right after "file=".
                    let is_drive = i == start + 1
                        || (i >= 5
                            && arg
                                .get(start..start + 5)
                                .map_or(false, |p| p.eq_ignore_ascii_case("file=")));
                    if is_drive
                        && i + 1 < bytes.len()
                        && (bytes[i + 1] == b'\\' || bytes[i + 1] == b'/')
                    {
                        i += 1;
                        continue;
                    }
                }
                parts.push(arg[start..i].to_string());
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    parts.push(arg[start..].to_string());
    parts
}

/// Effective level of `tagset` on output `idx`: replay the output's
/// selection history; the last selection covering the tag set wins,
/// NotMentioned keeps earlier values, and tag sets never mentioned fall
/// back to the output's default level.
fn output_level_for(inner: &ConfigInner, tagset: &LogTagSet, idx: usize) -> LogLevel {
    let Some(out) = inner.outputs.get(idx) else {
        return LogLevel::Off;
    };
    let mut level = None;
    for list in &out.selection_history {
        if let Some(l) = list.level_for(tagset) {
            level = Some(l);
        }
    }
    level.unwrap_or(out.default_level)
}

/// LogOutput::update_config_string equivalent — "all=<mcl>" plus one
/// entry per deviating tag set (exact tag-set selections only; the full
/// subset-cover algorithm of logOutput.cpp is approximated by listing
/// each deviating registered tag set).
fn compute_config_string(inner: &ConfigInner, idx: usize) -> String {
    let mut counts: HashMap<LogLevel, usize> = HashMap::new();
    let mut levels: Vec<(LogTagSet, LogLevel)> = Vec::new();
    for ts in LogTagSet::all() {
        let lvl = output_level_for(inner, &ts, idx);
        *counts.entry(lvl).or_insert(0) += 1;
        levels.push((ts, lvl));
    }
    let mcl = counts
        .iter()
        .max_by_key(|(_, c)| *c)
        .map(|(l, _)| *l)
        .unwrap_or(LogLevel::Off);
    let mut s = format!("all={}", mcl.name());
    for (ts, lvl) in levels {
        if lvl != mcl {
            s.push_str(&format!(",{}={}", ts.label(), lvl.name()));
        }
    }
    s
}

trait SetOptionStr {
    fn set_option_str(&self, options: &str) -> bool;
}

impl SetOptionStr for dyn LogOutput {
    fn set_option_str(&self, options: &str) -> bool {
        let mut ok = true;
        for opt in options.split(',') {
            let opt = opt.trim();
            if let Some((k, v)) = opt.split_once('=') {
                ok &= self.set_option(k.trim(), v.trim());
            } else {
                ok = false;
            }
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_names() {
        assert_eq!(normalize_output_name("stdout"), Some("stdout".into()));
        assert_eq!(normalize_output_name("stderr"), Some("stderr".into()));
        assert_eq!(normalize_output_name("gc.log"), Some("file=gc.log".into()));
        assert_eq!(normalize_output_name("file=gc.log"), Some("file=gc.log".into()));
        assert_eq!(normalize_output_name("\"gc.log\""), Some("file=gc.log".into()));
        assert_eq!(normalize_output_name("file=\"gc.log\""), Some("file=gc.log".into()));
        assert_eq!(normalize_output_name("\"unterminated"), None);
        assert_eq!(normalize_output_name("a=\"b\"c"), None);
    }

    #[test]
    fn split_colon_arguments() {
        assert_eq!(
            split_xlog_arguments("gc=debug:file=x.log:uptime,level"),
            vec!["gc=debug", "file=x.log", "uptime,level"]
        );
        assert_eq!(
            split_xlog_arguments("gc=debug:\"file=C:\\tmp\\a.log\":none"),
            vec!["gc=debug", "\"file=C:\\tmp\\a.log\"", "none"]
        );
        assert_eq!(split_xlog_arguments("all=info"), vec!["all=info"]);
        assert_eq!(
            split_xlog_arguments("gc=debug::uptime:filecount=5"),
            vec!["gc=debug", "", "uptime", "filecount=5"]
        );
    }

    #[test]
    fn parse_selections_to_stdout() {
        let cfg = LogConfiguration::new();
        cfg.parse_xlog("gc=debug").unwrap();
        let ts = LogTagSet::new(&[LogTag::Gc]);
        assert_eq!(cfg.level_for(&ts, 0), LogLevel::Debug);
        assert_eq!(cfg.level_for(&ts, 1), LogLevel::Off);
        // Unconfigured tag set keeps default Warning on stdout.
        let ts2 = LogTagSet::new(&[LogTag::Os]);
        assert_eq!(cfg.level_for(&ts2, 0), LogLevel::Warning);
        assert!(cfg.is_level_enabled(&ts, LogLevel::Info));
        assert!(!cfg.is_level_enabled(&ts, LogLevel::Trace));
        assert!(!cfg.is_level_enabled(&ts2, LogLevel::Info));
        assert!(cfg.is_level_enabled(&ts2, LogLevel::Warning));
    }

    #[test]
    fn parse_accumulates_and_off() {
        let cfg = LogConfiguration::new();
        cfg.parse_xlog("gc=debug").unwrap();
        cfg.parse_xlog("os=trace").unwrap();
        let gc = LogTagSet::new(&[LogTag::Gc]);
        let os = LogTagSet::new(&[LogTag::Os]);
        assert_eq!(cfg.level_for(&gc, 0), LogLevel::Debug);
        assert_eq!(cfg.level_for(&os, 0), LogLevel::Trace);
        cfg.parse_xlog("all=off").unwrap();
        assert_eq!(cfg.level_for(&gc, 0), LogLevel::Off);
        assert_eq!(cfg.level_for(&os, 0), LogLevel::Off);
        assert!(!cfg.is_level_enabled(&gc, LogLevel::Error));
    }

    #[test]
    fn wildcard_and_multi_tag_matching() {
        let cfg = LogConfiguration::new();
        cfg.parse_xlog("gc*=info").unwrap();
        let gc_heap = LogTagSet::new(&[LogTag::Gc, LogTag::Heap]);
        let os = LogTagSet::new(&[LogTag::Os]);
        assert_eq!(cfg.level_for(&gc_heap, 0), LogLevel::Info);
        assert_eq!(cfg.level_for(&os, 0), LogLevel::Warning);

        let cfg2 = LogConfiguration::new();
        cfg2.parse_xlog("gc+heap=debug").unwrap();
        // Exact match: tag set with extra tag is NOT covered.
        let gc_heap_ref = LogTagSet::new(&[LogTag::Gc, LogTag::Heap, LogTag::Ref]);
        assert_eq!(cfg2.level_for(&gc_heap_ref, 0), LogLevel::Warning);
        assert_eq!(cfg2.level_for(&gc_heap, 0), LogLevel::Debug);
    }

    #[test]
    fn file_output_created() {
        let cfg = LogConfiguration::new();
        let dir = std::env::temp_dir().join(format!("jvm25-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("vm.log");
        let arg = format!("gc=info:file={}:none", path.display());
        cfg.parse_xlog(&arg).unwrap();
        assert_eq!(cfg.output_count(), 3);
        assert!(cfg.inner.lock().unwrap().outputs[2].name.starts_with("file="));
        assert!(cfg.inner.lock().unwrap().outputs[2].decorators.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disable_and_help_and_async() {
        let cfg = LogConfiguration::new();
        assert_eq!(cfg.parse_xlog("disable").unwrap(), ParseOutcome::Disabled);
        let ts = LogTagSet::new(&[LogTag::Gc]);
        assert!(!cfg.is_level_enabled(&ts, LogLevel::Error));
        assert_eq!(cfg.parse_xlog("help").unwrap(), ParseOutcome::Help);
        assert_eq!(cfg.parse_xlog("async:stall").unwrap(), ParseOutcome::AsyncConfigured);
        assert_eq!(cfg.async_mode(), AsyncMode::Stall);
        cfg.parse_xlog("gc=info").unwrap();
        assert!(cfg.is_level_enabled(&ts, LogLevel::Info));
    }

    #[test]
    fn invalid_inputs_error() {
        let cfg = LogConfiguration::new();
        assert!(cfg.parse_xlog("gc=verbose").is_err());
        assert!(cfg.parse_xlog("nosuchtag=info").is_err());
        assert!(cfg.parse_xlog("gc=info:baddec=1").is_err());
        let cfg2 = LogConfiguration::new();
        assert!(cfg2.parse_xlog("gc=info::bogusdecorator").is_err());
        assert!(cfg2.parse_xlog("gc=info:#9").is_err());
    }
}
