use std::sync::atomic::{AtomicU64, Ordering};

pub type BitMapIndex = usize;

#[derive(Clone)]
pub struct ZBitMap {
    words: Vec<AtomicU64>,
    size_in_bits: usize,
}

impl ZBitMap {
    pub fn new(size_in_bits: usize) -> Self {
        Self {
            words: (0..size_in_bits.div_ceil(64)).map(|_| AtomicU64::new(0)).collect(),
            size_in_bits,
        }
    }

    pub fn with_cleared(size_in_bits: usize) -> Self {
        Self::new(size_in_bits)
    }

    pub fn size(&self) -> usize {
        self.size_in_bits
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|w| w.load(Ordering::Acquire) == 0)
    }

    fn verify_index(&self, index: usize) {
        assert!(index < self.size_in_bits, "bitmap index out of bounds");
    }

    #[inline]
    fn word_and_mask(index: usize) -> (usize, u64) {
        (index / 64, 1u64 << (index % 64))
    }

    pub fn at(&self, index: usize) -> bool {
        self.verify_index(index);
        let (word, mask) = Self::word_and_mask(index);
        (self.words[word].load(Ordering::Acquire) & mask) != 0
    }

    pub fn set_bit(&self, index: usize) -> bool {
        self.verify_index(index);
        let (word, mask) = Self::word_and_mask(index);
        let old = self.words[word].fetch_or(mask, Ordering::AcqRel);
        (old & mask) == 0
    }

    pub fn clear_bit(&self, index: usize) {
        self.verify_index(index);
        let (word, mask) = Self::word_and_mask(index);
        self.words[word].fetch_and(!mask, Ordering::AcqRel);
    }

    pub fn clear(&self) {
        for word in &self.words {
            word.store(0, Ordering::Release);
        }
    }

    pub fn clear_range(&self, start: usize, end: usize) {
        assert!(start <= end && end <= self.size_in_bits);
        for index in start..end {
            self.clear_bit(index);
        }
    }

    pub fn par_set_bit(&self, index: usize) -> bool {
        self.set_bit(index)
    }

    fn bit_mask_pair(index: usize) -> u64 {
        assert!(index % 64 <= 62, "invalid bit index");
        0b11u64 << (index % 64)
    }

    pub fn par_set_bit_pair(&self, index: usize, finalizable: bool) -> (bool, bool) {
        self.verify_index(index);
        if finalizable {
            let inc_live = self.set_bit(index);
            return (inc_live, inc_live);
        } else {
            let (word_index, _) = Self::word_and_mask(index);
            let pair_mask = Self::bit_mask_pair(index);
            let word = &self.words[word_index];
            loop {
                let old = word.load(Ordering::Acquire);
                let new = old | pair_mask;
                if new == old {
                    return (false, false);
                }
                match word.compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => {
                        let marked_mask = 1u64 << (index % 64);
                        return (true, (old & marked_mask) == 0);
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    pub fn iterate<F>(&self, mut function: F)
    where
        F: FnMut(BitMapIndex) -> bool,
    {
        for word_index in 0..self.words.len() {
            let mut word = self.words[word_index].load(Ordering::Acquire);
            while word != 0 {
                let bit = word.trailing_zeros() as usize;
                let index = word_index * 64 + bit;
                if index >= self.size_in_bits || !function(index) {
                    return;
                }
                word &= word - 1;
            }
        }
    }

    pub fn iterate_range<F>(&self, start: usize, end: usize, mut function: F)
    where
        F: FnMut(BitMapIndex) -> bool,
    {
        let end = end.min(self.size_in_bits);
        for index in start..end {
            if self.at(index) && !function(index) {
                return;
            }
        }
    }

    pub fn find_first_set_bit(&self, start: usize, end: usize) -> Option<usize> {
        let end = end.min(self.size_in_bits);
        for index in start..end {
            if self.at(index) {
                return Some(index);
            }
        }
        None
    }

    pub fn find_last_set_bit(&self, start: usize, end: usize) -> Option<usize> {
        assert!(start < end && end <= self.size_in_bits);
        for index in (start..end).rev() {
            if self.at(index) {
                return Some(index);
            }
        }
        None
    }

    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.load(Ordering::Acquire).count_ones() as usize).sum()
    }
}

impl Default for ZBitMap {
    fn default() -> Self {
        Self::new(0)
    }
}

#[derive(Clone)]
pub struct ZReverseBitMapIterator<'a> {
    bitmap: &'a ZBitMap,
    beg: usize,
    end: usize,
}

impl<'a> ZReverseBitMapIterator<'a> {
    pub fn new(bitmap: &'a ZBitMap) -> Self {
        Self { bitmap, beg: 0, end: bitmap.size() }
    }

    pub fn with_range(bitmap: &'a ZBitMap, beg: usize, end: usize) -> Self {
        assert!(beg < bitmap.size() && beg <= end && end <= bitmap.size());
        Self { bitmap, beg, end }
    }

    pub fn next(&mut self) -> Option<usize> {
        let index = self.bitmap.find_last_set_bit(self.beg, self.end)?;
        self.end = index;
        Some(index)
    }
}

pub type ZMovableBitMap = ZBitMap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_pair_set() {
        let bitmap = ZBitMap::new(128);
        let (changed, inc_live) = bitmap.par_set_bit_pair(8, false);
        assert!(changed);
        assert!(inc_live);
        assert!(bitmap.at(8));
        assert!(bitmap.at(9));
        let (changed, inc_live) = bitmap.par_set_bit_pair(8, false);
        assert!(!changed);
        assert!(!inc_live);
    }
}
