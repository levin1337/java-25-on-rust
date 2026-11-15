/*
 * Port of OpenJDK 25 HotSpot CDSConfig (share/cds/cdsConfig.hpp)
 *
 * CDSConfig controls Class Data Sharing (CDS) state and configuration modes
 * (dumping archive, using archive, dynamic archive, AOT).
 */

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::RwLock;

/// CDS mode bits mirroring CDSConfig's mode flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdsMode {
    Off,
    /// -Xshare:auto — use the archive if usable, continue without CDS
    /// otherwise (cdsConfig.cpp: AutoCreateSharedArchive-era semantics of
    /// the classic -Xshare:auto).
    Auto,
    DumpStatic,
    DumpDynamic,
    DumpAot,
    UseStatic,
    UseDynamic,
}

pub struct CDSConfig;

static DUMPING_ARCHIVE: AtomicBool = AtomicBool::new(false);
static USING_ARCHIVE: AtomicBool = AtomicBool::new(false);
static DUMPING_STATIC: AtomicBool = AtomicBool::new(false);
static DUMPING_DYNAMIC: AtomicBool = AtomicBool::new(false);
static DUMPING_PREIMAGE: AtomicBool = AtomicBool::new(false);
static USING_STATIC: AtomicBool = AtomicBool::new(false);
static USING_DYNAMIC: AtomicBool = AtomicBool::new(false);
static USING_PREIMAGE: AtomicBool = AtomicBool::new(false);
static AOT_ENABLED: AtomicBool = AtomicBool::new(false);
static AUTO_MODE: AtomicBool = AtomicBool::new(false);
static ARCHIVE_LOADING_FAILED: AtomicBool = AtomicBool::new(false);
static USE_SHARED_SPACES: AtomicBool = AtomicBool::new(true);
static AUTO_CREATE_SHARED_ARCHIVE: AtomicBool = AtomicBool::new(false);
static RECORD_DYNAMIC_DUMP_INFO: AtomicBool = AtomicBool::new(false);
static MODE: AtomicU8 = AtomicU8::new(CdsMode::Off as u8);

static SHARED_ARCHIVE_FILE: RwLock<Option<PathBuf>> = RwLock::new(None);
static ARCHIVE_CLASSES_AT_EXIT: RwLock<Option<PathBuf>> = RwLock::new(None);
static SHARED_CLASS_LIST_FILE: RwLock<Option<PathBuf>> = RwLock::new(None);

impl CDSConfig {
    pub(crate) fn initialize_from_archive() -> std::io::Result<()> {
        if Self::is_using_archive() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Restoring archived Java objects is unsupported",
            ));
        }
        Ok(())
    }

    // -- generic archive state -------------------------------------------------

    #[inline]
    pub fn is_dumping_archive() -> bool {
        DUMPING_ARCHIVE.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_dumping_archive(dumping: bool) {
        DUMPING_ARCHIVE.store(dumping, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_using_archive() -> bool {
        USING_ARCHIVE.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn set_using_archive(using: bool) {
        USING_ARCHIVE.store(using, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_sharing_enabled() -> bool {
        Self::is_dumping_archive() || Self::is_using_archive()
    }

    // -- mode-specific state (cdsConfig.hpp) ------------------------------------

    #[inline]
    pub fn mode() -> CdsMode {
        match MODE.load(Ordering::Relaxed) {
            1 => CdsMode::Auto,
            2 => CdsMode::DumpStatic,
            3 => CdsMode::DumpDynamic,
            4 => CdsMode::DumpAot,
            5 => CdsMode::UseStatic,
            6 => CdsMode::UseDynamic,
            _ => CdsMode::Off,
        }
    }

    /// Parse a -Xshare:{off,on,auto,dump,aot} value (cdsConfig.cpp
    /// set_mode_from_flag / Arguments::parse_shared_option).
    pub fn xshare_mode(arg: &str) -> Option<CdsMode> {
        match arg.to_ascii_lowercase().as_str() {
            "off" => Some(CdsMode::Off),
            "on" => Some(CdsMode::UseStatic),
            "auto" => Some(CdsMode::Auto),
            "dump" => Some(CdsMode::DumpStatic),
            "aot" => Some(CdsMode::DumpAot),
            _ => None,
        }
    }

    /// cdsConfig::set_mode / -Xshare:{off,on,dump,auto,aot}.
    pub fn set_mode(mode: CdsMode) {
        MODE.store(mode as u8, Ordering::Relaxed);
        match mode {
            CdsMode::Auto => {
                AUTO_MODE.store(true, Ordering::Relaxed);
                USING_STATIC.store(true, Ordering::Relaxed);
                USING_ARCHIVE.store(true, Ordering::Relaxed);
            }
            CdsMode::DumpStatic => {
                DUMPING_STATIC.store(true, Ordering::Relaxed);
                DUMPING_ARCHIVE.store(true, Ordering::Relaxed);
            }
            CdsMode::DumpDynamic => {
                DUMPING_DYNAMIC.store(true, Ordering::Relaxed);
                DUMPING_ARCHIVE.store(true, Ordering::Relaxed);
            }
            CdsMode::DumpAot => {
                AOT_ENABLED.store(true, Ordering::Relaxed);
                DUMPING_ARCHIVE.store(true, Ordering::Relaxed);
            }
            CdsMode::UseStatic => {
                USING_STATIC.store(true, Ordering::Relaxed);
                USING_ARCHIVE.store(true, Ordering::Relaxed);
            }
            CdsMode::UseDynamic => {
                USING_DYNAMIC.store(true, Ordering::Relaxed);
                USING_ARCHIVE.store(true, Ordering::Relaxed);
            }
            CdsMode::Off => {}
        }
    }

    #[inline]
    pub fn is_dumping_static_archive() -> bool {
        DUMPING_STATIC.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn is_dumping_dynamic_archive() -> bool {
        DUMPING_DYNAMIC.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn is_dumping_final_static_archive() -> bool {
        // True while writing the final static archive image (not a preimage).
        Self::is_dumping_static_archive() && !AOT_ENABLED.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn is_using_static_archive() -> bool {
        USING_STATIC.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn is_using_dynamic_archive() -> bool {
        USING_DYNAMIC.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn is_aot_enabled() -> bool {
        AOT_ENABLED.load(Ordering::Relaxed)
    }

    /// cdsConfig::is_using_preimage — AOT preimage archive in use.
    pub fn is_using_preimage() -> bool {
        (AOT_ENABLED.load(Ordering::Relaxed) || USING_PREIMAGE.load(Ordering::Relaxed))
            && Self::is_using_archive()
    }

    #[inline]
    pub fn is_dumping_preimage_static_archive() -> bool {
        DUMPING_PREIMAGE.load(Ordering::Relaxed)
    }

    // -- -XX: flag state (cdsConfig.cpp flag table) ----------------------------

    /// -XX:SharedArchiveFile=path — archive to use or produce.
    pub fn set_shared_archive_file(path: Option<PathBuf>) {
        *SHARED_ARCHIVE_FILE.write().unwrap() = path;
    }

    pub fn shared_archive_file() -> Option<PathBuf> {
        SHARED_ARCHIVE_FILE.read().unwrap().clone()
    }

    /// -XX:SharedClassListFile=path — class list for -Xshare:dump.
    pub fn set_shared_class_list_file(path: Option<PathBuf>) {
        *SHARED_CLASS_LIST_FILE.write().unwrap() = path;
    }

    pub fn shared_class_list_file() -> Option<PathBuf> {
        SHARED_CLASS_LIST_FILE.read().unwrap().clone()
    }

    /// -XX:ArchiveClassesAtExit=path — dynamic archive written at exit.
    pub fn set_archive_classes_at_exit(path: Option<PathBuf>) {
        *ARCHIVE_CLASSES_AT_EXIT.write().unwrap() = path;
    }

    pub fn archive_classes_at_exit() -> Option<PathBuf> {
        ARCHIVE_CLASSES_AT_EXIT.read().unwrap().clone()
    }

    /// -XX:+/-UseSharedSpaces (default: on).
    #[inline]
    pub fn use_shared_spaces() -> bool {
        USE_SHARED_SPACES.load(Ordering::Relaxed)
    }

    pub fn set_use_shared_spaces(v: bool) {
        USE_SHARED_SPACES.store(v, Ordering::Relaxed);
    }

    /// -XX:+AutoCreateSharedArchive.
    #[inline]
    pub fn auto_create_shared_archive() -> bool {
        AUTO_CREATE_SHARED_ARCHIVE.load(Ordering::Relaxed)
    }

    pub fn set_auto_create_shared_archive(v: bool) {
        AUTO_CREATE_SHARED_ARCHIVE.store(v, Ordering::Relaxed);
    }

    /// -XX:+RecordDynamicDumpInfo.
    #[inline]
    pub fn record_dynamic_dump_info() -> bool {
        RECORD_DYNAMIC_DUMP_INFO.load(Ordering::Relaxed)
    }

    pub fn set_record_dynamic_dump_info(v: bool) {
        RECORD_DYNAMIC_DUMP_INFO.store(v, Ordering::Relaxed);
    }

    #[inline]
    pub fn archive_loading_failed() -> bool {
        ARCHIVE_LOADING_FAILED.load(Ordering::Relaxed)
    }

    pub fn set_archive_loading_failed() {
        ARCHIVE_LOADING_FAILED.store(true, Ordering::Relaxed);
    }

    /// cdsConfig::is_using_archive resolution for -Xshare:auto: the archive
    /// is attempted first; on failure CDS is disabled and the VM continues.
    /// For -Xshare:on the failure is fatal (caller must abort).
    pub fn archive_error_is_fatal() -> bool {
        Self::is_using_archive() && !AUTO_MODE.load(Ordering::Relaxed)
    }

    /// Called when archive loading fails under -Xshare:auto: disable CDS
    /// and let the VM continue without shared spaces.
    pub fn disable_archive_after_failure() {
        ARCHIVE_LOADING_FAILED.store(true, Ordering::Relaxed);
        USING_ARCHIVE.store(false, Ordering::Relaxed);
        USING_STATIC.store(false, Ordering::Relaxed);
        USING_DYNAMIC.store(false, Ordering::Relaxed);
    }

    /// Resolve the effective archive file path: SharedArchiveFile if set,
    /// otherwise the default CDSArchive location is unknown to this port —
    /// callers pass the resolved path explicitly.
    pub fn effective_archive_path(default: PathBuf) -> PathBuf {
        Self::shared_archive_file().unwrap_or(default)
    }

    /// cdsConfig::is_dumping_heap — the Java-heap region is only dumped in
    /// full-archive mode; this port does not materialize heap objects.
    #[inline]
    pub fn is_dumping_heap() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        CDSConfig::set_mode(CdsMode::Off);
        AUTO_MODE.store(false, Ordering::Relaxed);
        ARCHIVE_LOADING_FAILED.store(false, Ordering::Relaxed);
        USE_SHARED_SPACES.store(true, Ordering::Relaxed);
        CDSConfig::set_shared_archive_file(None);
        CDSConfig::set_archive_classes_at_exit(None);
        CDSConfig::set_shared_class_list_file(None);
    }

    #[test]
    fn xshare_option_parsing() {
        reset();
        assert_eq!(CDSConfig::xshare_mode("off"), Some(CdsMode::Off));
        assert_eq!(CDSConfig::xshare_mode("on"), Some(CdsMode::UseStatic));
        assert_eq!(CDSConfig::xshare_mode("auto"), Some(CdsMode::Auto));
        assert_eq!(CDSConfig::xshare_mode("dump"), Some(CdsMode::DumpStatic));
        assert_eq!(CDSConfig::xshare_mode("aot"), Some(CdsMode::DumpAot));
        assert_eq!(CDSConfig::xshare_mode("bogus"), None);
        assert_eq!(CDSConfig::xshare_mode("AUTO"), Some(CdsMode::Auto));
    }

    #[test]
    fn mode_state_transitions() {
        reset();
        CDSConfig::set_mode(CdsMode::DumpStatic);
        assert!(CDSConfig::is_dumping_archive());
        assert!(CDSConfig::is_dumping_static_archive());
        assert!(!CDSConfig::is_using_archive());

        CDSConfig::set_mode(CdsMode::UseStatic);
        assert!(CDSConfig::is_using_archive());
        assert!(CDSConfig::is_using_static_archive());
        assert!(!CDSConfig::archive_error_is_fatal() == false); // on => fatal

        reset();
        CDSConfig::set_mode(CdsMode::Auto);
        assert!(CDSConfig::is_using_archive());
        assert!(!CDSConfig::archive_error_is_fatal());
        CDSConfig::disable_archive_after_failure();
        assert!(!CDSConfig::is_using_archive());
        assert!(CDSConfig::archive_loading_failed());
        reset();
    }

    #[test]
    fn flag_state() {
        reset();
        assert!(CDSConfig::use_shared_spaces());
        CDSConfig::set_use_shared_spaces(false);
        assert!(!CDSConfig::use_shared_spaces());
        CDSConfig::set_shared_archive_file(Some(PathBuf::from("C:/cds/app.jsa")));
        assert_eq!(
            CDSConfig::shared_archive_file().unwrap(),
            PathBuf::from("C:/cds/app.jsa")
        );
        assert_eq!(
            CDSConfig::effective_archive_path(PathBuf::from("default.jsa")),
            PathBuf::from("C:/cds/app.jsa")
        );
        CDSConfig::set_archive_classes_at_exit(Some(PathBuf::from("dyn.jsa")));
        assert_eq!(
            CDSConfig::archive_classes_at_exit().unwrap(),
            PathBuf::from("dyn.jsa")
        );
        reset();
    }
}
