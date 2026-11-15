use super::z_bitmap::ZBitMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const NUM_SEGMENTS: usize = 64;
const BITS_PER_OBJECT: usize = 2;
const SEGMENT_LIVE_BITS: u64 = 0;
const SEGMENT_CLAIM_BITS: u64 = 0;

pub struct ZLiveMap {
    segment_size: usize,
    segment_shift: u32,
    seqnum: AtomicU32,
    live_objects: AtomicU64,
    live_bytes: AtomicU64,
    segment_live_bits: AtomicU64,
    segment_claim_bits: AtomicU64,
    bitmap: std::sync::Mutex<Option<ZBitMap>>,
}

impl ZLiveMap {
    pub fn new(object_max_count: u32) -> Self {
        let segment_size = if object_max_count == 1 {
            BITS_PER_OBJECT
        } else {
            (object_max_count as usize / NUM_SEGMENTS) * BITS_PER_OBJECT
        };
        let segment_size = segment_size.max(BITS_PER_OBJECT);
        Self {
            segment_size,
            segment_shift: segment_size.trailing_zeros(),
            seqnum: AtomicU32::new(0),
            live_objects: AtomicU64::new(0),
            live_bytes: AtomicU64::new(0),
            segment_live_bits: AtomicU64::new(SEGMENT_LIVE_BITS),
            segment_claim_bits: AtomicU64::new(SEGMENT_CLAIM_BITS),
            bitmap: std::sync::Mutex::new(None),
        }
    }

    pub fn reset(&self) {
        self.seqnum.store(0, Ordering::Release);
    }

    pub fn seqnum(&self) -> u32 {
        self.seqnum.load(Ordering::Acquire)
    }

    pub fn is_marked(&self, generation_seqnum: u32) -> bool {
        self.seqnum() == generation_seqnum
    }

    pub fn live_objects(&self) -> u32 {
        self.live_objects.load(Ordering::Acquire) as u32
    }

    pub fn live_bytes(&self) -> usize {
        self.live_bytes.load(Ordering::Acquire) as usize
    }

    fn initialize_bitmap(&self) {
        let mut guard = self.bitmap.lock().unwrap();
        if guard.is_none() {
            *guard = Some(ZBitMap::new(self.segment_size * NUM_SEGMENTS));
        }
    }

    fn reset_for_seqnum(&self, seqnum: u32) {
        const INITIALIZING: u32 = u32::MAX;
        let current = self.seqnum.load(Ordering::Acquire);
        if current == seqnum {
            return;
        }
        if current != INITIALIZING
            && self
                .seqnum
                .compare_exchange(current, INITIALIZING, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.live_objects.store(0, Ordering::Relaxed);
            self.live_bytes.store(0, Ordering::Relaxed);
            self.segment_live_bits.store(0, Ordering::Release);
            self.segment_claim_bits.store(0, Ordering::Release);
            self.initialize_bitmap();
            self.seqnum.store(seqnum, Ordering::Release);
            return;
        }
        while self.seqnum.load(Ordering::Acquire) != seqnum {
            std::thread::yield_now();
        }
    }

    fn is_segment_live(&self, segment: usize) -> bool {
        (self.segment_live_bits.load(Ordering::Acquire) & (1u64 << segment)) != 0
    }

    fn set_segment_live(&self, segment: usize) -> bool {
        let old = self.segment_live_bits.fetch_or(1u64 << segment, Ordering::AcqRel);
        (old & (1u64 << segment)) == 0
    }

    fn claim_segment(&self, segment: usize) -> bool {
        let old = self.segment_claim_bits.fetch_or(1u64 << segment, Ordering::AcqRel);
        (old & (1u64 << segment)) == 0
    }

    fn reset_segment(&self, segment: usize) {
        if !self.claim_segment(segment) {
            while !self.is_segment_live(segment) {
                std::thread::yield_now();
            }
            return;
        }
        let start = segment * self.segment_size;
        let end = start + self.segment_size;
        if let Some(bitmap) = self.bitmap.lock().unwrap().as_ref() {
            bitmap.clear_range(start, end);
        }
        self.set_segment_live(segment);
    }

    fn index_to_segment(&self, index: usize) -> usize {
        index >> self.segment_shift
    }

    fn first_live_segment(&self) -> Option<usize> {
        let bits = self.segment_live_bits.load(Ordering::Acquire);
        if bits == 0 { None } else { Some(bits.trailing_zeros() as usize) }
    }

    fn next_live_segment(&self, segment: usize) -> Option<usize> {
        let bits = self.segment_live_bits.load(Ordering::Acquire) & (!0u64 << (segment + 1));
        if bits == 0 { None } else { Some(bits.trailing_zeros() as usize) }
    }

    pub fn get(&self, generation_seqnum: u32, index: usize) -> bool {
        let segment = self.index_to_segment(index);
        self.is_marked(generation_seqnum)
            && self.is_segment_live(segment)
            && self
                .bitmap
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|bitmap| bitmap.at(index))
    }

    pub fn set(&self, generation_seqnum: u32, index: usize, finalizable: bool) -> (bool, bool) {
        self.reset_for_seqnum(generation_seqnum);
        let segment = self.index_to_segment(index);
        if !self.is_segment_live(segment) {
            self.reset_segment(segment);
        }
        let bitmap = self.bitmap.lock().unwrap();
        let bitmap = bitmap.as_ref().expect("livemap initialized");
        let (changed, inc_live) = bitmap.par_set_bit_pair(index, finalizable);
        (changed, inc_live)
    }

    pub fn inc_live(&self, objects: u32, bytes: usize) {
        self.live_objects.fetch_add(objects as u64, Ordering::Relaxed);
        self.live_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn iterate<F>(&self, generation_seqnum: u32, mut function: F)
    where
        F: FnMut(usize) -> bool,
    {
        if !self.is_marked(generation_seqnum) {
            return;
        }
        let mut segment = self.first_live_segment();
        while let Some(current) = segment {
            let start = current * self.segment_size;
            let end = start + self.segment_size;
            let guard = self.bitmap.lock().unwrap();
            if let Some(bitmap) = guard.as_ref() {
                bitmap.iterate_range(start, end, |index| (index & 1) == 0 && function(index));
            }
            drop(guard);
            segment = self.next_live_segment(current);
        }
    }

    pub fn find_base_bit(&self, index: usize) -> Option<usize> {
        let start_segment = self.index_to_segment(index);
        if self.is_segment_live(start_segment) {
            if let Some(result) = self.find_base_bit_in_segment(start_segment * self.segment_size, index) {
                return Some(result);
            }
        }
        let mut segment = start_segment;
        while segment > 0 {
            segment -= 1;
            if self.is_segment_live(segment) {
                let end = (segment + 1) * self.segment_size;
                if let Some(result) = self.find_base_bit_in_segment(segment * self.segment_size, end - 1) {
                    return Some(result);
                }
            }
        }
        None
    }

    fn find_base_bit_in_segment(&self, start: usize, index: usize) -> Option<usize> {
        let guard = self.bitmap.lock().unwrap();
        let bitmap = guard.as_ref()?;
        bitmap
            .find_last_set_bit(start, index + 1)
            .map(|bit| bit & !1)
    }
}

impl Default for ZLiveMap {
    fn default() -> Self {
        Self::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn livemap_marks_pairs() {
        let map = ZLiveMap::new(128);
        let (changed, inc_live) = map.set(7, 4, false);
        assert!(changed);
        assert!(inc_live);
        assert!(map.get(7, 4));
        assert!(map.get(7, 5));
        let mut visited = Vec::new();
        map.iterate(7, |index| {
            visited.push(index);
            true
        });
        assert_eq!(visited, vec![4]);
    }
}
