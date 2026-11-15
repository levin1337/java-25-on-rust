/*
 * Port of OpenJDK 25 HotSpot CompileBroker (share/compiler/compileBroker.hpp/.cpp)
 *
 * The broker owns the compile queues (C1 and C2), assigns compile ids,
 * deduplicates requests, executes blocking compiles synchronously,
 * dispatches tasks to registered compiler backends on compiler threads,
 * installs resulting nmethods into the CodeCache and tracks statistics.
 *
 * A process-wide singleton is available via `CompileBroker::global()`.
 */

use super::abstractCompiler::{AbstractCompiler, CompileError};
use super::compileTask::{CompileOutcome, CompileReason, CompileTask, FailureReason, MethodIdentity};
use super::compilationPolicy::{CompilationPolicy, MethodProfile, PolicyActions};
use super::compilerDefinitions::CompLevel;
use super::compilerOracle::CompilerOracle;
use crate::share::c1::c1_GraphBuilder::MethodMetadata;
use crate::share::code::codeCache::CodeCache;
use crate::share::code::nmethod::NMethod;
use crate::share::oops::constMethod::ConstMethod;
use crate::share::oops::method::{Code, Method};
use crate::share::oops::methodCounters;
use crate::share::oops::symbol::Symbol;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::Duration;

/// Per-method compilability flags consulted by `compilation_is_prohibited`
/// (`method->is_native()`, `is_not_compilable(level)` etc.).
#[derive(Clone, Copy, Debug, Default)]
pub struct MethodCompileFlags {
    pub is_native: bool,
    pub is_abstract: bool,
    pub is_static_initializer: bool,
    /// Bitmask of levels the method must NOT be compiled at.
    /// Bit i corresponds to CompLevel i-1 (bit0 = Any/None region unused;
    /// levels 1..=4 map to bits 2..=5, level 0 to bit 1).
    pub not_compilable_mask: u8,
    pub not_osr_compilable_mask: u8,
    /// Accessor/constant-getter hint for the policy's is_trivial().
    pub is_trivial: bool,
}

impl MethodCompileFlags {
    fn mask_bit(level: CompLevel) -> u8 {
        // levels -1..4 -> bits 0..5
        1u8 << (level.as_i32() + 1).clamp(0, 7) as u32
    }

    pub fn set_not_compilable(&mut self, level: CompLevel) {
        self.not_compilable_mask |= Self::mask_bit(level);
    }

    pub fn set_not_osr_compilable(&mut self, level: CompLevel) {
        self.not_osr_compilable_mask |= Self::mask_bit(level);
    }

    pub fn is_not_compilable(&self, level: CompLevel) -> bool {
        self.not_compilable_mask & Self::mask_bit(level) != 0
    }

    pub fn is_not_osr_compilable(&self, level: CompLevel) -> bool {
        self.not_osr_compilable_mask & Self::mask_bit(level) != 0
    }
}

/// Resolves a `MethodIdentity` to a live `Method`/`ConstMethod` using the
/// JVM-wide `ClassLoader`.  The interpreter executes `Code` parsed from
/// `Member`s; the JIT task model expects HotSpot-style `Method` metadata.
/// This bridge rebuilds `ConstMethod` (bytecodes, max_stack/locals,
/// exception table) and `Method` (access flags) from the same `Member` so
/// C1/C2 can compile exactly the code the interpreter runs.  Returns `None`
/// when the class or method cannot be resolved (native/abstract/missing).
fn resolve_method_obj(identity: &MethodIdentity) -> Option<Arc<Method>> {
    let loader = crate::share::runtime::threading::jvm_loader()?;
    let klass = loader
        .classes
        .borrow()
        .get(identity.class_name.as_bytes())
        .cloned()?;
    let member = klass.class.methods.iter().find(|m| {
        m.name == identity.method_name.as_bytes()
            && m.descriptor == identity.descriptor.as_bytes()
    })?;
    // Skip members with no Code attribute (native/abstract) — nothing to JIT.
    if member.code_attribute().is_none() {
        return None;
    }
    let code = Code::parse(member).ok()?;
    let const_method = ConstMethod::new(
        0,
        0,
        code.maxStack.min(u16::MAX as usize) as u16,
        code.maxLocals.min(u16::MAX as usize) as u16,
        0,
        code.bytes.to_vec(),
        code.exception_table,
        Vec::new(),
        Vec::new(),
    );
    Some(Arc::new(Method::new(
        Arc::new(const_method),
        member.accessFlags,
    )))
}

/// Builds `MethodMetadata` for JIT graph building: resolves every
/// `ldc`/`ldc2_w` constant-pool index used by the method's bytecode to
/// its numeric HIR constant.  Reference constants are skipped — they
/// cannot appear in the primitive executable subset anyway, and their
/// `ldc` sites degrade to `Unsupported` during graph building.
fn resolve_jit_metadata(identity: &MethodIdentity, bytecode: &[u8]) -> Option<MethodMetadata> {
    let loader = crate::share::runtime::threading::jvm_loader()?;
    let klass = loader
        .classes
        .borrow()
        .get(identity.class_name.as_bytes())
        .cloned()?;
    let mut metadata = MethodMetadata::default();
    for idx in crate::share::c1::c1_GraphBuilder::ldc_cp_indices(bytecode) {
        let Ok(c) = klass.constant(idx as u16) else { continue };
        if let Some(hir) = crate::share::c1::c1_GraphBuilder::constant_to_hir(c) {
            metadata.ldc_constants.insert(idx, hir);
        }
    }
    Some(metadata)
}

/// A compile queue: FIFO of CompileTasks plus statistics
/// (`CompileQueue` analogue).  Selection is rate-based and performed by
/// `CompilationPolicy::select_task`; this structure only stores tasks.
#[derive(Debug)]
pub struct CompileQueue {
    name: &'static str,
    tasks: Mutex<VecDeque<CompileTask>>,
    new_task: Condvar,
    peak_size: AtomicUsize,
    total_added: AtomicU64,
    total_removed: AtomicU64,
    /// Tasks removed while stale, awaiting reclamation.
    stale: Mutex<Vec<CompileTask>>,
}

impl CompileQueue {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            tasks: Mutex::new(VecDeque::new()),
            new_task: Condvar::new(),
            peak_size: AtomicUsize::new(0),
            total_added: AtomicU64::new(0),
            total_removed: AtomicU64::new(0),
            stale: Mutex::new(Vec::new()),
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// `CompileQueue::add`.
    pub fn add(&self, task: CompileTask) {
        {
            let mut q = self.tasks.lock().unwrap();
            q.push_back(task);
            let size = q.len();
            self.peak_size.fetch_max(size, Ordering::Relaxed);
        }
        self.total_added.fetch_add(1, Ordering::Relaxed);
        self.new_task.notify_one();
    }

    /// `CompileQueue::remove` by compile_id.
    pub fn remove(&self, compile_id: usize) -> Option<CompileTask> {
        let mut q = self.tasks.lock().unwrap();
        if let Some(pos) = q.iter().position(|t| t.compile_id() == compile_id) {
            let task = q.remove(pos).unwrap();
            self.total_removed.fetch_add(1, Ordering::Relaxed);
            Some(task)
        } else {
            None
        }
    }

    /// `CompileQueue::remove_and_mark_stale`.
    pub fn remove_and_mark_stale(&self, compile_id: usize) {
        if let Some(mut task) = self.remove(compile_id) {
            task.mark_stale();
            task.set_failure_reason("stale task");
            self.stale.lock().unwrap().push(task);
        }
    }

    /// Purge stale tasks (CompileQueue::purge_stale_tasks): complete them
    /// with a failure reason so waiters are released.
    pub fn purge_stale_tasks(&self) {
        let drained: Vec<CompileTask> = {
            let mut s = self.stale.lock().unwrap();
            s.drain(..).collect()
        };
        for task in drained {
            if !task.is_complete() {
                task.set_failure_reason("stale task");
            }
        }
    }

    pub fn first(&self) -> Option<CompileTask> {
        self.tasks.lock().unwrap().front().cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.lock().unwrap().is_empty()
    }

    pub fn size(&self) -> usize {
        self.tasks.lock().unwrap().len()
    }

    pub fn get_peak_size(&self) -> usize {
        self.peak_size.load(Ordering::Relaxed)
    }

    pub fn total_added(&self) -> u64 {
        self.total_added.load(Ordering::Relaxed)
    }

    pub fn total_removed(&self) -> u64 {
        self.total_removed.load(Ordering::Relaxed)
    }

    /// Snapshot of queued tasks in FIFO order.
    pub fn snapshot(&self) -> Vec<CompileTask> {
        self.tasks.lock().unwrap().iter().cloned().collect()
    }

    /// Blocking pop used by compiler threads: waits up to `timeout` for a
    /// task, then asks the policy to select the best candidate.
    pub fn get_for_compiler(
        &self,
        policy: &CompilationPolicy,
        disabled_forever: &AtomicBool,
        timeout: Duration,
    ) -> Option<CompileTask> {
        let mut q = self.tasks.lock().unwrap();
        loop {
            if disabled_forever.load(Ordering::Acquire) {
                return None;
            }
            if !q.is_empty() {
                break;
            }
            let (guard, _) = self
                .new_task
                .wait_timeout(q, timeout)
                .unwrap_or_else(|e| e.into_inner());
            q = guard;
            if disabled_forever.load(Ordering::Acquire) {
                return None;
            }
        }
        drop(q);
        // Select outside the queue lock.
        if let Some(task) = policy.select_task(self) {
            self.remove(task.compile_id());
            self.purge_stale_tasks();
            return Some(task);
        }
        None
    }

    /// `CompileQueue::mark_on_stack`: no-op for Rust threads; retained for
    /// API parity (compiled frames are tracked via CodeCache sweeps).
    pub fn mark_on_stack(&self) {}

    /// `CompileQueue::free_all`: fail every queued task.
    pub fn free_all(&self) {
        let tasks: Vec<CompileTask> = {
            let mut q = self.tasks.lock().unwrap();
            q.drain(..).collect()
        };
        for task in tasks {
            task.set_failure_reason("compile queue freed");
        }
        self.purge_stale_tasks();
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let tasks = self.snapshot();
        let _ = writeln!(f, "{} compile queue: {} tasks", self.name, tasks.len());
        for t in tasks.iter().take(16) {
            t.print_on(f, None, true);
        }
        if tasks.len() > 16 {
            let _ = writeln!(f, "  ... {} more", tasks.len() - 16);
        }
    }
}

struct BrokerInner {
    compilation_id: usize,
    osr_compilation_id: usize,
    native_compilation_id: usize,
    compilation_disabled: bool,
    compilation_disabled_forever: bool,
    should_block: AtomicBool,
    should_compile_new_jobs: bool,
    total_compile_count: u64,
    total_bailout_count: u64,
    total_invalidated_count: u64,
    total_native_compile_count: u64,
    total_osr_compile_count: u64,
    total_standard_compile_count: u64,
    total_compiler_stopped_count: u64,
    total_compiler_restarted_count: u64,
    sum_osr_bytes_compiled: u64,
    sum_standard_bytes_compiled: u64,
    /// method_key (or method_key@osr_bci) currently queued or compiling.
    queued_methods: HashSet<String>,
    /// method_key -> highest completed regular level.
    completed_levels: HashMap<String, CompLevel>,
    /// method_key -> osr_bci -> completed osr level.
    completed_osr: HashMap<String, HashMap<i32, CompLevel>>,
    /// Per-method compilability flags.
    method_flags: HashMap<String, MethodCompileFlags>,
}

impl BrokerInner {
    fn new() -> Self {
        Self {
            compilation_id: 0,
            osr_compilation_id: 0,
            native_compilation_id: 0,
            compilation_disabled: false,
            compilation_disabled_forever: false,
            should_block: AtomicBool::new(false),
            should_compile_new_jobs: true,
            total_compile_count: 0,
            total_bailout_count: 0,
            total_invalidated_count: 0,
            total_native_compile_count: 0,
            total_osr_compile_count: 0,
            total_standard_compile_count: 0,
            total_compiler_stopped_count: 0,
            total_compiler_restarted_count: 0,
            sum_osr_bytes_compiled: 0,
            sum_standard_bytes_compiled: 0,
            queued_methods: HashSet::new(),
            completed_levels: HashMap::new(),
            completed_osr: HashMap::new(),
            method_flags: HashMap::new(),
        }
    }
}

pub struct CompileBroker {
    c1_queue: CompileQueue,
    c2_queue: CompileQueue,
    inner: Mutex<BrokerInner>,
    /// Registered compiler backends (C1, C2, JVMCI).
    compilers: RwLock<Vec<std::sync::Arc<Mutex<dyn AbstractCompiler>>>>,
    initialized: AtomicBool,
    /// Mirrors compilation_disabled_forever for lock-free queue waits.
    disabled_forever: AtomicBool,
    /// Tiered compilation policy (internally synchronized).
    policy: CompilationPolicy,
}

static GLOBAL_BROKER: OnceLock<CompileBroker> = OnceLock::new();

impl CompileBroker {
    pub fn new() -> Self {
        Self {
            c1_queue: CompileQueue::new("C1"),
            c2_queue: CompileQueue::new("C2"),
            inner: Mutex::new(BrokerInner::new()),
            compilers: RwLock::new(Vec::new()),
            initialized: AtomicBool::new(false),
            disabled_forever: AtomicBool::new(false),
            policy: CompilationPolicy::new(),
        }
    }

    pub fn global() -> &'static CompileBroker {
        let broker = GLOBAL_BROKER.get_or_init(Self::new);
        broker.initialized.store(true, Ordering::Release);
        broker
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Lazily registers the C1 backend and spawns the compiler thread that
    /// drains the C1 queue.  C2 has no code-generation backend yet, so only
    /// the C1 queue is serviced.  Idempotent and cheap (one atomic swap) —
    /// safe to call from the interpreter's hot path.
    pub fn ensure_compiler_threads_started() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        if STARTED.swap(true, Ordering::AcqRel) {
            return;
        }
        let broker = Self::global();
        let c1: Arc<Mutex<dyn AbstractCompiler>> =
            Arc::new(Mutex::new(crate::share::c1::c1_Compiler::C1Compiler::new()));
        broker.register_compiler(c1);
        let _ = std::thread::Builder::new()
            .name("C1 CompilerThread0".to_string())
            .spawn(move || {
                broker.compiler_thread_loop(CompLevel::Simple, None);
            });
    }

    // ---- queues ---------------------------------------------------------

    pub fn c1_compile_queue(&self) -> &CompileQueue {
        &self.c1_queue
    }

    pub fn c2_compile_queue(&self) -> &CompileQueue {
        &self.c2_queue
    }

    /// `CompileBroker::compile_queue(comp_level)`.
    pub fn compile_queue(&self, comp_level: CompLevel) -> Option<&CompileQueue> {
        if comp_level.is_c2() {
            Some(&self.c2_queue)
        } else if comp_level.is_c1() {
            Some(&self.c1_queue)
        } else {
            None
        }
    }

    /// `CompileBroker::queue_size(comp_level)` — used by the policy's
    /// threshold scaling.
    pub fn queue_size(&self, comp_level: CompLevel) -> usize {
        self.compile_queue(comp_level).map_or(0, |q| q.size())
    }

    pub fn queue_size_c1(&self) -> usize {
        self.c1_queue.size()
    }

    pub fn queue_size_c2(&self) -> usize {
        self.c2_queue.size()
    }

    // ---- compiler registration -------------------------------------------

    /// Register a compiler backend.  `supports_level` decides which queue
    /// it drains.
    pub fn register_compiler(&self, compiler: std::sync::Arc<Mutex<dyn AbstractCompiler>>) {
        self.compilers.write().unwrap().push(compiler);
        // Refresh policy compiler counts.
        let c1 = self.count_compilers_for(|c| c.is_c1());
        let c2 = self.count_compilers_for(|c| c.is_c2());
        self.policy.set_c1_count(c1);
        self.policy.set_c2_count(c2);
    }

    fn count_compilers_for(&self, pred: impl Fn(&dyn AbstractCompiler) -> bool) -> i32 {
        let comps = self.compilers.read().unwrap();
        comps
            .iter()
            .filter(|c| {
                let g = c.lock().unwrap();
                pred(&*g)
            })
            .count() as i32
    }

    /// `CompileBroker::compiler(comp_level)`: first registered compiler
    /// supporting the level.
    pub fn compiler_for_level(&self, comp_level: CompLevel) -> Option<std::sync::Arc<Mutex<dyn AbstractCompiler>>> {
        let comps = self.compilers.read().unwrap();
        comps
            .iter()
            .find(|c| c.lock().unwrap().supports_level(comp_level))
            .cloned()
    }

    pub fn policy(&self) -> &CompilationPolicy {
        &self.policy
    }

    // ---- flags ------------------------------------------------------------

    pub fn is_compilation_disabled(&self) -> bool {
        self.inner.lock().unwrap().compilation_disabled
    }

    pub fn set_compilation_disabled(&self, disabled: bool) {
        self.inner.lock().unwrap().compilation_disabled = disabled;
    }

    /// `is_compilation_disabled_forever`.
    pub fn is_compilation_disabled_forever(&self) -> bool {
        self.inner.lock().unwrap().compilation_disabled_forever
    }

    pub fn set_compilation_disabled_forever(&self, v: bool) {
        self.disabled_forever.store(v, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap();
        inner.compilation_disabled_forever = v;
        self.policy.set_compilation_disabled_forever(v);
    }

    /// `should_compile_new_jobs`.
    pub fn should_compile_new_jobs(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.should_compile_new_jobs && !inner.compilation_disabled
    }

    pub fn set_should_compile_new_jobs(&self, v: bool) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let old = inner.should_compile_new_jobs;
        inner.should_compile_new_jobs = v;
        old
    }

    /// `set_should_block` / `check_should_block`.
    pub fn set_should_block(&self) {
        self.inner.lock().unwrap().should_block.store(true, Ordering::SeqCst);
    }

    /// Called by compilers at safepoints to notice shutdown requests.
    pub fn check_should_block(&self) -> bool {
        self.inner.lock().unwrap().should_block.swap(false, Ordering::SeqCst)
    }

    // ---- method flags -------------------------------------------------------

    pub fn set_method_flags(&self, method_key: &str, flags: MethodCompileFlags) {
        self.inner
            .lock()
            .unwrap()
            .method_flags
            .insert(method_key.to_string(), flags);
    }

    pub fn method_flags(&self, method_key: &str) -> MethodCompileFlags {
        self.inner
            .lock()
            .unwrap()
            .method_flags
            .get(method_key)
            .copied()
            .unwrap_or_default()
    }

    pub fn mark_not_compilable(&self, method_key: &str, level: CompLevel, osr: bool) {
        let mut inner = self.inner.lock().unwrap();
        let flags = inner
            .method_flags
            .entry(method_key.to_string())
            .or_default();
        if osr {
            flags.set_not_osr_compilable(level);
        } else {
            flags.set_not_compilable(level);
        }
    }

    // ---- compile id assignment ----------------------------------------------

    /// `assign_compile_id`: monotonic ids; OSR gets its own counter.
    /// Returns 0 when compilation is disabled.
    fn assign_compile_id(&self, is_osr: bool, is_native: bool) -> usize {
        let mut inner = self.inner.lock().unwrap();
        if is_native {
            inner.native_compilation_id += 1;
            inner.native_compilation_id
        } else if is_osr {
            inner.osr_compilation_id += 1;
            inner.osr_compilation_id
        } else {
            inner.compilation_id += 1;
            inner.compilation_id
        }
    }

    pub fn get_compilation_id(&self) -> usize {
        self.inner.lock().unwrap().compilation_id
    }

    // ---- completion tracking -------------------------------------------------

    fn queue_key(method_key: &str, osr_bci: i32) -> String {
        if osr_bci == CompileTask::INVOCATION_ENTRY_BCI {
            method_key.to_string()
        } else {
            format!("{}@{}", method_key, osr_bci)
        }
    }

    /// `compilation_is_complete`: a usable nmethod at >= comp_level exists.
    pub fn compilation_is_complete(
        &self,
        holder: &str,
        name: &str,
        sig: &str,
        osr_bci: i32,
        comp_level: CompLevel,
    ) -> bool {
        let method_key = CodeCache::method_key(holder, name, sig);
        let cache = CodeCache::global();
        if osr_bci == CompileTask::INVOCATION_ENTRY_BCI {
            if let Some(nm) = cache.find_nmethod(holder, name, sig) {
                if nm.is_in_use() && nm.comp_level().as_i32() >= comp_level.as_i32() {
                    return true;
                }
            }
            // Also consult broker's recorded level (covers levels where the
            // installed nmethod is lower but policy already satisfied).
            let inner = self.inner.lock().unwrap();
            if let Some(&lvl) = inner.completed_levels.get(&method_key) {
                return lvl.as_i32() >= comp_level.as_i32();
            }
        } else if let Some(nm) = cache.find_osr_nmethod(holder, name, sig, osr_bci) {
            if nm.is_in_use() && nm.comp_level().as_i32() >= comp_level.as_i32() {
                return true;
            }
        }
        false
    }

    /// `compilation_is_in_queue`: task pending or in progress.
    pub fn compilation_is_in_queue_key(&self, queue_key: &str) -> bool {
        self.inner.lock().unwrap().queued_methods.contains(queue_key)
    }

    pub fn compilation_is_in_queue(&self, holder: &str, name: &str, sig: &str) -> bool {
        let key = CodeCache::method_key(holder, name, sig);
        self.compilation_is_in_queue_key(&key)
    }

    // ---- prohibition -----------------------------------------------------------

    /// `compilation_is_prohibited`.
    pub fn compilation_is_prohibited(
        &self,
        method_key: &str,
        osr_bci: i32,
        comp_level: CompLevel,
        flags: MethodCompileFlags,
    ) -> bool {
        let is_native = flags.is_native;
        let is_osr = osr_bci != CompileTask::INVOCATION_ENTRY_BCI;

        // Oracle exclusion.
        let parts = split_method_key(method_key);
        let excluded = CompilerOracle::global()
            .read()
            .map(|o| o.should_exclude(&parts.0, &parts.1, &parts.2))
            .unwrap_or(false);
        // CompileThresholdScaling == 0 also excludes.
        let scale_zero = CompilerOracle::global()
            .read()
            .ok()
            .and_then(|o| {
                o.option_value_f64(&parts.0, &parts.1, &parts.2, super::compilerOracle::CompileCommandEnum::CompileThresholdScaling)
            })
            .map_or(false, |s| s == 0.0);

        if excluded || scale_zero {
            self.mark_not_compilable(method_key, comp_level, is_osr);
        }

        if flags.is_not_compilable(comp_level) || (is_osr && flags.is_not_osr_compilable(comp_level)) {
            return true;
        }
        false
    }

    // ---- task creation -----------------------------------------------------------

    fn create_compile_task(
        &self,
        queue: &CompileQueue,
        compile_id: usize,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        osr_bci: i32,
        comp_level: CompLevel,
        hot_count: u32,
        reason: CompileReason,
        blocking: bool,
        profile_key: Option<usize>,
    ) -> CompileTask {
        let task = if osr_bci == CompileTask::INVOCATION_ENTRY_BCI {
            CompileTask::new(
                compile_id,
                class_name,
                method_name,
                descriptor,
                comp_level,
                hot_count,
                reason,
                blocking,
            )
        } else {
            CompileTask::new_osr(
                compile_id,
                class_name,
                method_name,
                descriptor,
                comp_level,
                osr_bci,
                hot_count,
                reason,
                blocking,
            )
        };
        if let Some(pk) = profile_key {
            // profile_key is stored via a wrapper — CompileTask has the field.
            let mut t = task;
            t.profile_key = Some(pk);
            queue.add(t.clone());
            t
        } else {
            queue.add(task.clone());
            task
        }
    }

    /// `compile_method_base`: the full request path.
    /// Returns the queued task (blocking callers should `wait_for_completion`).
    #[allow(clippy::too_many_arguments)]
    pub fn compile_method_base(
        &self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        osr_bci: i32,
        comp_level: CompLevel,
        hot_count: u32,
        reason: CompileReason,
        blocking: bool,
        flags: MethodCompileFlags,
        profile_key: Option<usize>,
    ) -> Option<CompileTask> {
        if !self.is_initialized() || comp_level == CompLevel::None {
            return None;
        }
        let method_key = CodeCache::method_key(
            class_name.as_str(),
            method_name.as_str(),
            descriptor.as_str(),
        );
        let queue_key = Self::queue_key(&method_key, osr_bci);

        // Fast bail-outs (compileBroker.cpp order).
        if self.compilation_is_complete(
            class_name.as_str(),
            method_name.as_str(),
            descriptor.as_str(),
            osr_bci,
            comp_level,
        ) {
            return None;
        }
        {
            let inner = self.inner.lock().unwrap();
            if inner.queued_methods.contains(&queue_key) {
                return None;
            }
        }
        if self.compilation_is_prohibited(&method_key, osr_bci, comp_level, flags) {
            return None;
        }
        // Need a compiler for this level.
        if self.compiler_for_level(comp_level).is_none() {
            return None;
        }
        if !self.should_compile_new_jobs() {
            return None;
        }

        let is_osr = osr_bci != CompileTask::INVOCATION_ENTRY_BCI;
        let compile_id = self.assign_compile_id(is_osr, flags.is_native);
        if compile_id == 0 {
            return None;
        }

        let queue = self.compile_queue(comp_level)?;
        let task = self.create_compile_task(
            queue,
            compile_id,
            class_name,
            method_name,
            descriptor,
            osr_bci,
            comp_level,
            hot_count,
            reason,
            blocking,
            profile_key,
        );

        // Mark queued (dedup) — cleared on completion.
        self.inner.lock().unwrap().queued_methods.insert(queue_key);
        self.policy.note_queued(&method_key);

        if blocking {
            task.wait_for_completion(None);
        }
        Some(task)
    }

    /// `CompileBroker::compile_method` convenience.
    #[allow(clippy::too_many_arguments)]
    pub fn compile_method(
        &self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
        hot_count: u32,
        reason: CompileReason,
        blocking: bool,
    ) -> Option<CompileTask> {
        self.compile_method_base(
            class_name,
            method_name,
            descriptor,
            CompileTask::INVOCATION_ENTRY_BCI,
            comp_level,
            hot_count,
            reason,
            blocking,
            MethodCompileFlags::default(),
            None,
        )
    }

    /// `CompileBroker::compile_osr` convenience.
    #[allow(clippy::too_many_arguments)]
    pub fn compile_osr_method(
        &self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
        osr_bci: i32,
        hot_count: u32,
        reason: CompileReason,
        blocking: bool,
    ) -> Option<CompileTask> {
        self.compile_method_base(
            class_name,
            method_name,
            descriptor,
            osr_bci,
            comp_level,
            hot_count,
            reason,
            blocking,
            MethodCompileFlags::default(),
            None,
        )
    }

    /// Full-flag variant used by the policy driver.
    #[allow(clippy::too_many_arguments)]
    pub fn compile_method_ex(
        &self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        osr_bci: i32,
        comp_level: CompLevel,
        hot_count: u32,
        reason: CompileReason,
        blocking: bool,
        flags: MethodCompileFlags,
        profile_key: Option<usize>,
    ) -> Option<CompileTask> {
        self.compile_method_base(
            class_name,
            method_name,
            descriptor,
            osr_bci,
            comp_level,
            hot_count,
            reason,
            blocking,
            flags,
            profile_key,
        )
    }

    // ---- blocking wait ---------------------------------------------------------

    /// `CompileBroker::wait_for_completion` (task-level wait already exists;
    /// this is the broker-side entry used by `compile_method_base`).
    pub fn wait_for_task(&self, task: &CompileTask) {
        task.wait_for_completion(None);
    }

    // ---- compiler thread loop -----------------------------------------------------

    /// `compiler_thread_loop` analogue: pop tasks for `comp_level`,
    /// invoke the matching compiler, install the result.
    /// Runs until `is_compilation_disabled_forever` or `max_iterations`
    /// (None = forever).
    pub fn compiler_thread_loop(&self, comp_level: CompLevel, max_iterations: Option<u64>) {
        let mut iters = 0u64;
        loop {
            if self.is_compilation_disabled_forever() {
                break;
            }
            if let Some(max) = max_iterations {
                if iters >= max {
                    break;
                }
            }
            iters += 1;

            let queue = match self.compile_queue(comp_level) {
                Some(q) => q,
                None => break,
            };
            let task = queue.get_for_compiler(&self.policy, &self.disabled_forever, Duration::from_secs(5));
            let task = match task {
                Some(t) => t,
                None => {
                    if self.is_compilation_disabled_forever() {
                        break;
                    }
                    continue;
                }
            };
            if std::env::var_os("JVM25_TRACE_COMPILE").is_some() {
                eprintln!(
                    "[jit-task] level={:?} {}.{}{} osr_bci={}",
                    task.comp_level(),
                    task.method.class_name.as_str(),
                    task.method.method_name.as_str(),
                    task.method.descriptor.as_str(),
                    task.osr_bci()
                );
            }

            self.invoke_compiler_on_method(&task);
        }
    }

    /// Execute one task through its compiler and install the nmethod.
    pub fn invoke_compiler_on_method(&self, task: &CompileTask) {
        // Bridge: populate `method_obj` from the live ClassLoader so the
        // compiler backend can read bytecode/locals/flags.  Tasks created
        // by the broker carry only `MethodIdentity` (Symbols); resolve the
        // real `Method` here on the compiler thread.
        let resolved_storage;
        let task = if task.method_obj.is_none() {
            let mut t = task.clone();
            t.method_obj = resolve_method_obj(&t.method);
            resolved_storage = t;
            &resolved_storage
        } else {
            task
        };
        // Populate graph metadata (ldc constants etc.) once per task;
        // bytecode comes from the resolved Method when available.
        let meta_storage;
        let task = if task.jit_metadata.is_none() {
            let mut t = task.clone();
            if let Some(m) = t.method_obj.as_ref() {
                t.jit_metadata = resolve_jit_metadata(&t.method, m.bytecodes());
            }
            meta_storage = t;
            &meta_storage
        } else {
            task
        };
        task.mark_started();
        let level = task.comp_level();
        let compiler = match self.compiler_for_level(level) {
            Some(c) => c,
            None => {
                self.handle_compile_error(task, None, "no compiler for level");
                return;
            }
        };

        let result = {
            let mut guard = compiler.lock().unwrap();
            guard.compile_method(task)
        };

        match result {
            Ok(outcome) => {
                self.install_outcome(task, outcome);
            }
            Err(err) => {
                self.handle_compile_error(task, Some(&err), &err.message);
            }
        }
        self.finish_task(task);
    }

    /// Install a successful CompileOutcome as an nmethod in the CodeCache.
    fn install_outcome(&self, task: &CompileTask, outcome: CompileOutcome) {
        let level = task.comp_level();
        // Normalize pc_descs: sorted with sentinels (HotSpot scopes_pcs
        // layout has lower/upper sentinel entries).
        let mut pc_descs = outcome.pc_descs;
        pc_descs.sort_by_key(|d| d.pc_offset());
        pc_descs.dedup_by_key(|d| d.pc_offset());
        let sentinel_low = crate::share::code::pcDesc::PcDesc::new(
            crate::share::code::pcDesc::PcDesc::LOWER_OFFSET_LIMIT,
            0,
            0,
        );
        let sentinel_high = crate::share::code::pcDesc::PcDesc::new(
            crate::share::code::pcDesc::PcDesc::UPPER_OFFSET_LIMIT,
            0,
            0,
        );
        let mut full = Vec::with_capacity(pc_descs.len() + 2);
        full.push(sentinel_low);
        full.extend(pc_descs);
        full.push(sentinel_high);

        let is_osr = task.is_osr();
        let entry_bci = if is_osr { task.osr_bci() } else { CompileTask::INVOCATION_ENTRY_BCI };
        let code_size = outcome.code_bytes.len();
        // Metadata estimate: pc_descs + scopes + handler table + oop maps.
        let metadata_size = full.len() * 32
            + outcome.scopes.len() * 128
            + outcome.scope_data.len()
            + outcome.exception_table.size_in_bytes()
            + outcome.oop_maps.maps().iter().map(|m| m.data_size() + 16).sum::<usize>();

        let nm = NMethod::with_entry_points(
            task.method.class_name.as_str().to_string(),
            task.method.method_name.as_str().to_string(),
            task.method.descriptor.as_str().to_string(),
            level,
            entry_bci,
            0, // relocated by install_nmethod
            code_size,
            0,
            0,
            0, // entry points relocated by install_nmethod
            full,
            outcome.scopes,
        );

        let cache = CodeCache::global();
        match cache.install_nmethod(nm, outcome.code_bytes.clone(), metadata_size) {
            Ok(addr) => {
                // Retrieve the installed nmethod to fill shared metadata.
                let method_key = CodeCache::method_key(
                    task.method.class_name.as_str(),
                    task.method.method_name.as_str(),
                    task.method.descriptor.as_str(),
                );
                let installed_nm = if is_osr {
                    cache.find_osr_nmethod_by_key(&method_key, entry_bci)
                } else {
                    cache.find_nmethod_by_key(&method_key)
                };
                if let Some(installed) = installed_nm {
                    installed.set_code_bytes(outcome.code_bytes);
                    installed.set_scope_data(outcome.scope_data);
                    installed.set_handler_table(outcome.exception_table);
                    installed.set_pc_handlers(outcome.pc_handlers);
                    let mut maps: Vec<_> = outcome.oop_maps.maps().to_vec();
                    maps.sort_by_key(|m| m.offset());
                    for map in maps {
                        installed.add_oop_map(map.offset(), map);
                    }

                    task.set_nm_total_size(metadata_size + code_size);
                    task.set_nm_content_size(code_size);
                    task.set_nm_insts_size(code_size);
                    task.mark_success();

                    // Publish the entry point to the interpreter's
                    // per-method dispatch: the next invocation of this
                    // method calls the compiled code directly. OSR
                    // tasks also produce full-method code (the C1
                    // backend has no separate OSR entry sequence), so
                    // they publish the same way; repeated OSR installs
                    // simply overwrite with an equivalent blob.
                    if let Some(key) = task.profile_key {
                        methodCounters::mark_compiled(key, addr);
                    }

                    let mut inner = self.inner.lock().unwrap();
                    inner.total_compile_count += 1;
                    if is_osr {
                        inner.total_osr_compile_count += 1;
                        inner.sum_osr_bytes_compiled += code_size as u64;
                        inner
                            .completed_osr
                            .entry(method_key.clone())
                            .or_default()
                            .insert(entry_bci, level);
                    } else {
                        inner.total_standard_compile_count += 1;
                        inner.sum_standard_bytes_compiled += code_size as u64;
                        inner.completed_levels.insert(method_key.clone(), level);
                    }
                    self.policy.note_level(&method_key, level, is_osr);
                    self.policy.note_completed(&method_key);
                }
            }
            Err(e) => {
                task.set_failure_reason(&format!("code cache allocation failed: {}", e));
                let mut inner = self.inner.lock().unwrap();
                inner.total_bailout_count += 1;
            }
        }
    }

    /// `handle_compile_error`: record failure on the task.
    fn handle_compile_error(&self, task: &CompileTask, err: Option<&CompileError>, msg: &str) {
        if std::env::var_os("JVM25_TRACE_COMPILE").is_some() {
            eprintln!(
                "[jit-fail] {}.{}{} level={:?}: {}",
                task.method.class_name.as_str(),
                task.method.method_name.as_str(),
                task.method.descriptor.as_str(),
                task.comp_level(),
                msg
            );
        }
        let reason = err.map(|e| e.reason).unwrap_or(FailureReason::InternalError);
        task.set_failure_reason_at(&format!("{}: {}", reason.as_str(), msg), task.comp_level());
        // Permanent, method-specific failures must not be retried — otherwise
        // every invocation re-queues the same doomed task (HotSpot marks the
        // method not-compilable for these).  Systemic reasons (internal error,
        // OOM, timeout, disabled) stay retryable.
        let permanent = matches!(
            reason,
            FailureReason::MethodNotCompilable
                | FailureReason::MethodTooBig
                | FailureReason::ParseError
                | FailureReason::Unresolved
                | FailureReason::Unsupported
                | FailureReason::ConstraintViolation
                | FailureReason::ForwardReference
                | FailureReason::CompilerOracleExcluded
                | FailureReason::StackOverflow
        );
        if permanent {
            let method_key = CodeCache::method_key(
                task.method.class_name.as_str(),
                task.method.method_name.as_str(),
                task.method.descriptor.as_str(),
            );
            self.mark_not_compilable(&method_key, task.comp_level(), task.is_osr);
        }
        let mut inner = self.inner.lock().unwrap();
        inner.total_bailout_count += 1;
    }

    /// Clear the queued marker and complete the task (success path already
    /// marked success; failure path set failure reason).
    fn finish_task(&self, task: &CompileTask) {
        let method_key = CodeCache::method_key(
            task.method.class_name.as_str(),
            task.method.method_name.as_str(),
            task.method.descriptor.as_str(),
        );
        let qkey = Self::queue_key(&method_key, task.osr_bci());
        {
            let mut inner = self.inner.lock().unwrap();
            inner.queued_methods.remove(&qkey);
            inner.queued_methods.remove(&method_key);
        }
        if !task.is_complete() {
            task.mark_complete();
        }
    }

    // ---- policy-driven entry points ----------------------------------------------

    /// Feed an invocation-counter event into the policy and enqueue any
    /// resulting compilation.  This is the integration point the
    /// interpreter calls on method entry (after incrementing counters).
    ///
    /// `profile_key` identifies the method in `methodCounters`; counters
    /// are snapshotted read-only (the live map is owned by oops).
    #[allow(clippy::too_many_arguments)]
    pub fn compile_if_required(
        &self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        profile_key: Option<usize>,
        flags: MethodCompileFlags,
        blocking: bool,
    ) -> Option<CompileTask> {
        Self::ensure_compiler_threads_started();
        let method_key = CodeCache::method_key(
            class_name.as_str(),
            method_name.as_str(),
            descriptor.as_str(),
        );
        let profile = profile_key
            .and_then(methodCounters::get)
            .map(|c| {
                let mut p = MethodProfile::from_counters(&c);
                p.is_trivial = flags.is_trivial;
                p.is_old = self.policy.is_old(&method_key);
                p
            })
            .unwrap_or_else(|| MethodProfile {
                is_trivial: flags.is_trivial,
                ..Default::default()
            });

        let oracle_scale = self
            .policy
            .oracle_threshold_scaling(&class_name, &method_name, &descriptor);

        let (q1, q2) = {
            let inner = self.inner.lock().unwrap();
            (self.policy.compiler_count(CompLevel::FullProfile) as usize, self.policy.compiler_count(CompLevel::FullOptimization) as usize)
        };
        let _ = (q1, q2);
        let actions = {
            let inner = self.inner.lock().unwrap();
            let cur = self.policy.comp_level(&method_key);
            self.policy.method_invocation_event(
                &method_key,
                &profile,
                cur,
                self.queue_size_c1(),
                self.queue_size_c2(),
                oracle_scale,
                flags.is_trivial,
            )
        };
        self.execute_actions(&method_key, &class_name, &method_name, &descriptor, profile_key, flags, blocking, actions, None)
    }

    /// Feed a back-edge event; may trigger an OSR compile and/or a
    /// regular recompile, plus not-entrant marking of the running nmethod.
    #[allow(clippy::too_many_arguments)]
    pub fn backedge_event(
        &self,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        bci: i32,
        profile_key: Option<usize>,
        flags: MethodCompileFlags,
        blocking: bool,
        enclosing: Option<(String, CompLevel, bool)>, // (key, level, is_osr)
        max_osr_level: CompLevel,
    ) -> Option<CompileTask> {
        Self::ensure_compiler_threads_started();
        let method_key = CodeCache::method_key(
            class_name.as_str(),
            method_name.as_str(),
            descriptor.as_str(),
        );
        let profile = profile_key
            .and_then(methodCounters::get)
            .map(|c| {
                let mut p = MethodProfile::from_counters(&c);
                p.is_trivial = flags.is_trivial;
                p
            })
            .unwrap_or_default();

        let actions = {
            let inner = self.inner.lock().unwrap();
            let cur = self.policy.comp_level(&method_key);
            self.policy.method_back_branch_event(
                &method_key,
                &profile,
                bci,
                cur,
                self.queue_size_c1(),
                self.queue_size_c2(),
                flags.is_trivial,
                enclosing.as_ref().map(|(k, l, o)| (k.as_str(), *l, *o)),
                max_osr_level,
            )
        };
        self.execute_actions(&method_key, &class_name, &method_name, &descriptor, profile_key, flags, blocking, actions, Some(bci))
    }

    /// Execute PolicyActions: enqueue OSR/regular compiles and mark
    /// not-entrant.  Returns the (first) created task.
    #[allow(clippy::too_many_arguments)]
    fn execute_actions(
        &self,
        method_key: &str,
        class_name: &Symbol,
        method_name: &Symbol,
        descriptor: &Symbol,
        profile_key: Option<usize>,
        flags: MethodCompileFlags,
        blocking: bool,
        actions: PolicyActions,
        osr_bci_hint: Option<i32>,
    ) -> Option<CompileTask> {
        if std::env::var_os("JVM25_TRACE_COMPILE").is_some() {
            eprintln!(
                "[jit-actions] {}.{}{} call={:?} osr={:?} not_entrant={}",
                class_name.as_str(),
                method_name.as_str(),
                descriptor.as_str(),
                actions.call_compile_level,
                actions.osr_compile,
                actions.make_not_entrant
            );
        }
        let mut first_task = None;
        let hot_count = profile_key
            .and_then(methodCounters::get)
            .map(|c| c.invocation_count().max(c.backedge_count()))
            .unwrap_or(0);

        if actions.make_not_entrant {
            self.make_not_entrant_by_key(method_key);
            let mut inner = self.inner.lock().unwrap();
            inner.total_invalidated_count += 1;
        }

        if let Some((level, bci)) = actions.osr_compile {
            let bci = osr_bci_hint.unwrap_or(bci);
            if let Some(t) = self.compile_method_ex(
                class_name.clone(),
                method_name.clone(),
                descriptor.clone(),
                bci,
                level,
                hot_count,
                CompileReason::ReasonTiered,
                blocking,
                flags,
                profile_key,
            ) {
                if first_task.is_none() {
                    first_task = Some(t);
                }
            }
        }

        if let Some(level) = actions.call_compile_level {
            if let Some(t) = self.compile_method_ex(
                class_name.clone(),
                method_name.clone(),
                descriptor.clone(),
                CompileTask::INVOCATION_ENTRY_BCI,
                level,
                hot_count,
                CompileReason::ReasonTiered,
                blocking,
                flags,
                profile_key,
            ) {
                if first_task.is_none() {
                    first_task = Some(t);
                }
            }
        }
        first_task
    }

    // ---- not-entrant ---------------------------------------------------------------

    /// `mark_for_deoptimization` + `make_not_entrant` for the nmethod of
    /// the given method (used by policy backedge events).
    pub fn make_not_entrant_by_key(&self, method_key: &str) -> bool {
        CodeCache::global().make_not_entrant_by_key(method_key)
    }

    pub fn make_not_entrant(&self, holder: &str, name: &str, sig: &str) -> bool {
        CodeCache::global().make_not_entrant(holder, name, sig)
    }

    // ---- sweeper / deopt ---------------------------------------------------------

    /// `deoptimize_all` / `deoptimize_invalidated`.
    pub fn deoptimize_all(&self) {
        CodeCache::global().mark_all_nmethods_for_deopt();
    }

    pub fn deoptimize_method(&self, holder: &str, name: &str, sig: &str) {
        let key = CodeCache::method_key(holder, name, sig);
        if let Some(nm) = CodeCache::global().find_nmethod_by_key(&key) {
            nm.mark_for_deoptimization();
        }
        let _ = self.make_not_entrant(holder, name, sig);
    }

    /// Run one sweeper cycle over the global code cache.
    pub fn sweep(&self, active_stack_keys: &HashSet<String>) -> crate::share::code::codeCache::SweepSummary {
        CodeCache::global().sweep_cycle(active_stack_keys)
    }

    // ---- stats / printing ---------------------------------------------------------

    pub fn total_compile_count(&self) -> u64 {
        self.inner.lock().unwrap().total_compile_count
    }

    pub fn total_bailout_count(&self) -> u64 {
        self.inner.lock().unwrap().total_bailout_count
    }

    pub fn total_osr_compile_count(&self) -> u64 {
        self.inner.lock().unwrap().total_osr_compile_count
    }

    pub fn total_standard_compile_count(&self) -> u64 {
        self.inner.lock().unwrap().total_standard_compile_count
    }

    pub fn print_compile_queues(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(f, "Current compiles:");
        self.c1_queue.print_on(f);
        self.c2_queue.print_on(f);
    }

    pub fn print_stats(&self, f: &mut dyn std::fmt::Write) {
        let inner = self.inner.lock().unwrap();
        let _ = writeln!(
            f,
            "CompileBroker: compiles={} bailouts={} osr={} standard={} invalidated={} bytes_osr={} bytes_std={}",
            inner.total_compile_count,
            inner.total_bailout_count,
            inner.total_osr_compile_count,
            inner.total_standard_compile_count,
            inner.total_invalidated_count,
            inner.sum_osr_bytes_compiled,
            inner.sum_standard_bytes_compiled,
        );
    }

    /// `free_all_compile_tasks`: fail every queued task (shutdown path).
    pub fn free_all_compile_tasks(&self) {
        self.c1_queue.free_all();
        self.c2_queue.free_all();
    }

    /// Remove stale tasks from both queues.
    pub fn remove_stale_tasks(&self) {
        self.c1_queue.purge_stale_tasks();
        self.c2_queue.purge_stale_tasks();
    }
}

impl Default for CompileBroker {
    fn default() -> Self {
        Self::new()
    }
}

fn split_method_key(key: &str) -> (String, String, String) {
    // key = "holder.name+descriptor" where descriptor starts at first '('
    // after the last '.'.  We stored "class.name sig" as class.name<sig>.
    if let Some(dot) = key.rfind('.') {
        let holder_name = &key[..dot];
        let rest = &key[dot + 1..];
        // rest = "methodName(sig)ret" — split name/sig at first '('
        if let Some(paren) = rest.find('(') {
            let name = rest[..paren].to_string();
            let sig = rest[paren..].to_string();
            // holder_name = "holder.method"? No: key = holder.name sig?
            // Our method_key = "{class}.{name}{sig}", so holder+name are
            // ambiguous; recover: holder = up to last '.' before name.
            if let Some(d2) = holder_name.rfind('.') {
                let holder = holder_name[..d2].to_string();
                let mname = format!("{}{}", &holder_name[d2 + 1..], name);
                return (holder, mname, sig);
            }
            return (holder_name.to_string(), name, sig);
        }
    }
    (key.to_string(), String::new(), String::new())
}
