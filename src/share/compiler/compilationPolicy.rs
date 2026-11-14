// Compilation policy ported from HotSpot compilationPolicy.hpp / .cpp

use super::compilerDefinitions::CompLevel;

#[derive(Clone, Debug)]
pub struct CompilationPolicy {
    pub tier3_invocation_threshold: u32,
    pub tier3_min_invocation_threshold: u32,
    pub tier3_compile_threshold: u32,
    pub tier3_backedge_threshold: u32,

    pub tier4_invocation_threshold: u32,
    pub tier4_min_invocation_threshold: u32,
    pub tier4_compile_threshold: u32,
    pub tier4_backedge_threshold: u32,

    pub tier3_load_feedback: usize,
    pub tier4_load_feedback: usize,
    pub tier3_delay_on: usize,
    pub tier3_delay_off: usize,
}

impl CompilationPolicy {
    pub fn new() -> Self {
        Self {
            tier3_invocation_threshold: 200,
            tier3_min_invocation_threshold: 100,
            tier3_compile_threshold: 2000,
            tier3_backedge_threshold: 60000,

            tier4_invocation_threshold: 2000,
            tier4_min_invocation_threshold: 600,
            tier4_compile_threshold: 15000,
            tier4_backedge_threshold: 40000,

            tier3_load_feedback: 5,
            tier4_load_feedback: 3,
            tier3_delay_on: 10000,
            tier3_delay_off: 2000,
        }
    }

    pub fn should_compile(&self, invocation_count: u32) -> Option<CompLevel> {
        if invocation_count >= self.tier4_invocation_threshold {
            Some(CompLevel::FullOptimization)
        } else if invocation_count >= self.tier3_invocation_threshold {
            Some(CompLevel::FullProfile)
        } else {
            None
        }
    }

    pub fn call_predicate(&self, cur_level: CompLevel, i: u32, b: u32, scale: f64) -> bool {
        let s = if scale <= 0.0 { 1.0 } else { scale };
        match cur_level {
            CompLevel::None | CompLevel::LimitedProfile => {
                (i as f64 >= self.tier3_invocation_threshold as f64 * s)
                    || (i as f64 >= self.tier3_min_invocation_threshold as f64 * s
                        && (i.saturating_add(b)) as f64 >= self.tier3_compile_threshold as f64 * s)
            }
            CompLevel::FullProfile => {
                (i as f64 >= self.tier4_invocation_threshold as f64 * s)
                    || (i as f64 >= self.tier4_min_invocation_threshold as f64 * s
                        && (i.saturating_add(b)) as f64 >= self.tier4_compile_threshold as f64 * s)
            }
            _ => true,
        }
    }

    pub fn loop_predicate(&self, cur_level: CompLevel, b: u32, scale: f64) -> bool {
        let s = if scale <= 0.0 { 1.0 } else { scale };
        match cur_level {
            CompLevel::None | CompLevel::LimitedProfile => {
                b as f64 >= self.tier3_backedge_threshold as f64 * s
            }
            CompLevel::FullProfile => b as f64 >= self.tier4_backedge_threshold as f64 * s,
            _ => true,
        }
    }

    pub fn threshold_scale(
        &self,
        cur_level: CompLevel,
        queue_size: usize,
        compiler_count: usize,
    ) -> f64 {
        let count = compiler_count.max(1);
        let feedback = match cur_level {
            CompLevel::None | CompLevel::LimitedProfile => self.tier3_load_feedback,
            _ => self.tier4_load_feedback,
        };
        (queue_size as f64 / (feedback * count) as f64) + 1.0
    }

    pub fn call_event(&self, cur_level: CompLevel, i: u32, b: u32) -> CompLevel {
        match cur_level {
            CompLevel::None => {
                if self.call_predicate(CompLevel::None, i, b, 1.0) {
                    CompLevel::FullProfile
                } else {
                    CompLevel::None
                }
            }
            CompLevel::LimitedProfile => {
                if self.call_predicate(CompLevel::LimitedProfile, i, b, 1.0) {
                    CompLevel::FullProfile
                } else {
                    CompLevel::LimitedProfile
                }
            }
            CompLevel::FullProfile => {
                if self.call_predicate(CompLevel::FullProfile, i, b, 1.0) {
                    CompLevel::FullOptimization
                } else {
                    CompLevel::FullProfile
                }
            }
            CompLevel::FullOptimization | CompLevel::Simple | CompLevel::Any => cur_level,
        }
    }

    pub fn loop_event(&self, cur_level: CompLevel, _i: u32, b: u32) -> CompLevel {
        match cur_level {
            CompLevel::None => {
                if self.loop_predicate(CompLevel::None, b, 1.0) {
                    CompLevel::FullProfile
                } else {
                    CompLevel::None
                }
            }
            CompLevel::LimitedProfile => {
                if self.loop_predicate(CompLevel::LimitedProfile, b, 1.0) {
                    CompLevel::FullProfile
                } else {
                    CompLevel::LimitedProfile
                }
            }
            CompLevel::FullProfile => {
                if self.loop_predicate(CompLevel::FullProfile, b, 1.0) {
                    CompLevel::FullOptimization
                } else {
                    CompLevel::FullProfile
                }
            }
            CompLevel::FullOptimization | CompLevel::Simple | CompLevel::Any => cur_level,
        }
    }

    pub fn must_be_compiled(&self, comp_level: CompLevel) -> bool {
        comp_level.is_compile()
    }
}

impl Default for CompilationPolicy {
    fn default() -> Self {
        Self::new()
    }
}
