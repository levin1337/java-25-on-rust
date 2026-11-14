/*
 * Port of OpenJDK 25 HotSpot BytecodeHistogram (share/interpreter/bytecodeHistogram.hpp, bytecodeHistogram.cpp)
 *
 * Collects runtime profiling statistics for bytecode frequency and opcode execution profiling.
 */

use super::bytecodes;
use std::time::Instant;

pub struct BytecodeCounter {
    counter_value: u64,
    start_time: Instant,
}

impl Default for BytecodeCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl BytecodeCounter {
    pub fn new() -> Self {
        Self {
            counter_value: 0,
            start_time: Instant::now(),
        }
    }

    pub fn reset(&mut self) {
        self.counter_value = 0;
        self.start_time = Instant::now();
    }

    pub fn count_bytecode(&mut self) {
        self.counter_value = self.counter_value.saturating_add(1);
    }

    pub fn counter_value(&self) -> u64 {
        self.counter_value
    }

    pub fn elapsed_seconds(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    pub fn frequency(&self) -> f64 {
        let elapsed = self.elapsed_seconds();
        if elapsed > 0.0 {
            self.counter_value as f64 / elapsed
        } else {
            0.0
        }
    }
}

pub struct BytecodeHistogram {
    counters: [u64; 256],
}

impl Default for BytecodeHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl BytecodeHistogram {
    pub fn new() -> Self {
        Self { counters: [0; 256] }
    }

    pub fn reset(&mut self) {
        self.counters = [0; 256];
    }

    pub fn record(&mut self, opcode: u8) {
        self.counters[opcode as usize] = self.counters[opcode as usize].saturating_add(1);
    }

    pub fn count(&self, opcode: u8) -> u64 {
        self.counters[opcode as usize]
    }

    pub fn total_count(&self) -> u64 {
        self.counters.iter().sum()
    }

    pub fn sorted_frequencies(&self) -> Vec<(u8, &'static str, u64, f32)> {
        let total = self.total_count();
        let mut result = Vec::new();
        for (op, &count) in self.counters.iter().enumerate() {
            if count > 0 {
                let name = bytecodes::name(op as u8).unwrap_or("unknown");
                let pct = if total > 0 {
                    (count as f64 / total as f64 * 100.0) as f32
                } else {
                    0.0
                };
                result.push((op as u8, name, count, pct));
            }
        }
        result.sort_by(|a, b| b.2.cmp(&a.2));
        result
    }
}

pub struct BytecodePairHistogram {
    counters: Box<[u64; 256 * 256]>,
    last_code: Option<u8>,
}

impl Default for BytecodePairHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl BytecodePairHistogram {
    pub fn new() -> Self {
        Self {
            counters: vec![0u64; 256 * 256].into_boxed_slice().try_into().unwrap(),
            last_code: None,
        }
    }

    pub fn reset(&mut self) {
        self.counters.fill(0);
        self.last_code = None;
    }

    pub fn record(&mut self, opcode: u8) {
        if let Some(prev) = self.last_code {
            let idx = ((prev as usize) << 8) | (opcode as usize);
            self.counters[idx] = self.counters[idx].saturating_add(1);
        }
        self.last_code = Some(opcode);
    }

    pub fn pair_count(&self, prev: u8, curr: u8) -> u64 {
        let idx = ((prev as usize) << 8) | (curr as usize);
        self.counters[idx]
    }
}
