/*
 * Port of OpenJDK 25 HotSpot G1CSetCandidateGroup /
 * G1CollectionSetCandidates (share/gc/g1/g1CollectionSetCandidates.hpp,
 * g1CollectionSet.hpp) — the grouping of old regions that share one
 * G1CardSet remembered-set table during young collections.
 *
 * In HotSpot each candidate group owns a G1CardSet plus a
 * G1CardSetMemoryManager; regions of the group install/uninstall the
 * group on their G1HeapRegionRemSet. This port keeps the same
 * structure with Rc<RefCell<>> sharing instead of raw pointers.
 */

use super::g1CardSet::{G1CardSet, G1CardSetConfiguration};
use std::cell::RefCell;
use std::rc::Rc;

/// `G1CSetCandidateGroup`: a group of candidate regions sharing one
/// card set. `group_id` corresponds to HotSpot's group id used for
/// remembered-set iteration ordering.
#[derive(Debug)]
pub struct G1CSetCandidateGroup {
    group_id: u32,
    card_set: G1CardSet,
    /// Number of regions currently installed in this group.
    num_regions: usize,
}

impl G1CSetCandidateGroup {
    pub fn new(group_id: u32, config: G1CardSetConfiguration) -> Self {
        Self { group_id, card_set: G1CardSet::new(config), num_regions: 0 }
    }

    #[inline]
    pub fn group_id(&self) -> u32 {
        self.group_id
    }

    #[inline]
    pub fn card_set(&self) -> &G1CardSet {
        &self.card_set
    }

    #[inline]
    pub fn card_set_mut(&mut self) -> &mut G1CardSet {
        &mut self.card_set
    }

    #[inline]
    pub fn num_regions(&self) -> usize {
        self.num_regions
    }

    pub fn install_region(&mut self) {
        self.num_regions += 1;
    }

    pub fn uninstall_region(&mut self) {
        self.num_regions = self.num_regions.saturating_sub(1);
    }

    /// `is_empty`: the shared card set has no entries.
    pub fn is_empty(&self) -> bool {
        self.card_set.is_empty()
    }

    pub fn occupied(&self) -> usize {
        self.card_set.occupied()
    }

    pub fn num_cards(&self) -> usize {
        self.card_set.num_cards()
    }
}

/// Shared handle to a candidate group, mirroring the raw
/// `G1CSetCandidateGroup*` pointer stored in G1HeapRegionRemSet.
pub type G1CSetCandidateGroupRef = Rc<RefCell<G1CSetCandidateGroup>>;

/// `G1CollectionSetCandidates`: the set of candidate groups selected
/// for the current young collection, ordered by group id.
#[derive(Debug, Default)]
pub struct G1CollectionSetCandidates {
    groups: Vec<G1CSetCandidateGroupRef>,
    /// Map region index -> group for quick lookup.
    region_to_group: Vec<Option<usize>>,
}

impl G1CollectionSetCandidates {
    pub fn new(num_regions: usize) -> Self {
        Self { groups: Vec::new(), region_to_group: vec![None; num_regions] }
    }

    /// `create_group`: allocates a new candidate group.
    pub fn create_group(&mut self, config: G1CardSetConfiguration) -> G1CSetCandidateGroupRef {
        let id = self.groups.len() as u32;
        let group = Rc::new(RefCell::new(G1CSetCandidateGroup::new(id, config)));
        self.groups.push(group.clone());
        group
    }

    /// `assign_region_to_group`.
    pub fn assign_region_to_group(&mut self, region_index: usize, group: &G1CSetCandidateGroupRef) {
        if region_index < self.region_to_group.len() {
            let gid = group.borrow().group_id() as usize;
            self.region_to_group[region_index] = Some(gid);
            group.borrow_mut().install_region();
        }
    }

    /// `remove_region_from_group`.
    pub fn remove_region_from_group(&mut self, region_index: usize) {
        if let Some(gid) = self.region_to_group.get_mut(region_index).and_then(|g| g.take()) {
            if let Some(group) = self.groups.get(gid) {
                group.borrow_mut().uninstall_region();
            }
        }
    }

    pub fn group_of(&self, region_index: usize) -> Option<G1CSetCandidateGroupRef> {
        self.region_to_group
            .get(region_index)
            .and_then(|g| *g)
            .and_then(|gid| self.groups.get(gid).cloned())
    }

    pub fn groups(&self) -> &[G1CSetCandidateGroupRef] {
        &self.groups
    }

    pub fn num_groups(&self) -> usize {
        self.groups.len()
    }

    /// Iterate all (card_index, group) pairs across groups; used by
    /// remembered-set scanning.
    pub fn for_each_card<F: FnMut(u32, &G1CSetCandidateGroupRef)>(&self, mut f: F) {
        for group in &self.groups {
            let mut cards = Vec::new();
            group.borrow().card_set().for_each_card(|c| cards.push(c));
            for c in cards {
                f(c, group);
            }
        }
    }

    pub fn clear(&mut self) {
        for group in &self.groups {
            group.borrow_mut().card_set_mut().clear();
        }
        for slot in &mut self.region_to_group {
            *slot = None;
        }
    }
}

