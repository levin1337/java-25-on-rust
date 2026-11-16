use std::sync::OnceLock;
use std::time::Instant;

static EPOCH: OnceLock<Instant> = OnceLock::new();

fn epoch() -> &'static Instant {
    EPOCH.get_or_init(Instant::now)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ElapsedCounterSource;

impl ElapsedCounterSource {

    pub fn frequency() -> u64 {
        1_000_000_000
    }

    pub fn now() -> i64 {
        epoch().elapsed().as_nanos() as i64
    }

    pub fn seconds(value: i64) -> f64 {
        value as f64 / Self::frequency() as f64
    }

    pub fn milliseconds(value: i64) -> u64 {
        (value as f64 * 1_000.0 / Self::frequency() as f64) as u64
    }

    pub fn microseconds(value: i64) -> u64 {
        (value as f64 * 1_000_000.0 / Self::frequency() as f64) as u64
    }

    pub fn nanoseconds(value: i64) -> u64 {
        value as u64
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct FastUnorderedElapsedCounterSource;

impl FastUnorderedElapsedCounterSource {

    pub fn frequency() -> u64 {
        ElapsedCounterSource::frequency()
    }

    pub fn now() -> i64 {
        ElapsedCounterSource::now()
    }

    pub fn seconds(value: i64) -> f64 {
        ElapsedCounterSource::seconds(value)
    }

    pub fn milliseconds(value: i64) -> u64 {
        ElapsedCounterSource::milliseconds(value)
    }

    pub fn microseconds(value: i64) -> u64 {
        ElapsedCounterSource::microseconds(value)
    }

    pub fn nanoseconds(value: i64) -> u64 {
        ElapsedCounterSource::nanoseconds(value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PairRep {
    pub val1: i64,
    pub val2: i64,
}

impl PairRep {
    pub fn new() -> Self {
        Self { val1: 0, val2: 0 }
    }

    pub fn add_assign(&mut self, rhs: &PairRep) {
        self.val1 += rhs.val1;
        self.val2 += rhs.val2;
    }

    pub fn sub_assign(&mut self, rhs: &PairRep) {
        self.val1 -= rhs.val1;
        self.val2 -= rhs.val2;
    }
}

impl PartialOrd for PairRep {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PairRep {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.val1.cmp(&other.val1)
    }
}

pub type CompositeTime = PairRep;

#[derive(Clone, Copy, Debug, Default)]
pub struct CompositeElapsedCounterSource;

impl CompositeElapsedCounterSource {

    pub fn frequency() -> u64 {
        ElapsedCounterSource::frequency()
    }

    pub fn now() -> CompositeTime {
        CompositeTime {
            val1: ElapsedCounterSource::now(),
            val2: FastUnorderedElapsedCounterSource::now(),
        }
    }

    pub fn seconds(value: CompositeTime) -> f64 {
        ElapsedCounterSource::seconds(value.val1)
    }

    pub fn milliseconds(value: CompositeTime) -> u64 {
        ElapsedCounterSource::milliseconds(value.val1)
    }

    pub fn microseconds(value: CompositeTime) -> u64 {
        ElapsedCounterSource::microseconds(value.val1)
    }

    pub fn nanoseconds(value: CompositeTime) -> u64 {
        ElapsedCounterSource::nanoseconds(value.val1)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Representation {
    pub rep: i64,
}

impl Representation {
    pub fn seconds(&self) -> f64 {
        ElapsedCounterSource::seconds(self.rep)
    }

    pub fn milliseconds(&self) -> u64 {
        ElapsedCounterSource::milliseconds(self.rep)
    }

    pub fn microseconds(&self) -> u64 {
        ElapsedCounterSource::microseconds(self.rep)
    }

    pub fn nanoseconds(&self) -> u64 {
        ElapsedCounterSource::nanoseconds(self.rep)
    }

    pub fn add_assign(&mut self, rhs: &Self) {
        self.rep += rhs.rep;
    }

    pub fn sub_assign(&mut self, rhs: &Self) {
        self.rep -= rhs.rep;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TimeInstant {
    rep: i64,
}

impl TimeInstant {
    pub fn stamp(&mut self) {
        self.rep = ElapsedCounterSource::now();
    }

    pub fn now() -> Self {
        let mut t = Self { rep: 0 };
        t.stamp();
        t
    }

    pub fn value(&self) -> i64 {
        self.rep
    }

    pub fn nanoseconds(&self) -> u64 {
        ElapsedCounterSource::nanoseconds(self.rep)
    }

    pub fn milliseconds(&self) -> u64 {
        ElapsedCounterSource::milliseconds(self.rep)
    }

    pub fn microseconds(&self) -> u64 {
        ElapsedCounterSource::microseconds(self.rep)
    }

    pub fn seconds(&self) -> f64 {
        ElapsedCounterSource::seconds(self.rep)
    }
}

impl std::ops::Sub for TimeInstant {
    type Output = Tickspan;

    fn sub(self, start: Self) -> Self::Output {
        Tickspan {
            rep: self.rep - start.rep,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tickspan {
    rep: i64,
}

impl Tickspan {
    pub fn nanoseconds(&self) -> u64 {
        ElapsedCounterSource::nanoseconds(self.rep)
    }

    pub fn milliseconds(&self) -> u64 {
        ElapsedCounterSource::milliseconds(self.rep)
    }

    pub fn microseconds(&self) -> u64 {
        ElapsedCounterSource::microseconds(self.rep)
    }

    pub fn seconds(&self) -> f64 {
        ElapsedCounterSource::seconds(self.rep)
    }

    pub fn add_assign(&mut self, rhs: &Self) {
        self.rep += rhs.rep;
    }

    pub fn sub_assign(&mut self, rhs: &Self) {
        self.rep -= rhs.rep;
    }
}

impl std::ops::Add for Tickspan {
    type Output = Tickspan;

    fn add(self, rhs: Self) -> Self::Output {
        Tickspan {
            rep: self.rep + rhs.rep,
        }
    }
}

impl std::ops::Sub for Tickspan {
    type Output = Tickspan;

    fn sub(self, rhs: Self) -> Self::Output {
        Tickspan {
            rep: self.rep - rhs.rep,
        }
    }
}

impl std::ops::AddAssign for Tickspan {
    fn add_assign(&mut self, rhs: Self) {
        self.rep += rhs.rep;
    }
}

impl std::ops::SubAssign for Tickspan {
    fn sub_assign(&mut self, rhs: Self) {
        self.rep -= rhs.rep;
    }
}

pub type Ticks = TimeInstant;
pub type TickSpan = Tickspan;

pub fn time_stamp_counter() -> u64 {
    ElapsedCounterSource::now() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickspan_basic() {
        let a = TimeInstant::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = TimeInstant::now();
        let span = b - a;
        assert!(span.milliseconds() >= 1);
        assert!(span.seconds() > 0.0);
        let doubled = span + span;
        assert!(doubled.nanoseconds() >= span.nanoseconds());
    }

    #[test]
    fn counter_source() {
        let v = ElapsedCounterSource::now();
        assert!(v >= 0);
        assert_eq!(ElapsedCounterSource::frequency(), 1_000_000_000);
        assert_eq!(ElapsedCounterSource::milliseconds(1_000_000_000), 1000);
        assert!((ElapsedCounterSource::seconds(500_000_000) - 0.5).abs() < 1e-9);
        let c = CompositeElapsedCounterSource::now();
        assert!(c.val1 >= 0);
        assert!(c.val2 >= 0);
    }
}
