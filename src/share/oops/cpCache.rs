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
