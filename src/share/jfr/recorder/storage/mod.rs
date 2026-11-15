/*
 * OpenJDK 25 HotSpot jfr/recorder/storage port — module index.
 *
 * Full storage.cpp is ~10k lines; this port provides the buffer model
 * (JfrBuffer), storage adapters and the storage type system used by the
 * checkpoint/event writers.
 */

pub mod jfr_buffer;
pub mod jfr_storage_adapter;
pub mod jfr_storage_types;
