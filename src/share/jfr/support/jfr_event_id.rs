/*
 * OpenJDK 25 HotSpot jfr/support/jfrEventId.hpp port
 *
 * Maps event names to JFR event ids. Event ids are assigned at build
 * time by the events.xml generator; see jfr_types::EVENT_NAME_TO_ID.
 */

use crate::share::jfr::utilities::jfr_types::{JfrEventId, EVENT_NAME_TO_ID};

pub struct JfrEventIdMap;

impl JfrEventIdMap {
    pub fn id_for_name(name: &str) -> Option<JfrEventId> {
        EVENT_NAME_TO_ID
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, id)| *id)
    }

    pub fn name_for_id(id: JfrEventId) -> Option<&'static str> {
        EVENT_NAME_TO_ID
            .iter()
            .find(|(_, i)| *i == id)
            .map(|(n, _)| *n)
    }

    pub fn is_valid(id: JfrEventId) -> bool {
        (id as u64) > EVENT_CHECKPOINT_ID as u64 && (id as u64) < MAX_JFR_EVENT_ID as u64
    }
}

const EVENT_CHECKPOINT_ID: JfrEventId = 1;
const MAX_JFR_EVENT_ID: JfrEventId = 255;
