/*
 * Port of OpenJDK 25 HotSpot TruncatedSeq / NumberSeq
 * (share/utilities/numberSeq.hpp) and G1Predictions
 * (share/gc/g1/g1Predictions.hpp).
 *
 * TruncatedSeq keeps the last N samples and maintains both a plain
 * average/variance over stored samples and an exponentially decaying
 * average (davg/dvariance) with alpha = 0.3.
 */

pub const DEFAULT_ALPHA: f64 = 0.3;
pub const DEFAULT_SEQ_LENGTH: usize = 10;

/// `AbsSeq` + `TruncatedSeq`: fixed-length sample sequence.
#[derive(Debug)]
pub struct TruncatedSeq {
    sequence: Vec<f64>,
    length: usize,
    next: usize,
    num: usize,
    sum: f64,
    sum_of_squares: f64,
    davg: f64,
    dvariance: f64,
    alpha: f64,
}

impl TruncatedSeq {
    pub fn new(length: usize, alpha: f64) -> Self {
        Self {
            sequence: vec![0.0; length.max(1)],
            length: length.max(1),
            next: 0,
            num: 0,
            sum: 0.0,
            sum_of_squares: 0.0,
            davg: 0.0,
            dvariance: 0.0,
            alpha,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_SEQ_LENGTH, DEFAULT_ALPHA)
    }

    /// `add`: push a sample, evicting the oldest when full.
    pub fn add(&mut self, val: f64) {
        // decaying average update (AbsSeq::add)
        if self.num == 0 {
            self.davg = val;
        } else {
            let diff = val - self.davg;
            let incr = self.alpha * diff;
            self.davg += incr;
            self.dvariance = (1.0 - self.alpha) * (self.dvariance + diff * incr);
        }

        let old_val = self.sequence[self.next];
        self.sum -= old_val;
        self.sum_of_squares -= old_val * old_val;
        self.sum += val;
        self.sum_of_squares += val * val;
        self.sequence[self.next] = val;
        self.next = (self.next + 1) % self.length;
        if self.num < self.length {
            self.num += 1;
        }
    }

    /// `avg`: plain average over stored samples.
    pub fn avg(&self) -> f64 {
        if self.num == 0 { 0.0 } else { self.sum / self.num as f64 }
    }

    /// `variance` over stored samples.
    pub fn variance(&self) -> f64 {
        if self.num == 0 {
            return 0.0;
        }
        let avg = self.avg();
        (self.sum_of_squares / self.num as f64) - avg * avg
    }

    /// `sd`: standard deviation over stored samples.
    pub fn sd(&self) -> f64 {
        self.variance().max(0.0).sqrt()
    }

    /// `davg`: decaying average.
    #[inline]
    pub fn davg(&self) -> f64 {
        self.davg
    }

    /// `dvariance`: decaying variance.
    #[inline]
    pub fn dvariance(&self) -> f64 {
        self.dvariance
    }

    /// `dsd`: decaying standard deviation.
    #[inline]
    pub fn dsd(&self) -> f64 {
        self.dvariance.max(0.0).sqrt()
    }

    /// `maximum`.
    pub fn maximum(&self) -> f64 {
        if self.num == 0 {
            return 0.0;
        }
        self.sequence[..self.num].iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }

    /// `last` value added.
    pub fn last(&self) -> f64 {
        if self.num == 0 {
            0.0
        } else {
            // _next points at the oldest slot; last added is _next-1.
            let idx = (self.next + self.length - 1) % self.length;
            self.sequence[idx]
        }
    }

    /// `oldest` valid value.
    pub fn oldest(&self) -> f64 {
        if self.num == 0 {
            return 0.0;
        }
        // oldest is at _next when full; when partially filled it's 0.
        if self.num < self.length {
            self.sequence[0]
        } else {
            self.sequence[self.next]
        }
    }

    /// `predict_next`: linear regression over the stored samples.
    pub fn predict_next(&self) -> f64 {
        if self.num < 2 {
            return self.davg();
        }
        // least squares over the last `num` samples in insertion order
        let n = self.num as f64;
        let mut sx = 0.0;
        let mut sy = 0.0;
        let mut sxx = 0.0;
        let mut sxy = 0.0;
        for i in 0..self.num {
            let x = i as f64;
            // samples in insertion order: oldest at `next` when full
            let idx = if self.num == self.length {
                (self.next + i) % self.length
            } else {
                i
            };
            let y = self.sequence[idx];
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let denom = n * sxx - sx * sx;
        if denom.abs() < 1e-9 {
            return self.davg();
        }
        let slope = (n * sxy - sx * sy) / denom;
        let intercept = (sy - slope * sx) / n;
        slope * (n - 1.0) + intercept
    }

    #[inline]
    pub fn num_samples(&self) -> usize {
        self.num
    }

    pub fn is_empty(&self) -> bool {
        self.num == 0
    }
}

/// `G1PhaseDependentSeq`: keeps two TruncatedSeqs — one for young-only
/// phases, one for mixed phases.
#[derive(Debug)]
pub struct G1PhaseDependentSeq {
    young_only: TruncatedSeq,
    mixed: TruncatedSeq,
}

impl G1PhaseDependentSeq {
    pub fn new() -> Self {
        Self {
            young_only: TruncatedSeq::with_defaults(),
            mixed: TruncatedSeq::with_defaults(),
        }
    }

    pub fn add(&mut self, val: f64, for_young_only_phase: bool) {
        if for_young_only_phase {
            self.young_only.add(val);
        } else {
            self.mixed.add(val);
        }
    }

    pub fn seq(&self, for_young_only_phase: bool) -> &TruncatedSeq {
        if for_young_only_phase { &self.young_only } else { &self.mixed }
    }
}

impl Default for G1PhaseDependentSeq {
    fn default() -> Self {
        Self::new()
    }
}

/// `G1Predictions`: wraps a TruncatedSeq and produces predictions
/// including a stddev safety margin.
#[derive(Debug)]
pub struct G1Predictions {
    stddev_scale: f64,
}

impl G1Predictions {
    pub fn new(stddev_scale: f64) -> Self {
        assert!(stddev_scale >= 0.0);
        Self { stddev_scale }
    }

    /// `stddev_estimate`: for small sample sets scales the sample-set
    /// average linearly; HotSpot: <5 samples -> interpolate between
    /// 2x avg and 0.5x avg.
    fn stddev_estimate(&self, seq: &TruncatedSeq) -> f64 {
        let n = seq.num_samples();
        if n >= 5 {
            seq.dsd()
        } else if n == 0 {
            0.0
        } else {
            // linear interpolation between 2*avg (n=1) and 0.5*avg (n=4)
            let factor = 2.0 - (n as f64) * 0.5;
            seq.avg().abs() * factor.max(0.0)
        }
    }

    /// `predict`: davg + stddev_scale * stddev_estimate.
    pub fn predict(&self, seq: &TruncatedSeq) -> f64 {
        seq.davg() + self.stddev_scale * self.stddev_estimate(seq)
    }

    /// `predict_in_unit_interval`: clamped to [0,1].
    pub fn predict_in_unit_interval(&self, seq: &TruncatedSeq) -> f64 {
        self.predict(seq).clamp(0.0, 1.0)
    }

    /// `predict_zero_bounded`: clamped to >= 0.
    pub fn predict_zero_bounded(&self, seq: &TruncatedSeq) -> f64 {
        self.predict(seq).max(0.0)
    }
}
