/*
 * Port of OpenJDK 25 HotSpot MemTracker (share/nmt/memTracker.hpp/.cpp)
 *
 * Native Memory Tracking (NMT): records malloc and VM (reserve/commit)
 * allocations per MemTag and produces "Native Memory Tracking" reports.
 *
 * Tracking levels mirror -XX:NativeMemoryTracking={off,summary,detail}:
 *   off     — no accounting
 *   summary — per-tag totals only
 *   detail  — per-tag totals + per-call-site granularity
 */

use super::memReporter::MemReporter;
use super::memTag::MemTag;
use super::mallocTracker::{FreeInfo, MallocHeader, MallocTracker};
use super::nativeCallStack::{self, NativeCallStack};
use super::nmtCommon::NmtUtil;
use super::virtualMemoryTracker::{TagVmTotals, VirtualMemoryTracker};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

/// Backwards-compatible name for the tag enum (earlier ports used
/// MemoryType; the HotSpot name is MemTag).
pub use super::memTag::MemoryType;

/// Tracking level type (C++ NMT_TrackingLevel). The name NmtLevel is
/// kept for backwards compatibility with earlier ports.
pub use super::nmtCommon::{NmtLevel, NmtTrackingLevel};

pub struct MemTracker;

static TRACKING_LEVEL: AtomicU8 = AtomicU8::new(NmtTrackingLevel::Unknown as u8);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static GENERATION_BLOCK: OnceLock<()> = OnceLock::new();

impl MemTracker {
    /// memTracker.cpp: MemTracker::initialize() — level comes from
    /// -XX:NativeMemoryTracking.
    pub fn initialize(level: NmtTrackingLevel) -> bool {
        if INITIALIZED.swap(true, Ordering::AcqRel) {
            // Already initialized; adopt the requested level.
            TRACKING_LEVEL.store(level as u8, Ordering::Release);
            return true;
        }
        TRACKING_LEVEL.store(level as u8, Ordering::Release);
        let ok = level == NmtTrackingLevel::Off
            || VirtualMemoryTracker::global().initialize(level)
                && MallocTracker::initialize(level);
        let _ = GENERATION_BLOCK.set(());
        ok
    }

    /// Parse -XX:NativeMemoryTracking value. `None` keeps Unknown
    /// (pre-init); explicit "off" disables tracking.
    pub fn initialize_from_flag(value: Option<&str>) -> NmtTrackingLevel {
        let level = NmtUtil::parse_tracking_level(value);
        if level == NmtTrackingLevel::Unknown {
            // Unknown flag value is treated as off (HotSpot errors out;
            // this port degrades gracefully).
            Self::initialize(NmtTrackingLevel::Off);
            return NmtTrackingLevel::Off;
        }
        Self::initialize(level);
        level
    }

    #[inline]
    pub fn tracking_level() -> NmtTrackingLevel {
        match TRACKING_LEVEL.load(Ordering::Acquire) {
            1 => NmtTrackingLevel::Off,
            2 => NmtTrackingLevel::Summary,
            3 => NmtTrackingLevel::Detail,
            _ => NmtTrackingLevel::Unknown,
        }
    }

    #[inline]
    pub fn is_initialized() -> bool {
        INITIALIZED.load(Ordering::Acquire)
    }

    #[inline]
    pub fn is_active() -> bool {
        Self::is_initialized() && Self::tracking_level() != NmtTrackingLevel::Off
    }

    /// NMT tracking enabled at summary or detail level (C++
    /// MemTracker::is_tracking_on). Also true pre-init at Unknown level
    /// only if explicitly requested — callers gate on is_initialized.
    #[inline]
    pub fn is_tracking_on() -> bool {
        Self::tracking_level() >= NmtTrackingLevel::Summary
    }

    pub fn global() -> &'static MemTracker {
        static INSTANCE: OnceLock<MemTracker> = OnceLock::new();
        INSTANCE.get_or_init(|| MemTracker)
    }

    #[inline]
    pub fn level(&self) -> NmtTrackingLevel {
        Self::tracking_level()
    }

    /// Generation-block marker used to reset tracker state between
    /// independent runs (tests). Mirrors the C++ generation-block idiom.
    pub fn generation_block(&self) {
        let _ = GENERATION_BLOCK.get_or_init(|| ());
    }

    // -- malloc accounting ------------------------------------------------------

    /// Record a malloc; returns the MallocHeader handle which must be
    /// passed to record_free_block when the block is freed.
    pub fn record_malloc(&self, size: usize, mem_type: MemTag) -> MallocHeader {
        if !Self::is_tracking_on() || size == 0 {
            return MallocHeader::new(size, mem_type, 0);
        }
        let detail = Self::tracking_level() == NmtTrackingLevel::Detail;
        let stack = nativeCallStack::current_pc(detail);
        MallocTracker::record_malloc(size, mem_type, &stack)
    }

    /// Record a malloc with an explicit call stack (callers that already
    /// captured the site, e.g. arena code).
    pub fn record_malloc_with_stack(
        &self,
        size: usize,
        mem_type: MemTag,
        stack: &NativeCallStack,
    ) -> MallocHeader {
        if !Self::is_tracking_on() || size == 0 {
            return MallocHeader::new(size, mem_type, 0);
        }
        MallocTracker::record_malloc(size, mem_type, stack)
    }

    /// Record free given the header returned by record_malloc.
    pub fn record_free_block(&self, header: &MallocHeader) {
        MallocTracker::record_free_block(header);
    }

    /// Record free given raw (size, tag). Detail-level site accounting
    /// requires the MallocHeader; this form only updates summary
    /// counters (marker 0 → site table no-op).
    pub fn record_free(&self, size: usize, mem_type: MemTag) {
        if !Self::is_tracking_on() || size == 0 {
            return;
        }
        MallocTracker::deaccount(FreeInfo {
            size,
            mem_tag: mem_type,
            mst_marker: 0,
        });
    }

    // -- arena accounting (for arenas / code cache callers) ----------------------

    #[inline]
    pub fn record_new_arena(&self, mem_tag: MemTag) {
        if Self::is_tracking_on() {
            MallocTracker::record_new_arena(mem_tag);
        }
    }

    #[inline]
    pub fn record_arena_free(&self, mem_tag: MemTag) {
        if Self::is_tracking_on() {
            MallocTracker::record_arena_free(mem_tag);
        }
    }

    #[inline]
    pub fn record_arena_size_change(&self, size: isize, mem_tag: MemTag) {
        if Self::is_tracking_on() {
            MallocTracker::record_arena_size_change(size, mem_tag);
        }
    }

    // -- VM accounting (virtualMemoryTracker feed) -------------------------------

    /// NMT-aware reserve: records with VirtualMemoryTracker then performs the
    /// OS reserve via the platform layer.
    #[cfg(windows)]
    pub fn reserve_memory(
        &self,
        addr: usize,
        bytes: usize,
        tag: MemTag,
    ) -> std::io::Result<usize> {
        use crate::share::platform::windows::OsWindows;
        let base = OsWindows::reserve_memory(addr, bytes)?;
        if Self::is_tracking_on() {
            let detail = Self::tracking_level() == NmtTrackingLevel::Detail;
            let stack = nativeCallStack::current_pc(detail);
            VirtualMemoryTracker::global().add_reserved_region(base, bytes, stack, tag);
        }
        Ok(base)
    }

    /// NMT-aware commit.
    #[cfg(windows)]
    pub fn commit_memory(
        &self,
        addr: usize,
        bytes: usize,
        exec: bool,
        tag: MemTag,
    ) -> std::io::Result<()> {
        use crate::share::platform::windows::OsWindows;
        OsWindows::commit_memory(addr, bytes, exec)?;
        if Self::is_tracking_on() {
            let detail = Self::tracking_level() == NmtTrackingLevel::Detail;
            let stack = nativeCallStack::current_pc(detail);
            VirtualMemoryTracker::global().add_committed_region(addr, bytes, stack);
        }
        Ok(())
    }

    /// NMT-aware uncommit.
    #[cfg(windows)]
    pub fn uncommit_memory(&self, addr: usize, bytes: usize) -> std::io::Result<()> {
        use crate::share::platform::windows::OsWindows;
        OsWindows::uncommit_memory(addr, bytes)?;
        if Self::is_tracking_on() {
            VirtualMemoryTracker::global().remove_uncommitted_region(addr, bytes);
        }
        Ok(())
    }

    /// NMT-aware release.
    #[cfg(windows)]
    pub fn release_memory(&self, addr: usize, bytes: usize) -> std::io::Result<()> {
        use crate::share::platform::windows::OsWindows;
        OsWindows::release_memory(addr, bytes)?;
        if Self::is_tracking_on() {
            VirtualMemoryTracker::global().remove_released_region(addr, bytes);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn reserve_memory(
        &self,
        _addr: usize,
        _bytes: usize,
        _tag: MemTag,
    ) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "NMT VM tracking requires Windows platform layer",
        ))
    }

    // -- reporting ---------------------------------------------------------------

    /// Total malloc-tracked bytes (excl. VM reservations).
    pub fn total_allocated(&self) -> usize {
        super::mallocTracker::MallocMemorySummary::global()
            .snapshot()
            .total()
    }

    /// Malloc-side bytes for a tag (malloc + arena).
    pub fn allocated_for_type(&self, mem_type: MemTag) -> usize {
        let snap = super::mallocTracker::MallocMemorySummary::global().snapshot();
        let m = snap.by_tag(mem_type);
        m.malloc_size() + m.arena_size()
    }

    pub fn vm_totals_for(&self, tag: MemTag) -> TagVmTotals {
        VirtualMemoryTracker::global().totals_for(tag)
    }

    /// memReporter-backed full report.
    pub fn generate_report(&self) -> String {
        MemReporter::new().generate()
    }

    /// Backwards-compatible summary report.
    pub fn summary_report(&self) -> String {
        self.generate_report()
    }

    /// Detail report: summary plus per-site sections.
    pub fn detail_report(&self) -> String {
        let mut out = self.generate_report();
        out.push_str("\nCall Site Detail (malloc):\n");
        out.push_str(&super::mallocSiteTable::MallocSiteTable::global().detail_section());
        out.push_str("\nVirtual Memory Allocation Sites:\n");
        out.push_str(&super::virtualMemoryTracker::VirtualMemoryTracker::global().site_section());
        out
    }

    /// NMT diagnostic command output (jcmd VM.native_memory).
    pub fn native_memory_report(detail: bool) -> String {
        let tracker = Self::global();
        if detail && Self::tracking_level() == NmtTrackingLevel::Detail {
            tracker.detail_report()
        } else {
            tracker.summary_report()
        }
    }

    pub fn reset(&self) {
        VirtualMemoryTracker::global().reset();
        super::mallocSiteTable::MallocSiteTable::global().reset();
        super::mallocTracker::MallocMemorySummary::global().reset();
        super::memoryFileTracker::MemoryFileTrackerInstance::global().reset();
        self.baseline().reset();
    }

    /// Establish a new baseline (MemBaseline owner inside MemTracker,
    /// mirroring MemTracker::set_baseline in C++).
    pub fn baseline(&self) -> std::sync::MutexGuard<'_, super::memBaseline::MemBaseline> {
        static BASELINE: OnceLock<std::sync::Mutex<super::memBaseline::MemBaseline>> = OnceLock::new();
        BASELINE
            .get_or_init(|| std::sync::Mutex::new(super::memBaseline::MemBaseline::new()))
            .lock()
            .unwrap()
    }

    /// Take a fresh baseline at the current tracking level.
    pub fn take_baseline(&self, summary_only: bool) {
        let mut b = self.baseline();
        b.baseline(summary_only);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(NmtUtil::parse_tracking_level(Some("summary")), NmtTrackingLevel::Summary);
        assert_eq!(NmtUtil::parse_tracking_level(Some("detail")), NmtTrackingLevel::Detail);
        assert_eq!(NmtUtil::parse_tracking_level(Some("off")), NmtTrackingLevel::Off);
    }

    #[test]
    fn record_and_report() {
        MemTracker::initialize(NmtTrackingLevel::Summary);
        let t = MemTracker::global();
        let h1 = t.record_malloc(4096, MemTag::MtSymbol);
        let h2 = t.record_malloc(2048, MemTag::MtSymbol);
        assert_eq!(t.allocated_for_type(MemTag::MtSymbol), 6144);
        let report = t.summary_report();
        assert!(report.contains("Native Memory Tracking"));
        t.record_free_block(&h1);
        t.record_free_block(&h2);
        assert_eq!(t.allocated_for_type(MemTag::MtSymbol), 0);
    }
}
