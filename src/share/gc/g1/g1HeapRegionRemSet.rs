/*
 * Port of OpenJDK 25 HotSpot G1HeapRegionRemSet
 * (share/gc/g1/g1HeapRegionRemSet.hpp).
 *
 * Per-region remembered set: code roots (nmethod handles), the
 * collection-set candidate group the region's card set lives in, and
 * the Untracked/Updating/Complete state machine that drives
 * concurrent refinement.
 */

use super::g1CollectionSetCandidates::G1CSetCandidateGroupRef;
use super::g1CardSet::G1AddCardResult;

/// `G1HeapRegionRemSet::RemSetState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemSetState {
    /// No remembered set information is kept for this region.
    Untracked,
    /// The remembered set is being updated (concurrent refinement).
    Updating,
    /// The remembered set is complete and consistent.
    Complete,
}

/// `G1HeapRegionRemSet`.
#[derive(Debug)]
pub struct G1HeapRegionRemSet {
    state: RemSetState,
    /// nmethod/code-root handles pointing into this region.
    code_roots: Vec<usize>,
    /// Candidate group owning the shared G1CardSet, if the region is
    /// a candidate for the current collection set.
    cset_group: Option<G1CSetCandidateGroupRef>,
    /// Card shift used to compute `to_card(from)`.
    card_shift: u32,
    /// Heap base for card index computation.
    heap_base: usize,
}

impl G1HeapRegionRemSet {
    pub fn new(card_shift: u32, heap_base: usize) -> Self {
        Self {
            state: RemSetState::Untracked,
            code_roots: Vec::new(),
            cset_group: None,
            card_shift,
            heap_base,
        }
    }

    #[inline]
    pub fn state(&self) -> RemSetState {
        self.state
    }

    #[inline]
    pub fn is_tracked(&self) -> bool {
        self.state != RemSetState::Untracked
    }

    #[inline]
    pub fn is_updating(&self) -> bool {
        self.state == RemSetState::Updating
    }

    #[inline]
    pub fn is_complete(&self) -> bool {
        self.state == RemSetState::Complete
    }

    pub fn set_state_untracked(&mut self) {
        self.state = RemSetState::Untracked;
    }

    pub fn set_state_updating(&mut self) {
        self.state = RemSetState::Updating;
    }

    pub fn set_state_complete(&mut self) {
        self.state = RemSetState::Complete;
    }

    /// `install_cset_group` / `uninstall_cset_group`.
    pub fn install_cset_group(&mut self, group: G1CSetCandidateGroupRef) {
        debug_assert!(self.cset_group.is_none());
        self.cset_group = Some(group);
    }

    pub fn uninstall_cset_group(&mut self) {
        self.cset_group = None;
    }

    #[inline]
    pub fn is_added_to_cset_group(&self) -> bool {
        self.cset_group.is_some()
    }

    #[inline]
    pub fn cset_group(&self) -> Option<&G1CSetCandidateGroupRef> {
        self.cset_group.as_ref()
    }

    #[inline]
    pub fn cset_group_id(&self) -> Option<u32> {
        self.cset_group.as_ref().map(|g| g.borrow().group_id())
    }

    /// `cardset_is_empty`.
    pub fn cardset_is_empty(&self) -> bool {
        match &self.cset_group {
            None => true,
            Some(g) => g.borrow().is_empty(),
        }
    }

    /// `is_empty`: no code roots and empty card set.
    pub fn is_empty(&self) -> bool {
        self.code_roots.is_empty() && self.cardset_is_empty()
    }

    /// `occupied`: occupied card regions in the shared card set.
    pub fn occupied(&self) -> usize {
        self.cset_group
            .as_ref()
            .map(|g| g.borrow().occupied())
            .unwrap_or(0)
    }

    /// `to_card`: absolute card index of `from`.
    #[inline]
    pub fn to_card(&self, from: usize) -> u32 {
        (from.saturating_sub(self.heap_base) >> self.card_shift) as u32
    }

    /// `add_reference`: records `from`'s card in the shared card set.
    /// Returns `true` when the card was newly added; `false` when the
    /// region has no candidate group (untracked) or the card was
    /// already present.
    pub fn add_reference(&mut self, from: usize) -> bool {
        let card = self.to_card(from);
        match &self.cset_group {
            Some(group) => {
                let result = group.borrow_mut().card_set_mut().add_card(card);
                result == G1AddCardResult::Added
            }
            None => false,
        }
    }

    /// `contains_reference`.
    pub fn contains_reference(&self, from: usize) -> bool {
        let card = self.to_card(from);
        self.cset_group
            .as_ref()
            .map(|g| g.borrow().card_set().contains_card(card))
            .unwrap_or(false)
    }

    /// Iterate all cards recorded for this region's group, calling
    /// `f` with the absolute card index.
    pub fn iterate_cards<F: FnMut(u32)>(&self, mut f: F) {
        if let Some(group) = &self.cset_group {
            let mut cards = Vec::new();
            group.borrow().card_set().for_each_card(|c| cards.push(c));
            for c in cards {
                f(c);
            }
        }
    }

    /// Iterate cards that belong to this region (card indexes within
    /// the region's own card range).
    pub fn iterate_own_cards<F: FnMut(u32)>(&self, region_index: usize, region_cards: u32, mut f: F) {
        let base = region_index as u32 * region_cards;
        let end = base + region_cards;
        self.iterate_cards(|c| {
            if c >= base && c < end {
                f(c);
            }
        });
    }

    /// `iterate_for_merge`: visit cards for merging into the card
    /// table at the start of evacuation.
    pub fn iterate_for_merge<F: FnMut(u32)>(&self, f: F) {
        self.iterate_cards(f);
    }

    /// `clear`: empties the card set contribution. Note: in HotSpot
    /// the card set is shared per group, so clearing affects the whole
    /// group — callers clear per group via G1CollectionSetCandidates.
    pub fn clear_card_set(&mut self) {
        if let Some(group) = &self.cset_group {
            group.borrow_mut().card_set_mut().clear();
        }
    }

    /// `clear(only_cardset, keep_tracked)`.
    pub fn clear(&mut self, only_card_set: bool, keep_tracked: bool) {
        self.clear_card_set();
        if !only_card_set {
            self.code_roots.clear();
        }
        if !keep_tracked {
            self.state = RemSetState::Untracked;
        }
    }

    // -- code roots ---------------------------------------------------

    pub fn add_code_root(&mut self, nm: usize) {
        if !self.code_roots.contains(&nm) {
            self.code_roots.push(nm);
        }
    }

    pub fn remove_code_root(&mut self, nm: usize) {
        self.code_roots.retain(|&x| x != nm);
    }

    pub fn bulk_remove_code_roots(&mut self) {
        self.code_roots.clear();
    }

    pub fn code_roots(&self) -> &[usize] {
        &self.code_roots
    }

    #[inline]
    pub fn code_roots_list_length(&self) -> usize {
        self.code_roots.len()
    }

    #[inline]
    pub fn code_roots_list_contains(&self, nm: usize) -> bool {
        self.code_roots.contains(&nm)
    }

    /// `clean_code_roots`: stub for heap integration — removes code
    /// roots of a region being reclaimed.
    pub fn clean_code_roots(&mut self) {
        self.code_roots.clear();
    }
}
