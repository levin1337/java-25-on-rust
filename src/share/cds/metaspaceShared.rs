/*
 * Port of OpenJDK 25 HotSpot MetaspaceShared (share/cds/metaspaceShared.hpp)
 *
 * MetaspaceShared coordinates dumping and loading of pre-parsed Java classes
 * to and from the CDS shared archive.
 */

use super::cdsConfig::CDSConfig;
use super::filemap::{FileMapHeader, FileMapRegion, RegionType, compute_crc};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};

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

    pub fn dump_to_file(&self, path: &str) -> io::Result<usize> {
        CDSConfig::set_dumping_archive(true);
        let mut file = File::create(path)?;

        let mut body = Vec::new();
        // Serialize archived classes
        body.write_all(&(self.archived_classes.len() as u32).to_be_bytes())?;
        for (name, cls) in &self.archived_classes {
            let name_bytes = name.as_bytes();
            body.write_all(&(name_bytes.len() as u16).to_be_bytes())?;
            body.write_all(name_bytes)?;

            body.write_all(&(cls.bytecode.len() as u32).to_be_bytes())?;
            body.write_all(&cls.bytecode)?;
        }

        let body_len = body.len();
        let body_crc = compute_crc(&body);
        let mut header = FileMapHeader::new("OpenJDK 25 (Rust CDS)".to_string());
        let region = FileMapRegion::new(RegionType::ReadOnly, 0, body_len, body_crc);
        header.regions.push(region);

        header.write_to(&mut file)?;
        file.write_all(&body)?;
        file.flush()?;

        CDSConfig::set_dumping_archive(false);
        Ok(body_len)
    }

    pub fn restore_from_file(&mut self, path: &str) -> io::Result<usize> {
        CDSConfig::set_using_archive(true);
        let mut file = File::open(path)?;

        let header = FileMapHeader::read_from(&mut file)?;
        if header.regions.is_empty() {
            CDSConfig::set_using_archive(false);
            return Err(io::Error::other("Empty CDS regions"));
        }

        let region = &header.regions[0];
        let mut body = vec![0u8; region.byte_size];
        file.read_exact(&mut body)?;

        if !region.check_crc(&body) {
            CDSConfig::set_using_archive(false);
            return Err(io::Error::other("CDS region CRC32 mismatch"));
        }

        let mut cursor = io::Cursor::new(body);
        let mut u32_buf = [0u8; 4];
        cursor.read_exact(&mut u32_buf)?;
        let class_count = u32::from_be_bytes(u32_buf) as usize;

        for _ in 0..class_count {
            let mut u16_buf = [0u8; 2];
            cursor.read_exact(&mut u16_buf)?;
            let name_len = u16::from_be_bytes(u16_buf) as usize;

            let mut name_buf = vec![0u8; name_len];
            cursor.read_exact(&mut name_buf)?;
            let name = String::from_utf8_lossy(&name_buf).to_string();

            cursor.read_exact(&mut u32_buf)?;
            let code_len = u32::from_be_bytes(u32_buf) as usize;

            let mut bytecode = vec![0u8; code_len];
            cursor.read_exact(&mut bytecode)?;

            self.archived_classes
                .insert(name.clone(), ArchivedClass { name, bytecode });
        }

        CDSConfig::set_using_archive(false);
        Ok(self.archived_classes.len())
    }
}

impl Default for MetaspaceShared {
    fn default() -> Self {
        Self::new()
    }
}
