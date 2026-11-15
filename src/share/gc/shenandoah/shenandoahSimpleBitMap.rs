/*
 * Port of OpenJDK 25 HotSpot ShenandoahSimpleBitMap
 * (share/gc/shenandoah/shenandoahSimpleBitMap.hpp / shenandoahSimpleBitMap.cpp /
 *  shenandoahSimpleBitMap.inline.hpp)
 *
 * Bit map over region indices with forward/reverse search and
 * consecutive-set-bit probes used by free-set and humongous allocation.
 */

pub const BITS_PER_WORD: usize = u64::BITS as usize;
pub const LOG_BITS_PER_WORD: u32 = 6;

pub type Idx = isize;

#[inline]
fn count_trailing_zeros_u64(x: u64) -> u32 {
    x.trailing_zeros()
}

#[inline]
fn count_leading_zeros_u64(x: u64) -> u32 {
    x.leading_zeros()
}

#[inline]
fn tail_mask(bit_number: usize) -> u64 {
    if bit_number >= BITS_PER_WORD {
        u64::MAX
    } else {
        (1u64 << bit_number) - 1
    }
}

pub struct ShenandoahSimpleBitMap {
    num_bits: Idx,
    num_words: usize,
    bitmap: Vec<u64>,
}

impl ShenandoahSimpleBitMap {
    pub fn new(num_bits: usize) -> Self {
        let num_words = (num_bits + BITS_PER_WORD - 1) / BITS_PER_WORD;
        Self {
            num_bits: num_bits as Idx,
            num_words,
            bitmap: vec![0; num_words],
        }
    }

    pub fn clear_all(&mut self) {
        for w in self.bitmap.iter_mut() {
            *w = 0;
        }
    }

    #[inline]
    pub fn size(&self) -> Idx {
        self.num_bits
    }

    #[inline]
    pub fn num_words(&self) -> usize {
        self.num_words
    }

    #[inline]
    pub fn aligned_index(&self, idx: Idx) -> Idx {
        debug_assert!(idx >= 0 && idx < self.num_bits);
        idx & !(BITS_PER_WORD as Idx - 1)
    }

    #[inline]
    pub fn alignment(&self) -> Idx {
        BITS_PER_WORD as Idx
    }

    #[inline]
    pub fn bits_at(&self, idx: Idx) -> u64 {
        debug_assert!(idx >= 0 && idx < self.num_bits);
        let array_idx = (idx >> LOG_BITS_PER_WORD) as usize;
        self.bitmap[array_idx]
    }

    #[inline]
    pub fn set_bit(&mut self, idx: Idx) {
        debug_assert!(idx >= 0 && idx < self.num_bits);
        let array_idx = (idx >> LOG_BITS_PER_WORD) as usize;
        let bit_number = (idx & (BITS_PER_WORD as Idx - 1)) as u32;
        self.bitmap[array_idx] |= 1u64 << bit_number;
    }

    #[inline]
    pub fn clear_bit(&mut self, idx: Idx) {
        debug_assert!(idx >= 0 && idx < self.num_bits);
        let array_idx = (idx >> LOG_BITS_PER_WORD) as usize;
        let bit_number = (idx & (BITS_PER_WORD as Idx - 1)) as u32;
        self.bitmap[array_idx] &= !(1u64 << bit_number);
    }

    #[inline]
    pub fn is_set(&self, idx: Idx) -> bool {
        debug_assert!(idx >= 0 && idx < self.num_bits);
        let array_idx = (idx >> LOG_BITS_PER_WORD) as usize;
        let bit_number = (idx & (BITS_PER_WORD as Idx - 1)) as u32;
        (self.bitmap[array_idx] & (1u64 << bit_number)) != 0
    }

    fn count_leading_ones(&self, start_idx: Idx) -> usize {
        debug_assert!(start_idx >= 0 && start_idx < self.num_bits);
        let mut array_idx = (start_idx >> LOG_BITS_PER_WORD) as usize;
        let mut bit_number = (start_idx & (BITS_PER_WORD as Idx - 1)) as usize;
        let mut element_bits = self.bitmap[array_idx];
        let mut mask = !tail_mask(bit_number);
        let mut counted_ones = 0usize;
        while (element_bits & mask) == mask {
            let found_ones = BITS_PER_WORD - bit_number;
            counted_ones += found_ones;
            array_idx += 1;
            element_bits = self.bitmap[array_idx];
            bit_number = 0;
            mask = u64::MAX;
        }
        let aligned = element_bits >> bit_number;
        let complement = !aligned;
        counted_ones + count_trailing_zeros_u64(complement) as usize
    }

    fn count_trailing_ones(&self, last_idx: Idx) -> usize {
        debug_assert!(last_idx >= 0 && last_idx < self.num_bits);
        let mut array_idx = (last_idx >> LOG_BITS_PER_WORD) as usize;
        let mut bit_number = (last_idx & (BITS_PER_WORD as Idx - 1)) as usize;
        let mut element_bits = self.bitmap[array_idx];
        let mut mask = tail_mask(bit_number + 1);
        let mut counted_ones = 0usize;
        while (element_bits & mask) == mask {
            let found_ones = bit_number + 1;
            counted_ones += found_ones;
            array_idx = array_idx.wrapping_sub(1);
            element_bits = self.bitmap[array_idx];
            bit_number = BITS_PER_WORD - 1;
            mask = u64::MAX;
        }
        let aligned = element_bits << (BITS_PER_WORD - (bit_number + 1));
        let complement = !aligned;
        counted_ones + count_leading_zeros_u64(complement) as usize
    }

    fn is_forward_consecutive_ones(&self, start_idx: Idx, count: Idx) -> bool {
        let mut start_idx = start_idx;
        let mut count = count;
        while count > 0 {
            debug_assert!(start_idx >= 0 && start_idx < self.num_bits);
            debug_assert!(start_idx + count <= self.num_bits);
            let array_idx = (start_idx >> LOG_BITS_PER_WORD) as usize;
            let bit_number = (start_idx & (BITS_PER_WORD as Idx - 1)) as usize;
            let mut element_bits = self.bitmap[array_idx] >> bit_number;
            let bits_to_examine = BITS_PER_WORD - bit_number;
            let complement = !element_bits;
            let trailing_ones = if complement != 0 {
                count_trailing_zeros_u64(complement) as usize
            } else {
                bits_to_examine
            };
            if trailing_ones >= count as usize {
                return true;
            } else if trailing_ones == bits_to_examine {
                start_idx += bits_to_examine as Idx;
                count -= bits_to_examine as Idx;
            } else {
                return false;
            }
        }
        true
    }

    fn is_backward_consecutive_ones(&self, last_idx: Idx, count: Idx) -> bool {
        let mut last_idx = last_idx;
        let mut count = count;
        while count > 0 {
            debug_assert!(last_idx >= 0 && last_idx < self.num_bits);
            debug_assert!(last_idx - count >= -1);
            let array_idx = (last_idx >> LOG_BITS_PER_WORD) as usize;
            let bit_number = (last_idx & (BITS_PER_WORD as Idx - 1)) as usize;
            let mut element_bits = self.bitmap[array_idx];
            let bits_to_examine = bit_number + 1;
            element_bits <<= BITS_PER_WORD - bits_to_examine;
            let complement = !element_bits;
            let leading_ones = if complement != 0 {
                count_leading_zeros_u64(complement) as usize
            } else {
                bits_to_examine
            };
            if leading_ones >= count as usize {
                return true;
            } else if leading_ones == bits_to_examine {
                last_idx -= leading_ones as Idx;
                count -= leading_ones as Idx;
            } else {
                return false;
            }
        }
        true
    }

    #[inline]
    pub fn find_first_set_bit(&self, beg: Idx) -> Idx {
        debug_assert!(beg >= 0 && beg < self.num_bits);
        self.find_first_set_bit_range(beg, self.num_bits)
    }

    #[inline]
    pub fn find_first_set_bit_range(&self, beg: Idx, end: Idx) -> Idx {
        debug_assert!(beg >= 0 && beg < self.num_bits);
        debug_assert!(end > beg && end <= self.num_bits);
        let mut beg = beg;
        loop {
            let array_idx = (beg >> LOG_BITS_PER_WORD) as usize;
            let bit_number = (beg & (BITS_PER_WORD as Idx - 1)) as usize;
            let mut element_bits = self.bitmap[array_idx];
            if bit_number > 0 {
                let mask_out = tail_mask(bit_number);
                element_bits &= !mask_out;
            }
            if element_bits != 0 {
                let aligned = element_bits >> bit_number;
                let first_set_bit = count_trailing_zeros_u64(aligned) as Idx;
                let candidate = (array_idx as Idx) * BITS_PER_WORD as Idx + bit_number as Idx + first_set_bit;
                return if candidate < end { candidate } else { end };
            } else {
                beg += BITS_PER_WORD as Idx - bit_number as Idx;
                if beg >= end {
                    return end;
                }
            }
        }
    }

    #[inline]
    pub fn find_last_set_bit(&self, end: Idx) -> Idx {
        debug_assert!(end >= 0 && end < self.num_bits);
        self.find_last_set_bit_range(-1, end)
    }

    #[inline]
    pub fn find_last_set_bit_range(&self, beg: Idx, end: Idx) -> Idx {
        debug_assert!(end >= 0 && end < self.num_bits);
        debug_assert!(beg >= -1 && beg < end);
        let mut end = end;
        loop {
            let array_idx = (end >> LOG_BITS_PER_WORD) as usize;
            let bit_number = (end & (BITS_PER_WORD as Idx - 1)) as usize;
            let mut element_bits = self.bitmap[array_idx];
            if bit_number < BITS_PER_WORD - 1 {
                let mask_in = tail_mask(bit_number + 1);
                element_bits &= mask_in;
            }
            if element_bits != 0 {
                let aligned = element_bits << (BITS_PER_WORD - (bit_number + 1));
                let first_set_bit = count_leading_zeros_u64(aligned) as Idx;
                let candidate = (array_idx as Idx) * BITS_PER_WORD as Idx
                    + bit_number as Idx
                    - first_set_bit;
                return if candidate > beg { candidate } else { beg };
            } else {
                end -= (bit_number + 1) as Idx;
                if end <= beg {
                    return beg;
                }
            }
        }
    }

    #[inline]
    pub fn find_first_consecutive_set_bits_n(&self, beg: Idx, num_bits: Idx) -> Idx {
        self.find_first_consecutive_set_bits_range(beg, self.num_bits, num_bits)
    }

    pub fn find_first_consecutive_set_bits_range(
        &self,
        beg: Idx,
        end: Idx,
        num_bits: Idx,
    ) -> Idx {
        debug_assert!(beg >= 0 && beg < self.num_bits);
        let start_boundary = end - num_bits;
        if beg > start_boundary {
            return end;
        }
        let mut beg = beg;
        let mut array_idx = (beg >> LOG_BITS_PER_WORD) as usize;
        let mut bit_number = (beg & (BITS_PER_WORD as Idx - 1)) as usize;
        let mut element_bits = self.bitmap[array_idx];
        if bit_number > 0 {
            let mask_out = tail_mask(bit_number);
            element_bits &= !mask_out;
        }
        loop {
            if element_bits == 0 {
                beg += BITS_PER_WORD as Idx - bit_number as Idx;
                if beg > start_boundary {
                    return end;
                }
                array_idx += 1;
                bit_number = 0;
                element_bits = self.bitmap[array_idx];
            } else if self.is_forward_consecutive_ones(beg, num_bits) {
                return beg;
            } else {
                let next_set_bit = count_trailing_zeros_u64(element_bits) as Idx;
                let next_start_candidate_1 =
                    (array_idx as Idx) * BITS_PER_WORD as Idx + next_set_bit;
                let trailing_ones = self.count_trailing_ones(beg + num_bits - 1);
                let next_start_candidate_2 = beg + num_bits - trailing_ones as Idx;
                beg = next_start_candidate_1.max(next_start_candidate_2);
                if beg > start_boundary {
                    return end;
                }
                array_idx = (beg >> LOG_BITS_PER_WORD) as usize;
                element_bits = self.bitmap[array_idx];
                bit_number = (beg & (BITS_PER_WORD as Idx - 1)) as usize;
                if bit_number > 0 {
                    let mask_out = tail_mask(bit_number);
                    element_bits &= !mask_out;
                }
            }
        }
    }

    #[inline]
    pub fn find_last_consecutive_set_bits_n(&self, end: Idx, num_bits: Idx) -> Idx {
        self.find_last_consecutive_set_bits_range(-1, end, num_bits)
    }

    pub fn find_last_consecutive_set_bits_range(
        &self,
        beg: Idx,
        end: Idx,
        num_bits: Idx,
    ) -> Idx {
        debug_assert!(end >= 0 && end < self.num_bits);
        let last_boundary = beg + num_bits;
        if end < last_boundary {
            return beg;
        }
        let mut end = end;
        let mut array_idx = (end >> LOG_BITS_PER_WORD) as usize;
        let mut bit_number = (end & (BITS_PER_WORD as Idx - 1)) as usize;
        let mut element_bits = self.bitmap[array_idx];
        if bit_number < BITS_PER_WORD - 1 {
            let mask_in = tail_mask(bit_number + 1);
            element_bits &= mask_in;
        }
        loop {
            if element_bits == 0 {
                end -= bit_number as Idx + 1;
                if end < last_boundary {
                    return beg;
                }
                array_idx = array_idx.wrapping_sub(1);
                bit_number = BITS_PER_WORD - 1;
                element_bits = self.bitmap[array_idx];
            } else if self.is_backward_consecutive_ones(end, num_bits) {
                return end + 1 - num_bits;
            } else {
                let next_set_bit =
                    BITS_PER_WORD as Idx - (1 + count_leading_zeros_u64(element_bits) as Idx);
                let next_last_candidate_1 = (array_idx as Idx) * BITS_PER_WORD as Idx + next_set_bit;
                let leading_ones = self.count_leading_ones(end - (num_bits - 1)) as Idx;
                let next_last_candidate_2 = end - (num_bits - leading_ones);
                end = next_last_candidate_1.min(next_last_candidate_2);
                if end < last_boundary {
                    return beg;
                }
                array_idx = (end >> LOG_BITS_PER_WORD) as usize;
                bit_number = (end & (BITS_PER_WORD as Idx - 1)) as usize;
                element_bits = self.bitmap[array_idx];
                if bit_number < BITS_PER_WORD - 1 {
                    let mask_in = tail_mask(bit_number + 1);
                    element_bits &= mask_in;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_find_bits() {
        let mut bm = ShenandoahSimpleBitMap::new(200);
        bm.set_bit(3);
        bm.set_bit(70);
        bm.set_bit(130);
        assert!(bm.is_set(3));
        assert!(!bm.is_set(4));
        assert_eq!(bm.find_first_set_bit(0), 3);
        assert_eq!(bm.find_first_set_bit(4), 70);
        assert_eq!(bm.find_last_set_bit(200), 130);
        assert_eq!(bm.find_last_set_bit_range(-1, 130), 70);
    }

    #[test]
    fn consecutive_bits_search() {
        let mut bm = ShenandoahSimpleBitMap::new(300);
        for i in 10..18 {
            bm.set_bit(i);
        }
        for i in 100..110 {
            bm.set_bit(i);
        }
        assert_eq!(bm.find_first_consecutive_set_bits_n(0, 8), 10);
        assert_eq!(bm.find_first_consecutive_set_bits_n(20, 8), 100);
        assert_eq!(bm.find_first_consecutive_set_bits_n(20, 11), 300);
        assert_eq!(bm.find_last_consecutive_set_bits_n(300, 10), 100);
        assert_eq!(bm.find_last_consecutive_set_bits_n(95, 8), -1);
    }
}
