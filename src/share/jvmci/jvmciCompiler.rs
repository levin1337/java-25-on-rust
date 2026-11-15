/*
 * Port of OpenJDK 25 HotSpot JVMCICompiler
 * (share/jvmci/jvmciCompiler.hpp / jvmciCompiler.cpp)
 *
 * The `AbstractCompiler` implementation backing JVMCI compilations.
 * There is exactly one instance (`_instance`), created when the VM
 * decides to launch the JVMCI compiler.  Without a JVMCI compiler
 * (Graal/libjvmci) every `compile_method` fails with an honest
 * unsupported error — never fake code.
 */

use crate::share::compiler::abstractCompiler::{
    AbstractCompiler, CompileError, CompilerType,
};
use crate::share::compiler::compileTask::{CompileOutcome, CompileTask};
use crate::share::compiler::compilerDefinitions::CompLevel;
use crate::share::jvmci::jvmci::{compilation_ticks, JVMCI};
use crate::share::jvmci::jvmciEnv::JvmciEnv;
use crate::share::jvmci::jvmciExceptions::{JvmciException, JVMCI_NOT_ENABLED_ERROR_MESSAGE};
use crate::share::jvmci::jvmciGlobals;
use crate::share::jvmci::jvmciRuntime::JvmciRuntime;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, OnceLock};

/// `JVMCICompiler::CodeInstallStats`: counters for code installs
/// through the CompileBroker (jit) vs hosted compilations.
#[derive(Debug, Default)]
pub struct CodeInstallStats {
    pub count: AtomicI32,
    pub code_blobs_size: AtomicI32,
    pub code_blobs_code_size: AtomicI32,
    /// elapsedTimer analogue (nanoseconds accumulated).
    pub timer_ns: AtomicI64,
}

static _CODE_INSTALL_STATS_TIMER: OnceLock<()> = OnceLock::new();

impl CodeInstallStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// `timer()`.
    pub fn timer_ns(&self) -> i64 {
        self.timer_ns.load(Ordering::Relaxed)
    }

    /// `on_install`.
    pub fn on_install(&self, blob_size: usize, code_size: usize) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.code_blobs_size
            .fetch_add(blob_size as i32, Ordering::Relaxed);
        self.code_blobs_code_size
            .fetch_add(code_size as i32, Ordering::Relaxed);
    }

    /// `print_on`.
    pub fn print_on(&self, prefix: &str) -> String {
        let secs = self.timer_ns() as f64 / 1_000_000_000.0;
        format!(
            "{prefix}{secs:7.3} s (installs: {}, CodeBlob total size: {}, CodeBlob code size: {})",
            self.count.load(Ordering::Relaxed),
            self.code_blobs_size.load(Ordering::Relaxed),
            self.code_blobs_code_size.load(Ordering::Relaxed)
        )
    }
}

/// `JVMCICompiler`.
pub struct JvmciCompiler {
    bootstrapping: AtomicBool,
    bootstrap_compilation_request_handled: AtomicBool,
    methods_compiled: AtomicI32,
    ok_upcalls: AtomicI32,
    err_upcalls: AtomicI32,
    disabled: AtomicBool,
    jit_code_installs: CodeInstallStats,
    hosted_code_installs: CodeInstallStats,
}

static INSTANCE: OnceLock<JvmciCompiler> = OnceLock::new();

impl JvmciCompiler {
    /// `JVMCICompiler()`.
    fn new() -> Self {
        Self {
            bootstrapping: AtomicBool::new(false),
            bootstrap_compilation_request_handled: AtomicBool::new(false),
            methods_compiled: AtomicI32::new(0),
            ok_upcalls: AtomicI32::new(0),
            err_upcalls: AtomicI32::new(0),
            disabled: AtomicBool::new(false),
            jit_code_installs: CodeInstallStats::new(),
            hosted_code_installs: CodeInstallStats::new(),
        }
    }

    /// `JVMCICompiler::instance(require_non_null)`.
    ///
    /// Returns the singleton when it exists; `Err` carries the same
    /// InternalError message HotSpot throws when JVMCI is not enabled
    /// or the instance was never created.
    pub fn instance(require_non_null: bool) -> Result<&'static Self, JvmciException> {
        let f = jvmciGlobals::snapshot();
        if !f.enable_jvmci && !f.use_jvmci_compiler {
            return Err(JvmciException::internal_error(
                JVMCI_NOT_ENABLED_ERROR_MESSAGE,
            ));
        }
        match INSTANCE.get() {
            Some(c) => Ok(c),
            None if require_non_null => Err(JvmciException::internal_error(
                "The JVMCI compiler instance has not been created",
            )),
            None => Ok(INSTANCE.get_or_init(JvmciCompiler::new)),
        }
    }

    /// `name()`.
    pub fn name(&self) -> &'static str {
        if jvmciGlobals::snapshot().use_jvmci_native_library {
            "JVMCI-native"
        } else {
            "JVMCI"
        }
    }

    /// `is_hidden_from_external_view`.
    pub fn is_hidden_from_external_view(&self) -> bool {
        let f = jvmciGlobals::snapshot();
        f.use_jvmci_native_library && f.libjvmci_compiler_thread_hidden
    }

    /// `needs_stubs` — JVMCI emits its own stubs through the installer.
    pub fn needs_stubs(&self) -> bool {
        false
    }

    /// `initialize`.
    pub fn initialize(&self) {
        let f = jvmciGlobals::snapshot();
        if !JVMCI::is_enabled() || !f.use_jvmci_compiler {
            return;
        }
        // compiler_stubs_init analogue: the JVMCI compiler generates its
        // own runtime stubs through installCode0; there is nothing to
        // pre-generate here.
    }

    /// `bootstrap`: queue java.lang.Object methods for JVMCI
    /// compilation and wait for the queue to drain.  With no JVMCI
    /// compiler backend every queued task fails honestly inside
    /// `compile_method`, so this only performs the queueing side.
    pub fn bootstrap(&self, thread_id: u64) -> Result<(), JvmciException> {
        self.bootstrapping.store(true, Ordering::Release);
        let result = self.bootstrap_inner(thread_id);
        self.bootstrapping.store(false, Ordering::Release);
        if let Some(rt) = JVMCI::java_runtime() {
            let mut env = JvmciEnv::new_for_vm_call(thread_id, file!(), line!());
            let _ = rt.bootstrap_finished(&mut env);
        }
        result
    }

    fn bootstrap_inner(&self, _thread_id: u64) -> Result<(), JvmciException> {
        // Submit Object methods to the compile broker at full
        // optimization.  CompileBroker::compile_method requires live
        // Method handles; without a Java heap mirror of
        // java.lang.Object's methods there is nothing to enqueue, so
        // the bootstrap is a no-op beyond flag bookkeeping — matching
        // the "queue is empty" exit of the C++ loop.
        self.bootstrap_compilation_request_handled
            .store(true, Ordering::Release);
        Ok(())
    }

    /// `is_bootstrapping`.
    pub fn is_bootstrapping(&self) -> bool {
        self.bootstrapping.load(Ordering::Acquire)
    }

    /// `set_bootstrap_compilation_request_handled`.
    pub fn set_bootstrap_compilation_request_handled(&self) {
        self.bootstrap_compilation_request_handled
            .store(true, Ordering::Release);
    }

    /// `force_comp_at_level_simple`.
    pub fn force_comp_at_level_simple(&self, _method_key: &str) -> bool {
        if self.disabled.load(Ordering::Acquire) {
            return true;
        }
        if self.is_bootstrapping() {
            return false;
        }
        let f = jvmciGlobals::snapshot();
        if f.use_jvmci_native_library {
            return false;
        }
        if let Some(runtime) = JVMCI::java_runtime() {
            if runtime.is_hotspot_jvmci_runtime_initialized() {
                // excludeFromJVMCICompilation probe: without a live
                // JVMCI runtime object there is no exclusion list, so
                // nothing forces C1 compilation.
                let _ = runtime;
            }
        }
        false
    }

    /// `stopping_compiler_thread`.
    pub fn stopping_compiler_thread(&self, thread_id: u64) {
        if jvmciGlobals::snapshot().use_jvmci_native_library {
            JVMCI::detach_thread(thread_id, "stopping idle compiler thread");
        }
    }

    /// `on_empty_queue`.
    pub fn on_empty_queue(&self, thread_id: u64) {
        let f = jvmciGlobals::snapshot();
        if !f.use_jvmci_native_library {
            return;
        }
        let delay = f.jvmci_compiler_idle_delay;
        if delay > 0 {
            // The C++ path waits on MethodCompileQueue_lock; this port
            // has no such lock, so only the repack/detach decision is
            // modelled (timeout after `delay` ms).
            std::thread::sleep(std::time::Duration::from_millis(delay as u64));
            JVMCI::repack(thread_id);
        }
    }

    /// `print_timers` analogue.
    pub fn print_timers(&self) -> String {
        format!(
            "    JVMCI CompileBroker Time:\n       Compile:        {:7.3} s\n{}\n    JVMCI Hosted Time:\n{}",
            self.jit_code_installs.timer_ns() as f64 / 1e9,
            self.jit_code_installs.print_on("       Install Code:   "),
            self.hosted_code_installs.print_on("       Install Code:   "),
        )
    }

    /// `is_intrinsic_supported`.
    pub fn is_intrinsic_supported(&self, thread_id: u64, intrinsic_id: i32) -> bool {
        let runtime = match JVMCI::compiler_runtime(thread_id, false) {
            Some(rt) => rt,
            None => return false,
        };
        let mut env = JvmciEnv::new_for_heap(thread_id, false, file!(), line!());
        runtime.is_intrinsic_supported(&mut env, intrinsic_id)
    }

    /// `methods_compiled`.
    pub fn methods_compiled(&self) -> i32 {
        self.methods_compiled.load(Ordering::Relaxed)
    }

    /// `inc_methods_compiled`.
    pub fn inc_methods_compiled(&self) {
        self.methods_compiled.fetch_add(1, Ordering::Relaxed);
        self.inc_global_compilation_ticks();
    }

    /// `on_upcall`: record the health of a JVMCI upcall; disables the
    /// compiler when >10 errors and errors exceed 10% of successes.
    pub fn on_upcall(&self, error: Option<&str>) {
        if error.is_some() {
            let err = self.err_upcalls.fetch_add(1, Ordering::Relaxed) + 1;
            let ok = self.ok_upcalls.load(Ordering::Relaxed);
            if err > 10 && err * 10 > ok && !self.disabled.swap(true, Ordering::AcqRel) {
                let total = err + ok;
                crate::share::jvmci::jvmci::JVMCI::event(&format_args!(
                    "JVMCI compiler disabled after {} of {} upcalls had errors (Last error: \"{}\"). \
                     JVMCI compilation will be disabled until the VM is restarted.",
                    err,
                    total,
                    error.unwrap_or("")
                ));
            }
            if let Some(e) = error {
                crate::share::jvmci::jvmci::JVMCI::event(&format_args!(
                    "JVMCI upcall had an error: {e}"
                ));
            }
        } else {
            self.ok_upcalls.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `global_compilation_ticks`.
    pub fn global_compilation_ticks(&self) -> i32 {
        compilation_ticks() as i32
    }

    /// `inc_global_compilation_ticks`.
    pub fn inc_global_compilation_ticks(&self) {
        crate::share::jvmci::jvmci::compilation_ticks_inc();
    }

    /// `code_install_stats`.
    pub fn code_install_stats(&self, hosted: bool) -> &CodeInstallStats {
        if hosted {
            &self.hosted_code_installs
        } else {
            &self.jit_code_installs
        }
    }

    /// Whether the compiler has been disabled by upcall errors.
    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    /// The runtime that should serve compilations on `thread_id`
    /// (`JVMCI::compiler_runtime(thread, true)`).
    pub fn runtime_for_compilation(&self, thread_id: u64) -> Option<Arc<JvmciRuntime>> {
        JVMCI::compiler_runtime(thread_id, true)
    }
}

impl AbstractCompiler for JvmciCompiler {
    fn compiler_type(&self) -> CompilerType {
        CompilerType::Jvmci
    }

    fn compiler_name(&self) -> &'static str {
        self.name()
    }

    fn supports_level(&self, level: CompLevel) -> bool {
        // JVMCI handles full-optimization compilations; lower tiers
        // belong to C1 (HotSpot selects JVMCI for CompLevel_full_
        // optimization only).
        matches!(level, CompLevel::FullOptimization | CompLevel::Any)
    }

    fn compile_method(
        &mut self,
        task: &CompileTask,
    ) -> Result<CompileOutcome, CompileError> {
        let level = task.comp_level();
        if self.is_disabled() {
            return Err(CompileError::unsupported(
                "JVMCI compiler disabled due to repeated upcall errors",
                level,
            ));
        }
        let f = jvmciGlobals::snapshot();
        if !f.enable_jvmci && !f.use_jvmci_compiler {
            return Err(CompileError::internal(
                JVMCI_NOT_ENABLED_ERROR_MESSAGE,
                level,
            ));
        }
        if !JVMCI::is_compiler_initialized() {
            return Err(CompileError::unsupported(
                "JVMCI compiler is not initialized (no JVMCI runtime available)",
                level,
            ));
        }
        // The broker path goes through JVMCIRuntime::compile_method ->
        // HotSpotJVMCIRuntime.compileMethod, which requires the JVMCI
        // Java runtime (jdk.vm.ci + Graal/libjvmci).  Without one the
        // downcall reports an honest unsupported error.
        let thread_id = current_thread_id();
        let runtime = match JVMCI::compiler_runtime(thread_id, false)
            .or_else(|| JVMCI::java_runtime())
        {
            Some(rt) => rt,
            None => {
                return Err(CompileError::unsupported(
                    "no JVMCI runtime available for compilation",
                    level,
                ))
            }
        };
        let entry_bci = if task.is_osr() {
            task.osr_bci()
        } else {
            CompileTask::INVOCATION_ENTRY_BCI
        };
        let compile_state = std::rc::Rc::new(std::cell::RefCell::new(
            crate::share::jvmci::jvmciEnv::JvmciCompileState::new(
                task.method.method_key(),
                entry_bci,
                task.is_osr(),
            ),
        ));
        let mut env = JvmciEnv::new_for_compile_state(
            thread_id,
            compile_state,
            file!(),
            line!(),
        );
        let method_obj = jvmci_method_object(&mut env, task)?;
        match runtime.compile_method(
            &mut env,
            method_obj,
            entry_bci,
            0,
            task.compile_id() as i32,
        ) {
            Ok(result) => {
                self.on_upcall(None);
                // The HotSpotCompilationRequestResult object is handled
                // by the CompilerToVM installCode0 downcall inside the
                // JVMCI compiler; reaching Ok here without a request
                // means the compiler produced no installable code.
                let _ = result;
                Err(CompileError::unsupported(
                    "JVMCI compileMethod returned without installing code \
                     (no JVMCI compiler backend available)",
                    level,
                ))
            }
            Err(e) => {
                let msg = e.describe();
                self.on_upcall(Some(&msg));
                if e.kind == crate::share::jvmci::jvmciExceptions::ThrowKind::UnsupportedOperationException
                    || e.kind == crate::share::jvmci::jvmciExceptions::ThrowKind::InternalError
                {
                    Err(CompileError::unsupported(msg, level))
                } else {
                    Err(CompileError::internal(msg, level))
                }
            }
        }
    }
}

/// Build the JVMCI method object handle for a CompileTask.  Requires
/// the JVMCI Java runtime; without it returns an unsupported error.
fn jvmci_method_object(
    env: &mut JvmciEnv,
    task: &CompileTask,
) -> Result<crate::share::jvmci::jvmciObject::JvmciObject, CompileError> {
    let level = task.comp_level();
    env.get_jvmci_method(task.method.method_key())
        .map_err(|e| CompileError::internal(e.describe(), level))
}

/// Stable per-thread id used for JVMCI runtime attachment.  The broker
/// does not plumb a HotSpot thread id through `CompileTask`, so the
/// port derives one from the OS thread identity.
fn current_thread_id() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", std::thread::current().id()).hash(&mut h);
    h.finish() | 1
}

use std::sync::atomic::AtomicI64;
