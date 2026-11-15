use super::z_address::{Z_GRANULE_SIZE, Z_GRANULE_SIZE_SHIFT, ZOffset, address_to_offset, offset_to_backing_index};

#[derive(Clone)]
pub struct ZGranuleMap<T> {
    size: usize,
    map: Vec<T>,
}

impl<T: Copy + Default> ZGranuleMap<T> {
    pub fn new(max_offset: ZOffset) -> Self {
        assert!(max_offset != 0 && max_offset % Z_GRANULE_SIZE == 0);
        Self {
            size: (max_offset >> Z_GRANULE_SIZE_SHIFT) as usize,
            map: vec![T::default(); (max_offset >> Z_GRANULE_SIZE_SHIFT) as usize],
        }
    }

    pub fn len(&self) -> usize {
        self.size
    }

    fn index_for_offset(&self, offset: ZOffset) -> usize {
        (offset >> Z_GRANULE_SIZE_SHIFT) as usize
    }

    pub fn get(&self, offset: ZOffset) -> T {
        let index = self.index_for_offset(offset);
        assert!(index < self.size);
        self.map[index]
    }

    pub fn put(&mut self, offset: ZOffset, value: T) {
        let index = self.index_for_offset(offset);
        assert!(index < self.size);
        self.map[index] = value;
    }

    pub fn put_range(&mut self, offset: ZOffset, size: usize, value: T) {
        assert!(size % Z_GRANULE_SIZE as usize == 0);
        let start = self.index_for_offset(offset);
        let end = start + (size / Z_GRANULE_SIZE as usize);
        assert!(end <= self.size);
        for slot in &mut self.map[start..end] {
            *slot = value;
        }
    }

    pub fn addr(&self, offset: ZOffset) -> &T {
        &self.map[self.index_for_offset(offset)]
    }

    pub fn addr_mut(&mut self, offset: ZOffset) -> &mut T {
        let index = self.index_for_offset(offset);
        &mut self.map[index]
    }
}

pub fn granule_index_for_address(address: u64) -> usize {
    offset_to_backing_index(address_to_offset(address)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granule_map_put_get() {
        let mut map = ZGranuleMap::<u32>::new(Z_GRANULE_SIZE * 4);
        map.put(Z_GRANULE_SIZE * 2, 42);
        assert_eq!(map.get(Z_GRANULE_SIZE * 2), 42);
        map.put_range(0, (Z_GRANULE_SIZE * 3) as usize, 7);
        assert_eq!(map.get(0), 7);
        assert_eq!(map.get(Z_GRANULE_SIZE * 2), 7);
    }
}
