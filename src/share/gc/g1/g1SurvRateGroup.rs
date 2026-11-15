/*
 * Port of OpenJDK 25 HotSpot G1SurvRateGroup
 * (share/gc/g1/g1SurvRateGroup.hpp) — survivor-rate prediction used
 * by `predict_eden_copy_time_ms`.
 *
 * HotSpot keeps, per region "age"/position group, the survival rate
 * observed at each young GC and predicts the cumulative survivor
 * rate for `n` regions via a weighted average over past GCs.
 *
 * This port keeps the same structure: per-group samples indexed by
 * region position in the young gen, with `accum_surv_rate_pred`
 * computing the predicted cumulative survival rate.
 */

/// `G1SurvRateGroup`: survival-rate samples for one region group.
#[derive(Debug, Default)]
pub struct G1SurvRateGroup {
    /// surv_rate_samples[group][region_pos] = survival rate in [0,1]
    /// observed at that GC for that region position.
    samples: Vec<Vec<f64>>,
    /// Per-position decayed average across GCs.
    position_avg: Vec<f64>,
    /// Global decayed average (fallback).
    global_avg: f64,
    sample_count: usize,
}

impl G1SurvRateGroup {
    pub fn new() -> Self {
        Self::default()
    }

    /// `add`: record survival rates for one GC. `rates[i]` is the
    /// fraction of region at young-gen position `i` that survived.
    pub fn add(&mut self, rates: &[f64]) {
        if rates.is_empty() {
            return;
        }
        let alpha = 0.5;
        if self.position_avg.len() < rates.len() {
            self.position_avg.resize(rates.len(), 0.0);
        }
        let mut sum = 0.0;
        for (i, &r) in rates.iter().enumerate() {
            let prev = self.position_avg[i];
            self.position_avg[i] = if self.sample_count == 0 { r } else { prev + alpha * (r - prev) };
            sum += r;
        }
        let gc_avg = sum / rates.len() as f64;
        self.global_avg = if self.sample_count == 0 {
            gc_avg
        } else {
            self.global_avg + alpha * (gc_avg - self.global_avg)
        };
        self.samples.push(rates.to_vec());
        self.sample_count += 1;
    }

    /// `accum_surv_rate_pred(n)`: predicted cumulative survival rate
    /// for `n` regions — HotSpot uses position-indexed averages with
    /// interpolation; we sum predicted per-position rates for the
    /// first `n` positions, falling back to the global average for
    /// positions never observed.
    pub fn accum_surv_rate_pred(&self, n: usize) -> f64 {
        if self.sample_count == 0 || n == 0 {
            return 0.0;
        }
        let mut acc = 0.0;
        for i in 0..n {
            acc += self
                .position_avg
                .get(i)
                .copied()
                .unwrap_or(self.global_avg);
        }
        acc
    }

    /// `accum_surv_rate` for observed data (last GC).
    pub fn accum_surv_rate(&self, n: usize) -> f64 {
        self.samples
            .last()
            .map(|r| r.iter().take(n).sum::<f64>())
            .unwrap_or(0.0)
    }

    #[inline]
    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    pub fn global_avg(&self) -> f64 {
        self.global_avg
    }
}
