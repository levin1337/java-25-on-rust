/*
 * Port of OpenJDK 25 HotSpot ElapsedTimer and TraceTime (share/runtime/timer.hpp / timer.cpp)
 *
 * Provides high-precision time measurement for GC pauses, JIT compilation,
 * and runtime diagnostics.
 */

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ElapsedTimer {
    start_instant: Option<Instant>,
    accumulated: Duration,
    is_active: bool,
}

impl ElapsedTimer {
    pub fn new() -> Self {
        Self {
            start_instant: None,
            accumulated: Duration::ZERO,
            is_active: false,
        }
    }

    pub fn start(&mut self) {
        if !self.is_active {
            self.start_instant = Some(Instant::now());
            self.is_active = true;
        }
    }

    pub fn stop(&mut self) {
        if self.is_active {
            if let Some(start) = self.start_instant.take() {
                self.accumulated += start.elapsed();
            }
            self.is_active = false;
        }
    }

    pub fn reset(&mut self) {
        self.start_instant = None;
        self.accumulated = Duration::ZERO;
        self.is_active = false;
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn duration(&self) -> Duration {
        if self.is_active {
            if let Some(start) = self.start_instant {
                return self.accumulated + start.elapsed();
            }
        }
        self.accumulated
    }

    #[inline]
    pub fn seconds(&self) -> f64 {
        self.duration().as_secs_f64()
    }

    #[inline]
    pub fn milliseconds(&self) -> i64 {
        self.duration().as_millis() as i64
    }

    #[inline]
    pub fn microseconds(&self) -> i64 {
        self.duration().as_micros() as i64
    }

    #[inline]
    pub fn nanoseconds(&self) -> i64 {
        self.duration().as_nanos() as i64
    }
}

impl Default for ElapsedTimer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TraceTime {
    name: &'static str,
    timer: ElapsedTimer,
}

impl TraceTime {
    pub fn new(name: &'static str) -> Self {
        let mut timer = ElapsedTimer::new();
        timer.start();
        Self { name, timer }
    }

    #[inline]
    pub fn name(&self) -> &'static str {
        self.name
    }

    #[inline]
    pub fn elapsed_millis(&self) -> i64 {
        self.timer.milliseconds()
    }

    #[inline]
    pub fn elapsed_nanos(&self) -> i64 {
        self.timer.nanoseconds()
    }
}

impl Drop for TraceTime {
    fn drop(&mut self) {
        self.timer.stop();
    }
}
