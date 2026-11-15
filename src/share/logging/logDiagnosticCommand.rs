/*
 * Port of OpenJDK 25 HotSpot LogDiagnosticCommand (share/logging/
 * logDiagnosticCommand.cpp) — the API surface behind `jcmd VM.log`.
 *
 * Subcommands (VM.log DCmd in services/diagnosticCommand.cpp delegates
 * here):
 *   VM.log                     — print current configuration
 *   VM.log list                — list registered tag sets
 *   VM.log disable             — turn off all logging
 *   VM.log rotate              — rotate all file outputs
 *   VM.log what=.. output=.. decorators=.. output-options=.. — configure
 *   VM.log print [if level]    — recent retained messages
 */

use super::log::Log;
use super::logConfiguration::{LogConfiguration, ParseOutcome};
use super::logLevel::LogLevel;
use super::logTag::LogTag;
use super::logTagSet::LogTagSet;

/// Execute a VM.log diagnostic command; returns command output text.
pub fn execute_vm_log(args: &[String]) -> Result<String, String> {
    let cfg = LogConfiguration::global();
    if args.is_empty() {
        return Ok(cfg.describe_current_configuration());
    }
    match args[0].as_str() {
        "list" => Ok(LogTagSet::list_all()),
        "disable" => {
            cfg.parse_xlog("disable")?;
            Ok("Logging is disabled\n".to_string())
        }
        "rotate" => {
            cfg.rotate_all_outputs();
            Ok("Log rotation triggered\n".to_string())
        }
        "print" => {
            let filter = args.get(1).and_then(|a| {
                a.strip_prefix("if=")
                    .or_else(|| a.strip_prefix("if "))
            });
            let mut out = String::new();
            for m in Log::recent_messages_raw() {
                if let Some(f) = filter {
                    let parts: Vec<&str> = f.split('+').collect();
                    let level = LogLevel::from_string(parts.last().unwrap_or(&""));
                    let tags: Vec<LogTag> = parts[..parts.len().saturating_sub(1)]
                        .iter()
                        .filter_map(|t| LogTag::from_string(t))
                        .collect();
                    if let Some(l) = level {
                        if m.level > l {
                            continue;
                        }
                    }
                    if !tags.is_empty() && !tags.iter().all(|t| m.tagset.contains(*t)) {
                        continue;
                    }
                }
                out.push_str(&format!(
                    "[{}][{}] {}\n",
                    m.level.name(),
                    m.tagset.label(),
                    m.text
                ));
            }
            Ok(out)
        }
        "help" => Ok(LogConfiguration::print_command_line_help()),
        other => {
            // Key=value options map onto the -Xlog grammar fields.
            let mut what = String::new();
            let mut output = String::new();
            let mut decorators = String::new();
            let mut options = String::new();
            for arg in std::iter::once(other).chain(args[1..].iter().map(|s| s.as_str())) {
                let arg = arg;
                if let Some(v) = arg.strip_prefix("what=") {
                    what = v.to_string();
                } else if let Some(v) = arg.strip_prefix("output=") {
                    output = v.to_string();
                } else if let Some(v) = arg.strip_prefix("decorators=") {
                    decorators = v.to_string();
                } else if let Some(v) = arg.strip_prefix("output-options=") {
                    options = v.to_string();
                } else if arg.contains('=') {
                    return Err(format!("Unknown VM.log option '{}'", arg));
                } else {
                    return Err(format!("Unknown VM.log argument '{}'", arg));
                }
            }
            cfg.parse_log_arguments(
                Some(&output),
                Some(&what),
                Some(&decorators),
                Some(&options),
            )?;
            Ok("Log configuration updated\n".to_string())
        }
    }
}

/// -Xlog:help handling: returns help text when outcome is Help.
pub fn help_text_if_needed(outcome: ParseOutcome) -> Option<String> {
    if outcome == ParseOutcome::Help {
        Some(LogConfiguration::print_command_line_help())
    } else {
        None
    }
}
