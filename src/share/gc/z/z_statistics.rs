use super::z_generation_id::ZGenerationId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZGCPhase {
    PauseMarkStart,
    ConcurrentMark,
    PauseMarkEnd,
    ConcurrentMarkContinue,
    ConcurrentProcessNonStrongReferences,
    ConcurrentResetRelocationSet,
    ConcurrentSelectRelocationSet,
    PauseRelocateStart,
    ConcurrentRelocate,
    ConcurrentRemapYoungRoots,
}

impl ZGCPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PauseMarkStart => "Pause Mark Start",
            Self::ConcurrentMark => "Concurrent Mark",
            Self::PauseMarkEnd => "Pause Mark End",
            Self::ConcurrentMarkContinue => "Concurrent Mark Continue",
            Self::ConcurrentProcessNonStrongReferences => "Concurrent Process Non-Strong References",
            Self::ConcurrentResetRelocationSet => "Concurrent Reset Relocation Set",
            Self::ConcurrentSelectRelocationSet => "Concurrent Select Relocation Set",
            Self::PauseRelocateStart => "Pause Relocate Start",
            Self::ConcurrentRelocate => "Concurrent Relocate",
            Self::ConcurrentRemapYoungRoots => "Concurrent Remap Young Roots",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZPauseReason {
    MarkStart,
    MarkEnd,
    RelocateStart,
    Verify,
    Other,
}

impl ZPauseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MarkStart => "Mark Start",
            Self::MarkEnd => "Mark End",
            Self::RelocateStart => "Relocate Start",
            Self::Verify => "Verify",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZPauseStats {
    pub count: u64,
    pub total_nanos: u64,
    pub max_nanos: u64,
}

pub struct ZStatistics {
    pauses: Mutex<[ZPauseStats; 5]>,
    collections: AtomicU64,
    cycle_start: Mutex<Option<Instant>>,
    last_pause_end: Mutex<Option<Instant>>,
    mmu_pauses: Mutex<Vec<(Instant, Instant)>>,
}

impl ZStatistics {
    pub fn new() -> Self {
        Self {
            pauses: Mutex::new([ZPauseStats::default(); 5]),
            collections: AtomicU64::new(0),
            cycle_start: Mutex::new(None),
            last_pause_end: Mutex::new(None),
            mmu_pauses: Mutex::new(Vec::new()),
        }
    }

    pub fn collection_count(&self) -> u64 {
        self.collections.load(Ordering::Acquire)
    }

    pub fn begin_collection(&self) {
        self.collections.fetch_add(1, Ordering::AcqRel);
        *self.cycle_start.lock().unwrap() = Some(Instant::now());
    }

    pub fn end_collection(&self) {
        *self.cycle_start.lock().unwrap() = None;
    }

    pub fn pause(&self, reason: ZPauseReason, duration: Duration) {
        let index = reason as usize;
        let mut pauses = self.pauses.lock().unwrap();
        let stats = &mut pauses[index];
        stats.count += 1;
        stats.total_nanos += duration.as_nanos() as u64;
        stats.max_nanos = stats.max_nanos.max(duration.as_nanos() as u64);
        let now = Instant::now();
        *self.last_pause_end.lock().unwrap() = Some(now);
        let mut mmu = self.mmu_pauses.lock().unwrap();
        mmu.push((now - duration, now));
        if mmu.len() > 200 {
            mmu.remove(0);
        }
    }

    pub fn pause_stats(&self, reason: ZPauseReason) -> ZPauseStats {
        self.pauses.lock().unwrap()[reason as usize]
    }

    pub fn average_pause_nanos(&self, reason: ZPauseReason) -> u64 {
        let stats = self.pause_stats(reason);
        if stats.count == 0 { 0 } else { stats.total_nanos / stats.count }
    }

    pub fn mmu(&self, time_slice: Duration) -> f64 {
        let pauses = self.mmu_pauses.lock().unwrap();
        if pauses.is_empty() {
            return 1.0;
        }
        let mut total_pause = Duration::ZERO;
        for (start, end) in pauses.iter() {
            let overlap_start = (*start).max(*end - time_slice);
            if *end > overlap_start {
                total_pause += *end - overlap_start;
            }
        }
        let slice_nanos = time_slice.as_nanos() as f64;
        if slice_nanos <= 0.0 {
            1.0
        } else {
            (1.0 - (total_pause.as_nanos() as f64 / slice_nanos)).clamp(0.0, 1.0)
        }
    }
}

impl Default for ZStatistics {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZYoungCollectionType {
    Minor,
    MajorFullPreclean,
    MajorFullRoots,
    MajorPartialRoots,
    None,
}

impl ZYoungCollectionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minor => "Minor",
            Self::MajorFullPreclean => "Major Full Preclean",
            Self::MajorFullRoots => "Major Full Roots",
            Self::MajorPartialRoots => "Major Partial Roots",
            Self::None => "None",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZCollectionCause {
        AllocationStall,
        Proactive,
        MajorGC,
        MetadataGC,
        Other,
}

impl ZCollectionCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllocationStall => "Allocation Stall",
            Self::Proactive => "Proactive",
            Self::MajorGC => "Major GC",
            Self::MetadataGC => "Metadata GC",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ZDriverRequest {
    pub cause: ZCollectionCause,
    pub young_type: ZYoungCollectionType,
    pub force_major: bool,
}

impl Default for ZDriverRequest {
    fn default() -> Self {
        Self { cause: ZCollectionCause::Proactive, young_type: ZYoungCollectionType::Minor, force_major: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZDriverState {
    Idle,
    MinorRunning,
    MajorRunning,
    Shutdown,
}

pub struct ZCollectionPolicy {
    pub young_headroom_percent: u32,
    pub old_headroom_percent: u32,
    pub major_interval: u64,
    pub min_cycle_interval: Duration,
    pub allocation_rate_sample: u64,
}

impl Default for ZCollectionPolicy {
    fn default() -> Self {
        Self {
            young_headroom_percent: 10,
            old_headroom_percent: 20,
            major_interval: 8,
            min_cycle_interval: Duration::from_millis(1),
            allocation_rate_sample: 0,
        }
    }
}

impl ZCollectionPolicy {
    pub fn should_collect_young(&self, used: usize, capacity: usize, allocation_rate: u64) -> bool {
        if capacity == 0 {
            return true;
        }
        let headroom = capacity * self.young_headroom_percent as usize / 100;
        used + headroom >= capacity || allocation_rate > self.allocation_rate_sample
    }

    pub fn should_collect_old(&self, young_collections: u64, live_ratio_percent: u32, used_ratio_percent: u32) -> bool {
        young_collections >= self.major_interval || live_ratio_percent > 70 || used_ratio_percent > 85
    }

    pub fn select_young_type(&self, force_major: bool, remembered_bytes: usize, used_bytes: usize) -> ZYoungCollectionType {
        if force_major {
            return ZYoungCollectionType::MajorFullRoots;
        }
        if remembered_bytes * 4 > used_bytes {
            ZYoungCollectionType::MajorPartialRoots
        } else {
            ZYoungCollectionType::Minor
        }
    }
}

pub struct ZDriver {
    state: Mutex<ZDriverState>,
    gc_cause: Mutex<ZCollectionCause>,
    policy: ZCollectionPolicy,
    statistics: ZStatistics,
    young_collections: AtomicU64,
    old_collections: AtomicU64,
}

impl ZDriver {
    pub fn new(policy: ZCollectionPolicy) -> Self {
        Self {
            state: Mutex::new(ZDriverState::Idle),
            gc_cause: Mutex::new(ZCollectionCause::Proactive),
            policy,
            statistics: ZStatistics::new(),
            young_collections: AtomicU64::new(0),
            old_collections: AtomicU64::new(0),
        }
    }

    pub fn state(&self) -> ZDriverState {
        *self.state.lock().unwrap()
    }

    pub fn set_state(&self, state: ZDriverState) {
        *self.state.lock().unwrap() = state;
    }

    pub fn set_gc_cause(&self, cause: ZCollectionCause) {
        *self.gc_cause.lock().unwrap() = cause;
    }

    pub fn gc_cause(&self) -> ZCollectionCause {
        *self.gc_cause.lock().unwrap()
    }

    pub fn statistics(&self) -> &ZStatistics {
        &self.statistics
    }

    pub fn policy(&self) -> &ZCollectionPolicy {
        &self.policy
    }

    pub fn young_collections(&self) -> u64 {
        self.young_collections.load(Ordering::Acquire)
    }

    pub fn old_collections(&self) -> u64 {
        self.old_collections.load(Ordering::Acquire)
    }

    pub fn request_collection(&self, request: ZDriverRequest) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state != ZDriverState::Idle {
            return false;
        }
        *state = if request.force_major { ZDriverState::MajorRunning } else { ZDriverState::MinorRunning };
        drop(state);
        self.statistics.begin_collection();
        true
    }

    pub fn finish_collection(&self, major: bool) {
        if major {
            self.old_collections.fetch_add(1, Ordering::AcqRel);
        } else {
            self.young_collections.fetch_add(1, Ordering::AcqRel);
        }
        self.statistics.end_collection();
        *self.state.lock().unwrap() = ZDriverState::Idle;
    }

    pub fn generation_for_request(&self, request: &ZDriverRequest) -> ZGenerationId {
        if request.force_major { ZGenerationId::Old } else { ZGenerationId::Young }
    }
}

impl Default for ZDriver {
    fn default() -> Self {
        Self::new(ZCollectionPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_record_pause() {
        let stats = ZStatistics::new();
        stats.pause(ZPauseReason::MarkStart, Duration::from_micros(100));
        stats.pause(ZPauseReason::MarkStart, Duration::from_micros(300));
        assert_eq!(stats.pause_stats(ZPauseReason::MarkStart).count, 2);
        assert_eq!(stats.average_pause_nanos(ZPauseReason::MarkStart), 200_000);
    }

    #[test]
    fn policy_selects_collection_type() {
        let policy = ZCollectionPolicy::default();
        assert_eq!(policy.select_young_type(false, 0, 100), ZYoungCollectionType::Minor);
        assert_eq!(policy.select_young_type(true, 0, 100), ZYoungCollectionType::MajorFullRoots);
    }
}
