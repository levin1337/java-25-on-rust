/*
 * Port of OpenJDK 25 HotSpot ShenandoahMarkBitMap
 * (share/gc/shenandoah/shenandoahMarkBitMap.hpp / shenandoahMarkBitMap.cpp /
 *  shenandoahMarkBitMap.inline.hpp)
 *
 * Concurrent mark bitmap covering the reserved heap range. Each
 * 8-byte-aligned slot owns two adjacent bit positions: the even one is
 * the strong mark, the odd one the weak mark.
 */

use std::sync::atomic::{AtomicU64, Ordering};

pub type BmWord = u64;
pub type Idx = usize;

const FIND_ONES_FLIP: BmWord = 0;
const FIND_ZEROS_FLIP: BmWord = !0u64;
const SMALL_RANGE_WORDS: usize = 32;
const LOG_MIN_OBJ_ALIGNMENT: u32 = 3;
const BITS_PER_WORD: u32 = 64;

pub struct ShenandoahMarkBitMap {
    shift: u32,
    covered_start: u64,
    covered_end: u64,
    map: Vec<AtomicU64>,
    size: Idx,
}

impl ShenandoahMarkBitMap {
    pub fn compute_size(heap_size: usize) -> usize {
        heap_size / Self::mark_distance()
    }

    pub fn mark_distance() -> usize {
        8 * 8 / 2
    }

    pub fn heap_map_factor() -> usize {
        Self::mark_distance()
    }

    pub fn new(covered_start: u64, covered_end: u64) -> Self {
        let heap_bytes = covered_end - covered_start;
        let size_bits = ((heap_bytes as u128) * 2 >> LOG_MIN_OBJ_ALIGNMENT) as usize;
        let num_words = size_bits.div_ceil(BITS_PER_WORD as usize);
        Self {
            shift: LOG_MIN_OBJ_ALIGNMENT,
            covered_start,
            covered_end,
            map: (0..num_words).map(|_| AtomicU64::new(0)).collect(),
            size: size_bits,
        }
    }

    pub fn covered(&self) -> (u64, u64) {
        (self.covered_start, self.covered_end)
    }

    pub fn size(&self) -> Idx {
        self.size
    }

    #[inline]
    fn address_to_index(&self, addr: u64) -> Idx {
        (((addr.wrapping_sub(self.covered_start)) << 1) >> self.shift) as Idx
    }

    #[inline]
    fn index_to_address(&self, offset: Idx) -> u64 {
        self.covered_start + (((offset >> 1) as u64) << self.shift)
    }

    fn check_mark(&self, addr: u64) {
        debug_assert!(
            addr >= self.covered_start && addr < self.covered_end,
            "mark address {addr:#x} outside covered range"
        );
    }

    #[inline]
    fn bit_mask(bit: Idx) -> BmWord {
        1u64 << Self::bit_in_word(bit)
    }

    #[inline]
    fn bit_index(word: Idx) -> Idx {
        word << BITS_PER_WORD as usize
    }

    #[inline]
    fn bit_in_word(bit: Idx) -> u32 {
        (bit & (BITS_PER_WORD as usize - 1)) as u32
    }

    fn word_addr(&self, bit: Idx) -> &AtomicU64 {
        &self.map[Self::to_words_align_down(bit)]
    }

    fn map_word(&self, word: Idx) -> BmWord {
        self.map[word].load(Ordering::Relaxed)
    }

    fn at(&self, index: Idx) -> bool {
        (self.map_word(Self::to_words_align_down(index)) & Self::bit_mask(index)) != 0
    }

    #[inline]
    fn raw_to_words_align_down(bit: Idx) -> Idx {
        bit >> BITS_PER_WORD as usize
    }

    #[inline]
    fn to_words_align_down(&self, bit: Idx) -> Idx {
        debug_assert!(bit <= self.size);
        Self::raw_to_words_align_down(bit)
    }

    #[inline]
    fn to_words_align_up(&self, bit: Idx) -> Idx {
        debug_assert!(bit <= self.size);
        Self::raw_to_words_align_down(bit + (BITS_PER_WORD as usize - 1))
    }

    fn verify_index(&self, bit: Idx) {
        debug_assert!(bit < self.size);
    }

    fn verify_limit(&self, bit: Idx) {
        debug_assert!(bit <= self.size);
    }

    fn verify_range(&self, beg: Idx, end: Idx) {
        debug_assert!(beg <= end);
        self.verify_limit(end);
    }

    pub fn mark_strong(&self, heap_addr: u64, was_upgraded: &mut bool) -> bool {
        self.check_mark(heap_addr);
        let bit = self.address_to_index(heap_addr);
        self.verify_index(bit);
        let addr = self.word_addr(bit);
        let mask = Self::bit_mask(bit);
        let mask_weak = 1u64 << (Self::bit_in_word(bit) + 1);
        let mut old_val = addr.load(Ordering::Relaxed);
        loop {
            let new_val = old_val | mask;
            if new_val == old_val {
                *was_upgraded = false;
                return false;
            }
            let cur_val = addr
                .compare_exchange_weak(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                .unwrap_or_else(|v| v);
            if cur_val == old_val {
                *was_upgraded = (cur_val & mask_weak) != 0;
                return true;
            }
            old_val = cur_val;
        }
    }

    pub fn mark_weak(&self, heap_addr: u64) -> bool {
        self.check_mark(heap_addr);
        let bit = self.address_to_index(heap_addr);
        self.verify_index(bit);
        let addr = self.word_addr(bit);
        let mask_weak = 1u64 << (Self::bit_in_word(bit) + 1);
        let mask_strong = 1u64 << Self::bit_in_word(bit);
        let mut old_val = addr.load(Ordering::Relaxed);
        loop {
            if (old_val & mask_strong) != 0 {
                return false;
            }
            let new_val = old_val | mask_weak;
            if new_val == old_val {
                return false;
            }
            let cur_val = addr
                .compare_exchange_weak(old_val, new_val, Ordering::Relaxed, Ordering::Relaxed)
                .unwrap_or_else(|v| v);
            if cur_val == old_val {
                return true;
            }
            old_val = cur_val;
        }
    }

    pub fn is_marked_strong(&self, addr: u64) -> bool {
        self.check_mark(addr);
        self.at(self.address_to_index(addr))
    }

    pub fn is_marked_weak(&self, addr: u64) -> bool {
        self.check_mark(addr);
        self.at(self.address_to_index(addr) + 1)
    }

    pub fn is_marked(&self, addr: u64) -> bool {
        self.check_mark(addr);
        let index = self.address_to_index(addr);
        self.verify_index(index);
        let mask = 3u64 << Self::bit_in_word(index);
        (self.map_word(Self::to_words_align_down(index)) & mask) != 0
    }

    fn get_next_bit_impl<const FLIP: BmWord>(&self, l_index: Idx, r_index: Idx) -> Idx {
        self.verify_range(l_index, r_index);
        if l_index < r_index {
            let mut index = Self::to_words_align_down(l_index);
            let mut cword = (self.map_word(index) ^ FLIP) >> Self::bit_in_word(l_index);
            if (cword & 0x03) != 0 {
                return l_index;
            } else if cword != 0 {
                let result = l_index + cword.trailing_zeros() as Idx;
                if result < r_index {
                    return result;
                }
            } else {
                let limit = Self::to_words_align_up(r_index);
                while {
                    index += 1;
                    index < limit
                } {
                    cword = self.map_word(index) ^ FLIP;
                    if cword != 0 {
                        let result = Self::bit_index(index) + cword.trailing_zeros() as Idx;
                        if result < r_index {
                            return result;
                        }
                        break;
                    }
                }
            }
        }
        r_index
    }

    fn get_prev_bit_impl<const FLIP: BmWord>(&self, l_index: Idx, r_index: Idx) -> Idx {
        self.verify_range(l_index, r_index);
        if l_index < r_index {
            let mut index = Self::to_words_align_down(r_index);
            let shift = BITS_PER_WORD - 2 - Self::bit_in_word(r_index);
            let mut cword = (self.map_word(index) ^ FLIP) << shift;
            const FIRST_OBJECT_MASK: BmWord = 0x3u64 << (BITS_PER_WORD - 2);
            if (cword & FIRST_OBJECT_MASK) != 0 {
                return r_index;
            } else if cword != 0 {
                let result = r_index + 1 - cword.leading_zeros() as Idx;
                if result >= l_index {
                    return result;
                } else {
                    return r_index + 2;
                }
            } else {
                let limit = Self::to_words_align_up(l_index);
                while index > limit {
                    index -= 1;
                    cword = self.map_word(index) ^ FLIP;
                    if cword != 0 {
                        let result =
                            Self::bit_index(index + 1) - (cword.leading_zeros() as Idx + 1);
                        if result >= l_index {
                            return result;
                        } else {
                            return r_index + 2;
                        }
                    }
                }
                return r_index + 2;
            }
        }
        r_index + 2
    }

    fn get_next_one_offset(&self, l_offset: Idx, r_offset: Idx) -> Idx {
        self.get_next_bit_impl::<FIND_ONES_FLIP>(l_offset, r_offset)
    }

    fn get_prev_one_offset(&self, l_offset: Idx, r_offset: Idx) -> Idx {
        self.get_prev_bit_impl::<FIND_ONES_FLIP>(l_offset, r_offset)
    }

    pub fn is_bitmap_clear_range(&self, start: u64, end: u64) -> bool {
        if start == end {
            return true;
        }
        let addr_offset = self.address_to_index(align_up(start, 8));
        let limit_offset = self.address_to_index(end);
        let next_offset = self.get_next_one_offset(addr_offset, limit_offset);
        let result = self.index_to_address(next_offset);
        result == end
    }

    pub fn get_prev_marked_addr(&self, limit: u64, addr: u64) -> u64 {
        let addr_offset = self.address_to_index(align_down(addr, 8));
        let limit_offset = self.address_to_index(limit);
        let last_offset = self.get_prev_one_offset(limit_offset, addr_offset);
        if last_offset > addr_offset {
            addr + 8
        } else {
            self.index_to_address(last_offset)
        }
    }

    pub fn get_next_marked_addr(&self, addr: u64, limit: u64) -> u64 {
        let addr_offset = self.address_to_index(align_up(addr, 8));
        let limit_offset = self.address_to_index(limit);
        let next_offset = self.get_next_one_offset(addr_offset, limit_offset);
        self.index_to_address(next_offset)
    }

    fn inverted_bit_mask_for_range(&self, beg: Idx, end: Idx) -> BmWord {
        debug_assert!(end != 0, "does not work when end == 0");
        debug_assert!(
            beg == end || Self::to_words_align_down(beg) == Self::to_words_align_down(end - 1),
            "must be a single-word range"
        );
        let mut mask = Self::bit_mask(beg).wrapping_sub(1);
        if Self::bit_in_word(end) != 0 {
            mask |= !(Self::bit_mask(end).wrapping_sub(1));
        }
        mask
    }

    fn clear_range_within_word(&self, beg: Idx, end: Idx) {
        if beg != end {
            let mask = self.inverted_bit_mask_for_range(beg, end);
            let word = self.word_addr(beg);
            word.fetch_and(mask, Ordering::Relaxed);
        }
    }

    pub fn clear_range(&self, beg: Idx, end: Idx) {
        self.verify_range(beg, end);
        let beg_full_word = Self::to_words_align_up(beg);
        let end_full_word = Self::to_words_align_down(end);
        if beg_full_word < end_full_word {
            self.clear_range_within_word(beg, Self::bit_index(beg_full_word));
            self.clear_range_of_words(beg_full_word, end_full_word);
            self.clear_range_within_word(Self::bit_index(end_full_word), end);
        } else {
            let boundary = Self::bit_index(beg_full_word).min(end);
            self.clear_range_within_word(beg, boundary);
            self.clear_range_within_word(boundary, end);
        }
    }

    fn is_small_range_of_words(beg_full_word: Idx, end_full_word: Idx) -> bool {
        beg_full_word + SMALL_RANGE_WORDS >= end_full_word
    }

    fn clear_large_range(&self, beg: Idx, end: Idx) {
        self.verify_range(beg, end);
        let beg_full_word = Self::to_words_align_up(beg);
        let end_full_word = Self::to_words_align_down(end);
        if Self::is_small_range_of_words(beg_full_word, end_full_word) {
            self.clear_range(beg, end);
            return;
        }
        self.clear_range_within_word(beg, Self::bit_index(beg_full_word));
        self.clear_large_range_of_words(beg_full_word, end_full_word);
        self.clear_range_within_word(Self::bit_index(end_full_word), end);
    }

    pub fn clear_range_large(&self, start: u64, end: u64) {
        let beg = self.address_to_index(start);
        let end_idx = self.address_to_index(end);
        self.clear_large_range(beg, end_idx);
    }

    pub fn clear_range_of_words(&self, beg: Idx, end: Idx) {
        Self::clear_range_of_words_impl(&self.map, beg, end);
    }

    fn clear_large_range_of_words(&self, beg: Idx, end: Idx) {
        debug_assert!(beg <= end);
        for w in &self.map[beg..end] {
            w.store(0, Ordering::Relaxed);
        }
    }

    pub fn clear_all(&self) {
        for w in &self.map {
            w.store(0, Ordering::Relaxed);
        }
    }

    pub fn is_bitmap_clear(&self) -> bool {
        self.map.iter().all(|w| w.load(Ordering::Relaxed) == 0)
    }

    pub fn is_bitmap_range_within_region_clear(&self, start: u64, end: u64) -> bool {
        self.is_bitmap_clear_range(start, end)
    }

    fn clear_range_of_words_impl(map: &[AtomicU64], beg: Idx, end: Idx) {
        for w in &map[beg..end] {
            w.store(0, Ordering::Relaxed);
        }
    }
}

#[inline]
fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

#[inline]
fn align_down(addr: u64, align: u64) -> u64 {
    addr & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap() -> ShenandoahMarkBitMap {
        ShenandoahMarkBitMap::new(0x1000, 0x1000 + 4 * 1024 * 1024)
    }

    #[test]
    fn strong_and_weak_marks() {
        let bm = bitmap();
        let addr = 0x1000 + 64;
        let mut upgraded = false;
        assert!(bm.mark_strong(addr, &mut upgraded));
        assert!(!upgraded);
        assert!(bm.is_marked_strong(addr));
        assert!(bm.is_marked(addr));
        assert!(!bm.mark_strong(addr, &mut upgraded));

        let addr2 = 0x1000 + 128;
        assert!(bm.mark_weak(addr2));
        assert!(bm.is_marked_weak(addr2));
        assert!(bm.is_marked(addr2));
        assert!(!bm.mark_weak(addr2));

        let mut upgraded2 = false;
        assert!(bm.mark_strong(addr2, &mut upgraded2));
        assert!(upgraded2);
    }

    #[test]
    fn next_and_prev_marked() {
        let bm = bitmap();
        bm.mark_strong(0x1000 + 0x100, &mut false);
        bm.mark_strong(0x1000 + 0x300, &mut false);
        bm.mark_strong(0x1000 + 0x500, &mut false);
        assert_eq!(bm.get_next_marked_addr(0x1000, 0x1000 + 0x1000), 0x1000 + 0x100);
        assert_eq!(bm.get_next_marked_addr(0x1000 + 0x101, 0x1000 + 0x1000), 0x1000 + 0x300);
        assert_eq!(bm.get_prev_marked_addr(0x1000 + 0x1000, 0x1000), 0x1000 + 0x500);
    }

    #[test]
    fn clear_range() {
        let bm = bitmap();
        for off in [0x40usize, 0x80, 0x100, 0x180, 0x200] {
            bm.mark_strong(0x1000 + off, &mut false);
        }
        bm.clear_range(bm.address_to_index(0x1000 + 0x80), bm.address_to_index(0x1000 + 0x180));
        assert!(!bm.is_marked_strong(0x1000 + 0x80));
        assert!(!bm.is_marked_strong(0x1000 + 0x100));
        assert!(!bm.is_marked_strong(0x1000 + 0x180 - 8));
        assert!(bm.is_marked_strong(0x1000 + 0x40));
        assert!(bm.is_marked_strong(0x1000 + 0x200));
    }
}
