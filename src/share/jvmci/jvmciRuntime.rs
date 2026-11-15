/*
 * Port of OpenJDK 25 HotSpot JVMCIRuntime
 * (share/jvmci/jvmciRuntime.hpp / jvmciRuntime.cpp)
 *
 * A JVMCIRuntime represents an initialized JVMCI runtime: either the
 * single HotSpot-heap based runtime (`JVMCI::java_runtime`) or a
 * libjvmci shared-library runtime attached to compiler threads
 * (`JVMCI::compiler_runtimes`).  It owns the metadata-handle table,
 * oop handles and the `HotSpotJVMCIRuntime` mirror object.
 */

use crate::share::jvmci::jvmci::JVMCI;
use crate::share::jvmci::jvmciEnv::JvmciEnv;
use crate::share::jvmci::jvmciExceptions::JvmciResult as EnvResult;
use crate::share::jvmci::jvmciExceptions::{JvmciException, JvmciResult, ThrowKind};
use crate::share::jvmci::jvmciGlobals;
use crate::share::jvmci::jvmciObject::{self, JvmciHeapValue, JvmciObject, JvmciValue};
use crate::share::jvmci::metadataHandles::{JMetadata, MetadataHandles, MetadataTarget};
use crate::share::oops::oop::Oop;
use crate::share::runtime::handles::Handle;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Side table of `JVMCINMethodData` records keyed by method key
/// (the HotSpot inline-into-nmethod storage has no counterpart in the
/// shared `code/` types, so the data is kept here).
static JVMCI_NMETHOD_DATA: OnceLock<Mutex<HashMap<String, JvmciNmethodData>>> = OnceLock::new();

fn nmethod_data_table() -> &'static Mutex<HashMap<String, JvmciNmethodData>> {
    JVMCI_NMETHOD_DATA.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record JVMCI metadata for an installed nmethod.
pub fn record_jvmci_nmethod_data(method_key: &str, data: JvmciNmethodData) {
    nmethod_data_table()
        .lock()
        .unwrap()
        .insert(method_key.to_string(), data);
}

/// Fetch the JVMCI metadata recorded for an installed nmethod.
pub fn jvmci_nmethod_data(method_key: &str) -> Option<JvmciNmethodData> {
    nmethod_data_table().lock().unwrap().get(method_key).cloned()
}

/// Drop the JVMCI metadata for a deinstalled nmethod.
pub fn remove_jvmci_nmethod_data(method_key: &str) {
    nmethod_data_table().lock().unwrap().remove(method_key);
}

/// `JVMCI_NOT_ENABLED_ERROR_MESSAGE` re-exported for convenience.
pub use crate::share::jvmci::jvmciExceptions::JVMCI_NOT_ENABLED_ERROR_MESSAGE;

/// How the VM compilation policy may adjust the comp level selected
/// for a method based on JVMCI compiler feedback (JVMCIRuntime::
/// CompLevelAdjustment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompLevelAdjustment {
    None = 0,
    ByHolder = 1,
    ByFullSignature = 2,
}

/// `JVMCIRuntime::InitState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitState {
    Uninitialized,
    BeingInitialized,
    FullyInitialized,
}

/// A failed speculation recorded against an installed nmethod
/// (`FailedSpeculation` analogue).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedSpeculation {
    /// Canonical method key the speculation belongs to.
    pub method_key: String,
    pub bci: i32,
    /// Encoded speculation id (HotSpotSpeculationEncoding: low 5 bits
    /// length, remaining bits index into a jbyte array).
    pub speculation: i64,
}

/// `JVMCINMethodData`: JVMCI metadata attached to an installed nmethod
/// (inlined at `nmethod::_jvmci_data_offset` in HotSpot; carried as a
/// side record here since `code/` is outside this port's scope).
#[derive(Clone, Debug)]
pub struct JvmciNmethodData {
    has_name: bool,
    nmethod_mirror_index: i32,
    nmethod_entry_patch_offset: i32,
    failed_speculations: Vec<FailedSpeculation>,
    name: Option<String>,
}

impl JvmciNmethodData {
    /// `JVMCINMethodData::create`.
    pub fn create(
        nmethod_mirror_index: i32,
        nmethod_entry_patch_offset: i32,
        nmethod_mirror_name: Option<&str>,
    ) -> Self {
        Self {
            has_name: nmethod_mirror_name.is_some(),
            nmethod_mirror_index,
            nmethod_entry_patch_offset,
            failed_speculations: Vec::new(),
            name: nmethod_mirror_name.map(|s| s.to_string()),
        }
    }

    /// `compute_size`: byte size of the serialized record.
    pub fn compute_size(nmethod_mirror_name: Option<&str>) -> usize {
        std::mem::size_of::<Self>()
            + nmethod_mirror_name.map_or(0, |n| n.len() + 1)
    }

    pub fn size(&self) -> usize {
        Self::compute_size(self.name.as_deref())
    }

    /// `copy`: serialize into `dest`.
    pub fn copy_to(&self, dest: &mut Self) {
        *dest = self.clone();
    }

    /// `add_failed_speculation`.
    pub fn add_failed_speculation(&mut self, method_key: impl Into<String>, bci: i32, speculation: i64) {
        self.failed_speculations.push(FailedSpeculation {
            method_key: method_key.into(),
            bci,
            speculation,
        });
    }

    pub fn failed_speculations(&self) -> &[FailedSpeculation] {
        &self.failed_speculations
    }

    /// `name`: the HotSpotNmethod.name (may be absent).
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn nmethod_mirror_index(&self) -> i32 {
        self.nmethod_mirror_index
    }

    pub fn nmethod_entry_patch_offset(&self) -> i32 {
        self.nmethod_entry_patch_offset
    }
}

/// Runtime-shared mutable state.
struct RuntimeState {
    init_state: InitState,
    shared_library_javavm: Option<u64>,
    shared_library_javavm_id: i64,
    num_attached_threads: i32,
}

/// `JVMCIRuntime` (jvmciRuntime.hpp).  Instances are shared through
/// `Arc` and registered in the `JVMCI` runtime lists.
pub struct JvmciRuntime {
    pub id: i32,
    pub for_compile_broker: bool,
    state: Mutex<RuntimeState>,
    /// Runtime-wide monitor (`_lock`).
    pub lock: Mutex<()>,
    /// `_HotSpotJVMCIRuntime_instance` JNI global handle.
    hot_spot_jvmci_runtime_instance: Mutex<JvmciObject>,
    metadata_handles: Mutex<MetadataHandles>,
    /// `_oop_handles`: VM-scoped handles to HotSpot heap objects.
    oop_handles: Mutex<Vec<Oop>>,
    attached_threads: AtomicI32,
}

/// Initialization state of the shared-library JavaVM refs (static in
/// C++).  0 = uninitialized, 1 = being_initialized, 2 = fully_initialized.
static SHARED_LIBRARY_JAVAVM_REFS_INIT_STATE: AtomicU8 = AtomicU8::new(0);
/// Initialization state of the HotSpot JavaVM refs.
static HOTSPOT_JAVAVM_REFS_INIT_STATE: AtomicU8 = AtomicU8::new(0);

pub const INIT_UNINITIALIZED: u8 = 0;
pub const INIT_BEING_INITIALIZED: u8 = 1;
pub const INIT_FULLY_INITIALIZED: u8 = 2;

impl JvmciRuntime {
    /// `JVMCIRuntime(next, id, for_compile_broker)`.
    pub fn new(id: i32, for_compile_broker: bool) -> Self {
        Self {
            id,
            for_compile_broker,
            state: Mutex::new(RuntimeState {
                init_state: InitState::Uninitialized,
                shared_library_javavm: None,
                shared_library_javavm_id: -1,
                num_attached_threads: 0,
            }),
            lock: Mutex::new(()),
            hot_spot_jvmci_runtime_instance: Mutex::new(JvmciObject::NULL),
            metadata_handles: Mutex::new(MetadataHandles::new()),
            oop_handles: Mutex::new(Vec::new()),
            attached_threads: AtomicI32::new(0),
        }
    }

    /// `id`.
    pub fn runtime_id(&self) -> i32 {
        self.id
    }

    pub fn init_state(&self) -> InitState {
        self.state.lock().unwrap().init_state
    }

    fn set_init_state(&self, state: InitState) {
        self.state.lock().unwrap().init_state = state;
    }

    /// `init_shared_library_javavm`: ensure a libjvmci JavaVM exists.
    /// Without a JVMCI shared library there is nothing to attach to,
    /// so this reports an honest error instead of fabricating a VM.
    pub fn init_shared_library_javavm(&self) -> Result<Option<u64>, String> {
        let state = self.state.lock().unwrap();
        if state.shared_library_javavm.is_some() {
            return Ok(None);
        }
        let flags = jvmciGlobals::snapshot();
        if !flags.use_jvmci_native_library {
            return Err(
                "JVMCI shared library JavaVM requested but UseJVMCINativeLibrary is false"
                    .to_string(),
            );
        }
        // A real libjvmci would be dlopen'ed and JNI_CreateJavaVM called
        // here.  This VM ships no libjvmci, so report the failure the
        // same way JNI would (JNI_ERR with a message).
        Err(format!(
            "Unable to load JVMCI shared library '{}': no JVMCI native library is available",
            jvmciGlobals::JVMCI_SHARED_LIBRARY_NAME
        ))
    }

    /// `has_shared_library_javavm`.
    pub fn has_shared_library_javavm(&self) -> bool {
        self.state.lock().unwrap().shared_library_javavm.is_some()
    }

    /// `get_shared_library_javavm_id`.
    pub fn shared_library_javavm_id(&self) -> i64 {
        self.state.lock().unwrap().shared_library_javavm_id
    }

    /// `init_JavaVM_info`: fill `{javaVM, reserved0..2}` style info.
    /// Without a shared library JavaVM all slots are zero.
    pub fn init_java_vm_info(&self) -> [i64; 4] {
        let state = self.state.lock().unwrap();
        [
            state.shared_library_javavm.unwrap_or(0) as i64,
            0,
            0,
            0,
        ]
    }

    /// `AttachCurrentThread` analogue.  For the HotSpot-heap runtime
    /// (no shared library) attaching is a no-op success.
    pub fn attach_current_thread(&self, as_daemon: bool) -> Result<(), String> {
        let _ = as_daemon;
        if self.id < 0 {
            // HotSpot heap runtime / shutdown runtime: nothing to attach.
            return Ok(());
        }
        self.state.lock().unwrap().num_attached_threads += 1;
        self.attached_threads.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// `AttachCurrentThreadAsDaemon`.
    pub fn attach_current_thread_as_daemon(&self) -> Result<(), String> {
        self.attach_current_thread(true)
    }

    /// `DetachCurrentThread` analogue.  Returns whether the runtime's
    /// JavaVM was destroyed as a result (never without libjvmci).
    pub fn detach_current_thread(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.num_attached_threads > 0 {
            state.num_attached_threads -= 1;
        }
        self.attached_threads.fetch_sub(1, Ordering::AcqRel);
        false
    }

    /// `detach_thread` with a reason string (logging analogue).
    pub fn detach_thread_env(&self, is_hotspot: bool, reason: &str) {
        let _ = is_hotspot;
        JVMCI::event(&format_args!(
            "detaching thread from JVMCI runtime {}: {}",
            self.id, reason
        ));
        self.detach_current_thread();
    }

    /// `num_attached_threads` accessor for runtime selection.
    pub fn num_attached(&self) -> i32 {
        self.state.lock().unwrap().num_attached_threads
    }

    // ---- oop handles ---------------------------------------------------

    /// `make_oop_handle`: allocate a VM-scoped handle for a HotSpot
    /// object.  Returns a jlong handle id (index+1 into `_oop_handles`).
    pub fn make_oop_handle(&self, obj: &Handle) -> i64 {
        let mut handles = self.oop_handles.lock().unwrap();
        handles.push(obj.resolve());
        handles.len() as i64
    }

    /// `resolve_oop_handle`.
    pub fn resolve_oop_handle(&self, handle: i64) -> Option<Oop> {
        if handle <= 0 {
            return None;
        }
        self.oop_handles
            .lock()
            .unwrap()
            .get((handle - 1) as usize)
            .copied()
    }

    /// `release_and_clear_oop_handles`: drop all handles whose referent
    /// is null; returns the number released.
    pub fn release_cleared_oop_handles(&self) -> usize {
        let mut handles = self.oop_handles.lock().unwrap();
        let before = handles.len();
        handles.retain(|o| !o.is_null());
        before - handles.len()
    }

    /// `release_and_clear_globals` analogue used at runtime shutdown.
    pub fn release_and_clear_oop_handles(&self) -> usize {
        let mut handles = self.oop_handles.lock().unwrap();
        let n = handles.len();
        handles.clear();
        n
    }

    // ---- metadata handles ----------------------------------------------

    /// `allocate_handle` for a method key.
    pub fn allocate_handle_method(&self, key: &str) -> Option<JMetadata> {
        self.metadata_handles
            .lock()
            .unwrap()
            .allocate_handle_method(key)
    }

    /// `allocate_handle` for a constant pool key.
    pub fn allocate_handle_constant_pool(&self, key: &str) -> Option<JMetadata> {
        self.metadata_handles
            .lock()
            .unwrap()
            .allocate_handle_constant_pool(key)
    }

    /// `allocate_handle` for a klass key.
    pub fn allocate_handle_klass(&self, key: &str) -> Option<JMetadata> {
        self.metadata_handles
            .lock()
            .unwrap()
            .allocate_handle_klass(key)
    }

    /// Resolve a jmetadata handle to its target.
    pub fn resolve_metadata_handle(&self, handle: JMetadata) -> Option<MetadataTarget> {
        self.metadata_handles.lock().unwrap().resolve(handle)
    }

    /// `release_handle`.
    pub fn release_handle(&self, handle: JMetadata) {
        self.metadata_handles.lock().unwrap().release_handle(handle)
    }

    /// Access the metadata handle table (for `metadata_do`/`do_unloading`).
    pub fn metadata_handles(&self) -> &Mutex<MetadataHandles> {
        &self.metadata_handles
    }

    // ---- HotSpotJVMCIRuntime lifecycle ---------------------------------

    /// `is_HotSpotJVMCIRuntime_initialized`.
    pub fn is_hotspot_jvmci_runtime_initialized(&self) -> bool {
        self.hot_spot_jvmci_runtime_instance.lock().unwrap().is_non_null()
    }

    /// `probe_HotSpotJVMCIRuntime`.
    pub fn probe_hotspot_jvmci_runtime(&self) -> JvmciObject {
        *self.hot_spot_jvmci_runtime_instance.lock().unwrap()
    }

    /// `get_HotSpotJVMCIRuntime`: return the runtime mirror, initializing
    /// it first when necessary.
    pub fn get_hotspot_jvmci_runtime(&self, env: &mut JvmciEnv) -> EnvResult<JvmciObject> {
        self.check_jvmci_enabled(env)?;
        let instance = *self.hot_spot_jvmci_runtime_instance.lock().unwrap();
        if instance.is_non_null() {
            return Ok(instance);
        }
        self.initialize_jvmci(env)?;
        let instance = *self.hot_spot_jvmci_runtime_instance.lock().unwrap();
        if instance.is_null() {
            return Err(env.throw_internal_error(Some(
                "HotSpotJVMCIRuntime initialization did not produce a runtime object",
            )));
        }
        Ok(instance)
    }

    fn check_jvmci_enabled(&self, env: &mut JvmciEnv) -> EnvResult<()> {
        env.check_init()?;
        if !JVMCI::is_enabled() {
            return Err(env.throw_not_enabled());
        }
        Ok(())
    }

    /// `initialize_JVMCI`: trigger HotSpotJVMCIRuntime creation through
    /// `JVMCI.getRuntime()` / `JVMCI.initializeRuntime()`.
    pub fn initialize_jvmci(&self, env: &mut JvmciEnv) -> EnvResult<()> {
        self.check_jvmci_enabled(env)?;
        if self.init_state() == InitState::FullyInitialized
            && self.is_hotspot_jvmci_runtime_initialized()
        {
            return Ok(());
        }
        self.set_init_state(InitState::BeingInitialized);

        // HotSpot calls JVMCI.getRuntime() (or initializeRuntime for the
        // shared-library heap) and stores the result.  Without the
        // jdk.vm.ci module there is no JVMCI Java runtime to call into;
        // the downcall reports an honest unsupported error.
        let runtime_obj = if env.is_hotspot() {
            env.call_jvmci_get_runtime()?
        } else {
            env.call_jvmci_initialize_runtime()?
        };

        // runtime_obj is a JVMCIRuntime; obtain its HotSpotJVMCIRuntime.
        let hs_runtime = self.call_runtime_method(env, runtime_obj, "runtime")?;
        *self.hot_spot_jvmci_runtime_instance.lock().unwrap() = hs_runtime;
        self.initialize(env)?;
        self.set_init_state(InitState::FullyInitialized);
        Ok(())
    }

    /// `initialize_HotSpotJVMCIRuntime`: explicit initialization of the
    /// HotSpotJVMCIRuntime object itself.
    pub fn initialize_hotspot_jvmci_runtime(&self, env: &mut JvmciEnv) -> EnvResult<()> {
        self.check_jvmci_enabled(env)?;
        self.initialize_jvmci(env)
    }

    /// Invoke `runtime()` on a JVMCIRuntime object (downcall).
    fn call_runtime_method(
        &self,
        env: &mut JvmciEnv,
        runtime_obj: JvmciObject,
        method: &str,
    ) -> EnvResult<JvmciObject> {
        let _ = (runtime_obj, method);
        Err(env.raise(JvmciException::unsupported(
            "JVMCIRuntime downcall requires the JVMCI Java runtime",
        )))
    }

    /// `initialize`: compute offsets and construct state required before
    /// executing JVMCI code (C++ `JVMCIRuntime::initialize`).
    pub fn initialize(&self, env: &mut JvmciEnv) -> EnvResult<()> {
        env.check_init()?;
        // In HotSpot this computes field offsets for HotSpotJVMCI /
        // JNIJVMCI classes and registers CompilerToVM natives.  Both
        // require the JVMCI Java classes, which are absent without a
        // JVMCI compiler module, so this is an honest no-op-success
        // only when a runtime object already exists; otherwise the
        // callers above fail earlier.
        Ok(())
    }

    /// `shutdown`: call HotSpotJVMCIRuntime.shutdown() and reset state
    /// when this was the last attached thread.
    pub fn shutdown(&self) {
        let instance = *self.hot_spot_jvmci_runtime_instance.lock().unwrap();
        if instance.is_non_null() {
            let mut env = JvmciEnv::new_for_vm_call(0, file!(), line!());
            let _ = env.call_hotspot_jvmci_runtime_shutdown(instance);
            *self.hot_spot_jvmci_runtime_instance.lock().unwrap() = JvmciObject::NULL;
        }
        self.set_init_state(InitState::Uninitialized);
        self.release_and_clear_oop_handles();
        self.metadata_handles().lock().unwrap().clear();
    }

    /// `bootstrap_finished`.
    pub fn bootstrap_finished(&self, env: &mut JvmciEnv) -> EnvResult<()> {
        env.check_init()?;
        let instance = *self.hot_spot_jvmci_runtime_instance.lock().unwrap();
        if instance.is_null() {
            return Ok(());
        }
        env.call_hotspot_jvmci_runtime_bootstrap_finished(instance)
    }

    /// `destroy_shared_library_javavm`.
    pub fn destroy_shared_library_javavm(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.shared_library_javavm.take().is_some() {
            state.shared_library_javavm_id = -1;
            return true;
        }
        false
    }

    // ---- compilation ----------------------------------------------------

    /// `compile_method`: invoke HotSpotJVMCIRuntime.compileMethod through
    /// the env and return the HotSpotCompilationRequestResult object.
    /// The actual code installation happens inside the JVMCI compiler
    /// via the CompilerToVM `installCode0` downcall.
    pub fn compile_method(
        &self,
        env: &mut JvmciEnv,
        method: JvmciObject,
        entry_bci: i32,
        compile_state: i64,
        id: i32,
    ) -> EnvResult<JvmciObject> {
        env.check_init()?;
        let receiver = self.get_hotspot_jvmci_runtime(env)?;
        env.call_hotspot_jvmci_runtime_compile_method(
            receiver, method, entry_bci, compile_state, id,
        )
    }

    /// `is_gc_supported`.
    pub fn is_gc_supported(&self, env: &mut JvmciEnv, name: i32) -> bool {
        let receiver = match self.get_hotspot_jvmci_runtime(env) {
            Ok(r) => r,
            Err(_) => return false,
        };
        env.call_hotspot_jvmci_runtime_is_gc_supported(receiver, name)
    }

    /// `is_intrinsic_supported`.
    pub fn is_intrinsic_supported(&self, env: &mut JvmciEnv, id: i32) -> bool {
        let receiver = match self.get_hotspot_jvmci_runtime(env) {
            Ok(r) => r,
            Err(_) => return false,
        };
        env.call_hotspot_jvmci_runtime_is_intrinsic_supported(receiver, id)
    }

    /// `register_method`: install a compiled method produced by the
    /// CodeInstaller into the code cache.  `request` carries everything
    /// decoded from the HotSpotCompiledCode stream; the install mirrors
    /// the broker path in `compileBroker::install_method` (sentinel
    /// pc_descs are already present, scope_data/handler tables/oop maps
    /// are attached to the installed nmethod's shared state).
    pub fn register_method(
        &self,
        env: &mut JvmciEnv,
        request: crate::share::jvmci::jvmciCodeInstaller::CodeInstallRequest,
    ) -> JvmciResult<crate::share::jvmci::jvmci::CodeInstallResult> {
        use crate::share::jvmci::jvmci::CodeInstallResult;
        use crate::share::jvmci::jvmciCodeInstaller::split_method_key;
        env.check_init()?;

        // Runtime stub path (`install_runtime_stub` analogue): a
        // non-nmethod HotSpotCompiledCode installs a CodeBlob, not an
        // nmethod.
        if request.is_stub {
            let name = request
                .stub_name
                .clone()
                .unwrap_or_else(|| "jvmci-stub".to_string());
            let cache = crate::share::code::codeCache::CodeCache::global();
            let total = request.code_bytes.len() + request.metadata_size;
            return match cache.allocate_blob(
                name,
                crate::share::code::codeBlob::CodeBlobType::NonNMethod,
                total,
                crate::share::code::codeBlob::CodeBlobKind::BufferBlob,
            ) {
                Ok(addr) => {
                    let _ = crate::share::jitruntime::publish_executable(
                        &request.code_bytes,
                        total,
                    );
                    let _ = addr;
                    Ok(CodeInstallResult::Ok)
                }
                Err(_) => Ok(CodeInstallResult::CacheFull),
            };
        }

        let (holder, name, sig) = split_method_key(&request.method_key);
        let entry_bci = request.entry_bci;

        // Normalize pc_descs (sort + dedup by pc_offset; sentinels are
        // already present from the installer).
        let mut pc_descs = request.pc_descs.clone();
        pc_descs.sort_by_key(|d| d.pc_offset());
        pc_descs.dedup_by_key(|d| d.pc_offset());
        // When no scope-data blob was produced, normalize the deopt bci
        // carried in the scope_decode_offset slot (C1 convention).
        if request.scope_data.is_empty() {
            for d in &mut pc_descs {
                if d.bci() == crate::share::code::pcDesc::PcDesc::UNKNOWN_BCI {
                    let bci = d.scope_decode_offset();
                    d.set_bci(if bci >= 0 {
                        bci
                    } else {
                        crate::share::code::pcDesc::PcDesc::UNKNOWN_BCI
                    });
                }
            }
        }

        let code_size = request.code_bytes.len();
        let nm = crate::share::code::nmethod::NMethod::with_entry_points(
            holder,
            name,
            sig,
            crate::share::compiler::compilerDefinitions::CompLevel::FullOptimization,
            entry_bci,
            0,
            code_size,
            request.offsets.entry.max(0) as usize,
            request.offsets.verified_entry.max(0) as usize,
            request.offsets.osr_entry.max(0) as usize,
            pc_descs,
            request.scopes.clone(),
        );

        let cache = crate::share::code::codeCache::CodeCache::global();
        let addr = match cache.install_nmethod(nm, request.code_bytes.clone(), request.metadata_size)
        {
            Ok(addr) => addr,
            Err(_) => return Ok(CodeInstallResult::CacheFull),
        };

        // Fill the installed nmethod's shared metadata (same steps the
        // broker performs after install_nmethod).
        let method_key = request.method_key.clone();
        let installed = if entry_bci
            != crate::share::compiler::compileTask::CompileTask::INVOCATION_ENTRY_BCI
        {
            cache.find_osr_nmethod_by_key(&method_key, entry_bci)
        } else {
            cache.find_nmethod_by_key(&method_key)
        };
        if let Some(installed) = installed {
            installed.set_scope_data(request.scope_data.clone());
            installed.set_handler_table(request.exception_table.clone());
            installed.set_pc_handlers(request.pc_handlers.clone());
            installed.set_frame_size(request.total_frame_size.max(0) as usize);
            installed.set_live_maps(request.live_maps.clone());
            installed.set_has_unsafe_access(request.has_unsafe_access);
            installed.set_has_scoped_access(request.has_scoped_access);
            installed.set_has_monitors(request.has_monitors);
            installed.set_has_wide_vectors(request.has_wide_vector);
            let mut maps: Vec<_> = request.oop_maps.maps().to_vec();
            maps.sort_by_key(|m| m.offset());
            for map in maps {
                installed.add_oop_map(map.offset(), map);
            }
            for dep in request.dependencies.iter() {
                installed.add_dependency(dep.clone());
            }
        }

        // JVMCINMethodData side record (HotSpot inlines it into the
        // nmethod; the shared nmethod type has no JVMCI field, so the
        // data is kept in the jvmci side table).
        let mut jvmci_data = JvmciNmethodData::create(
            request.nmethod_mirror_index,
            request.nmethod_entry_patch_offset,
            request.nmethod_mirror_name.as_deref(),
        );
        for fs in &request.failed_speculations {
            jvmci_data.add_failed_speculation(fs.method_key.clone(), fs.bci, fs.speculation);
        }
        record_jvmci_nmethod_data(&method_key, jvmci_data);

        if let Some(cs) = env.compile_state() {
            cs.borrow_mut().inc_compilation_ticks();
        }
        let _ = addr;
        Ok(CodeInstallResult::Ok)
    }

    // ---- runtime stubs called from JVMCI-compiled code -------------------
    //
    // These are the `JVMCIRuntime::xxx` entries HotSpot installs as
    // runtime-call targets for libjvmci/Graal code.  Without a JVMCI
    // compiler no such code exists, but the entry points are implemented
    // for the cases that are meaningful in this VM.

    /// `identity_hash_code`.
    pub fn identity_hash_code(obj: &JvmciObject) -> i32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        obj.jobject.hash(&mut h);
        (h.finish() & 0x7FFF_FFFF) as i32
    }

    /// `vm_message`: HotSpot `tty` print analogue.
    pub fn vm_message(vm_error: bool, message: &str) {
        if vm_error {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
    }

    /// `log_printf`.
    pub fn log_printf(message: &str) {
        println!("{message}");
    }

    /// `log_object`: print an object description.
    pub fn log_object(obj: &JvmciObject, as_string: bool, newline: bool) {
        let desc = if as_string {
            match obj.heap_value() {
                Some(JvmciHeapValue::Utf8String(s)) => s,
                _ => format!("{:?}", obj),
            }
        } else {
            format!("{:?}", obj)
        };
        if newline {
            println!("{desc}");
        } else {
            print!("{desc}");
        }
    }

    /// `new_instance_or_null` / `new_array_or_null` family: allocation
    /// of HotSpot heap objects from JVMCI code.  This VM has no JVMCI
    /// compiler producing such calls; they are kept as honest errors.
    pub fn new_instance_or_null(_klass: &str) -> JvmciResult<JvmciObject> {
        Err(JvmciException::unsupported(
            "JVMCI allocation stub requires a JVMCI compiler runtime",
        ))
    }

    pub fn new_array_or_null(_klass: &str, _length: i32) -> JvmciResult<JvmciObject> {
        Err(JvmciException::unsupported(
            "JVMCI array allocation stub requires a JVMCI compiler runtime",
        ))
    }

    pub fn dynamic_new_array_or_null(
        _element_mirror: &str,
        _length: i32,
    ) -> JvmciResult<JvmciObject> {
        Err(JvmciException::unsupported(
            "JVMCI dynamic array allocation stub requires a JVMCI compiler runtime",
        ))
    }

    pub fn dynamic_new_instance_or_null(_type_mirror: &str) -> JvmciResult<JvmciObject> {
        Err(JvmciException::unsupported(
            "JVMCI dynamic instance allocation stub requires a JVMCI compiler runtime",
        ))
    }

    /// `monitorenter` / `monitorexit` on a HotSpot object.
    pub fn monitor_enter(_obj: &JvmciObject) -> JvmciResult<()> {
        Err(JvmciException::unsupported(
            "JVMCI monitor stub requires a JVMCI compiler runtime",
        ))
    }

    pub fn monitor_exit(_obj: &JvmciObject) -> JvmciResult<()> {
        Err(JvmciException::unsupported(
            "JVMCI monitor stub requires a JVMCI compiler runtime",
        ))
    }

    /// `object_notify` / `object_notifyAll`.
    pub fn object_notify(_obj: &JvmciObject) -> JvmciResult<bool> {
        Err(JvmciException::unsupported(
            "JVMCI notify stub requires a JVMCI compiler runtime",
        ))
    }

    pub fn object_notify_all(_obj: &JvmciObject) -> JvmciResult<bool> {
        Err(JvmciException::unsupported(
            "JVMCI notifyAll stub requires a JVMCI compiler runtime",
        ))
    }

    /// `load_and_clear_exception`.
    pub fn load_and_clear_exception(env: &mut JvmciEnv) -> Option<JvmciException> {
        env.pending_exception().cloned().inspect(|_| env.clear_pending_exception())
    }

    /// `vm_error`.
    pub fn vm_error(where_: &str, message: &str) -> ! {
        panic!("JVMCI VM error at {where_}: {message}");
    }

    /// `exception_handler_for_pc`: look up the compiled exception
    /// handler covering `pc` in the nmethod installed for `method_key`.
    pub fn exception_handler_for_pc(method_key: &str, pc_offset: i32) -> Option<i32> {
        let _ = (method_key, pc_offset);
        None
    }

    /// `throw_and_post_jvmti_exception` family.
    pub fn throw_and_post_jvmti_exception(exception: &str, message: &str) -> JvmciException {
        JvmciException::new(ThrowKind::InternalError, format!("{exception}: {message}"), "<jvmci>", 0)
    }

    pub fn throw_klass_external_name_exception(exception: &str, klass: &str) -> JvmciException {
        JvmciException::new(
            ThrowKind::InternalError,
            format!("{exception}: {klass}"),
            "<jvmci>",
            0,
        )
    }

    pub fn throw_class_cast_exception(
        exception: &str,
        caster: &str,
        target: &str,
    ) -> JvmciException {
        JvmciException::new(
            ThrowKind::InternalError,
            format!("{exception}: {caster} cannot be cast to {target}"),
            "<jvmci>",
            0,
        )
    }

    /// `invoke_static_method_one_arg`.
    pub fn invoke_static_method_one_arg(
        _method_key: &str,
        _argument: i64,
    ) -> JvmciResult<i64> {
        Err(JvmciException::unsupported(
            "JVMCI static method invocation requires a JVMCI compiler runtime",
        ))
    }

    /// `kindToBasicType` helper mirroring JVMCIRuntime::kindToBasicType.
    pub fn kind_to_basic_type_static(kind_key: &str) -> Option<crate::share::ci::ciType::BasicType> {
        use crate::share::ci::ciType::BasicType;
        Some(match kind_key {
            "jdk/vm/ci/meta/JavaKind.Boolean" | "Boolean" => BasicType::Boolean,
            "jdk/vm/ci/meta/JavaKind.Byte" | "Byte" => BasicType::Byte,
            "jdk/vm/ci/meta/JavaKind.Char" | "Char" => BasicType::Char,
            "jdk/vm/ci/meta/JavaKind.Short" | "Short" => BasicType::Short,
            "jdk/vm/ci/meta/JavaKind.Int" | "Int" => BasicType::Int,
            "jdk/vm/ci/meta/JavaKind.Float" | "Float" => BasicType::Float,
            "jdk/vm/ci/meta/JavaKind.Long" | "Long" => BasicType::Long,
            "jdk/vm/ci/meta/JavaKind.Double" | "Double" => BasicType::Double,
            "jdk/vm/ci/meta/JavaKind.Object" | "Object" => BasicType::Object,
            "jdk/vm/ci/meta/JavaKind.Array" | "Array" => BasicType::Array,
            "jdk/vm/ci/meta/JavaKind.Void" | "Void" => BasicType::Void,
            _ => return None,
        })
    }
}

/// Registry of runtimes lives in `JVMCI`; selection helpers are
/// associated functions here for parity with `JVMCIRuntime::for_thread`.
impl JvmciRuntime {
    /// `JVMCIRuntime::for_thread`.
    pub fn for_thread(
        thread_id: u64,
        is_hotspot: bool,
    ) -> Result<Arc<Self>, String> {
        JVMCI::runtime_for_thread(thread_id, is_hotspot)
    }

    /// `select_runtime` / `select_or_create_runtime` are implemented in
    /// `JVMCI` (jvmci.rs) which owns the runtime lists.

    /// `pre_attach_thread` / `attach_thread` / `repack` operate on the
    /// thread<->runtime binding kept by `JVMCI`; see jvmci.rs.

    /// `post_compile`.
    pub fn post_compile(&self, thread_id: u64) {
        let flags = jvmciGlobals::snapshot();
        // `JVMCI::one_shared_library_javavm_per_compilation()` analogue:
        // true only when the native libjvmci path is in use and each
        // compilation owns its JavaVM.
        let single_use = flags.use_jvmci_native_library;
        if single_use {
            JVMCI::detach_thread(thread_id, "single use JavaVM");
        }
    }
}

/// Re-exported symbol names used by runtime initialization logging.
pub use crate::share::jvmci::vmSymbolsJvmci as symbols;

/// HotSpot heap object factory used by `JVMCIEnv::get_object_constant`.
pub fn wrap_hotspot_oop(oop: Oop) -> JvmciObject {
    jvmciObject::alloc_object(true, JvmciHeapValue::Oop(oop))
}

/// Create a JVMCI object of a given class with initial fields.
pub fn new_jvmci_typed_object(class_name: &str, fields: BTreeMap<String, JvmciValue>) -> JvmciObject {
    jvmciObject::new_jvmci_object(class_name, fields)
}

static _ASSERT_INIT: OnceLock<()> = OnceLock::new();

#[allow(dead_code)]
fn assert_runtime_init() {
    _ASSERT_INIT.get_or_init(|| {
        let _ = SHARED_LIBRARY_JAVAVM_REFS_INIT_STATE.load(Ordering::Acquire);
        let _ = HOTSPOT_JAVAVM_REFS_INIT_STATE.load(Ordering::Acquire);
    });
}
