/*
 * Port of OpenJDK 25 HotSpot klassVtable and klassItable (share/oops/klassVtable.hpp, klassVtable.cpp)
 *
 * klassVtable manages the virtual method table (vtable) for dynamic polymorphic
 * dispatch in Java. klassItable manages the interface table (itable) for invokeinterface dispatch.
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VtableEntry {
    holder_klass: String,
    method_name: String,
    signature: String,
    access_flags: u16,
    is_miranda: bool,
}

impl VtableEntry {
    pub fn new(
        holder_klass: String,
        method_name: String,
        signature: String,
        access_flags: u16,
    ) -> Self {
        Self {
            holder_klass,
            method_name,
            signature,
            access_flags,
            is_miranda: false,
        }
    }

    pub fn new_miranda(
        holder_klass: String,
        method_name: String,
        signature: String,
        access_flags: u16,
    ) -> Self {
        Self {
            holder_klass,
            method_name,
            signature,
            access_flags,
            is_miranda: true,
        }
    }

    #[inline]
    pub fn holder_klass(&self) -> &str {
        &self.holder_klass
    }

    #[inline]
    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    #[inline]
    pub fn signature(&self) -> &str {
        &self.signature
    }

    #[inline]
    pub fn access_flags(&self) -> u16 {
        self.access_flags
    }

    #[inline]
    pub fn is_miranda(&self) -> bool {
        self.is_miranda
    }

    #[inline]
    pub fn set_miranda(&mut self, miranda: bool) {
        self.is_miranda = miranda;
    }

    #[inline]
    pub fn is_abstract(&self) -> bool {
        (self.access_flags & 0x0400) != 0
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        (self.access_flags & 0x0010) != 0
    }

    #[inline]
    pub fn matches(&self, method_name: &str, signature: &str) -> bool {
        self.method_name == method_name && self.signature == signature
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KlassVtable {
    owner_klass: String,
    table: Vec<VtableEntry>,
}

impl KlassVtable {
    pub fn new(owner_klass: String) -> Self {
        Self {
            owner_klass,
            table: Vec::new(),
        }
    }

    pub fn from_super(owner_klass: String, super_vtable: &KlassVtable) -> Self {
        Self {
            owner_klass,
            table: super_vtable.table.clone(),
        }
    }

    #[inline]
    pub fn owner_klass(&self) -> &str {
        &self.owner_klass
    }

    #[inline]
    pub fn length(&self) -> usize {
        self.table.len()
    }

    /// HotSpot `klassVtable::vtable_length()`: one extra slot for the
    /// header/klass word preceding method entries.
    #[inline]
    pub fn vtable_length(&self) -> usize {
        self.table.len() + 1
    }

    /// HotSpot `klassVtable::is_valid_index`.
    #[inline]
    pub fn is_valid_index(&self, index: usize) -> bool {
        index < self.table.len()
    }

    /// HotSpot `klassVtable::method_at`: the entry at `index`.
    #[inline]
    pub fn method_at(&self, index: usize) -> Option<&VtableEntry> {
        self.table.get(index)
    }

    /// HotSpot `klassVtable::set_method_at`: replaces the entry at
    /// `index` (used when patching overrides during class linking).
    pub fn set_method_at(&mut self, index: usize, entry: VtableEntry) -> bool {
        if let Some(slot) = self.table.get_mut(index) {
            *slot = entry;
            true
        } else {
            false
        }
    }

    /// Replaces an existing entry in place without changing slot
    /// ordering; returns the slot index.
    pub fn replace_method(&mut self, method_name: &str, signature: &str, entry: VtableEntry) -> Option<usize> {
        let index = self.index_of_method(method_name, signature)?;
        self.table[index] = entry;
        Some(index)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    #[inline]
    pub fn entry_at(&self, index: usize) -> Option<&VtableEntry> {
        self.table.get(index)
    }

    #[inline]
    pub fn entry_at_mut(&mut self, index: usize) -> Option<&mut VtableEntry> {
        self.table.get_mut(index)
    }

    /// Iterates all vtable entries in slot order (HotSpot
    /// `klassVtable::methods` traversal used by JIT dispatch).
    pub fn entries(&self) -> impl Iterator<Item = &VtableEntry> {
        self.table.iter()
    }

    pub fn index_of_method(&self, method_name: &str, signature: &str) -> Option<usize> {
        self.table
            .iter()
            .position(|entry| entry.matches(method_name, signature))
    }

    pub fn index_of_miranda(&self, method_name: &str, signature: &str) -> Option<usize> {
        self.table
            .iter()
            .position(|entry| entry.is_miranda() && entry.matches(method_name, signature))
    }

    /// Adds or overrides a virtual method in the vtable.
    /// If an inherited entry matches (method_name, signature), it is overridden in-place.
    /// Otherwise, a new entry is appended to the end of the vtable.
    pub fn add_or_override_method(
        &mut self,
        holder_klass: String,
        method_name: String,
        signature: String,
        access_flags: u16,
    ) -> usize {
        if let Some(index) = self.index_of_method(&method_name, &signature) {
            self.table[index] =
                VtableEntry::new(holder_klass, method_name, signature, access_flags);
            index
        } else {
            let index = self.table.len();
            self.table.push(VtableEntry::new(
                holder_klass,
                method_name,
                signature,
                access_flags,
            ));
            index
        }
    }

    /// Adds a miranda method (an abstract interface method not implemented by the superclass).
    pub fn add_miranda_method(
        &mut self,
        holder_klass: String,
        method_name: String,
        signature: String,
        access_flags: u16,
    ) -> usize {
        let index = self.table.len();
        self.table.push(VtableEntry::new_miranda(
            holder_klass,
            method_name,
            signature,
            access_flags,
        ));
        index
    }

    /// Dumps representation of the vtable matching HotSpot klassVtable::dump_vtable.
    pub fn dump_vtable(&self) -> Vec<String> {
        self.table
            .iter()
            .enumerate()
            .map(|(i, e)| {
                format!(
                    "vtable[{}] = {}::{} {} (flags=0x{:04x}, miranda={})",
                    i,
                    e.holder_klass(),
                    e.method_name(),
                    e.signature(),
                    e.access_flags(),
                    e.is_miranda()
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItableOffsetEntry {
    pub interface_klass: String,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItableMethodEntry {
    interface_klass: String,
    method_name: String,
    signature: String,
    resolved_vtable_index: usize,
}

impl ItableMethodEntry {
    pub fn new(
        interface_klass: String,
        method_name: String,
        signature: String,
        resolved_vtable_index: usize,
    ) -> Self {
        Self {
            interface_klass,
            method_name,
            signature,
            resolved_vtable_index,
        }
    }

    #[inline]
    pub fn interface_klass(&self) -> &str {
        &self.interface_klass
    }

    #[inline]
    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    #[inline]
    pub fn signature(&self) -> &str {
        &self.signature
    }

    #[inline]
    pub fn resolved_vtable_index(&self) -> usize {
        self.resolved_vtable_index
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KlassItable {
    owner_klass: String,
    entries: Vec<ItableMethodEntry>,
}

impl KlassItable {
    pub fn new(owner_klass: String) -> Self {
        Self {
            owner_klass,
            entries: Vec::new(),
        }
    }

    #[inline]
    pub fn owner_klass(&self) -> &str {
        &self.owner_klass
    }

    #[inline]
    pub fn entries_count(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates all itable method entries in registration order.
    pub fn entries(&self) -> impl Iterator<Item = &ItableMethodEntry> {
        self.entries.iter()
    }

    pub fn add_entry(
        &mut self,
        interface_klass: String,
        method_name: String,
        signature: String,
        resolved_vtable_index: usize,
    ) {
        self.entries.push(ItableMethodEntry::new(
            interface_klass,
            method_name,
            signature,
            resolved_vtable_index,
        ));
    }

    pub fn resolve_interface_method(
        &self,
        interface_klass: &str,
        method_name: &str,
        signature: &str,
    ) -> Option<usize> {
        self.entries
            .iter()
            .find(|entry| {
                entry.interface_klass == interface_klass
                    && entry.method_name == method_name
                    && entry.signature == signature
            })
            .map(|entry| entry.resolved_vtable_index)
    }

    /// List of distinct interfaces registered in this itable.
    pub fn interfaces(&self) -> Vec<String> {
        let mut ifaces = Vec::new();
        for entry in &self.entries {
            if !ifaces.contains(&entry.interface_klass) {
                ifaces.push(entry.interface_klass.clone());
            }
        }
        ifaces
    }

    /// Computes offset table entries grouping methods by interface.
    pub fn compute_offset_table(&self) -> Vec<ItableOffsetEntry> {
        let mut offsets = Vec::new();
        let mut current_offset = 0;
        for iface in self.interfaces() {
            offsets.push(ItableOffsetEntry {
                interface_klass: iface.clone(),
                offset: current_offset,
            });
            let count = self
                .entries
                .iter()
                .filter(|e| e.interface_klass == iface)
                .count();
            current_offset += count;
        }
        offsets
    }

    /// Computes itable length in machine words (offset table + method entries).
    pub fn compute_itable_size_in_words(&self) -> usize {
        let iface_count = self.interfaces().len();
        // Each interface offset entry is 2 words (interface_klass ptr + offset)
        // Each method entry is 2 words (method ptr + code offset / vtable index)
        (iface_count * 2) + (self.entries.len() * 2)
    }

    /// Number of interface method entries registered.
    #[inline]
    pub fn method_count(&self) -> usize {
        self.entries.len()
    }

    /// HotSpot `klassItable::interface_offset`: the itable offset (in
    /// method-entry units) of `interface_klass`, i.e. where that
    /// interface's method block starts.
    pub fn interface_offset(&self, interface_klass: &str) -> Option<usize> {
        self.compute_offset_table()
            .into_iter()
            .find(|entry| entry.interface_klass == interface_klass)
            .map(|entry| entry.offset)
    }

    /// Whether `interface_klass` is registered in this itable.
    pub fn has_interface(&self, interface_klass: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.interface_klass == interface_klass)
    }

    /// Iterator over all method entries declared by `interface_klass`.
    pub fn entries_of_interface(
        &self,
        interface_klass: &str,
    ) -> impl Iterator<Item = &ItableMethodEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.interface_klass == interface_klass)
    }
}
