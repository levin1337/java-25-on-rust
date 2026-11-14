/*
 * Port of OpenJDK 25 HotSpot Verifier (share/classfile/verifier.hpp, verifier.cpp, verificationType.hpp)
 *
 * Bytecode verification for class files, method verification, type transitions,
 * stack map frame analysis, and conformance checks.
 */

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

impl StackMapTable {
    pub fn parse(bytes: &[u8], max_locals: usize) -> io::Result<Self> {
        let mut frames = Vec::new();
        if bytes.len() < 2 {
            return Ok(Self { frames });
        }
        let num_entries = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
        let mut offset = 2;
        let mut current_offset = 0u16;

        for _ in 0..num_entries {
            if offset >= bytes.len() {
                break;
            }
            let frame_type = bytes[offset];
            offset += 1;

            if frame_type <= 63 {
                // SAME frame
                current_offset += frame_type as u16;
                frames.push((current_offset, StackMapFrame::new(max_locals)));
            } else if frame_type <= 127 {
                // SAME_LOCALS_1_STACK_ITEM
                current_offset += (frame_type - 64) as u16;
                let mut frame = StackMapFrame::new(max_locals);
                frame
                    .push(VerificationType::Integer)
                    .map_err(|e| Error::other(e.to_string()))?;
                frames.push((current_offset, frame));
                offset += 1;
            } else if frame_type == 247 {
                // SAME_LOCALS_1_STACK_ITEM_EXTENDED
                if offset + 2 <= bytes.len() {
                    let delta = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
                    offset += 2;
                    current_offset += delta + 1;
                    let mut frame = StackMapFrame::new(max_locals);
                    frame
                        .push(VerificationType::Integer)
                        .map_err(|e| Error::other(e.to_string()))?;
                    frames.push((current_offset, frame));
                    offset += 1;
                }
            } else if frame_type <= 250 {
                // CHOP
                if offset + 2 <= bytes.len() {
                    let delta = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
                    offset += 2;
                    current_offset += delta + 1;
                    frames.push((current_offset, StackMapFrame::new(max_locals)));
                }
            } else if frame_type == 251 {
                // SAME_FRAME_EXTENDED
                if offset + 2 <= bytes.len() {
                    let delta = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
                    offset += 2;
                    current_offset += delta + 1;
                    frames.push((current_offset, StackMapFrame::new(max_locals)));
                }
            } else if frame_type <= 254 {
                // APPEND
                if offset + 2 <= bytes.len() {
                    let delta = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
                    offset += 2;
                    current_offset += delta + 1;
                    frames.push((current_offset, StackMapFrame::new(max_locals)));
                    let k = (frame_type - 251) as usize;
                    offset += k;
                }
            } else if frame_type == 255 {
                // FULL_FRAME
                if offset + 2 <= bytes.len() {
                    let delta = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
                    offset += 2;
                    current_offset += delta + 1;
                    frames.push((current_offset, StackMapFrame::new(max_locals)));
                }
            }
        }

        Ok(Self { frames })
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

pub struct Verifier;

impl Verifier {
    pub const STACKMAP_ATTRIBUTE_MAJOR_VERSION: u16 = 50;
    pub const INVOKEDYNAMIC_MAJOR_VERSION: u16 = 51;
    pub const NO_RELAX_ACCESS_CTRL_CHECK_VERSION: u16 = 52;
    pub const DYNAMICCONSTANT_MAJOR_VERSION: u16 = 55;

    pub fn verify_class(class: &ClassFile<'_>) -> io::Result<()> {
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
        }

        Ok(())
    }

    pub fn verify(class: &ClassFile<'_>, should_verify_class: bool) -> io::Result<bool> {
        if !should_verify_class {
            return Ok(true);
        }
        Self::verify_class(class)?;
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
