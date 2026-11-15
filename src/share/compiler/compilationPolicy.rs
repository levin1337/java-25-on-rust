/*
 * Port of OpenJDK 25 HotSpot CompilationPolicy
 * (share/compiler/compilationPolicy.hpp / compilationPolicy.cpp)
 *
 * Tiered compilation policy: decides when a method should move between
 * levels 0 (interpreter) -> 2/3 (C1 profiled) -> 4 (C2/full optimization),
 * handles OSR (on-stack-replacement) compilations at back-edges, maintains
 * per-method execution rates for compile-queue prioritization and staleness
 * detection, and implements the Tier3DelayOn/Off hysteresis that trades
 * full profiling for limited profiling when the C2 queue is congested.
 *
 * Counters: HotSpot reads invocation/backedge counters from Method and MDO.
 * Here `MethodProfile` carries the same data; `MethodCounters` snapshots
 * (share::oops::methodCounters) convert into it, and per-method highest
 * achieved levels / rates are tracked in `PolicyState` keyed by method key.
 */

use super::compileBroker::CompileQueue;
use super::compileTask::CompileTask;
use super::compilerDefinitions::{CompilationMode, CompilerConfig, CompLevel};
use super::compilerOracle::{CompileCommandEnum, CompilerOracle};
use crate::share::oops::methodCounters::MethodCounters;
use crate::share::oops::symbol::Symbol;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// HotSpot product defaults (globals.hpp).
pub const TIER3_INVOCATION_THRESHOLD: u32 = 200;
pub const TIER3_MIN_INVOCATION_THRESHOLD: u32 = 100;
pub const TIER3_COMPILE_THRESHOLD: u32 = 2000;
pub const TIER3_BACKEDGE_THRESHOLD: u32 = 60_000;
pub const TIER4_INVOCATION_THRESHOLD: u32 = 2000;
pub const TIER4_MIN_INVOCATION_THRESHOLD: u32 = 600;
pub const TIER4_COMPILE_THRESHOLD: u32 = 15_000;
pub const TIER4_BACKEDGE_THRESHOLD: u32 = 40_000;
pub const TIER3_LOAD_FEEDBACK: i32 = 5;
pub const TIER4_LOAD_FEEDBACK: i32 = 3;
pub const TIER3_DELAY_ON: usize = 10_000;
pub const TIER3_DELAY_OFF: usize = 2_000;
pub const TIER0_PROFILING_START_PERCENTAGE: f64 = 3.0;
pub const TIER0_DELAY: usize = 200;
pub const PROFILE_MATURITY_PERCENTAGE: f64 = 90.0;
pub const TIERED_COMPILE_TASK_TIMEOUT: i64 = 50; // seconds-ish granularity below
pub const TIERED_RATE_UPDATE_MIN_TIME_MS: i64 = 1;
pub const TIERED_RATE_UPDATE_MAX_TIME_MS: i64 = 1000;
pub const INCREASE_FIRST_TIER_COMPILE_THRESHOLD_AT: f64 = 50.0;

/// Snapshot of everything the policy needs to know about a method —
/// the `Method*` counters plus MDO deltas where available.
#[derive(Clone, Debug, Default)]
pub struct MethodProfile {
    pub invocation_count: u32,
    pub backedge_count: u32,
    /// Highest level the method has been compiled at (regular entry).
    pub highest_comp_level: CompLevel,
    /// Highest OSR level reached.
    pub highest_osr_comp_level: CompLevel,
    /// Profiled deltas collected since the last level-3 compile (MDO).
    /// `None` means no profiling data exists yet.
    pub mdo_invocation_delta: Option<u32>,
    pub mdo_backedge_delta: Option<u32>,
    /// Whether the MDO would produce useful profiling information.
    pub mdo_would_profile: bool,
    /// Whether the method is "old" (long-running in the interpreter).
    pub is_old: bool,
    /// Method is trivial (accessor / constant getter): C1==C2 quality.
    pub is_trivial: bool,
    /// MethodData is mature enough to drive 3->4 without thresholds.
    pub mdo_mature: bool,
}

impl MethodProfile {
    pub fn from_counters(c: &MethodCounters) -> Self {
        Self {
            invocation_count: c.invocation_count(),
            backedge_count: c.backedge_count(),
            highest_comp_level: CompLevel::from_i32(c.highest_comp_level() as i32)
                .unwrap_or(CompLevel::None),
            highest_osr_comp_level: CompLevel::from_i32(c.highest_osr_comp_level() as i32)
                .unwrap_or(CompLevel::None),
            mdo_invocation_delta: None,
            mdo_backedge_delta: None,
            mdo_would_profile: false,
            is_old: false,
            is_trivial: false,
            mdo_mature: false,
        }
    }
}

/// What the policy decided for one event; the broker executes the actions.
#[derive(Clone, Debug, Default)]
pub struct PolicyActions {
    /// Compile with a regular (non-OSR) entry at this level.
    pub call_compile_level: Option<CompLevel>,
    /// OSR-compile at this bci and level.
    pub osr_compile: Option<(CompLevel, i32)>,
    /// Mark the running nmethod not-entrant (OSR invalidation of the
    /// enclosing method, as in method_back_branch_event).
    pub make_not_entrant: bool,
    /// The method's new current level.
    pub next_level: CompLevel,
    /// Create an MDO (profile in the interpreter) before compiling.
    pub create_mdo: bool,
}

/// Per-method policy bookkeeping (rate + achieved levels + timestamps).
#[derive(Clone, Debug)]
struct MethodState {
    highest_comp_level: CompLevel,
    highest_osr_comp_level: CompLevel,
    last_update_ms: i64,
    prev_event_count: u64,
    rate: f64, // events per millisecond
    queued_time_ms: Option<i64>,
}

impl Default for MethodState {
    fn default() -> Self {
        Self {
            highest_comp_level: CompLevel::None,
            highest_osr_comp_level: CompLevel::None,
            last_update_ms: 0,
            prev_event_count: 0,
            rate: 0.0,
            queued_time_ms: None,
        }
    }
}

#[derive(Default)]
struct PolicyState {
    c1_count: i32,
    c2_count: i32,
    start_time_ms: i64,
    increase_threshold_at_ratio: f64,
    methods: HashMap<String, MethodState>,
    compilation_disabled_forever: bool,
}

static POLICY: OnceLock<CompilationPolicy> = OnceLock::new();

pub struct CompilationPolicy {
    thresholds: PolicyThresholds,
    mode: CompilationMode,
    state: Mutex<PolicyState>,
}

#[derive(Clone, Debug)]
pub struct PolicyThresholds {
    pub tier3_invocation_threshold: u32,
    pub tier3_min_invocation_threshold: u32,
    pub tier3_compile_threshold: u32,
    pub tier3_backedge_threshold: u32,
    pub tier4_invocation_threshold: u32,
    pub tier4_min_invocation_threshold: u32,
    pub tier4_compile_threshold: u32,
    pub tier4_backedge_threshold: u32,
    pub tier3_load_feedback: i32,
    pub tier4_load_feedback: i32,
    pub tier3_delay_on: usize,
    pub tier3_delay_off: usize,
    pub tier0_profiling_start_percentage: f64,
    pub tier0_delay: usize,
    pub profile_maturity_percentage: f64,
    pub tiered_stop_at_level: i32,
    pub compile_task_timeout_ms: i64,
    pub rate_update_min_time_ms: i64,
    pub rate_update_max_time_ms: i64,
    pub increase_first_tier_threshold_at: f64,
}

impl Default for PolicyThresholds {
    fn default() -> Self {
        Self {
            tier3_invocation_threshold: TIER3_INVOCATION_THRESHOLD,
            tier3_min_invocation_threshold: TIER3_MIN_INVOCATION_THRESHOLD,
            tier3_compile_threshold: TIER3_COMPILE_THRESHOLD,
            tier3_backedge_threshold: TIER3_BACKEDGE_THRESHOLD,
            tier4_invocation_threshold: TIER4_INVOCATION_THRESHOLD,
            tier4_min_invocation_threshold: TIER4_MIN_INVOCATION_THRESHOLD,
            tier4_compile_threshold: TIER4_COMPILE_THRESHOLD,
            tier4_backedge_threshold: TIER4_BACKEDGE_THRESHOLD,
            tier3_load_feedback: TIER3_LOAD_FEEDBACK,
            tier4_load_feedback: TIER4_LOAD_FEEDBACK,
            tier3_delay_on: TIER3_DELAY_ON,
            tier3_delay_off: TIER3_DELAY_OFF,
            tier0_profiling_start_percentage: TIER0_PROFILING_START_PERCENTAGE,
            tier0_delay: TIER0_DELAY,
            profile_maturity_percentage: PROFILE_MATURITY_PERCENTAGE,
            tiered_stop_at_level: CompLevel::FullOptimization.as_i32(),
            compile_task_timeout_ms: 0,
            rate_update_min_time_ms: TIERED_RATE_UPDATE_MIN_TIME_MS,
            rate_update_max_time_ms: TIERED_RATE_UPDATE_MAX_TIME_MS,
            increase_first_tier_threshold_at: INCREASE_FIRST_TIER_COMPILE_THRESHOLD_AT,
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl CompilationPolicy {
    pub fn new() -> Self {
        let mut state = PolicyState::default();
        state.start_time_ms = now_ms();
        state.increase_threshold_at_ratio =
            100.0 / (100.0 - state_increase_pct_placeholder());
        Self {
            thresholds: PolicyThresholds::default(),
            mode: CompilationMode::Normal,
            state: Mutex::new(state),
        }
    }

    pub fn with_thresholds(thresholds: PolicyThresholds) -> Self {
        let mut p = Self::new();
        p.thresholds = thresholds;
        p
    }

    pub fn with_mode(mut self, mode: CompilationMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn global() -> &'static CompilationPolicy {
        POLICY.get_or_init(Self::new)
    }

    pub fn thresholds(&self) -> &PolicyThresholds {
        &self.thresholds
    }

    pub fn set_thresholds(&mut self, t: PolicyThresholds) {
        self.thresholds = t;
    }

    // ---- compiler counts ------------------------------------------------

    pub fn set_c1_count(&self, x: i32) {
        self.state.lock().unwrap().c1_count = x;
    }

    pub fn set_c2_count(&self, x: i32) {
        self.state.lock().unwrap().c2_count = x;
    }

    pub fn c1_count(&self) -> i32 {
        self.state.lock().unwrap().c1_count
    }

    pub fn c2_count(&self) -> i32 {
        self.state.lock().unwrap().c2_count
    }

    pub fn compiler_count(&self, level: CompLevel) -> i32 {
        if level.is_c2() {
            self.c2_count().max(0)
        } else {
            self.c1_count().max(0)
        }
    }

    // ---- level helpers ---------------------------------------------------

    /// `CompilationPolicy::highest_compile_level`.
    pub fn highest_compile_level(&self) -> CompLevel {
        let mut level = CompilerConfig::highest_available_level();
        if self.thresholds.tiered_stop_at_level >= 0 {
            let stop = CompLevel::from_i32(self.thresholds.tiered_stop_at_level)
                .unwrap_or(CompLevel::FullOptimization);
            if level.as_i32() > stop.as_i32() {
                level = stop;
            }
        }
        // Clamp for non-normal compilation modes.
        level = match self.mode {
            CompilationMode::Normal => level,
            CompilationMode::QuickOnly => {
                if matches!(
                    level,
                    CompLevel::LimitedProfile | CompLevel::FullProfile | CompLevel::FullOptimization
                ) {
                    CompLevel::Simple
                } else {
                    level
                }
            }
            CompilationMode::HighOnly => {
                if matches!(
                    level,
                    CompLevel::Simple | CompLevel::LimitedProfile | CompLevel::FullProfile
                ) {
                    CompLevel::None
                } else {
                    level
                }
            }
            CompilationMode::HighOnlyQuickInternal => {
                if matches!(level, CompLevel::LimitedProfile | CompLevel::FullProfile) {
                    CompLevel::Simple
                } else {
                    level
                }
            }
        };
        level
    }

    /// `CompilationPolicy::limit_level`.
    pub fn limit_level(&self, level: CompLevel) -> CompLevel {
        let highest = self.highest_compile_level();
        if level.as_i32() > highest.as_i32() {
            highest
        } else {
            level
        }
    }

    /// `CompilationPolicy::initial_compile_level`.
    pub fn initial_compile_level(&self, is_trivial: bool) -> CompLevel {
        let level = match self.mode {
            CompilationMode::Normal => CompLevel::FullProfile,
            CompilationMode::QuickOnly => CompLevel::Simple,
            CompilationMode::HighOnly => CompLevel::FullOptimization,
            CompilationMode::HighOnlyQuickInternal => {
                if is_trivial {
                    CompLevel::Simple
                } else {
                    CompLevel::FullOptimization
                }
            }
        };
        self.limit_level(level)
    }

    /// `verify_level` (non-debug version): level within bounds and a
    /// compiler exists for it.
    pub fn verify_level(&self, level: CompLevel) -> bool {
        if level == CompLevel::Any {
            return false;
        }
        if self.thresholds.tiered_stop_at_level >= 0
            && level.as_i32() > self.thresholds.tiered_stop_at_level
        {
            return false;
        }
        match level {
            CompLevel::None | CompLevel::Any => true,
            l if l.is_c1() => CompilerConfig::is_c1_enabled(),
            l if l.is_c2() => CompilerConfig::is_c2_or_jvmci_compiler_enabled(),
            _ => false,
        }
    }

    // ---- predicates --------------------------------------------------------

    /// `threshold_scale`: queue-size feedback scaling factor `s`.
    /// `level` is the *target* queue level (C1 for 0->3, C2 for 3->4).
    /// `queue_size`/`compiler_count` come from the broker; the code-cache
    /// pressure factor uses `code_cache_reverse_free_ratio`.
    pub fn threshold_scale(&self, level: CompLevel, feedback_k: i32, queue_size: usize) -> f64 {
        let comp_count = self.compiler_count(level);
        if comp_count > 0 {
            let queue_size = queue_size as f64;
            let k = queue_size / (feedback_k as f64 * comp_count as f64) + 1.0;
            // Increase C1 thresholds when the code cache is nearly full.
            if CompilerConfig::is_tiered()
                && !self.mode.disable_intermediate()
                && level.is_c1()
            {
                let reverse_free = code_cache_reverse_free_ratio();
                let ratio = self.state.lock().unwrap().increase_threshold_at_ratio;
                if reverse_free > ratio {
                    return k * (reverse_free - ratio).exp();
                }
            }
            k
        } else {
            1.0
        }
    }

    /// `LoopPredicate::apply_scaled`.
    pub fn loop_predicate_scaled(&self, cur_level: CompLevel, _i: u32, b: u32, scale: f64) -> bool {
        let t = &self.thresholds;
        match cur_level {
            CompLevel::None | CompLevel::LimitedProfile => {
                (b as f64) >= t.tier3_backedge_threshold as f64 * scale
            }
            CompLevel::FullProfile => (b as f64) >= t.tier4_backedge_threshold as f64 * scale,
            _ => true,
        }
    }

    /// `CallPredicate::apply_scaled`.
    pub fn call_predicate_scaled(&self, cur_level: CompLevel, i: u32, b: u32, scale: f64) -> bool {
        let t = &self.thresholds;
        match cur_level {
            CompLevel::None | CompLevel::LimitedProfile => {
                (i as f64) >= t.tier3_invocation_threshold as f64 * scale
                    || ((i as f64) >= t.tier3_min_invocation_threshold as f64 * scale
                        && (i.saturating_add(b)) as f64 >= t.tier3_compile_threshold as f64 * scale)
            }
            CompLevel::FullProfile => {
                (i as f64) >= t.tier4_invocation_threshold as f64 * scale
                    || ((i as f64) >= t.tier4_min_invocation_threshold as f64 * scale
                        && (i.saturating_add(b)) as f64 >= t.tier4_compile_threshold as f64 * scale)
            }
            _ => true,
        }
    }

    /// `LoopPredicate::apply` (with queue-load feedback `k`).
    pub fn loop_predicate(&self, cur_level: CompLevel, i: u32, b: u32, queue_size_c1: usize, queue_size_c2: usize) -> bool {
        let k = match cur_level {
            CompLevel::None | CompLevel::LimitedProfile => {
                self.threshold_scale(CompLevel::FullProfile, self.thresholds.tier3_load_feedback, queue_size_c1)
            }
            CompLevel::FullProfile => {
                self.threshold_scale(CompLevel::FullOptimization, self.thresholds.tier4_load_feedback, queue_size_c2)
            }
            _ => return true,
        };
        self.loop_predicate_scaled(cur_level, i, b, k)
    }

    /// `CallPredicate::apply`.
    pub fn call_predicate(&self, cur_level: CompLevel, i: u32, b: u32, queue_size_c1: usize, queue_size_c2: usize) -> bool {
        let k = match cur_level {
            CompLevel::None | CompLevel::LimitedProfile => {
                self.threshold_scale(CompLevel::FullProfile, self.thresholds.tier3_load_feedback, queue_size_c1)
            }
            CompLevel::FullProfile => {
                self.threshold_scale(CompLevel::FullOptimization, self.thresholds.tier4_load_feedback, queue_size_c2)
            }
            _ => return true,
        };
        self.call_predicate_scaled(cur_level, i, b, k)
    }

    // ---- transitions ------------------------------------------------------

    /// `transition_from_none`: 0 -> 2/3/4 decision with Tier3Delay logic.
    fn transition_from_none(
        &self,
        profile: &MethodProfile,
        delay_profiling: bool,
        disable_feedback: bool,
        queue_size_c1: usize,
        queue_size_c2: usize,
        oracle_scale: Option<f64>,
    ) -> CompLevel {
        let i = profile.invocation_count;
        let b = profile.backedge_count;

        // If we were at full profile level, would we go to full opt?
        if self.transition_from_full_profile_inner(profile, i, b, queue_size_c1, queue_size_c2, oracle_scale)
            == CompLevel::FullOptimization
        {
            return CompLevel::FullOptimization;
        }

        // Apply predicate (delay_profiling uses Tier0ProfileDelayFactor=1.0
        // scaled path in HotSpot; here delay_profiling lowers thresholds).
        let scale = if delay_profiling { 1.0 } else { 1.0 };
        let mut hit = self.call_predicate_scaled(CompLevel::None, i, b, scale * oracle_scale.unwrap_or(1.0))
            || self.loop_predicate_scaled(CompLevel::None, i, b, scale * oracle_scale.unwrap_or(1.0));
        // With load feedback when not delaying:
        if !delay_profiling {
            hit = self.call_predicate(CompLevel::None, i, b, queue_size_c1, queue_size_c2)
                || self.loop_predicate(CompLevel::None, i, b, queue_size_c1, queue_size_c2);
        }

        if !self.mode.disable_intermediate() && hit {
            // Tier3Delay: if the C2 queue is long, prefer limited_profile.
            let c2_compilers = self.compiler_count(CompLevel::FullOptimization).max(1) as usize;
            let c2_queue = queue_size_c2;
            if delay_profiling
                || (!disable_feedback && c2_queue > self.thresholds.tier3_delay_on * c2_compilers)
            {
                CompLevel::LimitedProfile
            } else {
                CompLevel::FullProfile
            }
        } else {
            CompLevel::None
        }
    }

    /// Inner 3->4 predicate used both directly and from transition_from_none.
    fn transition_from_full_profile_inner(
        &self,
        profile: &MethodProfile,
        i: u32,
        b: u32,
        _queue_size_c1: usize,
        queue_size_c2: usize,
        _oracle_scale: Option<f64>,
    ) -> CompLevel {
        // MDO deltas when present; otherwise raw counters.
        let (mi, mb) = match (profile.mdo_invocation_delta, profile.mdo_backedge_delta) {
            (Some(di), Some(db)) => (di, db),
            _ => (i, b),
        };
        if profile.mdo_would_profile || self.mode.disable_intermediate() {
            if self.call_predicate(CompLevel::FullProfile, mi, mb, 0, queue_size_c2)
                || self.loop_predicate(CompLevel::FullProfile, mi, mb, 0, queue_size_c2)
            {
                CompLevel::FullOptimization
            } else {
                CompLevel::FullProfile
            }
        } else {
            CompLevel::FullOptimization
        }
    }

    /// `transition_from_full_profile` (3 -> 4).
    fn transition_from_full_profile(&self, profile: &MethodProfile, queue_size_c1: usize, queue_size_c2: usize) -> CompLevel {
        self.transition_from_full_profile_inner(
            profile,
            profile.invocation_count,
            profile.backedge_count,
            queue_size_c1,
            queue_size_c2,
            None,
        )
    }

    /// `transition_from_limited_profile` (2 -> 3/4).
    fn transition_from_limited_profile(
        &self,
        profile: &MethodProfile,
        delay_profiling: bool,
        disable_feedback: bool,
        queue_size_c1: usize,
        queue_size_c2: usize,
    ) -> CompLevel {
        let i = profile.invocation_count;
        let b = profile.backedge_count;
        let mut next = CompLevel::LimitedProfile;

        let c2_compilers = self.compiler_count(CompLevel::FullOptimization).max(1) as usize;
        let c2_queue_ok = queue_size_c2 <= self.thresholds.tier3_delay_off * c2_compilers;

        // C++: mdo && would_profile -> maybe full_profile;
        //      mdo && !would_profile -> full_optimization;
        //      no mdo -> must profile first (full_profile) when queue allows.
        let profile_cond = disable_feedback
            || (c2_queue_ok
                && (self.call_predicate_scaled(CompLevel::LimitedProfile, i, b, 1.0)
                    || self.loop_predicate_scaled(CompLevel::LimitedProfile, i, b, 1.0)));

        if profile.mdo_would_profile {
            if profile_cond {
                next = CompLevel::FullProfile;
            }
        } else if profile.mdo_invocation_delta.is_some() {
            // MDO exists but nothing worth profiling: skip to C2.
            next = CompLevel::FullOptimization;
        } else if profile_cond {
            next = CompLevel::FullProfile;
        }

        // If already profiled at level-3 quality, jump straight to C2.
        if next == CompLevel::FullProfile
            && self.is_method_profiled(profile, queue_size_c1, queue_size_c2)
        {
            next = CompLevel::FullOptimization;
        }
        next
    }

    /// `standard_transition`.
    fn standard_transition(
        &self,
        profile: &MethodProfile,
        cur_level: CompLevel,
        delay_profiling: bool,
        disable_feedback: bool,
        queue_size_c1: usize,
        queue_size_c2: usize,
        oracle_scale: Option<f64>,
    ) -> CompLevel {
        match cur_level {
            CompLevel::None => self.transition_from_none(
                profile,
                delay_profiling,
                disable_feedback,
                queue_size_c1,
                queue_size_c2,
                oracle_scale,
            ),
            CompLevel::LimitedProfile => self.transition_from_limited_profile(
                profile,
                delay_profiling,
                disable_feedback,
                queue_size_c1,
                queue_size_c2,
            ),
            CompLevel::FullProfile => self.transition_from_full_profile(profile, queue_size_c1, queue_size_c2),
            _ => cur_level,
        }
    }

    /// `common`: entry point for call/loop events.  Applies trivial-method
    /// shortcut, mode handling and level limiting.
    pub fn common(
        &self,
        profile: &MethodProfile,
        cur_level: CompLevel,
        is_trivial: bool,
        is_native: bool,
        queue_size_c1: usize,
        queue_size_c2: usize,
        oracle_scale: Option<f64>,
        disable_feedback: bool,
    ) -> CompLevel {
        let next = if profile.is_trivial || is_trivial || is_native {
            if self.mode.disable_intermediate() {
                CompLevel::FullOptimization
            } else {
                CompLevel::Simple
            }
        } else {
            self.standard_transition(
                profile,
                cur_level,
                false,
                disable_feedback,
                queue_size_c1,
                queue_size_c2,
                oracle_scale,
            )
        };
        if next != cur_level {
            self.limit_level(next)
        } else {
            next
        }
    }

    // ---- events ------------------------------------------------------------

    /// `CompilationPolicy::call_event`: decide next level for a method
    /// invocation, including OSR-level equalization.
    pub fn call_event(
        &self,
        key: &str,
        profile: &MethodProfile,
        cur_level: CompLevel,
        queue_size_c1: usize,
        queue_size_c2: usize,
        oracle_scale: Option<f64>,
    ) -> CompLevel {
        let osr_level = std::cmp::min(
            self.osr_level_of(key).min(self.highest_compile_level()),
            self.common(
                profile,
                cur_level,
                profile.is_trivial,
                false,
                queue_size_c1,
                queue_size_c2,
                oracle_scale,
                true, // disable_feedback for the OSR sub-check
            ),
        );
        let mut next_level = self.common(
            profile,
            cur_level,
            profile.is_trivial,
            false,
            queue_size_c1,
            queue_size_c2,
            oracle_scale,
            profile.is_old,
        );

        // Equalize levels when an OSR'd full-opt method exists.
        if osr_level == CompLevel::FullOptimization && cur_level == CompLevel::FullProfile {
            if profile.invocation_count >= 1 {
                next_level = CompLevel::FullOptimization;
            }
        } else {
            next_level = std::cmp::max(osr_level, next_level);
        }
        next_level
    }

    /// `CompilationPolicy::loop_event`: decide OSR level at a back-edge.
    pub fn loop_event(
        &self,
        key: &str,
        profile: &MethodProfile,
        cur_level: CompLevel,
        queue_size_c1: usize,
        queue_size_c2: usize,
    ) -> CompLevel {
        let mut next_level = self.common(
            profile,
            cur_level,
            profile.is_trivial,
            false,
            queue_size_c1,
            queue_size_c2,
            None,
            true,
        );
        if cur_level == CompLevel::None {
            let osr_level = std::cmp::min(self.osr_level_of(key), next_level);
            if osr_level > CompLevel::None {
                next_level = osr_level;
            }
        }
        next_level
    }

    /// `method_invocation_event`: full invocation-event handling —
    /// returns actions for the broker to execute.
    pub fn method_invocation_event(
        &self,
        key: &str,
        profile: &MethodProfile,
        cur_level: CompLevel,
        queue_size_c1: usize,
        queue_size_c2: usize,
        oracle_scale: Option<f64>,
        is_trivial: bool,
    ) -> PolicyActions {
        let mut actions = PolicyActions {
            next_level: cur_level,
            create_mdo: self.should_create_mdo(profile, cur_level, queue_size_c2),
            ..Default::default()
        };
        let next = self.call_event(key, profile, cur_level, queue_size_c1, queue_size_c2, oracle_scale);
        actions.next_level = next;
        if next != cur_level && next.is_compile() {
            actions.call_compile_level = Some(next);
        }
        actions
    }

    /// `method_back_branch_event`: OSR compile decision plus the
    /// enclosing-method invalidation logic.
    /// `enclosing` is `Some((enclosing_key, enclosing_level, osr_method))`
    /// when the back-edge happens inside an inlined method.
    #[allow(clippy::too_many_arguments)]
    pub fn method_back_branch_event(
        &self,
        key: &str,
        profile: &MethodProfile,
        bci: i32,
        cur_level: CompLevel,
        queue_size_c1: usize,
        queue_size_c2: usize,
        is_trivial: bool,
        enclosing: Option<(&str, CompLevel, bool)>, // (key, level, is_osr_nmethod)
        max_osr_level: CompLevel,
    ) -> PolicyActions {
        let mut actions = PolicyActions {
            next_level: cur_level,
            create_mdo: self.should_create_mdo(profile, cur_level, queue_size_c2),
            ..Default::default()
        };

        let next_osr_level = self.loop_event(key, profile, cur_level, queue_size_c1, queue_size_c2);
        // At the very least compile the OSR version.
        if next_osr_level != cur_level && next_osr_level.is_compile() {
            actions.osr_compile = Some((next_osr_level, bci));
        }

        match enclosing {
            Some((ekey, elevel, is_osr_nm)) => {
                let eprofile = MethodProfile {
                    highest_osr_comp_level: max_osr_level,
                    ..MethodProfile::default()
                };
                let next_level = self.call_event(
                    ekey,
                    &eprofile,
                    elevel,
                    queue_size_c1,
                    queue_size_c2,
                    None,
                );
                let mut make_not_entrant = false;
                if max_osr_level == CompLevel::FullOptimization {
                    if is_osr_nm {
                        make_not_entrant = true;
                    } else if next_level != CompLevel::FullOptimization {
                        make_not_entrant = true;
                    }
                }
                actions.make_not_entrant = make_not_entrant;
                let mut fixed = next_level;
                if fixed == CompLevel::LimitedProfile && max_osr_level == CompLevel::FullProfile {
                    fixed = CompLevel::FullProfile;
                }
                if fixed != elevel && fixed.is_compile() {
                    actions.call_compile_level = Some(fixed);
                }
                actions.next_level = fixed;
            }
            None => {
                let next_level = self.call_event(key, profile, cur_level, queue_size_c1, queue_size_c2, None);
                if next_level != cur_level && next_level.is_compile() {
                    actions.call_compile_level = Some(next_level);
                }
                actions.next_level = next_level;
            }
        }
        let _ = is_trivial;
        actions
    }

    // ---- mdo / profiling decisions ----------------------------------------

    /// `should_create_mdo`: start profiling in the interpreter when the
    /// method is hot but the level-3 compile hasn't arrived.
    pub fn should_create_mdo(&self, profile: &MethodProfile, cur_level: CompLevel, queue_size_c2: usize) -> bool {
        if cur_level != CompLevel::None || self.mode.quick_only() {
            return false;
        }
        if profile.is_old {
            return true;
        }
        let i = profile.invocation_count;
        let b = profile.backedge_count;
        let k = self.thresholds.tier0_profiling_start_percentage / 100.0;
        let c2_compilers = self.compiler_count(CompLevel::FullOptimization).max(1) as usize;
        if (queue_size_c2 as usize) <= self.thresholds.tier0_delay * c2_compilers {
            return self.call_predicate_scaled(CompLevel::None, i, b, k)
                || self.loop_predicate_scaled(CompLevel::None, i, b, k);
        }
        false
    }

    /// `is_mature`: MDO has accumulated enough profile to drive C2.
    pub fn is_mature(&self, profile: &MethodProfile) -> bool {
        if let (Some(i), Some(b)) = (profile.mdo_invocation_delta, profile.mdo_backedge_delta) {
            let k = self.thresholds.profile_maturity_percentage / 100.0;
            return self.call_predicate_scaled(CompLevel::FullProfile, i, b, k)
                || self.loop_predicate_scaled(CompLevel::FullProfile, i, b, k);
        }
        false
    }

    /// `is_method_profiled`: level-3 code already collected enough profile
    /// to justify skipping straight to C2.
    pub fn is_method_profiled(&self, profile: &MethodProfile, _q1: usize, _q2: usize) -> bool {
        if let (Some(i), Some(b)) = (profile.mdo_invocation_delta, profile.mdo_backedge_delta) {
            return self.call_predicate_scaled(CompLevel::FullProfile, i, b, 1.0)
                || self.loop_predicate_scaled(CompLevel::FullProfile, i, b, 1.0);
        }
        // Without MDO deltas, use the raw counters vs tier-4 thresholds.
        self.call_predicate_scaled(
            CompLevel::FullProfile,
            profile.invocation_count,
            profile.backedge_count,
            1.0,
        ) || self.loop_predicate_scaled(
            CompLevel::FullProfile,
            profile.invocation_count,
            profile.backedge_count,
            1.0,
        )
    }

    // ---- compilability -----------------------------------------------------

    /// `can_be_compiled`: method allowed at `comp_level`.
    pub fn can_be_compiled(&self, is_abstract: bool, is_native: bool, not_compilable: bool, comp_level: CompLevel) -> bool {
        if is_abstract {
            return false;
        }
        if is_native {
            // Native methods are compilable only as wrappers; policy treats
            // them as always compilable at any level (broker filters).
        }
        let level = if comp_level == CompLevel::Any {
            CompLevel::FullProfile
        } else {
            comp_level
        };
        if comp_level == CompLevel::Any || level.is_compile() {
            !not_compilable
        } else {
            false
        }
    }

    /// `can_be_osr_compiled`.
    pub fn can_be_osr_compiled(&self, is_abstract: bool, is_native: bool, not_osr_compilable: bool, comp_level: CompLevel) -> bool {
        !is_abstract && !is_native && !not_osr_compilable && self.can_be_compiled(is_abstract, is_native, false, comp_level)
    }

    /// `must_be_compiled`: -Xcomp / AlwaysCompileLoopMethods style forcing.
    pub fn must_be_compiled(&self, force_compile: bool, comp_level: CompLevel) -> bool {
        force_compile || comp_level.is_compile()
    }

    /// `is_trivial`: accessor/constant-getter methods compile identically
    /// at C1 and C2.
    pub fn is_trivial(&self, is_accessor: bool, is_constant_getter: bool) -> bool {
        is_accessor || is_constant_getter
    }

    // ---- rate / staleness --------------------------------------------------

    /// `update_rate`: maintain events-per-ms rate for a method.
    pub fn update_rate(&self, key: &str, event_count: u64) {
        let mut st = self.state.lock().unwrap();
        let m = st.methods.entry(key.to_string()).or_default();
        let t = now_ms();
        if m.last_update_ms > 0 && t > m.last_update_ms {
            let elapsed = (t - m.last_update_ms) as f64;
            if elapsed >= self.thresholds.rate_update_min_time_ms as f64 {
                let delta = event_count.saturating_sub(m.prev_event_count) as f64;
                m.rate = delta / elapsed;
                m.last_update_ms = t;
                m.prev_event_count = event_count;
            }
        } else {
            m.last_update_ms = t;
            m.prev_event_count = event_count;
        }
    }

    /// `weight`: scheduling weight of a queued method (event rate).
    pub fn weight(&self, key: &str) -> f64 {
        self.state
            .lock()
            .unwrap()
            .methods
            .get(key)
            .map(|m| m.rate)
            .unwrap_or(0.0)
    }

    /// `is_old`: method has been around longer than start_time grace.
    pub fn is_old(&self, key: &str) -> bool {
        let st = self.state.lock().unwrap();
        st.methods
            .get(key)
            .map(|m| m.queued_time_ms.map_or(false, |q| now_ms() - q > 60_000))
            .unwrap_or(false)
    }

    /// `is_stale`: no events for longer than the timeout.
    pub fn is_stale(&self, key: &str) -> bool {
        let st = self.state.lock().unwrap();
        if let Some(m) = st.methods.get(key) {
            if m.last_update_ms == 0 {
                return false;
            }
            let timeout = if self.thresholds.compile_task_timeout_ms > 0 {
                self.thresholds.compile_task_timeout_ms
            } else {
                60_000
            };
            now_ms() - m.last_update_ms > timeout
        } else {
            false
        }
    }

    /// Record a compile-queue insertion time for staleness tracking.
    pub fn note_queued(&self, key: &str) {
        let mut st = self.state.lock().unwrap();
        let m = st.methods.entry(key.to_string()).or_default();
        if m.queued_time_ms.is_none() {
            m.queued_time_ms = Some(now_ms());
        }
    }

    pub fn note_completed(&self, key: &str) {
        let mut st = self.state.lock().unwrap();
        if let Some(m) = st.methods.get_mut(key) {
            m.queued_time_ms = None;
        }
    }

    // ---- achieved levels ----------------------------------------------------

    pub fn level_of(&self, key: &str) -> CompLevel {
        self.state
            .lock()
            .unwrap()
            .methods
            .get(key)
            .map(|m| m.highest_comp_level)
            .unwrap_or(CompLevel::None)
    }

    pub fn osr_level_of(&self, key: &str) -> CompLevel {
        self.state
            .lock()
            .unwrap()
            .methods
            .get(key)
            .map(|m| m.highest_osr_comp_level)
            .unwrap_or(CompLevel::None)
    }

    /// `comp_level(Method*)`: current level of the installed nmethod.
    /// The broker maintains authoritative levels; this reads policy's
    /// record which is updated on successful installs.
    pub fn comp_level(&self, key: &str) -> CompLevel {
        self.level_of(key)
    }

    /// Record that `key` reached `level` (regular or OSR).
    pub fn note_level(&self, key: &str, level: CompLevel, is_osr: bool) {
        let mut st = self.state.lock().unwrap();
        let m = st.methods.entry(key.to_string()).or_default();
        if is_osr {
            if level.as_i32() > m.highest_osr_comp_level.as_i32() {
                m.highest_osr_comp_level = level;
            }
        } else if level.as_i32() > m.highest_comp_level.as_i32() {
            m.highest_comp_level = level;
        }
    }

    /// Merge a MethodCounters snapshot's highest levels into policy state.
    pub fn sync_from_counters(&self, key: &str, counters: &MethodCounters) {
        let lvl = CompLevel::from_i32(counters.highest_comp_level() as i32).unwrap_or(CompLevel::None);
        let osr = CompLevel::from_i32(counters.highest_osr_comp_level() as i32).unwrap_or(CompLevel::None);
        self.note_level(key, lvl, false);
        self.note_level(key, osr, true);
    }

    // ---- queue selection -----------------------------------------------------

    /// `CompilationPolicy::select_task`: pick the highest-rate non-stale
    /// task from the queue; purge stale/unloaded tasks as a side effect.
    /// Blocking whitebox tasks are returned in order (JVMCI behaviour).
    pub fn select_task(&self, queue: &CompileQueue) -> Option<CompileTask> {
        // Purge unloaded/stale tasks first (remove_and_mark_stale analogue).
        let tasks = queue.snapshot();
        for task in &tasks {
            if task.is_unloaded() || (task.can_become_stale() && self.is_stale(task.method_key())) {
                queue.remove_and_mark_stale(task.compile_id());
            }
        }
        let tasks = queue.snapshot();
        // Whitebox blocking tasks first, in FIFO order.
        for task in &tasks {
            if task.is_blocking() && task.reason() == super::compileTask::CompileReason::ReasonWhitebox {
                return Some(task.clone());
            }
        }
        // Otherwise pick max weight (rate).
        let mut best: Option<(f64, CompileTask)> = None;
        for task in &tasks {
            let w = self.weight(task.method_key());
            if best.as_ref().map_or(true, |(bw, _)| w > *bw) {
                best = Some((w, task.clone()));
            }
        }
        best.map(|(_, t)| t)
    }

    /// `is_compilation_enabled`: broker decides; policy exposes flag.
    pub fn is_compilation_enabled(&self) -> bool {
        !self.state.lock().unwrap().compilation_disabled_forever
    }

    pub fn set_compilation_disabled_forever(&self, v: bool) {
        self.state.lock().unwrap().compilation_disabled_forever = v;
    }

    // ---- oracle scaling -----------------------------------------------------

    /// `CompileThresholdScaling` option lookup for a method triple.
    pub fn oracle_threshold_scaling(
        &self,
        class_name: &Symbol,
        method_name: &Symbol,
        descriptor: &Symbol,
    ) -> Option<f64> {
        CompilerOracle::global()
            .read()
            .ok()
            .and_then(|o| {
                o.option_value_f64(
                    class_name.as_str(),
                    method_name.as_str(),
                    descriptor.as_str(),
                    CompileCommandEnum::CompileThresholdScaling,
                )
            })
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let st = self.state.lock().unwrap();
        let t = &self.thresholds;
        let _ = writeln!(
            f,
            "CompilationPolicy: mode={:?} c1={} c2={} methods_tracked={} t3(inv={},min={},cmp={},be={}) t4(inv={},min={},cmp={},be={})",
            self.mode,
            st.c1_count,
            st.c2_count,
            st.methods.len(),
            t.tier3_invocation_threshold,
            t.tier3_min_invocation_threshold,
            t.tier3_compile_threshold,
            t.tier3_backedge_threshold,
            t.tier4_invocation_threshold,
            t.tier4_min_invocation_threshold,
            t.tier4_compile_threshold,
            t.tier4_backedge_threshold,
        );
    }
}

impl Default for CompilationPolicy {
    fn default() -> Self {
        Self::new()
    }
}

fn state_increase_pct_placeholder() -> f64 {
    INCREASE_FIRST_TIER_COMPILE_THRESHOLD_AT
}

/// `CodeCache::reverse_free_ratio` analogue: used by threshold_scale to
/// inflate C1 thresholds under code-cache pressure.  Reads the global
/// code cache if present.
pub fn code_cache_reverse_free_ratio() -> f64 {
    let cache = crate::share::code::codeCache::CodeCache::global();
    let capacity = cache.total_capacity();
    if capacity == 0 {
        return 0.0;
    }
    let free = cache.total_free();
    // reverse_free_ratio = used / capacity * 100  (HotSpot returns % used)
    ((capacity - free) as f64 / capacity as f64) * 100.0
}

/// Queue descriptor for policy scaling (avoids hard dependency cycle at
/// type level; CompileQueue is defined in compileBroker).
pub trait QueueSizes {
    fn c1_size(&self) -> usize;
    fn c2_size(&self) -> usize;
}
