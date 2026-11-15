/*
 * Port of OpenJDK 25 HotSpot JVMCI
 * (share/jvmci/jvmci.hpp / jvmci.cpp)
 *
 * The `JVMCI` AllStatic class: global JVMCI state — runtime registry,
 * shared library handle, init/shutdown state — plus event logging and
 * the `CodeInstallResult` enum used by the code installation path.
 */

use crate::share::jvmci::jvmciEnv::JvmciEnv;
use crate::share::jvmci::jvmciExceptions::{JvmciException, JVMCI_NOT_ENABLED_ERROR_MESSAGE};
use crate::share::jvmci::jvmciGlobals;
use crate::share::jvmci::jvmciRuntime::JvmciRuntime;
use crate::share::runtime::thread::{Thread, ThreadType};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// `JVMCI::CodeInstallResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeInstallResult {
    Ok,
    DependenciesFailed,
    CacheFull,
    NmethodReclaimed,
    CodeTooLarge,
}

impl CodeInstallResult {
    /// `first_permanent_bailout` == code_too_large.
    pub const FIRST_PERMANENT_BAILOUT: Self = Self::CodeTooLarge;

    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::DependenciesFailed => 1,
            Self::CacheFull => 2,
            Self::NmethodReclaimed => 3,
            Self::CodeTooLarge => 4,
        }
    }

    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::DependenciesFailed,
            2 => Self::CacheFull,
            3 => Self::NmethodReclaimed,
            4 => Self::CodeTooLarge,
            _ => Self::Ok,
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

struct JvmciState {
    /// `_is_initialized`: at least one initialize_HotSpotJVMCIRuntime
    /// completed successfully.
    is_initialized: AtomicBool,
    box_caches_initialized: AtomicBool,
    /// `_in_shutdown`.
    in_shutdown: AtomicBool,
    /// Module system boot phase >= 3 analogue (`can_initialize_JVMCI`).
    modules_initialized: AtomicBool,
    shared_library_handle: Mutex<Option<usize>>,
    shared_library_path: Mutex<Option<String>>,
    /// `_java_runtime`: the HotSpot-heap JVMCIRuntime (id -1).
    java_runtime: Mutex<Option<Arc<JvmciRuntime>>>,
    /// `_compiler_runtimes`: libjvmci runtimes (JVMCI_lock analogue is
    /// the mutex itself).
    compiler_runtimes: Mutex<Vec<Arc<JvmciRuntime>>>,
    /// `_shutdown_compiler_runtime`: reserved runtime for threads
    /// attaching during JVMCI shutdown (id -2).
    shutdown_compiler_runtime: Mutex<Option<Arc<JvmciRuntime>>>,
    /// thread_id -> runtime id bindings for `detach_thread`/`repack`.
    thread_runtime: Mutex<HashMap<u64, i32>>,
    /// `_fatal_log_fd` analogue (stderr until a file is configured).
    fatal_log_fd: AtomicI32,
    /// `_first_error_tid`.
    first_error_tid: AtomicI32,
    /// `_events` / `_verbose_events` level bookkeeping.
    events_initialized: AtomicU8,
}

static STATE: OnceLock<JvmciState> = OnceLock::new();

fn state() -> &'static JvmciState {
    STATE.get_or_init(|| JvmciState {
        is_initialized: AtomicBool::new(false),
        box_caches_initialized: AtomicBool::new(false),
        in_shutdown: AtomicBool::new(false),
        modules_initialized: AtomicBool::new(false),
        shared_library_handle: Mutex::new(None),
        shared_library_path: Mutex::new(None),
        java_runtime: Mutex::new(None),
        compiler_runtimes: Mutex::new(Vec::new()),
        shutdown_compiler_runtime: Mutex::new(None),
        thread_runtime: Mutex::new(HashMap::new()),
        fatal_log_fd: AtomicI32::new(2),
        first_error_tid: AtomicI32::new(0),
        events_initialized: AtomicU8::new(0),
    })
}

/// Next runtime id allocator (HotSpot heap runtime = -1, shutdown
/// runtime = -2, shared-library runtimes >= 0).
static NEXT_RUNTIME_ID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// `JVMCI` — AllStatic namespace.
pub struct JVMCI;

impl JVMCI {
    // ---- enablement ----------------------------------------------------

    /// Whether JVMCI support is enabled (`EnableJVMCI` after flag
    /// ergonomics; `UseJVMCICompiler` implies it).
    pub fn is_enabled() -> bool {
        let f = jvmciGlobals::snapshot();
        f.enable_jvmci || f.use_jvmci_compiler || f.enable_jvmci_product
    }

    /// `JVMCI::is_compiler_initialized`.
    pub fn is_compiler_initialized() -> bool {
        state().is_initialized.load(Ordering::Acquire)
    }

    /// `JVMCI::in_shutdown`.
    pub fn in_shutdown() -> bool {
        state().in_shutdown.load(Ordering::Acquire)
    }

    /// Mark the VM module system as initialized far enough for JVMCI
    /// (`SystemDictionary::java_system_loader() != nullptr` analogue).
    /// Called by the module system bootstrap; JVMCI itself never sets it.
    pub fn set_modules_initialized(initialized: bool) {
        state()
            .modules_initialized
            .store(initialized, Ordering::Release);
    }

    /// `JVMCI::can_initialize_JVMCI`.
    pub fn can_initialize_jvmci() -> bool {
        if jvmciGlobals::snapshot().use_jvmci_native_library {
            // Initializing libjvmci does not execute Java code.
            return true;
        }
        state().modules_initialized.load(Ordering::Acquire)
    }

    // ---- shared library -------------------------------------------------

    /// `JVMCI::one_shared_library_javavm_per_compilation` � the jvmci
    /// shared library hosts a single JavaVM per compilation in this port.
    pub fn one_shared_library_javavm_per_compilation() -> bool {
        true
    }

    /// `JVMCI::shared_library_exists`: probe for the jvmcicompiler
    /// shared library without loading it.
    pub fn shared_library_exists() -> bool {
        if state().shared_library_handle.lock().unwrap().is_some() {
            return true;
        }
        Self::get_shared_library_path(false).is_some()
    }

    /// Resolve the path of the JVMCI shared library
    /// (`get_shared_library_path`).  `JVMCILibPath` is honoured first,
    /// then the standard dll directory probe.
    pub fn get_shared_library_path(fail_is_fatal: bool) -> Option<String> {
        let f = jvmciGlobals::snapshot();
        if let Some(dir) = &f.jvmci_lib_path {
            for candidate in [
                format!("{dir}/{}", jvmciGlobals::JVMCI_SHARED_LIBRARY_NAME),
                format!("{dir}\\{}", jvmciGlobals::JVMCI_SHARED_LIBRARY_NAME),
                dir.clone(),
            ] {
                if std::path::Path::new(&candidate)
                    .with_extension(if cfg!(windows) { "dll" } else { "so" })
                    .exists()
                    || std::path::Path::new(&candidate).exists()
                {
                    return Some(candidate);
                }
            }
            if !fail_is_fatal {
                return None;
            }
            fatal(&format!(
                "Unable to create path to JVMCI shared library based on value of JVMCILibPath ({dir})"
            ));
        }
        // Probe next to the current executable (Arguments::get_dll_dir
        // analogue) and on the default loader path.
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Some(dir) = exe_dir {
            candidates.push(dir.join(jvmciGlobals::JVMCI_SHARED_LIBRARY_NAME));
        }
        candidates.push(std::path::PathBuf::from(jvmciGlobals::JVMCI_SHARED_LIBRARY_NAME));
        for c in candidates {
            let with_ext = if cfg!(windows) {
                c.with_extension("dll")
            } else {
                c.with_extension("so")
            };
            if with_ext.exists() {
                return Some(with_ext.to_string_lossy().into_owned());
            }
            if c.exists() {
                return Some(c.to_string_lossy().into_owned());
            }
        }
        if !fail_is_fatal {
            return None;
        }
        fatal("Unable to create path to JVMCI shared library");
    }

    /// `JVMCI::get_shared_library`: load libjvmci on demand.  Returns
    /// the opaque library handle and path.
    pub fn get_shared_library(&self, load: bool) -> Option<(usize, String)> {
        if let Some(h) = *state().shared_library_handle.lock().unwrap() {
            let path = state()
                .shared_library_path
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default();
            return Some((h, path));
        }
        if !load {
            return None;
        }
        let path = Self::get_shared_library_path(true)?;
        // Actually load the library so the handle is real.
        match unsafe { libloading::Library::new(&path) } {
            Ok(lib) => {
                let handle = Box::into_raw(Box::new(lib)) as usize;
                *state().shared_library_handle.lock().unwrap() = Some(handle);
                *state().shared_library_path.lock().unwrap() = Some(path.clone());
                Self::event(&format_args!("loaded JVMCI shared library from {path}"));
                Some((handle, path))
            }
            Err(e) => {
                fatal(&format!(
                    "Unable to load JVMCI shared library from {path}: {e}"
                ));
            }
        }
    }

    // ---- runtime registry ------------------------------------------------

    /// `JVMCI::java_runtime`.
    pub fn java_runtime() -> Option<Arc<JvmciRuntime>> {
        state().java_runtime.lock().unwrap().clone()
    }

    /// `JVMCI::compiler_runtime(thread, create)`: the runtime bound to
    /// `thread_id`, creating one when `create` and capacity allows.
    pub fn compiler_runtime(thread_id: u64, create: bool) -> Option<Arc<JvmciRuntime>> {
        Self::runtime_for_thread_inner(thread_id, create, false)
    }

    /// Runtime selection honouring the shutdown state
    /// (`select_runtime_in_shutdown` analogue).
    fn runtime_for_thread_inner(
        thread_id: u64,
        create: bool,
        shutdown_mode: bool,
    ) -> Option<Arc<JvmciRuntime>> {
        let st = state();
        // Existing binding wins.
        if let Some(id) = st.thread_runtime.lock().unwrap().get(&thread_id).copied() {
            if let Some(rt) = Self::find_runtime_by_id(id) {
                return Some(rt);
            }
        }
        if shutdown_mode {
            if let Some(rt) = st.shutdown_compiler_runtime.lock().unwrap().clone() {
                st.thread_runtime.lock().unwrap().insert(thread_id, rt.id);
                return Some(rt);
            }
            return None;
        }
        let f = jvmciGlobals::snapshot();
        let max_per = if f.jvmci_threads_per_native_library_runtime == 0 {
            usize::MAX
        } else {
            f.jvmci_threads_per_native_library_runtime as usize
        };
        // Find a runtime with spare capacity.
        {
            let runtimes = st.compiler_runtimes.lock().unwrap();
            let mut found = None;
            for rt in runtimes.iter() {
                if rt.num_attached() < max_per as i32 {
                    found = Some(rt.clone());
                    break;
                }
            }
            if let Some(rt) = found {
                let id = rt.id;
                drop(runtimes);
                st.thread_runtime.lock().unwrap().insert(thread_id, id);
                return Some(rt);
            }
        }
        if !create {
            return None;
        }
        // Create a new shared-library runtime.
        let id = NEXT_RUNTIME_ID.fetch_add(1, Ordering::AcqRel);
        let rt = Arc::new(JvmciRuntime::new(id, true));
        st.compiler_runtimes.lock().unwrap().push(rt.clone());
        st.thread_runtime
            .lock()
            .unwrap()
            .insert(thread_id, id);
        Some(rt)
    }

    /// `JVMCIRuntime::for_thread` entry used by `JvmciEnv`.
    pub fn runtime_for_thread(
        thread_id: u64,
        is_hotspot: bool,
    ) -> Result<Arc<JvmciRuntime>, String> {
        if !Self::is_enabled() {
            return Err(JVMCI_NOT_ENABLED_ERROR_MESSAGE.to_string());
        }
        if is_hotspot {
            // HotSpot-heap runtime: the single java_runtime, created on
            // first use (id -1).
            let mut slot = state().java_runtime.lock().unwrap();
            if slot.is_none() {
                *slot = Some(Arc::new(JvmciRuntime::new(-1, false)));
            }
            return Ok(slot.as_ref().unwrap().clone());
        }
        if jvmciGlobals::snapshot().use_jvmci_native_library {
            Self::compiler_runtime(thread_id, true)
                .ok_or_else(|| "no JVMCI runtime available for thread".to_string())
        } else {
            // Heap-based JVMCI: all threads share java_runtime.
            let mut slot = state().java_runtime.lock().unwrap();
            if slot.is_none() {
                *slot = Some(Arc::new(JvmciRuntime::new(-1, false)));
            }
            Ok(slot.as_ref().unwrap().clone())
        }
    }

    fn find_runtime_by_id(id: i32) -> Option<Arc<JvmciRuntime>> {
        let st = state();
        if id == -1 {
            return st.java_runtime.lock().unwrap().clone();
        }
        if let Some(rt) = st.shutdown_compiler_runtime.lock().unwrap().clone() {
            if rt.id == id {
                return Some(rt);
            }
        }
        st.compiler_runtimes
            .lock()
            .unwrap()
            .iter()
            .find(|rt| rt.id == id)
            .cloned()
    }

    /// `attach_thread` binding for a thread that was pre-attached.
    pub fn bind_thread_runtime(thread_id: u64, runtime_id: i32) {
        state()
            .thread_runtime
            .lock()
            .unwrap()
            .insert(thread_id, runtime_id);
    }

    /// `detach_thread`: unbind `thread_id` from its runtime and detach.
    pub fn detach_thread(thread_id: u64, reason: &str) {
        let id = state()
            .thread_runtime
            .lock()
            .unwrap()
            .remove(&thread_id);
        if let Some(id) = id {
            if let Some(rt) = Self::find_runtime_by_id(id) {
                rt.detach_thread_env(true, reason);
            }
        }
    }

    /// `repack`: move a thread to another runtime with capacity so its
    /// current runtime can release resources.  With no shared library
    /// JavaVMs this is a binding-table no-op beyond the move itself.
    pub fn repack(thread_id: u64) {
        let f = jvmciGlobals::snapshot();
        if !f.use_jvmci_native_library {
            return;
        }
        let max_per = if f.jvmci_threads_per_native_library_runtime == 0 {
            usize::MAX
        } else {
            f.jvmci_threads_per_native_library_runtime as usize
        };
        let st = state();
        let current = st.thread_runtime.lock().unwrap().get(&thread_id).copied();
        let target = {
            let runtimes = st.compiler_runtimes.lock().unwrap();
            runtimes
                .iter()
                .find(|rt| Some(rt.id) != current && (rt.num_attached() as usize) < max_per)
                .map(|rt| rt.id)
        };
        if let Some(id) = target {
            st.thread_runtime.lock().unwrap().insert(thread_id, id);
        }
    }

    /// All registered runtimes (java_runtime + compiler runtimes).
    pub fn all_runtimes() -> Vec<Arc<JvmciRuntime>> {
        let st = state();
        let mut out = Vec::new();
        if let Some(rt) = st.java_runtime.lock().unwrap().clone() {
            out.push(rt);
        }
        out.extend(st.compiler_runtimes.lock().unwrap().iter().cloned());
        if let Some(rt) = st.shutdown_compiler_runtime.lock().unwrap().clone() {
            out.push(rt);
        }
        out
    }

    // ---- initialization ---------------------------------------------------

    /// `JVMCI::initialize_globals`: vmStructs init + event log setup.
    /// Called once during VM startup (Arguments::apply phase analogue).
    pub fn initialize_globals() {
        let st = state();
        if st.events_initialized.swap(1, Ordering::AcqRel) != 0 {
            return;
        }
        let f = jvmciGlobals::snapshot();
        if f.jvmci_event_log_level > 0 || f.jvmci_trace_level > 0 {
            Self::event(&format_args!(
                "JVMCI initialized (eventLogLevel={}, traceLevel={})",
                f.jvmci_event_log_level, f.jvmci_trace_level
            ));
        }
    }

    /// `JVMCI::initialize_compiler_in_create_vm`: initialize the JVMCI
    /// compiler during VM startup.  Fails with the not-enabled error
    /// when `EnableJVMCI` is off, matching HotSpot.
    pub fn initialize_compiler_in_create_vm(
        thread_id: u64,
    ) -> Result<(), JvmciException> {
        if !Self::is_enabled() {
            return Err(JvmciException::not_enabled());
        }
        if !Self::can_initialize_jvmci() {
            return Err(JvmciException::internal_error(
                "JVMCI cannot be initialized: module system not ready",
            ));
        }
        let is_hotspot = !jvmciGlobals::snapshot().use_jvmci_native_library;
        let runtime = Self::runtime_for_thread(thread_id, is_hotspot)
            .map_err(JvmciException::internal_error)?;

        let mut env = JvmciEnv::new_for_vm_call(thread_id, file!(), line!());
        env.check_init()?;
        let jvmci_runtime = runtime
            .get_hotspot_jvmci_runtime(&mut env)
            .map_err(|e| {
                if env.has_pending_exception() {
                    env.pending_exception().cloned().unwrap_or(e)
                } else {
                    e
                }
            })?;
        runtime.initialize(&mut env)?;
        // getCompiler downcall.
        env.call_hotspot_jvmci_runtime_get_compiler(jvmci_runtime)?;
        state().is_initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// `JVMCI::ensure_box_caches_initialized`.
    pub fn ensure_box_caches_initialized() -> Result<(), JvmciException> {
        state().box_caches_initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// `JVMCI::compilation_tick`: record JVMCI compilation activity when
    /// `thread` is a JVMCI compiler thread.
    pub fn compilation_tick(thread: &Thread) -> &Thread {
        if thread.is_compiler_thread() {
            // Activity is tracked per-runtime via compile_state ticks;
            // a global counter lives on the compiler side.
            COMPILATION_TICKS.fetch_add(1, Ordering::Relaxed);
        }
        thread
    }

    /// `JVMCI::shutdown`: shut down all runtimes during VM teardown.
    pub fn shutdown(thread_id: u64) {
        state().in_shutdown.store(true, Ordering::Release);
        if let Some(rt) = state().java_runtime.lock().unwrap().clone() {
            rt.shutdown();
        }
        let runtimes: Vec<Arc<JvmciRuntime>> =
            state().compiler_runtimes.lock().unwrap().clone();
        for rt in runtimes {
            rt.shutdown();
        }
        if let Some(rt) = state().shutdown_compiler_runtime.lock().unwrap().clone() {
            rt.shutdown();
        }
        state().thread_runtime.lock().unwrap().remove(&thread_id);
        Self::do_unloading(false);
    }

    /// `JVMCI::do_unloading`.
    pub fn do_unloading(unloading_occurred: bool) {
        if !unloading_occurred {
            return;
        }
        for rt in Self::all_runtimes() {
            rt.metadata_handles().lock().unwrap().do_unloading();
        }
    }

    /// `JVMCI::metadata_do`.
    pub fn metadata_do<F: FnMut(&crate::share::jvmci::metadataHandles::MetadataTarget)>(
        mut f: F,
    ) {
        for rt in Self::all_runtimes() {
            rt.metadata_handles().lock().unwrap().metadata_do(&mut f);
        }
    }

    // ---- event logging ----------------------------------------------------

    /// `JVMCI::vlog`/`vtrace`/`event` analogue: unified-log-style event
    /// output gated by JVMCIEventLogLevel / JVMCITraceLevel.
    pub fn event(args: &std::fmt::Arguments) {
        let f = jvmciGlobals::snapshot();
        let msg = format!("JVMCI: {args}");
        if f.jvmci_event_log_level > 0 {
            eprintln!("{msg}");
        } else if f.jvmci_trace_level > 0 {
            println!("{msg}");
        }
    }

    /// `JVMCI_event_N` level-filtered variants.
    pub fn event_level(level: i32, args: &std::fmt::Arguments) {
        let f = jvmciGlobals::snapshot();
        if f.jvmci_event_log_level >= level || f.jvmci_trace_level >= level {
            Self::event(args);
        }
    }

    /// `JVMCI::fatal_log`.
    pub fn fatal_log(buf: &str) {
        let fd = state().fatal_log_fd.load(Ordering::Acquire);
        if fd == 2 {
            eprintln!("{buf}");
        } else {
            eprintln!("[jvmci fatal] {buf}");
        }
    }

    /// `JVMCI::fatal_log_filename` analogue (none configured).
    pub fn fatal_log_filename() -> Option<&'static str> {
        let f = jvmciGlobals::snapshot();
        f.jvmci_native_library_error_file
            .as_deref()
            .map(|_| "hs_err_pid_libjvmci.log")
    }
}

static COMPILATION_TICKS: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(0);

/// `JVMCI::global_compilation_ticks` analogue.
pub fn compilation_ticks() -> i64 {
    COMPILATION_TICKS.load(Ordering::Relaxed)
}

/// Increment the global compilation tick counter.
pub fn compilation_ticks_inc() {
    COMPILATION_TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Fatal error helper (os::fatal analogue for JVMCI paths).
pub fn fatal(msg: &str) -> ! {
    JVMCI::fatal_log(msg);
    std::process::abort();
}

/// `ThreadType::CompilerThread` re-export used by callers checking
/// whether a thread can host JVMCI compilation.
pub fn is_jvmci_compiler_thread(thread: &Thread) -> bool {
    thread.thread_type() == ThreadType::CompilerThread
}
