/*
 * Port of OpenJDK 25 HotSpot MemTag (share/nmt/memTag.hpp)
 *
 * Defines Native Memory Tracking (NMT) subsystem memory classification tags.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryType {
    MtNone = 0,
    MtClass = 1,
    MtThread = 2,
    MtThreadStack = 3,
    MtCode = 4,
    MtGC = 5,
    MtCompiler = 6,
    MtInternal = 7,
    MtSymbol = 8,
    MtNMT = 9,
    MtTracing = 10,
}

impl MemoryType {
    pub fn name(self) -> &'static str {
        match self {
            Self::MtNone => "None",
            Self::MtClass => "Class",
            Self::MtThread => "Thread",
            Self::MtThreadStack => "Thread Stack",
            Self::MtCode => "Code",
            Self::MtGC => "GC",
            Self::MtCompiler => "Compiler",
            Self::MtInternal => "Internal",
            Self::MtSymbol => "Symbol",
            Self::MtNMT => "NMT",
            Self::MtTracing => "Tracing",
        }
    }
}
