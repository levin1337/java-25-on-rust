/*
 * Port of OpenJDK 25 HotSpot G1ConcurrentMarkBitMap
 * (share/gc/g1/g1ConcurrentMarkBitMap.hpp) and the per-region marking
 * view used by G1CMMarkStack / region iteration.
 *
 * One bit per HeapWord-sized granularity (we use one bit per `word`
 * bytes; default 8). The bitmap covers the whole heap contiguously;
 * per-region slices are addressed by region index.
 */

/// `G1ConcurrentMarkBitMap`: heap-wide mark bitmap.
#[derive(Debug)]
pub struct G1ConcurrentMarkBitMap {
    /// Heap base the bitmap starts at.
    base: usize,
    /// Region size in bytes.
    region_size: usize,
    /// Bytes per mark bit (HeapWord size; 8 default).
    word_size: usize,
    num_regions: usize,
    /// bits[region * words_per_region + word]
    bits: Vec<u64>,
    words_per_region: usize,
}

impl G1ConcurrentMarkBitMap {
    /// `G1ConcurrentMarkBitMap::G1ConcurrentMarkBitMap`.
    pub fn new(base: usize, region_size: usize, num_regions: usize, word_size: usize) -> Self {
        let bits_per_region = region_size / word_size.max(1);
        let words_per_region = (bits_per_region + 63) / 64;
        Self {
            base,
            region_size,
            word_size: word_size.max(1),
            num_regions,
            bits: vec![0u64; words_per_region * num_regions],
            words_per_region,
        }
    }

    #[inline]
    pub fn base(&self) -> usize {
        self.base
    }

    #[inline]
    pub fn region_size(&self) -> usize {
        self.region_size
    }

    #[inline]
    pub fn word_size(&self) -> usize {
        self.word_size
    }

    #[inline]
    pub fn num_regions(&self) -> usize {
        self.num_regions
    }

    #[inline]
    fn bit_index(&self, addr: usize) -> Option<(usize, usize)> {
        if addr < self.base {
            return None;
        }
        let offset = addr - self.base;
        let bit = offset / self.word_size;
        let region = bit / (self.region_size / self.word_size);
        if region >= self.num_regions {
            return None;
        }
        let bit_in_region = bit % (self.region_size / self.word_size);
        Some((region, bit_in_region))
    }

    /// `mark_bit` / `par_mark_bit`: set the mark bit for `addr`.
    /// Returns `true` if it was newly set.
    pub fn mark_bit(&mut self, addr: usize) -> bool {
        let Some((region, bit)) = self.bit_index(addr) else {
            return false;
        };
        let word_idx = region * self.words_per_region + bit / 64;
        let mask = 1u64 << (bit % 64);
        if self.bits[word_idx] & mask != 0 {
            return false;
        }
        self.bits[word_idx] |= mask;
        true
    }

    /// Atomic-style variant for parallel markers (single-threaded
    /// here; same semantics).
    pub fn par_mark_bit(&mut self, addr: usize) -> bool {
        self.mark_bit(addr)
    }

    /// `clear_bit`.
    pub fn clear_bit(&mut self, addr: usize) -> bool {
        let Some((region, bit)) = self.bit_index(addr) else {
            return false;
        };
        let word_idx = region * self.words_per_region + bit / 64;
        let mask = 1u64 << (bit % 64);
        if self.bits[word_idx] & mask == 0 {
            return false;
        }
        self.bits[word_idx] &= !mask;
        true
    }

    /// `is_marked`.
    pub fn is_marked(&self, addr: usize) -> bool {
        let Some((region, bit)) = self.bit_index(addr) else {
            return false;
        };
        let word_idx = region * self.words_per_region + bit / 64;
        (self.bits[word_idx] >> (bit % 64)) & 1 != 0
    }

    /// `mark_range` / `clear_range`: set/clear bits for an address
    /// range (aligned down/up to word granularity by caller).
    pub fn mark_range(&mut self, start: usize, end: usize) -> usize {
        let mut count = 0;
        let mut addr = start;
        while addr < end {
            if self.mark_bit(addr) {
                count += 1;
            }
            addr += self.word_size;
        }
        count
    }

    /// `clear_region`.
    pub fn clear_region(&mut self, region: usize) {
        let start = region * self.words_per_region;
        let end = (start + self.words_per_region).min(self.bits.len());
        self.bits[start..end].fill(0);
    }

    /// `clear_all`.
    pub fn clear_all(&mut self) {
        self.bits.fill(0);
    }

    /// Number of marked bits in a region.
    pub fn count_region(&self, region: usize) -> u64 {
        let start = region * self.words_per_region;
        let end = (start + self.words_per_region).min(self.bits.len());
        self.bits[start..end].iter().map(|w| w.count_ones() as u64).sum()
    }

    /// Total marked bits.
    pub fn count(&self) -> u64 {
        self.bits.iter().map(|w| w.count_ones() as u64).sum()
    }

    /// `is_region_full` style check: every bit in region set.
    pub fn is_region_full(&self, region: usize) -> bool {
        let bits_per_region = self.region_size / self.word_size;
        self.count_region(region) as usize >= bits_per_region
    }

    /// Iterate marked addresses in a region. `f` receives absolute
    /// addresses.
    pub fn for_each_marked_in_region<F: FnMut(usize)>(&self, region: usize, mut f: F) {
        let start_word = region * self.words_per_region;
        let bits_per_region = self.region_size / self.word_size;
        for (wi, &word) in self
            .bits
            .iter()
            .enumerate()
            .skip(start_word)
            .take(self.words_per_region)
        {
            let mut w = word;
            while w != 0 {
                let bit = w.trailing_zeros();
                let bit_in_region = (wi - start_word) * 64 + bit as usize;
                if bit_in_region >= bits_per_region {
                    break;
                }
                let addr = self.base + region * self.region_size + bit_in_region * self.word_size;
                f(addr);
                w &= w - 1;
            }
        }
    }

    /// First marked address at or after `addr` within its region
    /// (`getNextMarkedWordAddress` analogue). Returns `None` if none.
    pub fn next_marked(&self, addr: usize) -> Option<usize> {
        let (region, bit) = self.bit_index(addr)?;
        let start_word = region * self.words_per_region + bit / 64;
        let end_word = (region + 1) * self.words_per_region;
        let mut word_idx = start_word;
        let mut word = self.bits.get(word_idx).copied().unwrap_or(0)
            & (!0u64 << (bit % 64));
        while word_idx < end_word.min(self.bits.len()) {
            if word != 0 {
                let bit_in_region = (word_idx - region * self.words_per_region) * 64
                    + word.trailing_zeros() as usize;
                return Some(self.base + region * self.region_size + bit_in_region * self.word_size);
            }
            word_idx += 1;
            word = self.bits.get(word_idx).copied().unwrap_or(0);
        }
        None
    }
}

/// `MarkWord`-style two-bit marking state used by G1's concurrent
/// marker: distinguishes unmarked / marked / pending (grey) objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkState {
    Unmarked,
    Marked,
    Pending,
}

impl MarkState {
    #[inline]
    pub fn is_marked(self) -> bool {
        matches!(self, Self::Marked | Self::Pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_and_query() {
        let mut bm = G1ConcurrentMarkBitMap::new(0, 1024, 4, 8);
        assert!(bm.mark_bit(64));
        assert!(!bm.mark_bit(64)); // already marked
        assert!(bm.is_marked(64));
        assert!(!bm.is_marked(72));
        assert_eq!(bm.count(), 1);
    }

    #[test]
    fn region_isolation() {
        let mut bm = G1ConcurrentMarkBitMap::new(0, 1024, 4, 8);
        bm.mark_bit(0);
        bm.mark_bit(1024);
        bm.mark_bit(2048);
        bm.clear_region(1);
        assert!(bm.is_marked(0));
        assert!(!bm.is_marked(1024));
        assert!(bm.is_marked(2048));
    }

    #[test]
    fn iterate_marked() {
        let mut bm = G1ConcurrentMarkBitMap::new(0, 1024, 2, 8);
        bm.mark_bit(8);
        bm.mark_bit(24);
        bm.mark_bit(1032);
        let mut addrs = Vec::new();
        bm.for_each_marked_in_region(0, |a| addrs.push(a));
        assert_eq!(addrs, vec![8, 24]);
        let mut addrs1 = Vec::new();
        bm.for_each_marked_in_region(1, |a| addrs1.push(a));
        assert_eq!(addrs1, vec![1032]);
    }

    #[test]
    fn next_marked() {
        let mut bm = G1ConcurrentMarkBitMap::new(0, 1024, 1, 8);
        bm.mark_bit(104);
        bm.mark_bit(200);
        assert_eq!(bm.next_marked(0), Some(104));
        // word granularity: addr 105 is inside marked word 104..112
        assert_eq!(bm.next_marked(105), Some(104));
        assert_eq!(bm.next_marked(113), Some(200));
        assert_eq!(bm.next_marked(208), None);
    }
}
