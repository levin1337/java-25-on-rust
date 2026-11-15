/*
 * Port of OpenJDK 25 HotSpot ShenandoahPhaseTimings
 * (share/gc/shenandoah/shenandoahPhaseTimings.hpp / shenandoahPhaseTimings.cpp)
 *
 * Per-cycle and cumulative (global) phase time accounting with
 * HdrSeq global distributions.
 */

use super::shenandoahNumberSeq::HdrSeq;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Phase {
    conc_reset,
    conc_reset_after_collect,
    conc_reset_old,
    init_mark_gross,
    init_mark,
    init_mark_verify,
    init_manage_tlabs,
    init_swap_rset,
    init_transfer_satb,
    init_update_region_states,
    init_propagate_gc_state,
    init_scan_rset,
    conc_mark_roots,
    conc_mark,
    conc_mark_satb_flush,
    final_mark_gross,
    final_mark,
    final_mark_verify,
    finish_mark,
    final_mark_propagate_gc_state,
    purge,
    purge_weak_par,
    final_update_region_states,
    final_manage_labs,
    choose_cset,
    final_rebuild_freeset,
    init_evac,
    conc_thread_roots,
    conc_weak_refs,
    conc_weak_roots,
    conc_weak_roots_work,
    conc_weak_roots_rendezvous,
    conc_cleanup_early,
    conc_class_unload,
    conc_class_unload_unlink,
    conc_class_unload_unlink_sd,
    conc_class_unload_unlink_weak_klass,
    conc_class_unload_unlink_code_roots,
    conc_class_unload_rendezvous,
    conc_class_unload_purge,
    conc_class_unload_purge_coderoots,
    conc_class_unload_purge_cldg,
    conc_class_unload_purge_ec,
    conc_strong_roots,
    conc_evac,
    conc_final_roots,
    promote_in_place,
    final_roots_gross,
    final_roots,
    init_update_refs_gross,
    init_update_refs,
    init_update_refs_verify,
    conc_update_refs_prepare,
    conc_update_refs,
    conc_update_thread_roots,
    final_update_refs_gross,
    final_update_refs,
    final_update_refs_verify,
    final_update_refs_update_region_states,
    final_update_refs_transfer_satb,
    final_update_refs_trash_cset,
    final_update_refs_rebuild_freeset,
    final_update_refs_propagate_gc_state,
    conc_cleanup_complete,
    conc_coalesce_and_fill,
    degen_gc_gross,
    degen_gc,
    degen_gc_stw_mark,
    degen_gc_mark,
    degen_gc_purge,
    degen_gc_weakrefs,
    degen_gc_purge_class_unload,
    degen_gc_purge_weak_par,
    degen_gc_purge_cldg,
    degen_gc_final_update_region_states,
    degen_gc_final_manage_labs,
    degen_gc_choose_cset,
    degen_gc_final_rebuild_freeset,
    degen_gc_stw_evac,
    degen_gc_init_update_refs_manage_gclabs,
    degen_gc_update_refs,
    degen_gc_final_update_refs_trash_cset,
    degen_gc_final_update_refs_rebuild_freeset,
    degen_gc_update_roots,
    degen_gc_cleanup_complete,
    degen_gc_promote_regions,
    degen_gc_coalesce_and_fill,
    degen_gc_propagate_gc_state,
    full_gc_gross,
    full_gc,
    full_gc_heapdump_pre,
    full_gc_prepare,
    full_gc_update_roots,
    full_gc_mark,
    full_gc_purge,
    full_gc_weakrefs,
    full_gc_purge_class_unload,
    full_gc_purge_weak_par,
    full_gc_purge_cldg,
    full_gc_calculate_addresses,
    full_gc_calculate_addresses_regular,
    full_gc_calculate_addresses_humong,
    full_gc_adjust_pointers,
    full_gc_adjust_roots,
    full_gc_copy_objects,
    full_gc_copy_objects_regular,
    full_gc_copy_objects_humong,
    full_gc_recompute_generation_usage,
    full_gc_copy_objects_reset_complete,
    full_gc_copy_objects_rebuild,
    full_gc_reconstruct_remembered_set,
    full_gc_heapdump_post,
    full_gc_propagate_gc_state,
    pacing,
    heap_iteration_roots,
    NumPhases,
}

pub const NUM_PHASES: usize = Phase::NumPhases as usize;

const PHASE_NAMES: [&str; NUM_PHASES] = [
    "Concurrent Reset",
    "Concurrent Reset After Collect",
    "Concurrent Reset (OLD)",
    "Pause Init Mark (G)",
    "Pause Init Mark (N)",
    "  Verify",
    "  Manage TLABs",
    "  Swap Remembered Set",
    "  Transfer Old From SATB",
    "  Update Region States",
    "  Propagate GC State",
    "Concurrent Scan Remembered Set",
    "Concurrent Mark Roots",
    "Concurrent Marking",
    "  Flush SATB",
    "Pause Final Mark (G)",
    "Pause Final Mark (N)",
    "  Verify",
    "  Finish Mark",
    "  Propagate GC State",
    "  System Purge",
    "    Weak Roots",
    "  Update Region States",
    "  Manage GC/TLABs",
    "  Choose Collection Set",
    "  Rebuild Free Set",
    "  Initial Evacuation",
    "Concurrent Thread Roots",
    "Concurrent Weak References",
    "Concurrent Weak Roots",
    "  Roots",
    "  Rendezvous",
    "Concurrent Cleanup",
    "Concurrent Class Unloading",
    "  Unlink Stale",
    "    System Dictionary",
    "    Weak Class Links",
    "    Code Roots",
    "  Rendezvous",
    "  Purge Unlinked",
    "    Code Roots",
    "    CLDG",
    "    Exception Caches",
    "Concurrent Strong Roots",
    "Concurrent Evacuation",
    "Concurrent Final Roots",
    "  Promote Regions",
    "Pause Verify Final Roots (G)",
    "Pause Verify Final Roots (N)",
    "Pause Init Update Refs (G)",
    "Pause Init Update Refs (N)",
    "  Verify",
    "Concurrent Update Refs Prepare",
    "Concurrent Update Refs",
    "Concurrent Update Thread Roots",
    "Pause Final Update Refs (G)",
    "Pause Final Update Refs (N)",
    "  Verify",
    "  Update Region States",
    "  Transfer Old From SATB",
    "  Trash Collection Set",
    "  Rebuild Free Set",
    "  Propagate GC State",
    "Concurrent Cleanup",
    "Concurrent Coalesce and Fill",
    "Pause Degenerated GC (G)",
    "Pause Degenerated GC (N)",
    "  Degen STW Mark",
    "  Degen Mark",
    "    System Purge",
    "      Weak References",
    "      Unload Classes",
    "      Weak Roots",
    "      CLDG",
    "  Update Region States",
    "  Manage GC/TLABs",
    "  Choose Collection Set",
    "  Rebuild Free Set",
    "  Evacuation",
    "  Manage GCLABs",
    "  Update References",
    "  Trash Collection Set",
    "  Rebuild Free Set",
    "  Degen Update Roots",
    "  Cleanup",
    "  Degen Promote Regions",
    "  Degen Coalesce and Fill",
    "  Propagate GC State",
    "Pause Full GC (G)",
    "Pause Full GC (N)",
    "  Pre Heap Dump",
    "  Prepare",
    "    Update Roots",
    "  Mark",
    "    System Purge",
    "      Weak References",
    "      Unload Classes",
    "      Weak Roots",
    "      CLDG",
    "  Calculate Addresses",
    "    Regular Objects",
    "    Humongous Objects",
    "  Adjust Pointers",
    "  Adjust Roots",
    "  Copy Objects",
    "    Regular Objects",
    "    Humongous Objects",
    "    Recompute generation usage",
    "    Reset Complete Bitmap",
    "    Rebuild Region Sets",
    "    Reconstruct Remembered Set",
    "  Post Heap Dump",
    "  Propagate GC State",
    "Pacing",
    "Heap Iteration",
];

const PHASES: [Phase; NUM_PHASES] = [
    Phase::conc_reset,
    Phase::conc_reset_after_collect,
    Phase::conc_reset_old,
    Phase::init_mark_gross,
    Phase::init_mark,
    Phase::init_mark_verify,
    Phase::init_manage_tlabs,
    Phase::init_swap_rset,
    Phase::init_transfer_satb,
    Phase::init_update_region_states,
    Phase::init_propagate_gc_state,
    Phase::init_scan_rset,
    Phase::conc_mark_roots,
    Phase::conc_mark,
    Phase::conc_mark_satb_flush,
    Phase::final_mark_gross,
    Phase::final_mark,
    Phase::final_mark_verify,
    Phase::finish_mark,
    Phase::final_mark_propagate_gc_state,
    Phase::purge,
    Phase::purge_weak_par,
    Phase::final_update_region_states,
    Phase::final_manage_labs,
    Phase::choose_cset,
    Phase::final_rebuild_freeset,
    Phase::init_evac,
    Phase::conc_thread_roots,
    Phase::conc_weak_refs,
    Phase::conc_weak_roots,
    Phase::conc_weak_roots_work,
    Phase::conc_weak_roots_rendezvous,
    Phase::conc_cleanup_early,
    Phase::conc_class_unload,
    Phase::conc_class_unload_unlink,
    Phase::conc_class_unload_unlink_sd,
    Phase::conc_class_unload_unlink_weak_klass,
    Phase::conc_class_unload_unlink_code_roots,
    Phase::conc_class_unload_rendezvous,
    Phase::conc_class_unload_purge,
    Phase::conc_class_unload_purge_coderoots,
    Phase::conc_class_unload_purge_cldg,
    Phase::conc_class_unload_purge_ec,
    Phase::conc_strong_roots,
    Phase::conc_evac,
    Phase::conc_final_roots,
    Phase::promote_in_place,
    Phase::final_roots_gross,
    Phase::final_roots,
    Phase::init_update_refs_gross,
    Phase::init_update_refs,
    Phase::init_update_refs_verify,
    Phase::conc_update_refs_prepare,
    Phase::conc_update_refs,
    Phase::conc_update_thread_roots,
    Phase::final_update_refs_gross,
    Phase::final_update_refs,
    Phase::final_update_refs_verify,
    Phase::final_update_refs_update_region_states,
    Phase::final_update_refs_transfer_satb,
    Phase::final_update_refs_trash_cset,
    Phase::final_update_refs_rebuild_freeset,
    Phase::final_update_refs_propagate_gc_state,
    Phase::conc_cleanup_complete,
    Phase::conc_coalesce_and_fill,
    Phase::degen_gc_gross,
    Phase::degen_gc,
    Phase::degen_gc_stw_mark,
    Phase::degen_gc_mark,
    Phase::degen_gc_purge,
    Phase::degen_gc_weakrefs,
    Phase::degen_gc_purge_class_unload,
    Phase::degen_gc_purge_weak_par,
    Phase::degen_gc_purge_cldg,
    Phase::degen_gc_final_update_region_states,
    Phase::degen_gc_final_manage_labs,
    Phase::degen_gc_choose_cset,
    Phase::degen_gc_final_rebuild_freeset,
    Phase::degen_gc_stw_evac,
    Phase::degen_gc_init_update_refs_manage_gclabs,
    Phase::degen_gc_update_refs,
    Phase::degen_gc_final_update_refs_trash_cset,
    Phase::degen_gc_final_update_refs_rebuild_freeset,
    Phase::degen_gc_update_roots,
    Phase::degen_gc_cleanup_complete,
    Phase::degen_gc_promote_regions,
    Phase::degen_gc_coalesce_and_fill,
    Phase::degen_gc_propagate_gc_state,
    Phase::full_gc_gross,
    Phase::full_gc,
    Phase::full_gc_heapdump_pre,
    Phase::full_gc_prepare,
    Phase::full_gc_update_roots,
    Phase::full_gc_mark,
    Phase::full_gc_purge,
    Phase::full_gc_weakrefs,
    Phase::full_gc_purge_class_unload,
    Phase::full_gc_purge_weak_par,
    Phase::full_gc_purge_cldg,
    Phase::full_gc_calculate_addresses,
    Phase::full_gc_calculate_addresses_regular,
    Phase::full_gc_calculate_addresses_humong,
    Phase::full_gc_adjust_pointers,
    Phase::full_gc_adjust_roots,
    Phase::full_gc_copy_objects,
    Phase::full_gc_copy_objects_regular,
    Phase::full_gc_copy_objects_humong,
    Phase::full_gc_recompute_generation_usage,
    Phase::full_gc_copy_objects_reset_complete,
    Phase::full_gc_copy_objects_rebuild,
    Phase::full_gc_reconstruct_remembered_set,
    Phase::full_gc_heapdump_post,
    Phase::full_gc_propagate_gc_state,
    Phase::pacing,
    Phase::heap_iteration_roots,
];

impl Phase {
    pub fn name(self) -> &'static str {
        let idx = self as usize;
        if idx < NUM_PHASES { PHASE_NAMES[idx] } else { "<invalid>" }
    }
}

pub struct ShenandoahPhaseTimings {
    cycle_data: [f64; NUM_PHASES],
    global_data: [HdrSeq; NUM_PHASES],
}

impl ShenandoahPhaseTimings {
    pub fn new() -> Self {
        Self {
            cycle_data: [-1.0; NUM_PHASES],
            global_data: std::array::from_fn(|_| HdrSeq::new()),
        }
    }

    pub fn record_phase_time(&mut self, phase: Phase, time: f64, should_aggregate: bool) {
        let idx = phase as usize;
        if idx >= NUM_PHASES {
            return;
        }
        if should_aggregate {
            self.cycle_data[idx] += time;
        } else {
            self.cycle_data[idx] = time;
        }
    }

    pub fn cycle_time(&self, phase: Phase) -> f64 {
        let idx = phase as usize;
        if idx < NUM_PHASES { self.cycle_data[idx] } else { -1.0 }
    }

    pub fn flush_cycle_to_global(&mut self) {
        for i in 0..NUM_PHASES {
            let t = self.cycle_data[i];
            if t >= 0.0 {
                self.global_data[i].add(t);
                self.cycle_data[i] = -1.0;
            }
        }
    }

    pub fn global(&self, phase: Phase) -> &HdrSeq {
        &self.global_data[phase as usize]
    }

    pub fn print_cycle_on(&self) -> String {
        let mut out = String::new();
        out.push_str("Shenandoah GC cycle phase times:
");
        for (i, &t) in self.cycle_data.iter().enumerate() {
            if t >= 0.0 {
                out.push_str(&format!("  {:>12.3} ms  {}
", t, PHASE_NAMES[i]));
            }
        }
        out
    }
}

impl Default for ShenandoahPhaseTimings {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_names_cover_all() {
        assert_eq!(PHASE_NAMES.len(), NUM_PHASES);
        assert_eq!(Phase::conc_mark.name(), "Concurrent Marking");
        assert_eq!(Phase::pacing.name(), "Pacing");
    }

    #[test]
    fn record_and_flush() {
        let mut t = ShenandoahPhaseTimings::new();
        t.record_phase_time(Phase::init_mark, 1.5, false);
        t.record_phase_time(Phase::conc_mark, 10.0, false);
        t.record_phase_time(Phase::conc_evac, 3.0, false);
        t.flush_cycle_to_global();
        assert_eq!(t.cycle_time(Phase::init_mark), -1.0);
        assert_eq!(t.global(Phase::conc_mark).num(), 1);
        assert!((t.global(Phase::conc_mark).maximum() - 10.0).abs() < 1e-9);
    }
}
