use super::z_address::{ZAddress, ZOffset};
use super::z_forwarding::{ZForwarding, ZForwardingTable};
use super::z_generation_id::ZPageAge;
use super::z_page::{ZPage, Z_PAGE_SIZE_SMALL};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZRelocationSetSelectorStats {
    pub pages_candidates: usize,
    pub total_bytes: usize,
    pub live_bytes: usize,
    pub empty_bytes: usize,
    pub pages_selected: usize,
    pub relocate_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ZRelocationSetSelector {
    pub selected: Vec<usize>,
    pub stats: ZRelocationSetSelectorStats,
    pub forwarding_entries: usize,
}

impl ZRelocationSetSelector {
    pub fn select(
        pages: &[ZPage],
        generation_seqnum: u32,
        tenuring_threshold: u8,
        max_selected: usize,
        generation: Option<super::z_generation_id::ZGenerationId>,
    ) -> Self {
        let mut selected = Vec::new();
        let mut stats = ZRelocationSetSelectorStats::default();
        for (index, page) in pages.iter().enumerate() {
            if !page.is_relocatable(generation_seqnum) {
                continue;
            }
            if generation.is_some_and(|id| page.generation_id() != id) {
                continue;
            }
            stats.pages_candidates += 1;
            stats.total_bytes += page.size();
            stats.live_bytes += page.live_bytes();
            if page.live_bytes() == 0 {
                stats.empty_bytes += page.size();
                continue;
            }
            let density = page.live_bytes() as f64 / page.size() as f64;
            let promotes = page.age() as u8 >= tenuring_threshold;
            if density < 0.8 || promotes {
                selected.push(index);
                stats.pages_selected += 1;
                stats.relocate_bytes += page.live_bytes();
                if selected.len() >= max_selected {
                    break;
                }
            }
        }
        let forwarding_entries = selected
            .iter()
            .map(|index| ZForwarding::nentries(pages[*index].live_objects().max(1)))
            .sum();
        Self { selected, stats, forwarding_entries }
    }
}

pub struct ZRelocationSet {
    forwardings: Vec<ZForwarding>,
    forwarding_table: ZForwardingTable,
    flip_promoted_pages: Vec<usize>,
    in_place_relocate_promoted_pages: Vec<usize>,
}

impl ZRelocationSet {
    pub fn new(max_offset: ZOffset) -> Self {
        Self {
            forwardings: Vec::new(),
            forwarding_table: ZForwardingTable::new(max_offset),
            flip_promoted_pages: Vec::new(),
            in_place_relocate_promoted_pages: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.forwardings.is_empty()
    }

    pub fn len(&self) -> usize {
        self.forwardings.len()
    }

    pub fn forwardings(&self) -> &[ZForwarding] {
        &self.forwardings
    }

    pub fn forwardings_mut(&mut self) -> &mut [ZForwarding] {
        &mut self.forwardings
    }

    pub fn forwarding_table(&self) -> &ZForwardingTable {
        &self.forwarding_table
    }

    pub fn install(&mut self, pages: &[ZPage], selector: &ZRelocationSetSelector, tenuring_threshold: u8) {
        self.reset();
        for (forwarding_index, page_index) in selector.selected.iter().enumerate() {
            let page = &pages[*page_index];
            let to_age = compute_to_age(page.age(), tenuring_threshold);
            let forwarding = ZForwarding::new(page, to_age, *page_index);
            self.forwarding_table
                .insert(forwarding_index, page.start(), page.size());
            self.forwardings.push(forwarding);
        }
    }

    pub fn install_range(&mut self, selected: &[(usize, ZOffset, ZOffset, u32, ZPageAge, ZPageAge)]) {
        self.reset();
        for (forwarding_index, (page_index, start, end, live_objects, from_age, to_age)) in selected.iter().enumerate() {
            let shift = if (*end - *start) == Z_PAGE_SIZE_SMALL as u64 { 3 } else { 4 };
            let forwarding = ZForwarding::for_range(*start, *end, shift, *live_objects, *page_index, *from_age, *to_age);
            self.forwarding_table.insert(forwarding_index, *start, (*end - *start) as usize);
            self.forwardings.push(forwarding);
        }
    }

    pub fn reset(&mut self) {
        for forwarding in &self.forwardings {
            for entry in forwarding.iter_entries() {
                let _ = entry;
            }
        }
        let ranges: Vec<(ZOffset, usize)> = self
            .forwardings
            .iter()
            .map(|forwarding| (forwarding.start(), forwarding.size()))
            .collect();
        for (start, size) in ranges {
            self.forwarding_table.remove(start, size);
        }
        self.forwardings.clear();
        self.flip_promoted_pages.clear();
        self.in_place_relocate_promoted_pages.clear();
    }

    pub fn forwarding(&self, address: ZAddress) -> Option<&ZForwarding> {
        let index = self.forwarding_table.get_address(address)?;
        self.forwardings.get(index)
    }

    pub fn forwarding_offset(&self, offset: ZOffset) -> Option<&ZForwarding> {
        let index = self.forwarding_table.get_offset(offset)?;
        self.forwardings.get(index)
    }

    pub fn find_forwarded(&self, address: ZAddress) -> Option<ZAddress> {
        self.forwarding(address).and_then(|forwarding| forwarding.find_address(address))
    }

    pub fn relocate_or_remap(&self, address: ZAddress) -> ZAddress {
        self.find_forwarded(address).unwrap_or(address)
    }

    pub fn remap(&self, address: ZAddress) -> ZAddress {
        self.find_forwarded(address).unwrap_or(address)
    }

    pub fn insert_forwarding(&mut self, from_address: ZAddress, to_address: ZAddress) -> bool {
        let Some(index) = self.forwarding_table.get_address(from_address) else {
            return false;
        };
        let Some(forwarding) = self.forwardings.get(index) else {
            return false;
        };
        forwarding.insert_address(from_address, to_address);
        true
    }

    pub fn register_flip_promoted(&mut self, pages: &[usize]) {
        self.flip_promoted_pages.extend_from_slice(pages);
    }

    pub fn register_in_place_relocate_promoted(&mut self, page: usize) {
        self.in_place_relocate_promoted_pages.push(page);
    }

    pub fn flip_promoted_pages(&self) -> &[usize] {
        &self.flip_promoted_pages
    }

    pub fn in_place_relocate_promoted_pages(&self) -> &[usize] {
        &self.in_place_relocate_promoted_pages
    }

    pub fn compute_to_age(from_age: ZPageAge, tenuring_threshold: u8) -> ZPageAge {
        compute_to_age(from_age, tenuring_threshold)
    }
}

pub fn compute_to_age(from_age: ZPageAge, tenuring_threshold: u8) -> ZPageAge {
    if from_age.is_old() {
        return ZPageAge::Old;
    }
    if from_age as u8 >= tenuring_threshold {
        ZPageAge::Old
    } else {
        from_age.next().unwrap_or(ZPageAge::Old)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::gc::z::z_address::offset_to_address;

    #[test]
    fn selector_selects_sparse_pages() {
        let page = ZPage::new_small(0);
        let address = page.allocate(64).unwrap();
        page.mark_object(address, 1, false);
        page.live_objects();
        let selector = ZRelocationSetSelector::select(&[page], 2, 14, 4, Some(super::super::z_generation_id::ZGenerationId::Young));
        assert_eq!(selector.selected.len(), 1);
    }

    #[test]
    fn relocation_set_forwarding_lookup() {
        let mut set = ZRelocationSet::new(1 << 32);
        let selected = vec![(0usize, 0x200000u64, 0x400000u64, 4u32, ZPageAge::Eden, ZPageAge::Survivor1)];
        set.install_range(&selected);
        let from = offset_to_address(0x201000);
        let to = offset_to_address(0x300000);
        set.forwardings()[0].insert_address(from, to);
        assert_eq!(set.find_forwarded(from), Some(to));
        assert_eq!(set.relocate_or_remap(from), to);
    }
}
