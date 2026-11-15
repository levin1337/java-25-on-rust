/*
 * Port of OpenJDK 25 HotSpot NMT_PreInit
 * (share/nmt/nmtPreInit.hpp/.cpp)
 *
 * Captures the -XX:NativeMemoryTracking value as early as possible —
 * before the full Arguments parse — so that NMT can be enabled for
 * allocations that happen during VM bootstrap. The recorded level is
 * later consumed by MemTracker::initialize_from_flag.
 */

use super::nmtCommon::{NmtTrackingLevel, NmtUtil};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

static PRE_INIT_LEVEL: AtomicU8 = AtomicU8::new(NmtTrackingLevel::Unknown as u8);
static FLAG_VALUE: OnceLock<String> = OnceLock::new();

pub struct NmtPreInit;

impl NmtPreInit {
    /// Record the flag value seen during early argument scanning.
    /// Returns the parsed level (Unknown for invalid values).
    pub fn pre_initialize(value: &str) -> NmtTrackingLevel {
        let level = NmtUtil::parse_tracking_level(Some(value));
        PRE_INIT_LEVEL.store(level as u8, Ordering::Release);
        let _ = FLAG_VALUE.set(value.to_string());
        level
    }

    /// Level recorded pre-init (Unknown if the flag was never seen).
    pub fn tracking_level() -> NmtTrackingLevel {
        match PRE_INIT_LEVEL.load(Ordering::Acquire) {
            1 => NmtTrackingLevel::Off,
            2 => NmtTrackingLevel::Summary,
            3 => NmtTrackingLevel::Detail,
            _ => NmtTrackingLevel::Unknown,
        }
    }

    /// Raw flag value recorded pre-init, if any.
    pub fn flag_value() -> Option<&'static str> {
        FLAG_VALUE.get().map(|s| s.as_str())
    }

    /// Whether the flag was seen during pre-init.
    pub fn is_set() -> bool {
        FLAG_VALUE.get().is_some()
    }
}
