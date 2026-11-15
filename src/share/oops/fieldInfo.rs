/*
 * Port of OpenJDK 25 HotSpot FieldInfo (share/oops/fieldInfo.hpp, fieldInfo.cpp)
 *
 * FieldInfo holds metadata for a field declared in a class:
 * - Access flags (JVM spec)
 * - Field flags (internal VM flags: injected, stable, contended, etc.)
 * - Name and descriptor byte slices
 * - Slot index and byte offset in the instance or static memory layout
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldFlags {
    flags: u32,
}

impl FieldFlags {
    pub const FF_INITIALIZED: u32 = 1 << 0;
    pub const FF_INJECTED: u32 = 1 << 1;
    pub const FF_GENERIC: u32 = 1 << 2;
    pub const FF_STABLE: u32 = 1 << 3;
    pub const FF_CONTENDED: u32 = 1 << 4;

    pub fn new(flags: u32) -> Self {
        Self { flags }
    }

    #[inline]
    pub fn as_uint(&self) -> u32 {
        self.flags
    }

    #[inline]
    pub fn is_initialized(&self) -> bool {
        (self.flags & Self::FF_INITIALIZED) != 0
    }

    #[inline]
    pub fn is_injected(&self) -> bool {
        (self.flags & Self::FF_INJECTED) != 0
    }

    #[inline]
    pub fn is_generic(&self) -> bool {
        (self.flags & Self::FF_GENERIC) != 0
    }

    #[inline]
    pub fn is_stable(&self) -> bool {
        (self.flags & Self::FF_STABLE) != 0
    }

    #[inline]
    pub fn is_contended(&self) -> bool {
        (self.flags & Self::FF_CONTENDED) != 0
    }

    #[inline]
    pub fn set_initialized(&mut self, val: bool) {
        if val {
            self.flags |= Self::FF_INITIALIZED;
        } else {
            self.flags &= !Self::FF_INITIALIZED;
        }
    }

    #[inline]
    pub fn set_injected(&mut self, val: bool) {
        if val {
            self.flags |= Self::FF_INJECTED;
        } else {
            self.flags &= !Self::FF_INJECTED;
        }
    }

    #[inline]
    pub fn set_contended(&mut self, val: bool) {
        if val {
            self.flags |= Self::FF_CONTENDED;
        } else {
            self.flags &= !Self::FF_CONTENDED;
        }
    }
}

/// Machine-level kind of a field or method signature for JIT code
/// generation.  Mirrors the categories JIT frontends need for
/// getfield/putfield/getstatic/putstatic lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Int32,
    Int64,
    Float32,
    Float64,
    /// Reference type (`L...;` or array `[...`).
    Ref,
}

/// Maps a JVM field descriptor (`I`, `J`, `Ljava/lang/String;`, `[I`, ...)
/// to its machine kind.  Returns `None` for `V` and malformed input.
pub fn descriptor_kind(descriptor: &[u8]) -> Option<FieldKind> {
    match descriptor.first()? {
        b'I' | b'S' | b'C' | b'B' | b'Z' => Some(FieldKind::Int32),
        b'J' => Some(FieldKind::Int64),
        b'F' => Some(FieldKind::Float32),
        b'D' => Some(FieldKind::Float64),
        b'L' | b'[' => Some(FieldKind::Ref),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo<'a> {
    pub access_flags: u16,
    pub name: &'a [u8],
    pub descriptor: &'a [u8],
    pub slot: usize,
    pub offset: u32,
    pub field_flags: FieldFlags,
}

impl<'a> FieldInfo<'a> {
    pub fn new(access_flags: u16, name: &'a [u8], descriptor: &'a [u8], slot: usize) -> Self {
        Self {
            access_flags,
            name,
            descriptor,
            slot,
            offset: 0,
            field_flags: FieldFlags::default(),
        }
    }

    pub fn with_offset(
        access_flags: u16,
        name: &'a [u8],
        descriptor: &'a [u8],
        slot: usize,
        offset: u32,
    ) -> Self {
        Self {
            access_flags,
            name,
            descriptor,
            slot,
            offset,
            field_flags: FieldFlags::default(),
        }
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        (self.access_flags & 0x0008) != 0
    }

    #[inline]
    pub fn is_public(&self) -> bool {
        (self.access_flags & 0x0001) != 0
    }

    #[inline]
    pub fn is_private(&self) -> bool {
        (self.access_flags & 0x0002) != 0
    }

    #[inline]
    pub fn is_protected(&self) -> bool {
        (self.access_flags & 0x0004) != 0
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        (self.access_flags & 0x0010) != 0
    }

    #[inline]
    pub fn is_volatile(&self) -> bool {
        (self.access_flags & 0x0040) != 0
    }

    #[inline]
    pub fn is_transient(&self) -> bool {
        (self.access_flags & 0x0080) != 0
    }

    #[inline]
    pub fn is_synthetic(&self) -> bool {
        (self.access_flags & 0x1000) != 0
    }

    #[inline]
    pub fn offset(&self) -> u32 {
        self.offset
    }

    #[inline]
    pub fn set_offset(&mut self, offset: u32) {
        self.offset = offset;
    }

    #[inline]
    pub fn is_reference(&self) -> bool {
        matches!(self.descriptor.first(), Some(b'L' | b'['))
    }

    /// Size of the field in bytes based on JVM type descriptor.
    pub fn size_in_bytes(&self) -> usize {
        match self.descriptor.first() {
            Some(b'J' | b'D') => 8,
            Some(b'I' | b'F') => 4,
            Some(b'S' | b'C') => 2,
            Some(b'B' | b'Z') => 1,
            Some(b'L' | b'[') => 4, // 4 bytes for compressed oops (standard JVM mode)
            _ => 4,
        }
    }

    /// Required memory alignment in bytes for the field.
    pub fn alignment(&self) -> usize {
        self.size_in_bytes()
    }

    /// Machine kind of this field for JIT lowering.
    pub fn kind(&self) -> Option<FieldKind> {
        descriptor_kind(self.descriptor)
    }

    /// Number of local/stack slots the field occupies (long/double = 2).
    pub fn size_in_slots(&self) -> usize {
        match self.kind() {
            Some(FieldKind::Int64) | Some(FieldKind::Float64) => 2,
            _ => 1,
        }
    }
}
