/*
 * Port of OpenJDK 25 HotSpot OopMapCache (share/interpreter/oopMapCache.hpp, oopMapCache.cpp)
 *
 * Caches interpreter OOP maps (locals and expression stack slots containing live reference handles)
 * for accurate GC root scanning across safepoints and frames.
 */

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterOopMap {
    bci: u16,
    mask_size: usize,
    num_oops: usize,
    // Bitmask: bit i is 1 if slot i is an OOP reference, 0 if primitive or uninitialized
    bit_mask: Vec<u64>,
}

impl InterpreterOopMap {
    pub fn new(bci: u16, max_slots: usize) -> Self {
        let words = (max_slots + 63) / 64;
        Self {
            bci,
            mask_size: max_slots,
            num_oops: 0,
            bit_mask: vec![0; words],
        }
    }

    pub fn bci(&self) -> u16 {
        self.bci
    }

    pub fn mask_size(&self) -> usize {
        self.mask_size
    }

    pub fn num_oops(&self) -> usize {
        self.num_oops
    }

    pub fn set_oop(&mut self, slot: usize) {
        if slot < self.mask_size {
            let word = slot / 64;
            let bit = slot % 64;
            let mask = 1u64 << bit;
            if (self.bit_mask[word] & mask) == 0 {
                self.bit_mask[word] |= mask;
                self.num_oops += 1;
            }
        }
    }

    pub fn is_oop(&self, slot: usize) -> bool {
        if slot >= self.mask_size {
            return false;
        }
        let word = slot / 64;
        let bit = slot % 64;
        (self.bit_mask[word] & (1u64 << bit)) != 0
    }

    pub fn iterate_oops<F: FnMut(usize)>(&self, mut f: F) {
        for slot in 0..self.mask_size {
            if self.is_oop(slot) {
                f(slot);
            }
        }
    }
}

pub struct OopMapCache {
    entries: HashMap<(u64, u16), InterpreterOopMap>,
}

impl Default for OopMapCache {
    fn default() -> Self {
        Self::new()
    }
}

impl OopMapCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn lookup(&self, method_id: u64, bci: u16) -> Option<&InterpreterOopMap> {
        self.entries.get(&(method_id, bci))
    }

    pub fn insert(&mut self, method_id: u64, oop_map: InterpreterOopMap) {
        self.entries.insert((method_id, oop_map.bci()), oop_map);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
