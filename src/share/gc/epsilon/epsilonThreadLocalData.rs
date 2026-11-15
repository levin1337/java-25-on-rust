/*
 * Port of OpenJDK 25 HotSpot EpsilonThreadLocalData
 * (share/gc/epsilon/epsilonThreadLocalData.hpp)
 *
 * Per-thread ergonomic TLAB size and last-allocation timestamp used
 * by the elastic TLAB policy.
 */

#[derive(Clone, Debug)]
pub struct EpsilonThreadLocalData {
    ergo_tlab_size: usize,
    last_tlab_time: i64,
}

impl EpsilonThreadLocalData {
    pub fn new() -> Self {
        Self {
            ergo_tlab_size: 0,
            last_tlab_time: 0,
        }
    }

    pub fn ergo_tlab_size(&self) -> usize {
        self.ergo_tlab_size
    }

    pub fn set_ergo_tlab_size(&mut self, val: usize) {
        self.ergo_tlab_size = val;
    }

    pub fn last_tlab_time(&self) -> i64 {
        self.last_tlab_time
    }

    pub fn set_last_tlab_time(&mut self, time: i64) {
        self.last_tlab_time = time;
    }
}

impl Default for EpsilonThreadLocalData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct EpsilonThreadLocalDataRegistry {
    data: Vec<EpsilonThreadLocalData>,
}

impl EpsilonThreadLocalDataRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&mut self, thread: usize) {
        if self.data.len() <= thread {
            self.data.resize_with(thread + 1, EpsilonThreadLocalData::new);
        } else {
            self.data[thread] = EpsilonThreadLocalData::new();
        }
    }

    pub fn destroy(&mut self, thread: usize) {
        if thread < self.data.len() {
            self.data[thread] = EpsilonThreadLocalData::new();
        }
    }

    pub fn ergo_tlab_size(&self, thread: usize) -> usize {
        self.data.get(thread).map_or(0, |d| d.ergo_tlab_size())
    }

    pub fn set_ergo_tlab_size(&mut self, thread: usize, val: usize) {
        if thread < self.data.len() {
            self.data[thread].set_ergo_tlab_size(val);
        }
    }

    pub fn last_tlab_time(&self, thread: usize) -> i64 {
        self.data.get(thread).map_or(0, |d| d.last_tlab_time())
    }

    pub fn set_last_tlab_time(&mut self, thread: usize, time: i64) {
        if thread < self.data.len() {
            self.data[thread].set_last_tlab_time(time);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tlab_size_tracking() {
        let mut reg = EpsilonThreadLocalDataRegistry::new();
        reg.create(0);
        reg.create(1);
        reg.set_ergo_tlab_size(0, 4096);
        reg.set_ergo_tlab_size(1, 8192);
        reg.set_last_tlab_time(0, 12345);
        assert_eq!(reg.ergo_tlab_size(0), 4096);
        assert_eq!(reg.ergo_tlab_size(1), 8192);
        assert_eq!(reg.last_tlab_time(0), 12345);
        reg.destroy(0);
        assert_eq!(reg.ergo_tlab_size(0), 0);
    }
}
