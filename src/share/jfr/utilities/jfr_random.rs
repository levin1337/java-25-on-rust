/*
 * OpenJDK 25 HotSpot jfr/utilities/jfrRandom.hpp + .inline.hpp port
 */

const PRNG_MULT: u64 = 0x5DEECE66D;
const PRNG_ADD: u64 = 0xB;
const PRNG_MOD_MASK: u64 = (1u64 << 48) - 1;

pub fn next_rnd(rnd: u64) -> u64 {
    PRNG_MULT.wrapping_mul(rnd).wrapping_add(PRNG_ADD) & PRNG_MOD_MASK
}

// JfrPRNG — cheap pseudorandom number generator (lrand64).
pub struct JfrPrng {
    rnd: std::cell::Cell<u64>,
}

impl JfrPrng {
    pub fn new(seed: u64) -> JfrPrng {
        JfrPrng {
            rnd: std::cell::Cell::new(seed),
        }
    }

    pub fn next_uniform(&self) -> f64 {
        let r = next_rnd(self.rnd.get());
        self.rnd.set(r);
        // top 26 bits normalized by 2^26
        ((r >> (48 - 26)) as u32) as f64 / 67108864.0
    }
}
