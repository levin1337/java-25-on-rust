/*
 * Port of OpenJDK 25 HotSpot ConstantPoolCache (share/oops/cpCache.hpp)
 *
 * The ConstantPoolCache is a runtime resolution table used by the interpreter
 * to cache resolved field offsets, method entries, and invokedynamic call sites.
 */

use super::resolvedFieldEntry::ResolvedFieldEntry;
use super::resolvedIndyEntry::ResolvedIndyEntry;
use super::resolvedMethodEntry::ResolvedMethodEntry;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConstantPoolCache {
    field_entries: Vec<ResolvedFieldEntry>,
    method_entries: Vec<ResolvedMethodEntry>,
    indy_entries: Vec<ResolvedIndyEntry>,
}

impl ConstantPoolCache {
    // HotSpot cpCache.hpp: tos_state is packed into the top 4 bits of a
    // 32-bit cache entry word.
    pub const TOS_STATE_BITS: u8 = 4;
    pub const TOS_STATE_MASK: u8 = (1 << Self::TOS_STATE_BITS) - 1;
    pub const TOS_STATE_SHIFT: u8 = 32 - Self::TOS_STATE_BITS as u8;

    /// Packs a top-of-stack state nibble into the high bits of a cache
    /// entry word (HotSpot `tos_state` encoding).
    #[inline]
    pub fn pack_tos_state(entry_word: u32, tos_state: u8) -> u32 {
        (entry_word & !(u32::from(Self::TOS_STATE_MASK) << Self::TOS_STATE_SHIFT))
            | (u32::from(tos_state & Self::TOS_STATE_MASK) << Self::TOS_STATE_SHIFT)
    }

    /// Extracts the tos_state nibble from a packed cache entry word.
    #[inline]
    pub fn unpack_tos_state(entry_word: u32) -> u8 {
        ((entry_word >> Self::TOS_STATE_SHIFT) & u32::from(Self::TOS_STATE_MASK)) as u8
    }

    pub fn new() -> Self {
        Self {
            field_entries: Vec::new(),
            method_entries: Vec::new(),
            indy_entries: Vec::new(),
        }
    }

    #[inline]
    pub fn field_entries_count(&self) -> usize {
        self.field_entries.len()
    }

    #[inline]
    pub fn method_entries_count(&self) -> usize {
        self.method_entries.len()
    }

    #[inline]
    pub fn indy_entries_count(&self) -> usize {
        self.indy_entries.len()
    }

    #[inline]
    pub fn field_entry_at(&self, index: usize) -> Option<&ResolvedFieldEntry> {
        self.field_entries.get(index)
    }

    #[inline]
    pub fn field_entry_at_mut(&mut self, index: usize) -> Option<&mut ResolvedFieldEntry> {
        self.field_entries.get_mut(index)
    }

    #[inline]
    pub fn method_entry_at(&self, index: usize) -> Option<&ResolvedMethodEntry> {
        self.method_entries.get(index)
    }

    #[inline]
    pub fn method_entry_at_mut(&mut self, index: usize) -> Option<&mut ResolvedMethodEntry> {
        self.method_entries.get_mut(index)
    }

    #[inline]
    pub fn indy_entry_at(&self, index: usize) -> Option<&ResolvedIndyEntry> {
        self.indy_entries.get(index)
    }

    #[inline]
    pub fn indy_entry_at_mut(&mut self, index: usize) -> Option<&mut ResolvedIndyEntry> {
        self.indy_entries.get_mut(index)
    }

    /// Iterates all field entries (HotSpot `resolved_field_entries`).
    pub fn field_entries(&self) -> impl Iterator<Item = &ResolvedFieldEntry> {
        self.field_entries.iter()
    }

    /// Iterates all method entries (HotSpot `resolved_method_entries`).
    pub fn method_entries(&self) -> impl Iterator<Item = &ResolvedMethodEntry> {
        self.method_entries.iter()
    }

    /// Iterates all invokedynamic entries (HotSpot
    /// `resolved_indy_entries`).
    pub fn indy_entries(&self) -> impl Iterator<Item = &ResolvedIndyEntry> {
        self.indy_entries.iter()
    }

    /// Total number of cache entries across all kinds.
    #[inline]
    pub fn total_entries(&self) -> usize {
        self.field_entries.len() + self.method_entries.len() + self.indy_entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_entries() == 0
    }

    /// Resolved field entry for a constant-pool index, if cached.
    pub fn resolved_field_at(&self, cpool_index: u16) -> Option<&ResolvedFieldEntry> {
        self.find_field_entry_by_cpool_index(cpool_index)
            .and_then(|index| self.field_entry_at(index))
    }

    /// Resolved method entry for a constant-pool index, if cached.
    pub fn resolved_method_at(&self, cpool_index: u16) -> Option<&ResolvedMethodEntry> {
        self.find_method_entry_by_cpool_index(cpool_index)
            .and_then(|index| self.method_entry_at(index))
    }

    /// Resolved invokedynamic entry for a constant-pool index, if cached.
    pub fn resolved_indy_at(&self, cpool_index: u16) -> Option<&ResolvedIndyEntry> {
        self.find_indy_entry_by_cpool_index(cpool_index)
            .and_then(|index| self.indy_entry_at(index))
    }

    /// Resets every entry to its unresolved state while keeping the
    /// tables allocated (HotSpot
    /// `remove_resolved_*_entries_if_non_deterministic` analogue for
    /// non-deterministic replays).
    pub fn reset_entries(&mut self) {
        for entry in &mut self.field_entries {
            entry.clear();
        }
        for entry in &mut self.method_entries {
            entry.clear();
        }
        for entry in &mut self.indy_entries {
            entry.reset();
        }
    }

    pub fn add_field_entry(&mut self, entry: ResolvedFieldEntry) -> usize {
        let index = self.field_entries.len();
        self.field_entries.push(entry);
        index
    }

    pub fn add_method_entry(&mut self, entry: ResolvedMethodEntry) -> usize {
        let index = self.method_entries.len();
        self.method_entries.push(entry);
        index
    }

    pub fn add_indy_entry(&mut self, entry: ResolvedIndyEntry) -> usize {
        let index = self.indy_entries.len();
        self.indy_entries.push(entry);
        index
    }

    pub fn find_field_entry_by_cpool_index(&self, cpool_index: u16) -> Option<usize> {
        self.field_entries
            .iter()
            .position(|entry| entry.cpool_index() == cpool_index)
    }

    pub fn find_method_entry_by_cpool_index(&self, cpool_index: u16) -> Option<usize> {
        self.method_entries
            .iter()
            .position(|entry| entry.cpool_index() == cpool_index)
    }

    pub fn find_indy_entry_by_cpool_index(&self, cpool_index: u16) -> Option<usize> {
        self.indy_entries
            .iter()
            .position(|entry| entry.cpool_index() == cpool_index)
    }

    pub fn clear(&mut self) {
        self.field_entries.clear();
        self.method_entries.clear();
        self.indy_entries.clear();
    }
}
