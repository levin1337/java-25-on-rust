/*
 * Port of OpenJDK 25 HotSpot CDSConfig (share/cds/cdsConfig.hpp)
 *
 * CDSConfig controls Class Data Sharing (CDS) state and configuration modes
 * (dumping archive, using archive, dynamic archive).
 */

use std::sync::atomic::{AtomicBool, Ordering};

pub struct CDSConfig;

static DUMPING_ARCHIVE: AtomicBool = AtomicBool::new(false);
static USING_ARCHIVE: AtomicBool = AtomicBool::new(false);

impl CDSConfig {
    pub(crate) fn initializeFromArchive() -> std::io::Result<()> {
        if Self::is_using_archive() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Restoring archived Java objects is unsupported",
            ));
        }
        Ok(())
    }

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
}
