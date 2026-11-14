/*
 * Port of OpenJDK 25 HotSpot java (share/runtime/java.hpp / java.cpp)
 *
 * Provides JVM lifecycle exit handlers and JDK version metrics.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JDKVersion {
    major: u32,
    minor: u32,
    security: u32,
    build: u32,
}

impl JDKVersion {
    pub const CURRENT: Self = Self {
        major: 25,
        minor: 0,
        security: 0,
        build: 1,
    };

    pub fn new(major: u32, minor: u32, security: u32, build: u32) -> Self {
        Self {
            major,
            minor,
            security,
            build,
        }
    }

    #[inline]
    pub fn major(&self) -> u32 {
        self.major
    }

    #[inline]
    pub fn minor(&self) -> u32 {
        self.minor
    }

    #[inline]
    pub fn security(&self) -> u32 {
        self.security
    }

    #[inline]
    pub fn build(&self) -> u32 {
        self.build
    }

    pub fn is_at_least(&self, major: u32, minor: u32) -> bool {
        self.major > major || (self.major == major && self.minor >= minor)
    }

    pub fn to_string(&self) -> String {
        format!(
            "{}.{}.{}+{}",
            self.major, self.minor, self.security, self.build
        )
    }
}

pub struct Java;

impl Java {
    pub fn jdk_version() -> JDKVersion {
        JDKVersion::CURRENT
    }

    pub fn vm_shutdown_during_initialization(message: &str) -> ! {
        eprintln!("Error occurred during initialization of VM");
        eprintln!("{}", message);
        std::process::exit(1);
    }
}
