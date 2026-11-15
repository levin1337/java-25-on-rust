/*
 * Port of OpenJDK 25 HotSpot ConstMethod (share/oops/constMethod.hpp)
 *
 * ConstMethod holds read-only parts of a Java method:
 * - Bytecode instructions
 * - Exception table
 * - Line number table
 * - Local variable table
 * - Parameter and signature indices
 */

use super::method::{Code, ExceptionTableEntry};
use crate::classfile::Member;
use std::io::{self, Error};

/// Length in bytes of the field descriptor at the start of `bytes`
/// (`I` -> 1, `Ljava/lang/String;` -> 18, `[I` -> 2).  Returns `None`
/// for `V`, empty input or malformed descriptors.
pub fn field_descriptor_len(bytes: &[u8]) -> Option<usize> {
    let dims = bytes.iter().take_while(|b| **b == b'[').count();
    let tag = *bytes.get(dims)?;
    match tag {
        b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' => Some(dims + 1),
        b'L' => {
            let rest = &bytes[dims + 1..];
            let end = rest.iter().position(|b| *b == b';')?;
            if end == 0 {
                return None;
            }
            Some(dims + 1 + end + 1)
        }
        _ => None,
    }
}

/// Slot size of a field descriptor: 2 for `J`/`D`, 1 otherwise
/// (references and arrays occupy one compressed-oop slot).
pub fn field_descriptor_slots(descriptor: &[u8]) -> io::Result<usize> {
    if field_descriptor_len(descriptor) != Some(descriptor.len()) {
        return Err(Error::other("Invalid field descriptor"));
    }
    Ok(match descriptor.iter().position(|b| *b != b'[').map(|i| descriptor[i]) {
        Some(b'J') | Some(b'D') => 2,
        _ => 1,
    })
}

/// Splits a JVM method descriptor into its parameter descriptors and
/// the return descriptor (`None` for `V`).
pub fn split_method_descriptor(descriptor: &[u8]) -> io::Result<(Vec<&[u8]>, Option<&[u8]>)> {
    let invalid = || Error::other("Invalid method descriptor");
    let params = descriptor.strip_prefix(b"(").ok_or_else(invalid)?;
    let mut pos = 0usize;
    let mut arguments = Vec::new();
    while pos < params.len() && params[pos] != b')' {
        let len = field_descriptor_len(&params[pos..]).ok_or_else(invalid)?;
        arguments.push(&params[pos..pos + len]);
        pos += len;
    }
    if pos >= params.len() || params[pos] != b')' {
        return Err(invalid());
    }
    let result = &params[pos + 1..];
    let returns = if result == b"V" {
        None
    } else {
        if field_descriptor_len(result) != Some(result.len()) {
            return Err(invalid());
        }
        Some(result)
    };
    Ok((arguments, returns))
}

/// `size_of_parameters` for a method descriptor: total local-variable
/// slots occupied by the arguments (long/double count as 2).
pub fn descriptor_parameter_slots(descriptor: &[u8]) -> io::Result<u16> {
    let (arguments, _) = split_method_descriptor(descriptor)?;
    let mut slots = 0usize;
    for arg in arguments {
        slots += field_descriptor_slots(arg)?;
    }
    u16::try_from(slots).map_err(|_| Error::other("Too many method parameters"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalVariableTableElement {
    pub start_pc: u16,
    pub length: u16,
    pub name_cp_index: u16,
    pub descriptor_cp_index: u16,
    pub slot: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineNumberTableElement {
    pub start_pc: u16,
    pub line_number: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstMethod {
    name_index: u16,
    signature_index: u16,
    flags: u16,
    max_stack: u16,
    max_locals: u16,
    size_of_parameters: u16,
    bytecodes: Vec<u8>,
    exception_table: Vec<ExceptionTableEntry>,
    line_number_table: Vec<LineNumberTableElement>,
    local_variable_table: Vec<LocalVariableTableElement>,
}

impl ConstMethod {
    pub const HAS_LINENUMBER_TABLE: u16 = 0x0001;
    pub const HAS_LOCALVARIABLE_TABLE: u16 = 0x0002;
    pub const HAS_EXCEPTION_TABLE: u16 = 0x0004;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name_index: u16,
        signature_index: u16,
        max_stack: u16,
        max_locals: u16,
        size_of_parameters: u16,
        bytecodes: Vec<u8>,
        exception_table: Vec<ExceptionTableEntry>,
        line_number_table: Vec<LineNumberTableElement>,
        local_variable_table: Vec<LocalVariableTableElement>,
    ) -> Self {
        let mut flags = 0u16;
        if !line_number_table.is_empty() {
            flags |= Self::HAS_LINENUMBER_TABLE;
        }
        if !local_variable_table.is_empty() {
            flags |= Self::HAS_LOCALVARIABLE_TABLE;
        }
        if !exception_table.is_empty() {
            flags |= Self::HAS_EXCEPTION_TABLE;
        }

        Self {
            name_index,
            signature_index,
            flags,
            max_stack,
            max_locals,
            size_of_parameters,
            bytecodes,
            exception_table,
            line_number_table,
            local_variable_table,
        }
    }

    /// Builds a `ConstMethod` directly from a parsed class-file `Member`:
    /// bytecodes, max_stack/max_locals, exception table and
    /// `size_of_parameters` are all recovered from the Code attribute and
    /// the method descriptor.  Name/signature CP indices are left as 0 —
    /// callers that need them must resolve them against the owning
    /// constant pool separately.
    pub fn from_member(member: &Member<'_>) -> io::Result<Self> {
        let code = Code::parse(member)?;
        let size_of_parameters = descriptor_parameter_slots(member.descriptor)?;
        Ok(Self::new(
            0,
            0,
            code.maxStack.min(u16::MAX as usize) as u16,
            code.maxLocals.min(u16::MAX as usize) as u16,
            size_of_parameters,
            code.bytes.to_vec(),
            code.exception_table,
            Vec::new(),
            Vec::new(),
        ))
    }

    #[inline]
    pub fn name_index(&self) -> u16 {
        self.name_index
    }

    #[inline]
    pub fn signature_index(&self) -> u16 {
        self.signature_index
    }

    #[inline]
    pub fn flags(&self) -> u16 {
        self.flags
    }

    #[inline]
    pub fn max_stack(&self) -> u16 {
        self.max_stack
    }

    #[inline]
    pub fn max_locals(&self) -> u16 {
        self.max_locals
    }

    #[inline]
    pub fn size_of_parameters(&self) -> u16 {
        self.size_of_parameters
    }

    #[inline]
    pub fn code_size(&self) -> usize {
        self.bytecodes.len()
    }

    #[inline]
    pub fn bytecodes(&self) -> &[u8] {
        &self.bytecodes
    }

    #[inline]
    pub fn bytecode_at(&self, bci: usize) -> Option<u8> {
        self.bytecodes.get(bci).copied()
    }

    #[inline]
    pub fn exception_table(&self) -> &[ExceptionTableEntry] {
        &self.exception_table
    }

    #[inline]
    pub fn line_number_table(&self) -> &[LineNumberTableElement] {
        &self.line_number_table
    }

    #[inline]
    pub fn local_variable_table(&self) -> &[LocalVariableTableElement] {
        &self.local_variable_table
    }

    #[inline]
    pub fn has_linenumber_table(&self) -> bool {
        (self.flags & Self::HAS_LINENUMBER_TABLE) != 0
    }

    #[inline]
    pub fn has_localvariable_table(&self) -> bool {
        (self.flags & Self::HAS_LOCALVARIABLE_TABLE) != 0
    }

    #[inline]
    pub fn has_exception_table(&self) -> bool {
        (self.flags & Self::HAS_EXCEPTION_TABLE) != 0
    }

    /// Resolves line number from a bytecode index (bci), matching HotSpot ConstMethod::line_number_from_bci.
    pub fn line_number_from_bci(&self, bci: usize) -> Option<u16> {
        if self.line_number_table.is_empty() {
            return None;
        }

        let bci_u16 = bci as u16;
        let mut matched_line = None;
        for entry in &self.line_number_table {
            if entry.start_pc <= bci_u16 {
                matched_line = Some(entry.line_number);
            } else {
                break;
            }
        }
        matched_line.or_else(|| self.line_number_table.first().map(|e| e.line_number))
    }
}
