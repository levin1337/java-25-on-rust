/*
 * OpenJDK 25 HotSpot jfr/recorder/repository/jfrChunkRotation.hpp + .cpp port
 */

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

static THRESHOLD: AtomicI64 = AtomicI64::new(0);
static ROTATE: AtomicBool = AtomicBool::new(false);

// Notifies the Java chunk rotation monitor; in this port the recorder
// service polls should_rotate() on its loop, so no JNI notify is needed.
pub struct JfrChunkRotation;

impl JfrChunkRotation {
    pub fn evaluate(size_written: i64) {
        let threshold = THRESHOLD.load(Ordering::Acquire);
        if threshold <= 0 || ROTATE.load(Ordering::Acquire) {
            return;
        }
        if size_written > threshold {
            ROTATE.store(true, Ordering::Release);
        }
    }

    pub fn set_threshold(bytes: i64) {
        THRESHOLD.store(bytes, Ordering::Release);
    }

    pub fn should_rotate() -> bool {
        ROTATE.load(Ordering::Acquire)
    }

    pub fn on_rotation() {
        ROTATE.store(false, Ordering::Release);
    }
}
