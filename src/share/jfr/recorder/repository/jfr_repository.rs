/*
 * OpenJDK 25 HotSpot jfr/recorder/repository/jfrRepository.hpp + .cpp port
 *
 * JfrRepository — location on disk where chunk files are stored.
 * Singleton holding the repository path and the current JfrChunkWriter.
 */

use std::path::PathBuf;
use std::sync::Mutex;

use crate::share::jfr::recorder::repository::jfr_chunk_writer::JfrChunkWriter;
use crate::share::jfr::recorder::repository::jfr_chunk_rotation::JfrChunkRotation;
use crate::share::jfr::recorder::service::jfr_post_box::JfrPostBox;

pub struct JfrRepository {
    path: Option<String>,
    chunkwriter: Mutex<JfrChunkWriter>,
}

static INSTANCE: std::sync::OnceLock<JfrRepository> = std::sync::OnceLock::new();

impl JfrRepository {
    fn instance() -> &'static JfrRepository {
        INSTANCE.get_or_init(|| JfrRepository {
            path: None,
            chunkwriter: Mutex::new(JfrChunkWriter::new()),
        })
    }

    pub fn initialize() -> bool {
        true
    }

    // Access the chunk writer. Callers must not hold the lock across
    // recorder-service operations that also touch the post box.
    pub fn with_chunkwriter<R>(f: impl FnOnce(&mut JfrChunkWriter) -> R) -> R {
        let mut cw = Self::instance().chunkwriter.lock().unwrap();
        f(&mut cw)
    }

    pub fn set_path(&mut self, path: &str) -> bool {
        self.path = Some(path.to_string());
        true
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    fn set_chunk_path(&mut self, path: Option<&str>) {
        Self::with_chunkwriter(|cw| cw.set_path(path.map(|s| s.to_string())));
    }

    fn notify_on_new_chunk_path() {
        // rotations are synchronous; in this port the recorder service
        // drains the post box on its thread.
    }

    pub fn set_chunk_path_static(path: Option<&str>) {
        // Repository::set_chunk_path — open next chunk at path.
        let repo = Self::instance();
        // SAFETY-FREE version: perform through the mutex.
        {
            let mut cw = repo.chunkwriter.lock().unwrap();
            cw.set_path(path.map(|s| s.to_string()));
        }
        Self::notify_on_new_chunk_path();
    }

    pub fn mark_chunk_final() {
        Self::with_chunkwriter(|cw| cw.mark_chunk_final());
    }

    pub fn current_chunk_start_nanos() -> i64 {
        Self::with_chunkwriter(|cw| cw.current_chunk_start_nanos())
    }

    pub fn open_chunk(&self, vm_error: bool) -> bool {
        Self::with_chunkwriter(|cw| {
            if vm_error {
                if let Some(p) = self.path.clone() {
                    cw.set_path(Some(crate::share::jfr::recorder::repository::jfr_emergency_dump::chunk_path(&p)));
                }
            }
            cw.open()
        })
    }

    pub fn close_chunk() -> usize {
        Self::with_chunkwriter(|cw| cw.close() as usize)
    }

    pub fn flush_chunk() -> usize {
        Self::with_chunkwriter(|cw| cw.flush_chunk(true) as usize)
    }

    pub fn flush(_post_box: &JfrPostBox) {
        // Repository::flush posts MSG_FLUSHPOINT to the recorder thread.
    }

    pub fn evaluate_chunk_size_for_rotation() {
        Self::with_chunkwriter(|cw| {
            JfrChunkRotation::evaluate(cw.size_written());
        });
    }

    pub fn repository_path() -> Option<PathBuf> {
        Self::instance().path.clone().map(PathBuf::from)
    }
}
