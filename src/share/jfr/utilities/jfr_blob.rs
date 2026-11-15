/*
 * OpenJDK 25 HotSpot jfr/utilities/jfrBlob.hpp port
 */

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// JfrBlob — immutable byte blob with a linked list of follow-on blobs
// and exclusive write-once semantics for the first write pass.
pub struct JfrBlob {
    data: Arc<[u8]>,
    next: parking_lot::Mutex<Option<JfrBlobHandle>>,
    written: AtomicBool,
}

#[derive(Clone)]
pub struct JfrBlobHandle {
    inner: Arc<JfrBlob>,
}

impl JfrBlobHandle {
    pub fn size(&self) -> usize {
        self.inner.data.len()
    }

    pub fn data(&self) -> &[u8] {
        &self.inner.data
    }

    pub fn is_valid(&self) -> bool {
        true
    }

    // JfrBlob::write — write this blob and all chained blobs.
    pub fn write_to<W: JfrBlobWriter>(&self, writer: &mut W) {
        writer.write_bytes(&self.inner.data);
        let next = self.inner.next.lock().clone();
        if let Some(n) = next {
            n.write_to(writer);
        }
    }

    // JfrBlob::exclusive_write — write once only.
    pub fn exclusive_write_to<W: JfrBlobWriter>(&self, writer: &mut W) {
        if self.inner.written.swap(true, Ordering::AcqRel) {
            return;
        }
        writer.write_bytes(&self.inner.data);
        let next = self.inner.next.lock().clone();
        if let Some(n) = next {
            n.exclusive_write_to(writer);
        }
    }

    pub fn set_next(&self, next: JfrBlobHandle) {
        *self.inner.next.lock() = Some(next);
    }

    pub fn reset_write_state(&self) {
        self.inner.written.store(false, Ordering::Release);
    }
}

pub trait JfrBlobWriter {
    fn write_bytes(&mut self, data: &[u8]);
}

impl JfrBlob {
    pub fn make(data: &[u8]) -> JfrBlobHandle {
        JfrBlobHandle {
            inner: Arc::new(JfrBlob {
                data: Arc::from(data),
                next: parking_lot::Mutex::new(None),
                written: AtomicBool::new(false),
            }),
        }
    }
}
