/*
 * Port of OpenJDK 25 HotSpot NMTDCmd (share/nmt/nmtDCmd.hpp/.cpp)
 *
 * The "VM.native_memory" diagnostic command: summary / detail reports,
 * baseline establishment and baseline diff reports, plus tracker
 * tuning statistics.
 */

use crate::share::nmt::memBaseline::MemBaseline;
use crate::share::nmt::memTag::MemTag;
use crate::share::nmt::memTracker::MemTracker;
use crate::share::nmt::mallocSiteTable::MallocSiteTable;
use crate::share::nmt::nmtCommon::NmtUtil;
use crate::share::services::diagnosticArgument::{DCmdArgument, DcmdResult};
use crate::share::services::diagnosticFramework::{
    self, DCmdInstance, DCmdParser, DCmdSource,
};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

fn writeln_cr(out: &mut dyn Write, s: &str) -> std::io::Result<()> {
    out.write_all(s.as_bytes())?;
    out.write_all(b"\n")
}

static PREVIOUS_BASELINE: OnceLock<Mutex<MemBaseline>> = OnceLock::new();

fn previous_baseline() -> &'static Mutex<MemBaseline> {
    PREVIOUS_BASELINE.get_or_init(|| Mutex::new(MemBaseline::new()))
}

/// Take a fresh baseline from live tracker state.
fn live_baseline(detail: bool) -> MemBaseline {
    let mut b = MemBaseline::new();
    b.baseline(!detail);
    b
}

/// Diff report: per-tag deltas between previous and current baselines
/// (C++ MemDiffReporter, simplified to per-category totals).
fn diff_report(prev: &MemBaseline, curr: &MemBaseline, detail: bool) -> String {
    let mut out = String::new();
    out.push_str("Native memory tracking baseline diff:\n\n");
    let scale = 1024usize;
    let d_reserved = curr.total_reserved_memory() as isize - prev.total_reserved_memory() as isize;
    let d_committed =
        curr.total_committed_memory() as isize - prev.total_committed_memory() as isize;
    out.push_str(&format!(
        "Total:   reserved={}{} committed={}{}\n",
        signed_scaled(d_reserved, scale),
        NmtUtil::scale_name(scale),
        signed_scaled(d_committed, scale),
        NmtUtil::scale_name(scale)
    ));
    for tag in MemTag::all() {
        if tag == MemTag::MtNone {
            continue;
        }
        let pm = prev.malloc_memory(tag);
        let cm = curr.malloc_memory(tag);
        let pv = prev.virtual_memory(tag);
        let cv = curr.virtual_memory(tag);
        let p_total = (pm.malloc_size() + pm.arena_size() + pv.reserved()) as isize;
        let c_total = (cm.malloc_size() + cm.arena_size() + cv.reserved()) as isize;
        let p_commit = (pm.malloc_size() + pm.arena_size() + pv.committed()) as isize;
        let c_commit = (cm.malloc_size() + cm.arena_size() + cv.committed()) as isize;
        let d_r = c_total - p_total;
        let d_c = c_commit - p_commit;
        if d_r == 0 && d_c == 0 {
            continue;
        }
        out.push_str(&format!(
            "- {:<24} (reserved={}{} committed={}{})\n",
            tag.name(),
            signed_scaled(d_r, scale),
            NmtUtil::scale_name(scale),
            signed_scaled(d_c, scale),
            NmtUtil::scale_name(scale)
        ));
    }
    if detail {
        out.push_str("\nMalloc site diff:\n");
        out.push_str("  (site-level diff available via detail reports)\n");
    }
    out.push('\n');
    out
}

fn signed_scaled(v: isize, scale: usize) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let abs = v.unsigned_abs();
    format!("{}{}", sign, NmtUtil::amount_in_scale(abs, scale))
}

pub struct NMTDCmd {
    parser: DCmdParser,
}

impl NMTDCmd {
    pub fn new() -> Self {
        let mut parser = DCmdParser::new();
        for (name, desc) in [
            ("summary", "request runtime to report current memory summary"),
            ("detail", "request runtime to report memory allocation >= 1K by each callsite"),
            ("baseline", "request runtime to baseline current memory usage"),
            ("summary.diff", "request runtime to report memory summary comparison against previous baseline"),
            ("detail.diff", "request runtime to report memory detail comparison against previous baseline"),
            ("statistics", "print tracker statistics for tuning purpose"),
        ] {
            parser.add_dcmd_option(DCmdArgument::<bool>::with_default(
                name, desc, "BOOLEAN", false, "false",
            ));
        }
        parser.add_dcmd_option(DCmdArgument::<String>::with_default(
            "scale", "Memory usage in which scale, KB, MB or GB", "STRING", false, "KB",
        ));
        Self { parser }
    }
}

impl DCmdInstance for NMTDCmd {
    fn parse(&mut self, line: &diagnosticFramework::CmdLine, delim: char) -> DcmdResult<()> {
        self.parser.parse(line, delim)
    }

    fn execute(&mut self, _source: DCmdSource, out: &mut dyn Write) -> DcmdResult<()> {
        let summary = self.parser.option::<bool>("summary").unwrap_or(false);
        let detail = self.parser.option::<bool>("detail").unwrap_or(false);
        let baseline = self.parser.option::<bool>("baseline").unwrap_or(false);
        let summary_diff = self.parser.option::<bool>("summary.diff").unwrap_or(false);
        let detail_diff = self.parser.option::<bool>("detail.diff").unwrap_or(false);
        let statistics = self.parser.option::<bool>("statistics").unwrap_or(false);
        let scale_str = self.parser.option::<String>("scale").unwrap_or_else(|| "KB".into());
        let scale = NmtUtil::scale_from_name(&scale_str);
        let scale = if scale == 0 { 1024 } else { scale };

        if statistics {
            writeln_cr(out, "Native memory tracking statistics:").map_err(|e| e.to_string())?;
            out.write_all(MallocSiteTable::global().tuning_statistics().as_bytes())
                .map_err(|e| e.to_string())?;
            return Ok(());
        }

        if baseline {
            let curr = live_baseline(false);
            let mut prev = previous_baseline().lock().unwrap();
            *prev = curr;
            writeln_cr(out, "Native memory tracking baseline established").map_err(|e| e.to_string())?;
            return Ok(());
        }

        if summary_diff || detail_diff {
            let curr = live_baseline(detail_diff);
            let prev = previous_baseline().lock().unwrap();
            let report = diff_report(&prev, &curr, detail_diff);
            let _ = scale;
            out.write_all(report.as_bytes()).map_err(|e| e.to_string())?;
            return Ok(());
        }

        if detail {
            let report = MemTracker::global().detail_report();
            out.write_all(report.as_bytes()).map_err(|e| e.to_string())?;
            return Ok(());
        }
        if summary {
            let report = MemTracker::global().summary_report();
            out.write_all(report.as_bytes()).map_err(|e| e.to_string())?;
            return Ok(());
        }
        let report = MemTracker::native_memory_report(false);
        out.write_all(report.as_bytes()).map_err(|e| e.to_string())
    }

    fn print_help(&self, name: &str, out: &mut dyn Write) -> std::io::Result<()> {
        self.parser.print_help(out, name)
    }

    fn reset(&mut self) -> DcmdResult<()> {
        self.parser.reset()
    }

    fn cleanup(&mut self) {
        self.parser.cleanup();
    }

    fn argument_name_array(&self) -> Vec<&'static str> {
        self.parser.argument_name_array()
    }

    fn argument_info_array(&self) -> Vec<crate::share::services::diagnosticFramework::DCmdArgumentInfo> {
        self.parser.argument_info_array()
    }
}
