/*
 * Port of OpenJDK 25 HotSpot G1Policy + G1YoungLengthPredictor
 * (share/gc/g1/g1Policy.hpp/.cpp).
 *
 * G1Policy computes the desired/target young generation length in
 * regions each GC, balancing:
 *  - sizer min/max bounds (G1YoungGenSizer),
 *  - the pause-time goal for the whole young evacuation,
 *  - the MMU goal spacing GCs apart,
 *  - survivor-region counts and remembered-set sizes predicted by
 *    G1Analytics.
 *
 * Heap integration points: the caller supplies heap-state numbers
 * (num regions, survivor/eden/allocated young counts, free regions
 * at end of collection, young-only phase flag); all timing math is
 * heap-independent.
 */

use super::g1Analytics::G1Analytics;
use super::g1MMUTracker::G1MMUTracker;
use super::g1SurvRateGroup::G1SurvRateGroup;
use super::g1YoungGenSizer::{G1YoungGenSizer, YoungGenSizingParams};

/// `G1ConfidencePercent` / `TargetPLABWastePct` ergonomics used by
/// the young-length predictor's safety margin.
pub const G1_CONFIDENCE_PERCENT: f64 = 90.0;
pub const TARGET_PLAB_WASTE_PCT: f64 = 10.0;

/// Inputs describing current heap state for young-length calculation.
#[derive(Debug, Clone, Copy)]
pub struct YoungGenHeapState {
    /// Total heap regions.
    pub num_heap_regions: u32,
    /// Regions in the young generation (eden+survivor) currently.
    pub allocated_young_length: u32,
    /// Regions currently in survivor spaces.
    pub survivor_regions_count: u32,
    /// Free regions available at end of last collection.
    pub free_regions_at_end_of_collection: u32,
    /// Whether we are in the young-only phase (no concurrent mark).
    pub in_young_only_phase: bool,
}

/// `G1YoungLengthPredictor`: decides whether a candidate young
/// length fits the pause target and the free-space constraint.
struct G1YoungLengthPredictor<'a> {
    base_time_ms: f64,
    base_free_regions: f64,
    target_pause_time_ms: f64,
    policy: &'a G1Policy,
    region_grain_bytes: u64,
}

impl<'a> G1YoungLengthPredictor<'a> {
    fn will_fit(&self, young_length: u32) -> bool {
        if young_length as f64 >= self.base_free_regions {
            // end condition 1: not enough space
            return false;
        }
        let mut bytes_to_copy = 0usize;
        let copy_time_ms =
            self.policy
                .predict_eden_copy_time_ms(young_length, Some(&mut bytes_to_copy));
        let young_other_time_ms =
            self.policy.analytics.predict_young_other_time_ms(young_length as usize);
        let pause_time_ms = self.base_time_ms + copy_time_ms + young_other_time_ms;
        if pause_time_ms > self.target_pause_time_ms {
            // end condition 2: over target pause time
            return false;
        }
        let free_bytes =
            (self.base_free_regions - young_length as f64) * self.region_grain_bytes as f64;
        let safety_factor =
            (100.0 / G1_CONFIDENCE_PERCENT) * (100.0 + TARGET_PLAB_WASTE_PCT) / 100.0;
        let expected_bytes_to_copy = (safety_factor * bytes_to_copy as f64) as u64;
        if expected_bytes_to_copy as f64 > free_bytes {
            // end condition 3: out of space
            return false;
        }
        true
    }
}

/// `G1Policy`.
#[derive(Debug)]
pub struct G1Policy {
    pub analytics: G1Analytics,
    pub mmu_tracker: G1MMUTracker,
    pub surv_rate_group: G1SurvRateGroup,
    young_gen_sizer: G1YoungGenSizer,

    /// Max allowed GC pause time (ms); `MaxGCPauseMillis`.
    max_gc_pause_ms: u64,
    /// Region grain in bytes.
    region_grain_bytes: u64,

    /// Desired/target young list length in regions (atomics in
    /// HotSpot; plain fields here — external synchronization).
    young_list_desired_length: u32,
    young_list_target_length: u32,

    /// Flag: adaptive young sizing enabled.
    use_adaptive_young_list_length: bool,
    /// Old-gen full flag used by `should_allocate_mutator_region`.
    old_gen_is_full: bool,
    /// Tenuring threshold: regions surviving this many young GCs are
    /// promoted to old (`MaxTenuringThreshold` ergonomics, default 15
    /// like the previous scaffold / HotSpot default range).
    tenuring_threshold: u32,
}

impl G1Policy {
    /// `G1Policy::G1Policy` defaults: MaxGCPauseMillis=200,
    /// defaults young sizing params.
    pub fn new(region_grain_bytes: u64) -> Self {
        let sizer = G1YoungGenSizer::new(YoungGenSizingParams::default(), region_grain_bytes);
        Self {
            analytics: G1Analytics::new(),
            mmu_tracker: G1MMUTracker::new(),
            surv_rate_group: G1SurvRateGroup::new(),
            young_gen_sizer: sizer,
            max_gc_pause_ms: 200,
            region_grain_bytes: region_grain_bytes.max(1),
            young_list_desired_length: 0,
            young_list_target_length: 0,
            use_adaptive_young_list_length: true,
            old_gen_is_full: false,
            tenuring_threshold: 15,
        }
    }

    pub fn with_sizing_params(region_grain_bytes: u64, params: YoungGenSizingParams) -> Self {
        let mut p = Self::new(region_grain_bytes);
        p.young_gen_sizer = G1YoungGenSizer::new(params, region_grain_bytes);
        p
    }

    #[inline]
    pub fn max_gc_pause_ms(&self) -> u64 {
        self.max_gc_pause_ms
    }

    pub fn set_max_gc_pause_ms(&mut self, ms: u64) {
        self.max_gc_pause_ms = ms;
    }

    #[inline]
    pub fn region_grain_bytes(&self) -> u64 {
        self.region_grain_bytes
    }

    #[inline]
    pub fn young_list_desired_length(&self) -> u32 {
        self.young_list_desired_length
    }

    #[inline]
    pub fn young_list_target_length(&self) -> u32 {
        self.young_list_target_length
    }

    #[inline]
    pub fn young_gen_sizer(&self) -> &G1YoungGenSizer {
        &self.young_gen_sizer
    }

    #[inline]
    pub fn set_old_gen_is_full(&mut self, full: bool) {
        self.old_gen_is_full = full;
    }

    // -- predictions ----------------------------------------------------

    /// `predict_young_region_other_time_ms`.
    pub fn predict_young_region_other_time_ms(&self, count: usize) -> f64 {
        self.analytics.predict_young_other_time_ms(count)
    }

    /// `predict_non_young_region_other_time_ms`.
    pub fn predict_non_young_region_other_time_ms(&self, count: usize) -> f64 {
        self.analytics.predict_non_young_other_time_ms(count)
    }

    /// `predict_eden_copy_time_ms(count, bytes_to_copy)`.
    ///
    /// Expected copied bytes = cumulative predicted survivor rate for
    /// `count - 1` regions * region size; time = bytes * per-byte
    /// copy cost.
    pub fn predict_eden_copy_time_ms(&self, count: u32, bytes_to_copy: Option<&mut usize>) -> f64 {
        if count == 0 {
            if let Some(b) = bytes_to_copy {
                *b = 0;
            }
            return 0.0;
        }
        let expected_bytes = (self.surv_rate_group.accum_surv_rate_pred(
            (count - 1) as usize,
        ) * self.region_grain_bytes as f64) as usize;
        if let Some(b) = bytes_to_copy {
            *b = expected_bytes;
        }
        self.analytics
            .predict_object_copy_time_ms(expected_bytes, true)
    }

    /// `predict_region_copy_time_ms` for an arbitrary region size.
    pub fn predict_region_copy_time_ms(&self, bytes: usize) -> f64 {
        self.analytics.predict_object_copy_time_ms(bytes, true)
    }

    /// `predict_merge_scan_time`.
    pub fn predict_merge_scan_time(&self, card_rs_length: usize) -> f64 {
        let scan_card_num =
            self.analytics.predict_scan_card_num(card_rs_length, false);
        self.analytics.predict_card_merge_time_ms(card_rs_length, false)
            + self.analytics.predict_card_scan_time_ms(scan_card_num, false)
    }

    /// `calculate_desired_eden_length_by_mmu`.
    fn calculate_desired_eden_length_by_mmu(&self, _state: &YoungGenHeapState) -> u32 {
        if !self.use_adaptive_young_list_length {
            return 0;
        }
        // HotSpot: alloc_rate * when_max_gc_sec. Our MMU tracker
        // predicts heap-full time; convert to "regions until full".
        let now_sec = self.mmu_tracker.most_recent_gc_end_time_sec();
        let when_ms = (self.mmu_tracker.when_max_gc_sec_cached() - now_sec).max(0.0) * 1000.0;
        let alloc_rate_ms = self.analytics.predict_alloc_rate_ms();
        (alloc_rate_ms * when_ms / self.region_grain_bytes as f64).ceil() as u32
    }

    /// `calculate_desired_eden_length_by_pause` — binary search over
    /// [min_eden, max_eden] using G1YoungLengthPredictor.
    pub fn calculate_desired_eden_length_by_pause(
        &self,
        base_time_ms: f64,
        min_eden_length: u32,
        max_eden_length: u32,
        state: &YoungGenHeapState,
    ) -> u32 {
        if !self.use_adaptive_young_list_length {
            return min_eden_length;
        }
        assert!(min_eden_length <= max_eden_length);
        let predictor = G1YoungLengthPredictor {
            base_time_ms,
            base_free_regions: state.free_regions_at_end_of_collection as f64,
            target_pause_time_ms: self.max_gc_pause_ms as f64,
            policy: self,
            region_grain_bytes: self.region_grain_bytes,
        };
        let mut min_eden = min_eden_length;
        if predictor.will_fit(min_eden) {
            if predictor.will_fit(max_eden_length) {
                return max_eden_length;
            }
            let mut max_eden = max_eden_length;
            let mut diff = (max_eden - min_eden) / 2;
            while diff > 0 {
                let eden_length = min_eden + diff;
                if predictor.will_fit(eden_length) {
                    min_eden = eden_length;
                } else {
                    max_eden = eden_length;
                }
                diff = (max_eden - min_eden) / 2;
            }
        }
        min_eden
    }

    /// `calculate_young_desired_length` — the core young-length
    /// calculation from g1Policy.cpp.
    pub fn calculate_young_desired_length(
        &mut self,
        state: &YoungGenHeapState,
        pending_cards: usize,
        card_rs_length: usize,
        code_root_rs_length: usize,
    ) -> u32 {
        let _ = (pending_cards, card_rs_length, code_root_rs_length);
        // update sizer bounds for current heap size
        self.young_gen_sizer.heap_size_changed(state.num_heap_regions);
        let (min_young_by_sizer, max_young_by_sizer) =
            self.young_gen_sizer.recalculate_min_max_young_length(state.num_heap_regions);

        // absolute bounds: at least one eden region after GC,
        // at least what is currently allocated.
        let absolute_min_young_length = min_young_by_sizer
            .max(1)
            .max(state.allocated_young_length.saturating_sub(state.survivor_regions_count).max(1));
        let absolute_max_young_length =
            max_young_by_sizer.max(absolute_min_young_length);

        let desired_young_length = if self.use_adaptive_young_list_length {
            let desired_eden_length_by_mmu = self.calculate_desired_eden_length_by_mmu(state);

            // base_time_ms = time spent outside eden evacuation:
            // constant other + card merge/scan + code roots +
            // non-young other + remark/cleanup estimates.
            let for_young_only = state.in_young_only_phase;
            let mut base_time_ms = self.analytics.predict_constant_other_time_ms();
            base_time_ms += self.predict_merge_scan_time(card_rs_length);
            base_time_ms += self.analytics.predict_code_root_scan_time_ms(
                code_root_rs_length,
                for_young_only,
            );
            base_time_ms += self
                .predict_non_young_region_other_time_ms(
                    (state.num_heap_regions - state.allocated_young_length) as usize,
                );
            base_time_ms += self.analytics.predict_remark_time_ms();
            base_time_ms += self.analytics.predict_cleanup_time_ms();

            let survivor_length = state.survivor_regions_count;
            let desired_eden_length_by_pause = self.calculate_desired_eden_length_by_pause(
                base_time_ms,
                absolute_min_young_length.saturating_sub(survivor_length).max(1),
                absolute_max_young_length.saturating_sub(survivor_length),
                state,
            );

            let desired_eden_length =
                desired_eden_length_by_pause.max(desired_eden_length_by_mmu);
            desired_eden_length + survivor_length
        } else {
            min_young_by_sizer
        };

        desired_young_length.clamp(absolute_min_young_length, absolute_max_young_length)
    }

    /// `update_young_length_bounds`: full update including target
    /// length computation (`calculate_young_target_length` adds a
    /// fraction of desired for hysteresis).
    pub fn update_young_length_bounds(
        &mut self,
        state: &YoungGenHeapState,
        pending_cards: usize,
        card_rs_length: usize,
        code_root_rs_length: usize,
    ) -> (u32, u32) {
        let desired =
            self.calculate_young_desired_length(state, pending_cards, card_rs_length, code_root_rs_length);
        // HotSpot `calculate_young_target_length`: desired plus up to
        // 50% headroom bounded by max (simplified to desired here —
        // HotSpot uses _young_list_target_length = desired +
        // (desired - current)/2 style smoothing; we keep target ==
        // desired for determinism).
        let target = desired;
        self.young_list_desired_length = desired;
        self.young_list_target_length = target;
        (desired, target)
    }

    #[inline]
    pub fn tenuring_threshold(&self) -> u32 {
        self.tenuring_threshold
    }

    pub fn set_tenuring_threshold(&mut self, threshold: u32) {
        self.tenuring_threshold = threshold.clamp(1, 15);
    }

    /// `should_allocate_mutator_region`.
    pub fn should_allocate_mutator_region(&self, young_list_length: u32) -> bool {
        if self.old_gen_is_full {
            return young_list_length < self.young_list_desired_length.max(1);
        }
        young_list_length < self.young_list_target_length.max(1)
    }

    /// `record_young_gc_pause_start`-style: feed a completed GC's
    /// measurements into analytics.
    pub fn record_young_gc_pause_end(&mut self, pause_ms: f64) {
        self.analytics.report_gc_pause_time_ms(pause_ms);
    }

    /// `survivor rate` update hook: record per-position survival for
    /// the eden group after a GC.
    pub fn record_eden_survival_rates(&mut self, rates: &[f64]) {
        self.surv_rate_group.add(rates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> YoungGenHeapState {
        YoungGenHeapState {
            num_heap_regions: 200,
            allocated_young_length: 20,
            survivor_regions_count: 4,
            free_regions_at_end_of_collection: 100,
            in_young_only_phase: true,
        }
    }

    #[test]
    fn young_length_within_bounds() {
        let mut policy = G1Policy::new(1024 * 1024);
        let (desired, target) = policy.update_young_length_bounds(&state(), 0, 0, 0);
        // no analytics data -> predictions are zero -> eden by pause
        // bounded; desired >= min sizer bound (10) and <= max (120)
        assert!(desired >= 1);
        assert!(desired <= 124);
        assert_eq!(desired, target);
        assert_eq!(policy.young_list_desired_length(), desired);
    }

    #[test]
    fn pause_time_limits_eden() {
        let mut policy = G1Policy::new(1024 * 1024);
        policy.set_max_gc_pause_ms(1); // tiny pause budget
        // feed expensive costs so nothing fits
        for _ in 0..10 {
            policy.analytics.report_cost_per_byte_copied_ms(1000.0, true);
            policy.analytics.report_young_other_cost_per_region_ms(1000.0);
            policy.analytics.report_constant_other_time_ms(500.0);
        }
        policy.record_eden_survival_rates(&[1.0; 50]);
        let st = state();
        let desired = policy.calculate_young_desired_length(&st, 0, 0, 0);
        // predicted costs are huge -> eden collapses to the absolute
        // minimum: max(sizer_min=10, allocated-survivor=16) = 16
        assert_eq!(desired, 16);
    }

    #[test]
    fn sizer_bounds_respected() {
        let mut policy = G1Policy::new(1024 * 1024);
        let st = YoungGenHeapState {
            num_heap_regions: 100,
            allocated_young_length: 5,
            survivor_regions_count: 1,
            free_regions_at_end_of_collection: 50,
            in_young_only_phase: true,
        };
        let desired = policy.calculate_young_desired_length(&st, 0, 0, 0);
        // min sizer bound = 5% of 100 = 5
        assert!(desired >= 5);
        // max sizer bound = 60% = 60
        assert!(desired <= 60);
    }
}
