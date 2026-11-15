/*
 * Port of OpenJDK 25 HotSpot G1RemSet (share/gc/g1/g1RemSet.hpp).
 *
 * The heap-wide remembered set: one G1HeapRegionRemSet per region,
 * the from-card cache consulted by the post-write barrier, and the
 * collection-set candidates that own the shared card sets.
 *
 * Heap integration point: callers provide `card_shift` (512-byte
 * cards) and `heap_base`; `add_reference` is invoked by the write
 * barrier after the from-card cache check.
 */

use super::g1CardSet::G1CardSetConfiguration;
use super::g1CollectionSetCandidates::{G1CSetCandidateGroupRef, G1CollectionSetCandidates};
use super::g1FromCardCache::G1FromCardCache;
use super::g1HeapRegionRemSet::{G1HeapRegionRemSet, RemSetState};

/// `G1RemSet`: heap-wide remembered set.
#[derive(Debug)]
pub struct G1RemSet {
    remsets: Vec<G1HeapRegionRemSet>,
    from_card_cache: G1FromCardCache,
    candidates: G1CollectionSetCandidates,
    card_shift: u32,
    card_size: usize,
    heap_base: usize,
    num_regions: usize,
    region_size: usize,
}

impl G1RemSet {
    pub fn new(num_regions: usize, heap_base: usize, region_size: usize, card_shift: u32) -> Self {
        let card_size = 1usize << card_shift;
        let cards_per_region = (region_size / card_size.max(1)) as u32;
        let mut remsets = Vec::with_capacity(num_regions);
        for _ in 0..num_regions {
            remsets.push(G1HeapRegionRemSet::new(card_shift, heap_base));
        }
        Self {
            remsets,
            from_card_cache: G1FromCardCache::new(num_regions, cards_per_region),
            candidates: G1CollectionSetCandidates::new(num_regions),
            card_shift,
            card_size,
            heap_base,
            num_regions,
            region_size,
        }
    }

    #[inline]
    pub fn card_shift(&self) -> u32 {
        self.card_shift
    }

    #[inline]
    pub fn card_size(&self) -> usize {
        self.card_size
    }

    #[inline]
    pub fn heap_base(&self) -> usize {
        self.heap_base
    }

    #[inline]
    pub fn num_regions(&self) -> usize {
        self.num_regions
    }

    #[inline]
    pub fn region_size(&self) -> usize {
        self.region_size
    }

    /// Alias kept for callers that stored region size separately.
    #[inline]
    pub fn region_size_public(&self) -> usize {
        self.region_size
    }

    #[inline]
    pub fn remset(&self, region: usize) -> Option<&G1HeapRegionRemSet> {
        self.remsets.get(region)
    }

    #[inline]
    pub fn remset_mut(&mut self, region: usize) -> Option<&mut G1HeapRegionRemSet> {
        self.remsets.get_mut(region)
    }

    #[inline]
    pub fn remsets(&self) -> &[G1HeapRegionRemSet] {
        &self.remsets
    }

    #[inline]
    pub fn remsets_mut(&mut self) -> &mut [G1HeapRegionRemSet] {
        &mut self.remsets
    }

    #[inline]
    pub fn from_card_cache(&self) -> &G1FromCardCache {
        &self.from_card_cache
    }

    #[inline]
    pub fn from_card_cache_mut(&mut self) -> &mut G1FromCardCache {
        &mut self.from_card_cache
    }

    #[inline]
    pub fn candidates(&self) -> &G1CollectionSetCandidates {
        &self.candidates
    }

    #[inline]
    pub fn candidates_mut(&mut self) -> &mut G1CollectionSetCandidates {
        &mut self.candidates
    }

    /// `region_to_card`: absolute card index of `addr`.
    #[inline]
    pub fn to_card(&self, addr: usize) -> u32 {
        (addr.saturating_sub(self.heap_base) >> self.card_shift) as u32
    }

    /// `to_region_card`: card index within the region containing
    /// `addr` (0..cards_per_region).
    #[inline]
    pub fn to_region_card(&self, addr: usize, region_index: usize) -> u32 {
        let region_base = self.heap_base + region_index * self.region_size;
        (addr.saturating_sub(region_base) / self.card_size) as u32
    }

    /// Post-write barrier fast path: `add_reference(from_region,
    /// from_addr)` after the dirty-card check. Uses the from-card
    /// cache so repeat writes to the same card are cheap.
    pub fn add_reference(&mut self, from_region: usize, from_addr: usize) -> bool {
        let region_base = self.heap_base + from_region * self.region_size_bytes();
        let card_in_region = (from_addr.saturating_sub(region_base)) / self.card_size;
        if !self.from_card_cache.add(from_region, card_in_region as u32) {
            return false;
        }
        self.remsets[from_region].add_reference(from_addr)
    }

    /// Raw `add_reference` bypassing the from-card cache (used when
    /// the barrier already knows the card is new, e.g. dirty card
    /// queue processing).
    pub fn add_reference_uncached(&mut self, from_region: usize, from_addr: usize) -> bool {
        self.remsets[from_region].add_reference(from_addr)
    }

    fn region_size_bytes(&self) -> usize {
        self.region_size
    }

    /// `prepare_for_young_collection` / candidate-group assignment:
    /// assigns each region in `candidates` to a shared card-set group.
    /// Returns the number of groups created.
    pub fn assign_candidate_groups(
        &mut self,
        candidates: &[usize],
        group_size: usize,
        config: G1CardSetConfiguration,
    ) -> usize {
        let mut groups_created = 0;
        for chunk in candidates.chunks(group_size.max(1)) {
            let group: G1CSetCandidateGroupRef =
                self.candidates.create_group(config.clone());
            for &region in chunk {
                if let Some(remset) = self.remsets.get_mut(region) {
                    if remset.state() == RemSetState::Untracked {
                        remset.set_state_updating();
                    }
                    remset.install_cset_group(group.clone());
                }
            }
            groups_created += 1;
        }
        groups_created
    }

    /// Removes all candidate-group assignments (end of young GC).
    pub fn clear_candidate_groups(&mut self) {
        for remset in &mut self.remsets {
            remset.uninstall_cset_group();
        }
        self.candidates.clear();
    }

    /// Iterate dirty cards of the current collection set candidates:
    /// calls `f(card_index)` for every recorded card.
    pub fn for_each_candidate_card<F: FnMut(u32)>(&self, mut f: F) {
        self.candidates.for_each_card(|c, _| f(c));
    }

    /// Iterate cards belonging to a specific region within the
    /// candidates.
    pub fn for_each_region_card<F: FnMut(u32)>(
        &self,
        region_index: usize,
        cards_per_region: u32,
        f: F,
    ) {
        if let Some(remset) = self.remsets.get(region_index) {
            remset.iterate_own_cards(region_index, cards_per_region, f);
        }
    }

    /// `clear_region`: drops the region's remembered set state.
    pub fn clear_region(&mut self, region: usize) {
        if let Some(remset) = self.remsets.get_mut(region) {
            remset.clear(false, false);
        }
        self.from_card_cache.clear_region(region);
    }

    /// `clear`: full reset (e.g. after full GC).
    pub fn clear(&mut self) {
        for remset in &mut self.remsets {
            remset.clear(false, false);
        }
        self.from_card_cache.clear();
        self.candidates.clear();
    }

    /// Total occupied card regions across all remsets.
    pub fn total_occupied(&self) -> usize {
        self.remsets.iter().map(|r| r.occupied()).sum()
    }

    /// Number of tracked (non-untracked) regions.
    pub fn tracked_regions(&self) -> usize {
        self.remsets.iter().filter(|r| r.is_tracked()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::g1CardSet::G1CardSetConfiguration;

    #[test]
    fn add_reference_and_cards() {
        let mut remset = G1RemSet::new(4, 0, 1024 * 1024, 9);
        let groups = remset.assign_candidate_groups(
            &[1],
            4,
            G1CardSetConfiguration::default_for_region(1024 * 1024, 512),
        );
        assert_eq!(groups, 1);
        // region 1 starts at 1 MiB; addr = base + 3*512 -> card 3 in region
        let addr = 1024 * 1024 + 512 * 3;
        assert!(remset.add_reference_uncached(1, addr));
        // same card again -> already present
        assert!(!remset.add_reference_uncached(1, addr + 100));
        let mut cards = Vec::new();
        remset.for_each_region_card(1, 2048, |c| cards.push(c));
        // absolute card index = (1MiB + 1536) >> 9 = 2048 + 3
        assert_eq!(cards, vec![2051]);
    }

    #[test]
    fn from_card_cache_dedup() {
        let mut remset = G1RemSet::new(2, 0, 4096, 9);
        let a = 100usize;
        let first = remset.from_card_cache_mut().add(0, (a / 512) as u32);
        let second = remset.from_card_cache_mut().add(0, (a / 512) as u32);
        assert!(first);
        assert!(!second);
    }

    #[test]
    fn add_reference_uses_from_card_cache() {
        let mut remset = G1RemSet::new(2, 0, 4096, 9);
        remset.assign_candidate_groups(
            &[0],
            2,
            G1CardSetConfiguration::default_for_region(4096, 512),
        );
        // region_size=4096 -> only 8 cards per region
        let addr = 256usize; // card 0 of region 0
        assert!(remset.add_reference(0, addr));
        assert!(!remset.add_reference(0, addr + 64)); // same card, cached
        let addr2 = 512usize; // card 1
        assert!(remset.add_reference(0, addr2));
    }

    #[test]
    fn clear_region() {
        let mut remset = G1RemSet::new(2, 0, 4096, 9);
        remset.assign_candidate_groups(
            &[0],
            2,
            G1CardSetConfiguration::default_for_region(4096, 512),
        );
        remset.add_reference_uncached(0, 256);
        remset.clear_region(0);
        assert_eq!(remset.from_card_cache().count(0), 0);
    }
}
