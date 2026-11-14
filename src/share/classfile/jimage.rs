/*
 * Port of OpenJDK 25 HotSpot JImage Subsystem (share/jimage/imageFile.hpp, jimage.hpp)
 * and jdk.internal.jimage (BasicImageReader, ImageLocation, ImageStringsReader)
 *
 * Implements JImage reader for OpenJDK runtime modules (lib/modules, magic 0xCAFEDADA):
 * header validation, redirect/offsets/locations/strings tables, module-aware
 * resource indexing, and transparent decompression of compressed resources
 * ("zip" and "compact-cp" plugins via jimageDecompressor).
 */

use super::classFileParser::decodeString;
use super::jimageDecompressor::{self, StringsProvider};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Error, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const JIMAGE_MAGIC: u32 = 0xCAFE_DADA;
pub const JIMAGE_MAJOR_VERSION: u16 = 1;
pub const JIMAGE_MINOR_VERSION: u16 = 0;

const HEADER_SIZE: u64 = 28;

#[derive(Debug, Clone)]
pub struct JImageHeader {
    pub magic: u32,
    pub major_version: u16,
    pub minor_version: u16,
    pub flags: u32,
    pub resource_count: u32,
    pub table_length: u32,
    pub locations_size: u32,
    pub strings_size: u32,
}

impl JImageHeader {
    pub fn read_from(file: &mut File) -> io::Result<Self> {
        let mut buf = [0u8; 28];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut buf)?;

        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != JIMAGE_MAGIC {
            return Err(Error::other(format!(
                "Invalid JImage magic: 0x{:08X} (expected 0x{:08X})",
                magic, JIMAGE_MAGIC
            )));
        }

        let minor_version = u16::from_le_bytes(buf[4..6].try_into().unwrap());
        let major_version = u16::from_le_bytes(buf[6..8].try_into().unwrap());
        let flags = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        let resource_count = u32::from_le_bytes(buf[12..16].try_into().unwrap());
        let table_length = u32::from_le_bytes(buf[16..20].try_into().unwrap());
        let locations_size = u32::from_le_bytes(buf[20..24].try_into().unwrap());
        let strings_size = u32::from_le_bytes(buf[24..28].try_into().unwrap());

        if major_version != JIMAGE_MAJOR_VERSION || minor_version > JIMAGE_MINOR_VERSION {
            return Err(Error::other(format!(
                "Unsupported JImage version {major_version}.{minor_version}"
            )));
        }

        Ok(Self {
            magic,
            major_version,
            minor_version,
            flags,
            resource_count,
            table_length,
            locations_size,
            strings_size,
        })
    }
}

/// A single resource location inside the jimage data section.
/// `compressed_size == 0` means the resource is stored uncompressed with
/// `uncompressed_size` bytes; otherwise `compressed_size` bytes at `offset`
/// must be decompressed down to `uncompressed_size` bytes.
#[derive(Debug, Clone, Copy)]
pub struct JImageLocation {
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

impl JImageLocation {
    pub fn stored_size(&self) -> u64 {
        if self.compressed_size == 0 {
            self.uncompressed_size
        } else {
            self.compressed_size
        }
    }
}

#[derive(Debug, Clone)]
pub struct JImageResource {
    pub module: String,
    pub parent: String,
    pub base: String,
    pub extension: String,
    pub location: JImageLocation,
}

pub struct JImageReader {
    file: File,
    header: JImageHeader,
    /// Raw bytes of the jimage strings table (MUTF-8, NUL-terminated entries).
    strings: Vec<u8>,
    /// Absolute file offset where the data section begins.
    data_offset: u64,
    path_to_resource: HashMap<String, JImageResource>,
    /// Internal class name (e.g. "java/lang/Object") -> resource.
    /// java.base entries take precedence over other modules.
    classes: HashMap<String, JImageResource>,
    /// "module/class" -> resource for module-precise lookups.
    module_classes: HashMap<String, JImageResource>,
}

impl StringsProvider for JImageReader {
    fn get_string(&self, offset: u32) -> Option<String> {
        self.string_at(offset as usize)
    }
}

impl JImageReader {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let mut file = File::open(path.as_ref())?;
        let header = JImageHeader::read_from(&mut file)?;

        let mut reader = Self {
            file,
            header,
            strings: Vec::new(),
            data_offset: 0,
            path_to_resource: HashMap::new(),
            classes: HashMap::new(),
            module_classes: HashMap::new(),
        };
        reader.build_index()?;
        Ok(reader)
    }

    /// Automatically discovers and opens the system JDK lib/modules
    pub fn open_default() -> io::Result<Self> {
        if let Ok(java_home) = std::env::var("JAVA_HOME") {
            let candidate = PathBuf::from(java_home).join("lib").join("modules");
            if candidate.exists() {
                return Self::open(&candidate);
            }
        }

        let common_paths = [
            "C:\\Program Files\\Java\\jdk-21.0.12\\lib\\modules",
            "C:\\Program Files\\Java\\jdk-21\\lib\\modules",
            "C:\\Program Files\\Java\\jdk-25\\lib\\modules",
            "C:\\Program Files\\Java\\jdk-17\\lib\\modules",
        ];

        for path in &common_paths {
            let p = Path::new(path);
            if p.exists() {
                return Self::open(p);
            }
        }

        Err(Error::other("Could not find JDK lib/modules"))
    }

    pub fn header(&self) -> &JImageHeader {
        &self.header
    }

    fn build_index(&mut self) -> io::Result<()> {
        let header = &self.header;
        let table_size = u64::from(header.table_length) * 4;
        let offsets_table_offset = HEADER_SIZE + table_size; // skip redirect table
        let location_table_offset = offsets_table_offset + table_size;
        let strings_offset = location_table_offset + u64::from(header.locations_size);
        self.data_offset = strings_offset + u64::from(header.strings_size);

        // Read strings table into memory (needed by the decompressor).
        self.file.seek(SeekFrom::Start(strings_offset))?;
        self.strings = vec![0u8; header.strings_size as usize];
        self.file.read_exact(&mut self.strings)?;

        // Read locations table into memory
        self.file.seek(SeekFrom::Start(location_table_offset))?;
        let mut locations_buf = vec![0u8; header.locations_size as usize];
        self.file.read_exact(&mut locations_buf)?;

        // Scan every offsets-table entry; each non-zero entry points into the
        // locations buffer. Duplicate entries collapse in the maps below.
        self.file.seek(SeekFrom::Start(offsets_table_offset))?;
        let mut offsets_bytes = vec![0u8; table_size as usize];
        self.file.read_exact(&mut offsets_bytes)?;

        for chunk in offsets_bytes.chunks_exact(4) {
            let loc_offset = u32::from_le_bytes(chunk.try_into().unwrap()) as usize;
            if loc_offset == 0 || loc_offset >= locations_buf.len() {
                continue;
            }
            let Some(resource) = Self::parse_location(&locations_buf[loc_offset..], &self.strings)
            else {
                continue;
            };
            let path = resource_full_path(&resource);
            if let Some((module, class)) =
                path.strip_prefix('/').and_then(|path| path.split_once('/'))
                && !module.is_empty()
                && let Some(class) = class.strip_suffix(".class")
                && class != "module-info"
            {
                let keyed = format!("{module}/{class}");
                self.module_classes
                    .entry(keyed)
                    .or_insert_with(|| resource.clone());
                if module == "java.base" {
                    self.classes.insert(class.to_owned(), resource.clone());
                } else {
                    self.classes
                        .entry(class.to_owned())
                        .or_insert(resource.clone());
                }
            }
            self.path_to_resource.insert(path, resource);
        }

        Ok(())
    }

    fn string_at(&self, offset: usize) -> Option<String> {
        if offset >= self.strings.len() {
            return None;
        }
        let end = self.strings[offset..].iter().position(|&b| b == 0)?;
        decodeString(&self.strings[offset..offset + end]).ok()
    }

    /// Parses one location entry: a sequence of (kind, value) attributes
    /// terminated by ATTRIBUTE_END (kind 0). String attributes (module,
    /// parent, base, extension) hold offsets into the strings table;
    /// offset/compressed/uncompressed hold sizes and the data offset.
    fn parse_location(data: &[u8], strings: &[u8]) -> Option<JImageResource> {
        fn read_string(strings: &[u8], offset: u64) -> Option<String> {
            let offset = usize::try_from(offset).ok()?;
            if offset >= strings.len() {
                return None;
            }
            let end = strings[offset..].iter().position(|&b| b == 0)?;
            decodeString(&strings[offset..offset + end]).ok()
        }

        let mut module = String::new();
        let mut parent = String::new();
        let mut base = String::new();
        let mut extension = String::new();
        let mut offset = 0u64;
        let mut compressed_size = 0u64;
        let mut uncompressed_size = 0u64;

        let mut pos = 0usize;
        while pos < data.len() {
            let header = data[pos];
            pos += 1;
            let kind = header >> 3;
            if kind == 0 {
                break;
            }
            if kind > 7 {
                return None;
            }
            let length = usize::from(header & 0x07) + 1;
            let value_bytes = data.get(pos..pos + length)?;
            pos += length;
            let mut value = 0u64;
            for &byte in value_bytes {
                value = (value << 8) | u64::from(byte);
            }
            match kind {
                1 => module = read_string(strings, value)?,
                2 => parent = read_string(strings, value)?,
                3 => base = read_string(strings, value)?,
                4 => extension = read_string(strings, value)?,
                5 => offset = value,
                6 => compressed_size = value,
                7 => uncompressed_size = value,
                _ => {}
            }
        }

        if base.is_empty() {
            return None;
        }

        Some(JImageResource {
            module,
            parent,
            base,
            extension,
            location: JImageLocation {
                offset,
                compressed_size,
                uncompressed_size,
            },
        })
    }

    /// Read raw resource bytes given the resource path (e.g. "/java.base/java/lang/Object.class")
    pub fn read_resource(&mut self, full_path: &str) -> io::Result<Vec<u8>> {
        if let Some(resource) = self.path_to_resource.get(full_path) {
            self.read_location(resource.location)
        } else {
            Err(Error::other(format!(
                "Resource not found in JImage: {full_path}"
            )))
        }
    }

    /// Returns the module that provides the given internal class name, if any.
    pub fn class_module(&self, class_name: &str) -> Option<&str> {
        self.classes.get(class_name).map(|r| r.module.as_str())
    }

    /// Reads a class from a specific module (e.g. "java.base", "java/lang/Object").
    pub fn read_class_in_module(&mut self, module: &str, class_name: &str) -> io::Result<Vec<u8>> {
        let key = format!("{module}/{class_name}");
        if let Some(resource) = self.module_classes.get(&key) {
            return self.read_location(resource.location);
        }
        Err(Error::other(format!(
            "Class not found in JImage: /{module}/{class_name}.class"
        )))
    }

    pub(crate) fn findClass(&mut self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let location = self.classes.get(name).map(|resource| resource.location);
        location
            .map(|location| self.read_location(location))
            .transpose()
    }

    pub(crate) fn hasClass(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// All indexed resource paths ("/module/path/to/resource").
    pub fn resource_paths(&self) -> impl Iterator<Item = &str> {
        self.path_to_resource.keys().map(String::as_str)
    }

    #[allow(dead_code)]
    pub(crate) fn paths_with_suffix(&self, suffix: &str) -> Vec<String> {
        self.path_to_resource
            .keys()
            .filter(|path| path.ends_with(suffix))
            .cloned()
            .collect()
    }

    fn read_location(&mut self, location: JImageLocation) -> io::Result<Vec<u8>> {
        let stored_size = location.stored_size();
        if stored_size > (i32::MAX as u64) {
            return Err(Error::other(format!(
                "JImage resource too large: {stored_size} bytes"
            )));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(stored_size as usize)
            .map_err(Error::other)?;
        bytes.resize(stored_size as usize, 0);
        self.file
            .seek(SeekFrom::Start(self.data_offset + location.offset))?;
        self.file.read_exact(&mut bytes)?;

        if location.compressed_size == 0 {
            return Ok(bytes);
        }
        jimageDecompressor::decompress_resource(self, bytes)
    }

    /// Read a Java classfile (e.g. module "java.base", class "java/lang/String")
    pub fn read_class(&mut self, module: &str, class_name: &str) -> io::Result<Vec<u8>> {
        self.read_class_in_module(module, class_name)
    }

    pub fn has_resource(&self, full_path: &str) -> bool {
        self.path_to_resource.contains_key(full_path)
    }

    pub fn indexed_resources_count(&self) -> usize {
        self.path_to_resource.len()
    }

    pub fn compressed_resources_count(&self) -> usize {
        self.path_to_resource
            .values()
            .filter(|r| r.location.compressed_size != 0)
            .count()
    }

    pub fn resource_count(&self) -> u32 {
        self.header.resource_count
    }

    pub fn table_length(&self) -> u32 {
        self.header.table_length
    }
}

fn resource_full_path(resource: &JImageResource) -> String {
    let mut path = String::with_capacity(64);
    path.push('/');
    path.push_str(&resource.module);
    path.push('/');
    if !resource.parent.is_empty() {
        path.push_str(&resource.parent);
        path.push('/');
    }
    path.push_str(&resource.base);
    if !resource.extension.is_empty() {
        path.push('.');
        path.push_str(&resource.extension);
    }
    path
}
