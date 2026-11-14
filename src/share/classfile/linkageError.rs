/*
 * Port of OpenJDK 25 HotSpot linkage error reporting
 * (share/classfile/resolutionErrors.hpp / resolutionErrors.cpp)
 *
 * Produces JVM linkage errors with the same names and message shapes the
 * Java specification defines (JVMS §5), so failures surface to Java code
 * and to the launcher like real HotSpot errors.
 */

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkageErrorKind {
    ClassNotFound,
    NoClassDefFound,
    ClassFormat,
    UnsupportedClassVersion,
    Verify,
    ClassCircularity,
    IncompatibleClassChange,
    NoSuchField,
    NoSuchMethod,
    IllegalAccess,
    Instantiation,
    ExceptionInInitializer,
    UnsatisfiedLink,
}

impl LinkageErrorKind {
    pub fn java_name(self) -> &'static str {
        match self {
            Self::ClassNotFound => "java.lang.ClassNotFoundException",
            Self::NoClassDefFound => "java.lang.NoClassDefFoundError",
            Self::ClassFormat => "java.lang.ClassFormatError",
            Self::UnsupportedClassVersion => "java.lang.UnsupportedClassVersionError",
            Self::Verify => "java.lang.VerifyError",
            Self::ClassCircularity => "java.lang.ClassCircularityError",
            Self::IncompatibleClassChange => "java.lang.IncompatibleClassChangeError",
            Self::NoSuchField => "java.lang.NoSuchFieldError",
            Self::NoSuchMethod => "java.lang.NoSuchMethodError",
            Self::IllegalAccess => "java.lang.IllegalAccessError",
            Self::Instantiation => "java.lang.InstantiationError",
            Self::ExceptionInInitializer => "java.lang.ExceptionInInitializerError",
            Self::UnsatisfiedLink => "java.lang.UnsatisfiedLinkError",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinkageError {
    pub kind: LinkageErrorKind,
    pub message: String,
}

impl LinkageError {
    pub fn new(kind: LinkageErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn class_not_found(name: &[u8]) -> Self {
        Self::new(
            LinkageErrorKind::ClassNotFound,
            format!("{}", String::from_utf8_lossy(name)),
        )
    }

    pub fn no_class_def_found(name: &[u8]) -> Self {
        Self::new(
            LinkageErrorKind::NoClassDefFound,
            format!("{}", String::from_utf8_lossy(name)),
        )
    }

    pub fn no_such_method(class: &[u8], name: &[u8], descriptor: &[u8]) -> Self {
        Self::new(
            LinkageErrorKind::NoSuchMethod,
            format!(
                "{}.{}{}",
                String::from_utf8_lossy(class),
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor)
            ),
        )
    }

    pub fn no_such_field(class: &[u8], name: &[u8], descriptor: &[u8]) -> Self {
        Self::new(
            LinkageErrorKind::NoSuchField,
            format!(
                "{}.{}{}",
                String::from_utf8_lossy(class),
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor)
            ),
        )
    }

    pub fn incompatible_class_change(message: impl Into<String>) -> Self {
        Self::new(LinkageErrorKind::IncompatibleClassChange, message)
    }

    pub fn illegal_access(message: impl Into<String>) -> Self {
        Self::new(LinkageErrorKind::IllegalAccess, message)
    }

    pub fn io(self) -> std::io::Error {
        std::io::Error::other(self)
    }
}

impl fmt::Display for LinkageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.java_name(), self.message)
    }
}

impl std::error::Error for LinkageError {}

/// Extracts the Java exception name from an io::Error produced by
/// `LinkageError::io`, if present.
pub fn linkage_kind(error: &std::io::Error) -> Option<LinkageErrorKind> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<LinkageError>())
        .map(|e| e.kind)
}
