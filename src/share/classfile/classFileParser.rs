/*
 * Port of OpenJDK 25 HotSpot ClassFileParser (share/classfile/classFileParser.hpp, classFileParser.cpp)
 *
 * ClassFileParser parses JVM class files according to the Java Virtual Machine Specification (JVMS).
 * It verifies bytecode magic, versions, constant pool structure, class/field/method names,
 * attributes (Code, SourceFile, BootstrapMethods, NestHost, NestMembers, PermittedSubclasses, Record),
 * and computes field memory layouts and oop maps.
 */

use std::fmt;

pub const JAVA_CLASSFILE_MAGIC: u32 = 0xCAFEBABE;
pub const JAVA_MIN_SUPPORTED_VERSION: u16 = 45;
pub const JAVA_MAX_SUPPORTED_VERSION: u16 = 69; // Java 25
pub const JAVA_PREVIEW_MINOR_VERSION: u16 = 65535;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ClassError {
    Truncated,
    InvalidMagic,
    UnsupportedVersion(u16, u16),
    InvalidConstantPool,
    InvalidConstantTag(u8),
    InvalidConstantIndex(u16),
    TrailingBytes,
    ClassFormatError(String),
}

impl fmt::Display for ClassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "Truncated class file"),
            Self::InvalidMagic => write!(f, "Invalid magic number"),
            Self::UnsupportedVersion(maj, min) => {
                write!(f, "Unsupported class file version {maj}.{min}")
            }
            Self::InvalidConstantPool => write!(f, "Invalid constant pool"),
            Self::InvalidConstantTag(tag) => write!(f, "Invalid constant pool tag: {tag}"),
            Self::InvalidConstantIndex(idx) => write!(f, "Invalid constant pool index: {idx}"),
            Self::TrailingBytes => write!(f, "Unexpected trailing bytes in class file"),
            Self::ClassFormatError(msg) => write!(f, "ClassFormatError: {msg}"),
        }
    }
}

impl std::error::Error for ClassError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publicity {
    Internal,
    Broadcast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalNameType {
    LegalClass,
    LegalField,
    LegalMethod,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Constant<'a> {
    Unusable,
    Utf8(&'a [u8]),
    Integer(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class(u16),
    String(u16),
    FieldRef(u16, u16),
    MethodRef(u16, u16),
    InterfaceMethodRef(u16, u16),
    NameAndType(u16, u16),
    MethodHandle(u8, u16),
    MethodType(u16),
    Dynamic(u16, u16),
    InvokeDynamic(u16, u16),
    Module(u16),
    Package(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute<'a> {
    pub name: &'a [u8],
    pub bytes: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member<'a> {
    pub accessFlags: u16,
    pub name: &'a [u8],
    pub descriptor: &'a [u8],
    pub attributes: Vec<Attribute<'a>>,
}

impl<'a> Member<'a> {
    pub fn code_attribute(&self) -> Option<&'a [u8]> {
        self.attributes
            .iter()
            .find(|a| a.name == b"Code")
            .map(|a| a.bytes)
    }

    pub fn signature_index(&self) -> Option<u16> {
        self.attributes
            .iter()
            .find(|a| a.name == b"Signature")
            .and_then(|a| a.bytes.get(..2))
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn exceptions(&self) -> Vec<u16> {
        if let Some(attr) = self.attributes.iter().find(|a| a.name == b"Exceptions") {
            if attr.bytes.len() >= 2 {
                let count = u16::from_be_bytes([attr.bytes[0], attr.bytes[1]]) as usize;
                let mut list = Vec::with_capacity(count);
                for i in 0..count {
                    let offset = 2 + i * 2;
                    if let Some(slice) = attr.bytes.get(offset..offset + 2) {
                        list.push(u16::from_be_bytes([slice[0], slice[1]]));
                    }
                }
                return list;
            }
        }
        Vec::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OopMapBlock {
    pub offset: usize,
    pub count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct OopMapBlocksBuilder {
    pub nonstatic_oop_maps: Vec<OopMapBlock>,
}

impl OopMapBlocksBuilder {
    pub fn new(max_blocks: usize) -> Self {
        Self {
            nonstatic_oop_maps: Vec::with_capacity(max_blocks),
        }
    }

    pub fn add(&mut self, offset: usize, count: usize) {
        if let Some(last) = self.nonstatic_oop_maps.last_mut() {
            if last.offset + last.count == offset {
                last.count += count;
                return;
            }
        }
        self.nonstatic_oop_maps.push(OopMapBlock { offset, count });
    }

    pub fn compact(&mut self) {
        if self.nonstatic_oop_maps.len() <= 1 {
            return;
        }
        let mut compacted = Vec::with_capacity(self.nonstatic_oop_maps.len());
        for block in &self.nonstatic_oop_maps {
            if let Some(last) = compacted.last_mut() {
                let last: &mut OopMapBlock = last;
                if last.offset + last.count == block.offset {
                    last.count += block.count;
                    continue;
                }
            }
            compacted.push(*block);
        }
        self.nonstatic_oop_maps = compacted;
    }

    pub fn blocks(&self) -> &[OopMapBlock] {
        &self.nonstatic_oop_maps
    }
}

#[derive(Debug, Clone, Default)]
pub struct FieldLayoutInfo {
    pub instance_size: usize,
    pub nonstatic_field_size: usize,
    pub static_field_size: usize,
    pub has_nonstatic_fields: bool,
    pub oop_maps: Vec<OopMapBlock>,
}

#[derive(Debug, Clone)]
pub struct ClassFile<'a> {
    pub majorVersion: u16,
    pub minorVersion: u16,
    pub accessFlags: u16,
    pub name: &'a [u8],
    pub superName: Option<&'a [u8]>,
    pub interfaces: Vec<&'a [u8]>,
    pub constants: Vec<Constant<'a>>,
    pub fields: Vec<Member<'a>>,
    pub methods: Vec<Member<'a>>,
    pub attributes: Vec<Attribute<'a>>,
}

impl<'a> ClassFile<'a> {
    /// Post-parse structural verification mirroring the checks HotSpot's
    /// classFileParser performs while parsing: legal names, access flag
    /// combinations, superclass/interface constraints and duplicate members.
    fn verify_structure(&self) -> Result<(), ClassError> {
        let is_interface = (self.accessFlags & 0x0200) != 0;
        let is_object = self.name == b"java/lang/Object";

        if !ClassFileParser::verify_legal_class_name(self.name) {
            return Err(ClassError::ClassFormatError(format!(
                "Illegal class name {}",
                String::from_utf8_lossy(self.name)
            )));
        }

        for &iface in &self.interfaces {
            if !ClassFileParser::verify_legal_class_name(iface) {
                return Err(ClassError::ClassFormatError(format!(
                    "Illegal interface name {}",
                    String::from_utf8_lossy(iface)
                )));
            }
            if iface == self.name {
                return Err(ClassError::ClassFormatError(
                    "Interface cannot be implemented by itself".to_string(),
                ));
            }
        }

        if is_interface {
            if self.accessFlags & 0x0010 != 0 {
                return Err(ClassError::ClassFormatError(
                    "Interface cannot be final".to_string(),
                ));
            }
            if self.accessFlags & 0x0400 == 0 {
                return Err(ClassError::ClassFormatError(
                    "Interface must be abstract".to_string(),
                ));
            }
            if let Some(super_name) = self.superName
                && super_name != b"java/lang/Object"
            {
                return Err(ClassError::ClassFormatError(
                    "Interface superclass must be java/lang/Object".to_string(),
                ));
            }
        } else if is_object {
            if self.superName.is_some() {
                return Err(ClassError::ClassFormatError(
                    "java/lang/Object must not have a superclass".to_string(),
                ));
            }
            if !self.interfaces.is_empty() {
                return Err(ClassError::ClassFormatError(
                    "java/lang/Object must not implement interfaces".to_string(),
                ));
            }
        } else if self.superName.is_none() {
            return Err(ClassError::ClassFormatError(
                "Class must have a superclass".to_string(),
            ));
        }

        let mut seen_fields = std::collections::HashSet::with_capacity(self.fields.len());
        for field in &self.fields {
            if !ClassFileParser::verify_legal_field_name(field.name)
                || !ClassFileParser::verify_legal_field_signature(field.descriptor)
                || decodeString(field.name).is_err()
                || decodeString(field.descriptor).is_err()
            {
                return Err(ClassError::ClassFormatError(format!(
                    "Illegal field {}{}",
                    String::from_utf8_lossy(field.name),
                    String::from_utf8_lossy(field.descriptor)
                )));
            }
            if !seen_fields.insert((field.name, field.descriptor)) {
                return Err(ClassError::ClassFormatError(format!(
                    "Duplicate field {}{}",
                    String::from_utf8_lossy(field.name),
                    String::from_utf8_lossy(field.descriptor)
                )));
            }
        }

        let mut seen_methods = std::collections::HashSet::with_capacity(self.methods.len());
        for method in &self.methods {
            if !ClassFileParser::verify_legal_method_name(method.name, self.majorVersion)
                || !ClassFileParser::verify_legal_method_signature(method.name, method.descriptor)
                || decodeString(method.name).is_err()
                || decodeString(method.descriptor).is_err()
            {
                return Err(ClassError::ClassFormatError(format!(
                    "Illegal method {}{}",
                    String::from_utf8_lossy(method.name),
                    String::from_utf8_lossy(method.descriptor)
                )));
            }
            if !seen_methods.insert((method.name, method.descriptor)) {
                return Err(ClassError::ClassFormatError(format!(
                    "Duplicate method {}{}",
                    String::from_utf8_lossy(method.name),
                    String::from_utf8_lossy(method.descriptor)
                )));
            }
        }

        if is_interface {
            for method in &self.methods {
                if method.name == b"<init>" {
                    return Err(ClassError::ClassFormatError(format!(
                        "Interface {} cannot declare {}",
                        String::from_utf8_lossy(self.name),
                        String::from_utf8_lossy(method.name)
                    )));
                }
                let has_code = method.code_attribute().is_some();
                let is_static = (method.accessFlags & 0x0008) != 0;
                let is_private = (method.accessFlags & 0x0002) != 0;
                let is_abstract = (method.accessFlags & 0x0400) != 0;
                if is_abstract && has_code {
                    return Err(ClassError::ClassFormatError(
                        "Abstract interface method cannot have a Code attribute".to_string(),
                    ));
                }
                if !is_abstract && !is_static && !is_private && !has_code {
                    return Err(ClassError::ClassFormatError(
                        "Interface method without Code must be abstract".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn source_file(&self) -> Option<&'a [u8]> {
        let attr = self.attributes.iter().find(|a| a.name == b"SourceFile")?;
        let idx = u16::from_be_bytes([*attr.bytes.first()?, *attr.bytes.get(1)?]);
        utf8(&self.constants, idx).ok()
    }

    pub fn bootstrap_methods(&self) -> Option<&'a [u8]> {
        self.attributes
            .iter()
            .find(|a| a.name == b"BootstrapMethods")
            .map(|a| a.bytes)
    }

    pub fn nest_host(&self) -> Option<&'a [u8]> {
        let attr = self.attributes.iter().find(|a| a.name == b"NestHost")?;
        let idx = u16::from_be_bytes([*attr.bytes.first()?, *attr.bytes.get(1)?]);
        className(&self.constants, idx).ok()
    }

    pub fn nest_members(&self) -> Vec<u16> {
        if let Some(attr) = self.attributes.iter().find(|a| a.name == b"NestMembers") {
            if attr.bytes.len() >= 2 {
                let count = u16::from_be_bytes([attr.bytes[0], attr.bytes[1]]) as usize;
                let mut list = Vec::with_capacity(count);
                for i in 0..count {
                    let offset = 2 + i * 2;
                    if let Some(slice) = attr.bytes.get(offset..offset + 2) {
                        list.push(u16::from_be_bytes([slice[0], slice[1]]));
                    }
                }
                return list;
            }
        }
        Vec::new()
    }

    pub fn permitted_subclasses(&self) -> Vec<u16> {
        if let Some(attr) = self
            .attributes
            .iter()
            .find(|a| a.name == b"PermittedSubclasses")
        {
            if attr.bytes.len() >= 2 {
                let count = u16::from_be_bytes([attr.bytes[0], attr.bytes[1]]) as usize;
                let mut list = Vec::with_capacity(count);
                for i in 0..count {
                    let offset = 2 + i * 2;
                    if let Some(slice) = attr.bytes.get(offset..offset + 2) {
                        list.push(u16::from_be_bytes([slice[0], slice[1]]));
                    }
                }
                return list;
            }
        }
        Vec::new()
    }

    pub fn record_components(&self) -> Option<&'a [u8]> {
        self.attributes
            .iter()
            .find(|a| a.name == b"Record")
            .map(|a| a.bytes)
    }

    pub fn parse(bytes: &'a [u8], enablePreview: bool) -> Result<Self, ClassError> {
        let mut parser = ClassFileParser::new(bytes, Publicity::Broadcast, enablePreview);
        parser.parse()
    }
}

pub struct ClassFileParser<'a> {
    reader: Reader<'a>,
    pub_level: Publicity,
    enable_preview: bool,
    major_version: u16,
    minor_version: u16,
}

impl<'a> ClassFileParser<'a> {
    pub fn new(bytes: &'a [u8], pub_level: Publicity, enable_preview: bool) -> Self {
        Self {
            reader: Reader(bytes),
            pub_level,
            enable_preview,
            major_version: 0,
            minor_version: 0,
        }
    }

    pub fn publicity(&self) -> Publicity {
        self.pub_level
    }

    pub fn verify_legal_class_name(name: &[u8]) -> bool {
        if name.is_empty() {
            return false;
        }
        if name.starts_with(b"[") {
            // Array type descriptor check
            return name.len() > 1;
        }
        !name.contains(&b'.') && !name.contains(&b';') && !name.contains(&b'[')
    }

    pub fn verify_legal_field_name(name: &[u8]) -> bool {
        if name.is_empty() {
            return false;
        }
        !name.contains(&b'.')
            && !name.contains(&b';')
            && !name.contains(&b'[')
            && !name.contains(&b'/')
    }

    pub fn verify_legal_method_name(name: &[u8], _major_version: u16) -> bool {
        if name.is_empty() {
            return false;
        }
        if name == b"<init>" || name == b"<clinit>" {
            return true;
        }
        !name.contains(&b'.')
            && !name.contains(&b';')
            && !name.contains(&b'[')
            && !name.contains(&b'/')
            && !name.contains(&b'<')
            && !name.contains(&b'>')
    }

    pub fn verify_legal_field_signature(sig: &[u8]) -> bool {
        if sig.is_empty() {
            return false;
        }
        let mut idx = 0;
        while idx < sig.len() && sig[idx] == b'[' {
            idx += 1;
        }
        if idx >= sig.len() {
            return false;
        }
        match sig[idx] {
            b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' => idx + 1 == sig.len(),
            b'L' => sig.ends_with(b";") && sig.len() > idx + 2,
            _ => false,
        }
    }

    pub fn verify_legal_method_signature(name: &[u8], sig: &[u8]) -> bool {
        if !sig.starts_with(b"(") {
            return false;
        }
        let Some(close_paren) = sig.iter().position(|&b| b == b')') else {
            return false;
        };
        let ret = &sig[close_paren + 1..];
        if ret.is_empty() {
            return false;
        }
        if name == b"<init>" && ret != b"V" {
            return false;
        }
        true
    }

    pub fn parse(&mut self) -> Result<ClassFile<'a>, ClassError> {
        let magic = self.reader.read::<4>()?;
        if magic != [0xca, 0xfe, 0xba, 0xbe] {
            return Err(ClassError::InvalidMagic);
        }

        self.minor_version = self.reader.readU16()?;
        self.major_version = self.reader.readU16()?;

        let supported_minor = self.major_version < 56
            || self.minor_version == 0
            || (self.major_version == JAVA_MAX_SUPPORTED_VERSION
                && self.minor_version == JAVA_PREVIEW_MINOR_VERSION
                && self.enable_preview);

        if !(JAVA_MIN_SUPPORTED_VERSION..=JAVA_MAX_SUPPORTED_VERSION).contains(&self.major_version)
            || !supported_minor
        {
            return Err(ClassError::UnsupportedVersion(
                self.major_version,
                self.minor_version,
            ));
        }

        let constants = self.reader.constants()?;
        let accessFlags = self.reader.readU16()?;
        let name = className(&constants, self.reader.readU16()?)?;
        let superIndex = self.reader.readU16()?;
        let superName = (superIndex != 0)
            .then(|| className(&constants, superIndex))
            .transpose()?;

        let interfaces = self
            .reader
            .table(|reader| className(&constants, reader.readU16()?))?;
        let fields = self.reader.members(&constants)?;
        let methods = self.reader.members(&constants)?;
        let attributes = self.reader.attributes(&constants)?;

        if !self.reader.0.is_empty() {
            return Err(ClassError::TrailingBytes);
        }

        let class = ClassFile {
            majorVersion: self.major_version,
            minorVersion: self.minor_version,
            accessFlags,
            name,
            superName,
            interfaces,
            constants,
            fields,
            methods,
            attributes,
        };
        class.verify_structure()?;
        Ok(class)
    }
}

fn utf8<'a>(constants: &[Constant<'a>], index: u16) -> Result<&'a [u8], ClassError> {
    match constants.get(usize::from(index)) {
        Some(Constant::Utf8(bytes)) => Ok(bytes),
        _ => Err(ClassError::InvalidConstantIndex(index)),
    }
}

fn className<'a>(constants: &[Constant<'a>], index: u16) -> Result<&'a [u8], ClassError> {
    match constants.get(usize::from(index)) {
        Some(Constant::Class(nameIndex)) => utf8(constants, *nameIndex),
        _ => Err(ClassError::InvalidConstantIndex(index)),
    }
}

pub struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    pub fn take(&mut self, length: usize) -> Result<&'a [u8], ClassError> {
        let (bytes, rest) = self
            .0
            .split_at_checked(length)
            .ok_or(ClassError::Truncated)?;
        self.0 = rest;
        Ok(bytes)
    }

    pub fn read<const N: usize>(&mut self) -> Result<[u8; N], ClassError> {
        let (bytes, rest) = self.0.split_first_chunk().ok_or(ClassError::Truncated)?;
        self.0 = rest;
        Ok(*bytes)
    }

    pub fn readU16(&mut self) -> Result<u16, ClassError> {
        Ok(u16::from_be_bytes(self.read()?))
    }

    pub fn table<T>(
        &mut self,
        mut entry: impl FnMut(&mut Self) -> Result<T, ClassError>,
    ) -> Result<Vec<T>, ClassError> {
        let count = usize::from(self.readU16()?);
        (0..count).map(|_| entry(self)).collect()
    }

    pub fn constants(&mut self) -> Result<Vec<Constant<'a>>, ClassError> {
        let count = usize::from(self.readU16()?);
        if count == 0 {
            return Err(ClassError::InvalidConstantPool);
        }

        let mut constants = Vec::with_capacity(count.min(self.0.len().saturating_add(1)));
        constants.push(Constant::Unusable);
        while constants.len() < count {
            let constant = match self.read::<1>()?[0] {
                1 => {
                    let length = usize::from(self.readU16()?);
                    Constant::Utf8(self.take(length)?)
                }
                3 => Constant::Integer(i32::from_be_bytes(self.read()?)),
                4 => Constant::Float(f32::from_be_bytes(self.read()?)),
                5 => Constant::Long(i64::from_be_bytes(self.read()?)),
                6 => Constant::Double(f64::from_be_bytes(self.read()?)),
                7 => Constant::Class(self.readU16()?),
                8 => Constant::String(self.readU16()?),
                9 => Constant::FieldRef(self.readU16()?, self.readU16()?),
                10 => Constant::MethodRef(self.readU16()?, self.readU16()?),
                11 => Constant::InterfaceMethodRef(self.readU16()?, self.readU16()?),
                12 => Constant::NameAndType(self.readU16()?, self.readU16()?),
                15 => Constant::MethodHandle(self.read::<1>()?[0], self.readU16()?),
                16 => Constant::MethodType(self.readU16()?),
                17 => Constant::Dynamic(self.readU16()?, self.readU16()?),
                18 => Constant::InvokeDynamic(self.readU16()?, self.readU16()?),
                19 => Constant::Module(self.readU16()?),
                20 => Constant::Package(self.readU16()?),
                tag => return Err(ClassError::InvalidConstantTag(tag)),
            };
            let wide = matches!(constant, Constant::Long(_) | Constant::Double(_));
            constants.push(constant);
            if wide {
                if constants.len() == count {
                    return Err(ClassError::InvalidConstantPool);
                }
                constants.push(Constant::Unusable);
            }
        }
        Ok(constants)
    }

    pub fn members(&mut self, constants: &[Constant<'a>]) -> Result<Vec<Member<'a>>, ClassError> {
        self.table(|reader| {
            Ok(Member {
                accessFlags: reader.readU16()?,
                name: utf8(constants, reader.readU16()?)?,
                descriptor: utf8(constants, reader.readU16()?)?,
                attributes: reader.attributes(constants)?,
            })
        })
    }

    pub fn attributes(
        &mut self,
        constants: &[Constant<'a>],
    ) -> Result<Vec<Attribute<'a>>, ClassError> {
        self.table(|reader| {
            let name = utf8(constants, reader.readU16()?)?;
            let length = u32::from_be_bytes(reader.read()?) as usize;
            Ok(Attribute {
                name,
                bytes: reader.take(length)?,
            })
        })
    }
}

impl<'a> ClassFileParser<'a> {
    pub fn compute_field_layout(fields: &[Member<'a>]) -> FieldLayoutInfo {
        let mut static_fields = 0;
        let mut nonstatic_fields = 0;
        let mut oop_builder = OopMapBlocksBuilder::new(fields.len().max(1));

        for (i, field) in fields.iter().enumerate() {
            let is_static = (field.accessFlags & 0x0008) != 0;
            if is_static {
                static_fields += 1;
            } else {
                nonstatic_fields += 1;
                if field.descriptor.starts_with(b"L") || field.descriptor.starts_with(b"[") {
                    oop_builder.add(i, 1);
                }
            }
        }
        oop_builder.compact();

        FieldLayoutInfo {
            instance_size: nonstatic_fields * 8 + 16,
            nonstatic_field_size: nonstatic_fields,
            static_field_size: static_fields,
            has_nonstatic_fields: nonstatic_fields > 0,
            oop_maps: oop_builder.nonstatic_oop_maps,
        }
    }
}

/// Encodes a Rust string as modified UTF-8 (NUL -> C0 80) for jimage
/// constant-pool reconstruction.
pub fn encodeModifiedUtf8(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    super::symbolTable::unicode_to_modified_utf8(&units)
}

pub fn decodeString(bytes: &[u8]) -> std::io::Result<String> {
    decodeUtf16Units(bytes).map(|units| String::from_utf16_lossy(&units))
}

pub(crate) fn decodeUtf16Units(mut bytes: &[u8]) -> std::io::Result<Vec<u16>> {
    let mut units = Vec::with_capacity(bytes.len());
    while let Some((&first, rest)) = bytes.split_first() {
        let invalid = || std::io::Error::other("Invalid modified UTF-8");
        let (value, count) = match first {
            1..=0x7f => (u16::from(first), 0),
            0xc0..=0xdf => (u16::from(first & 0x1f), 1),
            0xe0..=0xef => (u16::from(first & 0x0f), 2),
            _ => return Err(invalid()),
        };
        let (tail, remaining) = rest.split_at_checked(count).ok_or_else(invalid)?;
        let mut unit = value;
        for &byte in tail {
            if byte & 0xc0 != 0x80 {
                return Err(invalid());
            }
            unit = (unit << 6) | u16::from(byte & 0x3f);
        }
        if (count == 1 && (1..0x80).contains(&unit)) || (count == 2 && unit < 0x800) {
            return Err(invalid());
        }
        units.push(unit);
        bytes = remaining;
    }
    Ok(units)
}
