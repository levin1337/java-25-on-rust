use super::z_address::{ZAddress, ZOffset, address_to_offset};
use super::z_granule_map::ZGranuleMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZPageTableEntry {
    pub page_index: usize,
    pub generation: u8,
}

pub struct ZPageTable {
    map: ZGranuleMap<Option<ZPageTableEntry>>,
}

impl ZPageTable {
    pub fn new(max_offset: ZOffset) -> Self {
        Self { map: ZGranuleMap::new(max_offset) }
    }

    pub fn insert(&mut self, page_index: usize, generation: u8, start: ZOffset, size: usize) {
        self.map.put_range(start, size, Some(ZPageTableEntry { page_index, generation }));
    }

    pub fn remove(&mut self, start: ZOffset, size: usize) {
        self.map.put_range(start, size, None);
    }

    pub fn get_offset(&self, offset: ZOffset) -> Option<ZPageTableEntry> {
        self.map.get(offset)
    }

    pub fn get_address(&self, address: ZAddress) -> Option<ZPageTableEntry> {
        self.get_offset(address_to_offset(address))
    }

    pub fn contains(&self, address: ZAddress) -> bool {
        self.get_address(address).is_some()
    }

    pub fn page_start_for_address(&self, address: ZAddress) -> Option<ZOffset> {
        let entry = self.get_address(address)?;
        let _ = entry;
        Some(address_to_offset(address) & !(super::z_address::Z_GRANULE_SIZE - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::gc::z::z_address::offset_to_address;

    #[test]
    fn page_table_lookup() {
        let mut table = ZPageTable::new(1 << 32);
        table.insert(7, 0, super::super::z_address::Z_GRANULE_SIZE, super::super::z_address::Z_GRANULE_SIZE as usize);
        let address = offset_to_address(super::super::z_address::Z_GRANULE_SIZE + 0x100);
        assert_eq!(table.get_address(address).unwrap().page_index, 7);
    }
}
