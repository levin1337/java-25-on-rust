/*
 * Port of OpenJDK 25 HotSpot Arguments (share/runtime/arguments.hpp / arguments.cpp)
 *
 * Implements JVM command-line argument processing, memory sizing (-Xms, -Xmx, -Xss),
 * garbage collector selection (-XX:+UseG1GC, -XX:+UseParallelGC, etc.),
 * execution mode selection (-Xint, -Xcomp, -Xmixed), and ergonomics validation.
 */

use std::collections::HashMap;
use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GCSelected {
    G1,
    Parallel,
    Serial,
    Epsilon,
    Z,
    Shenandoah,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Mixed,
    Interpreter,
    Compiler,
}

#[derive(Debug, Clone)]
pub struct Arguments {
    pub jar: Option<String>,
    pub classpath: Option<String>,
    pub properties: HashMap<String, String>,
    pub main_class: Option<String>,
    pub app_arguments: Vec<String>,

    // Memory sizing
    pub initial_heap_size: usize,
    pub max_heap_size: usize,
    pub thread_stack_size: usize,
    pub max_metaspace_size: usize,

    // GC & JIT selections
    pub gc: GCSelected,
    pub execution_mode: ExecutionMode,
    pub tiered_compilation: bool,
    pub tiered_stop_at_level: u8,

    // Diagnostics / platform flags
    /// -Xlog:<selectors>[:<output options>]
    pub xlog: Option<String>,
    /// -XX:NativeMemoryTracking={off,summary,detail}
    pub native_memory_tracking: Option<String>,
    /// -Xshare:{off,on,dump,auto,aot}
    pub share_mode: Option<String>,

    // -XX flag table: every `-XX:` option not consumed by a dedicated
    // field above lands here (key is the flag name without the +/- or
    // `Flag=` prefix decoration; value is "true"/"false" for boolean
    // flags or the raw value string otherwise).  Mirrors the
    // `FLAG_IS_CMDLINE` portion of HotSpot's arguments.cpp.
    pub xx_flags: HashMap<String, String>,
    /// Whether the user explicitly sized the heap (-Xms/-Xmx/-XX
    /// InitialHeapSize/MaxHeapSize).  Ergonomics only fills defaults
    /// for unset bounds, like `Arguments::set_heap_size`.
    pub heap_size_user_set: bool,
}

impl Default for Arguments {
    fn default() -> Self {
        Self {
            jar: None,
            classpath: None,
            properties: HashMap::new(),
            main_class: None,
            app_arguments: Vec::new(),

            initial_heap_size: 128 * 1024 * 1024,  // 128MB
            max_heap_size: 512 * 1024 * 1024,      // 512MB
            thread_stack_size: 1024 * 1024,        // 1MB
            max_metaspace_size: 256 * 1024 * 1024, // 256MB

            gc: GCSelected::G1,
            execution_mode: ExecutionMode::Mixed,
            tiered_compilation: true,
            tiered_stop_at_level: 4,

            xlog: None,
            native_memory_tracking: None,
            share_mode: None,

            xx_flags: HashMap::new(),
            heap_size_user_set: false,
        }
    }
}

impl Arguments {
    pub fn parse<I, T>(args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut parsed = Self::default();
        let raw_args: Vec<String> = args
            .into_iter()
            .map(|a| a.into().to_string_lossy().into_owned())
            .collect();

        let mut iter = raw_args.into_iter();
        while let Some(arg) = iter.next() {
            if arg == "-jar" {
                if let Some(jar_path) = iter.next() {
                    parsed.jar = Some(jar_path);
                    parsed.app_arguments.extend(iter);
                    break;
                }
            } else if arg == "-cp" || arg == "-classpath" {
                if let Some(cp) = iter.next() {
                    parsed.classpath = Some(cp);
                }
            } else if let Some(ms_str) = arg.strip_prefix("-Xms") {
                if let Some(sz) = Self::parse_memory_size(ms_str) {
                    parsed.initial_heap_size = sz;
                    parsed.heap_size_user_set = true;
                }
            } else if let Some(mx_str) = arg.strip_prefix("-Xmx") {
                if let Some(sz) = Self::parse_memory_size(mx_str) {
                    parsed.max_heap_size = sz;
                    parsed.heap_size_user_set = true;
                }
            } else if let Some(ss_str) = arg.strip_prefix("-Xss") {
                if let Some(sz) = Self::parse_memory_size(ss_str) {
                    parsed.thread_stack_size = sz;
                }
            } else if arg == "-Xint" {
                parsed.execution_mode = ExecutionMode::Interpreter;
            } else if arg == "-Xcomp" {
                parsed.execution_mode = ExecutionMode::Compiler;
            } else if arg == "-Xmixed" {
                parsed.execution_mode = ExecutionMode::Mixed;
            } else if arg == "-XX:+UseG1GC" {
                parsed.gc = GCSelected::G1;
            } else if arg == "-XX:+UseParallelGC" {
                parsed.gc = GCSelected::Parallel;
            } else if arg == "-XX:+UseSerialGC" {
                parsed.gc = GCSelected::Serial;
            } else if arg == "-XX:+UseEpsilonGC" {
                parsed.gc = GCSelected::Epsilon;
            } else if arg == "-XX:+UseZGC" {
                parsed.gc = GCSelected::Z;
            } else if arg == "-XX:+UseShenandoahGC" {
                parsed.gc = GCSelected::Shenandoah;
            } else if arg == "-XX:+TieredCompilation" {
                parsed.tiered_compilation = true;
            } else if arg == "-XX:-TieredCompilation" {
                parsed.tiered_compilation = false;
            } else if let Some(lvl_str) = arg.strip_prefix("-XX:TieredStopAtLevel=") {
                if let Ok(lvl) = lvl_str.parse::<u8>() {
                    parsed.tiered_stop_at_level = lvl.clamp(1, 4);
                }
            } else if let Some(xlog) = arg.strip_prefix("-Xlog:") {
                parsed.xlog = Some(xlog.to_string());
            } else if let Some(xlog) = arg.strip_prefix("-Xlog") {
                // Bare "-Xlog" enables default info logging.
                let _ = xlog;
                parsed.xlog = Some(String::new());
            } else if let Some(nmt) = arg.strip_prefix("-XX:NativeMemoryTracking=") {
                parsed.native_memory_tracking = Some(nmt.to_string());
            } else if let Some(xx) = arg.strip_prefix("-XX:") {
                // Generic -XX option: `+Flag`/`-Flag` booleans and
                // `Flag=value` diagnostics land in the flag table.  All
                // dedicated -XX arms above take precedence.
                parsed.set_xx_flag(xx);
                if xx.starts_with("InitialHeapSize=")
                    || xx.starts_with("MinHeapSize=")
                    || xx.starts_with("MaxHeapSize=")
                    || xx.starts_with("NewSize=")
                {
                    parsed.heap_size_user_set = true;
                }
            } else if let Some(share) = arg.strip_prefix("-Xshare:") {
                parsed.share_mode = Some(share.to_string());
            } else if let Some(prop) = arg.strip_prefix("-D") {
                if let Some((k, v)) = prop.split_once('=') {
                    parsed.properties.insert(k.to_string(), v.to_string());
                } else {
                    parsed
                        .properties
                        .insert(prop.to_string(), "true".to_string());
                }
            } else if !arg.starts_with('-') && parsed.main_class.is_none() {
                parsed.main_class = Some(arg);
                parsed.app_arguments.extend(iter);
                break;
            }
        }

        parsed.apply_ergo();
        parsed.apply_diagnostic_flags();
        parsed
    }

    /// Applies diagnostics/platform flags to the corresponding subsystems
    /// (unified logging, NMT, CDS) — mirrors the flag application phase of
    /// Arguments::parse / os::init in HotSpot.
    pub fn apply_diagnostic_flags(&self) {
        use crate::share::cds::cdsConfig::{CDSConfig, CdsMode};
        use crate::share::logging::log::Log;
        use crate::share::logging::logConfiguration::LogConfiguration;
        use crate::share::nmt::memTracker::MemTracker;

        if let Some(xlog) = &self.xlog {
            let cfg = LogConfiguration::global();
            if xlog.is_empty() {
                cfg.parse_log_argument("all=info").ok();
            } else if let Err(e) = cfg.parse_xlog(xlog) {
                // HotSpot prints a warning and continues.
                eprintln!("Warning: Invalid -Xlog option '{}': {}", xlog, e);
            }
            Log::reset_outputs();
        }

        if let Some(nmt) = &self.native_memory_tracking {
            MemTracker::initialize_from_flag(Some(nmt));
        }

        if let Some(share) = &self.share_mode {
            let mode = match share.to_ascii_lowercase().as_str() {
                "off" => Some(CdsMode::Off),
                "on" | "auto" => Some(CdsMode::UseStatic),
                "dump" => Some(CdsMode::DumpStatic),
                "aot" => Some(CdsMode::DumpAot),
                _ => None,
            };
            if let Some(m) = mode {
                CDSConfig::set_mode(m);
            }
        }
    }

    /// Parses human-readable memory size string (e.g. "512m", "2g", "128k")
    pub fn parse_memory_size(s: &str) -> Option<usize> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let (digits, multiplier) = if s.ends_with(|c| matches!(c, 'k' | 'K')) {
            (&s[..s.len() - 1], 1024usize)
        } else if s.ends_with(|c| matches!(c, 'm' | 'M')) {
            (&s[..s.len() - 1], 1024 * 1024usize)
        } else if s.ends_with(|c| matches!(c, 'g' | 'G')) {
            (&s[..s.len() - 1], 1024 * 1024 * 1024usize)
        } else if s.ends_with(|c| matches!(c, 't' | 'T')) {
            (&s[..s.len() - 1], 1024 * 1024 * 1024 * 1024usize)
        } else {
            (s, 1usize)
        };

        let val: usize = digits.parse().ok()?;
        val.checked_mul(multiplier)
    }

    /// Records one `-XX:` payload (the part after `-XX:`) into the flag
    /// table.  `+Name`/`-Name` store "true"/"false"; `Name=value` stores
    /// the value; a bare `Name` is treated as an enabled boolean.
    pub fn set_xx_flag(&mut self, payload: &str) {
        if let Some(name) = payload.strip_prefix('+') {
            self.xx_flags.insert(name.to_string(), "true".to_string());
        } else if let Some(name) = payload.strip_prefix('-') {
            self.xx_flags.insert(name.to_string(), "false".to_string());
        } else if let Some((name, value)) = payload.split_once('=') {
            self.xx_flags.insert(name.to_string(), value.to_string());
        } else {
            self.xx_flags.insert(payload.to_string(), "true".to_string());
        }
    }

    /// Boolean value of an `-XX` flag; `None` when the flag was not
    /// given on the command line (HotSpot's `FLAG_IS_DEFAULT` state).
    pub fn xx_flag_bool(&self, name: &str) -> Option<bool> {
        self.xx_flags.get(name).map(|v| v == "true")
    }

    /// Integer value of an `-XX:Name=value` flag.
    pub fn xx_flag_u64(&self, name: &str) -> Option<u64> {
        self.xx_flags.get(name).and_then(|v| v.parse().ok())
    }

    /// `-XX:HeapSizePerCPU` (ergonomics.cpp `HeapSizePerCPU` flag).
    /// Default 0 = disabled, matching the product default.
    pub fn heap_size_per_cpu(&self) -> u64 {
        self.xx_flag_u64("HeapSizePerCPU").unwrap_or(0)
    }

    /// `-XX:MaxRAMPercentage` — percent of physical memory usable for
    /// the max heap; HotSpot product default is 25.0.
    pub fn max_ram_percentage(&self) -> f64 {
        self.xx_flag_u64("MaxRAMPercentage")
            .map(|v| v as f64)
            .unwrap_or(25.0)
    }

    /// `-XX:InitialRAMPercentage` — percent of physical memory for the
    /// initial heap; HotSpot product default is 1.5625 (1/64).
    pub fn initial_ram_percentage(&self) -> f64 {
        self.xx_flag_u64("InitialRAMPercentage")
            .map(|v| v as f64)
            .unwrap_or(1.5625)
    }

    /// `-XX:ActiveProcessorCount` override used by ergonomics.
    pub fn active_processor_count(&self) -> usize {
        self.xx_flag_u64("ActiveProcessorCount")
            .map(|v| v.max(1) as usize)
            .unwrap_or_else(crate::share::runtime::os::Os::active_processor_count)
    }

    /// Ergonomic default for the maximum heap, mirroring
    /// `Arguments::set_heap_size` / `ergonomics.cpp`: `HeapSizePerCPU`
    /// (when non-zero, ParallelGC-style sizing) or `MaxRAMPercentage`
    /// of physical memory, clamped to sane bounds.
    pub fn ergonomic_max_heap(&self) -> usize {
        let per_cpu = self.heap_size_per_cpu();
        let phys = crate::share::runtime::os::Os::physical_memory() as usize;
        let computed = if per_cpu != 0 {
            per_cpu as usize * self.active_processor_count()
        } else {
            (phys as f64 * self.max_ram_percentage() / 100.0) as usize
        };
        // HotSpot clamps ergonomic heap to at least 8MB and at most
        // physical memory; keep the same floor/ceiling shape.
        computed.clamp(8 * 1024 * 1024, phys.max(8 * 1024 * 1024))
    }

    /// Ergonomic default for the initial heap (`InitialRAMPercentage`
    /// of physical memory, clamped, and never above the max heap).
    pub fn ergonomic_initial_heap(&self) -> usize {
        let phys = crate::share::runtime::os::Os::physical_memory() as usize;
        let computed =
            (phys as f64 * self.initial_ram_percentage() / 100.0) as usize;
        computed
            .clamp(8 * 1024 * 1024, self.max_heap_size.max(8 * 1024 * 1024))
    }

    /// JNI `JavaVMInitArgs` entry point: HotSpot feeds the option
    /// strings of `JNI_CreateJavaVM` through the same `Arguments`
    /// parser as the command line (`jni.cpp` -> `arguments.cpp`).
    /// `options` are the raw `JavaVMOption.optionString` values
    /// (`-Xmx512m`, `-Dk=v`, `-XX:+UseG1GC`, ...); positional entries
    /// are treated as the main class and the rest as app arguments.
    /// Returns `Err` when `ignore_unrecognized` is false and an option
    /// is not a recognized JVM flag/property — HotSpot aborts VM
    /// creation in that case.
    pub fn parse_jni_args(
        version: i32,
        options: &[String],
        ignore_unrecognized: bool,
    ) -> Result<Self, String> {
        // Supported JNI versions: 1.1 .. 1.8, 9, 10, 11(skip), 17, 19,
        // 20, 21, 24, 25 — same set jvmExports accepts.
        const JNI_VERSIONS: &[i32] = &[
            0x0001_0001, 0x0001_0002, 0x0001_0004, 0x0001_0006, 0x0001_0008,
            0x0009_0000, 0x000A_0000, 0x0011_0000, 0x0013_0000, 0x0014_0000,
            0x0015_0000, 0x0018_0000, 0x0019_0000,
        ];
        if !JNI_VERSIONS.contains(&version) {
            return Err(format!("Unsupported JNI version 0x{version:08x}"));
        }
        // Split the option list into VM options (fed through the regular
        // parser in one pass, exactly like HotSpot feeding JNI options
        // into Arguments::parse) and -D system properties.
        let mut vm_opts: Vec<String> = Vec::new();
        let mut properties: Vec<(String, String)> = Vec::new();
        let mut iter = options.iter().peekable();
        while let Some(arg) = iter.next() {
            if let Some(prop) = arg.strip_prefix("-D") {
                let (k, v) = prop.split_once('=').unwrap_or((prop, ""));
                if !k.is_empty() {
                    properties.push((k.to_string(), v.to_string()));
                }
                continue;
            }
            if matches!(arg.as_str(), "-cp" | "-classpath" | "-jar") {
                // These consume the following token as their value.
                vm_opts.push(arg.clone());
                if let Some(value) = iter.next() {
                    vm_opts.push(value.clone());
                }
                continue;
            }
            if arg.starts_with('-') {
                vm_opts.push(arg.clone());
                continue;
            }
            // Positional arguments are not VM options.  HotSpot's JNI
            // path rejects them unless ignoreUnrecognized is set.
            if !ignore_unrecognized {
                return Err(format!("Unrecognized option: {arg}"));
            }
        }
        let mut parsed = Self::parse(vm_opts);
        for (k, v) in properties {
            parsed.properties.insert(k, v);
        }
        Ok(parsed)
    }

    /// HotSpot ergonomics: adjusts memory bounds based on constraints
    pub fn apply_ergo(&mut self) {
        if !self.heap_size_user_set {
            let ergonomic_max = self.ergonomic_max_heap();
            if ergonomic_max < self.max_heap_size {
                self.max_heap_size = ergonomic_max;
            }
            let ergonomic_min = self.ergonomic_initial_heap();
            if self.initial_heap_size > self.max_heap_size {
                self.initial_heap_size = self.max_heap_size;
            } else if ergonomic_min < self.initial_heap_size {
                self.initial_heap_size = ergonomic_min;
            }
        }
        if self.initial_heap_size > self.max_heap_size {
            self.max_heap_size = self.initial_heap_size;
        }
        if self.execution_mode == ExecutionMode::Interpreter {
            self.tiered_compilation = false;
        }
    }

    /// Validates HotSpot argument limits and range constraints
    pub fn check_constraints(&self) -> Result<(), &'static str> {
        if self.initial_heap_size > self.max_heap_size {
            return Err("Initial heap size set to a larger value than the maximum heap size");
        }
        if self.thread_stack_size < 32 * 1024 {
            return Err("The stack size specified is too small, Specify at least 32k");
        }
        Ok(())
    }
}
