/*
 * Port of OpenJDK 25 HotSpot CDSConstants (share/cds/cdsConstants.hpp/.cpp)
 *
 * CDSConstants exposes well-known constants and field offsets that must stay
 * stable between the archive writer (-Xshare:dump) and the loader, so that a
 * CDS archive produced by one VM can be consumed by another. Values that are
 * VM-layout dependent are exposed as named entries; the Rust port keeps the
 * same names and fills in the constants that are compile-time fixed.
 */

/// Number of CDS file map regions (MetaspaceShared::n_regions).
pub const NUM_CDS_REGIONS: usize = 5;

// Archive magic values from share/include/cds.h
pub const CDS_ARCHIVE_MAGIC: u32 = 0xf00b_ab_a2;
pub const CDS_DYNAMIC_ARCHIVE_MAGIC: u32 = 0xf00b_ab_a8;
pub const CDS_PREIMAGE_ARCHIVE_MAGIC: u32 = 0xcafe_a07c;

/// Generic header is readable for versions >= this (share/include/cds.h).
pub const CDS_GENERIC_HEADER_SUPPORTED_MIN_VERSION: i32 = 13;
/// CURRENT_CDS_ARCHIVE_VERSION.
pub const CURRENT_CDS_ARCHIVE_VERSION: i32 = 19;

/// A named constant/offset pair (CDSConst).
#[derive(Debug, Clone, Copy)]
pub struct CdsConst {
    pub name: &'static str,
    pub value: usize,
}

/// Constants table — mirrors cdsConstants.cpp `constants[]` for the entries
/// that have fixed values in this port. VM-layout offsets (klass layouts,
/// compressed-oops bases) are resolved at runtime and are not part of this
/// static table; they return None via get_cds_offset.
pub const CONSTANTS: &[CdsConst] = &[
    CdsConst { name: "static_magic", value: CDS_ARCHIVE_MAGIC as usize },
    CdsConst { name: "dynamic_magic", value: CDS_DYNAMIC_ARCHIVE_MAGIC as usize },
    CdsConst { name: "preimage_magic", value: CDS_PREIMAGE_ARCHIVE_MAGIC as usize },
    CdsConst { name: "num_cds_regions", value: NUM_CDS_REGIONS },
    CdsConst { name: "current_cds_archive_version", value: CURRENT_CDS_ARCHIVE_VERSION as usize },
    CdsConst { name: "generic_header_supported_min_version", value: CDS_GENERIC_HEADER_SUPPORTED_MIN_VERSION as usize },
    // Region slot indices (FileMapInfo region order):
    //   0: RO metaspace, 1: RW metaspace, 2: bitmap, 3..4: heap regions.
    CdsConst { name: "ro_region_index", value: 0 },
    CdsConst { name: "rw_region_index", value: 1 },
    CdsConst { name: "bitmap_region_index", value: 2 },
];

/// Well-known named offsets recorded by cdsConstants.cpp `offsets[]`.
/// These are layout offsets that the Rust port does not yet materialize;
/// the table exists so callers can query by name.
pub const OFFSETS: &[&str] = &[
    "java_lang_Class::_klass_offset",
    "java_lang_Class::_java_mirror_offset",
    "InstanceKlass::layout_helper_offset",
    "Method::constMethod_offset",
    "Method::method_data_offset",
    "ConstantPool::tags_offset",
    "ConstantPool::_pool_index",
];

/// cdsConstants.cpp: get_cds_constant — look up a fixed constant by name.
pub fn get_cds_constant(name: &str) -> Option<usize> {
    CONSTANTS.iter().find(|c| c.name == name).map(|c| c.value)
}

/// cdsConstants.cpp: get_cds_offset — VM-layout dependent offsets are not
/// available in the Rust port; returns None unless the name is a known slot.
pub fn get_cds_offset(name: &str) -> Option<usize> {
    if OFFSETS.contains(&name) {
        // Recognized name, unknown value — distinct from an unknown name.
        return None;
    }
    get_cds_constant(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_values() {
        assert_eq!(CDS_ARCHIVE_MAGIC, 0xf00baba2);
        assert_eq!(CDS_DYNAMIC_ARCHIVE_MAGIC, 0xf00baba8);
        assert_eq!(CDS_PREIMAGE_ARCHIVE_MAGIC, 0xcafea07c);
        assert_eq!(NUM_CDS_REGIONS, 5);
        assert_eq!(CURRENT_CDS_ARCHIVE_VERSION, 19);
    }

    #[test]
    fn constant_lookup() {
        assert_eq!(get_cds_constant("static_magic"), Some(0xf00baba2));
        assert_eq!(get_cds_constant("num_cds_regions"), Some(5));
        assert_eq!(get_cds_constant("nonexistent"), None);
    }
}
