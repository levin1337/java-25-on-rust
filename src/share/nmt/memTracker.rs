/*
 * Port of OpenJDK 25 HotSpot MemTracker (share/nmt/memTracker.hpp/.cpp)
 *
 * Native Memory Tracking (NMT): records malloc and VM (reserve/commit)
 * allocations per MemTag and produces "Native Memory Tracking" reports.
 *
 * Tracking levels mirror -XX:NativeMemoryTracking={off,summary,detail}:
 *   off     — no accounting
 *   summary — per-tag totals only
 *   detail  — per-tag totals + call-site granularity (approximated by
 *             recording the allocating thread id as the site key)
 */

use super::memReporter::MemReporter;
use super::memTag::MemTag;
use super::virtualMemoryTracker::{TagVmTotals, VirtualMemoryTracker};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// Backwards-compatible name for the tag enum (earlier ports used
/// MemoryType; the HotSpot name is MemTag).
pub use super::memTag::MemoryType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmtLevel {
    Off,
    Summary,
    Detail,
}

impl NmtLevel {
    pub fn from_flag(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "summary" => Self::Summary,
            "detail" | "detailed" => Self::Detail,
            _ => Self::Summary,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Summary => "summary",
            Self::Detail => "detail",
        }
    }
}

/// Per-call-site detail entry (detail level only).
#[derive(Debug, Clone, Copy, Default)]
pub struct SiteStats {
    pub count: u64,
    pub bytes: usize,
}

pub struct MemTracker {
    /// malloc-side committed bytes per tag.
    allocations: Mutex<HashMap<MemTag, usize>>,
    /// detail level: (tag, site_key) → stats. site_key approximates
    /// NativeCallStack by using the allocating thread id.
    sites: Mutex<HashMap<(MemTag, u64), SiteStats>>,
}

static GLOBAL_MEM_TRACKER: OnceLock<MemTracker> = OnceLock::new();
static TRACKING_LEVEL: AtomicU8 = AtomicU8::new(NmtLevel::Off as u8);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

impl MemTracker {
    fn new() -> Self {
        Self {
            allocations: Mutex::new(HashMap::new()),
            sites: Mutex::new(HashMap::new()),
        }
    }

    /// memTracker.cpp: MemTracker::initialize() — level comes from
    /// -XX:NativeMemoryTracking (default summary when enabled via flag).
    pub fn initialize(level: NmtLevel) {
        TRACKING_LEVEL.store(level as u8, Ordering::Relaxed);
        INITIALIZED.store(true, Ordering::Relaxed);
    }

    pub fn initialize_from_flag(value: Option<&str>) -> NmtLevel {
        let level = value.map(NmtLevel::from_flag).unwrap_or(NmtLevel::Summary);
        Self::initialize(level);
        level
    }

    #[inline]
    pub fn tracking_level() -> NmtLevel {
        match TRACKING_LEVEL.load(Ordering::Relaxed) {
            1 => NmtLevel::Summary,
            2 => NmtLevel::Detail,
            _ => NmtLevel::Off,
        }
    }

    #[inline]
    pub fn is_initialized() -> bool {
        INITIALIZED.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn is_active() -> bool {
        Self::tracking_level() != NmtLevel::Off
    }

    pub fn global() -> &'static MemTracker {
        GLOBAL_MEM_TRACKER.get_or_init(Self::new)
    }

    #[inline]
    pub fn level(&self) -> NmtLevel {
        Self::tracking_level()
    }

    // -- malloc accounting ------------------------------------------------------

    pub fn record_malloc(&self, size: usize, mem_type: MemTag) {
        if Self::tracking_level() == NmtLevel::Off || size == 0 {
            return;
        }
        *self.allocations.lock().unwrap().entry(mem_type).or_insert(0) += size;
        if Self::tracking_level() == NmtLevel::Detail {
            let site = Self::current_site_key();
            let mut sites = self.sites.lock().unwrap();
            let e = sites.entry((mem_type, site)).or_default();
            e.count += 1;
            e.bytes += size;
        }
    }

    pub fn record_free(&self, size: usize, mem_type: MemTag) {
        if Self::tracking_level() == NmtLevel::Off {
            return;
        }
        if let Some(entry) = self.allocations.lock().unwrap().get_mut(&mem_type) {
            *entry = entry.saturating_sub(size);
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
        if Self::is_active() {
            VirtualMemoryTracker::global().record_reserve(base, bytes, tag);
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
        if Self::is_active() {
            VirtualMemoryTracker::global().record_commit(addr, bytes, tag);
        }
        Ok(())
    }

    /// NMT-aware uncommit.
    #[cfg(windows)]
    pub fn uncommit_memory(&self, addr: usize, bytes: usize) -> std::io::Result<()> {
        use crate::share::platform::windows::OsWindows;
        OsWindows::uncommit_memory(addr, bytes)?;
        if Self::is_active() {
            VirtualMemoryTracker::global().record_uncommit(addr, bytes);
        }
        Ok(())
    }

    /// NMT-aware release.
    #[cfg(windows)]
    pub fn release_memory(&self, addr: usize, bytes: usize) -> std::io::Result<()> {
        use crate::share::platform::windows::OsWindows;
        OsWindows::release_memory(addr, bytes)?;
        if Self::is_active() {
            VirtualMemoryTracker::global().record_release(addr, bytes);
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

    pub fn total_allocated(&self) -> usize {
        self.allocations.lock().unwrap().values().sum()
    }

    pub fn allocated_for_type(&self, mem_type: MemTag) -> usize {
        self.allocations.lock().unwrap().get(&mem_type).copied().unwrap_or(0)
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

    pub fn detail_report(&self) -> String {
        let mut out = self.generate_report();
        out.push_str("\nCall site details (detail level):\n");
        let sites = self.sites.lock().unwrap();
        let mut entries: Vec<_> = sites.iter().collect();
        entries.sort_by_key(|(_, s)| std::cmp::Reverse(s.bytes));
        for ((tag, site), stats) in entries.iter().take(64) {
            out.push_str(&format!(
                "  {:<20} site={:#x} count={} bytes={}KB\n",
                tag.name(),
                site,
                stats.count,
                stats.bytes / 1024
            ));
        }
        out
    }

    /// NMT diagnostic command output (jcmd VM.native_memory).
    pub fn native_memory_report(detail: bool) -> String {
        let tracker = Self::global();
        if detail && Self::tracking_level() == NmtLevel::Detail {
            tracker.detail_report()
        } else {
            tracker.summary_report()
        }
    }

    pub fn reset(&self) {
        self.allocations.lock().unwrap().clear();
        self.sites.lock().unwrap().clear();
        VirtualMemoryTracker::global().reset();
    }

    /// Site key: thread id approximates the NativeCallStack site grouping at
    /// detail level (documented simplification).
    fn current_site_key() -> u64 {
        #[cfg(windows)]
        {
            crate::share::platform::windows::OsWindows::current_thread_id() as u64
        }
        #[cfg(not(windows))]
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            use std::collections::HashMap;
            use std::sync::Mutex;
            static MAP: Mutex<Option<HashMap<std::thread::ThreadId, u64>>> = Mutex::new(None);
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let tid = std::thread::current().id();
            let mut guard = MAP.lock().unwrap();
            let map = guard.get_or_insert_with(HashMap::new);
            *map.entry(tid).or_insert_with(|| NEXT.fetch_add(1, Ordering::Relaxed))
        }
    }
}

impl Default for MemTracker {
    fn default() -> Self {
        // Default-off like HotSpot until -XX:NativeMemoryTracking is parsed.
        Self::initialize(NmtLevel::Off);
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(NmtLevel::from_flag("summary"), NmtLevel::Summary);
        assert_eq!(NmtLevel::from_flag("detail"), NmtLevel::Detail);
        assert_eq!(NmtLevel::from_flag("off"), NmtLevel::Off);
    }

    #[test]
    fn record_and_report() {
        // Force summary level for this test process.
        MemTracker::initialize(NmtLevel::Summary);
        let t = MemTracker::global();
        t.record_malloc(4096, MemTag::MtSymbol);
        t.record_malloc(2048, MemTag::MtSymbol);
        assert_eq!(t.allocated_for_type(MemTag::MtSymbol), 6144);
        let report = t.summary_report();
        assert!(report.contains("Native Memory Tracking"));
        t.record_free(1024, MemTag::MtSymbol);
        assert_eq!(t.allocated_for_type(MemTag::MtSymbol), 5120);
    }
}
