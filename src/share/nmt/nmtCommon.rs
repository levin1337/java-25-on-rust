/*
 * Port of OpenJDK 25 HotSpot nmtCommon (share/nmt/nmtCommon.hpp/.cpp)
 *
 * NMT tracking levels, tag <-> name conversion helpers and size scale
 * utilities shared by the whole NMT subsystem.
 */

use super::memTag::MemTag;

/// Native memory tracking level. Mirrors NMT_TrackingLevel:
/// keep the numeric order — unknown < off < summary < detail.
///
///   unknown — pre-init phase (before parsing -XX:NativeMemoryTracking)
///   off     — nothing is tracked
///   summary — per-tag category summaries are tracked
///   detail  — malloc details per call site and VM mapping info are tracked
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum NmtTrackingLevel {
    Unknown = 0,
    Off = 1,
    Summary = 2,
    Detail = 3,
}

/// Backwards-compatible alias (earlier ports used NmtLevel).
pub type NmtLevel = NmtTrackingLevel;

/// Number of stack frames captured per allocation site
/// (NMT_TrackingStackDepth).
pub const NMT_TRACKING_STACK_DEPTH: usize = 4;

/// Size scales used by NMT reports (NMTUtil::scale_name/scale_from_name).
pub const SCALE_BYTES: usize = 1;
pub const SCALE_KB: usize = 1024;
pub const SCALE_MB: usize = 1024 * 1024;
pub const SCALE_GB: usize = 1024 * 1024 * 1024;

/// NMTUtil — tag/index/name helpers and scale utilities.
pub struct NmtUtil;

impl NmtUtil {
    /// Check if index is a valid MemTag enum value (including mtNone).
    #[inline]
    pub fn tag_index_is_valid(index: i32) -> bool {
        index >= 0 && index < MemTag::MtNumberOfTags as i32
    }

    /// Check if tag is a valid MemTag enum value (including mtNone).
    #[inline]
    pub fn tag_is_valid(tag: MemTag) -> bool {
        Self::tag_index_is_valid(tag as i32)
    }

    /// Map memory tag to index.
    #[inline]
    pub fn tag_to_index(tag: MemTag) -> usize {
        tag as usize
    }

    /// Map memory tag to human readable name.
    #[inline]
    pub fn tag_to_name(tag: MemTag) -> &'static str {
        tag.name()
    }

    /// Map memory tag to literalized enum name (e.g. "mtTest").
    #[inline]
    pub fn tag_to_enum_name(tag: MemTag) -> &'static str {
        tag.enum_name()
    }

    /// Map an index to memory tag.
    #[inline]
    pub fn index_to_tag(index: usize) -> MemTag {
        debug_assert!(Self::tag_index_is_valid(index as i32), "Invalid tag index");
        MemTag::from_index(index).unwrap_or(MemTag::MtNone)
    }

    /// Scale name for reports: "", "KB", "MB", "GB".
    pub fn scale_name(scale: usize) -> &'static str {
        match scale {
            1 => "",
            SCALE_KB => "KB",
            SCALE_MB => "MB",
            SCALE_GB => "GB",
            _ => "",
        }
    }

    /// Scale factor from name: "1"/"b", "kb"/"k", "mb"/"m", "gb"/"g".
    /// Returns 0 for an invalid value (caller reports the error).
    pub fn scale_from_name(scale: &str) -> usize {
        match scale.to_ascii_lowercase().as_str() {
            "1" | "b" => SCALE_BYTES,
            "kb" | "k" => SCALE_KB,
            "mb" | "m" => SCALE_MB,
            "gb" | "g" => SCALE_GB,
            _ => 0,
        }
    }

    /// Translate memory size into the given scale (rounded).
    #[inline]
    pub fn amount_in_scale(amount: usize, scale: usize) -> usize {
        (amount + scale / 2) / scale
    }

    /// Parses the tracking level from a string. Returns Unknown if the
    /// string is not a valid level.
    pub fn parse_tracking_level(s: Option<&str>) -> NmtTrackingLevel {
        match s {
            Some("summary") => NmtTrackingLevel::Summary,
            Some("detail") => NmtTrackingLevel::Detail,
            Some("off") => NmtTrackingLevel::Off,
            _ => NmtTrackingLevel::Unknown,
        }
    }

    /// Returns textual representation of a tracking level.
    pub fn tracking_level_to_string(level: NmtTrackingLevel) -> &'static str {
        match level {
            NmtTrackingLevel::Unknown => "unknown",
            NmtTrackingLevel::Off => "off",
            NmtTrackingLevel::Summary => "summary",
            NmtTrackingLevel::Detail => "detail",
        }
    }

    /// Given a string, return associated MemTag. MtNone if the name is
    /// invalid. The string can be the human readable name or the
    /// stringified enum (with or without leading "mt"); case ignored.
    pub fn string_to_mem_tag(name: &str) -> MemTag {
        let needle = name.to_ascii_lowercase();
        for tag in MemTag::all() {
            if tag.name().eq_ignore_ascii_case(name)
                || tag.enum_name().eq_ignore_ascii_case(name)
                || tag.enum_name()[2..].eq_ignore_ascii_case(&needle)
            {
                return tag;
            }
        }
        MemTag::MtNone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_levels_order() {
        assert!(NmtTrackingLevel::Unknown < NmtTrackingLevel::Off);
        assert!(NmtTrackingLevel::Off < NmtTrackingLevel::Summary);
        assert!(NmtTrackingLevel::Summary < NmtTrackingLevel::Detail);
    }

    #[test]
    fn parse_levels() {
        assert_eq!(NmtUtil::parse_tracking_level(Some("summary")), NmtTrackingLevel::Summary);
        assert_eq!(NmtUtil::parse_tracking_level(Some("detail")), NmtTrackingLevel::Detail);
        assert_eq!(NmtUtil::parse_tracking_level(Some("off")), NmtTrackingLevel::Off);
        assert_eq!(NmtUtil::parse_tracking_level(Some("bogus")), NmtTrackingLevel::Unknown);
        assert_eq!(NmtUtil::parse_tracking_level(None), NmtTrackingLevel::Unknown);
    }

    #[test]
    fn tag_lookup() {
        assert_eq!(NmtUtil::string_to_mem_tag("Code"), MemTag::MtCode);
        assert_eq!(NmtUtil::string_to_mem_tag("mtCode"), MemTag::MtCode);
        assert_eq!(NmtUtil::string_to_mem_tag("code"), MemTag::MtCode);
        assert_eq!(NmtUtil::string_to_mem_tag("Java Heap"), MemTag::MtJavaHeap);
        assert_eq!(NmtUtil::string_to_mem_tag("nonsense"), MemTag::MtNone);
        assert_eq!(NmtUtil::tag_to_enum_name(MemTag::MtNmt), "mtNMT");
    }

    #[test]
    fn scales() {
        assert_eq!(NmtUtil::scale_from_name("KB"), SCALE_KB);
        assert_eq!(NmtUtil::scale_from_name("m"), SCALE_MB);
        assert_eq!(NmtUtil::scale_from_name("bogus"), 0);
        assert_eq!(NmtUtil::scale_name(SCALE_MB), "MB");
        assert_eq!(NmtUtil::amount_in_scale(1536, SCALE_KB), 2);
        assert_eq!(NmtUtil::amount_in_scale(512, SCALE_KB), 1);
        assert_eq!(NmtUtil::amount_in_scale(400, SCALE_KB), 0);
    }
}
