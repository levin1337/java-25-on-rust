/*
 * Port of OpenJDK 25 HotSpot C2 Compiler Phase (share/opto/phase.hpp / phase.cpp)
 *
 * Tracks compilation phases and timing for C2 Ideal Graph transformations.
 */

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhaseNumber {
    Compiler,
    Parser,
    GVN,
    IterGVN,
    EscapeAnalysis,
    Vectorize,
    MacroExpand,
    Matcher,
    RegisterAllocation,
    Output,
}

pub struct Phase {
    pub number: PhaseNumber,
    pub name: &'static str,
    start_time: Instant,
    elapsed: Duration,
}

impl Phase {
    pub fn new(number: PhaseNumber, name: &'static str) -> Self {
        Self {
            number,
            name,
            start_time: Instant::now(),
            elapsed: Duration::ZERO,
        }
    }

    pub fn complete(&mut self) -> Duration {
        self.elapsed = self.start_time.elapsed();
        self.elapsed
    }

    pub fn elapsed(&self) -> Duration {
        if self.elapsed == Duration::ZERO {
            self.start_time.elapsed()
        } else {
            self.elapsed
        }
    }
}
