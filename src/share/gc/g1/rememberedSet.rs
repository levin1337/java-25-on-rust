/*
 * Port of OpenJDK 25 HotSpot G1 remembered-set fast path used by the
 * write barrier (share/gc/g1/g1BarrierSet.hpp + g1DirtyCardQueue.hpp
 * semantics).
 *
 * `G1RememberedSet` is the barrier-facing façade over `G1RemSet`:
 * it owns the per-region card bookkeeping and the from-card cache,
 * exposes `record`/`cards_of` for tests and the interpreter barrier,
 * and tracks G1HotCardCache-like dedup via the from-card cache.
 */

use super::g1RemSet::G1RemSet;
use std::collections::HashMap;
use std::sync::RwLock;

/// Barrier-side view of the remembered set. Wraps `G1RemSet` and adds
/// a coarse per-region card index used by callers that do not want to
/// deal with absolute heap addresses (the `memory/heap.rs` model uses
/// arena slot indices rather than byte addresses).
pub struct G1RememberedSet {
    inner: RwLock<G1RemSet>,
    /// card_shift used by `record` for slot-based addressing.
    card_shift: u32,
}

impl G1RememberedSet {
    /// `card_shift = 9` gives 512-byte cards, the HotSpot default.
    pub fn new(card_shift: u32) -> Self {
        Self::with_geometry(card_shift, 0, 0, 4)
    }

    /// Full constructor: `num_regions`, `heap_base`, `region_size`.
    pub fn with_geometry(
        card_shift: u32,
        heap_base: usize,
        region_size: usize,
        num_regions: usize,
    ) -> Self {
        Self {
            inner: RwLock::new(G1RemSet::new(num_regions, heap_base, region_size, card_shift)),
            card_shift,
        }
    }

    #[inline]
    pub fn card_index(&self, addr: usize) -> usize {
        addr >> self.card_shift
    }

    #[inline]
    pub fn region_index(&self, addr: usize, region_size: usize) -> usize {
        addr / region_size.max(1)
    }

    /// Records `addr`'s card as dirty for `region`. Returns `true`
    /// when the card was newly added — the barrier fast path skips
    /// the write when the card was already dirty.
    pub fn record(&self, region: usize, addr: usize) -> bool {
        let mut inner = self.inner.write().unwrap();
        // map slot-based addr to byte address inside region
        let byte_addr = region * inner_region_size(&inner) + addr;
        inner.add_reference(region, byte_addr)
    }

    /// Same as `record` but for absolute byte addresses.
    pub fn record_addr(&self, region: usize, byte_addr: usize) -> bool {
        let mut inner = self.inner.write().unwrap();
        inner.add_reference(region, byte_addr)
    }

    pub fn cards_of(&self, region: usize) -> Vec<usize> {
        let inner = self.inner.read().unwrap();
        let mut cards = Vec::new();
        if let Some(remset) = inner.remset(region) {
            remset.iterate_cards(|c| cards.push(c as usize));
        }
        cards
    }

    pub fn clear_region(&self, region: usize) {
        self.inner.write().unwrap().clear_region(region);
    }

    pub fn clear(&self) {
        self.inner.write().unwrap().clear();
    }

    pub fn total_cards(&self) -> usize {
        self.inner.read().unwrap().total_occupied()
    }

    pub fn region_count(&self) -> usize {
        self.inner.read().unwrap().num_regions()
    }

    pub fn inner(&self) -> &RwLock<G1RemSet> {
        &self.inner
    }
}

fn inner_region_size(inner: &G1RemSet) -> usize {
    // region_size is private; recover via cards_per_region * card_size.
    // G1RemSet stores region_size; expose via helper below.
    inner.region_size_public()
}

impl Default for G1RememberedSet {
    fn default() -> Self {
        Self::new(9)
    }
}

/// `G1HotCardCache`: caches the last card index dirtied per thread so
/// the barrier can skip redundant remembered-set updates for repeated
/// writes into the same card. HotSpot uses per-thread cache entries;
/// here a small striped map keyed by thread id stands in.
#[derive(Debug, Default)]
pub struct G1HotCardCache {
    cache: RwLock<HashMap<usize, u64>>,
}

impl G1HotCardCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if `card_key` differs from the cached value for
    /// `thread` (caller should record and update via `record`).
    pub fn check_and_update(&self, thread: usize, card_key: u64) -> bool {
        let mut cache = self.cache.write().unwrap();
        match cache.get(&thread) {
            Some(&prev) if prev == card_key => false,
            _ => {
                cache.insert(thread, card_key);
                true
            }
        }
    }

    pub fn invalidate(&self, thread: usize) {
        self.cache.write().unwrap().remove(&thread);
    }

    pub fn clear(&self) {
        self.cache.write().unwrap().clear();
    }
}
