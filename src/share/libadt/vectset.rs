use std::ptr;

use crate::share::libadt::arena::{resource_area, Arena};

const WORD_BITS: u32 = 5;
const BIT_MASK: u32 = 31;

pub struct VectorSet {
    size: u32,
    data_size: u32,
    pub(crate) data: *mut u32,
    arena: *mut Arena,
}

impl VectorSet {
    pub fn new() -> VectorSet {
        Self::with_arena(resource_area())
    }

    pub fn with_arena(arena: &mut Arena) -> VectorSet {
        unsafe {
            let data = arena.amalloc_words(2 * size_of::<u32>()) as *mut u32;
            *data = 0;
            *data.add(1) = 0;
            VectorSet {
                size: 2,
                data_size: 2,
                data,
                arena: arena as *mut Arena,
            }
        }
    }

    fn grow(&mut self, new_word_capacity: u32) {
        debug_assert!(new_word_capacity >= self.size);
        debug_assert!(new_word_capacity < (1 << 30));
        let x = (new_word_capacity + 1).next_power_of_two();
        unsafe {
            if x > self.data_size {
                let arena = &mut *self.arena;
                let old_bytes = self.size as usize * size_of::<u32>();
                let new_bytes = x as usize * size_of::<u32>();
                self.data = arena.arealloc(
                    self.data as *mut u8,
                    old_bytes,
                    new_bytes,
                ) as *mut u32;
                self.data_size = x;
            }
            ptr::write_bytes(
                self.data.add(self.size as usize),
                0,
                (x - self.size) as usize,
            );
            self.size = x;
        }
    }

    fn maybe_grow(&mut self, new_word_capacity: u32) {
        if new_word_capacity >= self.size {
            self.grow(new_word_capacity);
        }
    }

    pub fn insert(&mut self, elem: u32) {
        let word = elem >> WORD_BITS;
        let mask = 1u32 << (elem & BIT_MASK);
        self.maybe_grow(word);
        unsafe {
            *self.data.add(word as usize) |= mask;
        }
    }

    pub fn is_empty(&self) -> bool {
        unsafe {
            for i in 0..self.size as usize {
                if *self.data.add(i) != 0 {
                    return false;
                }
            }
        }
        true
    }

    pub fn reset(&mut self) {
        self.size = 0;
    }

    pub fn clear(&mut self) {
        self.reset();
    }

    pub fn test_set(&mut self, elem: u32) -> bool {
        let word = elem >> WORD_BITS;
        self.maybe_grow(word);
        let mask = 1u32 << (elem & BIT_MASK);
        unsafe {
            let data = *self.data.add(word as usize);
            *self.data.add(word as usize) = data | mask;
            (data & mask) != 0
        }
    }

    pub fn test(&self, elem: u32) -> bool {
        let word = elem >> WORD_BITS;
        if word >= self.size {
            return false;
        }
        let mask = 1u32 << (elem & BIT_MASK);
        unsafe { (*self.data.add(word as usize) & mask) != 0 }
    }

    pub fn remove(&mut self, elem: u32) {
        let word = elem >> WORD_BITS;
        if word >= self.size {
            return;
        }
        let mask = 1u32 << (elem & BIT_MASK);
        unsafe {
            *self.data.add(word as usize) &= !mask;
        }
    }

    pub fn set(&mut self, elem: u32) {
        let word = elem >> WORD_BITS;
        self.maybe_grow(word);
        let mask = 1u32 << (elem & BIT_MASK);
        unsafe {
            *self.data.add(word as usize) |= mask;
        }
    }
}
