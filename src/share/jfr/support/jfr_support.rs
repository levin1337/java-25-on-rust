/*
 * OpenJDK 25 HotSpot jfr/support/jfrSupport.hpp port — module helpers.
 */

// Returns whether JFR is currently recording (enabled && recording).
pub fn is_recording() -> bool {
    crate::share::jfr::recorder::service::jfr_post_box::JfrPostBox::is_recording()
}

pub fn is_enabled() -> bool {
    crate::share::jfr::recorder::service::jfr_post_box::JfrPostBox::is_enabled()
}
