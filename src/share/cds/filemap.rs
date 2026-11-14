/*
 * Port of OpenJDK 25 HotSpot FileMapInfo (share/cds/filemap.hpp)
 *
 * FileMapInfo manages the serialized format and binary headers of the
 * Class Data Sharing (.jsa) archive.
 */

use std::io::{self, Error, Read, Write};

pub const CDS_MAGIC: u32 = 0xDAB0_0BEE;
pub const CURRENT_CDS_VERSION: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionType {
    ReadWrite = 0,
    ReadOnly = 1,
    Bitmap = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMapRegion {
    pub region_type: RegionType,
    pub file_offset: usize,
    pub byte_size: usize,
    pub crc: u32,
}

impl FileMapRegion {
    pub fn new(region_type: RegionType, file_offset: usize, byte_size: usize, crc: u32) -> Self {
        Self {
            region_type,
            file_offset,
            byte_size,
            crc,
        }
    }

    #[inline]
    pub fn check_crc(&self, data: &[u8]) -> bool {
        self.crc == 0 || compute_crc(data) == self.crc
    }
}

/// Standard IEEE 802.3 CRC32 checksum calculation.
pub fn compute_crc(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMapHeader {
    pub magic: u32,
    pub version: u32,
    pub jvm_ident: String,
    pub regions: Vec<FileMapRegion>,
}

impl FileMapHeader {
    pub fn new(jvm_ident: String) -> Self {
        Self {
            magic: CDS_MAGIC,
            version: CURRENT_CDS_VERSION,
            jvm_ident,
            regions: Vec::new(),
        }
    }

    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.magic.to_be_bytes())?;
        w.write_all(&self.version.to_be_bytes())?;

        let mut ident_buf = [0u8; 64];
        let bytes = self.jvm_ident.as_bytes();
        let len = bytes.len().min(63);
        ident_buf[..len].copy_from_slice(&bytes[..len]);
        w.write_all(&ident_buf)?;

        w.write_all(&(self.regions.len() as u32).to_be_bytes())?;
        for r in &self.regions {
            w.write_all(&(r.region_type as u8).to_be_bytes())?;
            w.write_all(&(r.file_offset as u64).to_be_bytes())?;
            w.write_all(&(r.byte_size as u64).to_be_bytes())?;
            w.write_all(&r.crc.to_be_bytes())?;
        }
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut u32_buf = [0u8; 4];
        r.read_exact(&mut u32_buf)?;
        let magic = u32::from_be_bytes(u32_buf);
        if magic != CDS_MAGIC {
            return Err(Error::other("Invalid CDS archive magic"));
        }

        r.read_exact(&mut u32_buf)?;
        let version = u32::from_be_bytes(u32_buf);
        if version != CURRENT_CDS_VERSION {
            return Err(Error::other("Unsupported CDS archive version"));
        }

        let mut ident_buf = [0u8; 64];
        r.read_exact(&mut ident_buf)?;
        let ident_len = ident_buf.iter().position(|&b| b == 0).unwrap_or(64);
        let jvm_ident = String::from_utf8_lossy(&ident_buf[..ident_len]).to_string();

        r.read_exact(&mut u32_buf)?;
        let region_count = u32::from_be_bytes(u32_buf) as usize;
        let mut regions = Vec::with_capacity(region_count);

        for _ in 0..region_count {
            let mut tag_buf = [0u8; 1];
            r.read_exact(&mut tag_buf)?;
            let region_type = match tag_buf[0] {
                0 => RegionType::ReadWrite,
                1 => RegionType::ReadOnly,
                2 => RegionType::Bitmap,
                _ => return Err(Error::other("Invalid region type")),
            };

            let mut u64_buf = [0u8; 8];
            r.read_exact(&mut u64_buf)?;
            let file_offset = u64::from_be_bytes(u64_buf) as usize;

            r.read_exact(&mut u64_buf)?;
            let byte_size = u64::from_be_bytes(u64_buf) as usize;

            r.read_exact(&mut u32_buf)?;
            let crc = u32::from_be_bytes(u32_buf);

            regions.push(FileMapRegion::new(region_type, file_offset, byte_size, crc));
        }

        Ok(Self {
            magic,
            version,
            jvm_ident,
            regions,
        })
    }
}
