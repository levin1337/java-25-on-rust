use super::z_address::{ZAddress, ZOffset, offset_to_address, address_to_offset};
use super::z_generation_id::{ZPageAge, ZPageType};
use super::z_granule_map::ZGranuleMap;
use super::z_page::ZPage;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZForwardingEntry(u64);

impl ZForwardingEntry {
    const POPULATED: u64 = 1;
    const TO_OFFSET_SHIFT: u32 = 1;
    const TO_OFFSET_BITS: u32 = 45;
    const FROM_INDEX_SHIFT: u32 = 46;
    const FROM_INDEX_BITS: u32 = 18;
    const TO_OFFSET_MASK: u64 = ((1u64 << Self::TO_OFFSET_BITS) - 1) << Self::TO_OFFSET_SHIFT;
    const FROM_INDEX_MASK: u64 = ((1u64 << Self::FROM_INDEX_BITS) - 1) << Self::FROM_INDEX_SHIFT;

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn new(from_index: usize, to_offset: ZOffset) -> Self {
        assert!(from_index < (1usize << Self::FROM_INDEX_BITS));
        assert!(to_offset < (1u64 << Self::TO_OFFSET_BITS));
        Self(
            Self::POPULATED
                | ((to_offset << Self::TO_OFFSET_SHIFT) & Self::TO_OFFSET_MASK)
                | (((from_index as u64) << Self::FROM_INDEX_SHIFT) & Self::FROM_INDEX_MASK),
        )
    }

    pub fn populated(self) -> bool {
        (self.0 & Self::POPULATED) != 0
    }

    pub fn to_offset(self) -> ZOffset {
        (self.0 & Self::TO_OFFSET_MASK) >> Self::TO_OFFSET_SHIFT
    }

    pub fn from_index(self) -> usize {
        ((self.0 & Self::FROM_INDEX_MASK) >> Self::FROM_INDEX_SHIFT) as usize
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

fn uint32_to_uint32(mut key: u32) -> u32 {
    key = !key.wrapping_add(key << 15);
    key ^= key >> 12;
    key = key.wrapping_add(key << 2);
    key ^= key >> 4;
    key = key.wrapping_mul(2057);
    key ^= key >> 16;
    key
}

fn hash_from_index(from_index: usize) -> usize {
    uint32_to_uint32(from_index as u32) as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZPublishState {
    None,
    Published,
    Reject,
    Accept,
}

pub struct ZForwarding {
    start: ZOffset,
    end: ZOffset,
    object_alignment_shift: u32,
    entries: Vec<AtomicU64>,
    page_index: usize,
    from_age: ZPageAge,
    to_age: ZPageAge,
    claimed: AtomicBool,
    ref_count: AtomicI32,
    done: AtomicBool,
    in_place: AtomicBool,
    in_place_top_at_start: AtomicU64,
    relocated_remembered_fields_state: AtomicU8,
    relocated_remembered_fields: Mutex<Vec<usize>>,
    relocated_remembered_fields_publish_young_seqnum: AtomicU32,
}

use std::sync::atomic::AtomicU32;

impl ZForwarding {
    pub fn nentries(live_objects: u32) -> usize {
        (live_objects as usize * 2).max(1).next_power_of_two()
    }

    pub fn new(page: &ZPage, to_age: ZPageAge, page_index: usize) -> Self {
        let nentries = Self::nentries(page.live_objects().max(1));
        Self {
            start: page.start(),
            end: page.end(),
            object_alignment_shift: page.object_alignment_shift(),
            entries: (0..nentries).map(|_| AtomicU64::new(0)).collect(),
            page_index,
            from_age: page.age(),
            to_age,
            claimed: AtomicBool::new(false),
            ref_count: AtomicI32::new(1),
            done: AtomicBool::new(false),
            in_place: AtomicBool::new(false),
            in_place_top_at_start: AtomicU64::new(0),
            relocated_remembered_fields_state: AtomicU8::new(ZPublishState::None as u8),
            relocated_remembered_fields: Mutex::new(Vec::new()),
            relocated_remembered_fields_publish_young_seqnum: AtomicU32::new(0),
        }
    }

    pub fn for_range(
        start: ZOffset,
        end: ZOffset,
        object_alignment_shift: u32,
        live_objects: u32,
        page_index: usize,
        from_age: ZPageAge,
        to_age: ZPageAge,
    ) -> Self {
        let nentries = Self::nentries(live_objects.max(1));
        Self {
            start,
            end,
            object_alignment_shift,
            entries: (0..nentries).map(|_| AtomicU64::new(0)).collect(),
            page_index,
            from_age,
            to_age,
            claimed: AtomicBool::new(false),
            ref_count: AtomicI32::new(1),
            done: AtomicBool::new(false),
            in_place: AtomicBool::new(false),
            in_place_top_at_start: AtomicU64::new(0),
            relocated_remembered_fields_state: AtomicU8::new(ZPublishState::None as u8),
            relocated_remembered_fields: Mutex::new(Vec::new()),
            relocated_remembered_fields_publish_young_seqnum: AtomicU32::new(0),
        }
    }

    pub fn page_type(&self) -> ZPageType {
        if self.end - self.start == super::z_page::Z_PAGE_SIZE_SMALL as u64 {
            ZPageType::Small
        } else if self.end - self.start <= super::z_page::Z_PAGE_SIZE_MEDIUM_MAX as u64 {
            ZPageType::Medium
        } else {
            ZPageType::Large
        }
    }

    pub fn from_age(&self) -> ZPageAge {
        self.from_age
    }

    pub fn to_age(&self) -> ZPageAge {
        self.to_age
    }

    pub fn start(&self) -> ZOffset {
        self.start
    }

    pub fn end(&self) -> ZOffset {
        self.end
    }

    pub fn size(&self) -> usize {
        (self.end - self.start) as usize
    }

    pub fn object_alignment_shift(&self) -> u32 {
        self.object_alignment_shift
    }

    pub fn page_index(&self) -> usize {
        self.page_index
    }

    pub fn is_promotion(&self) -> bool {
        !self.from_age.is_old() && self.to_age.is_old()
    }

    fn index(&self, from_offset: ZOffset) -> usize {
        ((from_offset - self.start) >> self.object_alignment_shift) as usize
    }

    fn entry_at(&self, cursor: usize) -> ZForwardingEntry {
        ZForwardingEntry::from_raw(self.entries[cursor].load(Ordering::Acquire))
    }

    fn first(&self, from_index: usize) -> (usize, ZForwardingEntry) {
        let cursor = hash_from_index(from_index) & (self.entries.len() - 1);
        (cursor, ZForwardingEntry::from_raw(self.entries[cursor].load(Ordering::Acquire)))
    }

    fn next_cursor(&self, cursor: usize) -> usize {
        (cursor + 1) & (self.entries.len() - 1)
    }

    pub fn find_entry(&self, from_index: usize) -> Option<ZForwardingEntry> {
        let (mut cursor, mut entry) = self.first(from_index);
        loop {
            if !entry.populated() {
                return None;
            }
            if entry.from_index() == from_index {
                return Some(entry);
            }
            cursor = self.next_cursor(cursor);
            entry = self.entry_at(cursor);
        }
    }

    pub fn find(&self, from_offset: ZOffset) -> Option<ZAddress> {
        let from_index = self.index(from_offset);
        self.find_entry(from_index)
            .map(|entry| offset_to_address(entry.to_offset()))
    }

    pub fn find_address(&self, from_address: ZAddress) -> Option<ZAddress> {
        self.find(address_to_offset(from_address))
    }

    pub fn insert_offset(&self, from_offset: ZOffset, to_offset: ZOffset) -> ZOffset {
        let from_index = self.index(from_offset);
        let new_entry = ZForwardingEntry::new(from_index, to_offset);
        let (mut cursor, mut entry) = self.first(from_index);
        loop {
            if !entry.populated() {
                match self.entries[cursor].compare_exchange(
                    0,
                    new_entry.raw(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return to_offset,
                    Err(_) => {}
                }
            }
            if entry.populated() && entry.from_index() == from_index {
                return entry.to_offset();
            }
            cursor = self.next_cursor(cursor);
            entry = ZForwardingEntry::from_raw(self.entries[cursor].load(Ordering::Acquire));
        }
    }

    pub fn insert(&self, from_offset: ZOffset, to_address: ZAddress) -> ZAddress {
        offset_to_address(self.insert_offset(from_offset, address_to_offset(to_address)))
    }

    pub fn insert_address(&self, from_address: ZAddress, to_address: ZAddress) -> ZAddress {
        self.insert(address_to_offset(from_address), to_address)
    }

    pub fn iter_entries(&self) -> impl Iterator<Item = ZForwardingEntry> + '_ {
        self.entries.iter().map(|entry| ZForwardingEntry::from_raw(entry.load(Ordering::Acquire))).filter(|entry| entry.populated())
    }

    pub fn claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::AcqRel)
    }

    pub fn retain_page(&self) -> bool {
        loop {
            let count = self.ref_count.load(Ordering::Acquire);
            if count <= 0 {
                return false;
            }
            if self
                .ref_count
                .compare_exchange(count, count + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub fn release_page(&self) {
        loop {
            let count = self.ref_count.load(Ordering::Acquire);
            assert_ne!(count, 0);
            let next = if count > 0 { count - 1 } else { count + 1 };
            if self
                .ref_count
                .compare_exchange(count, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }

    pub fn claim_page(&self) {
        loop {
            let count = self.ref_count.load(Ordering::Acquire);
            assert!(count > 0);
            if self
                .ref_count
                .compare_exchange(count, -count, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if count == 1 {
                    return;
                }
                while self.ref_count.load(Ordering::Acquire) != -1 {
                    std::thread::yield_now();
                }
                return;
            }
        }
    }

    pub fn ref_count(&self) -> i32 {
        self.ref_count.load(Ordering::Acquire)
    }

    pub fn mark_done(&self) {
        self.done.store(true, Ordering::Release);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn in_place_relocation(&self) -> bool {
        self.in_place.load(Ordering::Acquire)
    }

    pub fn in_place_relocation_start(&self, relocated_watermark: ZOffset) {
        self.in_place_top_at_start.store(relocated_watermark, Ordering::Release);
        self.in_place.store(true, Ordering::Release);
    }

    pub fn in_place_relocation_finish(&self) {
        self.in_place.store(false, Ordering::Release);
    }

    pub fn relocated_remembered_fields_register(&self, field_address: usize) {
        self.relocated_remembered_fields.lock().unwrap().push(field_address);
    }

    pub fn relocated_remembered_fields_publish(&self) {
        let _ = self.relocated_remembered_fields_state.compare_exchange(
            ZPublishState::None as u8,
            ZPublishState::Published as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub fn relocated_remembered_fields_notify_concurrent_scan(&self) {
        let _ = self.relocated_remembered_fields_state.compare_exchange(
            ZPublishState::None as u8,
            ZPublishState::Reject as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub fn relocated_remembered_fields_state(&self) -> ZPublishState {
        match self.relocated_remembered_fields_state.load(Ordering::Acquire) {
            1 => ZPublishState::Published,
            2 => ZPublishState::Reject,
            3 => ZPublishState::Accept,
            _ => ZPublishState::None,
        }
    }

    pub fn relocated_remembered_fields_publish_young_seqnum(&self) -> u32 {
        self.relocated_remembered_fields_publish_young_seqnum.load(Ordering::Acquire)
    }

    pub fn relocated_remembered_fields_apply<F>(&self, mut function: F)
    where
        F: FnMut(usize),
    {
        for field in self.relocated_remembered_fields.lock().unwrap().iter().copied() {
            function(field);
        }
    }

    pub fn relocated_remembered_fields_published_contains(&self, field_address: usize) -> bool {
        self.relocated_remembered_fields.lock().unwrap().contains(&field_address)
    }

    pub fn verify(&self) -> Result<(), &'static str> {
        if self.ref_count() == 0 {
            return Err("invalid reference count");
        }
        let mut from_indexes = std::collections::HashSet::new();
        let mut to_offsets = std::collections::HashSet::new();
        for entry in self.iter_entries() {
            if !from_indexes.insert(entry.from_index()) || !to_offsets.insert(entry.to_offset()) {
                return Err("duplicate forwarding entry");
            }
            if entry.to_offset() < self.start || entry.to_offset() >= self.end {
                return Err("to offset outside page");
            }
        }
        Ok(())
    }
}

pub struct ZForwardingTable {
    map: ZGranuleMap<Option<usize>>,
}

impl ZForwardingTable {
    pub fn new(max_offset: ZOffset) -> Self {
        Self { map: ZGranuleMap::new(max_offset) }
    }

    pub fn get_address(&self, address: ZAddress) -> Option<usize> {
        self.map.get(address_to_offset(address))
    }

    pub fn get_offset(&self, offset: ZOffset) -> Option<usize> {
        self.map.get(offset)
    }

    pub fn insert(&mut self, forwarding_index: usize, start: ZOffset, size: usize) {
        self.map.put_range(start, size, Some(forwarding_index));
    }

    pub fn remove(&mut self, start: ZOffset, size: usize) {
        self.map.put_range(start, size, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::gc::z::z_generation_id::ZPageAge;

    #[test]
    fn forwarding_entry_packing() {
        let entry = ZForwardingEntry::new(0x12345, 0x23456789);
        assert!(entry.populated());
        assert_eq!(entry.from_index(), 0x12345);
        assert_eq!(entry.to_offset(), 0x23456789);
        assert_eq!(ZForwardingEntry::empty().populated(), false);
    }

    #[test]
    fn forwarding_insert_find() {
        let forwarding = ZForwarding::for_range(0x200000, 0x400000, 3, 8, 0, ZPageAge::Eden, ZPageAge::Survivor1);
        let to = forwarding.insert_offset(0x201000, 0x300000);
        assert_eq!(to, 0x300000);
        assert_eq!(forwarding.find(0x201000), Some(offset_to_address(0x300000)));
        assert_eq!(forwarding.verify(), Ok(()));
    }
}
