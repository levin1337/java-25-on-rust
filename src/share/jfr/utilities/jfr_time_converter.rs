/*
 * OpenJDK 25 HotSpot jfr/utilities/jfrTimeConverter.hpp + .cpp port
 */

use super::jfr_time::{NANOS_PER_MILLISEC, NANOS_PER_SEC};

// Counter is nanosecond based; frequency = NANOS_PER_SEC.
fn counter_to_nano_multiplier() -> f64 {
    1.0
}

fn nano_to_counter_multiplier() -> f64 {
    1.0
}

pub fn counter_to_nanos(c: i64) -> i64 {
    (c as f64 * counter_to_nano_multiplier()) as i64
}

pub fn counter_to_millis(c: i64) -> i64 {
    ((c as f64 * counter_to_nano_multiplier()) / NANOS_PER_MILLISEC) as i64
}

pub fn nanos_to_countertime(nanos: i64) -> i64 {
    if nanos <= 0 {
        0
    } else {
        (nanos as f64 * nano_to_counter_multiplier()) as i64
    }
}

pub const NANOS_PER_SECOND_F64: f64 = NANOS_PER_SEC;
