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

use super::method::ExceptionTableEntry;

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
