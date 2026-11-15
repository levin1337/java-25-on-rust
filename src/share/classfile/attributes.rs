/*
 * Port of OpenJDK 25 HotSpot classfile attribute parsers
 * (share/classfile/classFileParser.cpp attribute readers, JVMS §4.7).
 *
 * Structured parsers for the attributes the parser keeps as opaque byte
 * slices: BootstrapMethods, InnerClasses, EnclosingMethod, Record,
 * Signature (generic signatures, JVMS §4.7.9), Runtime[In]Visible[Parameter]
 * Annotations and TypeAnnotations (JVMS §4.7.16-21), and the Module
 * attribute of module-info.class (JVMS §4.7.25).
 */

use super::classFileParser::{Attribute, ClassFile, Constant, Member};
use crate::share::oops::annotations::{Annotation, AnnotationElementValue};
use std::io::{self, Error};

fn err(msg: impl Into<String>) -> Error {
    Error::other(msg.into())
}

/// Big-endian cursor over a classfile attribute payload.
pub struct AttributeCursor<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}

impl<'a> AttributeCursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    pub fn done(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    pub fn u1(&mut self) -> io::Result<u8> {
        let b = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| err("Truncated attribute"))?;
        self.pos += 1;
        Ok(b)
    }

    pub fn u2(&mut self) -> io::Result<u16> {
        let b = self
            .bytes
            .get(self.pos..self.pos + 2)
            .ok_or_else(|| err("Truncated attribute"))?;
        self.pos += 2;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u4(&mut self) -> io::Result<u32> {
        let b = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| err("Truncated attribute"))?;
        self.pos += 4;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn take(&mut self, len: usize) -> io::Result<&'a [u8]> {
        let out = self
            .bytes
            .get(self.pos..self.pos + len)
            .ok_or_else(|| err("Truncated attribute"))?;
        self.pos += len;
        Ok(out)
    }

    pub fn skip(&mut self, len: usize) -> io::Result<()> {
        self.take(len).map(|_| ())
    }

    pub fn attributes(
        &mut self,
        constants: &[Constant<'a>],
    ) -> io::Result<Vec<Attribute<'a>>> {
        let count = usize::from(self.u2()?);
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let name_index = self.u2()?;
            let name = utf8_at(constants, name_index)?;
            let length = self.u4()? as usize;
            let bytes = self.take(length)?;
            out.push(Attribute { name, bytes });
        }
        Ok(out)
    }
}

fn utf8_at<'a>(constants: &[Constant<'a>], index: u16) -> io::Result<&'a [u8]> {
    match constants.get(usize::from(index)) {
        Some(Constant::Utf8(bytes)) => Ok(bytes),
        _ => Err(err(format!("Constant pool index {index} is not a CONSTANT_Utf8"))),
    }
}

fn utf8_string_at(constants: &[Constant<'_>], index: u16) -> io::Result<String> {
    Ok(String::from_utf8_lossy(utf8_at(constants, index)?).into_owned())
}

/// Resolves a CONSTANT_Class_info index to the internal class name
/// (`java/lang/Object`); index 0 yields `None`.
fn class_name_at<'a>(
    constants: &[Constant<'a>],
    index: u16,
) -> io::Result<Option<&'a [u8]>> {
    if index == 0 {
        return Ok(None);
    }
    match constants.get(usize::from(index)) {
        Some(Constant::Class(name_index)) => Ok(Some(utf8_at(constants, *name_index)?)),
        _ => Err(err(format!(
            "Constant pool index {index} is not a CONSTANT_Class"
        ))),
    }
}

fn class_name_string_at(constants: &[Constant<'_>], index: u16) -> io::Result<Option<String>> {
    Ok(class_name_at(constants, index)?.map(|name| {
        String::from_utf8_lossy(name).into_owned()
    }))
}

fn name_and_type_at<'a>(
    constants: &[Constant<'a>],
    index: u16,
) -> io::Result<(&'a [u8], &'a [u8])> {
    match constants.get(usize::from(index)) {
        Some(Constant::NameAndType(name, desc)) => {
            Ok((utf8_at(constants, *name)?, utf8_at(constants, *desc)?))
        }
        _ => Err(err(format!(
            "Constant pool index {index} is not a CONSTANT_NameAndType"
        ))),
    }
}

// ---------------------------------------------------------------------------
// BootstrapMethods (JVMS §4.7.23)
// ---------------------------------------------------------------------------

/// One `bootstrap_method` entry: a `CONSTANT_MethodHandle` reference to the
/// bootstrap method plus static bootstrap arguments (constant pool indices).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapMethodEntry {
    pub bsm_ref: u16,
    pub arguments: Vec<u16>,
}

/// Parses the whole BootstrapMethods attribute payload.
pub fn parse_bootstrap_methods(bytes: &[u8]) -> io::Result<Vec<BootstrapMethodEntry>> {
    let mut cursor = AttributeCursor::new(bytes);
    let count = usize::from(cursor.u2()?);
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let bsm_ref = cursor.u2()?;
        let arg_count = usize::from(cursor.u2()?);
        let mut arguments = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            arguments.push(cursor.u2()?);
        }
        entries.push(BootstrapMethodEntry {
            bsm_ref,
            arguments,
        });
    }
    if !cursor.done() {
        return Err(err("Trailing bytes in BootstrapMethods attribute"));
    }
    Ok(entries)
}

/// Resolved view of one BootstrapMethods entry: the bootstrap method
/// `CONSTANT_MethodHandle` constant and the bootstrap argument constants.
/// `'b` is the borrow of the constant pool, `'a` the classfile bytes
/// lifetime carried by the constants themselves.
#[derive(Debug, Clone)]
pub struct BootstrapMethodSpec<'a, 'b> {
    pub bsm_ref: &'b Constant<'a>,
    pub arguments: Vec<&'b Constant<'a>>,
}

/// Resolved invokedynamic / condy call site descriptor.
#[derive(Debug, Clone, Copy)]
pub struct CallSiteInfo<'a> {
    /// Index into the BootstrapMethods attribute table.
    pub bootstrap_method_attr_index: u16,
    pub name: &'a [u8],
    pub descriptor: &'a [u8],
}

impl<'a> ClassFile<'a> {
    fn attribute_named(&self, name: &[u8]) -> Option<&Attribute<'a>> {
        self.attributes.iter().find(|attr| attr.name == name)
    }

    /// All BootstrapMethods entries in attribute order.
    pub fn bootstrap_method_entries(&self) -> io::Result<Vec<BootstrapMethodEntry>> {
        match self.attribute_named(b"BootstrapMethods") {
            Some(attr) => parse_bootstrap_methods(attr.bytes),
            None => Ok(Vec::new()),
        }
    }

    /// Returns the resolved bootstrap specifier for `index` (the
    /// `bootstrap_method_attr_index` carried by CONSTANT_InvokeDynamic /
    /// CONSTANT_Dynamic entries).
    pub fn bootstrap_method_spec(
        &self,
        index: u16,
    ) -> io::Result<BootstrapMethodSpec<'a, '_>> {
        let entries = self.bootstrap_method_entries()?;
        let entry = entries
            .get(usize::from(index))
            .ok_or_else(|| err(format!("BootstrapMethods index {index} out of bounds")))?;
        let bsm_ref = self
            .constants
            .get(usize::from(entry.bsm_ref))
            .ok_or_else(|| err("BootstrapMethods bsm_ref out of bounds"))?;
        if !matches!(bsm_ref, Constant::MethodHandle(_, _)) {
            return Err(err(
                "BootstrapMethods bsm_ref is not a CONSTANT_MethodHandle",
            ));
        }
        let mut arguments = Vec::with_capacity(entry.arguments.len());
        for &arg_index in &entry.arguments {
            let constant = self
                .constants
                .get(usize::from(arg_index))
                .ok_or_else(|| err("BootstrapMethods argument index out of bounds"))?;
            match constant {
                Constant::Integer(_)
                | Constant::Long(_)
                | Constant::Float(_)
                | Constant::Double(_)
                | Constant::String(_)
                | Constant::Class(_)
                | Constant::MethodType(_)
                | Constant::MethodHandle(_, _) => {}
                _ => {
                    return Err(err(
                        "BootstrapMethods argument is not a loadable constant",
                    ))
                }
            }
            arguments.push(constant);
        }
        Ok(BootstrapMethodSpec {
            bsm_ref,
            arguments,
        })
    }

    /// Resolves a CONSTANT_InvokeDynamic pool entry to its call site info.
    pub fn invokedynamic_site(&self, cp_index: u16) -> io::Result<CallSiteInfo<'a>> {
        match self.constants.get(usize::from(cp_index)) {
            Some(Constant::InvokeDynamic(bsm_index, nat_index)) => {
                let (name, descriptor) = name_and_type_at(&self.constants, *nat_index)?;
                Ok(CallSiteInfo {
                    bootstrap_method_attr_index: *bsm_index,
                    name,
                    descriptor,
                })
            }
            _ => Err(err("Constant is not a CONSTANT_InvokeDynamic")),
        }
    }

    /// Resolves a CONSTANT_Dynamic (condy) pool entry to its call site info.
    pub fn dynamic_constant_site(&self, cp_index: u16) -> io::Result<CallSiteInfo<'a>> {
        match self.constants.get(usize::from(cp_index)) {
            Some(Constant::Dynamic(bsm_index, nat_index)) => {
                let (name, descriptor) = name_and_type_at(&self.constants, *nat_index)?;
                Ok(CallSiteInfo {
                    bootstrap_method_attr_index: *bsm_index,
                    name,
                    descriptor,
                })
            }
            _ => Err(err("Constant is not a CONSTANT_Dynamic")),
        }
    }

    /// Resolves a CONSTANT_MethodHandle to (reference_kind, referenced
    /// constant). JVMS §4.4.8 / §5.4.3.5: kinds 1-2 reference Fieldref,
    /// 3-4 InterfaceMethodref, 5-6 and 8 Methodref, 7 Methodref or
    /// InterfaceMethodref, 9 InterfaceMethodref.
    pub fn method_handle(&self, index: u16) -> io::Result<(u8, &Constant<'a>)> {
        match self.constants.get(usize::from(index)) {
            Some(Constant::MethodHandle(kind, ref_index)) => {
                let constant = self
                    .constants
                    .get(usize::from(*ref_index))
                    .ok_or_else(|| err("MethodHandle reference index out of bounds"))?;
                let ok = match kind {
                    1 | 2 => matches!(constant, Constant::FieldRef(..)),
                    3 | 4 | 9 => matches!(constant, Constant::InterfaceMethodRef(..)),
                    5 | 6 | 8 => matches!(constant, Constant::MethodRef(..)),
                    7 => matches!(
                        constant,
                        Constant::MethodRef(..) | Constant::InterfaceMethodRef(..)
                    ),
                    _ => false,
                };
                if !ok {
                    return Err(err(format!(
                        "MethodHandle kind {kind} incompatible with referenced constant"
                    )));
                }
                Ok((*kind, constant))
            }
            _ => Err(err("Constant is not a CONSTANT_MethodHandle")),
        }
    }

    /// Resolves a CONSTANT_MethodType to its descriptor string.
    pub fn method_type_descriptor(&self, index: u16) -> io::Result<&'a [u8]> {
        match self.constants.get(usize::from(index)) {
            Some(Constant::MethodType(desc_index)) => utf8_at(&self.constants, *desc_index),
            _ => Err(err("Constant is not a CONSTANT_MethodType")),
        }
    }
}

// ---------------------------------------------------------------------------
// InnerClasses / EnclosingMethod (JVMS §4.7.6 / §4.7.7)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InnerClassEntry {
    pub inner_class_info_index: u16,
    pub outer_class_info_index: u16,
    pub inner_name_index: u16,
    pub inner_class_access_flags: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnclosingMethodInfo {
    pub class_index: u16,
    pub method_index: u16,
}

impl<'a> ClassFile<'a> {
    /// Parses the InnerClasses attribute (absent -> empty vec).
    pub fn inner_classes(&self) -> io::Result<Vec<InnerClassEntry>> {
        let Some(attr) = self.attribute_named(b"InnerClasses") else {
            return Ok(Vec::new());
        };
        let mut cursor = AttributeCursor::new(attr.bytes);
        let count = usize::from(cursor.u2()?);
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(InnerClassEntry {
                inner_class_info_index: cursor.u2()?,
                outer_class_info_index: cursor.u2()?,
                inner_name_index: cursor.u2()?,
                inner_class_access_flags: cursor.u2()?,
            });
        }
        if !cursor.done() {
            return Err(err("Trailing bytes in InnerClasses attribute"));
        }
        Ok(entries)
    }

    /// InnerClasses entry describing this class itself, if present.
    pub fn inner_class_entry(&self) -> io::Result<Option<InnerClassEntry>> {
        let mut this_index = None;
        for (i, constant) in self.constants.iter().enumerate() {
            if let Constant::Class(name_index) = constant
                && utf8_at(&self.constants, *name_index)? == self.name
            {
                this_index = Some(i as u16);
                break;
            }
        }
        let Some(this_index) = this_index else {
            return Ok(None);
        };
        Ok(self
            .inner_classes()?
            .into_iter()
            .find(|entry| entry.inner_class_info_index == this_index))
    }

    /// Parses the EnclosingMethod attribute; `method_index == 0` means the
    /// enclosing method is absent (anonymous/local class initializer).
    pub fn enclosing_method(&self) -> io::Result<Option<EnclosingMethodInfo>> {
        let Some(attr) = self.attribute_named(b"EnclosingMethod") else {
            return Ok(None);
        };
        let mut cursor = AttributeCursor::new(attr.bytes);
        let info = EnclosingMethodInfo {
            class_index: cursor.u2()?,
            method_index: cursor.u2()?,
        };
        if !cursor.done() {
            return Err(err("Trailing bytes in EnclosingMethod attribute"));
        }
        Ok(Some(info))
    }

    /// Resolved inner class name for an InnerClasses entry.
    pub fn inner_class_name(&self, entry: &InnerClassEntry) -> io::Result<Option<String>> {
        let name = if entry.inner_name_index == 0 {
            None
        } else {
            Some(utf8_string_at(&self.constants, entry.inner_name_index)?)
        };
        let outer = class_name_string_at(&self.constants, entry.outer_class_info_index)?;
        Ok(match (outer, name) {
            (Some(outer), Some(inner)) => Some(format!("{outer}/{inner}")),
            (Some(outer), None) => Some(outer),
            (None, Some(inner)) => Some(inner),
            (None, None) => None,
        })
    }

    /// Inner class this class is enclosed in, when it is a member class.
    pub fn enclosing_class(&self) -> io::Result<Option<String>> {
        let Some(entry) = self.inner_class_entry()? else {
            return Ok(None);
        };
        class_name_string_at(&self.constants, entry.outer_class_info_index)
    }
}

// ---------------------------------------------------------------------------
// Record (JVMS §4.7.30)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RecordComponentInfo<'a> {
    pub name_index: u16,
    pub descriptor_index: u16,
    pub attributes: Vec<Attribute<'a>>,
}

impl<'a> RecordComponentInfo<'a> {
    pub fn name(&self, class: &ClassFile<'a>) -> io::Result<String> {
        utf8_string_at(&class.constants, self.name_index)
    }

    pub fn descriptor(&self, class: &ClassFile<'a>) -> io::Result<String> {
        utf8_string_at(&class.constants, self.descriptor_index)
    }

    pub fn signature_index(&self) -> Option<u16> {
        self.attributes
            .iter()
            .find(|attr| attr.name == b"Signature")
            .and_then(|attr| attr.bytes.get(..2))
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn annotations<'b>(
        &self,
        class: &'b ClassFile<'a>,
        visible: bool,
    ) -> io::Result<Vec<Annotation>> {
        let name: &[u8] = if visible {
            b"RuntimeVisibleAnnotations"
        } else {
            b"RuntimeInvisibleAnnotations"
        };
        match self.attributes.iter().find(|attr| attr.name == name) {
            Some(attr) => parse_annotations(attr.bytes, &class.constants),
            None => Ok(Vec::new()),
        }
    }
}

impl<'a> ClassFile<'a> {
    /// Parses the Record attribute into structured components.
    /// Absent attribute -> empty vec.
    pub fn record_component_infos(&self) -> io::Result<Vec<RecordComponentInfo<'a>>> {
        let Some(attr) = self.attribute_named(b"Record") else {
            return Ok(Vec::new());
        };
        let mut cursor = AttributeCursor::new(attr.bytes);
        let count = usize::from(cursor.u2()?);
        let mut components = Vec::with_capacity(count);
        for _ in 0..count {
            let name_index = cursor.u2()?;
            let descriptor_index = cursor.u2()?;
            let attributes = cursor.attributes(&self.constants)?;
            components.push(RecordComponentInfo {
                name_index,
                descriptor_index,
                attributes,
            });
        }
        if !cursor.done() {
            return Err(err("Trailing bytes in Record attribute"));
        }
        Ok(components)
    }

    /// Whether this class is a record (has a Record attribute).
    pub fn is_record_class(&self) -> bool {
        self.attribute_named(b"Record").is_some()
    }
}

// ---------------------------------------------------------------------------
// Signature (generic signatures, JVMS §4.7.9)
// ---------------------------------------------------------------------------

/// A type argument inside a parameterized class type:
/// `?`, `? extends T`, `? super T`, or an exact `T`.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeArgument {
    /// `*` — unbounded wildcard.
    Star,
    /// `+ FieldTypeSignature` — `? extends T`.
    Extends(Box<TypeSignature>),
    /// `- FieldTypeSignature` — `? super T`.
    Super(Box<TypeSignature>),
    /// Exact `FieldTypeSignature`.
    Exact(Box<TypeSignature>),
}

/// One simple name segment of a ClassTypeSignature with its type arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassTypeSegment {
    /// Simple class identifier (e.g. `Map`, `Entry`).
    pub identifier: String,
    pub type_arguments: Vec<TypeArgument>,
}

/// ClassTypeSignature `Lpkg/Outer<A>.Inner<B>;`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassTypeSignature {
    /// Package prefix with `.` separators, empty for the unnamed package.
    pub package: String,
    /// Outer-to-inner class segments.
    pub segments: Vec<ClassTypeSegment>,
}

impl ClassTypeSignature {
    /// Internal binary name (`java/util/Map$Entry` style is not produced;
    /// nested classes keep `.` between segments).
    pub fn internal_name(&self) -> String {
        let mut name = self.package.replace('.', "/");
        if !name.is_empty() {
            name.push('/');
        }
        for (i, segment) in self.segments.iter().enumerate() {
            if i > 0 {
                name.push('.');
            }
            name.push_str(&segment.identifier);
        }
        name
    }
}

/// TypeSignature / FieldTypeSignature grammar production.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeSignature {
    /// BaseType: B C D F I J S Z.
    Primitive(u8),
    /// TypeVariableSignature `Tname;`.
    TypeVariable(String),
    /// ArrayTypeSignature `[ TypeSignature`.
    Array(Box<TypeSignature>),
    /// ClassTypeSignature.
    Class(ClassTypeSignature),
}

impl TypeSignature {
    /// JVM descriptor form (`Ljava/util/List;`, `[I`, `TT;` stays symbolic).
    pub fn descriptor(&self) -> String {
        match self {
            TypeSignature::Primitive(tag) => (*tag as char).to_string(),
            TypeSignature::TypeVariable(name) => format!("T{name};"),
            TypeSignature::Array(inner) => format!("[{}", inner.descriptor()),
            TypeSignature::Class(class) => {
                let mut out = String::from("L");
                out.push_str(&class.internal_name());
                for (i, segment) in class.segments.iter().enumerate() {
                    if i > 0 {
                        out.push('.');
                    }
                    if !segment.type_arguments.is_empty() {
                        out.push('<');
                        for (j, arg) in segment.type_arguments.iter().enumerate() {
                            if j > 0 {
                                out.push_str(", ");
                            }
                            out.push_str(&arg.descriptor());
                        }
                        out.push('>');
                    }
                }
                out.push(';');
                out
            }
        }
    }
}

impl TypeArgument {
    pub fn descriptor(&self) -> String {
        match self {
            TypeArgument::Star => "?".to_owned(),
            TypeArgument::Extends(t) => format!("? extends {}", t.descriptor()),
            TypeArgument::Super(t) => format!("? super {}", t.descriptor()),
            TypeArgument::Exact(t) => t.descriptor(),
        }
    }
}

/// MethodTypeSignature result: either a TypeSignature or void `V`.
#[derive(Debug, Clone, PartialEq)]
pub enum MethodResult {
    Type(TypeSignature),
    Void,
}

/// Parses a FieldTypeSignature / TypeSignature (JVMS §4.7.9.1).
pub fn parse_type_signature(bytes: &[u8]) -> io::Result<TypeSignature> {
    let mut cursor = AttributeCursor::new(bytes);
    let ty = parse_field_type(&mut cursor)?;
    if !cursor.done() {
        return Err(err("Trailing bytes in type signature"));
    }
    Ok(ty)
}

fn parse_field_type(cursor: &mut AttributeCursor<'_>) -> io::Result<TypeSignature> {
    let tag = cursor.u1()?;
    Ok(match tag {
        b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' => TypeSignature::Primitive(tag),
        b'T' => TypeSignature::TypeVariable(parse_identifier(cursor)?),
        b'[' => TypeSignature::Array(Box::new(parse_field_type(cursor)?)),
        b'L' => TypeSignature::Class(parse_class_type(cursor)?),
        _ => return Err(err(format!("Illegal type signature tag {}", tag as char))),
    })
}

fn parse_identifier(cursor: &mut AttributeCursor<'_>) -> io::Result<String> {
    let start = cursor.pos;
    while cursor.pos < cursor.bytes.len() {
        let b = cursor.bytes[cursor.pos];
        if b == b';' || b == b':' || b == b'<' || b == b'(' || b == b')'
            || b == b'[' || b == b'/' || b == b'.'
        {
            break;
        }
        cursor.pos += 1;
    }
    if cursor.pos == start {
        return Err(err("Empty identifier in signature"));
    }
    Ok(String::from_utf8_lossy(&cursor.bytes[start..cursor.pos]).into_owned())
}

fn parse_type_arguments(cursor: &mut AttributeCursor<'_>) -> io::Result<Vec<TypeArgument>> {
    // cursor is positioned just after '<'. TypeArgument+ entries are
    // concatenated without separators (JVMS §4.7.9.1); each
    // FieldTypeSignature is self-delimiting, so the list ends at '>'.
    let mut args = Vec::new();
    loop {
        let b = cursor.u1()?;
        let arg = match b {
            b'>' => break,
            b'*' => TypeArgument::Star,
            b'+' => TypeArgument::Extends(Box::new(parse_field_type(cursor)?)),
            b'-' => TypeArgument::Super(Box::new(parse_field_type(cursor)?)),
            _ => {
                cursor.pos -= 1;
                TypeArgument::Exact(Box::new(parse_field_type(cursor)?))
            }
        };
        args.push(arg);
    }
    Ok(args)
}

/// Parses the body of a ClassTypeSignature after the leading 'L'
/// (package, simple names and type arguments up to the closing ';').
fn parse_class_type(cursor: &mut AttributeCursor<'_>) -> io::Result<ClassTypeSignature> {
    let bytes = cursor.bytes;
    let start = cursor.pos;
    // The package prefix ends at the last '/' before the first '<' or ';'.
    // '.' cannot appear in the package part of a signature (packages use
    // '.' only in source form; classfile signatures keep '/').
    let limit = bytes[start..]
        .iter()
        .position(|&b| b == b'<' || b == b';')
        .map(|p| start + p)
        .unwrap_or(bytes.len());
    let last_slash = bytes[start..limit]
        .iter()
        .rposition(|&b| b == b'/')
        .map(|p| start + p);
    let package = match last_slash {
        Some(slash) => String::from_utf8_lossy(&bytes[start..slash]).into_owned(),
        None => String::new(),
    };
    cursor.pos = last_slash.map_or(start, |slash| slash + 1);

    let mut segments = Vec::new();
    loop {
        let identifier = parse_identifier(cursor)?;
        let type_arguments = if cursor
            .bytes
            .get(cursor.pos)
            .is_some_and(|&b| b == b'<')
        {
            cursor.pos += 1;
            parse_type_arguments(cursor)?
        } else {
            Vec::new()
        };
        segments.push(ClassTypeSegment {
            identifier,
            type_arguments,
        });
        match cursor.u1()? {
            b'.' => continue,
            b';' => break,
            other => {
                return Err(err(format!(
                    "Illegal class type signature terminator {}",
                    other as char
                )))
            }
        }
    }
    Ok(ClassTypeSignature { package, segments })
}

/// Parses a MethodTypeSignature `( params ) result` (JVMS §4.7.9.1).
pub fn parse_method_type_signature(bytes: &[u8]) -> io::Result<(Vec<TypeSignature>, MethodResult)> {
    let mut cursor = AttributeCursor::new(bytes);
    if cursor.u1()? != b'(' {
        return Err(err("Method signature must start with '('"));
    }
    let mut params = Vec::new();
    while cursor.bytes.get(cursor.pos).is_some_and(|&b| b != b')') {
        params.push(parse_field_type(&mut cursor)?);
    }
    if cursor.u1()? != b')' {
        return Err(err("Unterminated method signature parameter list"));
    }
    let result = match cursor.u1()? {
        b'V' => MethodResult::Void,
        _ => {
            cursor.pos -= 1;
            MethodResult::Type(parse_field_type(&mut cursor)?)
        }
    };
    if !cursor.done() {
        return Err(err("Trailing bytes in method signature"));
    }
    Ok((params, result))
}

impl<'a> Member<'a> {
    /// Resolves the Signature attribute of this member to raw signature
    /// bytes, when present.
    pub fn signature_bytes<'b>(&self, class: &'b ClassFile<'a>) -> io::Result<Option<&'a [u8]>> {
        let Some(index) = self.signature_index() else {
            return Ok(None);
        };
        Ok(Some(utf8_at(&class.constants, index)?))
    }

    /// Generic signature of the member decoded into a TypeSignature
    /// (fields) or (params, result) pair (methods).
    pub fn generic_signature<'b>(
        &self,
        class: &'b ClassFile<'a>,
    ) -> io::Result<Option<GenericSignature>> {
        let Some(bytes) = self.signature_bytes(class)? else {
            return Ok(None);
        };
        if self.descriptor.starts_with(b"(") {
            let (params, result) = parse_method_type_signature(bytes)?;
            Ok(Some(GenericSignature::Method {
                params,
                result,
            }))
        } else {
            Ok(Some(GenericSignature::Field(parse_type_signature(bytes)?)))
        }
    }
}

/// Decoded generic Signature attribute.
#[derive(Debug, Clone, PartialEq)]
pub enum GenericSignature {
    Field(TypeSignature),
    Method {
        params: Vec<TypeSignature>,
        result: MethodResult,
    },
}

impl<'a> ClassFile<'a> {
    /// Generic signature of the class itself (Signature attribute).
    pub fn generic_signature(&self) -> io::Result<Option<TypeSignature>> {
        let Some(attr) = self.attribute_named(b"Signature") else {
            return Ok(None);
        };
        let index = u16::from_be_bytes([attr.bytes[0], attr.bytes[1]]);
        let bytes = utf8_at(&self.constants, index)?;
        Ok(Some(parse_type_signature(bytes)?))
    }
}

// ---------------------------------------------------------------------------
// Annotations (JVMS §4.7.16-21)
// ---------------------------------------------------------------------------

/// Reads a const_value_index (u2) and resolves the referenced constant
/// pool entry for primitive-typed annotation element values.
fn const_value_at<'c, 'a>(
    cursor: &mut AttributeCursor<'_>,
    constants: &'c [Constant<'a>],
) -> io::Result<&'c Constant<'a>> {
    let index = cursor.u2()?;
    constants
        .get(usize::from(index))
        .ok_or_else(|| err(format!("Annotation const_value_index {index} out of bounds")))
}

fn parse_element_value(
    cursor: &mut AttributeCursor<'_>,
    constants: &[Constant<'_>],
) -> io::Result<AnnotationElementValue> {
    let tag = cursor.u1()?;
    Ok(match tag {
        b'B' => match const_value_at(cursor, constants)? {
            Constant::Integer(v) => AnnotationElementValue::Byte(*v as i8),
            _ => return Err(err("Annotation byte element is not a CONSTANT_Integer")),
        },
        b'C' => match const_value_at(cursor, constants)? {
            Constant::Integer(v) => AnnotationElementValue::Char(
                char::from_u32(u32::from(*v as u16)).unwrap_or('\u{fffd}'),
            ),
            _ => return Err(err("Annotation char element is not a CONSTANT_Integer")),
        },
        b'D' => match const_value_at(cursor, constants)? {
            Constant::Double(v) => AnnotationElementValue::Double(*v),
            _ => return Err(err("Annotation double element is not a CONSTANT_Double")),
        },
        b'F' => match const_value_at(cursor, constants)? {
            Constant::Float(v) => AnnotationElementValue::Float(*v),
            _ => return Err(err("Annotation float element is not a CONSTANT_Float")),
        },
        b'I' => match const_value_at(cursor, constants)? {
            Constant::Integer(v) => AnnotationElementValue::Int(*v),
            _ => return Err(err("Annotation int element is not a CONSTANT_Integer")),
        },
        b'J' => match const_value_at(cursor, constants)? {
            Constant::Long(v) => AnnotationElementValue::Long(*v),
            _ => return Err(err("Annotation long element is not a CONSTANT_Long")),
        },
        b'S' => match const_value_at(cursor, constants)? {
            Constant::Integer(v) => AnnotationElementValue::Short(*v as i16),
            _ => return Err(err("Annotation short element is not a CONSTANT_Integer")),
        },
        b'Z' => match const_value_at(cursor, constants)? {
            Constant::Integer(v) => AnnotationElementValue::Boolean(*v != 0),
            _ => return Err(err("Annotation boolean element is not a CONSTANT_Integer")),
        },
        b's' => match const_value_at(cursor, constants)? {
            Constant::String(idx) => {
                AnnotationElementValue::String(utf8_string_at(constants, *idx)?)
            }
            _ => return Err(err("Annotation string element is not a CONSTANT_String")),
        },
        b'e' => AnnotationElementValue::Enum {
            type_name: utf8_string_at(constants, cursor.u2()?)?,
            const_name: utf8_string_at(constants, cursor.u2()?)?,
        },
        b'c' => AnnotationElementValue::Class(utf8_string_at(constants, cursor.u2()?)?),
        b'@' => AnnotationElementValue::Annotation(Box::new(parse_annotation(
            cursor,
            constants,
        )?)),
        b'[' => {
            let count = usize::from(cursor.u2()?);
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                values.push(parse_element_value(cursor, constants)?);
            }
            AnnotationElementValue::Array(values)
        }
        _ => return Err(err(format!("Illegal annotation element tag {}", tag as char))),
    })
}

fn parse_annotation(
    cursor: &mut AttributeCursor<'_>,
    constants: &[Constant<'_>],
) -> io::Result<Annotation> {
    let type_index = cursor.u2()?;
    let type_name = utf8_string_at(constants, type_index)?;
    let pair_count = usize::from(cursor.u2()?);
    let mut annotation = Annotation::new(type_name);
    for _ in 0..pair_count {
        let element_name = utf8_string_at(constants, cursor.u2()?)?;
        let value = parse_element_value(cursor, constants)?;
        annotation.add_pair(element_name, value);
    }
    Ok(annotation)
}

/// Parses a Runtime[In]VisibleAnnotations payload (num_annotations u2,
/// then annotation_info[]).
pub fn parse_annotations(bytes: &[u8], constants: &[Constant<'_>]) -> io::Result<Vec<Annotation>> {
    let mut cursor = AttributeCursor::new(bytes);
    let count = usize::from(cursor.u2()?);
    let mut annotations = Vec::with_capacity(count);
    for _ in 0..count {
        annotations.push(parse_annotation(&mut cursor, constants)?);
    }
    if !cursor.done() {
        return Err(err("Trailing bytes in annotations attribute"));
    }
    Ok(annotations)
}

/// Parses Runtime[In]VisibleParameterAnnotations:
/// u1 num_parameters followed by per-parameter annotation lists.
pub fn parse_parameter_annotations(
    bytes: &[u8],
    constants: &[Constant<'_>],
) -> io::Result<Vec<Vec<Annotation>>> {
    let mut cursor = AttributeCursor::new(bytes);
    let count = usize::from(cursor.u1()?);
    let mut parameters = Vec::with_capacity(count);
    for _ in 0..count {
        let ann_count = usize::from(cursor.u2()?);
        let mut annotations = Vec::with_capacity(ann_count);
        for _ in 0..ann_count {
            annotations.push(parse_annotation(&mut cursor, constants)?);
        }
        parameters.push(annotations);
    }
    if !cursor.done() {
        return Err(err("Trailing bytes in parameter annotations attribute"));
    }
    Ok(parameters)
}

/// Non-empty target_info payload of a type_annotation (JVMS §4.7.20.1).
#[derive(Debug, Clone, PartialEq)]
pub enum TypeAnnotationTarget {
    /// type_parameter_target: declaration (0x00) or type (0x01).
    TypeParameter { declaration: bool, index: u8 },
    /// supertype_target (0x10).
    Supertype { index: u16 },
    /// formal_parameter_target (0x13-0x15).
    FormalParameter { index: u8 },
    /// throws_target (0x16).
    Throws { index: u16 },
    /// localvar_target (0x19, 0x1A): table of (start_pc, length, index).
    LocalVar {
        table: Vec<(u16, u16, u16)>,
    },
    /// exception_parameter_target catch (0x40) / type (0x41).
    ExceptionParameter { catch_type_index: u16 },
    /// Empty targets carry no payload (0x11,0x12,0x17,0x18,0x41-type? etc.).
    Empty,
}

/// One type_annotation structure (JVMS §4.7.20).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAnnotation {
    pub target_type: u8,
    pub target_info: TypeAnnotationTarget,
    /// (type_path_kind, type_argument_index) pairs.
    pub type_path: Vec<(u8, u16)>,
    pub annotation: Annotation,
}

fn parse_type_annotation_target(
    cursor: &mut AttributeCursor<'_>,
    target_type: u8,
) -> io::Result<TypeAnnotationTarget> {
    Ok(match target_type {
        0x00 | 0x01 => TypeAnnotationTarget::TypeParameter {
            declaration: target_type == 0x00,
            index: cursor.u1()?,
        },
        0x10 => TypeAnnotationTarget::Supertype {
            index: cursor.u2()?,
        },
        0x13..=0x15 => TypeAnnotationTarget::FormalParameter {
            index: cursor.u1()?,
        },
        0x16 => TypeAnnotationTarget::Throws {
            index: cursor.u2()?,
        },
        0x19 | 0x1a => {
            let table_length = usize::from(cursor.u2()?);
            let mut table = Vec::with_capacity(table_length);
            for _ in 0..table_length {
                table.push((cursor.u2()?, cursor.u2()?, cursor.u2()?));
            }
            TypeAnnotationTarget::LocalVar { table }
        }
        0x40 => TypeAnnotationTarget::ExceptionParameter {
            catch_type_index: cursor.u2()?,
        },
        0x41 => TypeAnnotationTarget::ExceptionParameter {
            catch_type_index: cursor.u2()?,
        },
        // 0x11, 0x12, 0x17, 0x18, 0x42-0x4B: empty targets.
        _ => TypeAnnotationTarget::Empty,
    })
}

/// Parses a Runtime[In]VisibleTypeAnnotations payload.
pub fn parse_type_annotations(
    bytes: &[u8],
    constants: &[Constant<'_>],
) -> io::Result<Vec<TypeAnnotation>> {
    let mut cursor = AttributeCursor::new(bytes);
    let count = usize::from(cursor.u2()?);
    let mut annotations = Vec::with_capacity(count);
    for _ in 0..count {
        let target_type = cursor.u1()?;
        let target_info = parse_type_annotation_target(&mut cursor, target_type)?;
        let path_length = usize::from(cursor.u2()?);
        let mut type_path = Vec::with_capacity(path_length);
        for _ in 0..path_length {
            type_path.push((cursor.u1()?, cursor.u2()?));
        }
        let annotation = parse_annotation(&mut cursor, constants)?;
        annotations.push(TypeAnnotation {
            target_type,
            target_info,
            type_path,
            annotation,
        });
    }
    if !cursor.done() {
        return Err(err("Trailing bytes in type annotations attribute"));
    }
    Ok(annotations)
}

impl<'a> ClassFile<'a> {
    /// RuntimeVisibleAnnotations of the class.
    pub fn visible_annotations(&self) -> io::Result<Vec<Annotation>> {
        match self.attribute_named(b"RuntimeVisibleAnnotations") {
            Some(attr) => parse_annotations(attr.bytes, &self.constants),
            None => Ok(Vec::new()),
        }
    }

    /// RuntimeInvisibleAnnotations of the class.
    pub fn invisible_annotations(&self) -> io::Result<Vec<Annotation>> {
        match self.attribute_named(b"RuntimeInvisibleAnnotations") {
            Some(attr) => parse_annotations(attr.bytes, &self.constants),
            None => Ok(Vec::new()),
        }
    }

    /// Runtime[In]VisibleTypeAnnotations of the class.
    pub fn visible_type_annotations(&self) -> io::Result<Vec<TypeAnnotation>> {
        match self.attribute_named(b"RuntimeVisibleTypeAnnotations") {
            Some(attr) => parse_type_annotations(attr.bytes, &self.constants),
            None => Ok(Vec::new()),
        }
    }

    pub fn invisible_type_annotations(&self) -> io::Result<Vec<TypeAnnotation>> {
        match self.attribute_named(b"RuntimeInvisibleTypeAnnotations") {
            Some(attr) => parse_type_annotations(attr.bytes, &self.constants),
            None => Ok(Vec::new()),
        }
    }

    /// Runtime[In]VisibleAnnotations of a field or method.
    pub fn member_annotations(
        &self,
        member: &Member<'a>,
        visible: bool,
    ) -> io::Result<Vec<Annotation>> {
        let name: &[u8] = if visible {
            b"RuntimeVisibleAnnotations"
        } else {
            b"RuntimeInvisibleAnnotations"
        };
        match member.attributes.iter().find(|attr| attr.name == name) {
            Some(attr) => parse_annotations(attr.bytes, &self.constants),
            None => Ok(Vec::new()),
        }
    }

    /// Runtime[In]VisibleParameterAnnotations of a method.
    pub fn member_parameter_annotations(
        &self,
        member: &Member<'a>,
        visible: bool,
    ) -> io::Result<Vec<Vec<Annotation>>> {
        let name: &[u8] = if visible {
            b"RuntimeVisibleParameterAnnotations"
        } else {
            b"RuntimeInvisibleParameterAnnotations"
        };
        match member.attributes.iter().find(|attr| attr.name == name) {
            Some(attr) => parse_parameter_annotations(attr.bytes, &self.constants),
            None => Ok(Vec::new()),
        }
    }

    /// Runtime[In]VisibleTypeAnnotations of a field, method or code block.
    pub fn member_type_annotations(
        &self,
        member: &Member<'a>,
        visible: bool,
    ) -> io::Result<Vec<TypeAnnotation>> {
        let name: &[u8] = if visible {
            b"RuntimeVisibleTypeAnnotations"
        } else {
            b"RuntimeInvisibleTypeAnnotations"
        };
        match member.attributes.iter().find(|attr| attr.name == name) {
            Some(attr) => parse_type_annotations(attr.bytes, &self.constants),
            None => Ok(Vec::new()),
        }
    }

    /// Type annotations inside a Code attribute payload.
    pub fn code_type_annotations(
        &self,
        code_attribute: &Attribute<'a>,
        visible: bool,
    ) -> io::Result<Vec<TypeAnnotation>> {
        let name: &[u8] = if visible {
            b"RuntimeVisibleTypeAnnotations"
        } else {
            b"RuntimeInvisibleTypeAnnotations"
        };
        // code_attribute.bytes: max_stack u2, max_locals u2, code_length u4,
        // code[code_length], exception_table, attributes_count, attributes.
        let mut cursor = AttributeCursor::new(code_attribute.bytes);
        cursor.skip(8)?;
        let code_length = cursor.u4()? as usize;
        cursor.skip(code_length)?;
        let exc_count = usize::from(cursor.u2()?);
        cursor.skip(exc_count * 8)?;
        let sub_count = usize::from(cursor.u2()?);
        for _ in 0..sub_count {
            let attr_name = utf8_at(&self.constants, cursor.u2()?)?;
            let length = cursor.u4()? as usize;
            let bytes = cursor.take(length)?;
            if attr_name == name {
                return parse_type_annotations(bytes, &self.constants);
            }
        }
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Module (JVMS §4.7.25) — module-info.class
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleRequires {
    pub requires_index: u16,
    pub requires_flags: u16,
    pub requires_version_index: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleExports {
    pub exports_index: u16,
    pub exports_flags: u16,
    pub exports_to: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleOpens {
    pub opens_index: u16,
    pub opens_flags: u16,
    pub opens_to: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleProvides {
    pub provides_index: u16,
    pub provides_with: Vec<u16>,
}

/// Structured Module attribute of module-info.class.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleInfo {
    pub module_name_index: u16,
    pub module_flags: u16,
    pub requires: Vec<ModuleRequires>,
    pub exports: Vec<ModuleExports>,
    pub opens: Vec<ModuleOpens>,
    pub uses: Vec<u16>,
    pub provides: Vec<ModuleProvides>,
}

pub fn parse_module_attribute(bytes: &[u8]) -> io::Result<ModuleInfo> {
    let mut cursor = AttributeCursor::new(bytes);
    let mut info = ModuleInfo {
        module_name_index: cursor.u2()?,
        module_flags: cursor.u2()?,
        ..ModuleInfo::default()
    };
    let requires_count = usize::from(cursor.u2()?);
    for _ in 0..requires_count {
        info.requires.push(ModuleRequires {
            requires_index: cursor.u2()?,
            requires_flags: cursor.u2()?,
            requires_version_index: cursor.u2()?,
        });
    }
    let exports_count = usize::from(cursor.u2()?);
    for _ in 0..exports_count {
        let exports_index = cursor.u2()?;
        let exports_flags = cursor.u2()?;
        let to_count = usize::from(cursor.u2()?);
        let mut exports_to = Vec::with_capacity(to_count);
        for _ in 0..to_count {
            exports_to.push(cursor.u2()?);
        }
        info.exports.push(ModuleExports {
            exports_index,
            exports_flags,
            exports_to,
        });
    }
    let opens_count = usize::from(cursor.u2()?);
    for _ in 0..opens_count {
        let opens_index = cursor.u2()?;
        let opens_flags = cursor.u2()?;
        let to_count = usize::from(cursor.u2()?);
        let mut opens_to = Vec::with_capacity(to_count);
        for _ in 0..to_count {
            opens_to.push(cursor.u2()?);
        }
        info.opens.push(ModuleOpens {
            opens_index,
            opens_flags,
            opens_to,
        });
    }
    let uses_count = usize::from(cursor.u2()?);
    for _ in 0..uses_count {
        info.uses.push(cursor.u2()?);
    }
    let provides_count = usize::from(cursor.u2()?);
    for _ in 0..provides_count {
        let provides_index = cursor.u2()?;
        let with_count = usize::from(cursor.u2()?);
        let mut provides_with = Vec::with_capacity(with_count);
        for _ in 0..with_count {
            provides_with.push(cursor.u2()?);
        }
        info.provides.push(ModuleProvides {
            provides_index,
            provides_with,
        });
    }
    if !cursor.done() {
        return Err(err("Trailing bytes in Module attribute"));
    }
    Ok(info)
}

/// Resolved module-info view: names instead of constant pool indices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedModuleInfo {
    pub module_name: Option<String>,
    pub module_flags: u16,
    pub requires: Vec<ResolvedModuleRequires>,
    pub exports: Vec<ResolvedModuleExports>,
    pub opens: Vec<ResolvedModuleOpens>,
    pub uses: Vec<String>,
    pub provides: Vec<ResolvedModuleProvides>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleRequires {
    pub name: String,
    pub flags: u16,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleExports {
    pub package: String,
    pub flags: u16,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleOpens {
    pub package: String,
    pub flags: u16,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModuleProvides {
    pub service: String,
    pub providers: Vec<String>,
}

impl<'a> ClassFile<'a> {
    /// Parses the Module attribute (present only on module-info.class).
    pub fn module_info(&self) -> io::Result<Option<ModuleInfo>> {
        match self.attribute_named(b"Module") {
            Some(attr) => parse_module_attribute(attr.bytes).map(Some),
            None => Ok(None),
        }
    }

    /// Module attribute with all constant pool indices resolved to strings.
    pub fn resolved_module_info(&self) -> io::Result<Option<ResolvedModuleInfo>> {
        let Some(info) = self.module_info()? else {
            return Ok(None);
        };
        let resolve_targets = |indices: &[u16]| -> io::Result<Vec<String>> {
            indices
                .iter()
                .map(|&i| class_name_string_at(&self.constants, i))
                .collect::<io::Result<Vec<_>>>()
                .map(|v| v.into_iter().flatten().collect())
        };
        let mut requires = Vec::with_capacity(info.requires.len());
        for req in &info.requires {
            requires.push(ResolvedModuleRequires {
                name: class_name_string_at(&self.constants, req.requires_index)?
                    .unwrap_or_default(),
                flags: req.requires_flags,
                version: if req.requires_version_index == 0 {
                    None
                } else {
                    Some(utf8_string_at(&self.constants, req.requires_version_index)?)
                },
            });
        }
        let mut exports = Vec::with_capacity(info.exports.len());
        for exp in &info.exports {
            exports.push(ResolvedModuleExports {
                package: utf8_string_at(&self.constants, exp.exports_index)?
                    .replace('/', "."),
                flags: exp.exports_flags,
                targets: resolve_targets(&exp.exports_to)?,
            });
        }
        let mut opens = Vec::with_capacity(info.opens.len());
        for open in &info.opens {
            opens.push(ResolvedModuleOpens {
                package: utf8_string_at(&self.constants, open.opens_index)?
                    .replace('/', "."),
                flags: open.opens_flags,
                targets: resolve_targets(&open.opens_to)?,
            });
        }
        let uses = info
            .uses
            .iter()
            .map(|&i| class_name_string_at(&self.constants, i))
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        let mut provides = Vec::with_capacity(info.provides.len());
        for prov in &info.provides {
            provides.push(ResolvedModuleProvides {
                service: class_name_string_at(&self.constants, prov.provides_index)?
                    .unwrap_or_default(),
                providers: resolve_targets(&prov.provides_with)?,
            });
        }
        Ok(Some(ResolvedModuleInfo {
            module_name: if info.module_name_index == 0 {
                None
            } else {
                class_name_string_at(&self.constants, info.module_name_index)?
            },
            module_flags: info.module_flags,
            requires,
            exports,
            opens,
            uses,
            provides,
        }))
    }
}
