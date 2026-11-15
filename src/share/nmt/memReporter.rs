/*
 * Port of OpenJDK 25 HotSpot MemReporter / NMT report generation
 * (share/nmt/memReporter.cpp, share/nmt/nmtUsage.hpp)
 *
 * Produces the "Native Memory Tracking" diagnostic report in the same
 * section layout as `jcmd VM.native_memory`:
 *
 *   Native Memory Tracking:
 *
 *   Total:   reserved=XXXXKB, committed=YYYYKB
 *   -                 Java Heap (reserved=..., committed=...)
 *                               (mmap: reserved=..., committed=...)
 *   -                    Class (...)
 *   ...
 */

use super::memTag::MemTag;
use super::memTracker::MemTracker;
use super::virtualMemoryTracker::VirtualMemoryTracker;

fn kb(bytes: usize) -> String {
    format!("{}KB", bytes / 1024)
}

/// NMT report writer. In HotSpot this walks every MallocSite / VM region;
/// here it aggregates the per-tag totals maintained by MemTracker
/// (malloc accounting) and VirtualMemoryTracker (VM reserve/commit).
pub struct MemReporter;

impl MemReporter {
    pub fn new() -> Self {
        Self
    }

    /// Full report text (summary level).
    pub fn generate(&self) -> String {
        let mut out = String::new();
        out.push_str("Native Memory Tracking:\n\n");

        let vmt = VirtualMemoryTracker::global();
        let tracker = MemTracker::global();

        let total_reserved = vmt.total_reserved();
        let total_committed = vmt.total_committed();
        out.push_str(&format!(
            "Total:   reserved={}, committed={}\n",
            kb(total_reserved),
            kb(total_committed)
        ));

        // Per-tag sections in HotSpot report order.
        for tag in MemTag::all() {
            let totals = vmt.totals_for(tag);
            let malloc_bytes = tracker.allocated_for_type(tag);
            if totals.reserved == 0 && totals.committed == 0 && malloc_bytes == 0 {
                continue;
            }
            let name = format!("{:<24}", tag.name());
            out.push_str(&format!(
                "- {:<24} (reserved={}, committed={})\n",
                tag.name(),
                kb(totals.reserved),
                kb(totals.committed.max(malloc_bytes))
            ));
            let _ = name;
            // Sub-lines like HotSpot's "(mmap: ...)" / "(malloc: ...)".
            if totals.reserved > 0 {
                out.push_str(&format!(
                    "                            (mmap: reserved={}, committed={})\n",
                    kb(totals.reserved),
                    kb(totals.committed)
                ));
            }
            if malloc_bytes > 0 {
                out.push_str(&format!(
                    "                            (malloc: committed={})\n",
                    kb(malloc_bytes)
                ));
            }
        }

        out.push_str("\n");
        out
    }

    /// Compact one-line summary used by VM.native_memory summary mode.
    pub fn summary_line() -> String {
        let vmt = VirtualMemoryTracker::global();
        let tracker = MemTracker::global();
        format!(
            "NMT Summary: reserved={}KB committed={}KB malloc_tracked={}KB",
            vmt.total_reserved() / 1024,
            vmt.total_committed() / 1024,
            tracker.total_allocated() / 1024
        )
    }
}

impl Default for MemReporter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::nmt::memTag::MemTag;

    #[test]
    fn report_contains_totals() {
        let vmt = VirtualMemoryTracker::global();
        vmt.record_reserve(0x10000, 8192, MemTag::MtClass);
        vmt.record_commit(0x10000, 4096, MemTag::MtClass);
        let report = MemReporter::new().generate();
        assert!(report.contains("Native Memory Tracking"));
        assert!(report.contains("Class"));
        assert!(report.contains("reserved="));
        vmt.reset();
    }
}
