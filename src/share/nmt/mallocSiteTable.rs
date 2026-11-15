/*
 * Port of OpenJDK 25 HotSpot mallocSiteTable (share/nmt/mallocSiteTable.hpp/.cpp)
 *
 * Native memory tracking call site table — only used at NMT detail level.
 * Maps a NativeCallStack + MemTag to a MallocSite holding a MemoryCounter.
 * Entries are never removed; each bucket is an insertion-ordered list so
 * the (bucket, position) marker encoding stays stable.
 */

use super::allocationSite::AllocationSite;
use super::mallocTracker::MemoryCounter;
use super::memTag::MemTag;
use super::nativeCallStack::NativeCallStack;
use std::sync::{Mutex, OnceLock};

/// MallocSite — a code path that calls os::malloc(), with its counters.
pub struct MallocSite {
    site: AllocationSite,
    counter: MemoryCounter,
}

impl MallocSite {
    pub fn new(stack: NativeCallStack, mem_tag: MemTag) -> Self {
        Self { site: AllocationSite::new(stack, mem_tag), counter: MemoryCounter::new() }
    }

    pub fn allocate(&self, size: usize) {
        self.counter.allocate(size);
    }

    pub fn deallocate(&self, size: usize) {
        self.counter.deallocate(size);
    }

    pub fn size(&self) -> usize {
        self.counter.size()
    }

    pub fn peak_size(&self) -> usize {
        self.counter.peak_size()
    }

    pub fn count(&self) -> usize {
        self.counter.count()
    }

    pub fn counter(&self) -> &MemoryCounter {
        &self.counter
    }

    pub fn call_stack(&self) -> &NativeCallStack {
        self.site.call_stack()
    }

    pub fn mem_tag(&self) -> MemTag {
        self.site.mem_tag()
    }
}

struct MallocSiteHashtableEntry {
    site: MallocSite,
    hash: u32,
}

const TABLE_SIZE: usize = 4099;
const MAX_BUCKET_LENGTH: usize = u16::MAX as usize - 1;

fn build_marker(bucket_idx: usize, pos_idx: usize) -> u32 {
    debug_assert!(bucket_idx <= u16::MAX as usize && pos_idx < MAX_BUCKET_LENGTH);
    ((bucket_idx as u32) << 16) | pos_idx as u32
}

fn bucket_idx_from_marker(marker: u32) -> usize {
    (marker >> 16) as usize
}

fn pos_idx_from_marker(marker: u32) -> usize {
    (marker & 0xFFFF) as usize
}

/// MallocSiteTable — hashtable of allocation sites, one per bucket chain.
pub struct MallocSiteTable {
    table: Vec<Mutex<Vec<MallocSiteHashtableEntry>>>,
}

static GLOBAL_TABLE: OnceLock<MallocSiteTable> = OnceLock::new();

impl MallocSiteTable {
    pub fn global() -> &'static Self {
        GLOBAL_TABLE.get_or_init(|| Self {
            table: (0..TABLE_SIZE).map(|_| Mutex::new(Vec::new())).collect(),
        })
    }

    pub fn initialize() -> bool {
        let _ = Self::global();
        true
    }

    #[inline]
    pub fn hash_buckets() -> usize {
        TABLE_SIZE
    }

    #[inline]
    fn hash_to_index(hash: u32) -> usize {
        (hash as usize) % TABLE_SIZE
    }

    /// Record a new allocation from the specified call path.
    /// Returns the site marker (0 = failure, matching C++ semantics where
    /// marker stays 0 on failure).
    pub fn allocation_at(stack: &NativeCallStack, size: usize, mem_tag: MemTag) -> u32 {
        let table = Self::global();
        let hash = stack.calculate_hash();
        let index = Self::hash_to_index(hash);

        // Fast path: bucket already populated — look for existing site.
        {
            let bucket = table.table[index].lock().unwrap();
            if !bucket.is_empty() {
                for (pos, entry) in bucket.iter().enumerate() {
                    if entry.hash == hash
                        && entry.site.mem_tag() == mem_tag
                        && entry.site.call_stack().equals(stack)
                    {
                        let marker = build_marker(index, pos);
                        entry.site.allocate(size);
                        return marker;
                    }
                }
            }
        }

        // Insert a new entry at the end of the bucket chain.
        let mut bucket = table.table[index].lock().unwrap();
        if bucket.len() >= MAX_BUCKET_LENGTH {
            return 0;
        }
        let pos = bucket.len();
        let entry = MallocSiteHashtableEntry {
            site: MallocSite::new(*stack, mem_tag),
            hash,
        };
        entry.site.allocate(size);
        bucket.push(entry);
        build_marker(index, pos)
    }

    /// Record memory deallocation. `marker` indicates where the
    /// allocation information was recorded.
    pub fn deallocation_at(size: usize, marker: u32) {
        if marker == 0 {
            return;
        }
        let table = Self::global();
        let bucket_idx = bucket_idx_from_marker(marker);
        let pos_idx = pos_idx_from_marker(marker);
        if bucket_idx >= TABLE_SIZE {
            return;
        }
        let bucket = table.table[bucket_idx].lock().unwrap();
        if let Some(entry) = bucket.get(pos_idx) {
            entry.site.deallocate(size);
        }
    }

    /// Access the call stack recorded under `marker`.
    pub fn access_stack(marker: u32) -> Option<NativeCallStack> {
        if marker == 0 {
            return None;
        }
        let table = Self::global();
        let bucket_idx = bucket_idx_from_marker(marker);
        let pos_idx = pos_idx_from_marker(marker);
        if bucket_idx >= TABLE_SIZE {
            return None;
        }
        let bucket = table.table[bucket_idx].lock().unwrap();
        bucket.get(pos_idx).map(|e| *e.site.call_stack())
    }

    /// Walk all sites; `f` returns false to stop the walk.
    pub fn walk<F: FnMut(&MallocSite) -> bool>(&self, mut f: F) {
        for bucket in &self.table {
            let bucket = bucket.lock().unwrap();
            for entry in bucket.iter() {
                if entry.site.size() > 0 && !f(&entry.site) {
                    return;
                }
            }
        }
    }

    /// All malloc sites sorted by descending size (MemBaseline order).
    pub fn sites_by_size(&self) -> Vec<MallocSiteSnapshot> {
        let mut out = Vec::new();
        self.walk(|site| {
            out.push(MallocSiteSnapshot::from_site(site));
            true
        });
        out.sort_by(|a, b| b.size.cmp(&a.size));
        out
    }

    /// Clear all sites (tests / shutdown).
    pub fn reset(&self) {
        for bucket in &self.table {
            bucket.lock().unwrap().clear();
        }
    }

    /// Render the detail "malloc site" section used by reports.
    pub fn detail_section(&self) -> String {
        let mut out = String::new();
        let sites = self.sites_by_size();
        if sites.is_empty() {
            out.push_str("  (no malloc sites recorded)\n");
            return out;
        }
        for site in sites {
            out.push_str(&format!(
                "  {}: {} bytes in {} allocations\n",
                site.mem_tag.name(),
                site.size,
                site.count
            ));
            out.push_str(&site.stack.print_on());
        }
        out
    }

    /// Tuning statistics (jcmd VM.native_memory statistics).
    pub fn tuning_statistics(&self) -> String {
        let mut total_entries = 0usize;
        let mut empty_entries = 0usize;
        let mut lengths: Vec<usize> = Vec::with_capacity(TABLE_SIZE);
        let mut unused_buckets = 0usize;
        let mut depth_hist = [0usize; super::nmtCommon::NMT_TRACKING_STACK_DEPTH + 1];

        for bucket in &self.table {
            let bucket = bucket.lock().unwrap();
            if bucket.is_empty() {
                unused_buckets += 1;
            }
            let mut chain = 0usize;
            for entry in bucket.iter() {
                total_entries += 1;
                chain += 1;
                if entry.site.size() == 0 {
                    empty_entries += 1;
                }
                let d = entry.site.call_stack().frames().min(super::nmtCommon::NMT_TRACKING_STACK_DEPTH);
                depth_hist[d] += 1;
            }
            lengths.push(chain.min(u16::MAX as usize));
        }
        lengths.sort_unstable();

        let mut out = String::new();
        out.push_str("Malloc allocation site table:\n");
        out.push_str(&format!("\tTotal entries: {}\n", total_entries));
        let pct = if total_entries > 0 {
            (empty_entries as f64 * 100.0) / total_entries as f64
        } else {
            0.0
        };
        out.push_str(&format!(
            "\tEmpty entries (no outstanding mallocs): {} ({:.2}%)\n",
            empty_entries, pct
        ));
        out.push_str("\nBucket chain length distribution:\n");
        out.push_str(&format!("unused:  {}\n", unused_buckets));
        out.push_str(&format!(
            "longest: {}\nmedian:  {}\n",
            lengths.last().copied().unwrap_or(0),
            lengths.get(lengths.len() / 2).copied().unwrap_or(0)
        ));
        out.push_str("\nCall stack depth distribution:\n");
        for (i, &n) in depth_hist.iter().enumerate() {
            out.push_str(&format!("\t{}: {}\n", i, n));
        }
        out.push('\n');
        out
    }
}

/// Immutable view of a MallocSite used by baselines and reports.
#[derive(Clone)]
pub struct MallocSiteSnapshot {
    pub stack: NativeCallStack,
    pub mem_tag: MemTag,
    pub size: usize,
    pub peak_size: usize,
    pub count: usize,
    pub peak_count: usize,
}

impl MallocSiteSnapshot {
    fn from_site(site: &MallocSite) -> Self {
        Self {
            stack: *site.call_stack(),
            mem_tag: site.mem_tag(),
            size: site.size(),
            peak_size: site.peak_size(),
            count: site.count(),
            peak_count: site.counter().peak_count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_allocation_and_markers() {
        let _g = crate::share::nmt::nmtCommon::NMT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let table = MallocSiteTable::global();
        let stack = NativeCallStack::from_frames(&[0xaaaa, 0xbbbb]);
        let m1 = MallocSiteTable::allocation_at(&stack, 100, MemTag::MtTest);
        let m2 = MallocSiteTable::allocation_at(&stack, 50, MemTag::MtTest);
        assert_ne!(m1, 0);
        assert_eq!(m1, m2);
        let sites = table.sites_by_size();
        let found = sites.iter().find(|s| s.mem_tag == MemTag::MtTest && s.stack == stack);
        assert!(found.is_some());
        assert_eq!(found.unwrap().size, 150);
        MallocSiteTable::deallocation_at(150, m1);
        let sites2 = table.sites_by_size();
        assert!(sites2.iter().all(|s| !(s.mem_tag == MemTag::MtTest && s.stack == stack && s.size == 150)));
    }
}
