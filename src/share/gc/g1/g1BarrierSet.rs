/*
 * Port of OpenJDK 25 HotSpot G1BarrierSet (share/gc/g1/g1BarrierSet.hpp)
 *
 * Implements SATB (Snapshot At The Beginning) pre-write barriers
 * and Card Table post-write barriers for G1 concurrent marking and remset maintenance.
 */

use crate::share::gc::shared::barrierSet::BarrierSet;
use crate::share::oops::oop::Oop;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct G1BarrierSet {
    satb_buffer: Mutex<Vec<usize>>,
    dirty_cards: Mutex<Vec<usize>>,
    satb_mark_count: AtomicUsize,
    post_barrier_count: AtomicUsize,
}

impl G1BarrierSet {
    pub fn new() -> Self {
        Self {
            satb_buffer: Mutex::new(Vec::new()),
            dirty_cards: Mutex::new(Vec::new()),
            satb_mark_count: AtomicUsize::new(0),
            post_barrier_count: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub fn satb_marks(&self) -> usize {
        self.satb_mark_count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn post_barriers(&self) -> usize {
        self.post_barrier_count.load(Ordering::Relaxed)
    }

    pub fn clear_buffers(&self) {
        self.satb_buffer.lock().unwrap().clear();
        self.dirty_cards.lock().unwrap().clear();
    }
}

impl Default for G1BarrierSet {
    fn default() -> Self {
        Self::new()
    }
}

impl BarrierSet for G1BarrierSet {
    fn write_ref_field_pre(&self, field_addr: usize) {
        self.satb_mark_count.fetch_add(1, Ordering::Relaxed);
        let mut buf = self.satb_buffer.lock().unwrap();
        if buf.len() < 1024 {
            buf.push(field_addr);
        }
    }

    fn write_ref_field_post(&self, field_addr: usize, _new_val: Oop) {
        self.post_barrier_count.fetch_add(1, Ordering::Relaxed);
        let card_index = field_addr >> 9; // 512 bytes per card
        let mut cards = self.dirty_cards.lock().unwrap();
        if cards.len() < 1024 {
            cards.push(card_index);
        }
    }
}
