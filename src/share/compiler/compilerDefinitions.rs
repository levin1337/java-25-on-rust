// Compiler definitions ported from HotSpot compilerDefinitions.hpp / .cpp

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum CompLevel {
    Any = -1,
    None = 0,             // Interpreter
    Simple = 1,           // C1 pure (no profiling)
    LimitedProfile = 2,   // C1 basic profiling (invocation & backedge counters)
    FullProfile = 3,      // C1 full profiling (counters + MDO)
    FullOptimization = 4, // C2 or JVMCI
}

impl Default for CompLevel {
    fn default() -> Self {
        Self::None
    }
}

impl CompLevel {
    pub const COUNT: usize = 5;

    pub fn is_c1(&self) -> bool {
        matches!(
            self,
            Self::Simple | Self::LimitedProfile | Self::FullProfile
        )
    }

    pub fn is_c2(&self) -> bool {
        matches!(self, Self::FullOptimization)
    }

    pub fn is_compile(&self) -> bool {
        self.is_c1() || self.is_c2()
    }

    pub fn is_highest_tier(&self) -> bool {
        matches!(self, Self::FullOptimization)
    }

    pub fn from_i32(val: i32) -> Option<Self> {
        match val {
            -1 => Some(Self::Any),
            0 => Some(Self::None),
            1 => Some(Self::Simple),
            2 => Some(Self::LimitedProfile),
            3 => Some(Self::FullProfile),
            4 => Some(Self::FullOptimization),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::None => "interpreter",
            Self::Simple => "c1 simple",
            Self::LimitedProfile => "c1 limited profile",
            Self::FullProfile => "c1 full profile",
            Self::FullOptimization => "c2",
        }
    }

    pub fn highest_compile_level() -> Self {
        Self::FullOptimization
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CompilerType {
    None = 0,
    C1 = 1,
    C2 = 2,
    Jvmci = 3,
}

impl CompilerType {
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::C1 => "c1",
            Self::C2 => "c2",
            Self::Jvmci => "jvmci",
        }
    }

    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::None),
            1 => Some(Self::C1),
            2 => Some(Self::C2),
            3 => Some(Self::Jvmci),
            _ => None,
        }
    }
}

pub struct MethodCompilation;

impl MethodCompilation {
    pub const INVOCATION_ENTRY_BCI: i32 = -1;
    pub const BEFORE_BCI: i32 = -1;
    pub const AFTER_BCI: i32 = -2;
    pub const UNWIND_BCI: i32 = -3;
    pub const AFTER_EXCEPTION_BCI: i32 = -4;
    pub const UNKNOWN_BCI: i32 = -5;
    pub const INVALID_FRAME_STATE_BCI: i32 = -6;
}

/// `CompilationModeFlag` analogue: selects which levels the tiered
/// policy is allowed to produce (used for -Xcomp-style restricted modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilationMode {
    Normal,
    QuickOnly,
    HighOnly,
    HighOnlyQuickInternal,
}

impl CompilationMode {
    pub fn normal(&self) -> bool {
        matches!(self, Self::Normal)
    }
    pub fn quick_only(&self) -> bool {
        matches!(self, Self::QuickOnly)
    }
    pub fn high_only(&self) -> bool {
        matches!(self, Self::HighOnly)
    }
    pub fn quick_internal(&self) -> bool {
        matches!(self, Self::QuickOnly | Self::HighOnlyQuickInternal)
    }
    pub fn disable_intermediate(&self) -> bool {
        !matches!(self, Self::Normal)
    }
}

pub struct CompilerConfig;

impl CompilerConfig {
    pub fn has_c1() -> bool {
        true
    }

    pub fn has_c2() -> bool {
        true
    }

    pub fn has_tiered() -> bool {
        true
    }

    pub fn is_tiered() -> bool {
        true
    }

    pub fn scaled_compile_threshold(threshold: u32, scale: f64) -> u32 {
        if scale <= 0.0 {
            threshold
        } else {
            (threshold as f64 * scale).round() as u32
        }
    }

    pub fn scaled_freq_log(freq_log: i32, scale: f64) -> i32 {
        if scale <= 0.0 {
            freq_log
        } else {
            let scaled = (freq_log as f64 * scale).round() as i32;
            scaled.max(0)
        }
    }

    /// Highest level reachable under the current compiler configuration,
    /// before clamping by `TieredStopAtLevel` / compilation mode.
    pub fn highest_available_level() -> CompLevel {
        if Self::is_interpreter_only() {
            CompLevel::None
        } else if Self::is_c2_or_jvmci_compiler_enabled() {
            CompLevel::FullOptimization
        } else if Self::is_c1_enabled() {
            CompLevel::FullProfile
        } else {
            CompLevel::None
        }
    }

    pub fn is_interpreter_only() -> bool {
        false
    }

    pub fn is_c2_or_jvmci_compiler_enabled() -> bool {
        true
    }

    pub fn is_c1_enabled() -> bool {
        true
    }

    pub fn is_c1_simple_only() -> bool {
        false
    }
}
