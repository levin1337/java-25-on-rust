/*
 * Port of OpenJDK 25 HotSpot DiagnosticCommand (share/services/diagnosticCommand.hpp)
 *
 * Implements the HotSpot jcmd diagnostic command framework for runtime inspection,
 * JVM diagnostics, thread dumps, and GC control.
 */

use crate::share::runtime::os::Os;
use crate::share::runtime::threads::Threads;
use crate::share::services::management::Management;
use std::collections::HashMap;

pub trait DCmd {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn impact(&self) -> &'static str {
        "Low"
    }
    fn execute(&self, args: &[&str]) -> Result<String, String>;
}

pub struct VersionDCmd;
impl DCmd for VersionDCmd {
    fn name(&self) -> &'static str {
        "VM.version"
    }
    fn description(&self) -> &'static str {
        "Print JVM version information."
    }
    fn execute(&self, _args: &[&str]) -> Result<String, String> {
        Ok(format!(
            "OpenJDK 64-Bit HotSpot(TM) VM (Rust) (build 25-ea+1-rust, mixed mode, sharing, OS: {}, Arch: {})",
            Os::os_name(),
            Os::os_arch()
        ))
    }
}

pub struct UptimeDCmd;
impl DCmd for UptimeDCmd {
    fn name(&self) -> &'static str {
        "VM.uptime"
    }
    fn description(&self) -> &'static str {
        "Print VM uptime."
    }
    fn execute(&self, _args: &[&str]) -> Result<String, String> {
        let uptime_ms = Management::global().runtime_mxbean().uptime_ms();
        let secs = uptime_ms as f64 / 1000.0;
        Ok(format!("{:.3} s", secs))
    }
}

pub struct SystemPropertiesDCmd;
impl DCmd for SystemPropertiesDCmd {
    fn name(&self) -> &'static str {
        "VM.system_properties"
    }
    fn description(&self) -> &'static str {
        "Print system properties."
    }
    fn execute(&self, _args: &[&str]) -> Result<String, String> {
        let props = Management::global().runtime_mxbean().system_properties();
        let mut out = String::new();
        out.push_str("Active System Properties:\n");
        for (k, v) in props {
            out.push_str(&format!("{}={}\n", k, v));
        }
        Ok(out)
    }
}

pub struct VMFlagsDCmd;
impl DCmd for VMFlagsDCmd {
    fn name(&self) -> &'static str {
        "VM.flags"
    }
    fn description(&self) -> &'static str {
        "Print VM flag options and their current values."
    }
    fn execute(&self, _args: &[&str]) -> Result<String, String> {
        Ok(
            "-XX:+UseG1GC -XX:+TieredCompilation -XX:TieredStopAtLevel=4 -XX:+BytecodeVerification"
                .to_string(),
        )
    }
}

pub struct ThreadPrintDCmd;
impl DCmd for ThreadPrintDCmd {
    fn name(&self) -> &'static str {
        "Thread.print"
    }
    fn description(&self) -> &'static str {
        "Print all threads with stacktraces."
    }
    fn execute(&self, _args: &[&str]) -> Result<String, String> {
        let thread_names = Threads::global().thread_names();
        let mut out = String::new();
        out.push_str(&format!(
            "Full thread dump OpenJDK 64-Bit HotSpot(TM) VM (Rust):\n\nTotal threads: {}\n",
            thread_names.len()
        ));
        for name in thread_names {
            out.push_str(&format!(
                "\"{}\" prio=5 tid=0x0 nid=0x0 runnable\n   java.lang.Thread.State: RUNNABLE\n\n",
                name
            ));
        }
        Ok(out)
    }
}

pub struct DiagnosticFramework {
    commands: HashMap<&'static str, Box<dyn DCmd + Send + Sync>>,
}

impl DiagnosticFramework {
    pub fn new() -> Self {
        let mut framework = Self {
            commands: HashMap::new(),
        };
        framework.register(Box::new(VersionDCmd));
        framework.register(Box::new(UptimeDCmd));
        framework.register(Box::new(SystemPropertiesDCmd));
        framework.register(Box::new(VMFlagsDCmd));
        framework.register(Box::new(ThreadPrintDCmd));
        framework
    }

    pub fn register(&mut self, cmd: Box<dyn DCmd + Send + Sync>) {
        self.commands.insert(cmd.name(), cmd);
    }

    pub fn execute(&self, line: &str) -> Result<String, String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return Err("Empty diagnostic command".to_string());
        }

        let cmd_name = parts[0];
        let args = &parts[1..];

        if cmd_name == "help" {
            if args.is_empty() {
                let mut out = String::from("The following commands are available:\n");
                for (name, cmd) in &self.commands {
                    out.push_str(&format!("{:<22} : {}\n", name, cmd.description()));
                }
                return Ok(out);
            } else if let Some(cmd) = self.commands.get(args[0]) {
                return Ok(format!(
                    "{}\n{}\nImpact: {}\n",
                    cmd.name(),
                    cmd.description(),
                    cmd.impact()
                ));
            } else {
                return Err(format!("Unknown command '{}'", args[0]));
            }
        }

        if let Some(cmd) = self.commands.get(cmd_name) {
            cmd.execute(args)
        } else {
            Err(format!("Unknown diagnostic command '{}'", cmd_name))
        }
    }
}

impl Default for DiagnosticFramework {
    fn default() -> Self {
        Self::new()
    }
}
