

use parking_lot::Mutex;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug)]
pub struct LeakSample {
    pub cutoff_time: i64,
    pub emit_all: bool,
    pub send_event: bool,
}

static SAMPLES: LazyLock<Mutex<Vec<LeakSample>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub fn emit_events(cutoff_time: i64, emit_all: bool, send_event: bool) {
    SAMPLES.lock().push(LeakSample {
        cutoff_time,
        emit_all,
        send_event,
    });
}

pub fn take_samples() -> Vec<LeakSample> {
    std::mem::take(&mut *SAMPLES.lock())
}
