/*
 * Port of OpenJDK 25 HotSpot MemTag (share/nmt/memTag.hpp)
 *
 * Native Memory Tracking (NMT) subsystem memory classification tags.
 * The tag set mirrors MEMORY_TAG_DO in memTag.hpp exactly — order and
 * count matter: tags are numbered for snapshots, the malloc site table
 * and reports. mtNone ("Unknown") is a valid tag value used for regions
 * reserved before their final tag is known.
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
    MtNone = 27,
    /// Sentinel: number of memory tags (mt_number_of_tags). Not a tag.
    MtNumberOfTags = 28,
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
            Self::MtNone => "Unknown",
            Self::MtNumberOfTags => "<end>",
        }
    }

    /// Literalized enum name, e.g. "mtNMT" (NMTUtil::tag_to_enum_name).
    pub fn enum_name(self) -> &'static str {
        match self {
            Self::MtJavaHeap => "mtJavaHeap",
            Self::MtClass => "mtClass",
            Self::MtThread => "mtThread",
            Self::MtThreadStack => "mtThreadStack",
            Self::MtCode => "mtCode",
            Self::MtGC => "mtGC",
            Self::MtGCCardSet => "mtGCCardSet",
            Self::MtCompiler => "mtCompiler",
            Self::MtJvmci => "mtJVMCI",
            Self::MtInternal => "mtInternal",
            Self::MtOther => "mtOther",
            Self::MtSymbol => "mtSymbol",
            Self::MtNmt => "mtNMT",
            Self::MtClassShared => "mtClassShared",
            Self::MtChunk => "mtChunk",
            Self::MtTest => "mtTest",
            Self::MtTracing => "mtTracing",
            Self::MtLogging => "mtLogging",
            Self::MtStatistics => "mtStatistics",
            Self::MtArguments => "mtArguments",
            Self::MtModule => "mtModule",
            Self::MtSafepoint => "mtSafepoint",
            Self::MtSynchronizer => "mtSynchronizer",
            Self::MtServiceability => "mtServiceability",
            Self::MtMetaspace => "mtMetaspace",
            Self::MtStringDedup => "mtStringDedup",
            Self::MtObjectMonitor => "mtObjectMonitor",
            Self::MtNone => "mtNone",
            Self::MtNumberOfTags => "mtNumberOfTags",
        }
    }

    /// NMT report category order (same as enum order).
    pub fn report_order(self) -> u8 {
        self as u8
    }

    /// Map index to MemTag; None for the MtNumberOfTags sentinel.
    pub fn from_index(i: usize) -> Option<Self> {
        if i < Self::MtNumberOfTags as usize {
            Some(unsafe { std::mem::transmute(i as u8) })
        } else {
            None
        }
    }

    /// All tags in report order, including mtNone (which normally
    /// carries zero usage and is omitted from reports).
    pub fn all() -> impl Iterator<Item = Self> {
        (0..Self::MtNumberOfTags as usize).filter_map(Self::from_index)
    }

    /// Trackable tags only (excludes the mtNone sentinel usage — mtNone
    /// itself is a real tag value and is included).
    pub fn is_tag(self) -> bool {
        self != Self::MtNumberOfTags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_count_and_names() {
        assert_eq!(MemTag::MtNumberOfTags as usize, 28);
        assert_eq!(MemTag::MtNone as usize, 27);
        assert_eq!(MemTag::MtJavaHeap.name(), "Java Heap");
        assert_eq!(MemTag::MtNmt.name(), "Native Memory Tracking");
        assert_eq!(MemTag::MtNone.name(), "Unknown");
        assert_eq!(MemTag::MtObjectMonitor.enum_name(), "mtObjectMonitor");
    }

    #[test]
    fn tag_iteration_covers_all() {
        assert_eq!(MemTag::all().count(), 28);
        assert_eq!(MemTag::all().last(), Some(MemTag::MtNone));
    }
}
