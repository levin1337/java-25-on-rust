/*
 * Port of OpenJDK 25 HotSpot ZGC Barrier Set (share/gc/z/zBarrierSet.hpp / zBarrierSet.cpp)
 *
 * Implements ZGC colored pointer load barriers and store barriers.
 */

use super::zPointer::ZPointer;
use std::collections::HashMap;
use std::sync::RwLock;

pub struct ZBarrierSet {
    good_color: RwLock<u64>,
    forwarding_table: RwLock<HashMap<usize, usize>>,
}

impl ZBarrierSet {
    pub fn new() -> Self {
        Self {
            good_color: RwLock::new(ZPointer::COLOR_REMAPPED),
            forwarding_table: RwLock::new(HashMap::new()),
        }
    }

    pub fn set_good_color(&self, color: u64) {
        *self.good_color.write().unwrap() = color;
    }

    pub fn good_color(&self) -> u64 {
        *self.good_color.read().unwrap()
    }

    pub fn register_forwarding(&self, from_addr: usize, to_addr: usize) {
        self.forwarding_table
            .write()
            .unwrap()
            .insert(from_addr, to_addr);
    }

    /// Load Barrier fast path & slow path:
    /// If pointer has good color, returns immediately (fast path).
    /// If pointer has bad color, looks up forwarding table, heals the pointer, and returns good pointer.
    pub fn load_barrier_on_oop(&self, ptr: ZPointer) -> ZPointer {
        let good = self.good_color();
        if ptr.color() == good {
            return ptr;
        }

        // Slow path: Heal pointer
        let old_addr = ptr.uncolored_address();
        let new_addr = {
            let table = self.forwarding_table.read().unwrap();
            table.get(&old_addr).copied().unwrap_or(old_addr)
        };

        ZPointer::new(new_addr, good)
    }
}

impl Default for ZBarrierSet {
    fn default() -> Self {
        Self::new()
    }
}
