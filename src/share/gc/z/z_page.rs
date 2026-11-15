use super::z_generation_id::{ZGenerationId, ZPageAge, ZPageType};
use super::z_live_map::ZLiveMap;
use super::z_remembered_set::ZRememberedSet;
use super::z_address::{ZAddress, ZOffset, offset_to_address, address_to_offset};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

pub const Z_PAGE_SIZE_SMALL: usize = 2 * 1024 * 1024;
pub const Z_PAGE_SIZE_MEDIUM_MIN: usize = 4 * 1024 * 1024;
pub const Z_PAGE_SIZE_MEDIUM_MAX: usize = 32 * 1024 * 1024;
pub const Z_OBJECT_ALIGNMENT_SMALL_SHIFT: u32 = 3;
pub const Z_OBJECT_ALIGNMENT_SMALL: usize = 1 << Z_OBJECT_ALIGNMENT_SMALL_SHIFT;
pub const Z_OBJECT_ALIGNMENT_MEDIUM_SHIFT: u32 = 4;
pub const Z_OBJECT_ALIGNMENT_MEDIUM: usize = 1 << Z_OBJECT_ALIGNMENT_MEDIUM_SHIFT;
pub const Z_OBJECT_ALIGNMENT_LARGE_SHIFT: u32 = super::z_address::Z_GRANULE_SIZE_SHIFT;
pub const Z_OBJECT_ALIGNMENT_LARGE: usize = 1 << Z_OBJECT_ALIGNMENT_LARGE_SHIFT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZAllocatedObject {
    pub offset: ZOffset,
    pub size: usize,
}

pub struct ZPage {
    page_type: ZPageType,
    generation_id: ZGenerationId,
    age: ZPageAge,
    seqnum: AtomicU32,
    seqnum_other: AtomicU32,
    single_partition_id: u32,
    start: ZOffset,
    size: usize,
    top: AtomicU64,
    livemap: ZLiveMap,
    remembered_set: ZRememberedSet,
    allocated_objects: Mutex<Vec<ZAllocatedObject>>,
}

impl ZPage {
    pub fn new(page_type: ZPageType, age: ZPageAge, start: ZOffset, size: usize, partition_id: u32) -> Self {
        assert!(size > 0);
        let object_max_count = Self::object_max_count_for(page_type, size);
        let mut page = Self {
            page_type,
            generation_id: age.generation_id(),
            age,
            seqnum: AtomicU32::new(0),
            seqnum_other: AtomicU32::new(0),
            single_partition_id: partition_id,
            start,
            size,
            top: AtomicU64::new(start),
            livemap: ZLiveMap::new(object_max_count),
            remembered_set: ZRememberedSet::new(),
            allocated_objects: Mutex::new(Vec::new()),
        };
        page.reset(age);
        page
    }

    pub fn new_small(start: ZOffset) -> Self {
        Self::new(ZPageType::Small, ZPageAge::Eden, start, Z_PAGE_SIZE_SMALL, u32::MAX)
    }

    pub fn new_medium(start: ZOffset) -> Self {
        Self::new(ZPageType::Medium, ZPageAge::Eden, start, Z_PAGE_SIZE_MEDIUM_MIN, u32::MAX)
    }

    pub fn new_large(start: ZOffset, size: usize) -> Self {
        Self::new(ZPageType::Large, ZPageAge::Eden, start, size, u32::MAX)
    }

    pub fn clone_for_promotion(&self) -> Self {
        assert!(!self.age.is_old());
        let mut page = Self::new(self.page_type, ZPageAge::Old, self.start, self.size, self.single_partition_id);
        page.top.store(self.top(), Ordering::Release);
        page
    }

    fn object_max_count_for(page_type: ZPageType, size: usize) -> u32 {
        if page_type == ZPageType::Large {
            1
        } else {
            (size >> Self::object_alignment_shift_for(page_type)) as u32
        }
    }

    pub fn object_max_count(&self) -> u32 {
        Self::object_max_count_for(self.page_type, self.size)
    }

    pub fn object_alignment_shift(&self) -> u32 {
        Self::object_alignment_shift_for(self.page_type)
    }

    fn object_alignment_shift_for(page_type: ZPageType) -> u32 {
        match page_type {
            ZPageType::Small => Z_OBJECT_ALIGNMENT_SMALL_SHIFT,
            ZPageType::Medium => Z_OBJECT_ALIGNMENT_MEDIUM_SHIFT,
            ZPageType::Large => Z_OBJECT_ALIGNMENT_LARGE_SHIFT,
        }
    }

    pub fn object_alignment(&self) -> usize {
        1usize << self.object_alignment_shift()
    }

    pub fn page_type(&self) -> ZPageType {
        self.page_type
    }

    pub fn is_small(&self) -> bool {
        self.page_type == ZPageType::Small
    }

    pub fn is_medium(&self) -> bool {
        self.page_type == ZPageType::Medium
    }

    pub fn is_large(&self) -> bool {
        self.page_type == ZPageType::Large
    }

    pub fn generation_id(&self) -> ZGenerationId {
        self.generation_id
    }

    pub fn is_young(&self) -> bool {
        self.generation_id == ZGenerationId::Young
    }

    pub fn is_old(&self) -> bool {
        self.generation_id == ZGenerationId::Old
    }

    pub fn start(&self) -> ZOffset {
        self.start
    }

    pub fn end(&self) -> ZOffset {
        self.start + self.size as u64
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn top(&self) -> ZOffset {
        self.top.load(Ordering::Acquire)
    }

    pub fn remaining(&self) -> usize {
        (self.end() - self.top()) as usize
    }

    pub fn used(&self) -> usize {
        (self.top() - self.start) as usize
    }

    pub fn age(&self) -> ZPageAge {
        self.age
    }

    pub fn seqnum(&self) -> u32 {
        self.seqnum.load(Ordering::Acquire)
    }

    pub fn seqnum_other(&self) -> u32 {
        self.seqnum_other.load(Ordering::Acquire)
    }

    pub fn set_seqnums(&self, seqnum: u32, seqnum_other: u32) {
        self.seqnum_other.store(seqnum_other, Ordering::Relaxed);
        self.seqnum.store(seqnum, Ordering::Release);
    }

    pub fn single_partition_id(&self) -> u32 {
        self.single_partition_id
    }

    pub fn reset(&mut self, age: ZPageAge) -> &mut Self {
        self.age = age;
        self.generation_id = age.generation_id();
        self.set_seqnums(0, 0);
        if self.is_old() && !self.remembered_set.is_initialized() {
            self.remembered_set.initialize(self.size);
        }
        self
    }

    pub fn reset_livemap(&self) {
        self.livemap.reset();
    }

    pub fn reset_top_for_allocation(&self) {
        self.top.store(self.start, Ordering::Release);
        self.allocated_objects.lock().unwrap().clear();
    }

    pub fn is_in_offset(&self, offset: ZOffset) -> bool {
        offset >= self.start && offset < self.top()
    }

    pub fn is_in_address(&self, address: ZAddress) -> bool {
        self.is_in_offset(address_to_offset(address))
    }

    pub fn local_offset(&self, offset: ZOffset) -> usize {
        assert!(offset >= self.start && offset < self.end());
        (offset - self.start) as usize
    }

    pub fn local_offset_address(&self, address: ZAddress) -> usize {
        self.local_offset(address_to_offset(address))
    }

    pub fn global_offset(&self, local_offset: usize) -> ZOffset {
        self.start + local_offset as u64
    }

    pub fn is_allocating(&self, generation_seqnum: u32) -> bool {
        self.seqnum() == generation_seqnum
    }

    pub fn is_relocatable(&self, generation_seqnum: u32) -> bool {
        self.seqnum() < generation_seqnum
    }

    pub fn bit_index(&self, address: ZAddress) -> usize {
        (self.local_offset_address(address) >> self.object_alignment_shift()) * 2
    }

    pub fn offset_from_bit_index(&self, index: usize) -> ZOffset {
        self.start + (((index / 2) as u64) << self.object_alignment_shift())
    }

    pub fn address_from_bit_index(&self, index: usize) -> ZAddress {
        offset_to_address(self.offset_from_bit_index(index))
    }

    pub fn is_object_marked(&self, address: ZAddress, generation_seqnum: u32, finalizable: bool) -> bool {
        let index = self.bit_index(address);
        if finalizable {
            self.livemap.get(generation_seqnum, index)
        } else {
            self.livemap.get(generation_seqnum, index + 1)
        }
    }

    pub fn mark_object(&self, address: ZAddress, generation_seqnum: u32, finalizable: bool) -> (bool, bool) {
        let index = self.bit_index(address);
        let (changed, inc_live) = self.livemap.set(generation_seqnum, index, finalizable);
        if inc_live {
            let size = self
                .allocated_objects()
                .into_iter()
                .find(|object| offset_to_address(object.offset) == address)
                .map(|object| object.size)
                .unwrap_or(self.object_alignment());
            self.livemap.inc_live(1, size);
        }
        (changed, inc_live)
    }

    pub fn live_objects(&self) -> u32 {
        self.livemap.live_objects()
    }

    pub fn live_bytes(&self) -> usize {
        self.livemap.live_bytes()
    }

    pub fn allocate(&self, size: usize) -> Option<ZAddress> {
        let alignment = self.object_alignment();
        let aligned_size = size.next_multiple_of(alignment);
        loop {
            let top = self.top();
            let new_top = top + aligned_size as u64;
            if new_top > self.end() {
                return None;
            }
            if self.top.compare_exchange(top, new_top, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                self.allocated_objects.lock().unwrap().push(ZAllocatedObject { offset: top, size: aligned_size });
                return Some(offset_to_address(top));
            }
        }
    }

    pub fn undo_allocate(&self, address: ZAddress, size: usize) -> bool {
        let offset = address_to_offset(address);
        let mut objects = self.allocated_objects.lock().unwrap();
        if objects.last().is_some_and(|object| object.offset == offset && object.size == size) {
            objects.pop();
            self.top.store(offset, Ordering::Release);
            true
        } else {
            false
        }
    }

    pub fn allocated_objects(&self) -> Vec<ZAllocatedObject> {
        self.allocated_objects.lock().unwrap().clone()
    }

    pub fn object_iterate<F>(&self, mut function: F)
    where
        F: FnMut(ZAddress, usize),
    {
        for object in self.allocated_objects() {
            function(offset_to_address(object.offset), object.size);
        }
    }

    pub fn live_object_iterate<F>(&self, generation_seqnum: u32, mut function: F)
    where
        F: FnMut(ZAddress, usize) -> bool,
    {
        self.livemap.iterate(generation_seqnum, |index| {
            let offset = self.offset_from_bit_index(index);
            let local = self.local_offset(offset);
            let object = self
                .allocated_objects()
                .into_iter()
                .find(|object| object.offset == offset)
                .unwrap_or(ZAllocatedObject { offset, size: 0 });
            function(offset_to_address(offset), object.size.max(self.object_alignment())) && local < self.size()
        });
    }

    pub fn remember(&self, field_address: ZAddress) {
        if self.is_old() {
            let local = self.local_offset_address(field_address);
            self.remembered_set.set_current(local);
        }
    }

    pub fn is_remembered(&self, field_address: ZAddress) -> bool {
        if !self.is_old() {
            return false;
        }
        let local = self.local_offset_address(field_address);
        self.remembered_set.at_current(local) || self.remembered_set.at_previous(local)
    }

    pub fn clear_remset_bit_current(&self, local_offset: usize) {
        self.remembered_set.unset_current(local_offset);
    }

    pub fn clear_remset_range_current(&self, local_offset: usize, size: usize) {
        self.remembered_set.unset_range_current(local_offset, size);
    }

    pub fn swap_remset_bitmaps(&self) {
        self.remembered_set.swap_remset_bitmaps();
    }

    pub fn clear_remset_previous(&self) {
        self.remembered_set.clear_previous();
    }

    pub fn is_remset_cleared_current(&self) -> bool {
        !self.is_old() || self.remembered_set.is_cleared_current()
    }

    pub fn is_remset_cleared_previous(&self) -> bool {
        !self.is_old() || self.remembered_set.is_cleared_previous()
    }

    pub fn iterate_remembered_current<F>(&self, mut function: F)
    where
        F: FnMut(ZAddress),
    {
        if self.is_old() {
            let start = self.start;
            self.remembered_set.iterate_current(|local| function(offset_to_address(start + local as u64)));
        }
    }

    pub fn iterate_remembered_previous<F>(&self, mut function: F)
    where
        F: FnMut(ZAddress),
    {
        if self.is_old() {
            let start = self.start;
            self.remembered_set.iterate_previous(|local| function(offset_to_address(start + local as u64)));
        }
    }

    pub fn find_base_bit(&self, address: ZAddress) -> Option<ZAddress> {
        let index = self.bit_index(address);
        self.livemap.find_base_bit(index).map(|base| self.address_from_bit_index(base))
    }

    pub fn find_base(&self, field_address: ZAddress) -> Option<ZAddress> {
        self.find_base_bit(field_address)
    }
}

impl Clone for ZPage {
    fn clone(&self) -> Self {
        Self::new(self.page_type, self.age, self.start, self.size, self.single_partition_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_allocate_and_mark() {
        let page = ZPage::new_small(0x1000);
        let address = page.allocate(24).unwrap();
        assert!(page.is_in_address(address));
        assert_eq!(page.used(), page.object_alignment());
        let (changed, inc_live) = page.mark_object(address, 1, false);
        assert!(changed);
        assert!(inc_live);
        assert!(page.is_object_marked(address, 1, true));
        assert!(page.is_object_marked(address, 1, false));
    }
}
