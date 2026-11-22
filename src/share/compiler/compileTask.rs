/*
 * Port of OpenJDK 25 HotSpot CompileTask (share/compiler/compileTask.hpp/.cpp)
 *
 * A CompileTask represents one compilation request: method identity,
 * target level, OSR bci, reason, blocking semantics and completion state.
 * Tasks are shared between mutator threads (which may block waiting for a
 * blocking compile) and compiler threads (which execute them), so all
 * mutable state lives behind a shared `Arc<TaskShared>`.
 */

use super::compilerDefinitions::CompLevel;
use crate::share::oops::symbol::Symbol;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompileReason {
    ReasonNone = 0,
    ReasonInvocationCount,
    ReasonBackedgeCount,
    ReasonTiered,
    ReasonReplay,
    ReasonWhitebox,
    ReasonMustBeCompiled,
    ReasonBootstrap,
    ReasonCount,
}

impl CompileReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReasonNone => "no_reason",
            Self::ReasonInvocationCount => "count",
            Self::ReasonBackedgeCount => "backedge_count",
            Self::ReasonTiered => "tiered",
            Self::ReasonReplay => "replay",
            Self::ReasonWhitebox => "whitebox",
            Self::ReasonMustBeCompiled => "must_be_compiled",
            Self::ReasonBootstrap => "bootstrap",
            Self::ReasonCount => "count",
        }
    }

    /// `CompileTask::can_become_stale`: non-blocking count-driven tasks
    /// may be dropped from the queue when the method goes cold.
    pub fn can_become_stale(&self, is_blocking: bool) -> bool {
        match self {
            Self::ReasonBackedgeCount | Self::ReasonInvocationCount | Self::ReasonTiered => {
                !is_blocking
            }
            _ => false,
        }
    }

    /// `CompileTask::should_wait_for_compilation` (JVMCI reasons).
    pub fn should_wait_for_compilation(&self, is_blocking: bool) -> bool {
        matches!(
            self,
            Self::ReasonReplay | Self::ReasonWhitebox | Self::ReasonBootstrap
        ) && is_blocking
    }
}

/// Mutable per-task state shared across threads.
#[derive(Debug)]
pub struct TaskState {
    pub is_complete: bool,
    pub is_success: bool,
    pub is_blocking: bool,
    pub comp_level: CompLevel,
    pub failure_reason: Option<String>,
    pub failed_at_level: Option<CompLevel>,
    pub current_level: CompLevel,
    pub time_queued: i64,
    pub time_started: i64,
    pub waiting_count: u32,
    pub is_free: bool,
    pub stale: bool,
    pub unloaded: bool,
    pub nm_content_size: usize,
    pub nm_total_size: usize,
    pub nm_insts_size: usize,
    pub arena_bytes: usize,
    pub inlined_bytecodes: usize,
}

impl TaskState {
    fn new(comp_level: CompLevel, is_blocking: bool, time_queued: i64) -> Self {
        Self {
            is_complete: false,
            is_success: false,
            is_blocking,
            comp_level,
            failure_reason: None,
            failed_at_level: None,
            current_level: comp_level,
            time_queued,
            time_started: 0,
            waiting_count: 0,
            is_free: false,
            stale: false,
            unloaded: false,
            nm_content_size: 0,
            nm_total_size: 0,
            nm_insts_size: 0,
            arena_bytes: 0,
            inlined_bytecodes: 0,
        }
    }
}

#[derive(Debug)]
struct TaskShared {
    state: Mutex<TaskState>,
    done: Condvar,
    /// Per-bci live-reference maps for deoptimization, filled in by the
    /// compiler backend during `compile_method` (`&CompileTask` is
    /// shared, so the data lives here rather than in CompileOutcome
    /// which is produced by value).
    live_maps: Mutex<Vec<crate::share::code::nmethod::DeoptLiveMap>>,
}

impl TaskShared {
    fn new(comp_level: CompLevel, is_blocking: bool, time_queued: i64) -> Self {
        Self {
            state: Mutex::new(TaskState::new(comp_level, is_blocking, time_queued)),
            done: Condvar::new(),
            live_maps: Mutex::new(Vec::new()),
        }
    }
}

/// Identity of the method to compile.  `method_key` is the canonical
/// `holder.name+descriptor` string used for queue dedup and nmethod lookup;
/// the `Symbol` fields keep the raw components.
#[derive(Clone, Debug)]
pub struct MethodIdentity {
    pub class_name: Symbol,
    pub method_name: Symbol,
    pub descriptor: Symbol,
    pub method_key: String,
}

impl MethodIdentity {
    pub fn new(class_name: Symbol, method_name: Symbol, descriptor: Symbol) -> Self {
        let method_key = format!("{}.{}{}", class_name.as_str(), method_name.as_str(), descriptor.as_str());
        Self {
            class_name,
            method_name,
            descriptor,
            method_key,
        }
    }

    pub fn method_key(&self) -> &str {
        &self.method_key
    }
}

/// Result payload produced by a compiler for a finished task.
/// Filled by the compiler backend; consumed by the broker when installing
/// the nmethod into the code cache.
#[derive(Debug, Default)]
pub struct CompileOutcome {
    pub code_bytes: Vec<u8>,
    pub frame_size: usize,
    pub pc_descs: Vec<crate::share::code::pcDesc::PcDesc>,
    pub scopes: Vec<crate::share::code::scopeDesc::ScopeDesc>,
    pub scope_data: Vec<u8>,
    pub exception_table: crate::share::code::exceptionHandlerTable::ExceptionHandlerTable,
    pub pc_handlers: crate::share::code::exceptionHandlerTable::PcRangeHandlerTable,
    pub oop_maps: crate::share::compiler::oopMap::OopMapSet,
    pub num_inlined_bytecodes: usize,
}

/// Interpreter frame snapshot captured at the backedge that requested
/// an OSR compile: local-slot kinds (max_locals entries) and operand
/// stack depth at the target bci.  Carried on the task so the compiler
/// can build `opto::builder::OsrSpec` without re-deriving types.
#[derive(Clone, Debug, Default)]
pub struct OsrFrame {
    /// Local-slot kinds at the OSR entry bci (max_locals entries).
    pub local_kinds: Vec<crate::share::opto::builder::OsrKind>,
    /// Operand-stack slot kinds at the OSR entry bci.
    pub stack_kinds: Vec<crate::share::opto::builder::OsrKind>,
}

#[derive(Clone, Debug)]
pub struct CompileTask {
    pub compile_id: usize,
    pub method: MethodIdentity,
    /// Optional live `Method` handle when the caller has one; lets the
    /// compiler/policy read invocation and backedge counters.
    pub method_obj: Option<Arc<crate::share::oops::method::Method>>,
    /// Optional pre-resolved graph metadata (ldc constants, field and
    /// call-site info, exception handlers) collected by the broker from
    /// the owning `InstanceKlass`; `None` falls back to `Default`.
    pub jit_metadata: Option<crate::share::c1::c1_GraphBuilder::MethodMetadata>,
    /// Key into the interpreter's live counters (`methodCounters::get`).
    pub profile_key: Option<usize>,
    /// Broker deinstall epoch captured when the task was queued; the
    /// broker refuses to install a task whose epoch went stale (the
    /// method was deinstalled while the task was in flight).
    pub submit_epoch: u64,
    pub hot_count: u32,
    pub reason: CompileReason,
    pub is_osr: bool,
    pub osr_bci: i32,
    /// Frame snapshot for OSR compiles; `None` for invocation entries.
    pub osr_frame: Option<OsrFrame>,
    shared: Arc<TaskShared>,
}

impl CompileTask {
    pub const INVOCATION_ENTRY_BCI: i32 = -1;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        compile_id: usize,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
        hot_count: u32,
        reason: CompileReason,
        is_blocking: bool,
    ) -> Self {
        Self {
            compile_id,
            method: MethodIdentity::new(class_name, method_name, descriptor),
            method_obj: None,
            jit_metadata: None,
            profile_key: None,
            submit_epoch: 0,
            hot_count,
            reason,
            is_osr: false,
            osr_bci: Self::INVOCATION_ENTRY_BCI,
            osr_frame: None,
            shared: Arc::new(TaskShared::new(
                comp_level,
                is_blocking,
                now_millis(),
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_osr(
        compile_id: usize,
        class_name: Symbol,
        method_name: Symbol,
        descriptor: Symbol,
        comp_level: CompLevel,
        osr_bci: i32,
        hot_count: u32,
        reason: CompileReason,
        is_blocking: bool,
    ) -> Self {
        let mut t = Self::new(
            compile_id,
            class_name,
            method_name,
            descriptor,
            comp_level,
            hot_count,
            reason,
            is_blocking,
        );
        t.is_osr = true;
        t.osr_bci = osr_bci;
        t
    }

    // ---- accessors ---------------------------------------------------

    pub fn compile_id(&self) -> usize {
        self.compile_id
    }

    pub fn method_key(&self) -> &str {
        self.method.method_key()
    }

    pub fn class_name(&self) -> &Symbol {
        &self.method.class_name
    }

    pub fn method_name(&self) -> &Symbol {
        &self.method.method_name
    }

    pub fn descriptor(&self) -> &Symbol {
        &self.method.descriptor
    }

    pub fn comp_level(&self) -> CompLevel {
        self.shared.state.lock().unwrap().comp_level
    }

    pub fn set_comp_level(&self, level: CompLevel) {
        self.shared.state.lock().unwrap().comp_level = level;
    }

    pub fn current_level(&self) -> CompLevel {
        self.shared.state.lock().unwrap().current_level
    }

    pub fn set_current_level(&self, level: CompLevel) {
        self.shared.state.lock().unwrap().current_level = level;
    }

    pub fn osr_bci(&self) -> i32 {
        self.osr_bci
    }

    pub fn is_osr(&self) -> bool {
        self.is_osr || self.osr_bci != Self::INVOCATION_ENTRY_BCI
    }

    pub fn reason(&self) -> CompileReason {
        self.reason
    }

    pub fn is_blocking(&self) -> bool {
        self.shared.state.lock().unwrap().is_blocking
    }

    /// Convert a non-blocking task into a blocking one (used when a
    /// mutator decides to wait for its own compilation).
    pub fn set_blocking(&self, blocking: bool) {
        self.shared.state.lock().unwrap().is_blocking = blocking;
    }

    pub fn hot_count(&self) -> u32 {
        self.hot_count
    }

    pub fn num_inlined_bytecodes(&self) -> usize {
        self.shared.state.lock().unwrap().inlined_bytecodes
    }

    pub fn set_num_inlined_bytecodes(&self, n: usize) {
        let mut s = self.shared.state.lock().unwrap();
        s.inlined_bytecodes = n;
        s.nm_insts_size = n;
    }

    pub fn time_queued(&self) -> i64 {
        self.shared.state.lock().unwrap().time_queued
    }

    pub fn time_started(&self) -> i64 {
        self.shared.state.lock().unwrap().time_started
    }

    pub fn mark_started(&self) {
        self.shared.state.lock().unwrap().time_started = now_millis();
    }

    pub fn can_become_stale(&self) -> bool {
        self.reason.can_become_stale(self.is_blocking())
    }

    pub fn is_unloaded(&self) -> bool {
        self.shared.state.lock().unwrap().unloaded
    }

    pub fn mark_unloaded(&self) {
        self.shared.state.lock().unwrap().unloaded = true;
        self.shared.state.lock().unwrap().stale = true;
    }

    pub fn is_stale(&self) -> bool {
        self.shared.state.lock().unwrap().stale
    }

    pub fn mark_stale(&self) {
        self.shared.state.lock().unwrap().stale = true;
    }

    // ---- completion ---------------------------------------------------

    pub fn is_complete(&self) -> bool {
        self.shared.state.lock().unwrap().is_complete
    }

    pub fn is_success(&self) -> bool {
        self.shared.state.lock().unwrap().is_success
    }

    pub fn mark_complete(&self) {
        {
            let mut s = self.shared.state.lock().unwrap();
            s.is_complete = true;
        }
        self.shared.done.notify_all();
    }

    pub fn mark_success(&self) {
        self.shared.state.lock().unwrap().is_success = true;
    }

    pub fn set_failure_reason(&self, reason: &str) {
        self.set_failure_reason_at(reason, self.comp_level());
    }

    pub fn set_failure_reason_at(&self, reason: &str, level: CompLevel) {
        {
            let mut s = self.shared.state.lock().unwrap();
            s.failure_reason = Some(reason.to_string());
            s.failed_at_level = Some(level);
        }
        self.mark_complete();
    }

    pub fn failure_reason(&self) -> Option<String> {
        self.shared.state.lock().unwrap().failure_reason.clone()
    }

    pub fn failed_at_level(&self) -> Option<CompLevel> {
        self.shared.state.lock().unwrap().failed_at_level
    }

    // ---- blocking wait -------------------------------------------------

    /// `CompileBroker::wait_for_completion` analogue: block until the task
    /// completes or `timeout` elapses.  Returns true if completed.
    pub fn wait_for_completion(&self, timeout: Option<Duration>) -> bool {
        let mut s = self.shared.state.lock().unwrap();
        self.shared.lock_waiting_inc(&mut s);
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            if s.is_complete {
                break;
            }
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        break;
                    }
                    let remaining = d - now;
                    let (guard, _) = self
                        .shared
                        .done
                        .wait_timeout(s, remaining)
                        .unwrap_or_else(|e| e.into_inner());
                    s = guard;
                }
                None => {
                    s = self
                        .shared
                        .done
                        .wait(s)
                        .unwrap_or_else(|e| e.into_inner());
                }
            }
        }
        self.shared.lock_waiting_dec(&mut s);
        s.is_complete
    }

    pub fn waiting_for_completion_count(&self) -> u32 {
        self.shared.state.lock().unwrap().waiting_count
    }

    // ---- deopt live maps ------------------------------------------------

    /// Record the per-bci live-reference maps produced by the compiler
    /// (sorted by bci, deduplicated).
    pub fn set_live_maps(&self, mut maps: Vec<crate::share::code::nmethod::DeoptLiveMap>) {
        maps.sort_by_key(|m| m.bci());
        maps.dedup_by_key(|m| m.bci());
        *self.shared.live_maps.lock().unwrap() = maps;
    }

    /// Add or replace the live map for one bci.
    pub fn add_live_map(&self, map: crate::share::code::nmethod::DeoptLiveMap) {
        let mut maps = self.shared.live_maps.lock().unwrap();
        match maps.binary_search_by_key(&map.bci(), |m| m.bci()) {
            Ok(idx) => maps[idx] = map,
            Err(idx) => maps.insert(idx, map),
        }
    }

    /// All live maps recorded so far (compiler thread or broker).
    pub fn live_maps(&self) -> Vec<crate::share::code::nmethod::DeoptLiveMap> {
        self.shared.live_maps.lock().unwrap().clone()
    }

    // ---- code size stats ----------------------------------------------

    pub fn nm_content_size(&self) -> usize {
        self.shared.state.lock().unwrap().nm_content_size
    }

    pub fn set_nm_content_size(&self, size: usize) {
        self.shared.state.lock().unwrap().nm_content_size = size;
    }

    pub fn nm_insts_size(&self) -> usize {
        self.shared.state.lock().unwrap().nm_insts_size
    }

    pub fn set_nm_insts_size(&self, size: usize) {
        self.shared.state.lock().unwrap().nm_insts_size = size;
    }

    pub fn nm_total_size(&self) -> usize {
        self.shared.state.lock().unwrap().nm_total_size
    }

    pub fn set_nm_total_size(&self, size: usize) {
        self.shared.state.lock().unwrap().nm_total_size = size;
    }

    pub fn arena_bytes(&self) -> usize {
        self.shared.state.lock().unwrap().arena_bytes
    }

    pub fn set_arena_bytes(&self, size: usize) {
        self.shared.state.lock().unwrap().arena_bytes = size;
    }

    // ---- printing ------------------------------------------------------

    pub fn print_on(&self, f: &mut dyn std::fmt::Write, msg: Option<&str>, short_form: bool) {
        let blocking = if self.is_blocking() { " blocking" } else { "" };
        let osr = if self.is_osr() {
            format!(" osr_bci={}", self.osr_bci)
        } else {
            String::new()
        };
        let _ = writeln!(
            f,
            "CompileTask #{}: {} level {}{}{} reason {} hot_count {}{}",
            self.compile_id,
            self.method.method_key,
            self.comp_level().name(),
            osr,
            blocking,
            self.reason.as_str(),
            self.hot_count,
            msg.map(|m| format!(" ({})", m)).unwrap_or_default()
        );
        if !short_form {
            if let Some(fr) = self.failure_reason() {
                let _ = writeln!(f, "  failure: {}", fr);
            }
        }
    }
}

impl TaskShared {
    fn lock_waiting_inc(&self, s: &mut TaskState) {
        s.waiting_count += 1;
    }
    fn lock_waiting_dec(&self, s: &mut TaskState) {
        s.waiting_count = s.waiting_count.saturating_sub(1);
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Compile failure reasons (`ciFailureReason` analogue).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureReason {
    Nope,
    CompiledMethodLoadError,
    CompilerOracleExcluded,
    CompilationDisabled,
    CompilationTimedOut,
    ConstraintViolation,
    DependsOnInvalidatedClass,
    ForwardReference,
    InternalError,
    MemoryExhausted,
    MethodNotCompilable,
    MethodTooBig,
    ParseError,
    StackOverflow,
    Unresolved,
    Unsupported,
}

impl FailureReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nope => "no failure",
            Self::CompiledMethodLoadError => "compiled method load error",
            Self::CompilerOracleExcluded => "excluded by compiler oracle",
            Self::CompilationDisabled => "compilation disabled",
            Self::CompilationTimedOut => "compilation timed out",
            Self::ConstraintViolation => "constraint violation",
            Self::DependsOnInvalidatedClass => "depends on invalidated class",
            Self::ForwardReference => "forward reference",
            Self::InternalError => "internal error",
            Self::MemoryExhausted => "memory exhausted",
            Self::MethodNotCompilable => "method not compilable",
            Self::MethodTooBig => "method too big",
            Self::ParseError => "parse error",
            Self::StackOverflow => "stack overflow",
            Self::Unresolved => "unresolved",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Inlining outcome recorded during compilation (`InliningResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InliningResult {
    Success,
    Failure,
}
