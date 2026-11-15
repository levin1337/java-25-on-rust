use std::io::{self, Write};

pub const DEFAULT_ALPHA_VALUE: f64 = 0.3;

pub trait AbsSeq {
    fn add(&mut self, val: f64);
    fn maximum(&self) -> f64;
    fn last(&self) -> f64;
    fn num(&self) -> i32;
    fn sum(&self) -> f64;
    fn avg(&self) -> f64 {
        if self.num() == 0 {
            0.0
        } else {
            self.sum() / self.total() as f64
        }
    }
    fn total(&self) -> f64 {
        self.num() as f64
    }
    fn variance(&self) -> f64 {
        if self.num() <= 1 {
            return 0.0;
        }
        let x_bar = self.avg();
        let mut result = self.sum_of_squares() / self.total() - x_bar * x_bar;
        if result < 0.0 {
            result = 0.0;
        }
        result
    }
    fn sd(&self) -> f64 {
        self.variance().sqrt()
    }
    fn davg(&self) -> f64;
    fn dvariance(&self) -> f64 {
        if self.num() <= 1 {
            return 0.0;
        }
        let mut result = self.dvariance_raw();
        if result < 0.0 {
            result = 0.0;
        }
        result
    }
    fn dvariance_raw(&self) -> f64;
    fn dsd(&self) -> f64 {
        self.dvariance().sqrt()
    }
    fn sum_of_squares(&self) -> f64;
    fn dump_on(&self, s: &mut dyn Write) -> io::Result<()> {
        writeln!(
            s,
            "num={} sum={} sum_sq={} avg={} max={} last={} davg={} dvar={}",
            self.num(),
            self.sum(),
            self.sum_of_squares(),
            self.avg(),
            self.maximum(),
            self.last(),
            self.davg(),
            self.dvariance()
        )
    }
}

fn decaying_add(davg: &mut f64, dvariance: &mut f64, alpha: f64, val: f64, num: i32) {
    if num == 0 {
        *davg = val;
        *dvariance = 0.0;
    } else {
        let diff = val - *davg;
        let incr = alpha * diff;
        *davg += incr;
        *dvariance = (1.0 - alpha) * (*dvariance + diff * incr);
    }
}

#[derive(Clone, Debug)]
pub struct NumberSeq {
    num: i32,
    sum: f64,
    sum_of_squares: f64,
    davg: f64,
    dvariance: f64,
    alpha: f64,
    last: f64,
    maximum: f64,
}

impl NumberSeq {
    pub fn new(alpha: f64) -> Self {
        Self {
            num: 0,
            sum: 0.0,
            sum_of_squares: 0.0,
            davg: 0.0,
            dvariance: 0.0,
            alpha,
            last: 0.0,
            maximum: 0.0,
        }
    }

    pub fn with_default_alpha() -> Self {
        Self::new(DEFAULT_ALPHA_VALUE)
    }
}

impl AbsSeq for NumberSeq {
    fn add(&mut self, val: f64) {
        decaying_add(&mut self.davg, &mut self.dvariance, self.alpha, val, self.num);
        self.num += 1;
        self.sum += val;
        self.sum_of_squares += val * val;
        self.last = val;
        if val > self.maximum {
            self.maximum = val;
        }
    }

    fn maximum(&self) -> f64 {
        self.maximum
    }

    fn last(&self) -> f64 {
        self.last
    }

    fn num(&self) -> i32 {
        self.num
    }

    fn sum(&self) -> f64 {
        self.sum
    }

    fn sum_of_squares(&self) -> f64 {
        self.sum_of_squares
    }

    fn davg(&self) -> f64 {
        self.davg
    }

    fn dvariance_raw(&self) -> f64 {
        self.dvariance
    }
}

#[derive(Clone, Debug)]
pub struct TruncatedSeq {
    inner: NumberSeq,
    sequence: Vec<f64>,
    next: usize,
}

impl TruncatedSeq {
    pub const DEFAULT_SEQ_LENGTH: usize = 10;

    pub fn new(length: usize, alpha: f64) -> Self {
        Self {
            inner: NumberSeq::new(alpha),
            sequence: vec![0.0; length],
            next: 0,
        }
    }

    pub fn with_default() -> Self {
        Self::new(Self::DEFAULT_SEQ_LENGTH, DEFAULT_ALPHA_VALUE)
    }

    pub fn length(&self) -> usize {
        self.sequence.len()
    }

    pub fn oldest(&self) -> f64 {
        let num = self.inner.num as usize;
        if num == 0 {
            0.0
        } else if num < self.sequence.len() {
            self.sequence[0]
        } else {
            self.sequence[self.next]
        }
    }

    pub fn predict_next(&self) -> f64 {
        let num = self.inner.num as usize;
        if num == 0 {
            return 0.0;
        }
        if num == 1 {
            return self.sequence[0];
        }
        let len = self.sequence.len() as f64;
        let mut x_squared_sum = 0.0;
        let mut x_sum = 0.0;
        let mut y_sum = 0.0;
        let mut xy_sum = 0.0;
        let first = (self.next + self.sequence.len() - num) % self.sequence.len();
        for i in 0..num {
            let x = i as f64;
            let y = self.sequence[(first + i) % self.sequence.len()];
            x_squared_sum += x * x;
            x_sum += x;
            y_sum += y;
            xy_sum += x * y;
        }
        let x_avg = x_sum / len;
        let y_avg = y_sum / len;
        let sxx = x_squared_sum - x_sum * x_sum / len;
        let sxy = xy_sum - x_sum * y_sum / len;
        let b1 = sxy / sxx;
        let b0 = y_avg - b1 * x_avg;
        b0 + b1 * len
    }
}

impl AbsSeq for TruncatedSeq {
    fn add(&mut self, val: f64) {
        decaying_add(
            &mut self.inner.davg,
            &mut self.inner.dvariance,
            self.inner.alpha,
            val,
            self.inner.num,
        );
        let old_val = self.sequence[self.next];
        self.inner.sum -= old_val;
        self.inner.sum_of_squares -= old_val * old_val;
        self.inner.sum += val;
        self.inner.sum_of_squares += val * val;
        self.sequence[self.next] = val;
        self.next = (self.next + 1) % self.sequence.len();
        if self.inner.num < self.sequence.len() as i32 {
            self.inner.num += 1;
        }
        self.inner.last = val;
        if val > self.inner.maximum {
            self.inner.maximum = val;
        }
    }

    fn maximum(&self) -> f64 {
        let num = self.inner.num as usize;
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

    fn last(&self) -> f64 {
        let num = self.inner.num as usize;
        if num == 0 {
            return 0.0;
        }
        let last_index = (self.next + self.sequence.len() - 1) % self.sequence.len();
        self.sequence[last_index]
    }

    fn num(&self) -> i32 {
        self.inner.num
    }

    fn sum(&self) -> f64 {
        self.inner.sum
    }

    fn sum_of_squares(&self) -> f64 {
        self.inner.sum_of_squares
    }

    fn davg(&self) -> f64 {
        self.inner.davg
    }

    fn dvariance_raw(&self) -> f64 {
        self.inner.dvariance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_seq_stats() {
        let mut s = NumberSeq::with_default_alpha();
        for v in [1.0, 2.0, 3.0, 4.0] {
            s.add(v);
        }
        assert_eq!(s.num(), 4);
        assert_eq!(s.avg(), 2.5);
        assert_eq!(s.maximum(), 4.0);
        assert_eq!(s.last(), 4.0);
        assert!((s.variance() - 1.25).abs() < 1e-9);
        assert!(s.davg() > 0.0);
        assert!(s.dsd() >= 0.0);
    }

    #[test]
    fn truncated_seq() {
        let mut s = TruncatedSeq::new(3, DEFAULT_ALPHA_VALUE);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            s.add(v);
        }
        assert_eq!(s.num(), 3);
        assert_eq!(s.sum(), 12.0);
        assert_eq!(s.maximum(), 5.0);
        assert_eq!(s.last(), 5.0);
        assert_eq!(s.oldest(), 3.0);
        assert!(s.predict_next() > 0.0);
    }
}
