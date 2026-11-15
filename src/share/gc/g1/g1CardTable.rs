/*
 * Port of OpenJDK 25 HotSpot G1CardTable (share/gc/g1/g1CardTable.hpp)
 * built on the generic CardTable (share/gc/shared/cardTable.hpp).
 *
 * The card table maps each 512-byte card of the heap to a single byte.
 * During G1, card values are used for two purposes:
 *
 *  - Remembered-set / young-generation tracking: cards pointing into
 *    the young generation are marked `g1_young_gen`; other dirty
 *    cards are 0.
 *  - Incremental evacuation: during young collection the table
 *    consolidates cards to scan. LSB semantics:
 *      11111111 (0xff) - clean, do not scan
 *      00000001 (0x01) - already scanned, do not scan
 *      00000000 (0x00) - dirty, needs to be scanned
 *
 * `mark_range_dirty`/`mark_clean_as_dirty` preserve scanned cards.
 */

/// `CardTable::CardValue`.
pub type CardValue = u8;

pub struct G1CardTable {
    /// Byte map: one entry per card covering `heap_size` bytes.
    byte_map: Vec<CardValue>,
    /// Card size in bytes; HotSpot default is 512 (`card_shift = 9`).
    card_size: usize,
    /// Heap base the map starts at.
    base: usize,
    /// Heap size in bytes.
    heap_size: usize,
}

impl G1CardTable {
    /// `G1CardTable::G1CardValues`.
    pub const G1_YOUNG_GEN: CardValue = 0x01; // marker; real HotSpot uses a bit
    /// Bit used for the young-generation marker value.
    pub const YOUNG_GEN_BIT: u8 = 0x02;
    /// `g1_card_already_scanned`.
    pub const G1_CARD_ALREADY_SCANNED: CardValue = 0x01;

    pub const CLEAN: CardValue = 0xFF;
    pub const DIRTY: CardValue = 0x00;

    /// `CardTable::card_shift` default for G1: 512 bytes per card.
    pub const DEFAULT_CARD_SHIFT: u32 = 9;

    pub fn new(base: usize, heap_size: usize, card_shift: u32) -> Self {
        let card_size = 1usize << card_shift;
        let cards = (heap_size + card_size - 1) / card_size;
        Self {
            byte_map: vec![Self::CLEAN; cards.max(1)],
            card_size,
            base,
            heap_size,
        }
    }

    #[inline]
    pub fn card_size(&self) -> usize {
        self.card_size
    }

    #[inline]
    pub fn base(&self) -> usize {
        self.base
    }

    #[inline]
    pub fn heap_size(&self) -> usize {
        self.heap_size
    }

    #[inline]
    pub fn card_index(&self, addr: usize) -> usize {
        addr.saturating_sub(self.base) / self.card_size
    }

    #[inline]
    pub fn address_for_card_index(&self, card_index: usize) -> usize {
        self.base + card_index * self.card_size
    }

    #[inline]
    pub fn num_cards(&self) -> usize {
        self.byte_map.len()
    }

    #[inline]
    pub fn card(&self, index: usize) -> CardValue {
        self.byte_map.get(index).copied().unwrap_or(Self::CLEAN)
    }

    #[inline]
    pub fn card_mut(&mut self, index: usize) -> Option<&mut CardValue> {
        self.byte_map.get_mut(index)
    }

    /// `is_clean`/`is_dirty`: a card is dirty iff its low bit is 0 and
    /// it is not `g1_young_gen`-marked. Scanned cards (0x01) count as
    /// "clean" for scanning purposes.
    #[inline]
    pub fn is_clean(&self, index: usize) -> bool {
        let v = self.card(index);
        v == Self::CLEAN || v == Self::G1_CARD_ALREADY_SCANNED
    }

    #[inline]
    pub fn is_dirty(&self, index: usize) -> bool {
        !self.is_clean(index)
    }

    #[inline]
    pub fn is_scanned(&self, index: usize) -> bool {
        self.card(index) == Self::G1_CARD_ALREADY_SCANNED
    }

    #[inline]
    pub fn is_young_marked(&self, index: usize) -> bool {
        self.card(index) & Self::YOUNG_GEN_BIT != 0
    }

    /// `mark_card_dirty`: sets a clean card to dirty. Returns whether
    /// the card was clean before (matches the HotSpot non-atomic fast
    /// path semantics).
    #[inline]
    pub fn mark_clean_as_dirty(&mut self, index: usize) -> bool {
        if index >= self.byte_map.len() {
            return false;
        }
        let was_clean = self.byte_map[index] == Self::CLEAN;
        if was_clean {
            self.byte_map[index] = Self::DIRTY;
        }
        was_clean
    }

    /// `mark_range_dirty`: set clean cards in `[start_card_index,
    /// start+num_cards)` to dirty, preserving already-scanned cards.
    /// Assumes most cards are clean; scans word-wise in HotSpot, we do
    /// the equivalent per byte (card table granularity).
    pub fn mark_range_dirty(&mut self, start_card_index: usize, num_cards: usize) {
        let end = (start_card_index + num_cards).min(self.byte_map.len());
        for card in &mut self.byte_map[start_card_index..end] {
            if *card == Self::CLEAN {
                *card = Self::DIRTY;
            }
        }
    }

    /// `change_dirty_cards_to`: all cards in range must be dirty;
    /// sets them to `which` (e.g. `G1_CARD_ALREADY_SCANNED` after
    /// scanning or `CLEAN` when resetting).
    pub fn change_dirty_cards_to(&mut self, start: usize, end: usize, which: CardValue) {
        let end = end.min(self.byte_map.len());
        for card in &mut self.byte_map[start.min(end)..end] {
            debug_assert_eq!(*card, Self::DIRTY);
            *card = which;
        }
    }

    /// `g1_mark_as_young`: marks all cards covering `region` as
    /// young-generation cards.
    pub fn mark_as_young(&mut self, region_start: usize, region_end: usize) {
        let start_card = self.card_index(region_start);
        let end_card = self.card_index(region_end.saturating_sub(1));
        for index in start_card..=end_card.min(self.byte_map.len().saturating_sub(1)) {
            self.byte_map[index] = Self::G1_YOUNG_GEN | Self::YOUNG_GEN_BIT;
        }
    }

    /// Iterates dirty (scannable) card indices in a range. Used by
    /// remembered-set scanning and the dirty-card queue.
    pub fn for_each_dirty_card<F: FnMut(usize)>(&self, start_card: usize, num_cards: usize, mut f: F) {
        let end = (start_card + num_cards).min(self.byte_map.len());
        for index in start_card..end {
            if self.byte_map[index] == Self::DIRTY {
                f(index);
            }
        }
    }

    /// `clear`: resets the whole table to clean.
    pub fn clear(&mut self) {
        self.byte_map.fill(Self::CLEAN);
    }

    /// Iterate every card index that is dirty.
    pub fn dirty_cards(&self) -> impl Iterator<Item = usize> + '_ {
        self.byte_map
            .iter()
            .enumerate()
            .filter(|(_, v)| **v == Self::DIRTY)
            .map(|(i, _)| i)
    }

    /// Snapshot of the raw byte map (for verification / serialization).
    pub fn byte_map(&self) -> &[CardValue] {
        &self.byte_map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_index_math() {
        let table = G1CardTable::new(0, 4096, 9); // 8 cards
        assert_eq!(table.card_index(0), 0);
        assert_eq!(table.card_index(511), 0);
        assert_eq!(table.card_index(512), 1);
        assert_eq!(table.num_cards(), 8);
        assert_eq!(table.address_for_card_index(3), 1536);
    }

    #[test]
    fn dirty_preserves_scanned() {
        let mut table = G1CardTable::new(0, 2048, 9);
        table.mark_range_dirty(0, 4); // cards 0..4 dirty
        assert!(table.mark_clean_as_dirty(0) == false); // already dirty
        table.change_dirty_cards_to(0, 2, G1CardTable::G1_CARD_ALREADY_SCANNED);
        assert!(table.is_scanned(0));
        assert!(table.is_scanned(1));
        assert!(table.is_clean(0)); // scanned counts as clean
        assert_eq!(table.card(2), G1CardTable::DIRTY);
        table.mark_range_dirty(0, 4); // must not resurrect scanned cards
        assert!(table.is_scanned(0));
        assert!(table.is_scanned(1));
        assert_eq!(table.card(2), G1CardTable::DIRTY);
    }
}
