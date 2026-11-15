/*
 * OpenJDK 25 HotSpot jfr/jfr.hpp + jfr.cpp port
 *
 * Jfr — global JFR state and lifecycle entry points. All state is
 * delegated to the recorder service / post box singletons.
 */

use crate::share::jfr::recorder::jfr_recorder::JfrRecorder;
use crate::share::jfr::recorder::service::jfr_post_box::JfrPostBox;
use crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService;

pub struct Jfr;

impl Jfr {
    pub fn is_enabled() -> bool {
        JfrPostBox::is_enabled()
    }

    pub fn is_recording() -> bool {
        JfrPostBox::is_recording()
    }

    pub fn is_started() -> bool {
        JfrRecorderService::is_started()
    }

    pub fn on_create_jvm() -> bool {
        JfrRecorder::on_create_jvm()
    }

    pub fn on_start() -> bool {
        JfrRecorder::on_start()
    }

    pub fn on_stop() -> bool {
        JfrRecorder::on_stop()
    }

    pub fn on_shutdown() {
        JfrRecorder::on_shutdown();
    }

    pub fn on_initialize() -> bool {
        JfrRecorder::on_initialize()
    }

    pub fn on_oom() {
        JfrRecorder::on_oom();
    }

    pub fn dump_repository() -> bool {
        JfrRecorderService::dump_repository()
    }

    pub fn dump_repository_on_exit(name: &str) -> bool {
        JfrRecorderService::dump_repository_on_exit(name)
    }

    pub fn emergency_dump() -> u64 {
        JfrRecorderService::emergency_dump()
    }
}
