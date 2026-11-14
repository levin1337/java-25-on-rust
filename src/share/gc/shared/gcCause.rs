#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GCCause {
    JavaLangSystemGC,
    FullGCAlot,
    ScavengeAlot,
    AllocationProfiler,
    JvmtiEnvForceGarbageCollection,
    GCWatcher,
    AdaptiveSizePolicy,
    GCLockerRetryAllocation,
    AllocationFailure,
    TenuredGenerationFull,
    MetadataAllocationFailure,
    OldGenerationExpandedOnLastScavenge,
    OldGenerationTooFullToScavenge,
    Ergonomics,
    G1CompactionPause,
    G1IncCollectionPause,
    G1PeriodicCollection,
    NoGC,
}

impl GCCause {
    pub fn to_str(&self) -> &'static str {
        match self {
            Self::JavaLangSystemGC => "System.gc()",
            Self::AllocationFailure => "Allocation Failure",
            Self::MetadataAllocationFailure => "Metadata Allocation Failure",
            Self::GCLockerRetryAllocation => "GCLocker Retry Allocation",
            Self::AdaptiveSizePolicy => "Adaptive Size Policy",
            Self::Ergonomics => "Ergonomics",
            Self::G1CompactionPause => "G1 Compaction Pause",
            Self::G1IncCollectionPause => "G1 Evacuation Pause",
            Self::G1PeriodicCollection => "G1 Periodic Collection",
            Self::NoGC => "No GC",
            _ => "Unknown GC Cause",
        }
    }
}
