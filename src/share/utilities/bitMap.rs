use crate::share::utilities::globalDefinitions::{BITS_PER_WORD, BYTES_PER_WORD};

pub type idx_t = usize;
pub type bm_word_t = u64;

pub const LOG_BITS_PER_WORD: usize = 6;

#[derive(Clone, Debug)]
pub struct BitMap {
    map: Vec<bm_word_t>,
    size: idx_t,
}

impl BitMap {
    pub const MAX_SIZE_IN_BITS: idx_t = usize::MAX & !(BITS_PER_WORD - 1);

    pub fn calc_size_in_words(size_in_bits: usize) -> usize {
        size_in_bits.div_ceil(BITS_PER_WORD)
    }

    pub fn new(size_in_bits: usize) -> Self {
        let words = Self::calc_size_in_words(size_in_bits);
        Self {
            map: vec![0; words],
            size: size_in_bits,
        }
    }

    pub fn with_map(map: Vec<bm_word_t>, size_in_bits: usize) -> Self {
        Self { map, size: size_in_bits }
    }

    pub fn size(&self) -> idx_t {
        self.size
    }

    pub fn size_in_words(&self) -> usize {
        Self::calc_size_in_words(self.size)
    }

    pub fn size_in_bytes(&self) -> usize {
        self.size_in_words() * BYTES_PER_WORD
    }

    pub fn map(&self) -> &[bm_word_t] {
        &self.map
    }

    pub fn map_mut(&mut self) -> &mut [bm_word_t] {
        &mut self.map
    }

    fn verify_index(&self, bit: idx_t) {
        debug_assert!(bit < self.size, "bit index out of bounds");
    }

    fn verify_limit(&self, bit: idx_t) {
        debug_assert!(bit <= self.size, "bit limit out of bounds");
    }

    fn verify_range(&self, beg: idx_t, end: idx_t) {
        debug_assert!(beg <= end, "invalid range");
        debug_assert!(end <= self.size, "range end out of bounds");
    }

    fn word_index(bit: idx_t) -> usize {
        bit / BITS_PER_WORD
    }

    fn bit_in_word(bit: idx_t) -> u32 {
        (bit % BITS_PER_WORD) as u32
    }

    fn bit_mask(bit: idx_t) -> bm_word_t {
        1u64 << Self::bit_in_word(bit)
    }

    pub fn at(&self, index: idx_t) -> bool {
        self.verify_index(index);
        let w = self.map[Self::word_index(index)];
        (w & Self::bit_mask(index)) != 0
    }

    pub fn get(&self, index: idx_t) -> bool {
        self.at(index)
    }

    pub fn set_bit(&mut self, bit: idx_t) {
        self.verify_index(bit);
        let w = Self::word_index(bit);
        self.map[w] |= Self::bit_mask(bit);
    }

    pub fn clear_bit(&mut self, bit: idx_t) {
        self.verify_index(bit);
        let w = Self::word_index(bit);
        self.map[w] &= !Self::bit_mask(bit);
    }

    pub fn at_put(&mut self, bit: idx_t, value: bool) {
        if value {
            self.set_bit(bit);
        } else {
            self.clear_bit(bit);
        }
    }

    pub fn set(&mut self, bit: idx_t) {
        self.set_bit(bit);
    }

    pub fn clear(&mut self, bit: idx_t) {
        self.clear_bit(bit);
    }

    fn range_mask(beg_bit: idx_t, end_bit: idx_t) -> bm_word_t {
        let lo = Self::bit_in_word(beg_bit);
        let hi = Self::bit_in_word(end_bit);
        if lo == 0 && hi == 0 {
            !0u64
        } else {
            let hi_mask = if hi == 0 { !0u64 } else { (1u64 << hi) - 1 };
            let lo_mask = !0u64 << lo;
            lo_mask & hi_mask
        }
    }

    pub fn set_range(&mut self, beg: idx_t, end: idx_t) {
        self.verify_range(beg, end);
        if beg == end {
            return;
        }
        let beg_word = Self::word_index(beg);
        let end_word = Self::word_index(end - 1);
        if beg_word == end_word {
            self.map[beg_word] |= Self::range_mask(beg, end);
            return;
        }
        self.map[beg_word] |= Self::range_mask(beg, beg_word * BITS_PER_WORD + BITS_PER_WORD);
        for w in (beg_word + 1)..end_word {
            self.map[w] = !0u64;
        }
        self.map[end_word] |= Self::range_mask(end_word * BITS_PER_WORD, end);
    }

    pub fn clear_range(&mut self, beg: idx_t, end: idx_t) {
        self.verify_range(beg, end);
        if beg == end {
            return;
        }
        let beg_word = Self::word_index(beg);
        let end_word = Self::word_index(end - 1);
        if beg_word == end_word {
            self.map[beg_word] &= !Self::range_mask(beg, end);
            return;
        }
        self.map[beg_word] &= !Self::range_mask(beg, beg_word * BITS_PER_WORD + BITS_PER_WORD);
        for w in (beg_word + 1)..end_word {
            self.map[w] = 0;
        }
        self.map[end_word] &= !Self::range_mask(end_word * BITS_PER_WORD, end);
    }

    pub fn set_large_range(&mut self, beg: idx_t, end: idx_t) {
        self.set_range(beg, end);
    }

    pub fn clear_large_range(&mut self, beg: idx_t, end: idx_t) {
        self.clear_range(beg, end);
    }

    pub fn at_put_range(&mut self, beg: idx_t, end: idx_t, value: bool) {
        if value {
            self.set_range(beg, end);
        } else {
            self.clear_range(beg, end);
        }
    }

    pub fn clear_large(&mut self) {
        self.map.fill(0);
    }

    pub fn clear_all(&mut self) {
        self.clear_large();
    }

    pub fn count_one_bits(&self) -> idx_t {
        self.map.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn count_one_bits_range(&self, beg: idx_t, end: idx_t) -> idx_t {
        self.verify_range(beg, end);
        if beg == end {
            return 0;
        }
        let beg_word = Self::word_index(beg);
        let end_word = Self::word_index(end - 1);
        let mut count = 0usize;
        if beg_word == end_word {
            let m = Self::range_mask(beg, end);
            count += (self.map[beg_word] & m).count_ones() as usize;
            return count;
        }
        count += (self.map[beg_word] & Self::range_mask(beg, beg_word * BITS_PER_WORD + BITS_PER_WORD)).count_ones() as usize;
        for w in (beg_word + 1)..end_word {
            count += self.map[w].count_ones() as usize;
        }
        count += (self.map[end_word] & Self::range_mask(end_word * BITS_PER_WORD, end)).count_ones() as usize;
        count
    }

    fn find_first_bit(&self, beg: idx_t, end: idx_t, want_one: bool) -> Option<idx_t> {
        self.verify_range(beg, end);
        if beg >= end {
            return None;
        }
        let mut bit = beg;
        while bit < end {
            let w = Self::word_index(bit);
            let word_end_bit = (w + 1) * BITS_PER_WORD;
            let limit = word_end_bit.min(end);
            let mut word = self.map[w];
            let in_word_beg = Self::bit_in_word(bit);
            let in_word_end = Self::bit_in_word(limit);
            if in_word_beg > 0 {
                word &= !0u64 << in_word_beg;
            }
            if in_word_end > 0 || limit == end {
                if in_word_end > 0 {
                    word &= (1u64 << in_word_end) - 1;
                }
            }
            let found = if want_one {
                if word != 0 {
                    Some(word.trailing_zeros() as usize)
                } else {
                    None
                }
            } else {
                let inv = !word;
                if inv != 0 {
                    Some(inv.trailing_zeros() as usize)
                } else {
                    None
                }
            };
            if let Some(off) = found {
                let idx = w * BITS_PER_WORD + off;
                if idx >= beg && idx < end {
                    return Some(idx);
                }
            }
            bit = word_end_bit;
        }
        None
    }

    fn find_last_bit(&self, beg: idx_t, end: idx_t, want_one: bool) -> Option<idx_t> {
        self.verify_range(beg, end);
        if beg >= end {
            return None;
        }
        let end_word = Self::word_index(end - 1);
        let beg_word = Self::word_index(beg);
        let mut w = end_word;
        loop {
            let word_beg_bit = w * BITS_PER_WORD;
            let word_end_bit = word_beg_bit + BITS_PER_WORD;
            let limit_beg = word_beg_bit.max(beg);
            let limit_end = word_end_bit.min(end);
            if limit_beg < limit_end {
                let mut word = self.map[w];
                let lo = Self::bit_in_word(limit_beg);
                let hi = Self::bit_in_word(limit_end);
                if lo > 0 {
                    word &= !0u64 << lo;
                }
                if hi > 0 {
                    word &= (1u64 << hi) - 1;
                } else if limit_end < word_end_bit {
                    word &= (1u64 << BITS_PER_WORD) - 1;
                }
                let found = if want_one {
                    if word != 0 {
                        Some(63 - word.leading_zeros() as usize)
                    } else {
                        None
                    }
                } else {
                    let inv = !word;
                    if inv != 0 {
                        Some(63 - inv.leading_zeros() as usize)
                    } else {
                        None
                    }
                };
                if let Some(off) = found {
                    let idx = w * BITS_PER_WORD + off;
                    if idx >= beg && idx < end {
                        return Some(idx);
                    }
                }
            }
            if w == beg_word {
                break;
            }
            w -= 1;
        }
        None
    }

    pub fn find_first_set_bit(&self, beg: idx_t, end: idx_t) -> Option<idx_t> {
        self.find_first_bit(beg, end, true)
    }

    pub fn find_first_clear_bit(&self, beg: idx_t, end: idx_t) -> Option<idx_t> {
        self.find_first_bit(beg, end, false)
    }

    pub fn find_last_set_bit(&self, beg: idx_t, end: idx_t) -> Option<idx_t> {
        self.find_last_bit(beg, end, true)
    }

    pub fn find_last_clear_bit(&self, beg: idx_t, end: idx_t) -> Option<idx_t> {
        self.find_last_bit(beg, end, false)
    }

    pub fn first_set_bit(&self) -> Option<idx_t> {
        self.find_first_set_bit(0, self.size)
    }

    pub fn last_set_bit(&self) -> Option<idx_t> {
        self.find_last_set_bit(0, self.size)
    }

    pub fn iterate<F>(&self, mut function: F, beg: idx_t, end: idx_t) -> bool
    where
        F: FnMut(idx_t) -> bool,
    {
        self.verify_range(beg, end);
        let mut bit = beg;
        while bit < end {
            match self.find_first_set_bit(bit, end) {
                Some(idx) => {
                    if !function(idx) {
                        return false;
                    }
                    bit = idx + 1;
                }
                None => return true,
            }
        }
        true
    }

    pub fn iterate_all<F>(&self, function: F) -> bool
    where
        F: FnMut(idx_t) -> bool,
    {
        self.iterate(function, 0, self.size)
    }

    pub fn reverse_iterate<F>(&self, mut function: F, beg: idx_t, end: idx_t) -> bool
    where
        F: FnMut(idx_t) -> bool,
    {
        self.verify_range(beg, end);
        let mut bit = end;
        while bit > beg {
            match self.find_last_set_bit(beg, bit) {
                Some(idx) => {
                    if !function(idx) {
                        return false;
                    }
                    bit = idx;
                }
                None => return true,
            }
        }
        true
    }

    pub fn reverse_iterate_all<F>(&self, function: F) -> bool
    where
        F: FnMut(idx_t) -> bool,
    {
        self.reverse_iterate(function, 0, self.size)
    }

    pub fn set_union(&mut self, bits: &BitMap) {
        for (a, b) in self.map.iter_mut().zip(bits.map.iter()) {
            *a |= *b;
        }
    }

    pub fn set_difference(&mut self, bits: &BitMap) {
        for (a, b) in self.map.iter_mut().zip(bits.map.iter()) {
            *a &= !*b;
        }
    }

    pub fn set_intersection(&mut self, bits: &BitMap) {
        for (a, b) in self.map.iter_mut().zip(bits.map.iter()) {
            *a &= *b;
        }
    }

    pub fn contains(&self, bits: &BitMap) -> bool {
        for (a, b) in self.map.iter().zip(bits.map.iter()) {
            if a & b != *b {
                return false;
            }
        }
        true
    }

    pub fn intersects(&self, bits: &BitMap) -> bool {
        for (a, b) in self.map.iter().zip(bits.map.iter()) {
            if a & b != 0 {
                return true;
            }
        }
        false
    }

    pub fn set_from(&mut self, bits: &BitMap) {
        self.map.copy_from_slice(&bits.map);
    }

    pub fn is_same(&self, bits: &BitMap) -> bool {
        self.map == bits.map && self.size == bits.size
    }

    pub fn is_full(&self) -> bool {
        let full_words = self.size / BITS_PER_WORD;
        let rem = self.size % BITS_PER_WORD;
        for w in self.map.iter().take(full_words) {
            if *w != !0u64 {
                return false;
            }
        }
        if rem > 0 {
            let mask = (1u64 << rem) - 1;
            if self.map[full_words] & mask != mask {
                return false;
            }
            for w in self.map.iter().skip(full_words + 1) {
                if *w != 0 {
                    return false;
                }
            }
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        self.map.iter().all(|w| *w == 0)
    }

    pub fn iter(&self) -> BitMapIter<'_> {
        BitMapIter {
            map: self,
            end: self.size,
            next_bit: self.first_set_bit(),
        }
    }

    pub fn iter_range(&self, beg: idx_t, end: idx_t) -> BitMapIter<'_> {
        self.verify_range(beg, end);
        BitMapIter {
            map: self,
            end,
            next_bit: self.find_first_set_bit(beg, end),
        }
    }

    pub fn print_on(&self, f: &mut dyn FnMut(&str)) {
        f(&format!("BitMap size {}: ", self.size));
        let mut first = true;
        self.iterate_all(|bit| {
            if !first {
                f(", ");
            }
            f(&bit.to_string());
            first = false;
            true
        });
        f("\n");
    }

    pub fn resize(&mut self, new_size_in_bits: usize, clear: bool) {
        let new_words = Self::calc_size_in_words(new_size_in_bits);
        let old_words = self.map.len();
        if clear {
            self.map.clear();
            self.map.resize(new_words, 0);
        } else if new_words > old_words {
            self.map.resize(new_words, 0);
        } else {
            self.map.truncate(new_words);
        }
        self.size = new_size_in_bits;
    }

    pub fn reinitialize(&mut self, new_size_in_bits: usize, clear: bool) {
        self.resize(new_size_in_bits, clear);
    }

    pub fn truncate(&mut self, start_bit: idx_t, end_bit: idx_t) {
        self.verify_range(start_bit, end_bit);
        assert!(start_bit < end_bit, "empty truncate range");
        let words = Self::calc_size_in_words(end_bit - start_bit);
        let mut new_map = vec![0u64; words];
        let src_beg_word = Self::word_index(start_bit);
        let src_end_word = Self::word_index(end_bit - 1);
        let shift = Self::bit_in_word(start_bit);
        for i in 0..words {
            let src_word = src_beg_word + i;
            if src_word > src_end_word || src_word >= self.map.len() {
                break;
            }
            let mut v = self.map[src_word] >> shift;
            if shift > 0 && src_word + 1 <= src_end_word && src_word + 1 < self.map.len() {
                v |= self.map[src_word + 1] << (BITS_PER_WORD - shift);
            }
            new_map[i] = v;
        }
        let tail_bits = (end_bit - start_bit) % BITS_PER_WORD;
        if tail_bits > 0 && words > 0 {
            new_map[words - 1] &= (1u64 << tail_bits) - 1;
        }
        self.map = new_map;
        self.size = end_bit - start_bit;
    }
}

pub struct BitMapIter<'a> {
    map: &'a BitMap,
    end: idx_t,
    next_bit: Option<idx_t>,
}

impl<'a> Iterator for BitMapIter<'a> {
    type Item = idx_t;

    fn next(&mut self) -> Option<idx_t> {
        let bit = self.next_bit?;
        if bit + 1 >= self.end {
            self.next_bit = None;
        } else {
            self.next_bit = self.map.find_first_set_bit(bit + 1, self.end);
        }
        Some(bit)
    }
}

pub struct ReverseBitMapIter<'a> {
    map: &'a BitMap,
    prev_bit: Option<idx_t>,
}

impl<'a> ReverseBitMapIter<'a> {
    pub fn new(map: &'a BitMap, beg: idx_t, end: idx_t) -> Self {
        Self {
            map,
            prev_bit: map.find_last_set_bit(beg, end),
        }
    }
}

impl<'a> Iterator for ReverseBitMapIter<'a> {
    type Item = idx_t;

    fn next(&mut self) -> Option<idx_t> {
        let bit = self.prev_bit?;
        if bit == 0 {
            self.prev_bit = None;
        } else {
            self.prev_bit = self.map.find_last_set_bit(0, bit);
        }
        Some(bit)
    }
}

impl BitMap {
    pub fn reverse_iter(&self) -> ReverseBitMapIter<'_> {
        ReverseBitMapIter::new(self, 0, self.size)
    }

    pub fn reverse_iter_range(&self, beg: idx_t, end: idx_t) -> ReverseBitMapIter<'_> {
        ReverseBitMapIter::new(self, beg, end)
    }
}

#[derive(Clone, Debug)]
pub struct GrowableBitMap {
    inner: BitMap,
}

impl GrowableBitMap {
    pub fn new() -> Self {
        Self {
            inner: BitMap::with_map(Vec::new(), 0),
        }
    }

    pub fn with_size(size_in_bits: usize, clear: bool) -> Self {
        let mut g = Self::new();
        g.initialize(size_in_bits, clear);
        g
    }

    pub fn initialize(&mut self, size_in_bits: usize, clear: bool) {
        assert!(self.inner.map.is_empty() && self.inner.size == 0, "already initialized");
        self.inner.resize(size_in_bits, clear);
    }

    pub fn reinitialize(&mut self, new_size_in_bits: usize, clear: bool) {
        self.inner.resize(new_size_in_bits, clear);
    }

    pub fn resize(&mut self, new_size_in_bits: usize, clear: bool) {
        self.inner.resize(new_size_in_bits, clear);
    }

    pub fn truncate(&mut self, start_bit: idx_t, end_bit: idx_t) {
        self.inner.truncate(start_bit, end_bit);
    }

    pub fn as_bit_map(&self) -> &BitMap {
        &self.inner
    }

    pub fn bit_map(&self) -> &BitMap {
        &self.inner
    }

    pub fn bit_map_mut(&mut self) -> &mut BitMap {
        &mut self.inner
    }

    pub fn into_bit_map(self) -> BitMap {
        self.inner
    }

    pub fn size(&self) -> idx_t {
        self.inner.size()
    }
}

impl Default for GrowableBitMap {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BitMap2D {
    map: GrowableBitMap,
    bits_per_slot: idx_t,
}

impl BitMap2D {
    pub fn new(bits_per_slot: idx_t) -> Self {
        assert!(bits_per_slot > 0);
        Self {
            map: GrowableBitMap::new(),
            bits_per_slot,
        }
    }

    pub fn with_size(size_in_slots: idx_t, bits_per_slot: idx_t) -> Self {
        assert!(bits_per_slot > 0);
        Self {
            map: GrowableBitMap::with_size((size_in_slots * bits_per_slot) as usize, true),
            bits_per_slot,
        }
    }

    fn bit_index(&self, slot_index: idx_t, bit_within_slot_index: idx_t) -> idx_t {
        slot_index * self.bits_per_slot + bit_within_slot_index
    }

    fn verify_bit_within_slot_index(&self, index: idx_t) {
        debug_assert!(index < self.bits_per_slot);
    }

    pub fn size_in_bits(&self) -> idx_t {
        self.map.size()
    }

    pub fn is_valid_index(&self, slot_index: idx_t, bit_within_slot_index: idx_t) -> bool {
        bit_within_slot_index < self.bits_per_slot
            && slot_index * self.bits_per_slot + bit_within_slot_index < self.map.size()
    }

    pub fn at(&self, slot_index: idx_t, bit_within_slot_index: idx_t) -> bool {
        self.verify_bit_within_slot_index(bit_within_slot_index);
        self.map.bit_map().at(self.bit_index(slot_index, bit_within_slot_index))
    }

    pub fn set_bit(&mut self, slot_index: idx_t, bit_within_slot_index: idx_t) {
        self.verify_bit_within_slot_index(bit_within_slot_index);
        let idx = self.bit_index(slot_index, bit_within_slot_index);
        if self.map.size() == 0 {
            self.map.initialize(idx + 1, true);
        } else if idx >= self.map.size() {
            self.map.resize(idx + 1, true);
        }
        self.map.bit_map_mut().set_bit(idx);
    }

    pub fn clear_bit(&mut self, slot_index: idx_t, bit_within_slot_index: idx_t) {
        self.verify_bit_within_slot_index(bit_within_slot_index);
        let idx = self.bit_index(slot_index, bit_within_slot_index);
        if idx >= self.map.size() {
            return;
        }
        self.map.bit_map_mut().clear_bit(idx);
    }

    pub fn at_put(&mut self, slot_index: idx_t, bit_within_slot_index: idx_t, value: bool) {
        if value {
            self.set_bit(slot_index, bit_within_slot_index);
        } else {
            self.clear_bit(slot_index, bit_within_slot_index);
        }
    }

    pub fn at_put_grow(&mut self, slot_index: idx_t, bit_within_slot_index: idx_t, value: bool) {
        self.at_put(slot_index, bit_within_slot_index, value);
    }
}

pub trait BitMapClosure {
    fn do_bit(&mut self, index: idx_t) -> bool;
}

pub fn iterate_with_closure<F: BitMapClosure>(map: &BitMap, closure: &mut F) {
    map.iterate(|idx| closure.do_bit(idx), 0, map.size());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_set_get() {
        let mut bm = BitMap::new(200);
        assert!(!bm.at(0));
        bm.set_bit(0);
        bm.set_bit(63);
        bm.set_bit(64);
        bm.set_bit(199);
        assert!(bm.at(0));
        assert!(bm.at(63));
        assert!(bm.at(64));
        assert!(bm.at(199));
        assert!(!bm.at(65));
        bm.clear_bit(64);
        assert!(!bm.at(64));
        bm.at_put(100, true);
        assert!(bm.at(100));
    }

    #[test]
    fn ranges() {
        let mut bm = BitMap::new(300);
        bm.set_range(10, 70);
        assert!(bm.at(10));
        assert!(bm.at(69));
        assert!(!bm.at(9));
        assert!(!bm.at(70));
        assert!(bm.at(63));
        assert!(bm.at(64));
        bm.clear_range(20, 65);
        assert!(!bm.at(20));
        assert!(bm.at(19));
        assert!(bm.at(10));
        assert!(!bm.at(64));
        assert!(bm.at(66));
        bm.set_range(0, 300);
        assert!(bm.is_full());
        bm.clear_range(0, 300);
        assert!(bm.is_empty());
    }

    #[test]
    fn find_bits() {
        let mut bm = BitMap::new(200);
        bm.set_bit(5);
        bm.set_bit(70);
        bm.set_bit(130);
        bm.set_range(150, 160);
        assert_eq!(bm.find_first_set_bit(0, 200), Some(5));
        assert_eq!(bm.find_first_set_bit(6, 200), Some(70));
        assert_eq!(bm.find_last_set_bit(0, 200), Some(159));
        assert_eq!(bm.find_last_set_bit(0, 150), Some(130));
        assert_eq!(bm.find_first_clear_bit(0, 10), Some(0));
        assert_eq!(bm.find_last_clear_bit(0, 10), Some(9));
        assert_eq!(bm.find_first_set_bit(160, 200), None);
    }

    #[test]
    fn iterate_and_count() {
        let mut bm = BitMap::new(100);
        bm.set_range(10, 20);
        bm.set_bit(50);
        let bits: Vec<usize> = bm.iter().collect();
        assert_eq!(bits, (10..20).chain(std::iter::once(50)).collect::<Vec<_>>());
        assert_eq!(bm.count_one_bits(), 11);
        assert_eq!(bm.count_one_bits_range(0, 100), 11);
        assert_eq!(bm.count_one_bits_range(15, 55), 6);
        let rev: Vec<usize> = bm.reverse_iter().collect();
        assert_eq!(rev[0], 50);
        assert_eq!(rev[rev.len() - 1], 10);
    }

    #[test]
    fn set_ops() {
        let mut a = BitMap::new(64);
        let mut b = BitMap::new(64);
        a.set_range(0, 32);
        b.set_range(16, 48);
        let mut u = a.clone();
        u.set_union(&b);
        assert_eq!(u.count_one_bits(), 48);
        let mut i = a.clone();
        i.set_intersection(&b);
        assert_eq!(i.count_one_bits(), 16);
        let mut d = a.clone();
        d.set_difference(&b);
        assert_eq!(d.count_one_bits(), 16);
        assert!(a.intersects(&b));
        assert!(a.contains(&i));
        assert!(!a.is_same(&b));
        assert!(u.is_same(&u));
    }

    #[test]
    fn growable() {
        let mut g = GrowableBitMap::with_size(70, true);
        g.bit_map_mut().set_bit(69);
        assert!(g.bit_map().at(69));
        let mut g2 = GrowableBitMap::with_size(200, true);
        g2.resize(300, true);
        assert_eq!(g2.size(), 300);
        g2.truncate(10, 50);
        assert_eq!(g2.size(), 40);
        let bm = g2.into_bit_map();
        assert_eq!(bm.size(), 40);
    }

    #[test]
    fn bit_map_2d() {
        let mut m = BitMap2D::with_size(4, 8);
        m.set_bit(1, 3);
        assert!(m.at(1, 3));
        assert!(!m.at(1, 4));
        m.clear_bit(1, 3);
        assert!(!m.at(1, 3));
    }
}
