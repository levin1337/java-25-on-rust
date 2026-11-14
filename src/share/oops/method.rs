/*
 * Port of OpenJDK 25 HotSpot Method (share/oops/method.hpp, method.cpp)
 *
 * Method represents the executable metadata of a Java method:
 * - Read-only bytecode and structural metadata stored in ConstMethod
 * - Runtime execution counters and compilation triggers stored in MethodCounters
 * - Dynamic dispatch index in the virtual method table (vtable)
 * - Access flags and intrinsic identifiers
 */

use super::constMethod::ConstMethod;
use super::methodCounters::MethodCounters;
use crate::classfile::Member;
use std::io::{self, Error};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExceptionTableEntry {
    pub start_pc: usize,
    pub end_pc: usize,
    pub handler_pc: usize,
    pub catch_type: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code<'a> {
    pub maxStack: usize,
    pub maxLocals: usize,
    pub bytes: &'a [u8],
    pub exception_table: Vec<ExceptionTableEntry>,
}

impl<'a> Code<'a> {
    pub fn parse(method: &Member<'a>) -> io::Result<Self> {
        let attribute = method
            .attributes
            .iter()
            .find(|attribute| attribute.name == b"Code")
            .ok_or_else(|| {
                Error::other(format!(
                    "{}{}: {}",
                    String::from_utf8_lossy(method.name),
                    String::from_utf8_lossy(method.descriptor),
                    if method.accessFlags & 0x0100 != 0 {
                        "native method is not linked"
                    } else {
                        "method has no Code attribute"
                    }
                ))
            })?;
        let invalid = || Error::other("Invalid Code attribute");
        let (header, rest) = attribute
            .bytes
            .split_first_chunk::<8>()
            .ok_or_else(invalid)?;
        let [stackHigh, stackLow, localsHigh, localsLow, a, b, c, d] = *header;
        let length = u32::from_be_bytes([a, b, c, d]) as usize;
        let bytes = rest
            .get(..length)
            .filter(|_| (1..=65535).contains(&length))
            .ok_or_else(invalid)?;
        let tail = rest.get(length..).ok_or_else(invalid)?;
        if tail.len() < 2 {
            return Err(invalid());
        }
        let exception_table_length = u16::from_be_bytes([tail[0], tail[1]]) as usize;
        let mut exception_table = Vec::with_capacity(exception_table_length);
        let mut pos = 2;
        for _ in 0..exception_table_length {
            let entry_bytes = tail.get(pos..pos + 8).ok_or_else(invalid)?;
            let start_pc = usize::from(u16::from_be_bytes([entry_bytes[0], entry_bytes[1]]));
            let end_pc = usize::from(u16::from_be_bytes([entry_bytes[2], entry_bytes[3]]));
            let handler_pc = usize::from(u16::from_be_bytes([entry_bytes[4], entry_bytes[5]]));
            let catch_type = u16::from_be_bytes([entry_bytes[6], entry_bytes[7]]);
            exception_table.push(ExceptionTableEntry {
                start_pc,
                end_pc,
                handler_pc,
                catch_type,
            });
            pos += 8;
        }

        Ok(Code {
            maxStack: usize::from(u16::from_be_bytes([stackHigh, stackLow])),
            maxLocals: usize::from(u16::from_be_bytes([localsHigh, localsLow])),
            bytes,
            exception_table,
        })
    }
}

/// Represents an executable Java method in HotSpot metadata.
/// Separates mutable execution state and profiling from immutable ConstMethod bytecode.
#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    const_method: Rc<ConstMethod>,
    method_counters: Option<MethodCounters>,
    vtable_index: i32,
    access_flags: u16,
    method_flags: u32,
    intrinsic_id: u16,
}

impl Method {
    pub const NONVIRTUAL_VTABLE_INDEX: i32 = -1;

    // JVM Access Flags for methods
    pub const ACC_PUBLIC: u16 = 0x0001;
    pub const ACC_PRIVATE: u16 = 0x0002;
    pub const ACC_PROTECTED: u16 = 0x0004;
    pub const ACC_STATIC: u16 = 0x0008;
    pub const ACC_FINAL: u16 = 0x0010;
    pub const ACC_SYNCHRONIZED: u16 = 0x0020;
    pub const ACC_BRIDGE: u16 = 0x0040;
    pub const ACC_VARARGS: u16 = 0x0080;
    pub const ACC_NATIVE: u16 = 0x0100;
    pub const ACC_ABSTRACT: u16 = 0x0400;
    pub const ACC_STRICT: u16 = 0x0800;
    pub const ACC_SYNTHETIC: u16 = 0x1000;

    pub fn new(const_method: Rc<ConstMethod>, access_flags: u16) -> Self {
        Self {
            const_method,
            method_counters: None,
            vtable_index: Self::NONVIRTUAL_VTABLE_INDEX,
            access_flags,
            method_flags: 0,
            intrinsic_id: 0,
        }
    }

    pub fn with_vtable_index(
        const_method: Rc<ConstMethod>,
        access_flags: u16,
        vtable_index: i32,
    ) -> Self {
        Self {
            const_method,
            method_counters: None,
            vtable_index,
            access_flags,
            method_flags: 0,
            intrinsic_id: 0,
        }
    }

    #[inline]
    pub fn const_method(&self) -> &ConstMethod {
        &self.const_method
    }

    #[inline]
    pub fn const_method_rc(&self) -> &Rc<ConstMethod> {
        &self.const_method
    }

    #[inline]
    pub fn access_flags(&self) -> u16 {
        self.access_flags
    }

    #[inline]
    pub fn is_public(&self) -> bool {
        (self.access_flags & Self::ACC_PUBLIC) != 0
    }

    #[inline]
    pub fn is_private(&self) -> bool {
        (self.access_flags & Self::ACC_PRIVATE) != 0
    }

    #[inline]
    pub fn is_protected(&self) -> bool {
        (self.access_flags & Self::ACC_PROTECTED) != 0
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        (self.access_flags & Self::ACC_STATIC) != 0
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        (self.access_flags & Self::ACC_FINAL) != 0
    }

    #[inline]
    pub fn is_synchronized(&self) -> bool {
        (self.access_flags & Self::ACC_SYNCHRONIZED) != 0
    }

    #[inline]
    pub fn is_native(&self) -> bool {
        (self.access_flags & Self::ACC_NATIVE) != 0
    }

    #[inline]
    pub fn is_abstract(&self) -> bool {
        (self.access_flags & Self::ACC_ABSTRACT) != 0
    }

    #[inline]
    pub fn is_synthetic(&self) -> bool {
        (self.access_flags & Self::ACC_SYNTHETIC) != 0
    }

    #[inline]
    pub fn vtable_index(&self) -> i32 {
        self.vtable_index
    }

    #[inline]
    pub fn set_vtable_index(&mut self, index: i32) {
        self.vtable_index = index;
    }

    #[inline]
    pub fn has_vtable_index(&self) -> bool {
        self.vtable_index >= 0
    }

    #[inline]
    pub fn intrinsic_id(&self) -> u16 {
        self.intrinsic_id
    }

    #[inline]
    pub fn set_intrinsic_id(&mut self, id: u16) {
        self.intrinsic_id = id;
    }

    // Delegations to ConstMethod
    #[inline]
    pub fn name_index(&self) -> u16 {
        self.const_method.name_index()
    }

    #[inline]
    pub fn signature_index(&self) -> u16 {
        self.const_method.signature_index()
    }

    #[inline]
    pub fn max_stack(&self) -> u16 {
        self.const_method.max_stack()
    }

    #[inline]
    pub fn max_locals(&self) -> u16 {
        self.const_method.max_locals()
    }

    #[inline]
    pub fn size_of_parameters(&self) -> u16 {
        self.const_method.size_of_parameters()
    }

    #[inline]
    pub fn code_size(&self) -> usize {
        self.const_method.code_size()
    }

    #[inline]
    pub fn bytecodes(&self) -> &[u8] {
        self.const_method.bytecodes()
    }

    #[inline]
    pub fn bytecode_at(&self, bci: usize) -> Option<u8> {
        self.const_method.bytecode_at(bci)
    }

    #[inline]
    pub fn line_number_from_bci(&self, bci: usize) -> Option<u16> {
        self.const_method.line_number_from_bci(bci)
    }

    // Profiling and counters
    #[inline]
    pub fn method_counters(&self) -> Option<&MethodCounters> {
        self.method_counters.as_ref()
    }

    #[inline]
    pub fn method_counters_mut(&mut self) -> Option<&mut MethodCounters> {
        self.method_counters.as_mut()
    }

    pub fn get_or_create_method_counters(&mut self) -> &mut MethodCounters {
        if self.method_counters.is_none() {
            self.method_counters = Some(MethodCounters::new());
        }
        self.method_counters.as_mut().unwrap()
    }

    pub fn invocation_count(&self) -> u32 {
        self.method_counters
            .as_ref()
            .map_or(0, |mc| mc.invocation_count())
    }

    pub fn increment_invocation_counter(&mut self, step: u32) -> u32 {
        self.get_or_create_method_counters()
            .increment_invocation_counter(step)
    }

    pub fn backedge_count(&self) -> u32 {
        self.method_counters
            .as_ref()
            .map_or(0, |mc| mc.backedge_count())
    }

    pub fn increment_backedge_counter(&mut self, step: u32) -> u32 {
        self.get_or_create_method_counters()
            .increment_backedge_counter(step)
    }

    pub fn should_compile(&self, threshold: u32) -> bool {
        self.method_counters
            .as_ref()
            .is_some_and(|mc| mc.should_compile(threshold))
    }

    pub fn should_osr_compile(&self, threshold: u32) -> bool {
        self.method_counters
            .as_ref()
            .is_some_and(|mc| mc.should_osr_compile(threshold))
    }

    pub fn highest_comp_level(&self) -> u8 {
        self.method_counters
            .as_ref()
            .map_or(0, |mc| mc.highest_comp_level())
    }

    pub fn set_highest_comp_level(&mut self, level: u8) {
        self.get_or_create_method_counters()
            .set_highest_comp_level(level);
    }
}
