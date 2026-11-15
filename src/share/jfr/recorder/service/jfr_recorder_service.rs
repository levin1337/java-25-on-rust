/*
 * OpenJDK 25 HotSpot jfr/recorder/service/jfrRecorderService.hpp + .cpp port
 *
 * JfrRecorderService — the recorder-service state machine driving
 * start/stop/dump/checkpoint/rotation. In C++ this lives inside the
 * recorder thread's run loop; here it is a state object invoked by
 * JfrRecorderThread.
 */

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::share::jfr::metadata::jfr_metadata_repository::JfrMetadataRepository;
use crate::share::jfr::periodic::jfr_periodic_event_set::JfrPeriodicEventSet;
use crate::share::jfr::recorder::checkpoint::jfr_checkpoint_manager::JfrCheckpointManager;
use crate::share::jfr::recorder::repository::jfr_chunk_rotation::JfrChunkRotation;
use crate::share::jfr::recorder::repository::jfr_emergency_dump;
use crate::share::jfr::recorder::repository::jfr_repository::JfrRepository;
use crate::share::jfr::recorder::service::jfr_option_set::{global_get_option, global_set_option};
use crate::share::jfr::recorder::service::jfr_post_box::{JfrPostBox, JfrPostBoxMsg};
use crate::share::jfr::recorder::service::jfr_recorder_thread::JfrRecorderThread;
use crate::share::jfr::utilities::jfr_time::JfrTicks;

static STARTING: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static DUMP_PATH: Mutex<Option<String>> = Mutex::new(None);
static DUMP_ON_EXIT: AtomicBool = AtomicBool::new(false);
static EMERGENCY_DUMP: AtomicU64 = AtomicU64::new(0);

pub struct JfrRecorderService;

impl JfrRecorderService {
    pub fn initialize() -> bool {
        JfrRepository::initialize()
    }

    pub fn is_recording() -> bool {
        JfrPostBox::is_recording()
    }

    pub fn is_enabled() -> bool {
        JfrPostBox::is_enabled()
    }

    pub fn is_shutdown() -> bool {
        SHUTDOWN.load(Ordering::Acquire)
    }

    pub fn is_starting() -> bool {
        STARTING.load(Ordering::Acquire)
    }

    pub fn is_started() -> bool {
        STARTED.load(Ordering::Acquire)
    }

    // ---- VM-facing API (called from jfr.rs) ----

    pub fn on_create_jvm() {}

    pub fn on_start(jmx: bool, environment_controller: bool) -> bool {
        let _ = (jmx, environment_controller);
        if Self::is_started() || STARTING.load(Ordering::Acquire) {
            return false;
        }
        STARTING.store(true, Ordering::Release);
        // open first chunk
        let ok = JfrRepository::with_chunkwriter(|cw| {
            cw.set_path(None);
            cw.open()
        });
        if !ok {
            STARTING.store(false, Ordering::Release);
            return false;
        }
        // write initial checkpoint: metadata + statics + threads
        JfrCheckpointManager::do_checkpoint(JfrTicks::now(), true, false, false);
        JfrPostBox::set_is_recording(true);
        JfrPostBox::set_is_enabled(true);
        STARTED.store(true, Ordering::Release);
        STARTING.store(false, Ordering::Release);
        true
    }

    pub fn on_stop() -> bool {
        if !Self::is_recording() {
            return false;
        }
        JfrPostBox::set_is_recording(false);
        // final checkpoint then close chunk
        JfrCheckpointManager::do_checkpoint(JfrTicks::now(), false, true, false);
        let _ = JfrRepository::close_chunk();
        JfrCheckpointManager::on_stop();
        JfrPeriodicEventSet::on_stop();
        { let mut g = JfrMetadataRepository::instance().lock().unwrap(); if let Some(r) = g.as_mut() { JfrMetadataRepository::flush_pending(); } }
        STARTED.store(false, Ordering::Release);
        true
    }

    pub fn dump_repository() -> bool {
        *DUMP_PATH.lock().unwrap() = None;
        JfrRecorderThread::request_flushpoint();
        true
    }

    pub fn dump_repository_on_exit(_name: &str) -> bool {
        DUMP_ON_EXIT.store(true, Ordering::Release);
        true
    }

    pub fn on_rotation() {
        JfrChunkRotation::on_rotation();
    }

    pub fn flushpoint() {
        // flushpoint = write header checkpoint marking a safe flush point
        JfrCheckpointManager::do_checkpoint(JfrTicks::now(), false, false, true);
        let _ = JfrRepository::flush_chunk();
    }

    pub fn on_chunk_rotation() -> bool {
        // rotate: close current chunk and open the next
        JfrRepository::close_chunk();
        let ok = JfrRepository::with_chunkwriter(|cw| cw.open());
        if ok {
            JfrCheckpointManager::on_rotation();
            JfrPeriodicEventSet::on_rotation();
        }
        ok
    }

    pub fn emergency_dump() -> u64 {
        EMERGENCY_DUMP.fetch_add(1, Ordering::Relaxed)
    }

    pub fn on_emergency_dump(exception_handler: bool, oom: bool) {
        jfr_emergency_dump::on_vm_shutdown(exception_handler, oom);
        EMERGENCY_DUMP.store(1, Ordering::Release);
    }

    pub fn on_vm_start() -> bool {
        Self::on_start(false, false)
    }

    pub fn on_vm_init() -> bool {
        true
    }

    pub fn on_shutdown() {
        SHUTDOWN.store(true, Ordering::Release);
        if Self::is_recording() {
            let _ = Self::on_stop();
        }
        if DUMP_ON_EXIT.load(Ordering::Acquire) {
            let _ = Self::dump_repository();
        }
    }

    // ---- recorder thread loop body ----

    pub fn run() -> bool {
        JfrRecorderThread::start(|msg| Self::handle_message(msg));
        true
    }

    fn handle_message(msg: JfrPostBoxMsg) {
        match msg {
            JfrPostBoxMsg::MsgChunkRotation => {
                let _ = Self::on_chunk_rotation();
            }
            JfrPostBoxMsg::MsgFlushPoint => {
                Self::flushpoint();
            }
            JfrPostBoxMsg::MsgCheckpointRequest => {
                JfrCheckpointManager::do_checkpoint(JfrTicks::now(), false, false, false);
            }
            JfrPostBoxMsg::MsgShutdown => {
                let _ = Self::on_stop();
            }
            JfrPostBoxMsg::MsgStartRecording => {
                let _ = Self::on_start(false, false);
            }
            JfrPostBoxMsg::MsgStopRecording => {
                let _ = Self::on_stop();
            }
            JfrPostBoxMsg::MsgDump => {
                let _ = Self::dump_repository();
            }
            _ => {}
        }
    }
}
