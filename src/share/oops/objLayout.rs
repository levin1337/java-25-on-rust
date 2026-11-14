/*
 * Port of OpenJDK 25 HotSpot ObjLayout (share/oops/objLayout.hpp, objLayout.cpp)
 *
 * ObjLayout encapsulates object header layouts, compressed klass pointer settings,
 * and base offsets for fields and arrays in the Java heap.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjLayoutMode {
    /// +UseCompactObjectHeaders (narrow Klass* stored inside the MarkWord)
    Compact,
    /// +UseCompressedClassPointers (-UseCompactObjectHeaders)
    #[default]
    Compressed,
    /// -UseCompressedClassPointers (-UseCompactObjectHeaders)
    Uncompressed,
    /// Uninitialized state
    Undefined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjLayout {
    mode: ObjLayoutMode,
    oop_base_offset: usize,
    oop_has_klass_gap: bool,
}

impl Default for ObjLayout {
    fn default() -> Self {
        Self::new(ObjLayoutMode::Compressed)
    }
}

impl ObjLayout {
    pub const MARKWORD_SIZE: usize = 8;
    pub const NARROW_KLASS_SIZE: usize = 4;
    pub const WIDE_KLASS_SIZE: usize = 8;
    pub const ARRAY_LENGTH_SIZE: usize = 4;
    pub const HEAP_WORD_SIZE: usize = 8;

    pub fn new(mode: ObjLayoutMode) -> Self {
        let (oop_base_offset, oop_has_klass_gap) = match mode {
            ObjLayoutMode::Compact => (Self::MARKWORD_SIZE, false),
            ObjLayoutMode::Compressed => (Self::MARKWORD_SIZE + Self::NARROW_KLASS_SIZE, true),
            ObjLayoutMode::Uncompressed | ObjLayoutMode::Undefined => {
                (Self::MARKWORD_SIZE + Self::WIDE_KLASS_SIZE, false)
            }
        };

        Self {
            mode,
            oop_base_offset,
            oop_has_klass_gap,
        }
    }

    #[inline]
    pub fn mode(&self) -> ObjLayoutMode {
        self.mode
    }

    #[inline]
    pub fn oop_base_offset_in_bytes(&self) -> usize {
        self.oop_base_offset
    }

    #[inline]
    pub fn oop_has_klass_gap(&self) -> bool {
        self.oop_has_klass_gap
    }

    #[inline]
    pub fn header_size_in_bytes(&self) -> usize {
        self.oop_base_offset
    }

    /// Offset of array length in bytes.
    #[inline]
    pub fn length_offset_in_bytes(&self) -> usize {
        self.oop_base_offset
    }

    /// Size of array header including length, aligned to heap word (8 bytes).
    #[inline]
    pub fn array_header_size_in_bytes(&self) -> usize {
        let raw_header = self.length_offset_in_bytes() + Self::ARRAY_LENGTH_SIZE;
        Self::align_up(raw_header, Self::HEAP_WORD_SIZE)
    }

    /// Offset of the first element in an array of given element alignment.
    #[inline]
    pub fn array_base_offset_in_bytes(&self, element_align: usize) -> usize {
        let hs = self.length_offset_in_bytes() + Self::ARRAY_LENGTH_SIZE;
        Self::align_up(hs, element_align.max(Self::HEAP_WORD_SIZE))
    }

    #[inline]
    pub fn align_up(val: usize, alignment: usize) -> usize {
        debug_assert!(alignment > 0 && (alignment & (alignment - 1)) == 0);
        (val + alignment - 1) & !(alignment - 1)
    }
}

impl ObjLayoutMode {
    #[inline]
    pub fn array_length_offset(self) -> usize {
        ObjLayout::new(self).length_offset_in_bytes()
    }

    #[inline]
    pub fn narrow_oop_size(self) -> usize {
        match self {
            ObjLayoutMode::Compressed | ObjLayoutMode::Compact => 4,
            _ => 8,
        }
    }

    #[inline]
    pub fn array_base_offset(self, _elem_size: usize, elem_align: usize) -> usize {
        ObjLayout::new(self).array_base_offset_in_bytes(elem_align)
    }

    #[inline]
    pub fn array_allocation_size_in_bytes(
        self,
        length: usize,
        elem_size: usize,
        elem_align: usize,
    ) -> usize {
        let base = self.array_base_offset(elem_size, elem_align);
        let raw_size = base + length * elem_size;
        ObjLayout::align_up(raw_size, ObjLayout::HEAP_WORD_SIZE)
    }
}
