/*
 * Port of OpenJDK 25 HotSpot MemoryFileTracker
 * (share/nmt/memoryFileTracker.hpp/.cpp)
 *
 * Tracks malloc-like allocations backed by memory-mapped files
 * (e.g. perfdata, .hprof, CDS archive mapping). Allocations are
 * charged to the VirtualMemoryTracker summary so they appear in the
 * "reserved by memory file" totals; detail reports list the files.
 */

use super::memTag::MemTag;
use super::nmtCommon::NmtTrackingLevel;
use super::virtualMemoryTracker::VirtualMemorySnapshot;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One tracked file allocation (C++ MemoryFile). addr is the mapped
/// base, size is the file size; tag is the accounting category.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub addr: usize,
    pub size: usize,
    pub tag: MemTag,
}

/// Per-file statistics used by reporters (C++ MemIOStatFunctionData).
#[derive(Debug, Clone, Copy, Default)]
pub struct MemIoStatFunctionData {
    pub read_ops: usize,
    pub write_ops: usize,
    pub read_bytes: usize,
    pub write_bytes: usize,
    pub map_ops: usize,
    pub unmap_ops: usize,
    pub map_bytes: usize,
    pub unmap_bytes: usize,
}

struct MemoryFileTrackerInner {
    /// Files keyed by mapped base address.
    files: BTreeMap<usize, MemoryFile>,
    /// Total bytes mapped from files, per tag.
    reserved_by_tag: Vec<usize>,
    committed_by_tag: Vec<usize>,
}

impl MemoryFileTrackerInner {
    fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            reserved_by_tag: vec![0; MemTag::MtNumberOfTags as usize],
            committed_by_tag: vec![0; MemTag::MtNumberOfTags as usize],
        }
    }
}

/// MemoryFileTracker — singleton (C++ global MemoryFileTracker).
pub struct MemoryFileTrackerInstance {
    inner: Mutex<MemoryFileTrackerInner>,
    io_stats: Mutex<BTreeMap<usize, MemIoStatFunctionData>>,
    active: AtomicUsize,
}

impl MemoryFileTrackerInstance {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryFileTrackerInner::new()),
            io_stats: Mutex::new(BTreeMap::new()),
            active: AtomicUsize::new(0),
        }
    }

    /// The global tracker (always exists; tracking gated by level).
    pub fn global() -> &'static Self {
        static INSTANCE: std::sync::OnceLock<MemoryFileTrackerInstance> = std::sync::OnceLock::new();
        INSTANCE.get_or_init(Self::new)
    }

    /// Whether file allocation tracking is enabled at the current level.
    fn tracking_enabled() -> bool {
        super::memTracker::MemTracker::is_tracking_on()
            && super::memTracker::MemTracker::tracking_level()
                >= NmtTrackingLevel::Summary
    }

    /// Record a file mapping [addr, addr+size) charged to `tag`.
    /// Returns false if tracking is off or the region overlaps an
    /// existing file mapping.
    pub fn add_memory_file(&self, addr: usize, size: usize, tag: MemTag) -> bool {
        if !Self::tracking_enabled() || size == 0 {
            return false;
        }
        let mut inner = self.inner.lock();
        if let Some((&k, _)) = inner.files.range(..=addr).next_back() {
            let f = &inner.files[&k];
            if addr < f.addr + f.size {
                return false;
            }
        }
        if let Some((&k, _)) = inner.files.range(addr + 1..).next() {
            let f = &inner.files[&k];
            if addr + size > f.addr {
                return false;
            }
        }
        inner.reserved_by_tag[NmtTagIndex::of(tag)] += size;
        inner.committed_by_tag[NmtTagIndex::of(tag)] += size;
        inner.files.insert(addr, MemoryFile { addr, size, tag });
        drop(inner);
        self.active.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Remove a file mapping.
    pub fn remove_memory_file(&self, addr: usize, size: usize) {
        let mut inner = self.inner.lock();
        let Some(&key) = inner.files.get_key_value(&addr).map(|(k, _)| k).or_else(|| {
            inner
                .files
                .range(..=addr)
                .next_back()
                .map(|(&k, _)| k)
                .filter(|&k| inner.files[&k].addr + inner.files[&k].size > addr)
        }) else {
            return;
        };
        let f = inner.files.remove(&key).unwrap();
        let tag_idx = NmtTagIndex::of(f.tag);
        inner.reserved_by_tag[tag_idx] = inner.reserved_by_tag[tag_idx].saturating_sub(size.min(f.size));
        inner.committed_by_tag[tag_idx] = inner.committed_by_tag[tag_idx].saturating_sub(size.min(f.size));
        drop(inner);
        self.active.fetch_sub(1, Ordering::Relaxed);
    }

    /// Fold file-backed memory into a VM snapshot (reserved == committed
    /// for file mappings).
    pub fn summary_snapshot(&self, snapshot: &mut VirtualMemorySnapshot) {
        let inner = self.inner.lock();
        for tag in super::memTag::MemTag::all() {
            let i = NmtTagIndex::of(tag);
            let v = snapshot.by_tag_mut(tag);
            let add = inner.reserved_by_tag[i];
            v.reserve_memory(add);
            v.commit_memory(add);
        }
    }

    /// All tracked files (for detail reports).
    pub fn files(&self) -> Vec<MemoryFile> {
        self.inner.lock().files.values().cloned().collect()
    }

    /// I/O statistics for a mapped file (C++ record_mem_io_stat).
    pub fn record_io_stat(&self, addr: usize, read_ops: usize, write_ops: usize, read_bytes: usize, write_bytes: usize) {
        let mut stats = self.io_stats.lock();
        let e = stats.entry(addr).or_default();
        e.read_ops += read_ops;
        e.write_ops += write_ops;
        e.read_bytes += read_bytes;
        e.write_bytes += write_bytes;
    }

    /// All I/O statistics.
    pub fn io_stats(&self) -> Vec<(usize, MemIoStatFunctionData)> {
        self.io_stats.lock().iter().map(|(&k, &v)| (k, v)).collect()
    }

    /// Number of tracked file mappings.
    pub fn file_count(&self) -> usize {
        self.inner.lock().files.len()
    }
}

/// Internal helper: tag -> vector index (keeps the call sites short).
mod NmtTagIndex {
    pub fn of(tag: super::memTag::MemTag) -> usize {
        tag as usize
    }
}
