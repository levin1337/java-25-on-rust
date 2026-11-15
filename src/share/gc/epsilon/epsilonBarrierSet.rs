/*
 * Port of OpenJDK 25 HotSpot EpsilonBarrierSet
 * (share/gc/epsilon/epsilonBarrierSet.hpp/.cpp)
 *
 * "No interaction with application is required for Epsilon, and
 * therefore the barrier set is empty."  The port keeps the explicit
 * no-op barrier methods and the per-thread TLD lifecycle hooks
 * (create/destroy) used by the elastic TLAB policy.
 */

use crate::share::gc::epsilon::epsilonThreadLocalData::EpsilonThreadLocalDataRegistry;
use crate::share::gc::shared::barrierSet::BarrierSet;
use crate::share::oops::oop::Oop;
use std::sync::Mutex;

#[derive(Default)]
pub struct EpsilonBarrierSet {
    thread_local_data: Mutex<EpsilonThreadLocalDataRegistry>,
}

impl EpsilonBarrierSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn barrier_set_name(&self) -> &'static str {
        "EpsilonBarrierSet"
    }

    pub fn requires_barriers(&self) -> bool {
        false
    }

    pub fn on_thread_create(&self, thread: usize) {
        self.thread_local_data
            .lock()
            .unwrap()
            .create(thread);
    }

    pub fn on_thread_destroy(&self, thread: usize) {
        self.thread_local_data
            .lock()
            .unwrap()
            .destroy(thread);
    }

    pub fn ergo_tlab_size(&self, thread: usize) -> usize {
        self.thread_local_data
            .lock()
            .unwrap()
            .ergo_tlab_size(thread)
    }

    pub fn set_ergo_tlab_size(&self, thread: usize, val: usize) {
        self.thread_local_data
            .lock()
            .unwrap()
            .set_ergo_tlab_size(thread, val);
    }

    pub fn last_tlab_time(&self, thread: usize) -> i64 {
        self.thread_local_data
            .lock()
            .unwrap()
            .last_tlab_time(thread)
    }

    pub fn set_last_tlab_time(&self, thread: usize, time: i64) {
        self.thread_local_data
            .lock()
            .unwrap()
            .set_last_tlab_time(thread, time);
    }
}

impl BarrierSet for EpsilonBarrierSet {
    fn write_ref_field_pre(&self, _field_addr: usize) {}

    fn write_ref_field_post(&self, _field_addr: usize, _new_val: Oop) {}

    fn read_ref_field(&self, _field_addr: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_interaction_required() {
        let bs = EpsilonBarrierSet::new();
        assert!(!bs.requires_barriers());
        assert_eq!(bs.barrier_set_name(), "EpsilonBarrierSet");
        let oop = Oop::Null;
        bs.write_ref_field_pre(0x1000);
        bs.write_ref_field_post(0x1008, oop);
        bs.read_ref_field(0x1010);
    }

    #[test]
    fn thread_lifecycle_tracks_tld() {
        let bs = EpsilonBarrierSet::new();
        bs.on_thread_create(3);
        bs.set_ergo_tlab_size(3, 2048);
        bs.set_last_tlab_time(3, 999);
        assert_eq!(bs.ergo_tlab_size(3), 2048);
        assert_eq!(bs.last_tlab_time(3), 999);
        bs.on_thread_destroy(3);
        assert_eq!(bs.ergo_tlab_size(3), 0);
    }
}
