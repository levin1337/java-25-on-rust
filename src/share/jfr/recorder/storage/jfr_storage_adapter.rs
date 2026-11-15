/*
 * OpenJDK 25 HotSpot jfr/recorder/storage/jfrStorageAdapter.hpp port
 *
 * JfrStorageAdapter — base storage abstraction. In the full C++ port
 * this hosts the buffer lease/commit machinery per storage type; here it
 * provides the common adapter interface used by writers.
 */

use crate::share::jfr::recorder::storage::jfr_buffer::JfrBuffer;
use crate::share::jfr::recorder::storage::jfr_storage_types::JfrStorage;

pub struct JfrStorageAdapter {
    storage: JfrStorage,
}

impl JfrStorageAdapter {
    pub fn new(storage: JfrStorage) -> Self {
        JfrStorageAdapter { storage }
    }

    pub fn storage(&self) -> JfrStorage {
        self.storage
    }

    // acquire a buffer of the given kind for this storage
    pub fn acquire_buffer(&self, kind: crate::share::jfr::utilities::jfr_types::JfrCheckpointBufferKind) -> Option<JfrBuffer> {
        match self.storage {
            JfrStorage::Global => {
                crate::share::jfr::recorder::checkpoint::jfr_checkpoint_manager::JfrCheckpointManager::acquire_global_buffer(kind)
            }
            _ => crate::share::jfr::recorder::checkpoint::jfr_checkpoint_manager::JfrCheckpointManager::acquire_thread_local_buffer(),
        }
    }
}
