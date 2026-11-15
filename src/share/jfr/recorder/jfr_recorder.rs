/*
 * OpenJDK 25 HotSpot jfr/recorder/jfrRecorder.hpp port
 *
 * JfrRecorder — top-level recorder object owning repository, service,
 * checkpoint manager state. In this port most state is global/singleton;
 * this struct wires the lifecycle.
 */

pub struct JfrRecorder;

impl JfrRecorder {
    pub fn is_vm_initialized() -> bool {
        crate::share::jfr::utilities::jfr_types::vm_initialized()
    }

    pub fn is_creating_vm() -> bool {
        false
    }

    pub fn on_create_jvm() -> bool {
        crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService::on_create_jvm();
        true
    }

    pub fn on_start() -> bool {
        crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService::on_vm_start()
    }

    pub fn on_stop() -> bool {
        crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService::on_stop()
    }

    pub fn on_rotation() {
        crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService::on_rotation();
    }

    pub fn on_shutdown() {
        crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService::on_shutdown();
    }

    pub fn on_initialize() -> bool {
        crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService::initialize()
    }

    pub fn on_oom() {
        crate::share::jfr::recorder::service::jfr_recorder_service::JfrRecorderService::on_emergency_dump(false, true);
    }
}
