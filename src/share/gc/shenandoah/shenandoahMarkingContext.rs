/*
 * Port of OpenJDK 25 HotSpot ShenandoahMarkingContext
 * (share/gc/shenandoah/shenandoahMarkingContext.hpp / shenandoahMarkingContext.cpp /
 *  shenandoahMarkingContext.inline.hpp)
 *
 * Marking bitmap plus per-region top-at-mark-start (TAMS) and
 * top-bitmap watermarks.
 */

use super::shenandoahHeapRegion::{ShenandoahAffiliation, ShenandoahHeapRegion};
use super::shenandoahMarkBitMap::ShenandoahMarkBitMap;

pub struct ShenandoahMarkingContext {
    mark_bit_map: ShenandoahMarkBitMap,
    top_bitmaps: Vec<usize>,
    top_at_mark_starts_base: Vec<usize>,
    heap_base: u64,
}

impl ShenandoahMarkingContext {
    pub fn new(heap_base: u64, heap_end: u64, num_regions: usize) -> Self {
        Self {
            mark_bit_map: ShenandoahMarkBitMap::new(heap_base, heap_end),
            top_bitmaps: vec![0; num_regions],
            top_at_mark_starts_base: vec![0; num_regions],
            heap_base,
        }
    }

    pub fn mark_bit_map(&self) -> &ShenandoahMarkBitMap {
        &self.mark_bit_map
    }

    pub fn mark_strong(&self, obj: u64, was_upgraded: &mut bool) -> bool {
        !self.allocated_after_mark_start(obj)
            && self.mark_bit_map.mark_strong(obj, was_upgraded)
    }

    pub fn mark_weak(&self, obj: u64) -> bool {
        !self.allocated_after_mark_start(obj) && self.mark_bit_map.mark_weak(obj)
    }

    pub fn is_marked(&self, raw_obj: u64) -> bool {
        self.allocated_after_mark_start(raw_obj) || self.mark_bit_map.is_marked(raw_obj)
    }

    pub fn is_marked_strong(&self, raw_obj: u64) -> bool {
        self.allocated_after_mark_start(raw_obj) || self.mark_bit_map.is_marked_strong(raw_obj)
    }

    pub fn is_marked_weak(&self, raw_obj: u64) -> bool {
        self.allocated_after_mark_start(raw_obj) || self.mark_bit_map.is_marked_weak(raw_obj)
    }

    pub fn is_marked_or_old(&self, _old_during_young: bool, raw_obj: u64) -> bool {
        self.is_marked(raw_obj) || _old_during_young
    }

    pub fn is_marked_strong_or_old(&self, _old_during_young: bool, raw_obj: u64) -> bool {
        self.is_marked_strong(raw_obj) || _old_during_young
    }

    pub fn get_next_marked_addr(&self, addr: u64, limit: u64) -> u64 {
        self.mark_bit_map.get_next_marked_addr(addr, limit)
    }

    pub fn get_prev_marked_addr(&self, limit: u64, addr: u64) -> u64 {
        self.mark_bit_map.get_prev_marked_addr(limit, addr)
    }

    pub fn allocated_after_mark_start(&self, addr: u64) -> bool {
        let index = self.region_index(addr);
        if index >= self.top_at_mark_starts_base.len() {
            return true;
        }
        addr >= self.top_at_mark_starts_base[index] as u64
    }

    pub fn top_at_mark_start(&self, region: &ShenandoahHeapRegion) -> u64 {
        self.top_at_mark_starts_base[region.index()] as u64
    }

    pub fn top_bitmap(&self, region: &ShenandoahHeapRegion) -> u64 {
        self.top_bitmaps[region.index()] as u64
    }

    pub fn capture_top_at_mark_start(&mut self, region: &ShenandoahHeapRegion) {
        if !region.is_affiliated() {
            return;
        }
        let idx = region.index();
        let old_tams = self.top_at_mark_starts_base[idx] as u64;
        let new_tams = region.top() as u64;
        debug_assert!(new_tams >= old_tams);
        debug_assert!(
            new_tams == region.bottom() as u64
                || old_tams == region.bottom() as u64
                || new_tams >= self.top_bitmaps[idx] as u64
        );
        debug_assert!(
            old_tams == region.bottom() as u64
                || self.is_bitmap_range_within_region_clear(old_tams, new_tams)
        );
        self.top_at_mark_starts_base[idx] = new_tams as usize;
        self.top_bitmaps[idx] = new_tams as usize;
    }

    pub fn initialize_top_at_mark_start(&mut self, region: &ShenandoahHeapRegion) {
        let idx = region.index();
        self.top_at_mark_starts_base[idx] = region.bottom();
        self.top_bitmaps[idx] = region.bottom();
    }

    pub fn reset_top_at_mark_start(&mut self, region: &ShenandoahHeapRegion) {
        let idx = region.index();
        self.top_at_mark_starts_base[idx] = region.bottom();
    }

    pub fn reset_top_bitmap(&mut self, region: &ShenandoahHeapRegion) {
        debug_assert!(self.is_bitmap_range_within_region_clear(
            region.bottom() as u64,
            region.end() as u64
        ));
        let idx = region.index();
        self.top_bitmaps[idx] = region.bottom();
    }

    pub fn clear_bitmap(&mut self, region: &ShenandoahHeapRegion) {
        self.mark_bit_map.clear_range_large(region.bottom() as u64, region.end() as u64);
        self.reset_top_bitmap(region);
    }

    pub fn is_bitmap_clear(&self) -> bool {
        self.mark_bit_map.is_bitmap_clear()
    }

    pub fn is_bitmap_range_within_region_clear(&self, start: u64, end: u64) -> bool {
        self.mark_bit_map.is_bitmap_range_within_region_clear(start, end)
    }

    fn region_index(&self, addr: u64) -> usize {
        let delta = addr.wrapping_sub(self.heap_base);
        (delta >> ShenandoahHeapRegion::REGION_SIZE_BYTES_SHIFT) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tams_semantics() {
        let heap_base = 0x1_0000_0000u64;
        let region_size = ShenandoahHeapRegion::DEFAULT_REGION_SIZE;
        let mut ctx = ShenandoahMarkingContext::new(heap_base, heap_base + 4 * region_size as u64, 4);
        let mut region = ShenandoahHeapRegion::new_affiliated(
            0,
            heap_base as usize,
            region_size,
            ShenandoahAffiliation::Young,
            true,
        );
        ctx.initialize_top_at_mark_start(&region);
        let obj = heap_base + 0x40;
        region.allocate(0x40, 1);
        region.allocate(0x40, 2);
        ctx.capture_top_at_mark_start(&region);
        assert_eq!(ctx.top_at_mark_start(&region), region.top() as u64);
        let mut upgraded = false;
        assert!(ctx.mark_strong(obj, &mut upgraded));
        assert!(ctx.is_marked_strong(obj));
        assert!(ctx.allocated_after_mark_start(region.top() as u64 + 8));
    }
}
