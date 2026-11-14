/*
 * Port of OpenJDK 25 HotSpot ResolvedMethodEntry (share/oops/resolvedMethodEntry.hpp)
 *
 * ResolvedMethodEntry contains resolution information for method invoke bytecodes
 * (invokevirtual, invokespecial, invokestatic, invokeinterface).
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMethodEntry {
    cpool_index: u16,
    klass_name: Option<String>,
    method_name: Option<String>,
    signature: Option<String>,
    vtable_index: i32,
    itable_index: i32,
    number_of_parameters: u16,
    flags: u16,
    bytecode1: u8,
    bytecode2: u8,
}

impl ResolvedMethodEntry {
    pub const FLAG_IS_VIRTUAL: u16 = 1 << 0;
    pub const FLAG_IS_FINAL: u16 = 1 << 1;
    pub const FLAG_IS_INTERFACE: u16 = 1 << 2;
    pub const FLAG_IS_STATIC: u16 = 1 << 3;
    pub const FLAG_HAS_LOCAL_SIGNATURE: u16 = 1 << 4;

    pub const NONVIRTUAL_VTABLE_INDEX: i32 = -1;

    pub fn new(cpool_index: u16) -> Self {
        Self {
            cpool_index,
            klass_name: None,
            method_name: None,
            signature: None,
            vtable_index: Self::NONVIRTUAL_VTABLE_INDEX,
            itable_index: -1,
            number_of_parameters: 0,
            flags: 0,
            bytecode1: 0,
            bytecode2: 0,
        }
    }

    #[inline]
    pub fn cpool_index(&self) -> u16 {
        self.cpool_index
    }

    #[inline]
    pub fn is_resolved(&self, bytecode: u8) -> bool {
        self.bytecode1 == bytecode || self.bytecode2 == bytecode
    }

    #[inline]
    pub fn klass_name(&self) -> Option<&str> {
        self.klass_name.as_deref()
    }

    #[inline]
    pub fn method_name(&self) -> Option<&str> {
        self.method_name.as_deref()
    }

    #[inline]
    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    #[inline]
    pub fn vtable_index(&self) -> i32 {
        self.vtable_index
    }

    #[inline]
    pub fn itable_index(&self) -> i32 {
        self.itable_index
    }

    #[inline]
    pub fn number_of_parameters(&self) -> u16 {
        self.number_of_parameters
    }

    #[inline]
    pub fn flags(&self) -> u16 {
        self.flags
    }

    #[inline]
    pub fn is_virtual(&self) -> bool {
        (self.flags & Self::FLAG_IS_VIRTUAL) != 0
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        (self.flags & Self::FLAG_IS_FINAL) != 0
    }

    #[inline]
    pub fn is_interface(&self) -> bool {
        (self.flags & Self::FLAG_IS_INTERFACE) != 0
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        (self.flags & Self::FLAG_IS_STATIC) != 0
    }

    #[inline]
    pub fn bytecode1(&self) -> u8 {
        self.bytecode1
    }

    #[inline]
    pub fn bytecode2(&self) -> u8 {
        self.bytecode2
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fill_in(
        &mut self,
        klass_name: String,
        method_name: String,
        signature: String,
        vtable_index: i32,
        itable_index: i32,
        number_of_parameters: u16,
        flags: u16,
        bytecode1: u8,
        bytecode2: u8,
    ) {
        self.klass_name = Some(klass_name);
        self.method_name = Some(method_name);
        self.signature = Some(signature);
        self.vtable_index = vtable_index;
        self.itable_index = itable_index;
        self.number_of_parameters = number_of_parameters;
        self.flags = flags;
        self.bytecode1 = bytecode1;
        self.bytecode2 = bytecode2;
    }
}
