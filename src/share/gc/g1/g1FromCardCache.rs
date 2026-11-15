/*
 * Port of OpenJDK 25 HotSpot G1FromCardCache
 * (share/gc/g1/g1FromCardCache.hpp).
 *
 * The from-card cache remembers, per region, which "from" cards have
 * already contributed a remembered-set entry. The post-write barrier
 * checks the cache before inserting into the refinement buffers so
 * repeated writes to the same card are cheap no-ops.
 *
 * HotSpot keeps two bitmaps per region (one per GC thread set) inside
 * a `G1BiasedMappedArray`; this port uses one `Vec<u64>` bitmap per
 * region.
 */

/// `G1FromCardCache`: per-region bitmap of from-cards already seen.
#[derive(Debug)]
pub struct G1FromCardCache {
    /// cards per region (e.g. 2048 for 1 MiB / 512 B cards)
    cards_per_region: u32,
    /// region index -> bitmap of `cards_per_region` bits.
    bitmaps: Vec<Vec<u64>>,
    /// number of bits set per region (for quick emptiness checks).
    counts: Vec<u32>,
}

impl G1FromCardCache {
    pub fn new(num_regions: usize, cards_per_region: u32) -> Self {
        let words = ((cards_per_region as usize) + 63) / 64;
        Self {
            cards_per_region,
            bitmaps: vec![vec![0u64; words.max(1)]; num_regions],
            counts: vec![0; num_regions],
        }
    }

    #[inline]
    fn bit_index(&self, card_in_region: u32) -> (usize, u32) {
        (card_in_region as usize / 64, card_in_region % 64)
    }

    /// `add`: marks `card_in_region` as seen for `region`. Returns
    /// `true` if the card was newly added (i.e. the caller should
    /// record a remembered-set entry).
    pub fn add(&mut self, region: usize, card_in_region: u32) -> bool {
        let Some(bitmap) = self.bitmaps.get_mut(region) else {
            return false;
        };
        let (word, bit) = self.bit_index(card_in_region);
        if word >= bitmap.len() {
            return false;
        }
        let mask = 1u64 << bit;
        if bitmap[word] & mask != 0 {
            return false;
        }
        bitmap[word] |= mask;
        self.counts[region] = self.counts[region].saturating_add(1);
        true
    }

    /// `contains`.
    pub fn contains(&self, region: usize, card_in_region: u32) -> bool {
        let Some(bitmap) = self.bitmaps.get(region) else {
            return false;
        };
        let (word, bit) = self.bit_index(card_in_region);
        word < bitmap.len() && (bitmap[word] >> bit) & 1 != 0
    }

    /// `clear_region`.
    pub fn clear_region(&mut self, region: usize) {
        if let Some(bitmap) = self.bitmaps.get_mut(region) {
            bitmap.fill(0);
        }
        if let Some(c) = self.counts.get_mut(region) {
            *c = 0;
        }
    }

    /// `clear` all regions (called when the whole cache is invalidated,
    /// e.g. after a full GC).
    pub fn clear(&mut self) {
        for bitmap in &mut self.bitmaps {
            bitmap.fill(0);
        }
        self.counts.fill(0);
    }

    /// `invalidate_from_card_cache(start_idx, num_regions)` static in
    /// HotSpot: clears a range of regions.
    pub fn invalidate_range(&mut self, start: usize, num_regions: usize) {
        let end = (start + num_regions).min(self.bitmaps.len());
        for region in start..end {
            self.clear_region(region);
        }
    }

    /// Number of cached cards for a region.
    pub fn count(&self, region: usize) -> u32 {
        self.counts.get(region).copied().unwrap_or(0)
    }

    /// `is_empty` for a region.
    pub fn is_empty(&self, region: usize) -> bool {
        self.count(region) == 0
    }

    /// Iterate cached card indexes within a region.
    pub fn for_each_card<F: FnMut(u32)>(&self, region: usize, mut f: F) {
        let Some(bitmap) = self.bitmaps.get(region) else {
            return;
        };
        for (wi, &word) in bitmap.iter().enumerate() {
            let mut w = word;
            while w != 0 {
                let bit = w.trailing_zeros();
                f((wi * 64 + bit as usize) as u32);
                w &= w - 1;
            }
        }
    }

    #[inline]
    pub fn cards_per_region(&self) -> u32 {
        self.cards_per_region
    }
}
