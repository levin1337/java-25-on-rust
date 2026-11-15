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

    /// HotSpot `GCCause::is_user_requested_gc`: explicit collection
    /// requests (System.gc, WhiteBox, GC locker diagnostics).
    pub fn is_user_requested_gc(&self) -> bool {
        matches!(
            self,
            Self::JavaLangSystemGC | Self::JvmtiEnvForceGarbageCollection | Self::GCWatcher
        )
    }

    /// HotSpot `GCCause::is_gc_endopausal`: the cause corresponds to a
    /// stop-the-world pause rather than a concurrent cycle trigger.
    /// Every cause modelled here is a pause; concurrent marking cycles
    /// (G1 concurrent mark, ZGC concurrent mark) are not GCCauses.
    pub fn is_gc_endopausal(&self) -> bool {
        !matches!(self, Self::NoGC)
    }

    /// HotSpot `GCCause::is_allocation_failure`: the GC was triggered
    /// because an allocation could not be satisfied.
    pub fn is_allocation_failure(&self) -> bool {
        matches!(
            self,
            Self::AllocationFailure
                | Self::GCLockerRetryAllocation
                | Self::TenuredGenerationFull
                | Self::MetadataAllocationFailure
                | Self::OldGenerationTooFullToScavenge
        )
    }

    /// HotSpot `GCCause::is_tenured_full`: old generation is full.
    pub fn is_tenured_full(&self) -> bool {
        matches!(
            self,
            Self::TenuredGenerationFull | Self::OldGenerationTooFullToScavenge
        )
    }

    /// HotSpot `GCCause::is_metadata_gc`: metadata (class space) GC.
    pub fn is_metadata_gc(&self) -> bool {
        matches!(self, Self::MetadataAllocationFailure)
    }

    /// HotSpot `GCCause::is_jvmti_gc`: GC requested through JVMTI.
    pub fn is_jvmti_gc(&self) -> bool {
        matches!(self, Self::JvmtiEnvForceGarbageCollection)
    }
}
