/*
 * Port of OpenJDK 25 HotSpot JVMCI exception support
 * (share/jvmci/jvmciExceptions.hpp)
 *
 * The C++ code uses JVMCI_TRAPS / JVMCI_CHECK / JVMCI_THROW macros to
 * propagate exceptions pending on either the HotSpot or the shared
 * library heap.  In Rust the same role is played by `JvmciResult<T>`
 * and `JvmciException`, which records the Java exception class that
 * would have been thrown (`ThrowKind`) plus the message.
 */

use std::fmt;

/// Error message used whenever JVMCI is referenced while disabled.
/// Mirrors JVMCI_NOT_ENABLED_ERROR_MESSAGE in jvmciRuntime.hpp.
pub const JVMCI_NOT_ENABLED_ERROR_MESSAGE: &str =
    "JVMCI is not enabled. Must specify '-XX:+EnableJVMCI' or '--add-modules=jdk.internal.vm.ci' to the java launcher.";

/// Java exception classes throwable through a `JvmciEnv`
/// (the `DO_THROW` list in jvmciEnv.hpp).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThrowKind {
    InternalError,
    ArrayIndexOutOfBoundsException,
    IllegalStateException,
    NullPointerException,
    IllegalArgumentException,
    InvalidInstalledCodeException,
    UnsatisfiedLinkError,
    UnsupportedOperationException,
    OutOfMemoryError,
    NoClassDefFoundError,
}

impl ThrowKind {
    /// Internal (slash-separated) class name of the exception.
    pub fn class_name(&self) -> &'static str {
        match self {
            Self::InternalError => "java/lang/InternalError",
            Self::ArrayIndexOutOfBoundsException => "java/lang/ArrayIndexOutOfBoundsException",
            Self::IllegalStateException => "java/lang/IllegalStateException",
            Self::NullPointerException => "java/lang/NullPointerException",
            Self::IllegalArgumentException => "java/lang/IllegalArgumentException",
            Self::InvalidInstalledCodeException => "jdk/vm/ci/code/InvalidInstalledCodeException",
            Self::UnsatisfiedLinkError => "java/lang/UnsatisfiedLinkError",
            Self::UnsupportedOperationException => "java/lang/UnsupportedOperationException",
            Self::OutOfMemoryError => "java/lang/OutOfMemoryError",
            Self::NoClassDefFoundError => "java/lang/NoClassDefFoundError",
        }
    }

    /// Simple class name.
    pub fn simple_name(&self) -> &'static str {
        match self {
            Self::InternalError => "InternalError",
            Self::ArrayIndexOutOfBoundsException => "ArrayIndexOutOfBoundsException",
            Self::IllegalStateException => "IllegalStateException",
            Self::NullPointerException => "NullPointerException",
            Self::IllegalArgumentException => "IllegalArgumentException",
            Self::InvalidInstalledCodeException => "InvalidInstalledCodeException",
            Self::UnsatisfiedLinkError => "UnsatisfiedLinkError",
            Self::UnsupportedOperationException => "UnsupportedOperationException",
            Self::OutOfMemoryError => "OutOfMemoryError",
            Self::NoClassDefFoundError => "NoClassDefFoundError",
        }
    }
}

/// A pending JVMCI exception.  Carries the file/line of the throw site
/// for diagnostics, mirroring the JVMCI_ERROR / JVMCI_THROW macros.
#[derive(Clone, Debug)]
pub struct JvmciException {
    pub kind: ThrowKind,
    pub message: String,
    pub file: &'static str,
    pub line: u32,
}

impl JvmciException {
    pub fn new(kind: ThrowKind, message: impl Into<String>, file: &'static str, line: u32) -> Self {
        Self {
            kind,
            message: message.into(),
            file,
            line,
        }
    }

    /// `JVMCIENV->throw_<Kind>(msg)`.
    pub fn throw_kind(kind: ThrowKind, msg: Option<&str>, file: &'static str, line: u32) -> Self {
        Self::new(kind, msg.unwrap_or(""), file, line)
    }

    /// `JVMCIENV->fthrow_error(file, line, fmt, ...)`.
    pub fn fthrow_error(file: &'static str, line: u32, message: String) -> Self {
        Self::new(ThrowKind::InternalError, message, file, line)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::new(ThrowKind::InternalError, msg, "<jvmci>", 0)
    }

    /// The error raised when JVMCI is used while `EnableJVMCI` is false.
    pub fn not_enabled() -> Self {
        Self::internal_error(JVMCI_NOT_ENABLED_ERROR_MESSAGE)
    }

    /// Honest failure for operations that require a JVMCI compiler
    /// (Graal/libjvmci) which is not present in this VM.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::new(
            ThrowKind::UnsupportedOperationException,
            msg,
            "<jvmci>",
            0,
        )
    }

    pub fn null_pointer(msg: impl Into<String>) -> Self {
        Self::new(ThrowKind::NullPointerException, msg, "<jvmci>", 0)
    }

    pub fn illegal_argument(msg: impl Into<String>) -> Self {
        Self::new(ThrowKind::IllegalArgumentException, msg, "<jvmci>", 0)
    }

    pub fn out_of_memory(msg: impl Into<String>) -> Self {
        Self::new(ThrowKind::OutOfMemoryError, msg, "<jvmci>", 0)
    }

    /// `describe_pending_exception` style rendering.
    pub fn describe(&self) -> String {
        if self.message.is_empty() {
            format!("{} at {}:{}", self.kind.simple_name(), self.file, self.line)
        } else {
            format!(
                "{}: {} at {}:{}",
                self.kind.simple_name(),
                self.message,
                self.file,
                self.line
            )
        }
    }
}

impl fmt::Display for JvmciException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.describe())
    }
}

impl std::error::Error for JvmciException {}

/// Result type used in place of the `JVMCI_TRAPS` out-parameter.
pub type JvmciResult<T> = Result<T, JvmciException>;
