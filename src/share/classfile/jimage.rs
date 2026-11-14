/*
 * Port of OpenJDK 25 HotSpot JImage Subsystem (share/jimage/imageFile.hpp, jimage.hpp)
 *
 * Implements JImage reader for OpenJDK runtime modules (lib/modules, magic 0xCAFEDADA).
 * Reads standard OpenJDK library classfiles directly from the installed JDK runtime image.
 */

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Error, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const JIMAGE_MAGIC: u32 = 0xCAFE_DADA;
pub const JIMAGE_MAJOR_VERSION: u16 = 1;
pub const JIMAGE_MINOR_VERSION: u16 = 0;

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

pub struct JImageReader {
    file: File,
    header: JImageHeader,
    path_to_offset: HashMap<String, (u64, usize, bool)>,
    classes: HashMap<String, (u64, usize, bool)>,
}

impl JImageReader {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let mut file = File::open(path.as_ref())?;
        let header = JImageHeader::read_from(&mut file)?;

        let mut reader = Self {
            file,
            header,
            path_to_offset: HashMap::new(),
            classes: HashMap::new(),
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
        let index_offset = 28u64;
        let redirect_table_size = (header.table_length as u64) * 4;
        let offsets_table_size = (header.table_length as u64) * 4;
        let location_table_offset = index_offset + redirect_table_size + offsets_table_size;
        let strings_offset = location_table_offset + (header.locations_size as u64);
        let content_offset = strings_offset + (header.strings_size as u64);

        // Read strings table into memory
        self.file.seek(SeekFrom::Start(strings_offset))?;
        let mut strings_buf = vec![0u8; header.strings_size as usize];
        self.file.read_exact(&mut strings_buf)?;

        // Read locations table into memory
        self.file.seek(SeekFrom::Start(location_table_offset))?;
        let mut locations_buf = vec![0u8; header.locations_size as usize];
        self.file.read_exact(&mut locations_buf)?;

        // Read offsets table to traverse valid resource locations
        self.file
            .seek(SeekFrom::Start(index_offset + redirect_table_size))?;
        let mut offsets_bytes = vec![0u8; offsets_table_size as usize];
        self.file.read_exact(&mut offsets_bytes)?;

        for chunk in offsets_bytes.chunks_exact(4) {
            let loc_offset = u32::from_le_bytes(chunk.try_into().unwrap()) as usize;
            if loc_offset != 0 && loc_offset < locations_buf.len() {
                if let Some((path, data_offset, size, compressed)) =
                    Self::parse_location(&locations_buf[loc_offset..], &strings_buf)
                {
                    let location = (content_offset + data_offset, size, compressed);
                    if let Some((module, class)) =
                        path.strip_prefix('/').and_then(|path| path.split_once('/'))
                        && !module.is_empty()
                        && let Some(class) = class.strip_suffix(".class")
                        && class != "module-info"
                    {
                        if module == "java.base" {
                            self.classes.insert(class.to_owned(), location);
                        } else {
                            self.classes.entry(class.to_owned()).or_insert(location);
                        }
                    }
                    self.path_to_offset.insert(path, location);
                }
            }
        }

        Ok(())
    }

    fn read_string(strings: &[u8], offset: usize) -> Option<String> {
        if offset >= strings.len() {
            return None;
        }
        let end = strings[offset..].iter().position(|&b| b == 0)?;
        String::from_utf8(strings[offset..offset + end].to_vec()).ok()
    }

    fn parse_location(mut data: &[u8], strings: &[u8]) -> Option<(String, u64, usize, bool)> {
        let mut module_str = String::new();
        let mut parent_str = String::new();
        let mut base_str = String::new();
        let mut ext_str = String::new();
        let mut offset = 0u64;
        let mut uncompressed_size = 0usize;
        let mut compressed = false;

        while !data.is_empty() {
            let header = data[0];
            data = &data[1..];
            let tag = header >> 3;
            let length = usize::from(header & 0x07) + 1;

            if tag == 0 {
                // LOCATION_END
                break;
            }
            if length > data.len() {
                return None;
            }

            // Decode unsigned attribute value up to 8 bytes
            let mut val = 0u64;
            for &byte in &data[..length] {
                val = (val << 8) | u64::from(byte);
            }
            data = &data[length..];

            match tag {
                1 => {
                    // MODULE
                    if let Some(s) = Self::read_string(strings, val as usize) {
                        module_str = s;
                    }
                }
                2 => {
                    // PARENT
                    if let Some(s) = Self::read_string(strings, val as usize) {
                        parent_str = s;
                    }
                }
                3 => {
                    // BASE
                    if let Some(s) = Self::read_string(strings, val as usize) {
                        base_str = s;
                    }
                }
                4 => {
                    // EXTENSION
                    if let Some(s) = Self::read_string(strings, val as usize) {
                        ext_str = s;
                    }
                }
                5 => {
                    // OFFSET
                    offset = val;
                }
                6 => compressed = val != 0,
                7 => {
                    // UNCOMPRESSED_SIZE
                    uncompressed_size = val as usize;
                }
                _ => {}
            }
        }

        if base_str.is_empty() {
            return None;
        }

        let full_path = if ext_str.is_empty() {
            if parent_str.is_empty() {
                format!("/{module_str}/{base_str}")
            } else {
                format!("/{module_str}/{parent_str}/{base_str}")
            }
        } else if parent_str.is_empty() {
            format!("/{module_str}/{base_str}.{ext_str}")
        } else {
            format!("/{module_str}/{parent_str}/{base_str}.{ext_str}")
        };

        Some((full_path, offset, uncompressed_size, compressed))
    }

    /// Read raw resource bytes given the resource path (e.g. "/java.base/java/lang/Object.class")
    pub fn read_resource(&mut self, full_path: &str) -> io::Result<Vec<u8>> {
        if let Some(&location) = self.path_to_offset.get(full_path) {
            self.readLocation(location)
        } else {
            Err(Error::other(format!(
                "Resource not found in JImage: {full_path}"
            )))
        }
    }

    pub(crate) fn findClass(&mut self, name: &str) -> io::Result<Option<Vec<u8>>> {
        self.classes
            .get(name)
            .copied()
            .map(|location| self.readLocation(location))
            .transpose()
    }

    pub(crate) fn hasClass(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    pub(crate) fn paths_with_suffix(&self, suffix: &str) -> Vec<String> {
        self.path_to_offset
            .keys()
            .filter(|path| path.ends_with(suffix))
            .cloned()
            .collect()
    }

    fn readLocation(
        &mut self,
        (offset, size, compressed): (u64, usize, bool),
    ) -> io::Result<Vec<u8>> {
        if compressed {
            return Err(Error::new(
                io::ErrorKind::Unsupported,
                "Compressed JImage resource",
            ));
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(Error::other)?;
        bytes.resize(size, 0);
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    /// Read a Java classfile (e.g. module "java.base", class "java/lang/String")
    pub fn read_class(&mut self, module: &str, class_name: &str) -> io::Result<Vec<u8>> {
        let full_path = format!("/{module}/{class_name}.class");
        self.read_resource(&full_path)
    }

    pub fn has_resource(&self, full_path: &str) -> bool {
        self.path_to_offset.contains_key(full_path)
    }

    pub fn indexed_resources_count(&self) -> usize {
        self.path_to_offset.len()
    }
}
