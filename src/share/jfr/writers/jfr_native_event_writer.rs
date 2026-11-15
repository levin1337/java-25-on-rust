/*
 * OpenJDK 25 HotSpot jfr/writers/jfrNativeEventWriter.hpp port
 *
 * JfrNativeEventWriter — EventWriterHost over the thread-local native
 * JfrBuffer (Adapter<JfrFlush> equivalent). Events that do not fit are
 * aborted exactly like the C++ small/large event protocol.
 */

use crate::share::jfr::recorder::storage::jfr_buffer::JfrBuffer;
use crate::share::jfr::utilities::jfr_types::TraceId;
use crate::share::jfr::writers::jfr_writer::JfrWriter;

// Write-side view of a thread-local native buffer. The JfrWriter grows the
// backing storage on demand (equivalent of JfrFlush -> JfrStorage::flush
// migrating outstanding writes into a larger buffer).
pub struct JfrNativeEventWriter {
    writer: JfrWriter,
    pub thread: TraceId,
}

impl JfrNativeEventWriter {
    pub fn new(buffer: JfrBuffer, thread: TraceId, compressed_integers: bool) -> JfrNativeEventWriter {
        JfrNativeEventWriter {
            writer: JfrWriter::with_capacity(buffer.size(), compressed_integers),
            thread,
        }
    }

    pub fn over_vec(size: usize, thread: TraceId, compressed_integers: bool) -> JfrNativeEventWriter {
        JfrNativeEventWriter {
            writer: JfrWriter::with_capacity(size, compressed_integers),
            thread,
        }
    }

    pub fn writer_mut(&mut self) -> &mut JfrWriter {
        &mut self.writer
    }

    pub fn into_inner(self) -> JfrWriter {
        self.writer
    }
}
