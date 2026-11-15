/*
 * Port of OpenJDK 25 HotSpot DiagnosticCommand (share/services/diagnosticCommand.hpp/.cpp)
 *
 * Diagnostic commands executed through jcmd / the attach listener:
 * help, VM.*, GC.*, Thread.print, Compiler.queue, VM.symboltable,
 * VM.stringtable and VM.native_memory. JVMTI and JMX (ManagementAgent.*)
 * commands are intentionally not ported.
 */

use crate::share::compiler::compileBroker::CompileBroker;
use crate::share::memory::universe::Universe;
use crate::share::nmt::memTracker::MemTracker;
use crate::share::runtime::java::Java;
use crate::share::runtime::os::Os;
use crate::share::runtime::threads::Threads;
use crate::share::services::diagnosticArgument::{DcmdResult, DCmdArgument};
use crate::share::services::diagnosticFramework::{
    self, DCmdArgumentInfo, DCmdInstance, DCmdParser, DCmdSource, FULL_EXPORT,
};
use crate::share::services::management::Management;
use crate::share::services::writeableFlags::{JvmFlagOrigin, WriteableFlags};
use std::io::Write;

fn writeln_cr(out: &mut dyn Write, s: &str) -> std::io::Result<()> {
    out.write_all(s.as_bytes())?;
    out.write_all(b"\n")
}

fn thread_state_name(state: crate::share::runtime::javaThread::JavaThreadState) -> &'static str {
    use crate::share::runtime::javaThread::JavaThreadState as S;
    match state {
        S::Uninitialized => "UNINITIALIZED",
        S::New => "NEW",
        S::NewTrans => "NEW",
        S::InNative => "RUNNABLE",
        S::InNativeTrans => "RUNNABLE",
        S::InVm => "RUNNABLE",
        S::InVmTrans => "RUNNABLE",
        S::InJava => "RUNNABLE",
        S::InJavaTrans => "RUNNABLE",
        S::Blocked => "BLOCKED",
        S::BlockedTrans => "BLOCKED",
        S::Waiting => "WAITING",
        S::Terminated => "TERMINATED",
    }
}

/// Shared implementation of the thread dump used by Thread.print and the
/// attach "threaddump" operation (VM_PrintThreads + VM_FindDeadlocks).
pub fn print_thread_dump(
    out: &mut dyn Write,
    _print_concurrent_locks: bool,
    _print_extended_info: bool,
) -> std::io::Result<()> {
    let snapshots = Threads::global().snapshots();
    writeln_cr(
        out,
        "Full thread dump OpenJDK 64-Bit HotSpot(TM) VM (Rust) (25-ea mixed mode):",
    )?;
    writeln_cr(out, "")?;
    writeln_cr(out, &format!("Threads class SMR info:"))?;
    writeln_cr(out, &format!("_java_threads=0x{:x}", snapshots.len()))?;
    writeln_cr(out, "")?;
    for (i, t) in snapshots.iter().enumerate() {
        let state = thread_state_name(t.state);
        writeln_cr(
            out,
            &format!(
                "\"{}\" #{} prio=5 os_prio=0 tid=0x{:x} nid=0x{:x} {}",
                t.name,
                i + 1,
                t.id,
                t.id,
                state.to_lowercase()
            ),
        )?;
        writeln_cr(out, &format!("   java.lang.Thread.State: {}", state))?;
        writeln_cr(out, "")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// help
// ---------------------------------------------------------------------------

pub struct HelpDCmd {
    parser: DCmdParser,
}

impl HelpDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "-all",
            "Show help for all commands",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_argument(DCmdArgument::<String>::new(
            "command name",
            "The name of the command for which we want help",
            "STRING",
            false,
        ));
        Self { parser }
    }
}

impl DCmdInstance for HelpDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }

    fn execute(&mut self, source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let all = self.parser.option::<bool>("-all").unwrap_or(false);
        let cmd = self.parser.argument::<String>(0);
        if all {
            let names = diagnosticFramework::factory_names_sorted(source);
            for name in names {
                if let Some((desc, _impact, enabled)) = diagnosticFramework::factory_meta(&name) {
                    writeln_cr(
                        out,
                        &format!("{}{}", name, if enabled { "" } else { " [disabled]" }),
                    )
                    .map_err(|e| e.to_string())?;
                    writeln_cr(out, &format!("\t{}", desc)).map_err(|e| e.to_string())?;
                    writeln_cr(out, "").map_err(|e| e.to_string())?;
                }
            }
        } else if let Some(cmd_name) = cmd {
            if !cmd_name.is_empty() {
                match diagnosticFramework::factory_snapshot(&cmd_name) {
                    Some((name, desc, impact, enabled, _)) => {
                        writeln_cr(
                            out,
                            &format!("{}{}", name, if enabled { "" } else { " [disabled]" }),
                        )
                        .map_err(|e| e.to_string())?;
                        writeln_cr(out, &desc).map_err(|e| e.to_string())?;
                        writeln_cr(out, &format!("\nImpact: {}", impact))
                            .map_err(|e| e.to_string())?;
                        writeln_cr(out, "").map_err(|e| e.to_string())?;
                        if let Some(inst) =
                            diagnosticFramework::create_instance_for(&name)
                        {
                            inst.print_help(&name, out).map_err(|e| e.to_string())?;
                        }
                    }
                    None => {
                        writeln_cr(out, &format!("Help unavailable : '{}' : No such command", cmd_name))
                            .map_err(|e| e.to_string())?;
                    }
                }
                return Ok(());
            }
        }
        writeln_cr(out, "The following commands are available:").map_err(|e| e.to_string())?;
        let names = diagnosticFramework::factory_names_sorted(source);
        for name in names {
            let enabled = diagnosticFramework::factory_is_enabled(&name).unwrap_or(true);
            writeln_cr(out, &format!("{}{}", name, if enabled { "" } else { " [disabled]" }))
                .map_err(|e| e.to_string())?;
        }
        writeln_cr(
            out,
            "\nFor more information about a specific command use 'help <command>'.",
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }
    fn cleanup(&mut self) {
        self.parser.cleanup();
    }
    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

// ---------------------------------------------------------------------------
// VM.version / VM.command_line / VM.system_properties
// ---------------------------------------------------------------------------

pub struct VersionDCmd;

impl DCmdInstance for VersionDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let jdk = Java::jdk_version();
        writeln_cr(
            out,
            "OpenJDK 64-Bit HotSpot(TM) VM (Rust) version 25-ea",
        )
        .map_err(|e| e.to_string())?;
        if jdk.build() > 0 {
            writeln_cr(
                out,
                &format!(
                    "JDK {}.{}.{}.{}",
                    jdk.major(),
                    jdk.minor(),
                    jdk.security(),
                    jdk.build()
                ),
            )
            .map_err(|e| e.to_string())?;
        } else {
            writeln_cr(
                out,
                &format!(
                    "JDK {}.{}.{}",
                    jdk.major(),
                    jdk.minor(),
                    jdk.security()
                ),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

pub struct CommandLineDCmd;

impl DCmdInstance for CommandLineDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let args: Vec<String> = std::env::args().collect();
        writeln_cr(out, &args.join(" ")).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

pub struct PrintSystemPropertiesDCmd;

impl DCmdInstance for PrintSystemPropertiesDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let props = Management::global().runtime_mxbean().system_properties();
        let mut keys: Vec<&String> = props.keys().collect();
        keys.sort();
        for k in keys {
            writeln_cr(out, &format!("{}={}", k, props[k])).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// VM.flags / VM.set_flag
// ---------------------------------------------------------------------------

pub struct PrintVMFlagsDCmd {
    parser: DCmdParser,
}

impl PrintVMFlagsDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "-all",
            "Print all flags supported by the VM",
            "BOOLEAN",
            false,
            "false",
        ));
        Self { parser }
    }
}

impl DCmdInstance for PrintVMFlagsDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let all = self.parser.option::<bool>("-all").unwrap_or(false);
        WriteableFlags::print_flags(out, all).map_err(|e| e.to_string())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }
    fn cleanup(&mut self) {
        self.parser.cleanup();
    }
    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

pub struct SetVMFlagDCmd {
    parser: DCmdParser,
}

impl SetVMFlagDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_argument(DCmdArgument::<String>::new(
            "flag name",
            "The name of the flag we want to set",
            "STRING",
            true,
        ));
        parser.add_dcmd_argument(DCmdArgument::<String>::new(
            "string value",
            "The value we want to set",
            "STRING",
            false,
        ));
        Self { parser }
    }
}

impl DCmdInstance for SetVMFlagDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let flag = self.parser.argument::<String>(0);
        let value = self.parser.argument::<String>(1);
        let val = if self.parser.argument_is_set(1) {
            value.as_deref()
        } else {
            None
        };
        let (err, msg) = WriteableFlags::set_flag(
            flag.as_deref(),
            val,
            JvmFlagOrigin::Management,
        );
        if !err.is_success() {
            writeln_cr(out, &msg).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }
    fn cleanup(&mut self) {
        self.parser.cleanup();
    }
    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

// ---------------------------------------------------------------------------
// VM.dynlibs
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod dynlibs {
    use std::ffi::c_void;
    use std::io::Write;

    type Handle = *mut c_void;

    #[repr(C)]
    struct ModuleInfo {
        base: usize,
        size: u32,
        entry: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn K32EnumProcessModules(
            h_process: Handle,
            lph_module: *mut Handle,
            cb: u32,
            lpcb_needed: *mut u32,
        ) -> i32;
        fn K32GetModuleFileNameExW(
            h_process: Handle,
            h_module: Handle,
            lp_filename: *mut u16,
            n_size: u32,
        ) -> u32;
        fn K32GetModuleInformation(
            h_process: Handle,
            h_module: Handle,
            lp_modinfo: *mut ModuleInfo,
            cb: u32,
        ) -> i32;
        fn GetLastError() -> u32;
    }

    pub fn print_dll_info(out: &mut dyn Write) -> std::io::Result<()> {
        unsafe {
            let process = GetCurrentProcess();
            let mut needed: u32 = 0;
            K32EnumProcessModules(process, std::ptr::null_mut(), 0, &mut needed);
            let count = (needed as usize / std::mem::size_of::<Handle>()) as usize;
            let mut modules = vec![std::ptr::null_mut::<c_void>(); count.max(1)];
            if K32EnumProcessModules(
                process,
                modules.as_mut_ptr(),
                (modules.len() * std::mem::size_of::<Handle>()) as u32,
                &mut needed,
            ) == 0
            {
                return writeln!(out, "failed to enumerate modules (error {})", GetLastError());
            }
            let count = (needed as usize / std::mem::size_of::<Handle>()) as usize;
            for i in 0..count {
                let module = modules[i];
                let mut name_buf = [0u16; 1024];
                let n = K32GetModuleFileNameExW(
                    process,
                    module,
                    name_buf.as_mut_ptr(),
                    name_buf.len() as u32,
                );
                let mut info = ModuleInfo {
                    base: 0,
                    size: 0,
                    entry: 0,
                };
                K32GetModuleInformation(
                    process,
                    module,
                    &mut info,
                    std::mem::size_of::<ModuleInfo>() as u32,
                );
                let name = if n > 0 {
                    String::from_utf16_lossy(&name_buf[..n as usize])
                } else {
                    format!("<unknown {:p}>", module)
                };
                writeln!(
                    out,
                    "0x{:016x} - 0x{:016x}\t{}",
                    info.base,
                    info.base + info.size as usize,
                    name
                )?;
            }
        }
        Ok(())
    }
}

pub struct VMDynamicLibrariesDCmd;

impl DCmdInstance for VMDynamicLibrariesDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        #[cfg(windows)]
        {
            dynlibs::print_dll_info(out).map_err(|e| e.to_string())?;
            writeln_cr(out, "").map_err(|e| e.to_string())?;
        }
        #[cfg(not(windows))]
        {
            writeln_cr(out, "Dynamic library listing is not supported on this platform.")
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// VM.uptime / VM.info
// ---------------------------------------------------------------------------

pub struct VMUptimeDCmd {
    parser: DCmdParser,
}

impl VMUptimeDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "-date",
            "Add a prefix with current date",
            "BOOLEAN",
            false,
            "false",
        ));
        Self { parser }
    }
}

impl DCmdInstance for VMUptimeDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let date = self.parser.option::<bool>("-date").unwrap_or(false);
        if date {
            let secs = Os::java_time_millis() / 1000;
            let days = secs / 86400;
            let rem = secs % 86400;
            let _ = (days, rem);
            write!(out, "{} ", chrono_like_date()).map_err(|e| e.to_string())?;
        }
        let uptime = Os::vm_uptime_ms() as f64 / 1000.0;
        writeln_cr(out, &format!("{:.3} s", uptime)).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }
    fn cleanup(&mut self) {
        self.parser.cleanup();
    }
    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

fn chrono_like_date() -> String {
    let secs = Os::java_time_millis() / 1000;
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        y,
        m,
        d,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

pub struct VMInfoDCmd;

impl DCmdInstance for VMInfoDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let jdk = Java::jdk_version();
        writeln_cr(
            out,
            &format!(
                "VM version: OpenJDK 64-Bit HotSpot(TM) VM (Rust) 25-ea+1 (build {}, {} {})",
                jdk.build(),
                Os::os_name(),
                Os::os_arch()
            ),
        )
        .map_err(|e| e.to_string())?;
        writeln_cr(
            out,
            &format!(
                "JDK: {}.{}.{}.{}",
                jdk.major(),
                jdk.minor(),
                jdk.security(),
                jdk.build()
            ),
        )
        .map_err(|e| e.to_string())?;
        let universe = Universe::genesis();
        writeln_cr(
            out,
            &format!(
                "VM state: fully_initialized={} heap_kind={:?} heap_capacity={} heap_used={}",
                universe.is_fully_initialized(),
                universe.heap_kind(),
                universe.heap_capacity(),
                universe.heap_used()
            ),
        )
        .map_err(|e| e.to_string())?;
        writeln_cr(
            out,
            &format!(
                "VM uptime: {:.3} s",
                Os::vm_uptime_ms() as f64 / 1000.0
            ),
        )
        .map_err(|e| e.to_string())?;
        writeln_cr(
            out,
            &format!(
                "Threads: total={} daemon={} non_daemon={}",
                Threads::global().number_of_threads(),
                Threads::global().number_of_daemon_threads(),
                Threads::global().number_of_non_daemon_threads()
            ),
        )
        .map_err(|e| e.to_string())?;
        writeln_cr(
            out,
            &format!(
                "Memory: physical={} available={} page_size={}",
                Os::physical_memory(),
                Os::free_physical_memory(),
                Os::page_size()
            ),
        )
        .map_err(|e| e.to_string())?;
        writeln_cr(
            out,
            &format!(
                "Processors: count={} active={}",
                Os::processor_count(),
                Os::active_processor_count()
            ),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// GC.*
// ---------------------------------------------------------------------------

pub struct SystemGCDCmd;

impl DCmdInstance for SystemGCDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, _out: &mut dyn Write) -> DcmdResult<()> {
        Universe::genesis()
            .collect_heap(crate::share::gc::shared::gcCause::GCCause::JavaLangSystemGC)
            .map_err(|e| e.to_string())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

pub struct HeapInfoDCmd;

impl DCmdInstance for HeapInfoDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let universe = Universe::genesis();
        let capacity = universe.heap_capacity();
        let used = universe.heap_used();
        let free = capacity.saturating_sub(used);
        writeln_cr(
            out,
            &format!(
                "Heap: kind={:?} capacity={} used={} free={} max={}",
                universe.heap_kind(),
                capacity,
                used,
                free,
                capacity
            ),
        )
        .map_err(|e| e.to_string())?;
        let mx = Management::global().total_heap_memory_usage();
        writeln_cr(
            out,
            &format!(
                "Heap memory pools: init={} used={} committed={} max={}",
                mx.init, mx.used, mx.committed, mx.max
            ),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// GC.heap_dump
// ---------------------------------------------------------------------------

pub struct HeapDumpDCmd {
    parser: DCmdParser,
}

impl HeapDumpDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "-all",
            "Dump all objects, including unreachable objects",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_argument(DCmdArgument::<String>::new(
            "filename",
            "Name of the dump file",
            "FILE",
            true,
        ));
        parser.add_dcmd_option(DCmdArgument::<i64>::with_default(
            "-gz",
            "If specified, the heap dump is written in gzipped format using the given compression level. 1 (recommended) is the fastest, 9 the strongest compression.",
            "INT",
            false,
            "1",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "-overwrite",
            "If specified, the dump file will be overwritten if it exists",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<i64>::with_default(
            "-parallel",
            "Number of parallel threads to use for heap dump. The VM will try to use the specified number of threads, but might use fewer.",
            "INT",
            false,
            "1",
        ));
        Self { parser }
    }
}

impl DCmdInstance for HeapDumpDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let filename = self.parser.argument::<String>(0).unwrap_or_default();
        let all = self.parser.option::<bool>("-all").unwrap_or(false);
        let gzip_set = self.parser.option_is_set("-gz");
        let level: i64 = if gzip_set {
            self.parser.option::<i64>("-gz").unwrap_or(1)
        } else {
            -1
        };
        if level != -1 && !(1..=9).contains(&level) {
            writeln_cr(
                out,
                &format!("Compression level out of range (1-9): {}", level),
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
        let parallel = if self.parser.option_is_set("-parallel") {
            self.parser.option::<i64>("-parallel").unwrap_or(1)
        } else {
            1
        };
        if parallel < 0 {
            writeln_cr(out, "Invalid number of parallel dump threads.")
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
        let overwrite = self.parser.option::<bool>("-overwrite").unwrap_or(false);
        let request_gc = !all;
        let dumper = crate::share::services::heapDumper::HeapDumper::with_gc_request(request_gc);
        match dumper.dump(&filename, level as i32, overwrite, parallel.max(1) as u32) {
            Ok(bytes) => {
                writeln_cr(
                    out,
                    &format!(
                        "Heap dump file created: {} ({} bytes)",
                        filename, bytes
                    ),
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            }
            Err(e) => {
                writeln_cr(out, &format!("Dump file {} creation failed: {}", filename, e))
                    .map_err(|e2| e2.to_string())?;
                Ok(())
            }
        }
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }
    fn cleanup(&mut self) {
        self.parser.cleanup();
    }
    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

// ---------------------------------------------------------------------------
// Thread.print
// ---------------------------------------------------------------------------

pub struct ThreadDumpDCmd {
    parser: DCmdParser,
}

impl ThreadDumpDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "-l",
            "print java.util.concurrent locks",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "-e",
            "print extended thread information",
            "BOOLEAN",
            false,
            "false",
        ));
        Self { parser }
    }
}

impl DCmdInstance for ThreadDumpDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let locks = self.parser.option::<bool>("-l").unwrap_or(false);
        let extended = self.parser.option::<bool>("-e").unwrap_or(false);
        print_thread_dump(out, locks, extended).map_err(|e| e.to_string())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }
    fn cleanup(&mut self) {
        self.parser.cleanup();
    }
    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

// ---------------------------------------------------------------------------
// Compiler.queue
// ---------------------------------------------------------------------------

pub struct CompileQueueDCmd;

impl DCmdInstance for CompileQueueDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let mut buf = String::new();
        CompileBroker::global().print_compile_queues(&mut buf);
        out.write_all(buf.as_bytes()).map_err(|e| e.to_string())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Compiler.codecache
// ---------------------------------------------------------------------------

pub struct CodeCacheDCmd;

impl DCmdInstance for CodeCacheDCmd {
    fn parse(&mut self, _line: &diagnosticFramework::CmdLine, _delim: char) -> DcmdResult<()> {
        Ok(())
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let cache = crate::share::code::codeCache::CodeCache::global();
        writeln_cr(
            out,
            &format!(
                "CodeCache: segmented={} nmethods={} osr_nmethods={} blobs={}",
                cache.is_segmented(),
                cache.nmethods_count(),
                cache.osr_nmethods_count(),
                cache.blob_count()
            ),
        )
        .map_err(|e| e.to_string())?;
        writeln_cr(
            out,
            &format!(
                "CodeHeap totals: capacity={} used={} free={} free_ratio={:.2}%",
                cache.total_capacity(),
                cache.total_used(),
                cache.total_free(),
                cache.reverse_free_ratio()
            ),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "Syntax: {}", name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        Ok(())
    }
    fn cleanup(&mut self) {}
    fn argument_name_array(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// VM.symboltable / VM.stringtable / VM.native_memory
// ---------------------------------------------------------------------------

macro_rules! table_dump_dcmd {
    ($name:ident, $dcmd_name:literal, $desc:literal, $table:path, $entries_method:ident, $verbose_arg:literal) => {
        pub struct $name {
            parser: DCmdParser,
        }
        impl $name {
            pub fn new() -> Self {
                let mut parser = DCmdParser::new();
                parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
                    $verbose_arg,
                    "Dump the detailed content of the table",
                    "BOOLEAN",
                    false,
                    "false",
                ));
                Self { parser }
            }
        }
        impl DCmdInstance for $name {
            fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
                self.parser.parse(line, delim)
            }
            fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
                let verbose = self.parser.option::<bool>($verbose_arg).unwrap_or(false);
                let stats = <$table>::global().get_table_statistics();
                writeln_cr(
                    out,
                    &format!(
                        "{} statistics: table_size={} number_of_entries={} load_factor={:.2} empty_buckets={}",
                        $dcmd_name,
                        stats.table_size,
                        stats.number_of_entries,
                        stats.load_factor,
                        stats.empty_buckets
                    ),
                )
                .map_err(|e| e.to_string())?;
                if verbose {
                    writeln_cr(out, "Entries:").map_err(|e| e.to_string())?;
                    <$table>::global().$entries_method(|e| {
                        let _ = writeln!(out, "  {}", e);
                    });
                }
                Ok(())
            }
            fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
                self.parser.print_help(out, name)
            }
            fn reset(&mut self) -> DcmdResult<()> {
                self.parser.reset()
            }
            fn cleanup(&mut self) {
                self.parser.cleanup();
            }
            fn argument_name_array(&self) -> Vec<&'static str> {
                self.parser.argument_name_array()
            }
            fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
                self.parser.argument_info_array()
            }
        }
    };
}

table_dump_dcmd!(
    SymboltableDCmd,
    "VM.symboltable",
    "Dump symbol table.",
    crate::share::classfile::symbolTable::SymbolTable,
    symbols_do,
    "-verbose"
);
table_dump_dcmd!(
    StringtableDCmd,
    "VM.stringtable",
    "Dump string table.",
    crate::share::classfile::stringTable::StringTable,
    strings_do,
    "-verbose"
);

pub struct NMTDCmd {
    parser: DCmdParser,
}

impl NMTDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "summary",
            "request runtime to report current memory summary",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "detail",
            "request runtime to report memory allocation >= 1K by each callsite",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "baseline",
            "request runtime to baseline current memory usage",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "summary.diff",
            "request runtime to report memory summary comparison against previous baseline",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "detail.diff",
            "request runtime to report memory detail comparison against previous baseline",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
            "statistics",
            "print tracker statistics for tuning purpose",
            "BOOLEAN",
            false,
            "false",
        ));
        parser.add_dcmd_option(DCmdArgument::<String>::with_default(
            "scale",
            "Memory usage in which scale, KB, MB or GB",
            "STRING",
            false,
            "KB",
        ));
        Self { parser }
    }
}

impl DCmdInstance for NMTDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }
    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let summary = self.parser.option::<bool>("summary").unwrap_or(false);
        let detail = self.parser.option::<bool>("detail").unwrap_or(false);
        let baseline = self.parser.option::<bool>("baseline").unwrap_or(false);
        let _scale = self.parser.option::<String>("scale");
        if baseline {
            MemTracker::global().reset();
            writeln_cr(out, "Native memory tracking baseline established").map_err(|e| e.to_string())?;
            return Ok(());
        }
        if summary {
            let report = MemTracker::global().summary_report();
            out.write_all(report.as_bytes()).map_err(|e| e.to_string())?;
            return Ok(());
        }
        if detail {
            let report = MemTracker::global().detail_report();
            out.write_all(report.as_bytes()).map_err(|e| e.to_string())?;
            return Ok(());
        }
        let report = MemTracker::native_memory_report(false);
        out.write_all(report.as_bytes()).map_err(|e| e.to_string())
    }
    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }
    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }
    fn cleanup(&mut self) {
        self.parser.cleanup();
    }
    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }
    fn argument_info_array(&self) -> Vec<DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}

// ---------------------------------------------------------------------------
// Factory registration
// ---------------------------------------------------------------------------

fn reg(
    name: &'static str,
    description: &'static str,
    impact: &'static str,
    num_arguments: usize,
    export_flags: u32,
    enabled: bool,
    hidden: bool,
    ctor: fn() -> Box<dyn DCmdInstance>,
) {
    diagnosticFramework::register_dcmd_factory(Box::new(
        diagnosticFramework::DCmdFactoryImpl::new(
            name,
            description,
            impact,
            num_arguments,
            export_flags,
            enabled,
            hidden,
            ctor,
        ),
    ));
}

macro_rules! simple_dcmd {
    ($ctor:expr, $name:literal, $desc:literal, $impact:literal) => {
        reg($name, $desc, $impact, 0, FULL_EXPORT, true, false, $ctor)
    };
}

pub fn register_dcmds() {
    reg(
        "help",
        "For more information about a specific command use 'help <command>'. With no argument this will show a list of available commands. 'help all' will show help for all commands.",
        "Low",
        2,
        FULL_EXPORT,
        true,
        false,
        || Box::new(HelpDCmd::new()),
    );
    simple_dcmd!(
        || Box::new(VersionDCmd) as Box<dyn DCmdInstance>,
        "VM.version",
        "Print JVM version information.",
        "Low"
    );
    simple_dcmd!(
        || Box::new(CommandLineDCmd) as Box<dyn DCmdInstance>,
        "VM.command_line",
        "Print the command line used to start this VM instance.",
        "Low"
    );
    simple_dcmd!(
        || Box::new(PrintSystemPropertiesDCmd) as Box<dyn DCmdInstance>,
        "VM.system_properties",
        "Print system properties.",
        "Low"
    );
    reg(
        "VM.flags",
        "Print VM flag options and their current values.",
        "Low",
        1,
        FULL_EXPORT,
        true,
        false,
        || Box::new(PrintVMFlagsDCmd::new()),
    );
    reg(
        "VM.set_flag",
        "Sets VM flag option using the provided value.",
        "Low",
        2,
        FULL_EXPORT,
        true,
        false,
        || Box::new(SetVMFlagDCmd::new()),
    );
    simple_dcmd!(
        || Box::new(VMDynamicLibrariesDCmd) as Box<dyn DCmdInstance>,
        "VM.dynlibs",
        "Print loaded dynamic libraries.",
        "Low"
    );
    reg(
        "VM.uptime",
        "Print VM uptime.",
        "Low",
        1,
        FULL_EXPORT,
        true,
        false,
        || Box::new(VMUptimeDCmd::new()),
    );
    simple_dcmd!(
        || Box::new(VMInfoDCmd) as Box<dyn DCmdInstance>,
        "VM.info",
        "Print information about JVM environment and status.",
        "Low"
    );
    simple_dcmd!(
        || Box::new(SystemGCDCmd) as Box<dyn DCmdInstance>,
        "GC.run",
        "Call java.lang.System.gc().",
        "Medium: Depends on Java heap size and content."
    );
    simple_dcmd!(
        || Box::new(HeapInfoDCmd) as Box<dyn DCmdInstance>,
        "GC.heap_info",
        "Provide generic Java heap information.",
        "Medium"
    );
    reg(
        "GC.heap_dump",
        "Generate a HPROF format dump of the Java heap.",
        "High: Depends on Java heap size and content. Request a full GC unless the '-all' option is specified.",
        5,
        diagnosticFramework::DCMD_SOURCE_INTERNAL | diagnosticFramework::DCMD_SOURCE_ATTACH_API,
        true,
        false,
        || Box::new(HeapDumpDCmd::new()),
    );
    reg(
        "Thread.print",
        "Print all threads with stacktraces.",
        "Medium: Depends on the number of threads.",
        2,
        FULL_EXPORT,
        true,
        false,
        || Box::new(ThreadDumpDCmd::new()),
    );
    simple_dcmd!(
        || Box::new(CompileQueueDCmd) as Box<dyn DCmdInstance>,
        "Compiler.queue",
        "Print methods queued for compilation.",
        "Low"
    );
    simple_dcmd!(
        || Box::new(CodeCacheDCmd) as Box<dyn DCmdInstance>,
        "Compiler.codecache",
        "Print code cache layout and bounds.",
        "Low"
    );
    reg(
        "VM.symboltable",
        "Dump symbol table.",
        "Medium: Depends on Java content.",
        1,
        FULL_EXPORT,
        true,
        false,
        || Box::new(SymboltableDCmd::new()),
    );
    reg(
        "VM.stringtable",
        "Dump string table.",
        "Medium: Depends on Java content.",
        1,
        FULL_EXPORT,
        true,
        false,
        || Box::new(StringtableDCmd::new()),
    );
    reg(
        "VM.native_memory",
        "Native Memory Tracking",
        "Medium",
        7,
        FULL_EXPORT,
        true,
        false,
        || Box::new(NMTDCmd::new()),
    );
}

pub fn ensure_registered() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(register_dcmds);
}
