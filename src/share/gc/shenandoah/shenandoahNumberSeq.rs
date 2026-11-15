/*
 * Port of OpenJDK 25 HotSpot Shenandoah Number Sequences
 * (share/gc/shenandoah/shenandoahNumberSeq.hpp / shenandoahNumberSeq.cpp,
 *  built on share/utilities/numberSeq.hpp / numberSeq.cpp)
 *
 * Implements AbsSeq/NumberSeq/TruncatedSeq (decaying and truncated
 * statistics), HdrSeq (high-dynamic-range histogram) and
 * BinaryMagnitudeSeq (power-of-two magnitude histogram).
 */

use std::sync::atomic::{AtomicUsize, Ordering};

pub const DEFAULT_ALPHA_VALUE: f64 = 0.3;

#[derive(Clone, Debug)]
pub struct AbsSeq {
    pub(crate) num: u64,
    pub(crate) sum: f64,
    pub(crate) sum_of_squares: f64,
    davg: f64,
    dvariance: f64,
    alpha: f64,
}

impl AbsSeq {
    pub fn new(alpha: f64) -> Self {
        Self {
            num: 0,
            sum: 0.0,
            sum_of_squares: 0.0,
            davg: 0.0,
            dvariance: 0.0,
            alpha,
        }
    }

    pub fn add(&mut self, val: f64) {
        if self.num == 0 {
            self.davg = val;
            self.dvariance = 0.0;
        } else {
            let diff = val - self.davg;
            let incr = self.alpha * diff;
            self.davg += incr;
            self.dvariance = (1.0 - self.alpha) * (self.dvariance + diff * incr);
        }
    }

    pub fn num(&self) -> u64 {
        self.num
    }

    pub fn total(&self) -> f64 {
        self.num as f64
    }

    pub fn sum(&self) -> f64 {
        self.sum
    }

    pub fn sum_of_squares(&self) -> f64 {
        self.sum_of_squares
    }

    pub fn avg(&self) -> f64 {
        if self.num == 0 {
            0.0
        } else {
            self.sum / self.total()
        }
    }

    pub fn variance(&self) -> f64 {
        if self.num <= 1 {
            return 0.0;
        }
        let x_bar = self.avg();
        let mut result = self.sum_of_squares / self.total() - x_bar * x_bar;
        if result < 0.0 {
            result = 0.0;
        }
        result
    }

    pub fn sd(&self) -> f64 {
        self.variance().sqrt()
    }

    pub fn davg(&self) -> f64 {
        self.davg
    }

    pub fn dvariance(&self) -> f64 {
        if self.num <= 1 {
            return 0.0;
        }
        let result = self.dvariance;
        if result < 0.0 { 0.0 } else { result }
    }

    pub fn dsd(&self) -> f64 {
        self.dvariance().sqrt()
    }
}

#[derive(Clone, Debug)]
pub struct NumberSeq {
    base: AbsSeq,
    last: f64,
    maximum: f64,
}

impl NumberSeq {
    pub fn new(alpha: f64) -> Self {
        Self {
            base: AbsSeq::new(alpha),
            last: 0.0,
            maximum: 0.0,
        }
    }

    pub fn add(&mut self, val: f64) {
        self.base.add(val);
        self.last = val;
        if self.base.num == 0 || val > self.maximum {
            self.maximum = val;
        }
        self.base.sum += val;
        self.base.sum_of_squares += val * val;
        self.base.num += 1;
    }

    pub fn num(&self) -> u64 {
        self.base.num()
    }

    pub fn avg(&self) -> f64 {
        self.base.avg()
    }

    pub fn variance(&self) -> f64 {
        self.base.variance()
    }

    pub fn sd(&self) -> f64 {
        self.base.sd()
    }

    pub fn davg(&self) -> f64 {
        self.base.davg()
    }

    pub fn dvariance(&self) -> f64 {
        self.base.dvariance()
    }

    pub fn dsd(&self) -> f64 {
        self.base.dsd()
    }

    pub fn maximum(&self) -> f64 {
        self.maximum
    }

    pub fn last(&self) -> f64 {
        self.last
    }

    pub fn sum(&self) -> f64 {
        self.base.sum()
    }
}

#[derive(Clone, Debug)]
pub struct TruncatedSeq {
    base: AbsSeq,
    sequence: Vec<f64>,
    length: usize,
    next: usize,
}

impl TruncatedSeq {
    pub fn new(length: usize, alpha: f64) -> Self {
        Self {
            base: AbsSeq::new(alpha),
            sequence: vec![0.0; length.max(1)],
            length: length.max(1),
            next: 0,
        }
    }

    pub fn add(&mut self, val: f64) {
        self.base.add(val);
        let old_val = self.sequence[self.next];
        self.base.sum -= old_val;
        self.base.sum_of_squares -= old_val * old_val;
        self.base.sum += val;
        self.base.sum_of_squares += val * val;
        self.sequence[self.next] = val;
        self.next = (self.next + 1) % self.length;
        if self.base.num < self.length as u64 {
            self.base.num += 1;
        }
        debug_assert!(self.base.variance() > -1.0, "variance should be >= 0");
    }

    pub fn maximum(&self) -> f64 {
        let num = self.base.num as usize;
        if num == 0 {
            return 0.0;
        }
        let mut ret = self.sequence[0];
        for i in 1..num {
            let val = self.sequence[i];
            if val > ret {
                ret = val;
            }
        }
        ret
    }

    pub fn last(&self) -> f64 {
        if self.base.num == 0 {
            0.0
        } else {
            self.sequence[(self.next + self.length - 1) % self.length]
        }
    }

    pub fn oldest(&self) -> f64 {
        let num = self.base.num as usize;
        if num == 0 {
            return 0.0;
        }
        if num < self.length {
            self.sequence[0]
        } else {
            self.sequence[self.next]
        }
    }

    pub fn predict_next(&self) -> f64 {
        let num = self.base.num as usize;
        if num == 0 {
            return 0.0;
        }
        if num == 1 {
            return self.sequence[0];
        }

        let n = num as f64;
        let mut x_squared_sum = 0.0;
        let mut x_sum = 0.0;
        let mut y_sum = 0.0;
        let mut xy_sum = 0.0;

        let first = (self.next + self.length - num) % self.length;
        for i in 0..num {
            let x = i as f64;
            let y = self.sequence[(first + i) % self.length];
            x_squared_sum += x * x;
            x_sum += x;
            y_sum += y;
            xy_sum += x * y;
        }
        let x_avg = x_sum / n;
        let y_avg = y_sum / n;

        let sxx = x_squared_sum - x_sum * x_sum / n;
        let sxy = xy_sum - x_sum * y_sum / n;
        if sxx == 0.0 {
            return y_avg;
        }
        let b1 = sxy / sxx;
        let b0 = y_avg - b1 * x_avg;
        b0 + b1 * n
    }

    pub fn avg(&self) -> f64 {
        self.base.avg()
    }

    pub fn num(&self) -> u64 {
        self.base.num
    }
}

const HDR_VAL_BUCKETS: usize = 512;
const HDR_MAG_BUCKETS: usize = 24;
const HDR_MAG_MINIMUM: i32 = -12;

#[derive(Clone, Debug)]
pub struct HdrSeq {
    base: NumberSeq,
    hdr: Vec<Option<Vec<u32>>>,
}

impl HdrSeq {
    pub fn new() -> Self {
        Self {
            base: NumberSeq::new(DEFAULT_ALPHA_VALUE),
            hdr: vec![None; HDR_MAG_BUCKETS],
        }
    }

    pub fn add(&mut self, mut val: f64) {
        if val < 0.0 {
            debug_assert!(false, "value ({val}) is not negative");
            val = 0.0;
        }

        self.base.add(val);

        let v0 = val;
        let (mag, mut v) = if v0 > 0.0 {
            let mut mag = 0i32;
            let mut v = v0;
            while v >= 1.0 {
                mag += 1;
                v /= 10.0;
            }
            while v < 0.1 {
                mag -= 1;
                v *= 10.0;
            }
            (mag, v)
        } else {
            (HDR_MAG_MINIMUM, v0)
        };
        let _ = &mut v;

        let mut bucket = -HDR_MAG_MINIMUM + mag;
        let mut sub_bucket = (v * HDR_VAL_BUCKETS as f64) as i32;

        if bucket < 0 {
            debug_assert!(false, "bucket index ({bucket}) underflow for value ({val})");
            bucket = 0;
        }
        if bucket >= HDR_MAG_BUCKETS as i32 {
            debug_assert!(false, "bucket index ({bucket}) overflow for value ({val})");
            bucket = HDR_MAG_BUCKETS as i32 - 1;
        }
        if sub_bucket < 0 {
            debug_assert!(false, "sub-bucket index ({sub_bucket}) underflow for value ({val})");
            sub_bucket = 0;
        }
        if sub_bucket >= HDR_VAL_BUCKETS as i32 {
            debug_assert!(false, "sub-bucket index ({sub_bucket}) overflow for value ({val})");
            sub_bucket = HDR_VAL_BUCKETS as i32 - 1;
        }

        let bucket_idx = bucket as usize;
        let sub_idx = sub_bucket as usize;
        let b = self.hdr[bucket_idx].get_or_insert_with(|| vec![0; HDR_VAL_BUCKETS]);
        b[sub_idx] += 1;
    }

    pub fn add_seq(&mut self, other: &HdrSeq) {
        if other.num() == 0 {
            return;
        }
        for mag in 0..HDR_MAG_BUCKETS {
            let Some(other_bucket) = &other.hdr[mag] else {
                continue;
            };
            let bucket = self.hdr[mag].get_or_insert_with(|| vec![0; HDR_VAL_BUCKETS]);
            for val in 0..HDR_VAL_BUCKETS {
                bucket[val] += other_bucket[val];
            }
        }
        self.base.last = other.base.last;
        self.base.maximum = self.base.maximum.max(other.base.maximum);
        self.base.base.sum += other.base.base.sum;
        self.base.base.sum_of_squares += other.base.base.sum_of_squares;
        self.base.base.num += other.base.base.num;
    }

    pub fn percentile(&self, level: f64) -> f64 {
        let target = ((level * self.num() as f64 / 100.0) as u64).max(1);
        let mut cnt: u64 = 0;
        for mag in 0..HDR_MAG_BUCKETS {
            if let Some(bucket) = &self.hdr[mag] {
                for (val, &c) in bucket.iter().enumerate() {
                    cnt += c as u64;
                    if cnt >= target {
                        return 10f64.powi(HDR_MAG_MINIMUM + mag as i32) * val as f64 / HDR_VAL_BUCKETS as f64;
                    }
                }
            }
        }
        self.maximum()
    }

    pub fn clear(&mut self) {
        for bucket in self.hdr.iter_mut() {
            if let Some(b) = bucket {
                b.fill(0);
            }
        }
        self.base = NumberSeq::new(DEFAULT_ALPHA_VALUE);
    }

    pub fn num(&self) -> u64 {
        self.base.num()
    }

    pub fn avg(&self) -> f64 {
        self.base.avg()
    }

    pub fn variance(&self) -> f64 {
        self.base.variance()
    }

    pub fn sd(&self) -> f64 {
        self.base.sd()
    }

    pub fn maximum(&self) -> f64 {
        self.base.maximum()
    }

    pub fn last(&self) -> f64 {
        self.base.last()
    }
}

impl Default for HdrSeq {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct BinaryMagnitudeSeq {
    sum: AtomicUsize,
    mags: Vec<AtomicUsize>,
}

impl BinaryMagnitudeSeq {
    pub fn new() -> Self {
        Self {
            sum: AtomicUsize::new(0),
            mags: (0..usize::BITS as usize)
                .map(|_| AtomicUsize::new(0))
                .collect(),
        }
    }

    pub fn clear(&mut self) {
        for m in self.mags.iter() {
            m.store(0, Ordering::Relaxed);
        }
        self.sum.store(0, Ordering::Relaxed);
    }

    pub fn add(&self, val: usize) {
        self.sum.fetch_add(val, Ordering::Relaxed);
        let mut mag = log2_graceful(val) + 1;
        if mag < 0 {
            debug_assert!(false, "bucket index ({mag}) underflow for value ({val})");
            mag = 0;
        }
        if mag >= self.mags.len() as i32 {
            debug_assert!(false, "bucket index ({mag}) overflow for value ({val})");
            mag = self.mags.len() as i32 - 1;
        }
        self.mags[mag as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub fn level(&self, level: i32) -> usize {
        if level >= 0 && (level as usize) < self.mags.len() {
            self.mags[level as usize].load(Ordering::Relaxed)
        } else {
            0
        }
    }

    pub fn num(&self) -> usize {
        self.mags.iter().map(|m| m.load(Ordering::Relaxed)).sum()
    }

    pub fn sum(&self) -> usize {
        self.sum.load(Ordering::Relaxed)
    }

    pub fn min_level(&self) -> i32 {
        for (i, m) in self.mags.iter().enumerate() {
            if m.load(Ordering::Relaxed) > 0 {
                return i as i32;
            }
        }
        -1
    }

    pub fn max_level(&self) -> i32 {
        for i in (0..self.mags.len()).rev() {
            if self.mags[i].load(Ordering::Relaxed) > 0 {
                return i as i32;
            }
        }
        -1
    }
}

impl Default for BinaryMagnitudeSeq {
    fn default() -> Self {
        Self::new()
    }
}

fn log2_graceful(val: usize) -> i32 {
    if val == 0 {
        return -1;
    }
    usize::BITS as i32 - 1 - val.leading_zeros() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_seq_stats() {
        let mut seq = NumberSeq::new(0.5);
        for v in [1.0, 2.0, 3.0, 4.0] {
            seq.add(v);
        }
        assert_eq!(seq.num(), 4);
        assert_eq!(seq.avg(), 2.5);
        assert_eq!(seq.maximum(), 4.0);
        assert_eq!(seq.last(), 4.0);
    }

    #[test]
    fn truncated_seq_window() {
        let mut seq = TruncatedSeq::new(3, 0.5);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            seq.add(v);
        }
        assert_eq!(seq.num(), 3);
        assert_eq!(seq.oldest(), 3.0);
        assert_eq!(seq.last(), 5.0);
        assert_eq!(seq.maximum(), 5.0);
        let pred = seq.predict_next();
        assert!(pred > 4.0 && pred < 8.0);
    }

    #[test]
    fn hdr_seq_percentile() {
        let mut seq = HdrSeq::new();
        for i in 0..100 {
            seq.add(i as f64 * 10.0);
        }
        let p50 = seq.percentile(50.0);
        let p90 = seq.percentile(90.0);
        assert!(p50 > 0.0 && p90 > p50);
        assert_eq!(seq.num(), 100);
    }

    #[test]
    fn binary_magnitude_histogram() {
        let seq = BinaryMagnitudeSeq::new();
        seq.add(1);
        seq.add(2);
        seq.add(4);
        seq.add(8);
        seq.add(16);
        assert_eq!(seq.num(), 5);
        assert_eq!(seq.sum(), 31);
        assert_eq!(seq.level(1), 1);
        assert_eq!(seq.level(5), 1);
        assert_eq!(seq.min_level(), 1);
        assert_eq!(seq.max_level(), 5);
    }
}
