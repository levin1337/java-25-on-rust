/*
 * Port of OpenJDK 25 HotSpot OopMap (share/compiler/oopMap.hpp / oopMap.cpp)
 *
 * Implements frame maps describing which stack slots and registers hold live
 * OOP references at safepoints and calls, enabling accurate GC stack root scanning.
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OopMapType {
    Oop,
    NarrowOop,
    CalleeSaved,
    DerivedOop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OopMapSlot {
    pub slot: usize,
    pub map_type: OopMapType,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OopMap {
    pub pc_offset: usize,
    pub slots: Vec<OopMapSlot>,
    pub registers: Vec<u32>,
}

impl OopMap {
    pub fn new(pc_offset: usize) -> Self {
        Self {
            pc_offset,
            slots: Vec::new(),
            registers: Vec::new(),
        }
    }

    pub fn set_oop(&mut self, slot: usize) {
        self.slots.push(OopMapSlot {
            slot,
            map_type: OopMapType::Oop,
        });
    }

    pub fn set_narrow_oop(&mut self, slot: usize) {
        self.slots.push(OopMapSlot {
            slot,
            map_type: OopMapType::NarrowOop,
        });
    }

    pub fn set_register_oop(&mut self, reg_id: u32) {
        if !self.registers.contains(&reg_id) {
            self.registers.push(reg_id);
        }
    }

    pub fn has_oop_at_slot(&self, slot: usize) -> bool {
        self.slots
            .iter()
            .any(|s| s.slot == slot && s.map_type == OopMapType::Oop)
    }

    pub fn oop_count(&self) -> usize {
        self.slots.len() + self.registers.len()
    }
}

#[derive(Clone, Debug, Default)]
pub struct OopMapSet {
    maps: Vec<OopMap>,
}

impl OopMapSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_gc_map(&mut self, map: OopMap) {
        self.maps.push(map);
    }

    pub fn find_map_at_offset(&self, pc_offset: usize) -> Option<&OopMap> {
        self.maps.iter().find(|m| m.pc_offset == pc_offset)
    }

    pub fn total_maps(&self) -> usize {
        self.maps.len()
    }
}
