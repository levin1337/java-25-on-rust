

use std::time::{Duration, Instant};

pub struct GranularTimer {
    start: Instant,
    last: Instant,
}

impl GranularTimer {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last: now,
        }
    }

    pub fn start(&mut self) {
        self.start = Instant::now();
        self.last = self.start;
    }

    pub fn stop(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn elapsed(&self) -> Duration {
        self.last.elapsed()
    }

    pub fn lap(&mut self) -> Duration {
        let now = Instant::now();
        let delta = now.duration_since(self.last);
        self.last = now;
        delta
    }
}

impl Default for GranularTimer {
    fn default() -> Self {
        Self::new()
    }
}
