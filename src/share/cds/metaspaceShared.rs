/*
 * Port of OpenJDK 25 HotSpot MetaspaceShared (share/cds/metaspaceShared.hpp)
 *
 * MetaspaceShared coordinates dumping and loading of pre-parsed Java classes
 * to and from the CDS shared archive. The on-disk container is the
 * FileMapInfo layout implemented in `archive.rs` (share/include/cds.h
 * GenericCDSFileMapHeader + CDSFileMapRegion records).
 */

use super::archive::{self, CdsRegion, GenericHeader};
use super::cdsConfig::CDSConfig;
use super::cdsConstants::NUM_CDS_REGIONS;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

/// Region indices in the archive (MetaspaceShared::n_regions layout).
pub const RO_REGION: usize = 0;
pub const RW_REGION: usize = 1;
pub const BITMAP_REGION: usize = 2;
pub const FIRST_HEAP_REGION: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedClass {
    pub name: String,
    pub bytecode: Vec<u8>,
}

pub struct MetaspaceShared {
    archived_classes: HashMap<String, ArchivedClass>,
}

impl MetaspaceShared {
    pub fn new() -> Self {
        Self {
            archived_classes: HashMap::new(),
        }
    }

    pub fn add_class(&mut self, name: String, bytecode: Vec<u8>) {
        self.archived_classes
            .insert(name.clone(), ArchivedClass { name, bytecode });
    }

    pub fn get_class(&self, name: &str) -> Option<&ArchivedClass> {
        self.archived_classes.get(name)
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.archived_classes.len()
    }

    /// Serialize the class table into the RO-region payload.
    /// Layout: u32 class_count, then per class { u16 name_len, name,
    /// u32 code_len, code } — the payload format archiveHeapWriter produces
    /// for the metaspace RO region in simplified form.
    pub fn serialize_classes(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.write_all(&(self.archived_classes.len() as u32).to_le_bytes())
            .unwrap();
        // Deterministic order (BTreeMap-like) so archives are reproducible.
        let mut entries: Vec<_> = self.archived_classes.values().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for cls in entries {
            let name_bytes = cls.name.as_bytes();
            body.write_all(&(name_bytes.len() as u16).to_le_bytes())
                .unwrap();
            body.write_all(name_bytes).unwrap();
            body.write_all(&(cls.bytecode.len() as u32).to_le_bytes())
                .unwrap();
            body.write_all(&cls.bytecode).unwrap();
        }
        body
    }

    fn deserialize_classes(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut cursor = io::Cursor::new(data);
        let mut u32_buf = [0u8; 4];
        cursor.read_exact(&mut u32_buf)?;
        let class_count = u32::from_le_bytes(u32_buf) as usize;

        for _ in 0..class_count {
            let mut u16_buf = [0u8; 2];
            cursor.read_exact(&mut u16_buf)?;
            let name_len = u16::from_le_bytes(u16_buf) as usize;
            if name_len > 65535 {
                return Err(io::Error::other("CDS class name too long"));
            }
            let mut name_buf = vec![0u8; name_len];
            cursor.read_exact(&mut name_buf)?;
            let name = String::from_utf8_lossy(&name_buf).to_string();

            cursor.read_exact(&mut u32_buf)?;
            let code_len = u32::from_le_bytes(u32_buf) as usize;
            let mut bytecode = vec![0u8; code_len];
            cursor.read_exact(&mut bytecode)?;

            self.archived_classes
                .insert(name.clone(), ArchivedClass { name, bytecode });
        }
        Ok(self.archived_classes.len())
    }

    /// archiveBuilder.cpp: write the shared archive (static CDS dump).
    pub fn dump_to_file(&self, path: &str) -> io::Result<usize> {
        self.dump_to_path(Path::new(path), None)
    }

    /// Write archive; `base_name` selects dynamic-archive mode.
    pub fn dump_to_path(&self, path: &Path, base_name: Option<&str>) -> io::Result<usize> {
        CDSConfig::set_dumping_archive(true);
        let result = (|| -> io::Result<usize> {
            let payload = self.serialize_classes();
            let mut file = File::create(path)?;

            let header = match base_name {
                Some(name) => GenericHeader::dynamic_archive(name),
                None => GenericHeader::static_archive(),
            };

            let mut regions = vec![CdsRegion::default(); NUM_CDS_REGIONS];
            // RO metaspace region carries the class table.
            regions[RO_REGION] = CdsRegion {
                read_only: true,
                allow_exec: false,
                is_heap_region: false,
                is_bitmap_region: false,
                mapped_from_file: true,
                used: payload.len() as u64,
                ..CdsRegion::default()
            };

            let header_end = archive::write_header(&mut file, &header, base_name, &regions)?;

            // Place payload at the first aligned offset after the header.
            let (offset, crc) = archive::append_region(&mut file, &payload)?;
            regions[RO_REGION].file_offset = offset;
            regions[RO_REGION].crc = crc;
            regions[RO_REGION].used = payload.len() as u64;

            // Rewrite header with the final region table.
            archive::write_header(&mut file, &header, base_name, &regions)?;
            debug_assert!(header_end <= offset);

            file.flush()?;
            Ok(payload.len())
        })();
        CDSConfig::set_dumping_archive(false);
        result
    }

    /// archiveHeapLoader.cpp: load classes from a shared archive.
    pub fn restore_from_file(&mut self, path: &str) -> io::Result<usize> {
        self.restore_from_path(Path::new(path))
    }

    pub fn restore_from_path(&mut self, path: &Path) -> io::Result<usize> {
        CDSConfig::set_using_archive(true);
        let result = (|| -> io::Result<usize> {
            let (mut file, archive_hdr) = archive::open_archive(path)?;
            let ro = archive_hdr
                .regions
                .get(RO_REGION)
                .ok_or_else(|| io::Error::other("archive missing RO region"))?;
            if ro.used == 0 {
                return Ok(0);
            }
            let data = archive::read_region(&mut file, ro)?;
            self.deserialize_classes(&data)
        })();
        CDSConfig::set_using_archive(false);
        result
    }

    /// cdsConfig integration: dump all registered classes ("-Xshare:dump").
    pub fn dump_archive(&self, path: &str) -> io::Result<usize> {
        if !CDSConfig::is_dumping_archive() {
            CDSConfig::set_dumping_archive(true);
            let r = self.dump_to_file(path);
            CDSConfig::set_dumping_archive(false);
            return r;
        }
        self.dump_to_file(path)
    }
}

impl Default for MetaspaceShared {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_and_load_roundtrip() {
        let mut store = MetaspaceShared::new();
        store.add_class("java/lang/Object".into(), vec![1, 2, 3, 4]);
        store.add_class("com/example/Main".into(), vec![9, 8, 7]);

        let dir = std::env::temp_dir();
        let path = dir.join("cds_roundtrip.jsa");
        let n = store.dump_to_file(path.to_str().unwrap()).unwrap();
        assert!(n > 0);

        let mut loaded = MetaspaceShared::new();
        let count = loaded.restore_from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            loaded.get_class("java/lang/Object").unwrap().bytecode,
            vec![1, 2, 3, 4]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_bad_magic() {
        let dir = std::env::temp_dir();
        let path = dir.join("cds_bad.jsa");
        std::fs::write(&path, b"NOTACDSARCHIVE").unwrap();
        let mut store = MetaspaceShared::new();
        assert!(store.restore_from_file(path.to_str().unwrap()).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
