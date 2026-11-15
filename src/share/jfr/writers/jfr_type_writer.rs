/*
 * OpenJDK 25 HotSpot jfr/writers/jfrTypeWriterHost.hpp port
 *
 * JfrTypeWriterHost — RAII helper that writes a type id header followed by
 * a count of written elements; on drop with zero elements the context is
 * rewound.
 */

use crate::share::jfr::recorder::checkpoint::jfr_checkpoint_writer::JfrCheckpointWriter;
use crate::share::jfr::utilities::jfr_types::{JfrCheckpointContext, JfrTypeId};

pub struct JfrTypeWriterHost<'a> {
    writer: &'a mut JfrCheckpointWriter,
    count: i32,
    count_offset: i64,
    skip_header: bool,
    ctx: JfrCheckpointContext,
}

impl<'a> JfrTypeWriterHost<'a> {
    pub fn new(
        writer: &'a mut JfrCheckpointWriter,
        type_id: JfrTypeId,
        class_unload: bool,
        skip_header: bool,
    ) -> JfrTypeWriterHost<'a> {
        let _ = class_unload;
        let ctx = writer.context();
        let mut host = JfrTypeWriterHost {
            writer,
            count: 0,
            count_offset: -1,
            skip_header,
            ctx,
        };
        if !skip_header {
            host.writer.write_type_id(type_id);
            host.count_offset = host.writer.reserve(4);
        }
        host
    }

    // Write one element; returns number of entries the element accounts for
    // (usually 1, more for collapsed multi-entry writes).
    pub fn element(&mut self, f: impl FnOnce(&mut JfrCheckpointWriter) -> i32) -> i32 {
        let n = f(self.writer);
        self.count += n;
        n
    }

    pub fn element_unit(&mut self, f: impl FnOnce(&mut JfrCheckpointWriter)) {
        f(self.writer);
        self.count += 1;
    }

    pub fn add(&mut self, count: i32) {
        self.count += count;
    }

    pub fn count(&self) -> i32 {
        self.count
    }

    pub fn writer_mut(&mut self) -> &mut JfrCheckpointWriter {
        self.writer
    }

    fn finish_inner(&mut self) {
        if self.count == 0 {
            // nothing written, restore context for rewind
            self.writer.set_context(self.ctx);
            return;
        }
        if !self.skip_header {
            self.writer.write_count_at(self.count as u32, self.count_offset);
        }
    }
}

impl<'a> Drop for JfrTypeWriterHost<'a> {
    fn drop(&mut self) {
        self.finish_inner();
    }
}
