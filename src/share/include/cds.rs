use std::ffi::c_char;

pub const NUM_CDS_REGIONS: usize = 5;
pub const CDS_ARCHIVE_MAGIC: u32 = 0xf00baba2;
pub const CDS_DYNAMIC_ARCHIVE_MAGIC: u32 = 0xf00baba8;
pub const CDS_PREIMAGE_ARCHIVE_MAGIC: u32 = 0xcafea07c;
pub const CDS_GENERIC_HEADER_SUPPORTED_MIN_VERSION: i32 = 13;
pub const CURRENT_CDS_ARCHIVE_VERSION: i32 = 19;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CdsFileMapRegion {
    pub crc: i32,
    pub read_only: i32,
    pub allow_exec: i32,
    pub is_heap_region: i32,
    pub is_bitmap_region: i32,
    pub mapped_from_file: i32,
    pub file_offset: usize,
    pub mapping_offset: usize,
    pub used: usize,
    pub oopmap_offset: usize,
    pub oopmap_size_in_bits: usize,
    pub ptrmap_offset: usize,
    pub ptrmap_size_in_bits: usize,
    pub mapped_base: Option<*mut c_char>,
    pub in_reserved_space: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GenericCdsFileMapHeader {
    pub magic: u32,
    pub crc: i32,
    pub version: i32,
    pub header_size: u32,
    pub base_archive_name_offset: u32,
    pub base_archive_name_size: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CdsFileMapHeaderBase {
    pub generic_header: GenericCdsFileMapHeader,
    pub regions: [CdsFileMapRegion; NUM_CDS_REGIONS],
}

impl CdsFileMapHeaderBase {
    pub fn magic(&self) -> u32 {
        self.generic_header.magic
    }

    pub fn is_supported(&self) -> bool {
        self.generic_header.version >= CDS_GENERIC_HEADER_SUPPORTED_MIN_VERSION
    }

    pub fn base_archive_name<'a>(&self, data: &'a [u8]) -> Option<&'a [u8]> {
        let h = &self.generic_header;
        if h.base_archive_name_size == 0 {
            return None;
        }
        let off = h.base_archive_name_offset as usize;
        let len = h.base_archive_name_size as usize;
        data.get(off..off + len)
    }
}
