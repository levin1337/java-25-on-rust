/*
 * OpenJDK 25 HotSpot jfr/writers/jfrEventWriter.hpp (helper)
 *
 * Shared event-write protocol used by native and java event writers:
 * begin_event_write reserves the size slot, payload is written through
 * JfrWriter, end_event_write patches or aborts.
 */

use crate::share::jfr::writers::jfr_writer::JfrWriter;

pub struct JfrEventWriter<'a> {
    writer: &'a mut JfrWriter,
    large: bool,
    started: bool,
}

impl<'a> JfrEventWriter<'a> {
    pub fn begin(writer: &'a mut JfrWriter, large: bool) -> JfrEventWriter<'a> {
        writer.begin_event_write(large);
        JfrEventWriter {
            writer,
            large,
            started: true,
        }
    }

    pub fn writer_mut(&mut self) -> &mut JfrWriter {
        self.writer
    }

    pub fn is_valid(&self) -> bool {
        self.writer.is_valid()
    }

    // Returns bytes written for the event; 0 if aborted.
    pub fn end(mut self) -> i64 {
        self.started = false;
        self.writer.end_event_write(self.large)
    }
}

impl<'a> Drop for JfrEventWriter<'a> {
    fn drop(&mut self) {
        if self.started {
            let _ = self.writer.end_event_write(self.large);
        }
    }
}
