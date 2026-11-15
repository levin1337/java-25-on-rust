/*
 * Port of OpenJDK 25 HotSpot MemTag (share/nmt/memTag.hpp)
 *
 * Defines Native Memory Tracking (NMT) subsystem memory classification tags.
 * The tag set mirrors MEMORY_TAG_DO in memTag.hpp exactly (order matters —
 * tags are numbered for the bitmap and reports).
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum MemTag {
    MtJavaHeap = 0,
    MtClass = 1,
    MtThread = 2,
    MtThreadStack = 3,
    MtCode = 4,
    MtGC = 5,
    MtGCCardSet = 6,
    MtCompiler = 7,
    MtJvmci = 8,
    MtInternal = 9,
    MtOther = 10,
    MtSymbol = 11,
    MtNmt = 12,
    MtClassShared = 13,
    MtChunk = 14,
    MtTest = 15,
    MtTracing = 16,
    MtLogging = 17,
    MtStatistics = 18,
    MtArguments = 19,
    MtModule = 20,
    MtSafepoint = 21,
    MtSynchronizer = 22,
    MtServiceability = 23,
    MtMetaspace = 24,
    MtStringDedup = 25,
    MtObjectMonitor = 26,
    MtNumberOfTags = 27,
}

/// Backwards-compatible alias used by earlier ports.
pub type MemoryType = MemTag;

impl Default for MemTag {
    fn default() -> Self {
        Self::MtInternal
    }
}

impl MemTag {
    /// Human-readable name from memTag.hpp.
    pub fn name(self) -> &'static str {
        match self {
            Self::MtJavaHeap => "Java Heap",
            Self::MtClass => "Class",
            Self::MtThread => "Thread",
            Self::MtThreadStack => "Thread Stack",
            Self::MtCode => "Code",
            Self::MtGC => "GC",
            Self::MtGCCardSet => "GCCardSet",
            Self::MtCompiler => "Compiler",
            Self::MtJvmci => "JVMCI",
            Self::MtInternal => "Internal",
            Self::MtOther => "Other",
            Self::MtSymbol => "Symbol",
            Self::MtNmt => "Native Memory Tracking",
            Self::MtClassShared => "Shared class space",
            Self::MtChunk => "Arena Chunk",
            Self::MtTest => "Test",
            Self::MtTracing => "Tracing",
            Self::MtLogging => "Logging",
            Self::MtStatistics => "Statistics",
            Self::MtArguments => "Arguments",
            Self::MtModule => "Module",
            Self::MtSafepoint => "Safepoint",
            Self::MtSynchronizer => "Synchronization",
            Self::MtServiceability => "Serviceability",
            Self::MtMetaspace => "Metaspace",
            Self::MtStringDedup => "String Deduplication",
            Self::MtObjectMonitor => "Object Monitors",
            Self::MtNumberOfTags => "<end>",
        }
    }

    /// NMT report category order (memReporter groups by this).
    pub fn report_order(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: usize) -> Option<Self> {
        if i < Self::MtNumberOfTags as usize {
            // SAFETY-free: transmute via match is verbose; use the fact that
            // discriminants are contiguous.
            Some(unsafe { std::mem::transmute(i as u8) })
        } else {
            None
        }
    }

    /// All trackable tags in report order.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..Self::MtNumberOfTags as usize).filter_map(Self::from_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_count_and_names() {
        assert_eq!(MemTag::MtNumberOfTags as usize, 27);
        assert_eq!(MemTag::MtJavaHeap.name(), "Java Heap");
        assert_eq!(MemTag::MtNmt.name(), "Native Memory Tracking");
    }
}
