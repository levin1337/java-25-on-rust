use super::z_bitmap::{ZBitMap, ZReverseBitMapIterator};
use std::sync::atomic::{AtomicUsize, Ordering};

const OOP_SIZE: usize = 8;
static CURRENT: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
pub struct ZRememberedSet {
    bitmap: [Option<ZBitMap>; 2],
}

impl ZRememberedSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn flip() {
        CURRENT.fetch_xor(1, Ordering::AcqRel);
    }

    pub fn current_index() -> usize {
        CURRENT.load(Ordering::Acquire)
    }

    pub fn is_initialized(&self) -> bool {
        self.bitmap[0].is_some()
    }

    pub fn initialize(&mut self, page_size: usize) {
        assert!(!self.is_initialized());
        let bits = Self::to_bit_size(page_size);
        self.bitmap[0] = Some(ZBitMap::new(bits));
        self.bitmap[1] = Some(ZBitMap::new(bits));
    }

    fn bitmap(&self, index: usize) -> &ZBitMap {
        self.bitmap[index].as_ref().expect("remembered set not initialized")
    }

    pub fn current(&self) -> &ZBitMap {
        self.bitmap(Self::current_index())
    }

    pub fn previous(&self) -> &ZBitMap {
        self.bitmap(Self::current_index() ^ 1)
    }

    pub fn to_offset(index: usize) -> usize {
        index * OOP_SIZE
    }

    pub fn to_index(offset: usize) -> usize {
        offset / OOP_SIZE
    }

    pub fn to_bit_size(size: usize) -> usize {
        size / OOP_SIZE
    }

    pub fn at_current(&self, offset: usize) -> bool {
        self.current().at(Self::to_index(offset))
    }

    pub fn at_previous(&self, offset: usize) -> bool {
        self.previous().at(Self::to_index(offset))
    }

    pub fn set_current(&self, offset: usize) -> bool {
        self.current().set_bit(Self::to_index(offset))
    }

    pub fn unset_current(&self, offset: usize) {
        self.current().clear_bit(Self::to_index(offset));
    }

    pub fn unset_range_current(&self, offset: usize, size: usize) {
        let start = Self::to_index(offset);
        let end = Self::to_index(offset + size);
        self.current().clear_range(start, end);
    }

    pub fn is_cleared_current(&self) -> bool {
        self.current().is_empty()
    }

    pub fn is_cleared_previous(&self) -> bool {
        self.previous().is_empty()
    }

    pub fn clear_previous(&self) {
        self.previous().clear();
    }

    pub fn swap_remset_bitmaps(&self) {
        assert!(self.is_cleared_previous());
        let current = self.current();
        let previous = self.previous();
        current.iterate(|index| {
            previous.set_bit(index);
            true
        });
        current.clear();
    }

    pub fn iterate_current<F>(&self, mut function: F)
    where
        F: FnMut(usize),
    {
        self.current().iterate(|index| {
            function(Self::to_offset(index));
            true
        });
    }

    pub fn iterate_previous<F>(&self, mut function: F)
    where
        F: FnMut(usize),
    {
        self.previous().iterate(|index| {
            function(Self::to_offset(index));
            true
        });
    }

    pub fn iterator_reverse_previous(&self) -> ZReverseBitMapIterator<'_> {
        ZReverseBitMapIterator::new(self.previous())
    }

    pub fn iterator_limited_current<F>(&self, offset: usize, size: usize, mut function: F)
    where
        F: FnMut(usize) -> bool,
    {
        let start = Self::to_index(offset);
        let end = start + Self::to_bit_size(size);
        self.current().iterate_range(start, end, |index| function(Self::to_offset(index)));
    }

    pub fn iterator_limited_previous<F>(&self, offset: usize, size: usize, mut function: F)
    where
        F: FnMut(usize) -> bool,
    {
        let start = Self::to_index(offset);
        let end = start + Self::to_bit_size(size);
        self.previous().iterate_range(start, end, |index| function(Self::to_offset(index)));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZRememberedSetContaining {
    pub field_addr: usize,
    pub addr: usize,
}

pub struct ZRememberedSetContainingIterator<'a> {
    remset: &'a ZRememberedSet,
    iterator: ZReverseBitMapIterator<'a>,
    obj: Option<usize>,
    obj_iterator: Option<ZReverseBitMapIterator<'a>>,
    page_start: usize,
}

impl<'a> ZRememberedSetContainingIterator<'a> {
    pub fn new(remset: &'a ZRememberedSet, page_start: usize) -> Self {
        Self {
            remset,
            iterator: remset.iterator_reverse_previous(),
            obj: None,
            obj_iterator: None,
            page_start,
        }
    }

    fn to_addr(&self, index: usize) -> usize {
        self.page_start + ZRememberedSet::to_offset(index)
    }

    pub fn next(&mut self) -> Option<ZRememberedSetContaining> {
        if let Some(obj) = self.obj {
            if let Some(iterator) = self.obj_iterator.as_mut() {
                if let Some(index) = iterator.next() {
                    return Some(ZRememberedSetContaining {
                        field_addr: self.to_addr(index),
                        addr: obj,
                    });
                }
            }
            self.obj = None;
            self.obj_iterator = None;
        }

        let index = self.iterator.next()?;
        let field_addr = self.to_addr(index);
        let addr = self.page_start + ZRememberedSet::to_offset(index.saturating_sub(1));
        self.obj = Some(addr);
        self.obj_iterator = Some(ZReverseBitMapIterator::with_range(
            self.remset.previous(),
            index.saturating_sub(1),
            index,
        ));
        Some(ZRememberedSetContaining { field_addr, addr })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_set_current_previous() {
        let mut remset = ZRememberedSet::new();
        remset.initialize(4096);
        assert!(remset.set_current(16));
        assert!(remset.at_current(16));
        remset.swap_remset_bitmaps();
        assert!(remset.at_previous(16));
        assert!(remset.is_cleared_current());
        ZRememberedSet::flip();
        assert!(remset.at_current(16));
    }
}
