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
    }
}

impl Default for MethodCounters {
    fn default() -> Self {
        Self::new()
    }
}
