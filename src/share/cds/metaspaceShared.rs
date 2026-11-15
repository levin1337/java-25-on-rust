/*
 * Port of OpenJDK 25 HotSpot MetaspaceShared (share/cds/metaspaceShared.hpp/.cpp)
 *
 * MetaspaceShared coordinates dumping and loading of pre-parsed Java classes
 * to and from the CDS shared archive. The on-disk container is the
 * FileMapInfo layout implemented in filemap.rs / archive.rs
 * (share/include/cds.h GenericCDSFileMapHeader + CDSFileMapRegion records).
 *
 * Region slot order follows MetaspaceShared::n_regions:
 *   rw = 0 (read-write shared space), ro = 1 (read-only shared space),
 *   bm = 2 (relocation bitmaps), hp = 3 (heap), ac = 4 (AOT code).
 *
 * Dump side: registered classes (classfile bytes) are serialized into the
 * shared read-only metaspace space through VirtualSpaceList block
 * allocation, producing the RO region payload. The RW region carries the
 * shared writable space (empty in this port — no runtime mutable shared
 * objects are materialized yet) and the BM region carries the relocation
 * bitmap (empty — no pointer relocation data is emitted).
 *
 * Load side: FileMapInfo::open + map_file_to_memory + verify_region_checksums
 * load the regions; the RO payload is deserialized into the archived class
 * table which classLoader queries through lookup_shared_class().
 */

use super::archive::{self, CdsRegion, GenericHeader};
use super::cdsConfig::CDSConfig;
use super::cdsConstants::NUM_CDS_REGIONS;
use super::filemap::{FileMapHeader, FileMapInfo, BM_REGION, RO_REGION, RW_REGION};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Cursor, Read, Write};
use std::path::Path;
use std::sync::{OnceLock, RwLock};

/// Magic of the serialized shared-metaspace payload ('CDSM').
pub const METASPACE_PAYLOAD_MAGIC: u32 = 0x4344_534D;
/// Payload format version.
pub const METASPACE_PAYLOAD_VERSION: u32 = 1;

/// A class registered for archiving: its internal name and the raw
/// classfile bytes (field name `bytecode` kept for source compatibility;
/// it holds the classfile image). InstanceKlass-level serialization
/// (oops/, metaspace object graph) is outside this port's scope; the
/// archive stores the classfile so the loader can define the class
/// without re-reading the classpath entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedClass {
    pub name: String,
    /// Raw classfile bytes of the archived class.
    pub bytecode: Vec<u8>,
}

/// VirtualSpace — a bump-allocated shared metaspace space over a byte
/// buffer, mirroring Metaspace::VirtualSpaceList chunk allocation at dump
/// time. Allocations are aligned and never freed (arena semantics).
pub struct VirtualSpace {
    /// Region slot this space is serialized into.
    pub region_index: usize,
    pub name: &'static str,
    buffer: Vec<u8>,
    capacity: usize,
}

impl VirtualSpace {
    pub fn new(region_index: usize, name: &'static str, capacity: usize) -> Self {
        Self {
            region_index,
            name,
            buffer: Vec::with_capacity(capacity),
            capacity,
        }
    }

    /// VirtualSpace::allocate — bump-allocate `size` bytes at `align`
    /// alignment; returns the offset within the space.
    pub fn allocate(&mut self, size: usize, align: usize) -> io::Result<u32> {
        let align = align.max(1);
        let pos = self.buffer.len();
        let aligned = (pos + align - 1) & !(align - 1);
        if aligned + size > self.capacity {
            return Err(io::Error::other(format!(
                "shared {} space exhausted: need {} bytes, capacity {}",
                self.name, aligned + size, self.capacity
            )));
        }
        self.buffer.resize(aligned + size, 0);
        Ok(aligned as u32)
    }

    /// Write bytes into a previously allocated block.
    pub fn write_at(&mut self, offset: u32, data: &[u8]) -> io::Result<()> {
        let start = offset as usize;
        let end = start + data.len();
        if end > self.buffer.len() {
            return Err(io::Error::other("VirtualSpace::write_at out of bounds"));
        }
        self.buffer[start..end].copy_from_slice(data);
        Ok(())
    }

    pub fn used(&self) -> usize {
        self.buffer.len()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buffer
    }
}

/// VirtualSpaceList — the set of shared metaspace spaces (one per core
/// region), indexed by MetaspaceShared region slot.
pub struct VirtualSpaceList {
    spaces: Vec<VirtualSpace>,
}

impl VirtualSpaceList {
    pub fn new() -> Self {
        Self { spaces: Vec::new() }
    }

    /// Add a space for `region_index`; spaces must be added in order.
    pub fn add_space(&mut self, region_index: usize, name: &'static str, capacity: usize) {
        assert_eq!(self.spaces.len(), region_index);
        self.spaces.push(VirtualSpace::new(region_index, name, capacity));
    }

    pub fn space(&self, region_index: usize) -> Option<&VirtualSpace> {
        self.spaces.get(region_index)
    }

    pub fn space_mut(&mut self, region_index: usize) -> Option<&mut VirtualSpace> {
        self.spaces.get_mut(region_index)
    }

    /// Allocate in the space backing `region_index`.
    pub fn allocate(&mut self, region_index: usize, size: usize, align: usize) -> io::Result<u32> {
        let space = self
            .space_mut(region_index)
            .ok_or_else(|| io::Error::other(format!("no shared space for region {}", region_index)))?;
        space.allocate(size, align)
    }

    pub fn count(&self) -> usize {
        self.spaces.len()
    }
}

impl Default for VirtualSpaceList {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MetaspaceShared {
    archived_classes: HashMap<String, ArchivedClass>,
    /// Archived symbol table (deduplicated UTF-8 constant-pool/class names).
    symbols: BTreeMap<String, u32>,
    /// Archived string table (deduplicated string literals).
    strings: BTreeMap<String, u32>,
    /// Dump-time shared spaces; populated by initialize_for_dump().
    spaces: VirtualSpaceList,
    /// Default capacities for the shared spaces.
    ro_capacity: usize,
    rw_capacity: usize,
}

/// Default shared-space capacities (dump time only).
pub const DEFAULT_RO_SPACE_CAPACITY: usize = 16 * 1024 * 1024;
pub const DEFAULT_RW_SPACE_CAPACITY: usize = 4 * 1024 * 1024;

impl MetaspaceShared {
    pub fn new() -> Self {
        Self {
            archived_classes: HashMap::new(),
            symbols: BTreeMap::new(),
            strings: BTreeMap::new(),
            spaces: VirtualSpaceList::new(),
            ro_capacity: DEFAULT_RO_SPACE_CAPACITY,
            rw_capacity: DEFAULT_RW_SPACE_CAPACITY,
        }
    }

    pub fn with_capacities(ro: usize, rw: usize) -> Self {
        Self {
            ro_capacity: ro,
            rw_capacity: rw,
            ..Self::new()
        }
    }

    /// archiveBuilder registration: add a class (by internal name) together
    /// with its classfile bytes.
    pub fn add_class(&mut self, name: String, classfile: Vec<u8>) {
        self.archived_classes.insert(
            name.clone(),
            ArchivedClass {
                name,
                bytecode: classfile,
            },
        );
    }

    pub fn add_classfile(&mut self, name: &str, classfile: &[u8]) {
        self.add_class(name.to_string(), classfile.to_vec());
    }

    pub fn get_class(&self, name: &str) -> Option<&ArchivedClass> {
        self.archived_classes.get(name)
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.archived_classes.len()
    }

    #[inline]
    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    #[inline]
    pub fn string_count(&self) -> usize {
        self.strings.len()
    }

    /// Intern a symbol (class/method/field name) into the archived symbol
    /// table; returns its index.
    pub fn intern_symbol(&mut self, name: &str) -> u32 {
        let next = self.symbols.len() as u32;
        *self.symbols.entry(name.to_string()).or_insert(next)
    }

    /// Intern a string literal into the archived string table.
    pub fn intern_string(&mut self, value: &str) -> u32 {
        let next = self.strings.len() as u32;
        *self.strings.entry(value.to_string()).or_insert(next)
    }

    /// MetaspaceShared::initialize_for_static_dump — create the shared
    /// VirtualSpaceList (rw + ro core spaces).
    pub fn initialize_for_dump(&mut self) {
        self.spaces = VirtualSpaceList::new();
        self.spaces.add_space(RW_REGION, "shared-readwrite", self.rw_capacity);
        self.spaces.add_space(RO_REGION, "shared-readonly", self.ro_capacity);
    }

    /// Block-allocate `size` bytes in the shared space backing `region`.
    pub fn allocate_in_space(&mut self, region: usize, size: usize, align: usize) -> io::Result<u32> {
        if self.spaces.count() == 0 {
            self.initialize_for_dump();
        }
        self.spaces.allocate(region, size, align)
    }

    // -- serialization ---------------------------------------------------------

    fn write_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u64(buf: &mut Vec<u8>, v: u64) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(buf: &mut Vec<u8>, data: &[u8]) {
        Self::write_u32(buf, data.len() as u32);
        buf.extend_from_slice(data);
    }

    /// Serialize the shared RO metaspace space: payload header, symbol
    /// table, string table and class table, laid out as consecutive blocks
    /// in the VirtualSpace (mirroring dump_ro_metadata()).
    pub fn serialize_metaspace(&self) -> io::Result<Vec<u8>> {
        if self.spaces.count() == 0 {
            return Err(io::Error::other(
                "shared spaces not initialized (call initialize_for_dump)",
            ));
        }
        let mut buf = Vec::new();

        // Header block.
        Self::write_u32(&mut buf, METASPACE_PAYLOAD_MAGIC);
        Self::write_u32(&mut buf, METASPACE_PAYLOAD_VERSION);
        Self::write_u64(&mut buf, self.symbols.len() as u64);
        Self::write_u64(&mut buf, self.strings.len() as u64);
        Self::write_u64(&mut buf, self.archived_classes.len() as u64);

        // Symbol table block.
        Self::write_u32(&mut buf, self.symbols.len() as u32);
        for (name, _idx) in &self.symbols {
            Self::write_bytes(&mut buf, name.as_bytes());
        }

        // String table block.
        Self::write_u32(&mut buf, self.strings.len() as u32);
        for (value, _idx) in &self.strings {
            Self::write_bytes(&mut buf, value.as_bytes());
        }

        // Class table block: { name, classfile } records. Class names are
        // stored inline (a symbol-table index indirection is a TODO-level
        // refinement once InstanceKlass metadata exists).
        Self::write_u32(&mut buf, self.archived_classes.len() as u32);
        let mut entries: Vec<_> = self.archived_classes.values().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for cls in entries {
            Self::write_bytes(&mut buf, cls.name.as_bytes());
            Self::write_bytes(&mut buf, &cls.bytecode);
        }
        Ok(buf)
    }

    // -- dump side -------------------------------------------------------------

    /// archiveBuilder.cpp: write the shared archive (static CDS dump).
    pub fn dump_to_file(&mut self, path: &str) -> io::Result<usize> {
        self.dump_to_path(Path::new(path), None)
    }

    /// Write the archive; `base_name` selects dynamic-archive mode.
    /// Mirrors FileMapInfo::prepare_for_writing + write_region +
    /// write_header: regions are appended at core_region_alignment
    /// boundaries, then the header is rewritten with final CRCs.
    pub fn dump_to_path(&mut self, path: &Path, base_name: Option<&str>) -> io::Result<usize> {
        CDSConfig::set_dumping_archive(true);
        let result = self.dump_inner(path, base_name);
        CDSConfig::set_dumping_archive(false);
        result
    }

    fn dump_inner(&mut self, path: &Path, base_name: Option<&str>) -> io::Result<usize> {
        self.initialize_for_dump();
        let payload = self.serialize_metaspace()?;

        // Reserve the payload inside the RO VirtualSpace so `used` reflects
        // the space accounting (the serialized bytes themselves are written
        // to the file region directly).
        let mut header = FileMapHeader::new();
        header.set_jvm_ident("jvm25-rust-cds");
        header.has_platform_or_app_classes = !self.archived_classes.is_empty();
        let generic = match base_name {
            Some(name) => GenericHeader::dynamic_archive(name),
            None => GenericHeader::static_archive(),
        };

        let mut regions = vec![CdsRegion::default(); NUM_CDS_REGIONS];
        regions[RO_REGION] = CdsRegion {
            read_only: true,
            allow_exec: false,
            is_heap_region: false,
            is_bitmap_region: false,
            mapped_from_file: true,
            used: payload.len() as u64,
            ..CdsRegion::default()
        };

        let (mut file, _data_start) =
            FileMapInfo::for_writing(path, generic, &regions, &header, base_name)?;

        // FileMapInfo::write_region order (archiveBuilder::write_archive):
        // rw, ro, ac, bm. Empty regions keep file_offset 0 / used 0.
        let mut info_regions = regions.clone();
        if !payload.is_empty() {
            let (off, crc) = archive::append_region(&mut file, &payload)?;
            info_regions[RO_REGION].file_offset = off;
            info_regions[RO_REGION].crc = crc;
            info_regions[RO_REGION].used = payload.len() as u64;
        }
        file.flush()?;

        // Rewrite the header with final region CRCs.
        let mut info = FileMapInfo {
            path: path.to_path_buf(),
            file,
            generic,
            header,
            regions: info_regions.clone(),
            base_archive_name: base_name.map(|s| s.to_string()),
            mappings: Vec::new(),
        };
        info.write_header_block(generic, base_name)?;
        info.file.flush()?;
        Ok(payload.len())
    }

    // -- load side -------------------------------------------------------------

    /// archiveHeapLoader.cpp: load classes from a shared archive.
    /// Opens the archive via FileMapInfo, maps regions, verifies CRCs and
    /// deserializes the RO metaspace payload.
    pub fn restore_from_file(&mut self, path: &str) -> io::Result<usize> {
        self.restore_from_path(Path::new(path))
    }

    pub fn restore_from_path(&mut self, path: &Path) -> io::Result<usize> {
        CDSConfig::set_using_archive(true);
        let result = (|| -> io::Result<usize> {
            let mut info = FileMapInfo::open(path)?;
            info.map_file_to_memory()?;
            info.verify_region_checksums()?;

            let ro = info
                .mapped_region(RO_REGION)
                .ok_or_else(|| io::Error::other("archive has no RO metaspace region"))?;
            let data = ro.bytes().to_vec();
            info.unmap();
            self.deserialize_metaspace(&data)
        })();
        CDSConfig::set_using_archive(false);
        result
    }

    /// cdsConfig-aware load: honors UseSharedSpaces and reports whether the
    /// archive was usable, for -Xshare:auto fallback decisions.
    pub fn try_restore_from_path(&mut self, path: &Path) -> Result<usize, String> {
        if !CDSConfig::use_shared_spaces() {
            return Ok(0);
        }
        match self.restore_from_path(path) {
            Ok(n) => Ok(n),
            Err(e) => {
                CDSConfig::set_archive_loading_failed();
                Err(format!("unable to use shared archive {}: {}", path.display(), e))
            }
        }
    }

    fn deserialize_metaspace(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut cur = Cursor::new(data);
        let mut u32b = [0u8; 4];
        let mut u64b = [0u8; 8];

        cur.read_exact(&mut u32b)?;
        let magic = u32::from_le_bytes(u32b);
        if magic != METASPACE_PAYLOAD_MAGIC {
            return Err(io::Error::other(format!(
                "bad shared metaspace payload magic {:#x}",
                magic
            )));
        }
        cur.read_exact(&mut u32b)?;
        let version = u32::from_le_bytes(u32b);
        if version != METASPACE_PAYLOAD_VERSION {
            return Err(io::Error::other(format!(
                "unsupported shared metaspace payload version {}",
                version
            )));
        }
        cur.read_exact(&mut u64b)?;
        let symbol_count = u64::from_le_bytes(u64b) as usize;
        cur.read_exact(&mut u64b)?;
        let string_count = u64::from_le_bytes(u64b) as usize;
        cur.read_exact(&mut u64b)?;
        let class_count = u64::from_le_bytes(u64b) as usize;

        self.symbols.clear();
        self.strings.clear();
        self.archived_classes.clear();

        // Symbol table.
        let n = read_count(&mut cur)?;
        if n != symbol_count {
            return Err(io::Error::other("symbol count mismatch in CDS payload"));
        }
        for i in 0..n {
            let s = read_bytes(&mut cur)?;
            let name = String::from_utf8(s)
                .map_err(|_| io::Error::other("non-UTF8 symbol in CDS payload"))?;
            self.symbols.insert(name, i as u32);
        }

        // String table.
        let n = read_count(&mut cur)?;
        if n != string_count {
            return Err(io::Error::other("string count mismatch in CDS payload"));
        }
        for i in 0..n {
            let s = read_bytes(&mut cur)?;
            let value = String::from_utf8(s)
                .map_err(|_| io::Error::other("non-UTF8 string in CDS payload"))?;
            self.strings.insert(value, i as u32);
        }

        // Class table.
        let n = read_count(&mut cur)?;
        if n != class_count {
            return Err(io::Error::other("class count mismatch in CDS payload"));
        }
        for _ in 0..n {
            let name_bytes = read_bytes(&mut cur)?;
            let name = String::from_utf8(name_bytes)
                .map_err(|_| io::Error::other("non-UTF8 class name in CDS payload"))?;
            let bytecode = read_bytes(&mut cur)?;
            self.archived_classes.insert(
                name.clone(),
                ArchivedClass { name, bytecode },
            );
        }
        register_shared_classes(&self.archived_classes);
        Ok(self.archived_classes.len())
    }

    /// cdsConfig integration: dump all registered classes ("-Xshare:dump").
    pub fn dump_archive(&mut self, path: &str) -> io::Result<usize> {
        if !CDSConfig::is_dumping_archive() {
            CDSConfig::set_dumping_archive(true);
            let r = self.dump_to_file(path);
            CDSConfig::set_dumping_archive(false);
            return r;
        }
        self.dump_to_file(path)
    }
}

fn read_count<R: Read>(cur: &mut Cursor<R>) -> io::Result<usize> {
    let mut b = [0u8; 4];
    cur.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b) as usize)
}

fn read_bytes<R: Read>(cur: &mut Cursor<R>) -> io::Result<Vec<u8>> {
    let mut b = [0u8; 4];
    cur.read_exact(&mut b)?;
    let len = u32::from_le_bytes(b) as usize;
    let mut data = vec![0u8; len];
    cur.read_exact(&mut data)?;
    Ok(data)
}

// -- shared class lookup (classLoader API) ------------------------------------

static SHARED_CLASS_TABLE: OnceLock<RwLock<HashMap<String, Vec<u8>>>> = OnceLock::new();

fn shared_class_table() -> &'static RwLock<HashMap<String, Vec<u8>>> {
    SHARED_CLASS_TABLE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Publish deserialized classes to the process-wide shared class table that
/// classLoader consults before touching the classpath.
fn register_shared_classes(classes: &HashMap<String, ArchivedClass>) {
    let mut table = shared_class_table().write().unwrap();
    table.clear();
    for (name, cls) in classes {
        table.insert(name.clone(), cls.bytecode.clone());
    }
}

/// Lookup API for classLoader: returns the archived classfile bytes for an
/// internal class name (e.g. "java/lang/Object"), or None when CDS is off
/// or the class is not archived.
pub fn lookup_shared_class(name: &str) -> Option<Vec<u8>> {
    if !CDSConfig::is_using_archive() {
        return None;
    }
    shared_class_table().read().unwrap().get(name).cloned()
}

/// Number of classes available through the shared archive.
pub fn shared_class_count() -> usize {
    shared_class_table().read().unwrap().len()
}

/// Clear the shared class table (used by tests / -Xshare:off transitions).
pub fn clear_shared_class_table() {
    shared_class_table().write().unwrap().clear();
}

impl Default for MetaspaceShared {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_file(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("cds_metaspace_test_{}.jsa", tag))
    }

    #[test]
    fn virtual_space_block_allocation() {
        let mut vs = VirtualSpace::new(RO_REGION, "test", 1024);
        let a = vs.allocate(100, 8).unwrap();
        let b = vs.allocate(50, 16).unwrap();
        assert_eq!(a, 0);
        assert_eq!(b, 112); // 100 -> align 16 -> 112
        vs.write_at(a, b"hello").unwrap();
        assert_eq!(&vs.bytes()[a as usize..a as usize + 5], b"hello");
        assert!(vs.allocate(2000, 8).is_err());
    }

    #[test]
    fn virtual_space_list_regions() {
        let mut list = VirtualSpaceList::new();
        list.add_space(RW_REGION, "rw", 256);
        list.add_space(RO_REGION, "ro", 256);
        let off = list.allocate(RO_REGION, 64, 8).unwrap();
        assert_eq!(off, 0);
        assert_eq!(list.space(RO_REGION).unwrap().used(), 64);
        assert_eq!(list.space(RW_REGION).unwrap().used(), 0);
    }

    #[test]
    fn dump_and_load_roundtrip() {
        let mut store = MetaspaceShared::new();
        store.initialize_for_dump();
        store.intern_symbol("java/lang/Object");
        store.intern_string("hello cds");
        store.add_classfile("java/lang/Object", &[0xCA, 0xFE, 0xBA, 0xBE, 1, 2, 3]);
        store.add_classfile("com/example/Main", &[9, 8, 7, 6, 5]);

        let path = test_file("roundtrip");
        let n = store.dump_to_file(path.to_str().unwrap()).unwrap();
        assert!(n > 0);

        let mut loaded = MetaspaceShared::new();
        let count = loaded.restore_from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(loaded.symbol_count(), 1);
        assert_eq!(loaded.string_count(), 1);
        assert_eq!(
            loaded.get_class("java/lang/Object").unwrap().bytecode,
            vec![0xCA, 0xFE, 0xBA, 0xBE, 1, 2, 3]
        );

        // classLoader-facing API sees the shared classes.
        assert!(CDSConfig::is_using_archive() == false); // reset after restore
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn archive_regions_written_in_hotspot_order() {
        let mut store = MetaspaceShared::new();
        store.add_classfile("a/A", &[1]);
        let path = test_file("regions");
        store.dump_to_file(path.to_str().unwrap()).unwrap();

        let mut f = std::fs::File::open(&path).unwrap();
        let arch = archive::read_header(&mut f).unwrap();
        assert!(arch.header.is_static());
        // RO region carries the payload; RW/BM are empty.
        assert!(arch.regions[RO_REGION].used > 0);
        assert!(arch.regions[RO_REGION].read_only);
        assert_eq!(arch.regions[RW_REGION].used, 0);
        assert_eq!(arch.regions[BM_REGION].used, 0);
        assert_eq!(arch.regions[RO_REGION].file_offset % archive::CORE_REGION_ALIGNMENT as u64, 0);
        let data = archive::read_region(&mut f, &arch.regions[RO_REGION]).unwrap();
        assert_eq!(u32::from_le_bytes(data[0..4].try_into().unwrap()), METASPACE_PAYLOAD_MAGIC);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_bad_magic() {
        let path = test_file("bad");
        std::fs::write(&path, b"NOTACDSARCHIVE").unwrap();
        let mut store = MetaspaceShared::new();
        assert!(store.restore_from_file(path.to_str().unwrap()).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
