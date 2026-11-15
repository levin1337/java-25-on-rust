/*
 * Port of OpenJDK 25 HotSpot FileMapInfo / FileMapHeader
 * (share/cds/filemap.hpp, share/cds/filemap.cpp, share/include/cds.h)
 *
 * FileMapInfo manages the serialized format and binary headers of the
 * Class Data Sharing (.jsa) archive. The on-disk layout is:
 *
 *   [0]                    GenericCDSFileMapHeader            (24 bytes)
 *   [24]                   CDSFileMapRegion[NUM_CDS_REGIONS]   (5 x 96 bytes)
 *   [504]                  FileMapHeader extension fields     (this file)
 *   [base_archive_name_off] base archive name (dynamic archives)
 *   align_up(header_size, core_region_alignment)
 *   then region payloads at their recorded file_offset
 *
 * The generic header + region table is exactly the CDSFileMapHeaderBase
 * layout that the Serviceability Agent reads (share/include/cds.h), so the
 * prefix of an archive produced here is byte-compatible with HotSpot's.
 * The FileMapHeader extension mirrors the field order of the C++ struct
 * (filemap.hpp) serialized with LP64 natural alignment.
 */

use super::archive::{self, CdsArchive, CdsRegion, CORE_REGION_ALIGNMENT};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// JVM_IDENT_MAX (filemap.hpp).
pub const JVM_IDENT_MAX: usize = 256;

/// Serialized size of one CDSFileMapRegion record (share/include/cds.h, LP64).
pub const REGION_RECORD_SIZE: usize = archive::REGION_RECORD_SIZE;
/// Generic header + region table size — start of FileMapHeader fields.
pub const CORE_HEADER_SIZE: usize = archive::CORE_HEADER_SIZE;

/// Region slot indices — MetaspaceShared::n_regions order (metaspaceShared.hpp).
pub const RW_REGION: usize = 0;
pub const RO_REGION: usize = 1;
pub const BM_REGION: usize = 2;
pub const HP_REGION: usize = 3;
pub const AC_REGION: usize = 4;

/// HeapRootSegments (archiveUtils.hpp) — inline sub-struct of FileMapHeader.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeapRootSegments {
    pub base_offset: u64,
    pub count: u64,
    pub roots_count: i32,
    pub max_size_in_bytes: i32,
    pub max_size_in_elems: i32,
}

/// FileMapHeader — the fields FileMapHeader adds after CDSFileMapHeaderBase.
/// Field order mirrors filemap.hpp; serialization uses LP64 natural alignment.
#[derive(Debug, Clone)]
pub struct FileMapHeader {
    pub core_region_alignment: u64,
    pub obj_alignment: i32,
    pub narrow_oop_base: u64,
    pub narrow_oop_shift: i32,
    pub compact_strings: bool,
    pub compact_headers: bool,
    pub max_heap_size: u64,
    pub narrow_oop_mode: i32,
    pub compressed_oops: bool,
    pub compressed_class_pointers: bool,
    pub narrow_klass_pointer_bits: i32,
    pub narrow_klass_shift: i32,
    pub cloned_vtables_offset: u64,
    pub early_serialized_data_offset: u64,
    pub serialized_data_offset: u64,
    pub jvm_ident: [u8; JVM_IDENT_MAX],
    pub class_location_config_offset: u64,
    pub verify_local: bool,
    pub verify_remote: bool,
    pub has_platform_or_app_classes: bool,
    pub requested_base_address: u64,
    pub mapped_base_address: u64,
    pub allow_archiving_with_java_agent: bool,
    pub use_optimized_module_handling: bool,
    pub has_aot_linked_classes: bool,
    pub has_full_module_graph: bool,
    pub heap_root_segments: HeapRootSegments,
    pub heap_oopmap_start_pos: u64,
    pub heap_ptrmap_start_pos: u64,
    pub rw_ptrmap_start_pos: u64,
    pub ro_ptrmap_start_pos: u64,
    pub type_profile_level: u32,
    pub type_profile_args_limit: i32,
    pub type_profile_parms_limit: i32,
    pub type_profile_width: i64,
    pub bci_profile_width: i64,
    pub profile_traps: bool,
    pub type_profile_casts: bool,
    pub spec_trap_limit_extra_entries: i32,
}

impl Default for FileMapHeader {
    fn default() -> Self {
        Self {
            core_region_alignment: CORE_REGION_ALIGNMENT as u64,
            obj_alignment: 8,
            narrow_oop_base: 0,
            narrow_oop_shift: 3,
            compact_strings: true,
            compact_headers: false,
            max_heap_size: 0,
            narrow_oop_mode: 0,
            compressed_oops: true,
            compressed_class_pointers: true,
            narrow_klass_pointer_bits: 0,
            narrow_klass_shift: 0,
            cloned_vtables_offset: 0,
            early_serialized_data_offset: 0,
            serialized_data_offset: 0,
            jvm_ident: [0; JVM_IDENT_MAX],
            class_location_config_offset: 0,
            verify_local: false,
            verify_remote: false,
            has_platform_or_app_classes: false,
            requested_base_address: 0,
            mapped_base_address: 0,
            allow_archiving_with_java_agent: false,
            use_optimized_module_handling: true,
            has_aot_linked_classes: false,
            has_full_module_graph: false,
            heap_root_segments: HeapRootSegments::default(),
            heap_oopmap_start_pos: 0,
            heap_ptrmap_start_pos: 0,
            rw_ptrmap_start_pos: 0,
            ro_ptrmap_start_pos: 0,
            type_profile_level: 0,
            type_profile_args_limit: 0,
            type_profile_parms_limit: 0,
            type_profile_width: 0,
            bci_profile_width: 0,
            profile_traps: false,
            type_profile_casts: false,
            spec_trap_limit_extra_entries: 0,
        }
    }
}

struct Writer<'a> {
    buf: &'a mut Vec<u8>,
}

impl Writer<'_> {
    fn align(&mut self, align: usize) {
        let n = self.buf.len();
        let padded = (n + align - 1) & !(align - 1);
        self.buf.resize(padded, 0);
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn boolean(&mut self, v: bool) {
        self.u8(v as u8);
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn align(&mut self, align: usize) {
        let n = (self.pos + align - 1) & !(align - 1);
        if n > self.buf.len() {
            self.pos = self.buf.len();
        } else {
            self.pos = n;
        }
    }
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            return Err(io::Error::other("truncated FileMapHeader extension"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> io::Result<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn i64(&mut self) -> io::Result<i64> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes(b.try_into().unwrap()))
    }
    fn boolean(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }
}

impl FileMapHeader {
    /// Serialized size of the extension fields (after the region table).
    pub const EXTENSION_SIZE: usize = 984 - CORE_HEADER_SIZE;

    pub fn new() -> Self {
        Self::default()
    }

    /// Build the jvm_ident string the way filemap.cpp does:
    /// vm version string truncated into JVM_IDENT_MAX bytes.
    pub fn set_jvm_ident(&mut self, ident: &str) {
        let bytes = ident.as_bytes();
        let len = bytes.len().min(JVM_IDENT_MAX - 1);
        self.jvm_ident = [0; JVM_IDENT_MAX];
        self.jvm_ident[..len].copy_from_slice(&bytes[..len]);
    }

    pub fn jvm_ident_str(&self) -> &str {
        let end = self
            .jvm_ident
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(JVM_IDENT_MAX);
        std::str::from_utf8(&self.jvm_ident[..end]).unwrap_or("")
    }

    /// Serialize the extension fields (FileMapHeader members after the
    /// CDSFileMapHeaderBase sub-object), with LP64 natural alignment.
    pub fn serialize_extension(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::EXTENSION_SIZE);
        {
            let mut w = Writer { buf: &mut buf };
            w.align(8);
            w.u64(self.core_region_alignment);
            w.align(4);
            w.i32(self.obj_alignment);
            w.align(8);
            w.u64(self.narrow_oop_base);
            w.align(4);
            w.i32(self.narrow_oop_shift);
            w.boolean(self.compact_strings);
            w.boolean(self.compact_headers);
            w.align(8);
            w.u64(self.max_heap_size);
            w.align(4);
            w.i32(self.narrow_oop_mode);
            w.boolean(self.compressed_oops);
            w.boolean(self.compressed_class_pointers);
            w.align(4);
            w.i32(self.narrow_klass_pointer_bits);
            w.i32(self.narrow_klass_shift);
            w.align(8);
            w.u64(self.cloned_vtables_offset);
            w.u64(self.early_serialized_data_offset);
            w.u64(self.serialized_data_offset);
            w.buf.extend_from_slice(&self.jvm_ident);
            w.align(8);
            w.u64(self.class_location_config_offset);
            w.boolean(self.verify_local);
            w.boolean(self.verify_remote);
            w.boolean(self.has_platform_or_app_classes);
            w.align(8);
            w.u64(self.requested_base_address);
            w.u64(self.mapped_base_address);
            w.boolean(self.allow_archiving_with_java_agent);
            w.boolean(self.use_optimized_module_handling);
            w.boolean(self.has_aot_linked_classes);
            w.boolean(self.has_full_module_graph);
            w.align(8);
            w.u64(self.heap_root_segments.base_offset);
            w.u64(self.heap_root_segments.count);
            w.i32(self.heap_root_segments.roots_count);
            w.i32(self.heap_root_segments.max_size_in_bytes);
            w.i32(self.heap_root_segments.max_size_in_elems);
            w.align(8);
            w.u64(self.heap_oopmap_start_pos);
            w.u64(self.heap_ptrmap_start_pos);
            w.u64(self.rw_ptrmap_start_pos);
            w.u64(self.ro_ptrmap_start_pos);
            w.u32(self.type_profile_level);
            w.i32(self.type_profile_args_limit);
            w.i32(self.type_profile_parms_limit);
            w.align(8);
            w.i64(self.type_profile_width);
            w.i64(self.bci_profile_width);
            w.boolean(self.profile_traps);
            w.boolean(self.type_profile_casts);
            w.align(4);
            w.i32(self.spec_trap_limit_extra_entries);
        }
        debug_assert_eq!(buf.len(), Self::EXTENSION_SIZE);
        buf
    }

    /// Parse the extension fields; tolerates trailing padding/short reads by
    /// leaving remaining fields at default.
    pub fn parse_extension(data: &[u8]) -> io::Result<Self> {
        let mut h = Self::default();
        let mut r = Reader { buf: data, pos: 0 };
        if data.len() < Self::EXTENSION_SIZE {
            return Err(io::Error::other(format!(
                "FileMapHeader extension too small: {} < {}",
                data.len(),
                Self::EXTENSION_SIZE
            )));
        }
        r.align(8);
        h.core_region_alignment = r.u64()?;
        r.align(4);
        h.obj_alignment = r.i32()?;
        r.align(8);
        h.narrow_oop_base = r.u64()?;
        r.align(4);
        h.narrow_oop_shift = r.i32()?;
        h.compact_strings = r.boolean()?;
        h.compact_headers = r.boolean()?;
        r.align(8);
        h.max_heap_size = r.u64()?;
        r.align(4);
        h.narrow_oop_mode = r.i32()?;
        h.compressed_oops = r.boolean()?;
        h.compressed_class_pointers = r.boolean()?;
        r.align(4);
        h.narrow_klass_pointer_bits = r.i32()?;
        h.narrow_klass_shift = r.i32()?;
        r.align(8);
        h.cloned_vtables_offset = r.u64()?;
        h.early_serialized_data_offset = r.u64()?;
        h.serialized_data_offset = r.u64()?;
        h.jvm_ident = r.take(JVM_IDENT_MAX)?.try_into().unwrap();
        r.align(8);
        h.class_location_config_offset = r.u64()?;
        h.verify_local = r.boolean()?;
        h.verify_remote = r.boolean()?;
        h.has_platform_or_app_classes = r.boolean()?;
        r.align(8);
        h.requested_base_address = r.u64()?;
        h.mapped_base_address = r.u64()?;
        h.allow_archiving_with_java_agent = r.boolean()?;
        h.use_optimized_module_handling = r.boolean()?;
        h.has_aot_linked_classes = r.boolean()?;
        h.has_full_module_graph = r.boolean()?;
        r.align(8);
        h.heap_root_segments.base_offset = r.u64()?;
        h.heap_root_segments.count = r.u64()?;
        h.heap_root_segments.roots_count = r.i32()?;
        h.heap_root_segments.max_size_in_bytes = r.i32()?;
        h.heap_root_segments.max_size_in_elems = r.i32()?;
        r.align(8);
        h.heap_oopmap_start_pos = r.u64()?;
        h.heap_ptrmap_start_pos = r.u64()?;
        h.rw_ptrmap_start_pos = r.u64()?;
        h.ro_ptrmap_start_pos = r.u64()?;
        h.type_profile_level = {
            let b = r.take(4)?;
            u32::from_le_bytes(b.try_into().unwrap())
        };
        h.type_profile_args_limit = r.i32()?;
        h.type_profile_parms_limit = r.i32()?;
        r.align(8);
        h.type_profile_width = r.i64()?;
        h.bci_profile_width = r.i64()?;
        h.profile_traps = r.boolean()?;
        h.type_profile_casts = r.boolean()?;
        r.align(4);
        h.spec_trap_limit_extra_entries = r.i32()?;
        Ok(h)
    }

    /// FileMapHeader::validate — compare dump-time VM settings with runtime.
    /// Returns Err listing the first mismatch (archive unusable).
    pub fn validate(&self, runtime: &FileMapHeader) -> io::Result<()> {
        if self.obj_alignment != runtime.obj_alignment {
            return Err(io::Error::other(format!(
                "archive ObjectAlignmentInBytes {} != runtime {}",
                self.obj_alignment, runtime.obj_alignment
            )));
        }
        if self.compact_strings != runtime.compact_strings {
            return Err(io::Error::other(format!(
                "archive CompactStrings ({}) != runtime ({})",
                self.compact_strings, runtime.compact_strings
            )));
        }
        if self.compressed_oops != runtime.compressed_oops {
            return Err(io::Error::other("archive compressed oops setting mismatch"));
        }
        if self.compressed_class_pointers != runtime.compressed_class_pointers {
            return Err(io::Error::other("archive compressed class pointers setting mismatch"));
        }
        Ok(())
    }
}

/// One mapped (or read-back) region of the archive.
#[derive(Debug)]
pub struct RegionMapping {
    pub region_index: usize,
    pub file_offset: u64,
    pub used: u64,
    pub read_only: bool,
    /// Virtual address when memory-mapped (Windows); 0 when copied.
    pub mapped_base: usize,
    /// Backing bytes when the region was read instead of mapped.
    pub data: Option<Vec<u8>>,
}

impl RegionMapping {
    /// View of the region contents regardless of backing.
    pub fn bytes(&self) -> &[u8] {
        if let Some(d) = &self.data {
            d
        } else {
            // SAFETY: mapped_base/used describe a live mapping owned by
            // MappedArchive for the duration of this borrow.
            unsafe { std::slice::from_raw_parts(self.mapped_base as *const u8, self.used as usize) }
        }
    }
}

/// FileMapInfo — open archive, verify header, map regions, check CRCs.
pub struct FileMapInfo {
    pub path: PathBuf,
    pub file: File,
    pub generic: archive::GenericHeader,
    pub header: FileMapHeader,
    pub regions: Vec<CdsRegion>,
    pub base_archive_name: Option<String>,
    /// Set after map_file_to_memory().
    pub mappings: Vec<RegionMapping>,
}

impl FileMapInfo {
    /// FileMapInfo::open_for_reading + header sanity checks.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let archive: CdsArchive = archive::read_header(&mut file)?;
        let header = FileMapHeader::parse_extension(&archive.extension)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            generic: archive.header,
            header,
            regions: archive.regions,
            base_archive_name: archive.base_archive_name,
            mappings: Vec::new(),
        })
    }

    /// FileMapInfo constructor used while dumping: builds the full header
    /// block with the given region table and FileMapHeader fields.
    /// Returns the file positioned at the first payload offset.
    pub fn for_writing(
        path: &Path,
        generic: archive::GenericHeader,
        regions: &[CdsRegion],
        header: &FileMapHeader,
        base_name: Option<&str>,
    ) -> io::Result<(File, u64)> {
        let mut file = File::create(path)?;
        let ext = header.serialize_extension();
        let data_start =
            archive::write_header(&mut file, &generic, regions, &ext, base_name)?;
        Ok((file, data_start))
    }

    /// Rewrite just the header block (after region CRCs are known).
    pub fn write_header_block(
        &mut self,
        generic: archive::GenericHeader,
        base_name: Option<&str>,
    ) -> io::Result<()> {
        let ext = self.header.serialize_extension();
        archive::write_header(
            &mut self.file,
            &generic,
            &self.regions,
            &ext,
            base_name,
        )?;
        Ok(())
    }

    pub fn is_static(&self) -> bool {
        self.generic.is_static()
    }

    pub fn is_dynamic(&self) -> bool {
        self.generic.is_dynamic()
    }

    pub fn region_at(&self, region: usize) -> io::Result<&CdsRegion> {
        self.regions
            .get(region)
            .ok_or_else(|| io::Error::other(format!("invalid CDS region index {}", region)))
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// FileMapInfo::map_region for every non-empty region.
    /// On Windows this uses CreateFileMappingW/MapViewOfFileEx through the
    /// platform layer; elsewhere (and for small regions) the payload is read
    /// into memory — semantically identical for CRC verification and lookup.
    pub fn map_file_to_memory(&mut self) -> io::Result<()> {
        self.mappings.clear();
        for (i, r) in self.regions.iter().enumerate() {
            if r.used == 0 || r.file_offset == 0 {
                continue;
            }
            let mapping = map_region(&mut self.file, i, r)?;
            self.mappings.push(mapping);
        }
        Ok(())
    }

    /// FileMapRegion::check_region_crc for all mapped regions.
    pub fn verify_region_checksums(&self) -> io::Result<()> {
        for m in &self.mappings {
            let r = &self.regions[m.region_index];
            if r.used == 0 {
                continue;
            }
            let crc = archive::crc32_pub(m.bytes()) as i32;
            if r.crc != 0 && crc != r.crc {
                return Err(io::Error::other(format!(
                    "CDS region {} CRC mismatch: stored {:#x} computed {:#x}",
                    m.region_index, r.crc, crc
                )));
            }
        }
        Ok(())
    }

    /// Read the payload of one region (FileMapInfo::read_region path).
    pub fn read_region(&mut self, region: usize) -> io::Result<Vec<u8>> {
        let r = self.regions[region].clone();
        archive::read_region(&mut self.file, &r)
    }

    /// Look up an already-mapped region by index.
    pub fn mapped_region(&self, region: usize) -> Option<&RegionMapping> {
        self.mappings.iter().find(|m| m.region_index == region)
    }

    pub fn unmap(&mut self) {
        #[cfg(windows)]
        for m in self.mappings.drain(..) {
            if m.mapped_base != 0 {
                let _ = crate::share::platform::windows::OsWindows::unmap_file_memory(
                    m.mapped_base,
                    m.used as usize,
                );
            }
        }
        #[cfg(not(windows))]
        self.mappings.clear();
    }
}

impl Drop for FileMapInfo {
    fn drop(&mut self) {
        self.unmap();
    }
}

fn map_region(file: &mut File, index: usize, r: &CdsRegion) -> io::Result<RegionMapping> {
    let used = r.used;
    #[cfg(windows)]
    {
        // MapViewOfFileEx requires the offset to be allocation-granularity
        // aligned; region file_offsets already are (core_region_alignment).
        let base = crate::share::platform::windows::OsWindows::map_file_memory(
            file,
            r.file_offset,
            used,
            0,
            false,
        )?;
        return Ok(RegionMapping {
            region_index: index,
            file_offset: r.file_offset,
            used,
            read_only: r.read_only,
            mapped_base: base,
            data: None,
        });
    }
    #[cfg(not(windows))]
    {
        use std::io::{Read, Seek, SeekFrom};
        let mut data = vec![0u8; used as usize];
        file.seek(SeekFrom::Start(r.file_offset))?;
        file.read_exact(&mut data)?;
        Ok(RegionMapping {
            region_index: index,
            file_offset: r.file_offset,
            used,
            read_only: r.read_only,
            mapped_base: 0,
            data: Some(data),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::cds::archive::{self, CdsRegion, GenericHeader};

    fn test_archive(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cds_filemap_test_{}.jsa", tag))
    }

    #[test]
    fn header_extension_roundtrip() {
        let mut h = FileMapHeader::new();
        h.set_jvm_ident("OpenJDK 64-Bit Server VM 25-internal");
        h.obj_alignment = 16;
        h.max_heap_size = 1 << 30;
        h.type_profile_level = 4;
        h.heap_root_segments = HeapRootSegments {
            base_offset: 4096,
            count: 3,
            roots_count: 12,
            max_size_in_bytes: 256,
            max_size_in_elems: 64,
        };
        let ext = h.serialize_extension();
        assert_eq!(ext.len(), FileMapHeader::EXTENSION_SIZE);
        let parsed = FileMapHeader::parse_extension(&ext).unwrap();
        assert_eq!(parsed.obj_alignment, 16);
        assert_eq!(parsed.max_heap_size, 1 << 30);
        assert_eq!(parsed.type_profile_level, 4);
        assert_eq!(
            parsed.jvm_ident_str(),
            "OpenJDK 64-Bit Server VM 25-internal"
        );
        assert_eq!(parsed.heap_root_segments.roots_count, 12);
        assert_eq!(parsed.heap_root_segments.count, 3);
    }

    #[test]
    fn extension_size_matches_header_layout() {
        assert_eq!(CORE_HEADER_SIZE, 504);
        assert_eq!(FileMapHeader::EXTENSION_SIZE, 480);
        assert_eq!(CORE_HEADER_SIZE + FileMapHeader::EXTENSION_SIZE, 984);
    }

    #[test]
    fn filemap_open_map_and_crc() {
        let path = test_archive("map");
        let payload_rw = vec![0xABu8; 100];
        let payload_ro = b"read-only metaspace".to_vec();

        // Dump side: write header placeholder, append regions, rewrite header.
        let mut regions = vec![CdsRegion::default(); 5];
        let mut fh = FileMapHeader::new();
        fh.set_jvm_ident("test-vm");
        let generic = GenericHeader::static_archive();
        {
            let (mut file, data_start) =
                FileMapInfo::for_writing(&path, generic, &regions, &fh, None).unwrap();
            assert_eq!(data_start, archive::CORE_REGION_ALIGNMENT as u64);
            let (off, crc) = archive::append_region(&mut file, &payload_rw).unwrap();
            regions[RW_REGION].file_offset = off;
            regions[RW_REGION].used = payload_rw.len() as u64;
            regions[RW_REGION].crc = crc as i32;
            let (off2, crc2) = archive::append_region(&mut file, &payload_ro).unwrap();
            regions[RO_REGION].file_offset = off2;
            regions[RO_REGION].used = payload_ro.len() as u64;
            regions[RO_REGION].crc = crc2 as i32;
            regions[RO_REGION].read_only = true;
            file.flush().unwrap();
        }
        {
            let mut info = FileMapInfo::open(&path).unwrap();
            info.regions = regions.clone();
            info.write_header_block(generic, None).unwrap();
            info.file.flush().unwrap();
        }

        // Load side: open, map regions, verify CRCs.
        let mut info = FileMapInfo::open(&path).unwrap();
        assert!(info.is_static());
        assert_eq!(info.header.jvm_ident_str(), "test-vm");
        info.map_file_to_memory().unwrap();
        info.verify_region_checksums().unwrap();
        let rw = info.mapped_region(RW_REGION).unwrap();
        assert_eq!(rw.bytes(), payload_rw.as_slice());
        let ro = info.mapped_region(RO_REGION).unwrap();
        assert_eq!(ro.bytes(), payload_ro.as_slice());
        assert!(info.mapped_region(BM_REGION).is_none());
        info.unmap();
        drop(info);

        // Corrupt payload -> CRC failure.
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            f.seek(SeekFrom::Start(regions[RW_REGION].file_offset))
                .unwrap();
            f.write_all(&[0xFF]).unwrap();
        }
        let mut info = FileMapInfo::open(&path).unwrap();
        info.map_file_to_memory().unwrap();
        assert!(info.verify_region_checksums().is_err());
        drop(info);
        let _ = std::fs::remove_file(&path);
    }
}
