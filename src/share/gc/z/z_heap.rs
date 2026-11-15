use super::z_address::{ZAddress, ZOffset, offset_to_address};
use super::z_generation::{ZGenerationOld, ZGenerationYoung};
use super::z_generation_id::{ZPageAge, ZPageType};
use super::z_page::{ZPage, Z_PAGE_SIZE_MEDIUM_MIN, Z_PAGE_SIZE_SMALL};
use super::z_page_table::ZPageTable;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct ZHeap {
    pages: Vec<ZPage>,
    page_table: ZPageTable,
    next_page_offset: ZOffset,
    max_offset: ZOffset,
    young: ZGenerationYoung,
    old: ZGenerationOld,
    used_bytes: AtomicU64,
    next_relocation_offset: AtomicU64,
}

impl ZHeap {
    pub fn new(max_capacity: usize, nworkers: usize) -> Self {
        let max_offset = (max_capacity as u64).max(1 << 32).next_power_of_two();
        Self {
            pages: Vec::new(),
            page_table: ZPageTable::new(max_offset),
            next_page_offset: 0,
            max_offset,
            young: ZGenerationYoung::new(nworkers, max_offset, 10),
            old: ZGenerationOld::new(nworkers, max_offset),
            used_bytes: AtomicU64::new(0),
            next_relocation_offset: AtomicU64::new((max_offset / 2).max(super::z_address::Z_GRANULE_SIZE)),
        }
    }

    pub fn young(&self) -> &ZGenerationYoung {
        &self.young
    }

    pub fn old(&self) -> &ZGenerationOld {
        &self.old
    }

    pub fn page_table(&self) -> &ZPageTable {
        &self.page_table
    }

    pub fn pages(&self) -> &[ZPage] {
        &self.pages
    }

    pub fn page(&self, index: usize) -> Option<&ZPage> {
        self.pages.get(index)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn used(&self) -> usize {
        self.used_bytes.load(Ordering::Acquire) as usize
    }

    pub fn max_offset(&self) -> ZOffset {
        self.max_offset
    }

    pub fn allocate(&mut self, size: usize) -> Option<ZAddress> {
        let page_type = Self::page_type_for(size);
        for page in &self.pages {
            if page.page_type() == page_type && page.is_young() && !page.is_relocatable(self.young.generation().seqnum()) {
                if let Some(address) = page.allocate(size) {
                    self.used_bytes.fetch_add(size as u64, Ordering::Relaxed);
                    return Some(address);
                }
            }
        }
        self.allocate_new_page(page_type, ZPageAge::Eden, size)
    }

    pub fn allocate_old(&mut self, size: usize) -> Option<ZAddress> {
        let page_type = Self::page_type_for(size);
        for page in &self.pages {
            if page.page_type() == page_type && page.is_old() && !page.is_relocatable(self.old.generation().seqnum()) {
                if let Some(address) = page.allocate(size) {
                    self.used_bytes.fetch_add(size as u64, Ordering::Relaxed);
                    return Some(address);
                }
            }
        }
        self.allocate_new_page(page_type, ZPageAge::Old, size)
    }

    fn page_type_for(size: usize) -> ZPageType {
        if size <= 256 * 1024 {
            ZPageType::Small
        } else if size <= Z_PAGE_SIZE_MEDIUM_MIN {
            ZPageType::Medium
        } else {
            ZPageType::Large
        }
    }

    fn page_size_for(page_type: ZPageType, size: usize) -> usize {
        match page_type {
            ZPageType::Small => Z_PAGE_SIZE_SMALL,
            ZPageType::Medium => Z_PAGE_SIZE_MEDIUM_MIN,
            ZPageType::Large => size.next_multiple_of(super::z_address::Z_GRANULE_SIZE as usize),
        }
    }

    fn allocate_new_page(&mut self, page_type: ZPageType, age: ZPageAge, size: usize) -> Option<ZAddress> {
        let page_size = Self::page_size_for(page_type, size);
        if self.next_page_offset + page_size as u64 > self.max_offset {
            return None;
        }
        let start = self.next_page_offset;
        self.next_page_offset += page_size as u64;
        let mut page = match page_type {
            ZPageType::Small => ZPage::new_small(start),
            ZPageType::Medium => ZPage::new(ZPageType::Medium, age, start, page_size, u32::MAX),
            ZPageType::Large => ZPage::new_large(start, page_size),
        };
        page.reset(age);
        let generation_seqnum = if page.is_old() { self.old.generation().seqnum() } else { self.young.generation().seqnum() };
        let seqnum_other = if page.is_old() { self.young.generation().seqnum() } else { self.old.generation().seqnum() };
        page.set_seqnums(generation_seqnum, seqnum_other);
        let index = self.pages.len();
        let address = page.allocate(size);
        self.page_table.insert(index, page.generation_id() as u8, start, page_size);
        self.pages.push(page);
        if address.is_some() {
            self.used_bytes.fetch_add(size as u64, Ordering::Relaxed);
        }
        address
    }

    pub fn find_page(&self, address: ZAddress) -> Option<usize> {
        self.page_table.get_address(address).map(|entry| entry.page_index)
    }

    pub fn find_page_offset(&self, offset: ZOffset) -> Option<usize> {
        self.page_table.get_offset(offset).map(|entry| entry.page_index)
    }

    pub fn is_in(&self, address: ZAddress) -> bool {
        self.find_page(address).is_some()
    }

    pub fn is_young(&self, address: ZAddress) -> bool {
        self.find_page(address).is_some_and(|index| self.pages[index].is_young())
    }

    pub fn is_old(&self, address: ZAddress) -> bool {
        self.find_page(address).is_some_and(|index| self.pages[index].is_old())
    }

    pub fn mark_address(&self, address: ZAddress, finalizable: bool) -> bool {
        let Some(index) = self.find_page(address) else { return false; };
        let page = &self.pages[index];
        let seqnum = if page.is_old() { self.old.generation().seqnum() } else { self.young.generation().seqnum() };
        page.mark_object(address, seqnum, finalizable).1
    }

    pub fn remember(&self, field_address: ZAddress) {
        if let Some(index) = self.find_page(field_address) {
            self.pages[index].remember(field_address);
            if self.pages[index].is_old() {
                self.young.register_remembered_page(index);
            }
        }
    }

    pub fn collect_young<I, F>(&mut self, roots: I, mut relocate: F) -> bool
    where
        I: IntoIterator<Item = ZAddress>,
        F: FnMut(ZAddress, usize) -> Option<ZAddress>,
    {
        let seqnum = self.young.generation().next_seqnum();
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::Mark);
        for address in roots {
            if address != 0 {
                self.mark_address(address, false);
            }
        }
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::MarkComplete);
        let selector = self.young.generation().select_relocation_set(&self.pages, self.young.tenuring_threshold(), usize::MAX);
        self.young.generation().install_relocation_set(&self.pages, &selector, self.young.tenuring_threshold());
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::Relocate);
        {
            let mut relocation_set = self.young.generation().relocation_set().lock().unwrap();
            let mut pending = Vec::new();
            for forwarding in relocation_set.forwardings_mut() {
                let page_index = forwarding.page_index();
                let page = &self.pages[page_index];
                let mut live = Vec::new();
                page.live_object_iterate(seqnum, |address, size| {
                    live.push((address, size));
                    true
                });
                for (from, size) in live {
                    if let Some(to) = relocate(from, size) {
                        pending.push((from, to));
                    }
                }
                forwarding.mark_done();
            }
            for (from, to) in pending {
                relocation_set.insert_forwarding(from, to);
            }
        }
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::Mark);
        true
    }

    pub fn collect_young_auto<I>(&mut self, roots: I) -> bool
    where
        I: IntoIterator<Item = ZAddress>,
    {
        let seqnum = self.young.generation().next_seqnum();
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::Mark);
        for address in roots {
            if address != 0 {
                self.mark_address(address, false);
            }
        }
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::MarkComplete);
        let selector = self.young.generation().select_relocation_set(&self.pages, self.young.tenuring_threshold(), usize::MAX);
        self.young.generation().install_relocation_set(&self.pages, &selector, self.young.tenuring_threshold());
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::Relocate);
        {
            let mut relocation_set = self.young.generation().relocation_set().lock().unwrap();
            let mut pending = Vec::new();
            for forwarding in relocation_set.forwardings_mut() {
                let page_index = forwarding.page_index();
                let page = &self.pages[page_index];
                let mut live = Vec::new();
                page.live_object_iterate(seqnum, |address, size| {
                    live.push((address, size));
                    true
                });
                for (from, size) in live {
                    let aligned = size.max(1).next_multiple_of(page.object_alignment());
                    let to_offset = self.next_relocation_offset.fetch_add(aligned as u64, Ordering::AcqRel);
                    let to = offset_to_address(to_offset);
                    pending.push((from, to));
                }
                forwarding.mark_done();
            }
            for (from, to) in pending {
                relocation_set.insert_forwarding(from, to);
            }
        }
        self.young.generation().set_phase(super::z_generation::ZGenerationPhase::Mark);
        true
    }

    pub fn collect_old<I>(&mut self, roots: I) -> bool
    where
        I: IntoIterator<Item = ZAddress>,
    {
        self.old.generation().next_seqnum();
        self.old.generation().set_phase(super::z_generation::ZGenerationPhase::Mark);
        for address in roots {
            if address != 0 {
                self.mark_address(address, false);
            }
        }
        for page_index in self.young.remembered_pages() {
            if let Some(page) = self.pages.get(page_index) {
                page.iterate_remembered_previous(|field| {
                    let _ = field;
                });
            }
        }
        self.old.generation().set_phase(super::z_generation::ZGenerationPhase::MarkComplete);
        let selector = self.old.generation().select_relocation_set(&self.pages, 0, usize::MAX);
        self.old.generation().install_relocation_set(&self.pages, &selector, 0);
        self.old.generation().set_phase(super::z_generation::ZGenerationPhase::Relocate);
        self.old.generation().set_phase(super::z_generation::ZGenerationPhase::Mark);
        true
    }

    pub fn relocate_or_remap_young(&self, address: ZAddress) -> ZAddress {
        self.young.generation().relocation_set().lock().unwrap().relocate_or_remap(address)
    }

    pub fn relocate_or_remap_old(&self, address: ZAddress) -> ZAddress {
        self.old.generation().relocation_set().lock().unwrap().relocate_or_remap(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::gc::z::z_address::{address_to_offset, offset_to_address};

    #[test]
    fn heap_allocates_and_finds_page() {
        let mut heap = ZHeap::new(64 * 1024 * 1024, 1);
        let address = heap.allocate(64).unwrap();
        assert!(heap.is_in(address));
        assert!(heap.is_young(address));
        assert!(heap.mark_address(address, false));
    }

    #[test]
    fn young_collection_installs_forwarding() {
        let mut heap = ZHeap::new(64 * 1024 * 1024, 1);
        let from = heap.allocate(64).unwrap();
        heap.collect_young(vec![from], |_from, _size| Some(offset_to_address(address_to_offset(_from) + 0x1000)));
        assert_eq!(heap.relocate_or_remap_young(from), offset_to_address(address_to_offset(from) + 0x1000));
    }
}
