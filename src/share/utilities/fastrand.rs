use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn java_time_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

pub fn next_random(seed: u64) -> u64 {
    (seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB)) & ((1u64 << 48) - 1)
}

static GLOBAL_SEED: AtomicU64 = AtomicU64::new(0);

pub fn init_random() {
    let nanos = java_time_nanos();
    GLOBAL_SEED.store((nanos ^ 0x9E3779B97F4A7C15) & ((1u64 << 48) - 1), Ordering::Relaxed);
}

pub fn random() -> u32 {
    let seed = GLOBAL_SEED.load(Ordering::Relaxed);
    let next = next_random(seed);
    GLOBAL_SEED.store(next, Ordering::Relaxed);
    (next >> 16) as u32
}

#[derive(Clone, Debug)]
pub struct FastRandom {
    seed: u64,
}

impl FastRandom {
    pub fn new() -> Self {
        Self {
            seed: java_time_nanos() & ((1u64 << 48) - 1),
        }
    }

    pub fn with_seed(seed: u64) -> Self {
        Self {
            seed: seed & ((1u64 << 48) - 1),
        }
    }

    pub fn next(&mut self) -> u64 {
        self.seed = next_random(self.seed);
        self.seed
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next() >> 16) as u32
    }

    pub fn next_below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next() % n
    }
}

impl Default for FastRandom {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lcg_deterministic() {
        let mut a = FastRandom::with_seed(42);
        let mut b = FastRandom::with_seed(42);
        for _ in 0..10 {
            assert_eq!(a.next(), b.next());
        }
        let mut c = FastRandom::with_seed(42);
        assert_ne!(c.next(), FastRandom::with_seed(43).next());
    }

    #[test]
    fn bounded() {
        let mut r = FastRandom::with_seed(7);
        for _ in 0..100 {
            let v = r.next_below(10);
            assert!(v < 10);
        }
    }
}
