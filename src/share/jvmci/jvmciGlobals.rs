/*
 * Port of OpenJDK 25 HotSpot JVMCI globals
 * (share/jvmci/jvmci_globals.hpp / jvmci_globals.cpp)
 *
 * Holds the JVMCI_FLAGS flag set with HotSpot defaults and the
 * JVMCIGlobals consistency checks (`check_jvmci_flags_are_consistent`,
 * `gc_supports_jvmci`, `check_jvmci_supported_gc`,
 * `enable_jvmci_product_mode`).
 */

use crate::share::compiler::compilerDefinitions::CompLevel;
use crate::share::runtime::arguments::GCSelected;
use std::sync::{OnceLock, RwLock};

pub const LIBJVMCI_ERR_FILE: &str = "hs_err_pid%p_libjvmci.log";
pub const DEFAULT_COMPILER_IDLE_DELAY: u32 = 1000;
pub const JVMCI_SHARED_LIBRARY_NAME: &str = "jvmcicompiler";

/// Word size used for `JVMCINMethodSizeLimit` default (64-bit port).
const WORD_SIZE: i64 = 8;

/// The JVMCI flag set (JVMCI_FLAGS macro expansion).
#[derive(Clone, Debug)]
pub struct JvmciFlags {
    /// -XX:+EnableJVMCI (default false)
    pub enable_jvmci: bool,
    /// -XX:+UseGraalJIT
    pub use_graal_jit: bool,
    /// -XX:+EnableJVMCIProduct
    pub enable_jvmci_product: bool,
    /// -XX:+UseJVMCICompiler
    pub use_jvmci_compiler: bool,
    /// -XX:JVMCIThreadsPerNativeLibraryRuntime=N (default 1)
    pub jvmci_threads_per_native_library_runtime: u32,
    /// -XX:JVMCICompilerIdleDelay=N (default 1000)
    pub jvmci_compiler_idle_delay: u32,
    /// -XX:+JVMCIPrintProperties
    pub jvmci_print_properties: bool,
    /// -XX:+BootstrapJVMCI
    pub bootstrap_jvmci: bool,
    /// -XX:+EagerJVMCI
    pub eager_jvmci: bool,
    /// -XX:+/-PrintBootstrap (default true)
    pub print_bootstrap: bool,
    /// -XX:JVMCIThreads=N (default 1)
    pub jvmci_threads: i32,
    /// -XX:JVMCIHostThreads=N (default 1)
    pub jvmci_host_threads: i32,
    /// -XX:MaxVectorSize=N (default 64, C2 only in HotSpot)
    pub max_vector_size: i32,
    /// -XX:+/-ReduceInitialCardMarks (default true, non-C2 only)
    pub reduce_initial_card_marks: bool,
    /// -XX:JVMCIEventLogLevel=N (default 1, range 0..=4)
    pub jvmci_event_log_level: i32,
    /// -XX:JVMCITraceLevel=N (default 0, range 0..=4)
    pub jvmci_trace_level: i32,
    /// -XX:JVMCICounterSize=N (default 0)
    pub jvmci_counter_size: i32,
    /// -XX:+/-JVMCICountersExcludeCompiler (default true)
    pub jvmci_counters_exclude_compiler: bool,
    /// -XX:JVMCINMethodSizeLimit=N (default 80K*wordSize)
    pub jvmci_nmethod_size_limit: i64,
    /// -XX:JVMCILibPath=path
    pub jvmci_lib_path: Option<String>,
    /// -XX:JVMCILibDumpJNIConfig=file
    pub jvmci_lib_dump_jni_config: Option<String>,
    /// -XX:+UseJVMCINativeLibrary
    pub use_jvmci_native_library: bool,
    /// -XX:JVMCINativeLibraryThreadFraction=F (default 0.66)
    pub jvmci_native_library_thread_fraction: f64,
    /// -XX:JVMCINativeLibraryErrorFile=file
    pub jvmci_native_library_error_file: Option<String>,
    /// -XX:+/-LibJVMCICompilerThreadHidden (default true)
    pub libjvmci_compiler_thread_hidden: bool,
    /// -XX:+UseVectorStubs (C2 only in HotSpot)
    pub use_vector_stubs: bool,
    // Non-C2 JVMCI flags (EnableVectorSupport, EnableVectorReboxing,
    // EnableVectorAggressiveReboxing, UseMultiplyToLenIntrinsic,
    // UseSquareToLenIntrinsic, UseMulAddIntrinsic,
    // UseMontgomeryMultiplyIntrinsic, UseMontgomerySquareIntrinsic) are
    // tracked for flag-consistency completeness.
    pub enable_vector_support: bool,
    pub enable_vector_reboxing: bool,
    pub enable_vector_aggressive_reboxing: bool,
    pub use_multiply_to_len_intrinsic: bool,
    pub use_square_to_len_intrinsic: bool,
    pub use_mul_add_intrinsic: bool,
    pub use_montgomery_multiply_intrinsic: bool,
    pub use_montgomery_square_intrinsic: bool,
}

impl Default for JvmciFlags {
    fn default() -> Self {
        Self {
            enable_jvmci: false,
            use_graal_jit: false,
            enable_jvmci_product: false,
            use_jvmci_compiler: false,
            jvmci_threads_per_native_library_runtime: 1,
            jvmci_compiler_idle_delay: DEFAULT_COMPILER_IDLE_DELAY,
            jvmci_print_properties: false,
            bootstrap_jvmci: false,
            eager_jvmci: false,
            print_bootstrap: true,
            jvmci_threads: 1,
            jvmci_host_threads: 1,
            max_vector_size: 64,
            reduce_initial_card_marks: true,
            jvmci_event_log_level: 1,
            jvmci_trace_level: 0,
            jvmci_counter_size: 0,
            jvmci_counters_exclude_compiler: true,
            jvmci_nmethod_size_limit: 80 * 1024 * WORD_SIZE,
            jvmci_lib_path: None,
            jvmci_lib_dump_jni_config: None,
            use_jvmci_native_library: false,
            jvmci_native_library_thread_fraction: 0.66,
            jvmci_native_library_error_file: None,
            libjvmci_compiler_thread_hidden: true,
            use_vector_stubs: false,
            enable_vector_support: false,
            enable_vector_reboxing: false,
            enable_vector_aggressive_reboxing: false,
            use_multiply_to_len_intrinsic: false,
            use_square_to_len_intrinsic: false,
            use_mul_add_intrinsic: false,
            use_montgomery_multiply_intrinsic: false,
            use_montgomery_square_intrinsic: false,
        }
    }
}

static FLAGS: OnceLock<RwLock<JvmciFlags>> = OnceLock::new();

/// Global JVMCI flags (`DECLARE_FLAGS(JVMCI_FLAGS)`).
pub fn flags() -> &'static RwLock<JvmciFlags> {
    FLAGS.get_or_init(|| RwLock::new(JvmciFlags::default()))
}

/// Snapshot of the current flags (cheap enough for flag checks).
pub fn snapshot() -> JvmciFlags {
    flags().read().unwrap().clone()
}

/// `-XX:+Flag` / `-XX:-Flag` style names of every JVMCI flag, used by
/// `JVMCIGlobals::enable_jvmci_product_mode`.
pub const JVMCI_FLAG_NAMES: &[&str] = &[
    "EnableJVMCI",
    "EnableJVMCIProduct",
    "UseJVMCICompiler",
    "JVMCIThreadsPerNativeLibraryRuntime",
    "JVMCICompilerIdleDelay",
    "JVMCIPrintProperties",
    "EagerJVMCI",
    "JVMCIThreads",
    "JVMCICounterSize",
    "JVMCICountersExcludeCompiler",
    "JVMCINMethodSizeLimit",
    "JVMCIEventLogLevel",
    "JVMCITraceLevel",
    "JVMCILibPath",
    "JVMCILibDumpJNIConfig",
    "UseJVMCINativeLibrary",
    "JVMCINativeLibraryThreadFraction",
    "JVMCINativeLibraryErrorFile",
    "LibJVMCICompilerThreadHidden",
];

impl JvmciFlags {
    /// Apply one `-XX:` style argument; returns true if the argument was
    /// recognised as a JVMCI flag.
    pub fn apply_arg(&mut self, arg: &str) -> bool {
        let body = match arg.strip_prefix("-XX:") {
            Some(b) => b,
            None => return false,
        };
        if let Some(name) = body.strip_prefix('+') {
            return self.set_bool(name, true);
        }
        if let Some(name) = body.strip_prefix('-') {
            return self.set_bool(name, false);
        }
        if let Some((name, value)) = body.split_once('=') {
            return self.set_value(name, value);
        }
        false
    }

    fn set_bool(&mut self, name: &str, value: bool) -> bool {
        match name {
            "EnableJVMCI" => self.enable_jvmci = value,
            "UseGraalJIT" => self.use_graal_jit = value,
            "EnableJVMCIProduct" => self.enable_jvmci_product = value,
            "UseJVMCICompiler" => self.use_jvmci_compiler = value,
            "JVMCIPrintProperties" => self.jvmci_print_properties = value,
            "BootstrapJVMCI" => self.bootstrap_jvmci = value,
            "EagerJVMCI" => self.eager_jvmci = value,
            "PrintBootstrap" => self.print_bootstrap = value,
            "ReduceInitialCardMarks" => self.reduce_initial_card_marks = value,
            "JVMCICountersExcludeCompiler" => self.jvmci_counters_exclude_compiler = value,
            "UseJVMCINativeLibrary" => self.use_jvmci_native_library = value,
            "LibJVMCICompilerThreadHidden" => self.libjvmci_compiler_thread_hidden = value,
            "UseVectorStubs" => self.use_vector_stubs = value,
            "EnableVectorSupport" => self.enable_vector_support = value,
            "EnableVectorReboxing" => self.enable_vector_reboxing = value,
            "EnableVectorAggressiveReboxing" => self.enable_vector_aggressive_reboxing = value,
            "UseMultiplyToLenIntrinsic" => self.use_multiply_to_len_intrinsic = value,
            "UseSquareToLenIntrinsic" => self.use_square_to_len_intrinsic = value,
            "UseMulAddIntrinsic" => self.use_mul_add_intrinsic = value,
            "UseMontgomeryMultiplyIntrinsic" => {
                self.use_montgomery_multiply_intrinsic = value
            }
            "UseMontgomerySquareIntrinsic" => self.use_montgomery_square_intrinsic = value,
            _ => return false,
        }
        true
    }

    fn set_value(&mut self, name: &str, value: &str) -> bool {
        match name {
            "JVMCIThreadsPerNativeLibraryRuntime" => {
                self.jvmci_threads_per_native_library_runtime =
                    value.parse().unwrap_or(self.jvmci_threads_per_native_library_runtime)
            }
            "JVMCICompilerIdleDelay" => {
                self.jvmci_compiler_idle_delay = value.parse().unwrap_or(self.jvmci_compiler_idle_delay)
            }
            "JVMCIThreads" => self.jvmci_threads = value.parse().unwrap_or(self.jvmci_threads),
            "JVMCIHostThreads" => {
                self.jvmci_host_threads = value.parse().unwrap_or(self.jvmci_host_threads)
            }
            "MaxVectorSize" => self.max_vector_size = value.parse().unwrap_or(self.max_vector_size),
            "JVMCIEventLogLevel" => {
                self.jvmci_event_log_level =
                    value.parse().unwrap_or(self.jvmci_event_log_level).clamp(0, 4)
            }
            "JVMCITraceLevel" => {
                self.jvmci_trace_level =
                    value.parse().unwrap_or(self.jvmci_trace_level).clamp(0, 4)
            }
            "JVMCICounterSize" => {
                self.jvmci_counter_size = value.parse().unwrap_or(self.jvmci_counter_size)
            }
            "JVMCINMethodSizeLimit" => {
                self.jvmci_nmethod_size_limit =
                    value.parse().unwrap_or(self.jvmci_nmethod_size_limit)
            }
            "JVMCILibPath" => self.jvmci_lib_path = Some(value.to_string()),
            "JVMCILibDumpJNIConfig" => {
                self.jvmci_lib_dump_jni_config = Some(value.to_string())
            }
            "JVMCINativeLibraryThreadFraction" => {
                self.jvmci_native_library_thread_fraction =
                    value.parse().unwrap_or(self.jvmci_native_library_thread_fraction)
            }
            "JVMCINativeLibraryErrorFile" => {
                self.jvmci_native_library_error_file = Some(value.to_string())
            }
            _ => return false,
        }
        true
    }

    /// Whether the flag still holds its HotSpot default (`FLAG_IS_DEFAULT`
    /// analogue — tracked via `explicitly_set` is not modelled; callers
    /// use `is_default` helpers on the fields that matter).
    pub fn is_default(&self, name: &str) -> bool {
        match name {
            "EnableJVMCI" => !self.enable_jvmci,
            "UseJVMCICompiler" => !self.use_jvmci_compiler,
            "EagerJVMCI" => !self.eager_jvmci,
            "UseJVMCINativeLibrary" => !self.use_jvmci_native_library,
            _ => false,
        }
    }
}

/// `JVMCIGlobals` analogue.
pub struct JvmciGlobals;

impl JvmciGlobals {
    /// Port of `check_jvmci_flags_are_consistent`.  `tiered_stop_at_level`
    /// supplies the non-JVMCI `TieredStopAtLevel` flag.
    /// `shared_library_exists` reports whether a libjvmci shared library
    /// was located (defaults to the `JVMCI::shared_library_exists` probe).
    pub fn check_jvmci_flags_are_consistent(
        tiered_stop_at_level: u8,
        shared_library_exists: impl Fn() -> bool,
    ) -> Result<(), String> {
        let mut f = flags().write().unwrap();

        // EnableJVMCIProduct defaults EnableJVMCI and UseJVMCICompiler
        // before the consistency checks below.
        if f.enable_jvmci_product {
            if !f.enable_jvmci {
                f.enable_jvmci = true;
            }
            if !f.use_jvmci_compiler {
                f.use_jvmci_compiler = true;
            }
        }

        // CHECK_NOT_SET(FLAG, GUARD): flag must not be non-default while
        // the guard flag is off.  `is_set` records whether the flag
        // differs from its HotSpot default.
        let not_set_checks: &[(&str, &str, bool, bool)] = &[
            ("BootstrapJVMCI", "UseJVMCICompiler", f.bootstrap_jvmci && !f.use_jvmci_compiler, false),
            ("PrintBootstrap", "UseJVMCICompiler", !f.print_bootstrap && !f.use_jvmci_compiler, false),
            ("JVMCIThreads", "UseJVMCICompiler", f.jvmci_threads != 1 && !f.use_jvmci_compiler, false),
            (
                "JVMCIHostThreads",
                "UseJVMCICompiler",
                f.jvmci_host_threads != 1 && !f.use_jvmci_compiler,
                false,
            ),
            (
                "LibJVMCICompilerThreadHidden",
                "UseJVMCICompiler",
                !f.libjvmci_compiler_thread_hidden && !f.use_jvmci_compiler,
                false,
            ),
            ("JVMCIEventLogLevel", "EnableJVMCI", f.jvmci_event_log_level != 1 && !f.enable_jvmci, false),
            ("JVMCITraceLevel", "EnableJVMCI", f.jvmci_trace_level != 0 && !f.enable_jvmci, false),
            ("JVMCICounterSize", "EnableJVMCI", f.jvmci_counter_size != 0 && !f.enable_jvmci, false),
            (
                "JVMCICountersExcludeCompiler",
                "EnableJVMCI",
                !f.jvmci_counters_exclude_compiler && !f.enable_jvmci,
                false,
            ),
            (
                "JVMCINMethodSizeLimit",
                "EnableJVMCI",
                f.jvmci_nmethod_size_limit != 80 * 1024 * WORD_SIZE && !f.enable_jvmci,
                false,
            ),
            (
                "JVMCIPrintProperties",
                "EnableJVMCI",
                f.jvmci_print_properties && !f.enable_jvmci,
                false,
            ),
            (
                "JVMCIThreadsPerNativeLibraryRuntime",
                "EnableJVMCI",
                f.jvmci_threads_per_native_library_runtime != 1 && !f.enable_jvmci,
                false,
            ),
            (
                "JVMCICompilerIdleDelay",
                "EnableJVMCI",
                f.jvmci_compiler_idle_delay != DEFAULT_COMPILER_IDLE_DELAY && !f.enable_jvmci,
                false,
            ),
            (
                "UseJVMCINativeLibrary",
                "EnableJVMCI",
                f.use_jvmci_native_library && !f.enable_jvmci,
                false,
            ),
            (
                "JVMCINativeLibraryThreadFraction",
                "EnableJVMCI",
                (f.jvmci_native_library_thread_fraction - 0.66).abs() > f64::EPSILON
                    && !f.enable_jvmci,
                false,
            ),
            (
                "JVMCINativeLibraryErrorFile",
                "EnableJVMCI",
                f.jvmci_native_library_error_file.is_some() && !f.enable_jvmci,
                false,
            ),
            ("JVMCILibPath", "EnableJVMCI", f.jvmci_lib_path.is_some() && !f.enable_jvmci, false),
            (
                "JVMCILibDumpJNIConfig",
                "EnableJVMCI",
                f.jvmci_lib_dump_jni_config.is_some() && !f.enable_jvmci,
                false,
            ),
        ];
        for (flag, guard, violated, _is_default) in not_set_checks {
            if *violated {
                return Err(format!(
                    "Improperly specified VM option '{flag}': '{guard}' must be enabled"
                ));
            }
        }

        if f.use_jvmci_compiler {
            if !f.enable_jvmci {
                return Err(
                    "Improperly specified VM option UseJVMCICompiler: EnableJVMCI cannot be disabled"
                        .to_string(),
                );
            }
            f.enable_jvmci = true;
            if !f.eager_jvmci {
                f.eager_jvmci = true;
            }
        }

        if f.enable_jvmci && !f.use_jvmci_native_library {
            if shared_library_exists() {
                // JVMCI native library present: enable by default.
                f.use_jvmci_native_library = true;
            }
        }

        if f.use_jvmci_compiler {
            if f.bootstrap_jvmci && f.use_jvmci_native_library {
                return Err(
                    "-XX:+BootstrapJVMCI is not compatible with -XX:+UseJVMCINativeLibrary"
                        .to_string(),
                );
            }
            if f.bootstrap_jvmci
                && (tiered_stop_at_level as i32) < CompLevel::FullOptimization.as_i32()
            {
                return Err(format!(
                    "-XX:+BootstrapJVMCI is not compatible with -XX:TieredStopAtLevel={tiered_stop_at_level}"
                ));
            }
        }

        if !f.enable_jvmci && f.eager_jvmci {
            // EagerJVMCI is reset to default (not an error).
            f.eager_jvmci = false;
        }

        if f.jvmci_lib_dump_jni_config.is_some() {
            // In HotSpot this opens a fileStream for the JNI config dump;
            // there is no libjvmci to dump, so record the request and let
            // the caller decide (dump happens in initialize_globals).
        }

        Ok(())
    }

    /// `JVMCIGlobals::enable_jvmci_product_mode`: convert JVMCI
    /// experimental flags to product flags and set EnableJVMCIProduct
    /// (+UseGraalJIT when requested).
    pub fn enable_jvmci_product_mode(use_graal_jit: bool) -> bool {
        // All listed flags are already plain fields; "clearing
        // experimental" is a no-op for the data model.  The effect on
        // defaults is deferred to check_jvmci_flags_are_consistent.
        let mut f = flags().write().unwrap();
        f.enable_jvmci_product = true;
        if use_graal_jit {
            f.use_graal_jit = true;
        }
        true
    }

    /// `JVMCIGlobals::gc_supports_jvmci`.
    pub fn gc_supports_jvmci(gc: GCSelected) -> bool {
        // All current collectors support JVMCI (mirrors the C++
        // disjunction over the Use*GC flags).
        matches!(
            gc,
            GCSelected::Serial
                | GCSelected::Parallel
                | GCSelected::G1
                | GCSelected::Z
                | GCSelected::Shenandoah
                | GCSelected::Epsilon
        )
    }

    /// `JVMCIGlobals::check_jvmci_supported_gc`: fatal when JVMCI is
    /// enabled with an unsupported GC.  Returns the fatal message, if any.
    pub fn check_jvmci_supported_gc(gc: GCSelected) -> Option<String> {
        let enabled = flags().read().unwrap().enable_jvmci;
        if enabled && !Self::gc_supports_jvmci(gc) {
            return Some("JVMCI does not support the selected GC".to_string());
        }
        None
    }
}
