/*
 * Port of OpenJDK 25 HotSpot InvocationCounter (share/interpreter/invocationCounter.hpp, invocationCounter.cpp)
 *
 * InvocationCounters track method invocation and loop backedge execution counts.
 * When a counter exceeds the compilation threshold, HotSpot triggers tiered JIT compilation (C1 / C2).
 *
 * Counter layout (32 bits):
 * [ count (31 bits) | carry (1 bit) ]
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationCounter {
    counter: u32,
    limit: u32,
}

impl Default for InvocationCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl InvocationCounter {
    pub const NUMBER_OF_CARRY_BITS: u32 = 1;
    pub const NUMBER_OF_NONCOUNT_BITS: u32 = Self::NUMBER_OF_CARRY_BITS;
    pub const COUNT_GRAIN: u32 = 1 << Self::NUMBER_OF_CARRY_BITS;
    pub const CARRY_MASK: u32 = (1 << Self::NUMBER_OF_CARRY_BITS) - 1;
    pub const COUNT_MASK: u32 = !Self::CARRY_MASK;
    pub const COUNT_INCREMENT: u32 = Self::COUNT_GRAIN;
    pub const NUMBER_OF_COUNT_BITS: u32 = 32 - Self::NUMBER_OF_NONCOUNT_BITS;
    pub const COUNT_LIMIT: u32 = 1 << (Self::NUMBER_OF_COUNT_BITS - 1);
    pub const DEFAULT_COMPILE_THRESHOLD: u32 = 10_000;

    pub fn new() -> Self {
        Self {
            counter: 0,
            limit: Self::DEFAULT_COMPILE_THRESHOLD,
        }
    }

    pub fn with_limit(limit: u32) -> Self {
        Self { counter: 0, limit }
    }

    pub fn init(&mut self) {
        self.counter = 0;
    }

    pub fn reset(&mut self) {
        self.update(0);
    }

    pub fn increment(&mut self) {
        self.counter = self.counter.wrapping_add(Self::COUNT_INCREMENT);
    }

    pub fn count(&self) -> u32 {
        self.counter >> Self::NUMBER_OF_NONCOUNT_BITS
    }

    pub fn carry(&self) -> bool {
        (self.counter & Self::CARRY_MASK) != 0
    }

    pub fn set_carry(&mut self) {
        self.counter |= Self::CARRY_MASK;
    }

    pub fn raw_counter(&self) -> u32 {
        self.counter
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    pub fn set_limit(&mut self, limit: u32) {
        self.limit = limit;
    }

    pub fn reached_limit(&self) -> bool {
        self.carry() || self.count() >= self.limit
    }

    pub fn set_carry_on_overflow(&mut self) {
        if !self.carry() && self.count() > (Self::COUNT_LIMIT / 2) {
            self.set_carry();
        }
    }

    pub fn set(&mut self, count: u32) {
        let carry = self.counter & Self::CARRY_MASK;
        self.counter = (count << Self::NUMBER_OF_NONCOUNT_BITS) | carry;
    }

    pub fn set_with_carry(&mut self, count: u32, carry: bool) {
        let carry_flag = if carry { Self::CARRY_MASK } else { 0 };
        self.counter = (count << Self::NUMBER_OF_NONCOUNT_BITS) | carry_flag;
    }

    pub fn update(&mut self, mut new_count: u32) {
        let c = self.count();
        let carry = self.carry();
        // Prevent going to zero if it was previously executed to distinguish from unexecuted
        if c > 0 && new_count == 0 {
            new_count = 1;
        }
        self.set_with_carry(new_count, carry);
    }
}
