/*
 * Port of OpenJDK 25 HotSpot G1CardSet (share/gc/g1/g1CardSet.hpp)
 * and its containers (g1CardSetContainers.hpp).
 *
 * A G1CardSet is a sparse set of card indexes used by remembered
 * sets. Cards are stored per "card region" (a virtualization of the
 * heap region: by default one card region == one heap region). Each
 * card region's set is a container that coarsens as it fills:
 *
 *   Array of Cards  ->  Bitmap  ->  Howl  ->  Full
 *
 * Howl is itself an array of smaller containers ("buckets"), each
 * covering a slice of the card region's card indexes; buckets
 * coarsen to Full independently when too dense.
 *
 * The Rust port keeps the same configuration knobs and thresholds
 * (G1RemSetArrayOfCardsEntries, G1RemSetHowlNumBuckets,
 * G1RemSetCoarsenHowlBitmapToHowlFullPercent,
 * G1RemSetCoarsenHowlToFullPercent) and the same add/remove/iterate
 * semantics, minus the lock-free ContainerPtr encoding (single
 * writer per remembered set table scan, mutable access).
 */

use std::collections::HashMap;

/// `G1AddCardResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum G1AddCardResult {
    /// The card set is more than full; the caller should coarsen and
    /// retry (the entry may or may not have been added).
    Overflow,
    /// The card is already in the set.
    Found,
    /// The card was added by this attempt.
    Added,
}

/// `G1CardSetContainer::LogCardsPerRegionLimit`: maximum number of
/// distinct cards a single container can address. HotSpot derives it
/// from `uint16_t` array entries -> 16 bits.
pub const LOG_CARDS_PER_REGION_LIMIT: u32 = 16;

/// `G1CardSetConfiguration` — mirrors g1CardSet.hpp. All thresholds
/// and bucket counts are computed exactly like the C++ constructor.
#[derive(Debug, Clone)]
pub struct G1CardSetConfiguration {
    /// Bits in the inline pointer encoding (HotSpot uses this to
    /// pack small card sets directly into the table pointer).
    inline_ptr_bits_per_card: u32,
    /// `G1RemSetArrayOfCardsEntries`: max entries in Array container.
    max_cards_in_array: u32,
    /// `G1RemSetCoarsenHowlBitmapToHowlFullPercent` as a fraction.
    cards_in_bitmap_threshold_percent: f64,
    /// `G1RemSetHowlNumBuckets`: number of buckets in a Howl.
    num_buckets_in_howl: u32,
    /// `G1RemSetCoarsenHowlToFullPercent` as a fraction.
    cards_in_howl_threshold_percent: f64,
    /// Max cards in a card region (= `CardsPerRegion >>
    /// log2_card_regions_per_heap_region`).
    max_cards_in_card_set: u32,
    /// Log2 of card regions per heap region.
    log2_card_regions_per_heap_region: u32,
    /// Log2 of cards per card region.
    log2_max_cards_in_howl_bitmap: u32,
    bitmap_hash_mask: u32,
    log2_cards_per_card_region: u32,
    cards_in_howl_bitmap_threshold: u32,
    cards_in_howl_threshold: u32,
    max_cards_in_howl_bitmap: u32,
}

fn log2i_exact(mut v: u32) -> u32 {
    let mut r = 0;
    while v > 1 {
        v >>= 1;
        r += 1;
    }
    r
}

fn round_up_power_of_2(v: u32) -> u32 {
    if v <= 1 {
        return 1;
    }
    1u32 << (32 - (v - 1).leading_zeros())
}

impl G1CardSetConfiguration {
    /// `G1HeapRegion::LogCardsPerRegion` for a 1 MiB region with
    /// 512-byte cards = 2048 cards -> log2 = 11.
    pub const LOG_CARDS_PER_HEAP_REGION_DEFAULT: u32 = 11;

    /// Ergonomic defaults mirroring `G1CardSetConfiguration()`:
    /// `max_cards_in_array = 8 * cards_per_region_mb`,
    /// `num_buckets_in_howl` derived from the 8k-bitmap bound,
    /// `max_cards_in_card_set = cards per heap region`.
    pub fn new(cards_per_heap_region: u32, region_size_mb: u32) -> Self {
        let log2_cards_per_heap_region = log2i_exact(cards_per_heap_region);
        // default_log2_card_regions_per_region(): keep each card region
        // within the container addressability limit.
        let log2_card_regions_per_heap_region =
            log2_cards_per_heap_region.saturating_sub(LOG_CARDS_PER_REGION_LIMIT);
        let max_cards_in_card_set =
            cards_per_heap_region >> log2_card_regions_per_heap_region;

        let max_cards_in_array = (8 * region_size_mb).max(1);
        // HowlNumBuckets: power of two; each bucket bitmap covers
        // `roundup2(max_cards_in_card_set / num_buckets)` cards. Pick
        // the largest bucket count (<=1024) keeping >=64 cards/bucket,
        // mirroring the ergonomic G1RemSetHowlNumBuckets constraint.
        let num_buckets_in_howl = {
            let mut n = 1u32;
            while n < 1024 && max_cards_in_card_set / (n << 1) >= 64 {
                n <<= 1;
            }
            n.max(1)
        };
        let max_cards_in_howl_bitmap =
            round_up_power_of_2((max_cards_in_card_set / num_buckets_in_howl).max(1));
        let log2_max_cards_in_howl_bitmap = log2i_exact(max_cards_in_howl_bitmap);

        Self {
            inline_ptr_bits_per_card: log2_cards_per_heap_region
                .saturating_sub(log2_card_regions_per_heap_region),
            max_cards_in_array,
            cards_in_bitmap_threshold_percent: 0.90,
            num_buckets_in_howl,
            cards_in_howl_threshold_percent: 0.90,
            max_cards_in_card_set,
            log2_card_regions_per_heap_region,
            log2_max_cards_in_howl_bitmap,
            bitmap_hash_mask: (1u32 << log2_max_cards_in_howl_bitmap) - 1,
            log2_cards_per_card_region: log2i_exact(max_cards_in_card_set),
            cards_in_howl_bitmap_threshold: ((max_cards_in_howl_bitmap as f64)
                * 0.90) as u32,
            cards_in_howl_threshold: ((max_cards_in_card_set as f64) * 0.90) as u32,
            max_cards_in_howl_bitmap,
        }
    }

    /// Default configuration for the HotSpot 1 MiB region / 512 B card
    /// geometry (2048 cards per region).
    pub fn default_for_region(region_size: usize, card_size: usize) -> Self {
        let cards = (region_size / card_size.max(1)) as u32;
        let mb = (region_size / (1024 * 1024)).max(1) as u32;
        Self::new(cards.max(1), mb)
    }

    #[inline]
    pub fn inline_ptr_bits_per_card(&self) -> u32 {
        self.inline_ptr_bits_per_card
    }

    #[inline]
    pub fn max_cards_in_array(&self) -> u32 {
        self.max_cards_in_array
    }

    #[inline]
    pub fn cards_in_bitmap_threshold(&self) -> u32 {
        self.cards_in_howl_bitmap_threshold
    }

    #[inline]
    pub fn num_buckets_in_howl(&self) -> u32 {
        self.num_buckets_in_howl
    }

    #[inline]
    pub fn cards_in_howl_threshold(&self) -> u32 {
        self.cards_in_howl_threshold
    }

    #[inline]
    pub fn max_cards_in_card_set(&self) -> u32 {
        self.max_cards_in_card_set
    }

    #[inline]
    pub fn log2_card_regions_per_heap_region(&self) -> u32 {
        self.log2_card_regions_per_heap_region
    }

    #[inline]
    pub fn log2_cards_per_card_region(&self) -> u32 {
        self.log2_cards_per_card_region
    }

    /// `howl_bucket_index`: bucket within a Howl for a card index
    /// inside the card region.
    #[inline]
    pub fn howl_bucket_index(&self, card_in_region: u32) -> u32 {
        card_in_region >> self.log2_max_cards_in_howl_bitmap
    }

    /// `howl_bitmap_offset`: offset within the bucket bitmap.
    #[inline]
    pub fn howl_bitmap_offset(&self, card_in_region: u32) -> u32 {
        card_in_region & self.bitmap_hash_mask
    }

    /// Cards covered by one Howl bucket.
    #[inline]
    pub fn howl_cards_per_bucket(&self) -> u32 {
        1u32 << self.log2_max_cards_in_howl_bitmap
    }
}

/// A card set container. `card_count` bounds are implicit: each
/// container knows how many cards of its card region it can address
/// (`max_card` = cards per card region for top-level containers, or
/// the bucket slice size inside a Howl).
#[derive(Debug, Clone)]
pub enum G1CardSetContainer {
    /// G1CardSetArray: sorted list of u16 card indexes.
    Array { cards: Vec<u16> },
    /// G1CardSetBitMap: bit per card index.
    Bitmap { bits: Vec<u64>, num_set: u32, card_count: u32 },
    /// G1CardSetHowl: array of bucket containers; each bucket covers
    /// `per_bucket` consecutive card indexes.
    Howl { buckets: Vec<G1CardSetContainer>, card_count: u32, per_bucket: u32 },
    /// G1CardSetFull / inline full marker: every card in range is set.
    Full,
}

impl G1CardSetContainer {
    fn new_array() -> Self {
        Self::Array { cards: Vec::new() }
    }

    fn new_bitmap(card_count: u32) -> Self {
        let words = ((card_count as usize) + 63) / 64;
        Self::Bitmap { bits: vec![0; words], num_set: 0, card_count }
    }

    fn new_howl(config: &G1CardSetConfiguration, card_count: u32) -> Self {
        let buckets = config.num_buckets_in_howl() as usize;
        let per_bucket = config.howl_cards_per_bucket();
        let mut v = Vec::with_capacity(buckets);
        for _ in 0..buckets {
            v.push(Self::new_bitmap(per_bucket.min(card_count)));
        }
        Self::Howl { buckets: v, card_count, per_bucket }
    }

    /// Number of set cards (approximate for `Full`).
    pub fn num_entries(&self) -> usize {
        match self {
            Self::Array { cards } => cards.len(),
            Self::Bitmap { num_set, .. } => *num_set as usize,
            Self::Howl { buckets, .. } => buckets.iter().map(|b| b.num_entries()).sum(),
            Self::Full => usize::MAX / 2,
        }
    }

    fn add(&mut self, card: u32, config: &G1CardSetConfiguration) -> G1AddCardResult {
        match self {
            Self::Array { cards } => {
                if cards.contains(&(card as u16)) {
                    return G1AddCardResult::Found;
                }
                if cards.len() as u32 >= config.max_cards_in_array() {
                    return G1AddCardResult::Overflow;
                }
                let pos = cards.partition_point(|&c| (c as u32) < card);
                cards.insert(pos, card as u16);
                G1AddCardResult::Added
            }
            Self::Bitmap { bits, num_set, card_count } => {
                let idx = card as usize;
                if idx >= *card_count as usize {
                    return G1AddCardResult::Overflow;
                }
                let word = idx / 64;
                let bit = idx % 64;
                if bits[word] & (1u64 << bit) != 0 {
                    return G1AddCardResult::Found;
                }
                bits[word] |= 1u64 << bit;
                *num_set += 1;
                G1AddCardResult::Added
            }
            Self::Howl { buckets, .. } => {
                let bucket_idx = config.howl_bucket_index(card) as usize;
                let offset = config.howl_bitmap_offset(card);
                if bucket_idx >= buckets.len() {
                    // card beyond configured howl coverage -> coarsen all
                    return G1AddCardResult::Overflow;
                }
                match buckets[bucket_idx].add(offset, config) {
                    G1AddCardResult::Overflow => {
                        // Coarsen the bucket: replace with a Full bucket
                        // covering its slice.
                        buckets[bucket_idx] = Self::Full;
                        G1AddCardResult::Added
                    }
                    other => other,
                }
            }
            Self::Full => G1AddCardResult::Found,
        }
    }

    fn contains(&self, card: u32, config: &G1CardSetConfiguration) -> bool {
        match self {
            Self::Array { cards } => cards.contains(&(card as u16)),
            Self::Bitmap { bits, .. } => {
                let idx = card as usize;
                idx / 64 < bits.len() && (bits[idx / 64] >> (idx % 64)) & 1 != 0
            }
            Self::Howl { buckets, .. } => {
                let bucket_idx = config.howl_bucket_index(card) as usize;
                let offset = config.howl_bitmap_offset(card);
                bucket_idx < buckets.len() && buckets[bucket_idx].contains(offset, config)
            }
            Self::Full => true,
        }
    }

    fn remove(&mut self, card: u32, config: &G1CardSetConfiguration) -> bool {
        match self {
            Self::Array { cards } => {
                if let Some(pos) = cards.iter().position(|&c| c as u32 == card) {
                    cards.remove(pos);
                    true
                } else {
                    false
                }
            }
            Self::Bitmap { bits, num_set, .. } => {
                let idx = card as usize;
                if idx / 64 >= bits.len() {
                    return false;
                }
                let mask = 1u64 << (idx % 64);
                if bits[idx / 64] & mask != 0 {
                    bits[idx / 64] &= !mask;
                    *num_set = num_set.saturating_sub(1);
                    true
                } else {
                    false
                }
            }
            Self::Howl { buckets, .. } => {
                let bucket_idx = config.howl_bucket_index(card) as usize;
                let offset = config.howl_bitmap_offset(card);
                bucket_idx < buckets.len() && buckets[bucket_idx].remove(offset, config)
            }
            Self::Full => false,
        }
    }

    /// Iterate set cards, calling `f` with `base + card_in_container`.
    /// `limit` bounds the number of cards this container covers;
    /// cards at or beyond `limit` are skipped (Howl bucket tails).
    fn for_each(&self, base: u32, limit: u32, f: &mut dyn FnMut(u32)) {
        match self {
            Self::Array { cards } => {
                for &c in cards {
                    let v = base + c as u32;
                    if v - base < limit {
                        f(v);
                    }
                }
            }
            Self::Bitmap { bits, .. } => {
                for (wi, &word) in bits.iter().enumerate() {
                    let mut w = word;
                    while w != 0 {
                        let bit = w.trailing_zeros();
                        let card = (wi * 64 + bit as usize) as u32;
                        if card >= limit {
                            return;
                        }
                        f(base + card);
                        w &= w - 1;
                    }
                }
            }
            Self::Howl { buckets, card_count, per_bucket } => {
                let _ = card_count;
                for (bi, bucket) in buckets.iter().enumerate() {
                    let sub_base = base + (bi as u32) * per_bucket;
                    let covered = limit.saturating_sub((bi as u32) * per_bucket);
                    let sub_limit = (*per_bucket).min(covered);
                    bucket.for_each(sub_base, sub_limit, f);
                }
            }
            Self::Full => {
                for c in 0..limit {
                    f(base + c);
                }
            }
        }
    }

    /// Upgrade the container one level (Array -> Bitmap -> Howl ->
    /// Full) preserving existing entries.
    fn upgrade(&mut self, config: &G1CardSetConfiguration, card_count: u32) {
        let old = std::mem::replace(self, Self::Full);
        match old {
            Self::Array { cards } => {
                let mut bm = Self::new_bitmap(card_count);
                for c in cards {
                    let _ = bm.add(c as u32, config);
                }
                *self = bm;
            }
            Self::Bitmap { bits, num_set, .. } => {
                let mut howl = Self::new_howl(config, card_count);
                if let Self::Howl { buckets, .. } = &mut howl {
                    for (wi, &word) in bits.iter().enumerate() {
                        let mut w = word;
                        while w != 0 {
                            let bit = w.trailing_zeros();
                            let card = (wi * 64 + bit as usize) as u32;
                            let b = config.howl_bucket_index(card) as usize;
                            let off = config.howl_bitmap_offset(card);
                            if b < buckets.len() {
                                let _ = buckets[b].add(off, config);
                            }
                            w &= w - 1;
                        }
                    }
                }
                let _ = num_set;
                *self = howl;
            }
            Self::Howl { .. } | Self::Full => {
                *self = Self::Full;
            }
        }
    }
}

/// `G1CardSet`: sparse set of card indexes, keyed by card region.
///
/// `card_region(card) = card >> log2_cards_per_card_region`; the
/// absolute card index of a region's base is
/// `card_region << log2_cards_per_card_region`.
#[derive(Debug, Clone)]
pub struct G1CardSet {
    config: G1CardSetConfiguration,
    /// card_region -> container
    table: HashMap<u32, G1CardSetContainer>,
}

impl G1CardSet {
    pub fn new(config: G1CardSetConfiguration) -> Self {
        Self { config, table: HashMap::new() }
    }

    #[inline]
    pub fn config(&self) -> &G1CardSetConfiguration {
        &self.config
    }

    #[inline]
    fn card_region_of(&self, card_index: u32) -> u32 {
        card_index >> self.config.log2_cards_per_card_region()
    }

    #[inline]
    fn base_card_of_region(&self, card_region: u32) -> u32 {
        card_region << self.config.log2_cards_per_card_region()
    }

    #[inline]
    fn card_in_region(&self, card_index: u32) -> u32 {
        card_index & ((1u32 << self.config.log2_cards_per_card_region()) - 1)
    }

    /// `add_card`: add `card_index` to the set. Returns
    /// `G1AddCardResult`; `Overflow` means the entry may have been
    /// added but the set needs coarsening — we coarsen internally and
    /// retry, matching the container upgrade chain.
    pub fn add_card(&mut self, card_index: u32) -> G1AddCardResult {
        let region = self.card_region_of(card_index);
        let in_region = self.card_in_region(card_index);
        let config = self.config.clone();
        let card_count = config.max_cards_in_card_set();

        let entry = self.table.entry(region).or_insert_with(G1CardSetContainer::new_array);
        let mut result = entry.add(in_region, &config);
        while result == G1AddCardResult::Overflow {
            entry.upgrade(&config, card_count);
            result = entry.add(in_region, &config);
        }
        result
    }

    #[inline]
    pub fn contains_card(&self, card_index: u32) -> bool {
        let region = self.card_region_of(card_index);
        let in_region = self.card_in_region(card_index);
        self.table
            .get(&region)
            .map(|c| c.contains(in_region, &self.config))
            .unwrap_or(false)
    }

    #[inline]
    pub fn remove_card(&mut self, card_index: u32) -> bool {
        let region = self.card_region_of(card_index);
        let in_region = self.card_in_region(card_index);
        match self.table.get_mut(&region) {
            Some(c) => {
                let removed = c.remove(in_region, &self.config);
                if removed && c.num_entries() == 0 {
                    self.table.remove(&region);
                }
                removed
            }
            None => false,
        }
    }

    /// `iterate`: visit every card index in the set (absolute card
    /// indexes, not region-relative).
    pub fn for_each_card<F: FnMut(u32)>(&self, mut f: F) {
        for (&region, container) in &self.table {
            let base = self.base_card_of_region(region);
            let config = &self.config;
            container.for_each(0, config.max_cards_in_card_set(), &mut |c| {
                f(base + c);
            });
        }
    }

    /// Collect all card indexes (for tests / iteration).
    pub fn cards(&self) -> Vec<u32> {
        let mut out = Vec::new();
        self.for_each_card(|c| out.push(c));
        out.sort_unstable();
        out
    }

    /// `occupied`: number of card regions with non-empty sets.
    pub fn occupied(&self) -> usize {
        self.table.len()
    }

    /// Total number of cards stored (Full regions counted by their
    /// card count).
    pub fn num_cards(&self) -> usize {
        self.table.values().map(|c| c.num_entries()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn clear(&mut self) {
        self.table.clear();
    }
}

impl Default for G1CardSet {
    fn default() -> Self {
        Self::new(G1CardSetConfiguration::default_for_region(
            HeapRegionRef::DEFAULT_REGION_SIZE,
            512,
        ))
    }
}

// Local constant to avoid a dependency cycle on heapRegion.rs.
mod HeapRegionRef {
    pub const DEFAULT_REGION_SIZE: usize = 1024 * 1024;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> G1CardSetConfiguration {
        G1CardSetConfiguration::default_for_region(1024 * 1024, 512)
    }

    #[test]
    fn array_add_find() {
        let mut set = G1CardSet::new(config());
        assert_eq!(set.add_card(10), G1AddCardResult::Added);
        assert_eq!(set.add_card(10), G1AddCardResult::Found);
        assert!(set.contains_card(10));
        assert!(!set.contains_card(11));
        assert_eq!(set.cards(), vec![10]);
    }

    #[test]
    fn coarsens_to_bitmap() {
        let cfg = config();
        let mut set = G1CardSet::new(cfg);
        // Fill beyond max_cards_in_array (8 * 1MB = 8 entries)
        for i in 0..20u32 {
            set.add_card(i * 7);
        }
        assert!(set.contains_card(19 * 7));
        let cards = set.cards();
        assert_eq!(cards.len(), 20);
        assert!(cards.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn coarsens_to_howl_and_full() {
        let mut set = G1CardSet::new(config());
        // Add many cards to force Array->Bitmap->Howl->Full chain.
        for i in 0..1500u32 {
            set.add_card(i);
        }
        assert_eq!(set.cards().len(), 1500);
        // Now add every card in region 0 -> should become Full.
        for i in 0..2048u32 {
            set.add_card(i);
        }
        for i in 0..2048u32 {
            assert!(set.contains_card(i), "missing card {}", i);
        }
    }

    #[test]
    fn remove_card() {
        let mut set = G1CardSet::new(config());
        set.add_card(5);
        set.add_card(9);
        assert!(set.remove_card(5));
        assert!(!set.remove_card(5));
        assert!(set.contains_card(9));
        assert_eq!(set.cards(), vec![9]);
    }

    #[test]
    fn multi_region_cards() {
        let mut set = G1CardSet::new(config());
        // card indexes spanning two card regions (2048 cards each)
        set.add_card(100);
        set.add_card(2048 + 200);
        set.add_card(4096 + 7);
        let mut cards = set.cards();
        cards.sort_unstable();
        assert_eq!(cards, vec![100, 2248, 4103]);
    }
}
