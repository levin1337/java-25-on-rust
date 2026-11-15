/*
 * Port of OpenJDK 25 HotSpot FileMapInfo / CDS archive container
 * (share/cds/filemap.hpp/.cpp, share/cds/archiveBuilder.cpp)
 *
 * Implements the on-disk layout of a CDS shared archive:
 *
 *   offset 0:  GenericCDSFileMapHeader (24 bytes)
 *              u32 magic          — CDS_ARCHIVE_MAGIC / _DYNAMIC_ / _PREIMAGE_
 *              i32 crc            — header CRC (of everything after this field
 *                                   up to base_archive_name region)
 *              i32 version        — CURRENT_CDS_ARCHIVE_VERSION
 *              u32 header_size    — total header bytes (generic + regions)
 *              u32 base_archive_name_offset
 *              u32 base_archive_name_size
 *   then:      base archive name (dynamic archives only; NUL-terminated)
 *   then:      CDSFileMapRegion[NUM_CDS_REGIONS]
 *   then:      region payloads at their file_offset, page-aligned
 *
 * Region payload order (MetaspaceShared):
 *   [0] RO metaspace, [1] RW metaspace, [2] relocation bitmap,
 *   [3..4] heap regions (unused in the Rust port — serialized as empty).
 *
 * The Rust port writes little-endian fields explicitly; the format is
 * self-describing through the magic/version header so the Serviceability
 * Agent layout in share/include/cds.h stays recognizable.
 */

use super::cdsConstants::{
    CDS_ARCHIVE_MAGIC, CDS_DYNAMIC_ARCHIVE_MAGIC, CDS_GENERIC_HEADER_SUPPORTED_MIN_VERSION,
    CDS_PREIMAGE_ARCHIVE_MAGIC, CURRENT_CDS_ARCHIVE_VERSION, NUM_CDS_REGIONS,
};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Size of the serialized GenericCDSFileMapHeader (share/include/cds.h).
pub const GENERIC_HEADER_SIZE: usize = 24;
/// Serialized size of one CDSFileMapRegion record on LP64:
/// i32 crc + 5 x i32 flags + 7 x u64 + u64 mapped_base + u8 in_reserved_space + 7 pad.
pub const REGION_RECORD_SIZE: usize = 96;
/// Byte size of the generic header plus the region table.
pub const CORE_HEADER_SIZE: usize = GENERIC_HEADER_SIZE + NUM_CDS_REGIONS * REGION_RECORD_SIZE;
/// os::cds_core_region_alignment() on Windows = os::vm_allocation_granularity().
pub const CORE_REGION_ALIGNMENT: usize = 64 * 1024;
/// Byte size of the fixed generic+region prefix (kept for compatibility).
pub const FIXED_HEADER_SIZE: usize = CORE_HEADER_SIZE;

/// GenericCDSFileMapHeader — the version-stable prefix of the archive.
#[derive(Debug, Clone, Copy)]
pub struct GenericHeader {
    pub magic: u32,
    pub crc: i32,
    pub version: i32,
    pub header_size: u32,
    pub base_archive_name_offset: u32,
    pub base_archive_name_size: u32,
}

impl GenericHeader {
    pub fn static_archive() -> Self {
        Self {
            magic: CDS_ARCHIVE_MAGIC,
            crc: 0,
            version: CURRENT_CDS_ARCHIVE_VERSION,
            header_size: FIXED_HEADER_SIZE as u32,
            base_archive_name_offset: 0,
            base_archive_name_size: 0,
        }
    }

    pub fn dynamic_archive(_base_name: &str) -> Self {
        // base_archive_name_offset/size are filled in by serialize_header_block;
        // the name is placed after the FileMapHeader extension fields.
        Self {
            magic: CDS_DYNAMIC_ARCHIVE_MAGIC,
            crc: 0,
            version: CURRENT_CDS_ARCHIVE_VERSION,
            header_size: 0,
            base_archive_name_offset: 0,
            base_archive_name_size: 0,
        }
    }

    pub fn is_static(&self) -> bool {
        self.magic == CDS_ARCHIVE_MAGIC
    }
    pub fn is_dynamic(&self) -> bool {
        self.magic == CDS_DYNAMIC_ARCHIVE_MAGIC
    }
    pub fn is_preimage(&self) -> bool {
        self.magic == CDS_PREIMAGE_ARCHIVE_MAGIC
    }
    pub fn is_supported(&self) -> bool {
        self.version >= CDS_GENERIC_HEADER_SUPPORTED_MIN_VERSION
            && self.version <= CURRENT_CDS_ARCHIVE_VERSION
    }

    fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.magic.to_le_bytes())?;
        w.write_all(&self.crc.to_le_bytes())?;
        w.write_all(&self.version.to_le_bytes())?;
        w.write_all(&self.header_size.to_le_bytes())?;
        w.write_all(&self.base_archive_name_offset.to_le_bytes())?;
        w.write_all(&self.base_archive_name_size.to_le_bytes())?;
        Ok(())
    }
}

/// One serialized CDSFileMapRegion (share/include/cds.h layout, pointers
/// omitted — `mapped_base` is a runtime-only field written as 0).
#[derive(Debug, Clone, Copy, Default)]
pub struct CdsRegion {
    pub crc: i32,
    pub read_only: bool,
    pub allow_exec: bool,
    pub is_heap_region: bool,
    pub is_bitmap_region: bool,
    pub mapped_from_file: bool,
    pub file_offset: u64,
    pub mapping_offset: u64,
    pub used: u64,
    pub oopmap_offset: u64,
    pub oopmap_size_in_bits: u64,
    pub ptrmap_offset: u64,
    pub ptrmap_size_in_bits: u64,
    /// Runtime-only field in C++; serialized as the raw pointer value
    /// (always 0 in archives produced by FileMapInfo::write_region).
    pub mapped_base: u64,
    pub in_reserved_space: bool,
}

impl CdsRegion {
    fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.crc.to_le_bytes())?;
        for flag in [
            self.read_only,
            self.allow_exec,
            self.is_heap_region,
            self.is_bitmap_region,
            self.mapped_from_file,
        ] {
            w.write_all(&(flag as i32).to_le_bytes())?;
        }
        for v in [
            self.file_offset,
            self.mapping_offset,
            self.used,
            self.oopmap_offset,
            self.oopmap_size_in_bits,
            self.ptrmap_offset,
            self.ptrmap_size_in_bits,
            self.mapped_base,
        ] {
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&[self.in_reserved_space as u8])?;
        w.write_all(&[0u8; 7])?;
        Ok(())
    }

    fn read_from<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut b4 = [0u8; 4];
        let mut b8 = [0u8; 8];
        r.read_exact(&mut b4)?;
        let crc = i32::from_le_bytes(b4);
        let mut flags = [false; 5];
        for f in flags.iter_mut() {
            r.read_exact(&mut b4)?;
            *f = i32::from_le_bytes(b4) != 0;
        }
        let mut vals = [0u64; 8];
        for v in vals.iter_mut() {
            r.read_exact(&mut b8)?;
            *v = u64::from_le_bytes(b8);
        }
        let mut pad = [0u8; 8];
        r.read_exact(&mut pad)?;
        Ok(Self {
            crc,
            read_only: flags[0],
            allow_exec: flags[1],
            is_heap_region: flags[2],
            is_bitmap_region: flags[3],
            mapped_from_file: flags[4],
            file_offset: vals[0],
            mapping_offset: vals[1],
            used: vals[2],
            oopmap_offset: vals[3],
            oopmap_size_in_bits: vals[4],
            ptrmap_offset: vals[5],
            ptrmap_size_in_bits: vals[6],
            mapped_base: vals[7],
            in_reserved_space: pad[0] != 0,
        })
    }
}

/// FileMapInfo equivalent: header + regions + payload access.
/// `extension` holds the serialized FileMapHeader fields that follow the
/// region table in HotSpot's raw struct dump (parsed by filemap.rs).
pub struct CdsArchive {
    pub header: GenericHeader,
    pub base_archive_name: Option<String>,
    pub regions: Vec<CdsRegion>,
    pub extension: Vec<u8>,
}

impl CdsArchive {
    /// filemap.cpp sanity checks.
    pub fn validate(&self) -> io::Result<()> {
        if self.regions.len() != NUM_CDS_REGIONS {
            return Err(io::Error::other("wrong number of CDS regions"));
        }
        for (i, r) in self.regions.iter().enumerate() {
            if r.file_offset == 0 && r.used > 0 {
                return Err(io::Error::other(format!(
                    "region {} has payload but zero file_offset",
                    i
                )));
            }
        }
        Ok(())
    }
}

pub fn crc32_pub(data: &[u8]) -> u32 {
    crc32(data)
}

fn crc32(data: &[u8]) -> u32 {
    // IEEE CRC32 (same polynomial as ClassLoader::crc32 / zlib crc32).
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub(crate) fn align_up(v: u64, align: u64) -> u64 {
    (v + align - 1) & !(align - 1)
}

/// Compute the header CRC. FileMapHeader::compute_crc() starts at the byte
/// after `_header_size` (file offset 16) and covers everything up to
/// header_size — i.e. base-name fields, region table, FileMapHeader
/// extension fields and the base archive name.
fn header_crc(header_block: &[u8]) -> i32 {
    crc32(&header_block[16..]) as i32
}

/// Build the complete header block: generic header, region table, FileMapHeader
/// extension bytes, base archive name. Returns the block and the computed CRC.
pub fn serialize_header_block(
    mut header: GenericHeader,
    regions: &[CdsRegion],
    extension: &[u8],
    base_name: Option<&str>,
) -> io::Result<Vec<u8>> {
    assert_eq!(regions.len(), NUM_CDS_REGIONS);
    let name_bytes = base_name.map(|s| s.as_bytes());
    let name_size = name_bytes.map_or(0, |b| b.len() + 1);
    let struct_size = CORE_HEADER_SIZE + extension.len();
    let name_offset = if name_size > 0 { struct_size } else { 0 };
    let header_size = struct_size + name_size;

    header.header_size = header_size as u32;
    header.base_archive_name_offset = name_offset as u32;
    header.base_archive_name_size = name_size as u32;

    let mut block = Vec::with_capacity(header_size);
    header.write_to(&mut block)?;
    for r in regions {
        r.write_to(&mut block)?;
    }
    block.extend_from_slice(extension);
    if let Some(bytes) = name_bytes {
        block.extend_from_slice(bytes);
        block.push(0);
    }
    assert_eq!(block.len(), header_size);

    let crc = header_crc(&block);
    block[4..8].copy_from_slice(&crc.to_le_bytes());
    Ok(block)
}

/// Read an archive header + region table + extension from an open file.
pub fn read_header(file: &mut File) -> io::Result<CdsArchive> {
    let mut b4 = [0u8; 4];

    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut b4)?;
    let magic = u32::from_le_bytes(b4);
    let header = GenericHeader {
        magic,
        crc: {
            file.read_exact(&mut b4)?;
            i32::from_le_bytes(b4)
        },
        version: {
            file.read_exact(&mut b4)?;
            i32::from_le_bytes(b4)
        },
        header_size: {
            file.read_exact(&mut b4)?;
            u32::from_le_bytes(b4)
        },
        base_archive_name_offset: {
            file.read_exact(&mut b4)?;
            u32::from_le_bytes(b4)
        },
        base_archive_name_size: {
            file.read_exact(&mut b4)?;
            u32::from_le_bytes(b4)
        },
    };
    if header.magic != CDS_ARCHIVE_MAGIC
        && header.magic != CDS_DYNAMIC_ARCHIVE_MAGIC
        && header.magic != CDS_PREIMAGE_ARCHIVE_MAGIC
    {
        return Err(io::Error::other(format!(
            "Not a CDS archive (magic {:#x})",
            header.magic
        )));
    }
    if !header.is_supported() {
        return Err(io::Error::other(format!(
            "Unsupported CDS archive version {}",
            header.version
        )));
    }
    let header_size = header.header_size as u64;
    if header_size < CORE_HEADER_SIZE as u64 {
        return Err(io::Error::other(format!(
            "CDS header too small: {} bytes",
            header_size
        )));
    }

    // Region table immediately follows the 24-byte generic header.
    file.seek(SeekFrom::Start(GENERIC_HEADER_SIZE as u64))?;
    let mut regions = Vec::with_capacity(NUM_CDS_REGIONS);
    for _ in 0..NUM_CDS_REGIONS {
        regions.push(CdsRegion::read_from(file)?);
    }

    // Base archive name sits at base_archive_name_offset (after the
    // FileMapHeader extension fields).
    let mut base_name = None;
    if header.base_archive_name_size > 0 {
        let name_off = header.base_archive_name_offset as u64;
        let name_end = name_off + header.base_archive_name_size as u64;
        if name_end > header_size {
            return Err(io::Error::other("CDS base archive name out of header bounds"));
        }
        file.seek(SeekFrom::Start(name_off))?;
        let mut buf = vec![0u8; header.base_archive_name_size as usize];
        file.read_exact(&mut buf)?;
        if let Some(nul) = buf.iter().position(|&b| b == 0) {
            buf.truncate(nul);
        }
        base_name = Some(String::from_utf8_lossy(&buf).to_string());
    }

    // FileMapHeader extension: everything between the region table and the
    // base archive name (or end of header when there is no name).
    let ext_start = CORE_HEADER_SIZE as u64;
    let ext_end = if header.base_archive_name_size > 0 {
        header.base_archive_name_offset as u64
    } else {
        header_size
    };
    let mut extension = Vec::new();
    if ext_end > ext_start {
        file.seek(SeekFrom::Start(ext_start))?;
        extension.resize((ext_end - ext_start) as usize, 0);
        file.read_exact(&mut extension)?;
    }

    // Verify header CRC over bytes [16, header_size).
    if header.crc != 0 {
        file.seek(SeekFrom::Start(16))?;
        let mut hb = vec![0u8; (header_size - 16) as usize];
        file.read_exact(&mut hb)?;
        let computed = crc32(&hb) as i32;
        if computed != header.crc {
            return Err(io::Error::other(format!(
                "CDS header CRC mismatch: stored {} computed {}",
                header.crc, computed
            )));
        }
    }

    let archive = CdsArchive {
        header,
        base_archive_name: base_name,
        regions,
        extension,
    };
    archive.validate()?;
    Ok(archive)
}

/// Write a complete archive header block (generic + regions + extension +
/// name) at offset 0. Returns the aligned file offset where region payloads
/// must begin (FileMapInfo::prepare_for_writing: align_up(header_size,
/// core_region_alignment)).
pub fn write_header(
    file: &mut File,
    header: &GenericHeader,
    regions: &[CdsRegion],
    extension: &[u8],
    base_name: Option<&str>,
) -> io::Result<u64> {
    let block = serialize_header_block(*header, regions, extension, base_name)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&block)?;
    let data_start = align_up(block.len() as u64, CORE_REGION_ALIGNMENT as u64);
    // Extend the file up to the first payload boundary if needed, but never
    // truncate: the header is rewritten after region payloads are appended
    // (FileMapInfo::write_header runs twice during a dump).
    if file.metadata()?.len() < data_start {
        file.set_len(data_start)?;
    }
    file.seek(SeekFrom::Start(data_start))?;
    Ok(data_start)
}

/// Load the payload of one region from the archive file.
pub fn read_region(file: &mut File, region: &CdsRegion) -> io::Result<Vec<u8>> {
    let used = region.used as usize;
    if used == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(region.file_offset))?;
    let mut data = vec![0u8; used];
    file.read_exact(&mut data)?;
    let crc = crc32(&data) as i32;
    if region.crc != 0 && crc != region.crc {
        return Err(io::Error::other(format!(
            "CDS region CRC mismatch at offset {}: stored {} computed {}",
            region.file_offset, region.crc, crc
        )));
    }
    Ok(data)
}

/// Append a region payload to the archive, returning (file_offset, crc).
/// Mirrors FileMapInfo::write_bytes_aligned: the file position is aligned to
/// core_region_alignment() before and after the payload.
pub fn append_region(file: &mut File, data: &[u8]) -> io::Result<(u64, i32)> {
    let pos = file.seek(SeekFrom::Current(0))?;
    let aligned = align_up(pos, CORE_REGION_ALIGNMENT as u64);
    if aligned > pos {
        file.set_len(aligned)?;
        file.seek(SeekFrom::Start(aligned))?;
    }
    file.write_all(data)?;
    let end = file.seek(SeekFrom::Current(0))?;
    let aligned_end = align_up(end, CORE_REGION_ALIGNMENT as u64);
    file.set_len(aligned_end)?;
    Ok((aligned, crc32(data) as i32))
}

/// Open an archive file for reading; verifies magic.
pub fn open_archive(path: &Path) -> io::Result<(File, CdsArchive)> {
    let mut f = File::open(path)?;
    let archive = read_header(&mut f)?;
    Ok((f, archive))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_file(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("cds_archive_test_{}.jsa", tag))
    }

    #[test]
    fn header_roundtrip() {
        let header = GenericHeader::static_archive();
        let mut regions = vec![CdsRegion::default(); NUM_CDS_REGIONS];
        regions[0] = CdsRegion {
            crc: 123,
            read_only: true,
            file_offset: 65536,
            used: 9,
            mapped_from_file: true,
            ..CdsRegion::default()
        };
        let path = test_file("roundtrip");
        {
            let mut f = File::create(&path).unwrap();
            write_header(&mut f, &header, &regions, b"ext-bytes", None).unwrap();
            assert_eq!(f.stream_position().unwrap(), CORE_REGION_ALIGNMENT as u64);
            let (off, crc) = append_region(&mut f, b"hello cds").unwrap();
            assert_eq!(off % CORE_REGION_ALIGNMENT as u64, 0);
            regions[0].file_offset = off;
            regions[0].crc = crc;
        }
        // Rewrite header with final region CRCs.
        let mut f = File::options().read(true).write(true).open(&path).unwrap();
        write_header(&mut f, &header, &regions, b"ext-bytes", None).unwrap();
        drop(f);

        let mut rf = File::open(&path).unwrap();
        let arch = read_header(&mut rf).unwrap();
        assert!(arch.header.is_static());
        assert!(arch.header.is_supported());
        assert_eq!(arch.regions.len(), NUM_CDS_REGIONS);
        assert_eq!(arch.regions[0].used, 9);
        assert!(arch.regions[0].read_only);
        assert_eq!(arch.regions[0].file_offset % CORE_REGION_ALIGNMENT as u64, 0);
        assert_eq!(arch.extension, b"ext-bytes");
        assert_eq!(arch.header.header_size as usize, CORE_HEADER_SIZE + 9);
        let data = read_region(&mut rf, &arch.regions[0]).unwrap();
        assert_eq!(data, b"hello cds");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dynamic_header_name() {
        let path = test_file("dynamic");
        let header = GenericHeader::dynamic_archive("base.jsa");
        let regions = vec![CdsRegion::default(); NUM_CDS_REGIONS];
        let mut f = File::create(&path).unwrap();
        write_header(&mut f, &header, &regions, b"", Some("base.jsa")).unwrap();
        drop(f);

        let mut rf = File::open(&path).unwrap();
        let arch = read_header(&mut rf).unwrap();
        assert!(arch.header.is_dynamic());
        assert_eq!(arch.base_archive_name.as_deref(), Some("base.jsa"));
        // Name sits after the region table + extension, inside header_size.
        let name_end = arch.header.base_archive_name_offset as usize
            + arch.header.base_archive_name_size as usize;
        assert_eq!(name_end as u32, arch.header.header_size);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_bad_magic_and_corrupt_crc() {
        let path = test_file("corrupt");
        std::fs::write(&path, b"NOTACDSARCHIVE").unwrap();
        let mut rf = File::open(&path).unwrap();
        assert!(read_header(&mut rf).is_err());

        // Valid header, then corrupt a payload byte covered by region CRC.
        let header = GenericHeader::static_archive();
        let mut regions = vec![CdsRegion::default(); NUM_CDS_REGIONS];
        regions[1] = CdsRegion {
            file_offset: CORE_REGION_ALIGNMENT as u64,
            used: 4,
            ..CdsRegion::default()
        };
        {
            let mut f = File::create(&path).unwrap();
            write_header(&mut f, &header, &regions, b"", None).unwrap();
            let (off, crc) = append_region(&mut f, &[1, 2, 3, 4]).unwrap();
            regions[1].file_offset = off;
            regions[1].crc = crc;
        }
        {
            let mut f = File::options().read(true).write(true).open(&path).unwrap();
            write_header(&mut f, &header, &regions, b"", None).unwrap();
            use std::io::{Seek, SeekFrom, Write};
            f.seek(SeekFrom::Start(CORE_REGION_ALIGNMENT as u64)).unwrap();
            f.write_all(&[9, 9, 9, 9]).unwrap();
        }
        let mut rf = File::open(&path).unwrap();
        let arch = read_header(&mut rf).unwrap();
        assert!(read_region(&mut rf, &arch.regions[1]).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn region_record_size_is_96() {
        let mut buf = Vec::new();
        CdsRegion::default().write_to(&mut buf).unwrap();
        assert_eq!(buf.len(), REGION_RECORD_SIZE);
        assert_eq!(CORE_HEADER_SIZE, GENERIC_HEADER_SIZE + NUM_CDS_REGIONS * 96);
    }
}
