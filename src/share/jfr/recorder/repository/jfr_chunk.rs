/*
 * OpenJDK 25 HotSpot jfr/recorder/repository/jfrChunk.hpp + jfrChunk.cpp port
 */

use crate::share::jfr::utilities::jfr_time::{nanos_now, JfrTicks};
use crate::share::jfr::utilities::jfr_time_converter;

pub const COMPLETE: u8 = 0;
pub const GUARD: u8 = 0xff;
pub const PAD: u8 = 0;

const MAGIC: &[u8; 3] = b"FLR";
pub const JFR_VERSION_MAJOR: u16 = 2;
pub const JFR_VERSION_MINOR: u16 = 1;

// JfrChunk — chunk file state: timestamps, offsets, generation counter.
pub struct JfrChunk {
    pub path: Option<String>,
    start_ticks: i64,
    previous_start_ticks: i64,
    start_nanos: i64,
    previous_start_nanos: i64,
    last_update_nanos: i64,
    last_checkpoint_offset: i64,
    last_metadata_offset: i64,
    generation: u8,
    final_: bool,
}

impl JfrChunk {
    pub fn new() -> JfrChunk {
        JfrChunk {
            path: None,
            start_ticks: 0,
            previous_start_ticks: crate::share::jfr::utilities::jfr_types::INVALID_TIME,
            start_nanos: 0,
            previous_start_nanos: crate::share::jfr::utilities::jfr_types::INVALID_TIME,
            last_update_nanos: 0,
            last_checkpoint_offset: 0,
            last_metadata_offset: 0,
            generation: 1,
            final_: false,
        }
    }

    pub fn reset(&mut self) {
        self.path = None;
        self.last_checkpoint_offset = 0;
        self.last_metadata_offset = 0;
        self.generation = 1;
        self.final_ = false;
    }

    pub fn magic(&self) -> &'static [u8; 3] {
        MAGIC
    }

    pub fn major_version(&self) -> u16 {
        JFR_VERSION_MAJOR
    }

    pub fn minor_version(&self) -> u16 {
        JFR_VERSION_MINOR
    }

    pub fn mark_final(&mut self) {
        self.final_ = true;
    }

    pub fn flags(&self) -> u16 {
        let mut flags: u16 = 0;
        // CompressedIntegers capability — JfrOptionSet::compressed_integers()
        if crate::share::jfr::recorder::service::jfr_option_set::compressed_integers() {
            flags |= 1 << 0;
        }
        if self.final_ {
            flags |= 1 << 1;
        }
        flags
    }

    pub fn cpu_frequency(&self) -> i64 {
        crate::share::jfr::utilities::jfr_time::JfrTime::frequency()
    }

    pub fn set_last_checkpoint_offset(&mut self, offset: i64) {
        self.last_checkpoint_offset = offset;
    }

    pub fn last_checkpoint_offset(&self) -> i64 {
        self.last_checkpoint_offset
    }

    pub fn start_ticks(&self) -> i64 {
        self.start_ticks
    }

    pub fn start_nanos(&self) -> i64 {
        self.start_nanos
    }

    pub fn previous_start_ticks(&self) -> i64 {
        self.previous_start_ticks
    }

    pub fn previous_start_nanos(&self) -> i64 {
        self.previous_start_nanos
    }

    fn update_start_ticks(&mut self) {
        self.start_ticks = JfrTicks::now().value();
    }

    fn update_start_nanos(&mut self) {
        let now = nanos_now();
        debug_assert!(now >= self.start_nanos);
        debug_assert!(now >= self.last_update_nanos);
        self.start_nanos = now;
        self.last_update_nanos = now;
    }

    fn update_current_nanos(&mut self) {
        let now = nanos_now();
        debug_assert!(now >= self.last_update_nanos);
        self.last_update_nanos = now;
    }

    fn save_current_and_update_start_ticks(&mut self) {
        self.previous_start_ticks = self.start_ticks;
        self.update_start_ticks();
    }

    fn save_current_and_update_start_nanos(&mut self) {
        self.previous_start_nanos = self.start_nanos;
        self.update_start_nanos();
    }

    pub fn set_time_stamp(&mut self) {
        self.save_current_and_update_start_nanos();
        self.save_current_and_update_start_ticks();
    }

    pub fn last_chunk_duration(&self) -> i64 {
        self.start_nanos - self.previous_start_nanos
    }

    pub fn set_path(&mut self, path: Option<String>) {
        self.path = path;
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn is_started(&self) -> bool {
        self.start_nanos != 0
    }

    pub fn is_finished(&self) -> bool {
        self.generation == 0
    }

    pub fn duration(&self) -> i64 {
        debug_assert!(self.last_update_nanos >= self.start_nanos);
        self.last_update_nanos - self.start_nanos
    }

    pub fn last_metadata_offset(&self) -> i64 {
        self.last_metadata_offset
    }

    pub fn set_last_metadata_offset(&mut self, offset: i64) {
        debug_assert!(offset > self.last_metadata_offset);
        self.last_metadata_offset = offset;
    }

    pub fn has_metadata(&self) -> bool {
        self.last_metadata_offset != 0
    }

    // strictly monotone generation counter
    pub fn generation_pub(&mut self) -> u8 {
        self.generation()
    }

    // strictly monotone generation counter
    pub fn generation(&mut self) -> u8 {
        debug_assert!(self.generation > 0);
        let this = self.generation;
        self.generation = self.generation.wrapping_add(1);
        if self.generation == GUARD {
            self.generation = 1;
        }
        this
    }

    pub fn next_generation(&self) -> u8 {
        debug_assert!(self.generation > 0);
        if self.generation == GUARD {
            1
        } else {
            self.generation
        }
    }

    pub fn update_current_nanos_pub(&mut self) {
        self.update_current_nanos();
    }

    pub fn update_start_nanos_pub(&mut self) {
        self.update_start_nanos();
    }

    pub fn update_start_ticks_pub(&mut self) {
        self.update_start_ticks();
    }
}

impl Default for JfrChunk {
    fn default() -> Self {
        Self::new()
    }
}


