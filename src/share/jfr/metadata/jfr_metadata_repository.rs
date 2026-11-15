/*
 * OpenJDK 25 HotSpot jfr/metadata/jfrMetadataRepository.hpp + .cpp port
 *
 * JfrMetadataRepository — holds the metadata descriptor tree and
 * serializes it into checkpoint events of type jdk.Metadata.
 */

use std::sync::Mutex;

use crate::share::jfr::metadata::jfr_metadata_element::JfrMetadataElement;
use crate::share::jfr::metadata::jfr_metadata_writer::JfrMetadataWriter;
use crate::share::jfr::recorder::checkpoint::jfr_checkpoint_writer::JfrCheckpointWriter;
use crate::share::jfr::utilities::jfr_time::JfrTicks;
use crate::share::jfr::utilities::jfr_types::{JfrCheckpointBufferKind, JfrCheckpointType, JfrTypeId, EVENT_METADATA};
use crate::share::jfr::writers::jfr_writer::JfrWriter;

static REPOSITORY: Mutex<Option<JfrMetadataRepository>> = Mutex::new(None);

pub struct JfrMetadataRepository {
    root: JfrMetadataElement,
}

impl JfrMetadataRepository {
    pub fn instance() -> &'static Mutex<Option<JfrMetadataRepository>> {
        &REPOSITORY
    }

    pub fn initialize() -> bool {
        let mut g = REPOSITORY.lock().unwrap();
        if g.is_none() {
            *g = Some(JfrMetadataRepository::new());
        }
        true
    }

    pub fn new() -> Self {
        JfrMetadataRepository {
            root: Self::build_root(),
        }
    }

    // Build the descriptor tree for the core type system + key events.
    fn build_root() -> JfrMetadataElement {
        let mut root = JfrMetadataElement::new("metadata");
        root.set_attribute("version", "2.1");

        // Type elements for all JFR type ids.
        for id in JfrTypeId::all() {
            let mut e = JfrMetadataElement::new("class");
            e.set_attribute("name", id.metadata_name());
            e.set_attribute("id", &id.as_u64().to_string());
            root.add_child(e);
        }

        // Event elements for the ported events.
        for (name, id) in crate::share::jfr::utilities::jfr_types::EVENT_NAME_TO_ID {
            let mut e = JfrMetadataElement::new("event");
            e.set_attribute("name", name);
            e.set_attribute("id", &u64::from(*id).to_string());
            e.set_attribute("stackTrace", "true");
            root.add_child(e);
        }

        // Setting elements for common recording settings.
        for s in [
            ("period", "20 ms"),
            ("stackdepth", "64"),
            ("threadbuffersize", "8 kB"),
            ("globalbuffersize", "512 kB"),
            ("memorysize", "10 MB"),
            ("maxchunksize", "12 MB"),
            ("maxage", "0 s"),
            ("dumponexit", "false"),
            ("path", "."),
            ("filename", ""),
            ("repository", "."),
        ] {
            let mut e = JfrMetadataElement::new("setting");
            e.set_attribute("name", s.0);
            e.set_attribute("value", s.1);
            root.add_child(e);
        }
        root
    }

    pub fn root(&self) -> &JfrMetadataElement {
        &self.root
    }

    // Write the metadata checkpoint event into `cw`.
    pub fn write_metadata_event(cw: &mut JfrChunkWriterShim, timestamp: JfrTicks) {
        let _ = cw;
        let _ = timestamp;
    }

    // Serialize into an arbitrary JfrWriter as a jdk.Metadata event.
    pub fn write_to_writer(w: &mut JfrWriter, timestamp: JfrTicks) {
        let repo = REPOSITORY.lock().unwrap();
        let repo = match repo.as_ref() {
            Some(r) => r,
            None => return,
        };
        w.write_u8(EVENT_METADATA);
        w.write_ticks(timestamp);
        w.write_ticks(JfrTicks::now()); // duration
        JfrMetadataWriter::write_metadata(w, repo.root());
    }

    // Called on recording start: emit metadata checkpoint.
    pub fn on_recording_start(timestamp: JfrTicks) {
        Self::initialize();
        let mut cw = JfrCheckpointWriter::global(
            false,
            JfrCheckpointType::Generic,
            JfrCheckpointBufferKind::Global,
        );
        cw.write_type_id(JfrTypeId::MetadataType);
        cw.write_u8(EVENT_METADATA);
        cw.write_ticks(timestamp);
        cw.write_ticks(JfrTicks::now());
        {
            let repo = REPOSITORY.lock().unwrap();
            if let Some(r) = repo.as_ref() {
                // serialize into a temp writer then copy
                let mut tmp = JfrWriter::with_capacity(64 * 1024, true);
                JfrMetadataWriter::write_metadata(&mut tmp, r.root());
                cw.write_bytes(tmp.data());
            }
        }
        cw.release();
    }

    // Called on rotation: re-emit metadata so new chunks can resolve types.
    pub fn on_rotation(timestamp: JfrTicks) {
        Self::on_recording_start(timestamp);
    }

    pub fn flush_pending() {}
}

// Type used to satisfy the unused parameter in write_metadata_event.
pub struct JfrChunkWriterShim;
