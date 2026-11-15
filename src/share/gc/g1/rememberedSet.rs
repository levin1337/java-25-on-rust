use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

/// G1 remembered set: per-region sets of dirty cards recorded by the
/// post-write barrier. A card covers `card_shift`-aligned bytes of a
/// region; during young evacuation the collector iterates the cards of
/// each old region to find references into the collection set
/// (HotSpot `G1RemSet`/`G1CardTable` semantics, coarse rather than the
/// full per-region fine-grained tables).
pub struct G1RememberedSet {
    card_shift: u32,
    cards: RwLock<HashMap<usize, HashSet<usize>>>,
    total_cards: RwLock<usize>,
}

impl G1RememberedSet {
    /// `card_shift = 9` gives 512-byte cards, the HotSpot default card
    /// size for G1's card table.
    pub fn new(card_shift: u32) -> Self {
        Self {
            card_shift,
            cards: RwLock::new(HashMap::new()),
            total_cards: RwLock::new(0),
        }
    }

    #[inline]
    pub fn card_index(&self, addr: usize) -> usize {
        addr >> self.card_shift
    }

    #[inline]
    pub fn region_index(&self, addr: usize, region_size: usize) -> usize {
        addr / region_size
    }

    /// Records `addr`'s card as dirty for `region`. Returns `true` when
    /// the card was newly added (the barrier fast path can skip the
    /// write when the card was already dirty).
    pub fn record(&self, region: usize, addr: usize) -> bool {
        let card = self.card_index(addr);
        let mut cards = self.cards.write().unwrap();
        let entry = cards.entry(region).or_default();
        if entry.insert(card) {
            *self.total_cards.write().unwrap() += 1;
            true
        } else {
            false
        }
    }

    pub fn cards_of(&self, region: usize) -> Vec<usize> {
        self.cards
            .read()
            .unwrap()
            .get(&region)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn clear_region(&self, region: usize) {
        if let Some(set) = self.cards.write().unwrap().remove(&region) {
            let mut total = self.total_cards.write().unwrap();
            *total = total.saturating_sub(set.len());
        }
    }

    pub fn clear(&self) {
        self.cards.write().unwrap().clear();
        *self.total_cards.write().unwrap() = 0;
    }

    pub fn total_cards(&self) -> usize {
        *self.total_cards.read().unwrap()
    }

    pub fn region_count(&self) -> usize {
        self.cards.read().unwrap().len()
    }
}

impl Default for G1RememberedSet {
    fn default() -> Self {
        Self::new(9)
    }
}
