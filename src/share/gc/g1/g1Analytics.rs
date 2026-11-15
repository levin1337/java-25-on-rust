/*
 * Port of OpenJDK 25 HotSpot G1Analytics (share/gc/g1/g1Analytics.hpp).
 *
 * G1Analytics accumulates per-GC-phase timing/cost sequences and
 * produces the predictions consumed by G1Policy (pause-time and
 * young-gen sizing). Each predictor is `davg + stddev_scale *
 * stddev_estimate` over a TruncatedSeq (see g1Predictions.rs).
 */

use super::g1Predictions::{G1PhaseDependentSeq, G1Predictions, TruncatedSeq};

/// Ergonomic constants from g1_globals.hpp used by analytics.
pub const G1_CONFIDENCE_PERCENT: f64 = 90.0;
pub const TARGET_PLAB_WASTE_PCT: f64 = 10.0;

/// `G1Analytics`: timing/cost model for G1 pauses.
#[derive(Debug)]
pub struct G1Analytics {
    predictions: G1Predictions,

    // GC pause times.
    recent_gc_times_ms: TruncatedSeq,
    recent_prev_end_times_for_all_gcs_sec: TruncatedSeq,

    // Concurrent mark phase times.
    concurrent_mark_remark_times_ms: TruncatedSeq,
    concurrent_mark_cleanup_times_ms: TruncatedSeq,

    // Allocation / refinement rates.
    alloc_rate_ms_seq: TruncatedSeq,
    concurrent_refine_rate_ms_seq: TruncatedSeq,
    dirtied_cards_rate_ms_seq: TruncatedSeq,
    dirtied_cards_in_thread_buffers_seq: TruncatedSeq,

    // Per-card / per-region / per-byte costs (phase dependent).
    cost_per_card_scan_ms_seq: G1PhaseDependentSeq,
    cost_per_card_merge_ms_seq: G1PhaseDependentSeq,
    cost_per_code_root_ms_seq: G1PhaseDependentSeq,
    cost_per_byte_copied_ms_seq: G1PhaseDependentSeq,
    cost_per_byte_ms_during_cm_seq: TruncatedSeq,

    // Fixed overheads.
    constant_other_time_ms_seq: TruncatedSeq,
    young_other_cost_per_region_ms_seq: TruncatedSeq,
    non_young_other_cost_per_region_ms_seq: TruncatedSeq,

    // Remembered-set sizes at GC start.
    card_rs_length_seq: G1PhaseDependentSeq,
    code_root_rs_length_seq: G1PhaseDependentSeq,
    pending_cards_seq: G1PhaseDependentSeq,
    scan_cards_seq: G1PhaseDependentSeq,
}

impl G1Analytics {
    /// `G1Analytics::G1Analytics` — stddev scale mirrors
    /// `G1AnalyticsConfidence` (default ~1.64 for 90% confidence on
    /// a normal distribution is approximated by HotSpot's constant).
    pub fn new() -> Self {
        Self {
            predictions: G1Predictions::new(1.64),
            recent_gc_times_ms: TruncatedSeq::with_defaults(),
            recent_prev_end_times_for_all_gcs_sec: TruncatedSeq::with_defaults(),
            concurrent_mark_remark_times_ms: TruncatedSeq::with_defaults(),
            concurrent_mark_cleanup_times_ms: TruncatedSeq::with_defaults(),
            alloc_rate_ms_seq: TruncatedSeq::with_defaults(),
            concurrent_refine_rate_ms_seq: TruncatedSeq::with_defaults(),
            dirtied_cards_rate_ms_seq: TruncatedSeq::with_defaults(),
            dirtied_cards_in_thread_buffers_seq: TruncatedSeq::with_defaults(),
            cost_per_card_scan_ms_seq: G1PhaseDependentSeq::new(),
            cost_per_card_merge_ms_seq: G1PhaseDependentSeq::new(),
            cost_per_code_root_ms_seq: G1PhaseDependentSeq::new(),
            cost_per_byte_copied_ms_seq: G1PhaseDependentSeq::new(),
            cost_per_byte_ms_during_cm_seq: TruncatedSeq::with_defaults(),
            constant_other_time_ms_seq: TruncatedSeq::with_defaults(),
            young_other_cost_per_region_ms_seq: TruncatedSeq::with_defaults(),
            non_young_other_cost_per_region_ms_seq: TruncatedSeq::with_defaults(),
            card_rs_length_seq: G1PhaseDependentSeq::new(),
            code_root_rs_length_seq: G1PhaseDependentSeq::new(),
            pending_cards_seq: G1PhaseDependentSeq::new(),
            scan_cards_seq: G1PhaseDependentSeq::new(),
        }
    }

    // -- report_* ------------------------------------------------------

    /// `report_gc_pause_time_ms`.
    pub fn report_gc_pause_time_ms(&mut self, ms: f64) {
        self.recent_gc_times_ms.add(ms);
    }

    /// `update_recent_gc_times`.
    pub fn update_recent_gc_times(&mut self, end_time_sec: f64, pause_time_ms: f64) {
        self.recent_gc_times_ms.add(pause_time_ms);
        self.recent_prev_end_times_for_all_gcs_sec.add(end_time_sec);
    }

    /// `report_alloc_rate_ms`.
    pub fn report_alloc_rate_ms(&mut self, rate: f64) {
        self.alloc_rate_ms_seq.add(rate);
    }

    /// `report_concurrent_refine_rate_ms`.
    pub fn report_concurrent_refine_rate_ms(&mut self, rate: f64) {
        self.concurrent_refine_rate_ms_seq.add(rate);
    }

    /// `report_dirtied_cards_rate_ms` / `..._in_thread_buffers`.
    pub fn report_dirtied_cards_rate_ms(&mut self, rate: f64) {
        self.dirtied_cards_rate_ms_seq.add(rate);
    }

    pub fn report_dirtied_cards_in_thread_buffers(&mut self, count: f64) {
        self.dirtied_cards_in_thread_buffers_seq.add(count);
    }

    pub fn report_concurrent_mark_remark_times_ms(&mut self, ms: f64) {
        self.concurrent_mark_remark_times_ms.add(ms);
    }

    pub fn report_concurrent_mark_cleanup_times_ms(&mut self, ms: f64) {
        self.concurrent_mark_cleanup_times_ms.add(ms);
    }

    pub fn report_cost_per_card_scan_ms(&mut self, cost: f64, young_only: bool) {
        self.cost_per_card_scan_ms_seq.add(cost, young_only);
    }

    pub fn report_cost_per_card_merge_ms(&mut self, cost: f64, young_only: bool) {
        self.cost_per_card_merge_ms_seq.add(cost, young_only);
    }

    pub fn report_cost_per_code_root_scan_ms(&mut self, cost: f64, young_only: bool) {
        self.cost_per_code_root_ms_seq.add(cost, young_only);
    }

    pub fn report_cost_per_byte_copied_ms(&mut self, cost: f64, young_only: bool) {
        self.cost_per_byte_copied_ms_seq.add(cost, young_only);
    }

    pub fn report_cost_per_byte_ms_during_cm(&mut self, cost: f64) {
        self.cost_per_byte_ms_during_cm_seq.add(cost);
    }

    pub fn report_constant_other_time_ms(&mut self, ms: f64) {
        self.constant_other_time_ms_seq.add(ms);
    }

    pub fn report_young_other_cost_per_region_ms(&mut self, cost: f64) {
        self.young_other_cost_per_region_ms_seq.add(cost);
    }

    pub fn report_non_young_other_cost_per_region_ms(&mut self, cost: f64) {
        self.non_young_other_cost_per_region_ms_seq.add(cost);
    }

    pub fn report_card_rs_length(&mut self, len: f64, young_only: bool) {
        self.card_rs_length_seq.add(len, young_only);
    }

    pub fn report_code_root_rs_length(&mut self, len: f64, young_only: bool) {
        self.code_root_rs_length_seq.add(len, young_only);
    }

    pub fn report_pending_cards(&mut self, cards: f64, young_only: bool) {
        self.pending_cards_seq.add(cards, young_only);
    }

    pub fn report_scan_cards(&mut self, cards: f64, young_only: bool) {
        self.scan_cards_seq.add(cards, young_only);
    }

    // -- predict_* -----------------------------------------------------

    fn predict_size(&self, seq: &TruncatedSeq) -> usize {
        self.predictions.predict_zero_bounded(seq) as usize
    }

    fn predict_size_phase(&self, seq: &G1PhaseDependentSeq, young_only: bool) -> usize {
        self.predict_size(seq.seq(young_only))
    }

    fn predict_zero_bounded_phase(&self, seq: &G1PhaseDependentSeq, young_only: bool) -> f64 {
        self.predictions.predict_zero_bounded(seq.seq(young_only))
    }

    fn predict_zero_bounded(&self, seq: &TruncatedSeq) -> f64 {
        self.predictions.predict_zero_bounded(seq)
    }

    /// `predict_gc_pause_time_ms`.
    pub fn predict_gc_pause_time_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.recent_gc_times_ms)
    }

    /// `predict_alloc_rate_ms`.
    pub fn predict_alloc_rate_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.alloc_rate_ms_seq)
    }

    /// `predict_concurrent_refine_rate_ms`.
    pub fn predict_concurrent_refine_rate_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.concurrent_refine_rate_ms_seq)
    }

    /// `predict_dirtied_cards_rate_ms`.
    pub fn predict_dirtied_cards_rate_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.dirtied_cards_rate_ms_seq)
    }

    /// `predict_dirtied_cards_in_thread_buffers`.
    pub fn predict_dirtied_cards_in_thread_buffers(&self) -> f64 {
        self.predict_zero_bounded(&self.dirtied_cards_in_thread_buffers_seq)
    }

    /// `predict_concurrent_mark_remark_time_ms`.
    pub fn predict_concurrent_mark_remark_time_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.concurrent_mark_remark_times_ms)
    }

    /// `predict_remark_time_ms`.
    pub fn predict_remark_time_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.concurrent_mark_remark_times_ms)
    }

    /// `predict_cleanup_time_ms`.
    pub fn predict_cleanup_time_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.concurrent_mark_cleanup_times_ms)
    }

    /// `predict_cost_per_card_scan_ms`.
    pub fn predict_cost_per_card_scan_ms(&self, young_only: bool) -> f64 {
        self.predict_zero_bounded_phase(&self.cost_per_card_scan_ms_seq, young_only)
    }

    /// `predict_card_scan_time_ms(card_num)`.
    pub fn predict_card_scan_time_ms(&self, card_num: usize, young_only: bool) -> f64 {
        card_num as f64 * self.predict_cost_per_card_scan_ms(young_only)
    }

    /// `predict_cost_per_card_merge_ms`.
    pub fn predict_cost_per_card_merge_ms(&self, young_only: bool) -> f64 {
        self.predict_zero_bounded_phase(&self.cost_per_card_merge_ms_seq, young_only)
    }

    /// `predict_card_merge_time_ms(card_num)`.
    pub fn predict_card_merge_time_ms(&self, card_num: usize, young_only: bool) -> f64 {
        card_num as f64 * self.predict_cost_per_card_merge_ms(young_only)
    }

    /// `predict_cost_per_code_root_ms`.
    pub fn predict_cost_per_code_root_ms(&self, young_only: bool) -> f64 {
        self.predict_zero_bounded_phase(&self.cost_per_code_root_ms_seq, young_only)
    }

    /// `predict_code_root_scan_time_ms(code_root_num)`.
    pub fn predict_code_root_scan_time_ms(&self, code_root_num: usize, young_only: bool) -> f64 {
        code_root_num as f64 * self.predict_cost_per_code_root_ms(young_only)
    }

    /// `predict_cost_per_byte_copied_ms`.
    pub fn predict_cost_per_byte_copied_ms(&self, young_only: bool) -> f64 {
        self.predict_zero_bounded_phase(&self.cost_per_byte_copied_ms_seq, young_only)
    }

    /// `predict_object_copy_time_ms(bytes_to_copy)`.
    pub fn predict_object_copy_time_ms(&self, bytes_to_copy: usize, young_only: bool) -> f64 {
        bytes_to_copy as f64 * self.predict_cost_per_byte_copied_ms(young_only)
    }

    /// `predict_cost_per_byte_ms_during_concurrent_mark`.
    pub fn predict_cost_per_byte_ms_during_concurrent_mark(&self) -> f64 {
        self.predict_zero_bounded(&self.cost_per_byte_ms_during_cm_seq)
    }

    /// `predict_constant_other_time_ms`.
    pub fn predict_constant_other_time_ms(&self) -> f64 {
        self.predict_zero_bounded(&self.constant_other_time_ms_seq)
    }

    /// `predict_young_other_time_ms(young_num)`.
    pub fn predict_young_other_time_ms(&self, young_num: usize) -> f64 {
        young_num as f64
            * self.predict_zero_bounded(&self.young_other_cost_per_region_ms_seq)
    }

    /// `predict_non_young_other_time_ms(non_young_num)`.
    pub fn predict_non_young_other_time_ms(&self, non_young_num: usize) -> f64 {
        non_young_num as f64
            * self.predict_zero_bounded(&self.non_young_other_cost_per_region_ms_seq)
    }

    /// `predict_card_rs_length`.
    pub fn predict_card_rs_length(&self, young_only: bool) -> usize {
        self.predict_size_phase(&self.card_rs_length_seq, young_only)
    }

    /// `predict_code_root_rs_length`.
    pub fn predict_code_root_rs_length(&self, young_only: bool) -> usize {
        self.predict_size_phase(&self.code_root_rs_length_seq, young_only)
    }

    /// `predict_pending_cards`.
    pub fn predict_pending_cards(&self, young_only: bool) -> usize {
        self.predict_size_phase(&self.pending_cards_seq, young_only)
    }

    /// `predict_scan_card_num`: HotSpot maps remembered-set length to
    /// the number of cards actually scanned (with a fixed overhead).
    pub fn predict_scan_card_num(&self, card_rs_length: usize, young_only: bool) -> usize {
        // approximation: scan cards ~ card_rs_length (they are the
        // same unit in G1's card table); kept as a function for
        // parity with the C++ call sites.
        let _ = self.scan_cards_seq.seq(young_only);
        card_rs_length
    }

    /// `oldest_known_gc_end_time_sec` / `most_recent_gc_end_time_sec`.
    pub fn oldest_known_gc_end_time_sec(&self) -> f64 {
        self.recent_prev_end_times_for_all_gcs_sec.oldest()
    }

    pub fn most_recent_gc_end_time_sec(&self) -> f64 {
        self.recent_prev_end_times_for_all_gcs_sec.last()
    }

    /// Number of recorded GC pauses (for "enough samples" checks).
    pub fn num_gc_pauses(&self) -> usize {
        self.recent_gc_times_ms.num_samples()
    }
}

impl Default for G1Analytics {
    fn default() -> Self {
        Self::new()
    }
}
