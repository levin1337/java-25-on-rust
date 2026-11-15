/*
 * Port of OpenJDK 25 HotSpot MemReporter (share/nmt/memReporter.hpp/.cpp)
 *
 * Produces the "Native Memory Tracking" diagnostic report in the same
 * section layout as `jcmd VM.native_memory`:
 *
 *   Native Memory Tracking:
 *
 *   Total:   reserved=XXXXKB, committed=YYYYKB
 *   -                 Java Heap (reserved=..., committed=...)
 *                               (mmap: reserved=..., committed=...)
 *                               (malloc: committed=...)
 *   ...
 *
 * Detail reports add per-call-site sections for malloc and virtual
 * memory allocations, plus memory-file I/O statistics.
 */

use super::memBaseline::MemBaseline;
use super::memTag::MemTag;
use super::mallocTracker::MallocMemory;
use super::nmtCommon::{NmtUtil, SCALE_KB};
use super::virtualMemoryTracker::VirtualMemory;

fn kb(bytes: usize) -> String {
    format!("{}KB", bytes / 1024)
}

fn scaled(bytes: usize, scale: usize) -> String {
    format!("{}{}", NmtUtil::amount_in_scale(bytes, scale), NmtUtil::scale_name(scale))
}

/// NMT report writer. Aggregates a MemBaseline (summary counters plus
/// optional detail sites).
pub struct MemReporter<'a> {
    baseline: &'a MemBaseline,
    scale: usize,
}

impl<'a> MemReporter<'a> {
    pub fn new(baseline: &'a MemBaseline) -> Self {
        Self { baseline, scale: SCALE_KB }
    }

    /// Report scale in bytes (1/1024/1024/...).
    pub fn with_scale(mut self, scale: usize) -> Self {
        self.scale = if scale == 0 { SCALE_KB } else { scale };
        self
    }

    fn fmt(&self, bytes: usize) -> String {
        scaled(bytes, self.scale)
    }

    /// Full report at the baseline's level.
    pub fn generate(&self) -> String {
        match self.baseline.baseline_type() {
            super::memBaseline::BaselineType::Detail Baselined => self.generate_detail(),
            _ => self.generate_summary(),
        }
    }

    /// Summary report: totals + per-tag sections.
    pub fn generate_summary(&self) -> String {
        let mut out = String::new();
        out.push_str("Native Memory Tracking:\n\n");
        self.print_totals(&mut out);
        self.print_tag_sections(&mut out, false);
        out.push('\n');
        out
    }

    /// Detail report: summary sections plus call-site breakdowns.
    pub fn generate_detail(&self) -> String {
        let mut out = String::new();
        out.push_str("Native Memory Tracking:\n\n");
        self.print_totals(&mut out);
        self.print_tag_sections(&mut out, true);
        self.print_metaspace(&mut out);
        self.print_mmap_detail(&mut out);
        self.print_malloc_detail(&mut out);
        self.print_memory_file_stats(&mut out);
        out.push('\n');
        out
    }

    fn print_totals(&self, out: &mut String) {
        out.push_str(&format!(
            "Total:   reserved={}, committed={}\n",
            self.fmt(self.baseline.total_reserved_memory()),
            self.fmt(self.baseline.total_committed_memory())
        ));
        let mm = self.baseline.malloc_memory_snapshot();
        out.push_str(&format!(
            "-                 {} (reserved={}, committed={}{})\n",
            "reserved by malloc tracking overhead",
            self.fmt(mm.malloc_overhead()),
            self.fmt(mm.malloc_overhead()),
            ""
        ));
        let arena = self.baseline.total_arena_memory();
        if arena > 0 {
            out.push_str(&format!(
                "-                 {} (reserved={}, committed={})\n",
                "reserved by arena bookkeeping",
                self.fmt(arena),
                self.fmt(arena)
            ));
        }
    }

    /// Per-tag sections in MemTag order (HotSpot report order).
    fn print_tag_sections(&self, out: &mut String, detail: bool) {
        for tag in MemTag::all() {
            if tag == MemTag::MtNone {
                continue; // mtNone carries no usage; skipped like C++.
            }
            let mm = self.baseline.malloc_memory(tag);
            let vm = self.baseline.virtual_memory(tag);
            let malloc_bytes = mm.malloc_size() + mm.arena_size();
            if malloc_bytes == 0 && vm.reserved() == 0 {
                continue;
            }
            let committed = malloc_bytes + vm.committed();
            out.push_str(&format!(
                "- {:<24} (reserved={}, committed={})\n",
                tag.name(),
                self.fmt(malloc_bytes + vm.reserved()),
                self.fmt(committed)
            ));
            self.print_mmap_line(out, vm, tag);
            self.print_malloc_line(out, mm, tag);
            if detail {
                self.print_tag_detail_sites(out, tag);
            }
        }
    }

    fn print_mmap_line(&self, out: &mut String, vm: &VirtualMemory, tag: MemTag) {
        if vm.reserved() == 0 {
            return;
        }
        out.push_str(&format!(
            "                            (mmap: reserved={}, committed={}, peak={})\n",
            self.fmt(vm.reserved()),
            self.fmt(vm.committed()),
            self.fmt(vm.peak_size())
        ));
        let _ = tag;
    }

    fn print_malloc_line(&self, out: &mut String, mm: &MallocMemory, _tag: MemTag) {
        let malloc = mm.malloc_size();
        let arena = mm.arena_size();
        if malloc > 0 {
            out.push_str(&format!(
                "                            (malloc: reserved={}, committed={})\n",
                self.fmt(malloc),
                self.fmt(malloc)
            ));
        }
        if arena > 0 {
            out.push_str(&format!(
                "                            (arena=committed {} in {} chunks)\n",
                self.fmt(arena),
                mm.arena_count()
            ));
        }
    }

    /// Detail: malloc call sites for the tag, sorted by size.
    fn print_tag_detail_sites(&self, out: &mut String, tag: MemTag) {
        let sites: Vec<_> = self
            .baseline_malloc_sites()
            .iter()
            .filter(|s| s.mem_tag == tag)
            .collect();
        if sites.is_empty() {
            return;
        }
        out.push_str(&format!(
            "                            Detailed malloc site information for {}:\n",
            tag.name()
        ));
        for site in sites {
            out.push_str(&format!(
                "                              {} bytes in {} allocations\n",
                self.fmt(site.size),
                site.count
            ));
            out.push_str(&site.stack.print_on());
        }
    }

    fn baseline_malloc_sites(&self) -> &[super::memBaseline::MallocSiteSnapshotView] {
        // The baseline stores MallocSiteSnapshot directly; this helper is
        // only used when detail baselines exist.
        unsafe { std::mem::transmute::<&[super::mallocSiteTable::MallocSiteSnapshot], &[super::memBaseline::MallocSiteSnapshotView]>(self.baseline.malloc_sites_ref()) }
    }

    /// Metaspace section (populated by the VM; zeros omitted).
    fn print_metaspace(&self, out: &mut String) {
        let stats = self.baseline.metaspace_stats();
        let total = stats.non_class.reserved + stats.class_type.reserved;
        if total == 0 {
            return;
        }
        out.push_str(&format!(
            "-              Metaspace (reserved={}, committed={})\n",
            self.fmt(total),
            self.fmt(stats.non_class.committed + stats.class_type.committed)
        ));
    }

    /// Detail: mmap site breakdown by call stack.
    fn print_mmap_detail(&self, out: &mut String) {
        let sites = self.baseline.virtual_memory_sites_for_report();
        if sites.is_empty() {
            return;
        }
        out.push_str("\nMmap site information:\n");
        for site in sites {
            out.push_str(&format!(
                "  {}: reserved={} committed={} peak={}\n",
                site.mem_tag.name(),
                self.fmt(site.reserved),
                self.fmt(site.committed),
                self.fmt(site.peak_size)
            ));
            out.push_str(&site.stack.print_on());
        }
    }

    /// Detail: malloc call-site section (sorted by size).
    fn print_malloc_detail(&self, out: &mut String) {
        let sites = self.baseline.malloc_sites_for_report();
        if sites.is_empty() {
            return;
        }
        out.push_str("\nMalloc site information:\n");
        for site in sites {
            out.push_str(&format!(
                "  {}: {} bytes in {} allocations\n",
                site.mem_tag.name(),
                self.fmt(site.size),
                site.count
            ));
            out.push_str(&site.stack.print_on());
        }
    }

    /// Detail: memory-file I/O statistics (C++ "Memory File Statistics").
    fn print_memory_file_stats(&self, out: &mut String) {
        let files = super::memoryFileTracker::MemoryFileTrackerInstance::global().files();
        if files.is_empty() {
            return;
        }
        out.push_str("\nMemory File Statistics:\n");
        for f in files {
            out.push_str(&format!(
                "  addr={:#x} size={} tag={}\n",
                f.addr,
                self.fmt(f.size),
                f.tag.name()
            ));
        }
    }
}

impl Default for MemReporter<'_> {
    fn default() -> Self {
        // Reports need a baseline; callers should use MemReporter::new.
        // This default is never used — kept for API completeness.
        unreachable!("MemReporter requires a baseline; use MemReporter::new(&baseline)")
    }
}

/// Compatibility wrapper: produces a report from a live MemBaseline.
pub struct LiveMemReporter;

impl LiveMemReporter {
    pub fn generate() -> String {
        let baseline = MemBaseline::new();
        // Summary-only baseline for the compat path.
        let mut b = baseline;
        b.baseline(true);
        MemReporter::new(&b).generate_summary()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::nmt::memBaseline::MemBaseline;
    use crate::share::nmt::memTracker::MemTracker;
    use crate::share::nmt::virtualMemoryTracker::VirtualMemoryTracker;

    #[test]
    fn report_contains_totals() {
        MemTracker::initialize(super::super::nmtCommon::NmtTrackingLevel::Summary);
        let vmt = VirtualMemoryTracker::global();
        let stack = super::super::nativeCallStack::NativeCallStack::empty();
        vmt.add_reserved_region(0x10000, 0x1000, stack, MemTag::MtClass);
        vmt.add_committed_region(0x10000, 0x800, stack);
        let mut b = MemBaseline::new();
        b.baseline(true);
        let report = MemReporter::new(&b).generate();
        assert!(report.contains("Native Memory Tracking"));
        assert!(report.contains("Total:"));
        assert!(report.contains("Class"));
        vmt.reset();
    }

    #[test]
    fn report_format_sections() {
        MemTracker::initialize(super::super::nmtCommon::NmtTrackingLevel::Summary);
        let t = MemTracker::global();
        let h = t.record_malloc(8192, MemTag::MtSymbol);
        t.record_free_block(&h);
        let mut b = MemBaseline::new();
        b.baseline(true);
        let report = MemReporter::new(&b).generate_summary();
        assert!(report.starts_with("Native Memory Tracking:"));
        assert!(report.contains("reserved="));
        assert!(report.contains("committed="));
    }
}
