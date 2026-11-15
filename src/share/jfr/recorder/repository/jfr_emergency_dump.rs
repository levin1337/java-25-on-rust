/*
 * OpenJDK 25 HotSpot jfr/recorder/repository/jfrEmergencyDump.hpp + .cpp port
 *
 * Emergency dump on VM error / shutdown: writes a minimal chunk containing
 * whatever is in flight to a crash file.
 */

use std::path::PathBuf;

pub fn chunk_path(repository_path: &str) -> String {
    let mut p = PathBuf::from(repository_path);
    p.push("hotspot-pid");
    // pid placeholder replaced by caller with actual pid when available
    p.to_string_lossy().replace("hotspot-pid", &format!("hs_err_pid{}", std::process::id()))
}

pub fn emergency_file_name() -> String {
    format!("hs_err_pid{}.jfr", std::process::id())
}

// JfrEmergencyDump::on_vm_shutdown — dump in-flight data on abnormal exit.
pub fn on_vm_shutdown(exception_handler: bool, oom: bool) -> bool {
    let _ = (exception_handler, oom);
    // The repository chunk writer finalizes the current chunk; the
    // recorder service performs the actual dump when recording.
    crate::share::jfr::recorder::repository::jfr_repository::JfrRepository::mark_chunk_final();
    true
}

pub fn on_vm_error_report(repository_path: Option<&str>) -> Option<String> {
    repository_path.map(|p| chunk_path(p))
}
