/*
 * Port of OpenJDK 25 HotSpot CDSConfig (share/cds/cdsConfig.hpp)
 *
 * CDSConfig controls Class Data Sharing (CDS) state and configuration modes
 * (dumping archive, using archive, dynamic archive, AOT).
 */

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// CDS mode bits mirroring CDSConfig's mode flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdsMode {
    Off,
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
static USING_STATIC: AtomicBool = AtomicBool::new(false);
static USING_DYNAMIC: AtomicBool = AtomicBool::new(false);
static AOT_ENABLED: AtomicBool = AtomicBool::new(false);
static MODE: AtomicU8 = AtomicU8::new(CdsMode::Off as u8);

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
            1 => CdsMode::DumpStatic,
            2 => CdsMode::DumpDynamic,
            3 => CdsMode::DumpAot,
            4 => CdsMode::UseStatic,
            5 => CdsMode::UseDynamic,
            _ => CdsMode::Off,
        }
    }

    /// cdsConfig::set_mode / -Xshare:{off,on,dump,auto,aot}.
    pub fn set_mode(mode: CdsMode) {
        MODE.store(mode as u8, Ordering::Relaxed);
        match mode {
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
        AOT_ENABLED.load(Ordering::Relaxed) && Self::is_using_archive()
    }
}
