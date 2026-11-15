/*
 * OpenJDK 25 HotSpot jfr/utilities/jfrSignal.hpp port
 */

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct JfrSignal {
    signaled: AtomicBool,
}

impl JfrSignal {
    pub fn new() -> JfrSignal {
        JfrSignal {
            signaled: AtomicBool::new(false),
        }
    }

    pub fn signal(&self) {
        self.signaled.store(true, Ordering::Release);
    }

    pub fn reset(&self) {
        self.signaled.store(false, Ordering::Release);
    }

    pub fn is_signaled(&self) -> bool {
        self.signaled.load(Ordering::Acquire)
    }

    pub fn signal_if_not_set(&self) {
        if !self.is_signaled() {
            self.signal();
        }
    }

    pub fn is_signaled_with_reset(&self) -> bool {
        if self.is_signaled() {
            self.reset();
            return true;
        }
        false
    }
}
