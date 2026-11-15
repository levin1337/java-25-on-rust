/*
 * Port of OpenJDK 25 HotSpot AbstractCompiler interface
 * (share/compiler/abstractCompiler.hpp / .cpp)
 *
 * A compiler backend (C1, C2, JVMCI) implements this trait.  The broker
 * hands it CompileTasks popped from the compile queues and receives a
 * CompileOutcome describing the generated code and metadata, which the
 * broker then installs into the code cache as an nmethod.
 */

use super::compileTask::{CompileOutcome, CompileTask, FailureReason};
use super::compilerDefinitions::CompLevel;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompilerType {
    None,
    C1,
    C2,
    Jvmci,
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
}

/// Error reported by a compiler backend for a failed compilation.
#[derive(Debug)]
pub struct CompileError {
    pub reason: FailureReason,
    pub message: String,
    /// Level at which compilation failed.
    pub failed_at_level: CompLevel,
}

impl CompileError {
    pub fn new(reason: FailureReason, message: impl Into<String>, level: CompLevel) -> Self {
        Self {
            reason,
            message: message.into(),
            failed_at_level: level,
        }
    }

    pub fn internal(msg: impl Into<String>, level: CompLevel) -> Self {
        Self::new(FailureReason::InternalError, msg, level)
    }

    pub fn unsupported(msg: impl Into<String>, level: CompLevel) -> Self {
        Self::new(FailureReason::Unsupported, msg, level)
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.reason.as_str(), self.message)
    }
}

impl std::error::Error for CompileError {}

/// The compiler backend contract.  Implementations must be `Send` so they
/// can run on dedicated compiler threads.
pub trait AbstractCompiler: Send {
    /// Which kind of compiler this is (C1/C2/JVMCI).
    fn compiler_type(&self) -> CompilerType;

    /// Human-readable compiler name for logs.
    fn compiler_name(&self) -> &'static str {
        self.compiler_type().name()
    }

    /// Whether this compiler handles the given comp level.
    fn supports_level(&self, level: CompLevel) -> bool;

    /// Compile `task`.  On success returns the generated code and metadata
    /// (`CompileOutcome`); on failure returns `CompileError` — the broker
    /// records it on the task and marks it complete.
    ///
    /// Implementations that cannot actually generate code should return
    /// `Err(CompileError::unsupported(...))` rather than fake output.
    fn compile_method(&mut self, task: &CompileTask) -> Result<CompileOutcome, CompileError>;

    /// Called when the compiler's queue runs dry (C1/C2 on_empty_queue).
    fn on_empty_queue(&mut self) {}

    /// Whether this compiler is a JVMCI compiler.
    fn is_jvmci(&self) -> bool {
        self.compiler_type() == CompilerType::Jvmci
    }

    /// Whether this compiler is C2 or JVMCI (needs class resolution first).
    fn is_c2(&self) -> bool {
        matches!(self.compiler_type(), CompilerType::C2 | CompilerType::Jvmci)
    }

    /// Whether this compiler is C1.
    fn is_c1(&self) -> bool {
        self.compiler_type() == CompilerType::C1
    }
}
