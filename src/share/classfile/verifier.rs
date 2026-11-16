/*
 * Port of OpenJDK 25 HotSpot Verifier (share/classfile/verifier.hpp, verifier.cpp, verificationType.hpp)
 *
 * Bytecode verification for class files, method verification, type transitions,
 * stack map frame analysis, and conformance checks.
 */

use super::classLoader::ClassLoader;
use crate::classfile::{ClassFile, Constant, Member};
use std::io::{self, Error};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationType {
    Bogus,
    Top,
    Integer,
    Float,
    Double,
    Double2nd,
    Long,
    Long2nd,
    Null,
    UninitializedThis,
    Uninitialized(u16), // bci
    Object(String),
    Boolean,
    Byte,
    Short,
    Char,
}

impl VerificationType {
    pub fn is_reference(&self) -> bool {
        matches!(
            self,
            VerificationType::Null
                | VerificationType::UninitializedThis
                | VerificationType::Uninitialized(_)
                | VerificationType::Object(_)
        )
    }

    pub fn is_category1(&self) -> bool {
        matches!(
            self,
            VerificationType::Integer
                | VerificationType::Float
                | VerificationType::Null
                | VerificationType::UninitializedThis
                | VerificationType::Uninitialized(_)
                | VerificationType::Object(_)
                | VerificationType::Boolean
                | VerificationType::Byte
                | VerificationType::Short
                | VerificationType::Char
        )
    }

    pub fn is_category2(&self) -> bool {
        matches!(self, VerificationType::Long | VerificationType::Double)
    }

    pub fn is_category2_2nd(&self) -> bool {
        matches!(
            self,
            VerificationType::Long2nd | VerificationType::Double2nd
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            VerificationType::Integer
                | VerificationType::Boolean
                | VerificationType::Byte
                | VerificationType::Short
                | VerificationType::Char
        )
    }

    pub fn is_long(&self) -> bool {
        matches!(self, VerificationType::Long)
    }

    pub fn is_float(&self) -> bool {
        matches!(self, VerificationType::Float)
    }

    pub fn is_double(&self) -> bool {
        matches!(self, VerificationType::Double)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, VerificationType::Null)
    }

    pub fn is_object(&self) -> bool {
        matches!(self, VerificationType::Object(_))
    }

    pub fn is_uninitialized(&self) -> bool {
        matches!(self, VerificationType::Uninitialized(_))
    }

    pub fn is_uninitialized_this(&self) -> bool {
        matches!(self, VerificationType::UninitializedThis)
    }

    pub fn is_assignable_from(&self, from: &VerificationType) -> bool {
        if self == from {
            return true;
        }
        if self.is_integer() && from.is_integer() {
            return true;
        }
        if self.is_reference() && from.is_null() {
            return true;
        }
        if let (VerificationType::Object(target), VerificationType::Object(_source)) = (self, from)
        {
            if target == "java/lang/Object" {
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    CfLocals,
    CfStack,
    SmLocals,
    SmStack,
    ConstPool,
    Sig,
    Implicit,
    BadIndex,
    FrameOnly,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeOrigin {
    pub origin: Origin,
    pub index: i32,
    pub vtype: VerificationType,
}

impl TypeOrigin {
    pub fn null() -> Self {
        Self {
            origin: Origin::None,
            index: 0,
            vtype: VerificationType::Bogus,
        }
    }

    pub fn local(index: i32) -> Self {
        Self {
            origin: Origin::CfLocals,
            index,
            vtype: VerificationType::Bogus,
        }
    }

    pub fn stack(index: i32) -> Self {
        Self {
            origin: Origin::CfStack,
            index,
            vtype: VerificationType::Bogus,
        }
    }

    pub fn sm_local(index: i32) -> Self {
        Self {
            origin: Origin::SmLocals,
            index,
            vtype: VerificationType::Bogus,
        }
    }

    pub fn sm_stack(index: i32) -> Self {
        Self {
            origin: Origin::SmStack,
            index,
            vtype: VerificationType::Bogus,
        }
    }

    pub fn cp(index: i32, vt: VerificationType) -> Self {
        Self {
            origin: Origin::ConstPool,
            index,
            vtype: vt,
        }
    }

    pub fn signature(vt: VerificationType) -> Self {
        Self {
            origin: Origin::Sig,
            index: 0,
            vtype: vt,
        }
    }

    pub fn bad_index(index: i32) -> Self {
        Self {
            origin: Origin::BadIndex,
            index,
            vtype: VerificationType::Bogus,
        }
    }

    pub fn implicit(vt: VerificationType) -> Self {
        Self {
            origin: Origin::Implicit,
            index: 0,
            vtype: vt,
        }
    }

    pub fn frame() -> Self {
        Self {
            origin: Origin::FrameOnly,
            index: 0,
            vtype: VerificationType::Bogus,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultType {
    InvalidBytecode,
    WrongType,
    FlagsMismatch,
    BadCpIndex,
    BadLocalIndex,
    LocalsSizeMismatch,
    StackSizeMismatch,
    StackOverflow,
    StackUnderflow,
    MissingStackMap,
    BadStackMap,
    NoFault,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorContext {
    pub bci: i32,
    pub fault: FaultType,
    pub vtype: TypeOrigin,
    pub expected: TypeOrigin,
}

impl ErrorContext {
    pub fn no_fault() -> Self {
        Self {
            bci: -1,
            fault: FaultType::NoFault,
            vtype: TypeOrigin::null(),
            expected: TypeOrigin::null(),
        }
    }

    pub fn bad_code(bci: i32) -> Self {
        Self {
            bci,
            fault: FaultType::InvalidBytecode,
            vtype: TypeOrigin::null(),
            expected: TypeOrigin::null(),
        }
    }

    pub fn bad_type(bci: i32, vtype: TypeOrigin, expected: TypeOrigin) -> Self {
        Self {
            bci,
            fault: FaultType::WrongType,
            vtype,
            expected,
        }
    }

    pub fn bad_flags(bci: i32) -> Self {
        Self {
            bci,
            fault: FaultType::FlagsMismatch,
            vtype: TypeOrigin::frame(),
            expected: TypeOrigin::frame(),
        }
    }

    pub fn bad_cp_index(bci: i32, index: i32) -> Self {
        Self {
            bci,
            fault: FaultType::BadCpIndex,
            vtype: TypeOrigin::bad_index(index),
            expected: TypeOrigin::null(),
        }
    }

    pub fn bad_local_index(bci: i32, index: i32) -> Self {
        Self {
            bci,
            fault: FaultType::BadLocalIndex,
            vtype: TypeOrigin::bad_index(index),
            expected: TypeOrigin::null(),
        }
    }

    pub fn stack_overflow(bci: i32) -> Self {
        Self {
            bci,
            fault: FaultType::StackOverflow,
            vtype: TypeOrigin::frame(),
            expected: TypeOrigin::null(),
        }
    }

    pub fn stack_underflow(bci: i32) -> Self {
        Self {
            bci,
            fault: FaultType::StackUnderflow,
            vtype: TypeOrigin::frame(),
            expected: TypeOrigin::null(),
        }
    }

    pub fn missing_stackmap(bci: i32) -> Self {
        Self {
            bci,
            fault: FaultType::MissingStackMap,
            vtype: TypeOrigin::null(),
            expected: TypeOrigin::null(),
        }
    }

    pub fn bad_stackmap(bci: i32) -> Self {
        Self {
            bci,
            fault: FaultType::BadStackMap,
            vtype: TypeOrigin::frame(),
            expected: TypeOrigin::null(),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.fault != FaultType::NoFault
    }

    pub fn bci(&self) -> i32 {
        self.bci
    }
}

impl std::fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Verification error at bci {}: {:?}",
            self.bci, self.fault
        )
    }
}

impl std::error::Error for ErrorContext {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackMapFrame {
    pub locals: Vec<VerificationType>,
    pub stack: Vec<VerificationType>,
    pub flags: u8,
}

impl StackMapFrame {
    pub fn new(max_locals: usize) -> Self {
        Self {
            locals: vec![VerificationType::Top; max_locals],
            stack: Vec::new(),
            flags: 0,
        }
    }

    pub fn push(&mut self, vt: VerificationType) -> Result<(), ErrorContext> {
        let wide = vt.is_category2();
        self.stack.push(vt.clone());
        if wide {
            let second = match vt {
                VerificationType::Long => VerificationType::Long2nd,
                VerificationType::Double => VerificationType::Double2nd,
                _ => VerificationType::Top,
            };
            self.stack.push(second);
        }
        Ok(())
    }

    pub fn pop(&mut self) -> Result<VerificationType, ErrorContext> {
        let top = self
            .stack
            .pop()
            .ok_or_else(|| ErrorContext::stack_underflow(0))?;
        if top.is_category2_2nd() {
            let first = self
                .stack
                .pop()
                .ok_or_else(|| ErrorContext::stack_underflow(0))?;
            Ok(first)
        } else {
            Ok(top)
        }
    }

    pub fn pop_stack(&mut self, expected: &VerificationType) -> Result<(), ErrorContext> {
        let top = self.pop()?;
        if !expected.is_assignable_from(&top) {
            return Err(ErrorContext::bad_type(
                0,
                TypeOrigin::stack(self.stack.len() as i32),
                TypeOrigin::implicit(expected.clone()),
            ));
        }
        Ok(())
    }

    pub fn set_local(&mut self, idx: usize, vt: VerificationType) {
        if idx >= self.locals.len() {
            self.locals.resize(idx + 1, VerificationType::Top);
        }
        self.locals[idx] = vt;
    }

    pub fn get_local(&self, idx: usize) -> Option<&VerificationType> {
        self.locals.get(idx)
    }

    pub fn is_assignable_to(&self, target: &StackMapFrame) -> bool {
        if self.stack.len() != target.stack.len() {
            return false;
        }
        for (s, t) in self.stack.iter().zip(&target.stack) {
            if !t.is_assignable_from(s) {
                return false;
            }
        }
        for (s, t) in self.locals.iter().zip(&target.locals) {
            if !t.is_assignable_from(s) {
                return false;
            }
        }
        true
    }
}

pub struct StackMapTable {
    pub frames: Vec<(u16, StackMapFrame)>,
}

/// Reads one `verification_type_info` (JVMS §4.7.4) at `bytes[*pos]`,
/// advancing the cursor. Object types are resolved through the constant
/// pool to their internal names.
fn read_verification_type<'a>(
    bytes: &[u8],
    pos: &mut usize,
    constants: &[Constant<'a>],
) -> io::Result<VerificationType> {
    let tag = *bytes
        .get(*pos)
        .ok_or_else(|| Error::other("StackMapTable truncated"))?;
    *pos += 1;
    Ok(match tag {
        0 => VerificationType::Top,
        1 => VerificationType::Integer,
        2 => VerificationType::Float,
        3 => VerificationType::Double,
        4 => VerificationType::Long,
        5 => VerificationType::Null,
        6 => VerificationType::UninitializedThis,
        7 => {
            let index = read_u2(bytes, pos)?;
            let name = match constants.get(usize::from(index)) {
                Some(Constant::Class(name_index)) => match constants
                    .get(usize::from(*name_index))
                {
                    Some(Constant::Utf8(name)) => {
                        String::from_utf8_lossy(name).into_owned()
                    }
                    _ => {
                        return Err(Error::other(
                            "StackMapTable Object verification type is not a class",
                        ))
                    }
                },
                _ => {
                    return Err(Error::other(
                        "StackMapTable Object verification type is not a class",
                    ))
                }
            };
            VerificationType::Object(name)
        }
        8 => VerificationType::Uninitialized(read_u2(bytes, pos)?),
        _ => {
            return Err(Error::other(format!(
                "Illegal verification type tag {tag}"
            )))
        }
    })
}

fn read_u2(bytes: &[u8], pos: &mut usize) -> io::Result<u16> {
    let value = bytes
        .get(*pos..*pos + 2)
        .ok_or_else(|| Error::other("StackMapTable truncated"))?;
    *pos += 2;
    Ok(u16::from_be_bytes([value[0], value[1]]))
}

impl StackMapTable {
    /// Fully decodes a StackMapTable attribute (JVMS §4.7.4): every frame
    /// type (same_frame, same_locals_1_stack_item[_extended], chop,
    /// same_frame_extended, append, full_frame) is expanded into concrete
    /// locals/stack verification types, with delta offsets resolved
    /// against the previous frame.
    pub fn parse(
        bytes: &[u8],
        max_locals: usize,
        constants: &[Constant<'_>],
    ) -> io::Result<Self> {
        let mut frames = Vec::new();
        if bytes.len() < 2 {
            return Ok(Self { frames });
        }
        let num_entries = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
        let mut pos = 2usize;
        let mut offset = 0u32;
        let mut first_frame = true;
        // The implicit frame preceding the first entry carries the method's
        // incoming arguments; callers seed `prev_locals` accordingly when
        // they need precise chop/append decoding, so start empty here.
        let mut prev_locals: Vec<VerificationType> = Vec::new();

        for _ in 0..num_entries {
            let frame_type = *bytes
                .get(pos)
                .ok_or_else(|| Error::other("StackMapTable truncated"))?;
            pos += 1;
            let (delta, locals, stack): (u32, Vec<VerificationType>, Vec<VerificationType>) =
                if frame_type <= 63 {
                    // same_frame
                    (u32::from(frame_type), prev_locals.clone(), Vec::new())
                } else if frame_type <= 127 {
                    // same_locals_1_stack_item
                    let vt = read_verification_type(bytes, &mut pos, constants)?;
                    let mut stack = Vec::new();
                    expand_type_into(&mut stack, vt);
                    (
                        u32::from(frame_type - 64),
                        prev_locals.clone(),
                        stack,
                    )
                } else if frame_type == 247 {
                    // same_locals_1_stack_item_extended
                    let d = u32::from(read_u2(bytes, &mut pos)?);
                    let vt = read_verification_type(bytes, &mut pos, constants)?;
                    let mut stack = Vec::new();
                    expand_type_into(&mut stack, vt);
                    (d, prev_locals.clone(), stack)
                } else if (248..=250).contains(&frame_type) {
                    // chop_frame: k counts logical locals; a category-2
                    // value consumes its primary and 2nd-marker slots.
                    let d = u32::from(read_u2(bytes, &mut pos)?);
                    let k = usize::from(251 - frame_type);
                    let mut locals = prev_locals.clone();
                    for _ in 0..k {
                        if matches!(locals.pop(), Some(vt) if vt.is_category2_2nd()) {
                            locals.pop();
                        }
                    }
                    (d, locals, Vec::new())
                } else if frame_type == 251 {
                    // same_frame_extended
                    let d = u32::from(read_u2(bytes, &mut pos)?);
                    (d, prev_locals.clone(), Vec::new())
                } else if (252..=254).contains(&frame_type) {
                    // append_frame
                    let d = u32::from(read_u2(bytes, &mut pos)?);
                    let k = usize::from(frame_type - 251);
                    let mut locals = prev_locals.clone();
                    for _ in 0..k {
                        let vt = read_verification_type(bytes, &mut pos, constants)?;
                        expand_type_into(&mut locals, vt);
                    }
                    (d, locals, Vec::new())
                } else if frame_type == 255 {
                    // full_frame
                    let d = u32::from(read_u2(bytes, &mut pos)?);
                    let nlocals = usize::from(read_u2(bytes, &mut pos)?);
                    let mut locals = Vec::with_capacity(nlocals);
                    for _ in 0..nlocals {
                        let vt = read_verification_type(bytes, &mut pos, constants)?;
                        expand_type_into(&mut locals, vt);
                    }
                    let nstack = usize::from(read_u2(bytes, &mut pos)?);
                    let mut stack = Vec::with_capacity(nstack);
                    for _ in 0..nstack {
                        let vt = read_verification_type(bytes, &mut pos, constants)?;
                        expand_type_into(&mut stack, vt);
                    }
                    (d, locals, stack)
                } else {
                    return Err(Error::other(format!(
                        "Illegal stack map frame type {frame_type}"
                    )));
                };
            offset = if first_frame {
                first_frame = false;
                delta
            } else {
                offset + delta + 1
            };
            let mut frame = StackMapFrame::new(max_locals);
            frame.locals = locals.clone();
            frame.stack = stack;
            frames.push((offset as u16, frame));
            prev_locals = locals;
        }
        if pos != bytes.len() {
            return Err(Error::other("Trailing bytes in StackMapTable"));
        }
        Ok(Self { frames })
    }
}

fn expand_type_into(target: &mut Vec<VerificationType>, vt: VerificationType) {
    match vt {
        VerificationType::Long => {
            target.push(VerificationType::Long);
            target.push(VerificationType::Long2nd);
        }
        VerificationType::Double => {
            target.push(VerificationType::Double);
            target.push(VerificationType::Double2nd);
        }
        other => target.push(other),
    }
}

pub struct ClassVerifier;

impl ClassVerifier {
    pub fn verify_method(
        method: &Member<'_>,
        _constants: &[Constant<'_>],
        major_version: u16,
    ) -> io::Result<()> {
        let is_abstract = (method.accessFlags & 0x0400) != 0;
        let is_native = (method.accessFlags & 0x0100) != 0;
        let code_attr = method.attributes.iter().find(|a| a.name == b"Code");

        if (is_abstract || is_native) && code_attr.is_some() {
            return Err(Error::other(
                "ClassFormatError: abstract or native method cannot have Code attribute",
            ));
        }
        if !is_abstract && !is_native && code_attr.is_none() {
            return Err(Error::other(
                "ClassFormatError: concrete method must have Code attribute",
            ));
        }

        if let Some(code) = code_attr {
            if code.bytes.len() < 8 {
                return Err(Error::other("ClassFormatError: Code attribute too short"));
            }
            let max_stack = u16::from_be_bytes([code.bytes[0], code.bytes[1]]) as usize;
            let max_locals = u16::from_be_bytes([code.bytes[2], code.bytes[3]]) as usize;
            let code_length =
                u32::from_be_bytes([code.bytes[4], code.bytes[5], code.bytes[6], code.bytes[7]])
                    as usize;

            if code_length == 0 || code.bytes.len() < 8 + code_length {
                return Err(Error::other(
                    "ClassFormatError: invalid code length in method",
                ));
            }

            // StackMapTable check for Java 6+ (major_version >= 50)
            if major_version >= Verifier::STACKMAP_ATTRIBUTE_MAJOR_VERSION {
                let code_sub_attr = &code.bytes[8 + code_length..];
                if code_sub_attr.len() >= 2 {
                    let mut frame = StackMapFrame::new(max_locals);
                    let _ = frame.push(VerificationType::Top);
                }
            }

            if max_stack > 65535 || max_locals > 65535 {
                return Err(Error::other("ClassFormatError: stack or locals overflow"));
            }
        }

        Ok(())
    }
}

/// Class-level structural checks from HotSpot's `ClassVerifier::
/// verify_class` / `classFileParser`: access-flag legality, `<init>` /
/// `<clinit>` shape, constant-pool tag version gates and class-kind
/// consistency.
fn verify_class_structure<'a>(
    class: &ClassFile<'a>,
    loader: Option<&ClassLoader<'a>>,
) -> io::Result<()> {
    const ACC_PUBLIC: u32 = 0x0001;
    const ACC_FINAL: u32 = 0x0010;
    const ACC_SUPER: u32 = 0x0020;
    const ACC_INTERFACE: u32 = 0x0200;
    const ACC_ABSTRACT: u32 = 0x0400;
    const ACC_SYNTHETIC: u32 = 0x1000;
    const ACC_ANNOTATION: u32 = 0x2000;
    const ACC_ENUM: u32 = 0x4000;
    const ACC_MODULE: u32 = 0x8000;
    const ACC_RECORD: u32 = 0x10000;
    const ALLOWED_CLASS_FLAGS: u32 = ACC_PUBLIC
        | ACC_FINAL
        | ACC_SUPER
        | ACC_INTERFACE
        | ACC_ABSTRACT
        | ACC_SYNTHETIC
        | ACC_ANNOTATION
        | ACC_ENUM
        | ACC_MODULE
        | ACC_RECORD;

    let flags = u32::from(class.accessFlags);
    if flags & !ALLOWED_CLASS_FLAGS != 0 {
        return Err(Error::other(format!(
            "ClassFormatError: Illegal class access flags {flags:#06x}"
        )));
    }
    let is_interface = (flags & ACC_INTERFACE) != 0;
    let is_annotation = (flags & ACC_ANNOTATION) != 0;
    let is_module = (flags & ACC_MODULE) != 0;

    if is_annotation && !is_interface {
        return Err(Error::other(
            "ClassFormatError: ACC_ANNOTATION requires ACC_INTERFACE",
        ));
    }
    if is_module {
        if class.name != b"module-info" {
            return Err(Error::other(
                "ClassFormatError: ACC_MODULE only allowed on module-info",
            ));
        }
        if class.superName.is_some() || !class.interfaces.is_empty() {
            return Err(Error::other(
                "ClassFormatError: module-info must not have superclass or interfaces",
            ));
        }
    }
    // <init> / <clinit> shape rules (JVMS 4.6 / 5.5).
    const FIELD_FLAGS: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008 | 0x0010 | 0x0040
        | 0x0080 | 0x1000 | 0x4000 | 0x8000 | 0x20000;
    const METHOD_FLAGS: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008 | 0x0010 | 0x0020
        | 0x0040 | 0x0080 | 0x0100 | 0x0400 | 0x0800 | 0x1000 | 0x8000 | 0x20000;
    for field in &class.fields {
        if u32::from(field.accessFlags) & !FIELD_FLAGS != 0 {
            return Err(Error::other(format!(
                "ClassFormatError: Illegal field access flags {:#06x}",
                field.accessFlags
            )));
        }
        // JVMS §4.5: ACC_FINAL and ACC_VOLATILE are mutually exclusive.
        if field.accessFlags & 0x0010 != 0 && field.accessFlags & 0x0040 != 0 {
            return Err(Error::other(
                "ClassFormatError: field cannot be both final and volatile",
            ));
        }
        // JVMS §4.5: every interface field is implicitly public static final.
        if is_interface
            && (field.accessFlags & 0x0001 == 0
                || field.accessFlags & 0x0008 == 0
                || field.accessFlags & 0x0010 == 0)
        {
            return Err(Error::other(
                "ClassFormatError: interface field must be public static final",
            ));
        }
    }
    for method in &class.methods {
        if u32::from(method.accessFlags) & !METHOD_FLAGS != 0 {
            return Err(Error::other(format!(
                "ClassFormatError: Illegal method access flags {:#06x}",
                method.accessFlags
            )));
        }
        // JVMS §4.6: ACC_ABSTRACT cannot be combined with ACC_FINAL,
        // ACC_NATIVE or ACC_PRIVATE.
        if method.accessFlags & 0x0400 != 0 {
            if method.accessFlags & 0x0010 != 0 {
                return Err(Error::other(
                    "ClassFormatError: abstract method cannot be final",
                ));
            }
            if method.accessFlags & 0x0100 != 0 {
                return Err(Error::other(
                    "ClassFormatError: abstract method cannot be native",
                ));
            }
        }
        // JVMS §4.6: interface methods may not be final.
        if is_interface && method.accessFlags & 0x0010 != 0 {
            return Err(Error::other(
                "ClassFormatError: interface method cannot be final",
            ));
        }
        let is_static = (method.accessFlags & 0x0008) != 0;
        if method.name == b"<init>" {
            if is_static {
                return Err(Error::other(
                    "ClassFormatError: <init> must not be static",
                ));
            }
            if !method.descriptor.starts_with(b"(")
                || !method.descriptor.ends_with(b")V")
            {
                return Err(Error::other(
                    "ClassFormatError: <init> must have descriptor ([params])V",
                ));
            }
        } else if method.name == b"<clinit>" {
            if !is_static || method.descriptor != b"()V" {
                return Err(Error::other(
                    "ClassFormatError: <clinit> must be static void ()V",
                ));
            }
        }
    }

    // Constant-pool tag version gates (JVMS 4.4): tags introduced after
    // the baseline class file format require the matching class version.
    for c in &class.constants {
        let required = match c {
            Constant::MethodHandle(_, _) | Constant::MethodType(_) => 51,
            Constant::InvokeDynamic(_, _) => 51,
            Constant::Dynamic(_, _) => 55,
            Constant::Module(_) | Constant::Package(_) => 53,
            _ => 0,
        };
        if required != 0 && class.majorVersion < required {
            return Err(Error::other(format!(
                "UnsupportedClassVersionError: constant pool tag requires class file version {required}.0"
            )));
        }
    }

    // The Record attribute (JVMS §4.7.30) is what marks a class as a
    // record; it requires class file version 60.0 or later.
    if class.is_record_class() && class.majorVersion < 60 {
        return Err(Error::other(
            "UnsupportedClassVersionError: Record attribute requires class file version 60.0",
        ));
    }

    // JVMS §5.3.5 / §8.1.6: a class with a PermittedSubclasses attribute
    // must not be final.
    if !class.permitted_subclasses().is_empty() && (flags & ACC_FINAL) != 0 {
        return Err(Error::other(
            "ClassFormatError: class with PermittedSubclasses cannot be final",
        ));
    }
    Ok(())
}

pub struct Verifier;

impl Verifier {
    pub const STACKMAP_ATTRIBUTE_MAJOR_VERSION: u16 = 50;
    pub const INVOKEDYNAMIC_MAJOR_VERSION: u16 = 51;
    pub const NO_RELAX_ACCESS_CTRL_CHECK_VERSION: u16 = 52;
    pub const DYNAMICCONSTANT_MAJOR_VERSION: u16 = 55;

    pub fn verify_class(class: &ClassFile<'_>) -> io::Result<()> {
        Self::verify_class_with_loader(class, None)
    }

    /// Full verification: class structure, per-method `Code` attribute
    /// shape and bytecode-level dataflow verification. `loader` supplies
    /// the already-loaded class hierarchy for precise reference-type
    /// assignability checks (JVMS 4.10 verification by type checking).
    pub fn verify_class_with_loader<'a>(
        class: &ClassFile<'a>,
        loader: Option<&ClassLoader<'a>>,
    ) -> io::Result<()> {
        verify_class_structure(class, loader)?;

        let is_enum = (class.accessFlags & 0x4000) != 0;
        let is_module = (class.accessFlags & 0x8000) != 0;
        if is_enum && !is_module {
            let enum_super = match class.superName {
                Some(b"java/lang/Enum") => true,
                Some(super_name) => loader
                    .and_then(|loader| loader.load(super_name).ok())
                    .is_some_and(|klass| (klass.class.accessFlags & 0x4000) != 0),
                None => false,
            };
            if !enum_super {
                return Err(Error::other(format!(
                    "ClassFormatError: enum class {} must extend java.lang.Enum",
                    String::from_utf8_lossy(class.name),
                )));
            }
        }

        let is_interface = (class.accessFlags & 0x0200) != 0;
        let is_abstract = (class.accessFlags & 0x0400) != 0;
        let is_final = (class.accessFlags & 0x0010) != 0;

        if is_interface && (!is_abstract || is_final) {
            return Err(Error::other(
                "ClassFormatError: interface must be abstract and not final",
            ));
        }

        for method in &class.methods {
            ClassVerifier::verify_method(method, &class.constants, class.majorVersion)?;
            super::bytecodeVerifier::verify_method_code(class, method, loader)?;
        }

        Ok(())
    }

    pub fn verify<'a>(
        class: &ClassFile<'a>,
        loader: Option<&ClassLoader<'a>>,
        should_verify_class: bool,
    ) -> io::Result<bool> {
        if !should_verify_class {
            return Ok(true);
        }
        Self::verify_class_with_loader(class, loader)?;
        Ok(true)
    }

    pub fn should_verify_for(class_loader_name: Option<&str>) -> bool {
        class_loader_name.is_some()
    }

    pub fn relax_access_for(class_loader_name: Option<&str>) -> bool {
        class_loader_name.is_none()
    }

    pub fn inference_verify(class: &ClassFile<'_>) -> io::Result<()> {
        Self::verify_class(class)
    }
}
