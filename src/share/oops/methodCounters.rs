/*
 * Port of OpenJDK 25 HotSpot MethodCounters (share/oops/methodCounters.hpp, methodCounters.cpp)
 *
 * MethodCounters maintains execution metrics for tiered compilation:
 * - Invocation counter (how many times method has been entered)
 * - Backedge counter (how many backward loops executed)
 * - Highest compilation level achieved
 * - Rate of event execution per millisecond
 * - Threshold invocation and backedge masks
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InvocationCounter {
    count: u32,
    carry: bool,
}

impl InvocationCounter {
    pub const COUNT_MASK: u32 = 0x7FFF_FFFF;
    /// HotSpot `InvocationCounter::number_of_noncount_bits`: the low bit
    /// is reserved for the carry flag, the count occupies the high 31
    /// bits (`count_shift` = 1).
    pub const NUMBER_OF_NONCOUNT_BITS: u32 = 1;
    pub const COUNT_SHIFT: u32 = Self::NUMBER_OF_NONCOUNT_BITS;

    /// Raw counter word including the carry flag in the low bit, as
    /// stored in the metaspace object.
    #[inline]
    pub fn raw_value(&self) -> u32 {
        (self.count << Self::COUNT_SHIFT) | u32::from(self.carry)
    }

    /// Reconstructs from a raw counter word.
    #[inline]
    pub fn from_raw_value(raw: u32) -> Self {
        Self {
            count: (raw >> Self::COUNT_SHIFT) & Self::COUNT_MASK,
            carry: (raw & 1) != 0,
        }
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.count == 0 && !self.carry
    }

    pub fn new() -> Self {
        Self {
            count: 0,
            carry: false,
        }
    }

    #[inline]
    pub fn count(&self) -> u32 {
        self.count
    }

    #[inline]
    pub fn increment(&mut self, step: u32) -> u32 {
        let (new_count, overflow) = self.count.overflowing_add(step);
        if overflow {
            self.carry = true;
            self.count = Self::COUNT_MASK;
        } else {
            self.count = new_count;
        }
        self.count
    }

    #[inline]
    pub fn reset(&mut self) {
        self.count = 0;
        self.carry = false;
    }

    #[inline]
    pub fn carry(&self) -> bool {
        self.carry
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodCounters {
    invocation_counter: InvocationCounter,
    backedge_counter: InvocationCounter,
    highest_comp_level: u8,
    highest_osr_comp_level: u8,
    rate: f32,
    prev_time_ms: i64,
    prev_event_count: u32,
    interpreter_throwout_count: u16,
    invoke_mask: u32,
    backedge_mask: u32,
    /// MDO-side interpreter counters (HotSpot tracks these in
    /// MethodData; kept here so compile policy can read one snapshot).
    interpreter_invocation_count: u32,
    interpreter_backedge_count: u32,
    /// Bitmask of compilation levels currently locked by the compiler
    /// (`_compiler_locks` in earlier HotSpot releases; used to veto
    /// recompilation while a compile is in flight).
    compiler_locks: u32,
    /// JVMTI breakpoint count (`_number_of_breakpoints`).
    number_of_breakpoints: u16,
}

impl MethodCounters {
    pub const DEFAULT_INVOCATION_THRESHOLD: u32 = 1000;
    pub const DEFAULT_BACKEDGE_THRESHOLD: u32 = 5000;

    pub fn new() -> Self {
        Self {
            invocation_counter: InvocationCounter::new(),
            backedge_counter: InvocationCounter::new(),
            highest_comp_level: 0,
            highest_osr_comp_level: 0,
            rate: 0.0,
            prev_time_ms: 0,
            prev_event_count: 0,
            interpreter_throwout_count: 0,
            invoke_mask: 0,
            backedge_mask: 0,
            interpreter_invocation_count: 0,
            interpreter_backedge_count: 0,
            compiler_locks: 0,
            number_of_breakpoints: 0,
        }
    }

    #[inline]
    pub fn invocation_counter(&self) -> &InvocationCounter {
        &self.invocation_counter
    }

    #[inline]
    pub fn invocation_count(&self) -> u32 {
        self.invocation_counter.count()
    }

    #[inline]
    pub fn backedge_counter(&self) -> &InvocationCounter {
        &self.backedge_counter
    }

    #[inline]
    pub fn backedge_count(&self) -> u32 {
        self.backedge_counter.count()
    }

    #[inline]
    pub fn increment_invocation_counter(&mut self, step: u32) -> u32 {
        self.invocation_counter.increment(step)
    }

    #[inline]
    pub fn increment_backedge_counter(&mut self, step: u32) -> u32 {
        self.backedge_counter.increment(step)
    }

    #[inline]
    pub fn highest_comp_level(&self) -> u8 {
        self.highest_comp_level
    }

    #[inline]
    pub fn set_highest_comp_level(&mut self, level: u8) {
        if level > self.highest_comp_level {
            self.highest_comp_level = level;
        }
    }

    #[inline]
    pub fn highest_osr_comp_level(&self) -> u8 {
        self.highest_osr_comp_level
    }

    #[inline]
    pub fn set_highest_osr_comp_level(&mut self, level: u8) {
        if level > self.highest_osr_comp_level {
            self.highest_osr_comp_level = level;
        }
    }

    #[inline]
    pub fn interpreter_throwout_count(&self) -> u16 {
        self.interpreter_throwout_count
    }

    #[inline]
    pub fn set_interpreter_throwout_count(&mut self, count: u16) {
        self.interpreter_throwout_count = count;
    }

    #[inline]
    pub fn increment_throwout_count(&mut self) {
        self.interpreter_throwout_count = self.interpreter_throwout_count.saturating_add(1);
    }

    #[inline]
    pub fn invoke_mask(&self) -> u32 {
        self.invoke_mask
    }

    #[inline]
    pub fn set_invoke_mask(&mut self, mask: u32) {
        self.invoke_mask = mask;
    }

    #[inline]
    pub fn backedge_mask(&self) -> u32 {
        self.backedge_mask
    }

    #[inline]
    pub fn set_backedge_mask(&mut self, mask: u32) {
        self.backedge_mask = mask;
    }

    // ---- Interpreter counters (MethodData-side in HotSpot) ----

    #[inline]
    pub fn interpreter_invocation_count(&self) -> u32 {
        self.interpreter_invocation_count
    }

    #[inline]
    pub fn interpreter_backedge_count(&self) -> u32 {
        self.interpreter_backedge_count
    }

    #[inline]
    pub fn increment_interpreter_invocation_count(&mut self) {
        self.interpreter_invocation_count =
            self.interpreter_invocation_count.saturating_add(1);
    }

    #[inline]
    pub fn increment_interpreter_backedge_count(&mut self) {
        self.interpreter_backedge_count =
            self.interpreter_backedge_count.saturating_add(1);
    }

    #[inline]
    pub fn set_interpreter_invocation_count(&mut self, count: u32) {
        self.interpreter_invocation_count = count;
    }

    #[inline]
    pub fn set_interpreter_backedge_count(&mut self, count: u32) {
        self.interpreter_backedge_count = count;
    }

    // ---- Compiler locks ----

    /// HotSpot `compiler_locks()`: bitmask of locked compilation levels.
    #[inline]
    pub fn compiler_locks(&self) -> u32 {
        self.compiler_locks
    }

    /// HotSpot `has_compiler_locks()`.
    #[inline]
    pub fn has_compiler_locks(&self) -> bool {
        self.compiler_locks != 0
    }

    /// Locks the given compilation level (bit position = level).
    #[inline]
    pub fn lock_compiler(&mut self, level: u8) {
        self.compiler_locks |= 1u32 << level.min(31);
    }

    /// Unlocks the given compilation level.
    #[inline]
    pub fn unlock_compiler(&mut self, level: u8) {
        self.compiler_locks &= !(1u32 << level.min(31));
    }

    /// Whether the given compilation level is locked.
    #[inline]
    pub fn is_compiler_locked(&self, level: u8) -> bool {
        (self.compiler_locks & (1u32 << level.min(31))) != 0
    }

    // ---- JVMTI breakpoints ----

    #[inline]
    pub fn number_of_breakpoints(&self) -> u16 {
        self.number_of_breakpoints
    }

    #[inline]
    pub fn set_number_of_breakpoints(&mut self, count: u16) {
        self.number_of_breakpoints = count;
    }

    #[inline]
    pub fn increment_breakpoint_count(&mut self) {
        self.number_of_breakpoints = self.number_of_breakpoints.saturating_add(1);
    }

    #[inline]
    pub fn should_compile(&self, threshold: u32) -> bool {
        self.invocation_count() >= threshold
    }

    #[inline]
    pub fn should_osr_compile(&self, threshold: u32) -> bool {
        self.backedge_count() >= threshold
    }

    pub fn update_rate(&mut self, current_time_ms: i64) {
        let total_events = self
            .invocation_count()
            .saturating_add(self.backedge_count());
        if self.prev_time_ms > 0 && current_time_ms > self.prev_time_ms {
            let elapsed = (current_time_ms - self.prev_time_ms) as f32;
            let delta_events = total_events.saturating_sub(self.prev_event_count) as f32;
            self.rate = delta_events / elapsed;
        }
        self.prev_time_ms = current_time_ms;
        self.prev_event_count = total_events;
    }

    #[inline]
    pub fn rate(&self) -> f32 {
        self.rate
    }

    #[inline]
    pub fn clear_counters(&mut self) {
        self.invocation_counter.reset();
        self.backedge_counter.reset();
    }

    pub fn reset(&mut self) {
        self.clear_counters();
        self.highest_comp_level = 0;
        self.highest_osr_comp_level = 0;
        self.rate = 0.0;
        self.prev_time_ms = 0;
        self.prev_event_count = 0;
        self.interpreter_throwout_count = 0;
        self.interpreter_invocation_count = 0;
        self.interpreter_backedge_count = 0;
        self.compiler_locks = 0;
        self.number_of_breakpoints = 0;
    }
}

impl Default for MethodCounters {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Live per-method counters keyed by the identity of the method's `Code`
// bytes (`Code::bytes.as_ptr()`). Every interpreted call funnels through
// `bytecodeInterpreter::execute_labeled`, so the code pointer is a stable,
// unique key for the lifetime of the loader's bump storage.
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static LIVE_COUNTERS: OnceLock<Mutex<HashMap<usize, MethodCounters>>> = OnceLock::new();
/// Methods whose counter has already triggered a compile request; prevents
/// re-enqueueing on every subsequent tick.
static HOT_METHODS: OnceLock<Mutex<std::collections::HashSet<usize>>> = OnceLock::new();

fn counters() -> &'static Mutex<HashMap<usize, MethodCounters>> {
    LIVE_COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn hot() -> &'static Mutex<std::collections::HashSet<usize>> {
    HOT_METHODS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Compile threshold for the interpreter -> JIT transition. Mirrors
/// `Tier3InvocationThreshold`-style behaviour with a conservative default;
/// `JVM25_C1_THRESHOLD` overrides it for tuning.
pub fn compile_threshold() -> u32 {
    static THRESHOLD: OnceLock<u32> = OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var("JVM25_C1_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000)
    })
}

/// Records one method entry. Returns `true` when the invocation threshold is
/// crossed for the first time (caller should hand the method to the compile
/// broker).
pub fn invocation_tick(code_key: usize) -> bool {
    let threshold = compile_threshold();
    let mut map = counters().lock().unwrap();
    let counters = map.entry(code_key).or_insert_with(MethodCounters::new);
    let count = counters.increment_invocation_counter(1);
    if count >= threshold && hot().lock().unwrap().insert(code_key) {
        return true;
    }
    false
}

/// Records one backward branch (loop backedge). Same contract as
/// [`invocation_tick`].
pub fn backedge_tick(code_key: usize) -> bool {
    let threshold = compile_threshold().saturating_mul(4);
    let mut map = counters().lock().unwrap();
    let counters = map.entry(code_key).or_insert_with(MethodCounters::new);
    let count = counters.increment_backedge_counter(1);
    if count >= threshold && hot().lock().unwrap().insert(code_key) {
        return true;
    }
    false
}

/// Returns the live counters for a method, if it has been ticked.
pub fn get(code_key: usize) -> Option<MethodCounters> {
    counters().lock().unwrap().get(&code_key).cloned()
}

/// Whether the method has already crossed a compile threshold.
pub fn is_hot(code_key: usize) -> bool {
    hot().lock().unwrap().contains(&code_key)
}

// ---------------------------------------------------------------------------
// Installed compiled code, keyed the same way as the counters. The value is
// the entry-point address of the nmethod's code blob; 0/absent means the
// method still runs in the interpreter.
// ---------------------------------------------------------------------------

static COMPILED: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();

fn compiled_map() -> &'static Mutex<HashMap<usize, usize>> {
    COMPILED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Records that `code_key`'s method now has executable code at `addr`.
/// Called by the compile broker when an nmethod is installed.
pub fn mark_compiled(code_key: usize, addr: usize) {
    if addr != 0 {
        if std::env::var_os("JVM25_TRACE_COMPILE").is_some() {
            eprintln!("[jit-installed] code_key={code_key:#x} entry={addr:#x}");
        }
        compiled_map().lock().unwrap().insert(code_key, addr);
    }
}

/// Entry-point address of the installed nmethod for `code_key`, or 0.
/// The interpreter checks this once per method entry; a hit replaces
/// interpretation with a direct call into the compiled code.
#[inline]
pub fn compiled_addr(code_key: usize) -> usize {
    *compiled_map().lock().unwrap().get(&code_key).unwrap_or(&0)
}

/// Drops the compiled-code binding (class unloading / deoptimization).
pub fn clear_compiled(code_key: usize) {
    compiled_map().lock().unwrap().remove(&code_key);
}
