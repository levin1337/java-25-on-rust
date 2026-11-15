/*
 * OpenJDK 25 HotSpot jfr/recorder/service/jfrEventThrottler.hpp + .cpp port
 *
 * Simple per-event throttling: minimum interval between events of the
 * same type (throttle period from settings, in ticks).
 */

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct JfrEventThrottler {
    last_event_time: Mutex<HashMap<u64, i64>>,
    period_ticks: Mutex<HashMap<u64, i64>>,
}

static THROTTLER: std::sync::OnceLock<JfrEventThrottler> = std::sync::OnceLock::new();

impl JfrEventThrottler {
    fn instance() -> &'static JfrEventThrottler {
        THROTTLER.get_or_init(JfrEventThrottler::default)
    }

    pub fn register(event_id: u64, period_ticks: i64) {
        let t = Self::instance();
        t.period_ticks.lock().unwrap().insert(event_id, period_ticks);
    }

    // Returns true if the event may be written now.
    pub fn should_commit(event_id: u64, now_ticks: i64) -> bool {
        let t = Self::instance();
        let periods = t.period_ticks.lock().unwrap();
        let period = match periods.get(&event_id) {
            Some(&p) => p,
            None => return true,
        };
        drop(periods);
        let mut last = t.last_event_time.lock().unwrap();
        let prev = last.get(&event_id).copied().unwrap_or(i64::MIN);
        if period <= 0 || now_ticks - prev >= period {
            last.insert(event_id, now_ticks);
            true
        } else {
            false
        }
    }

    pub fn clear() {
        let t = Self::instance();
        t.last_event_time.lock().unwrap().clear();
        t.period_ticks.lock().unwrap().clear();
    }
}
