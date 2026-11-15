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

/// Resolved byte layout of a Java array object for JIT code generation:
/// where `length` lives and where element 0 starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayLayout {
    /// Byte offset of the `length` field (i32).
    pub length_offset: usize,
    /// Byte offset of element 0.
    pub data_offset: usize,
    /// Size of one element in bytes.
    pub element_size: usize,
    /// Alignment of one element in bytes.
    pub element_align: usize,
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

    /// Full array layout (length offset + data offset) for an element of
    /// the given size/alignment — the constants JIT array
    /// load/store/length instructions need.
    #[inline]
    pub fn array_layout(&self, element_size: usize, element_align: usize) -> ArrayLayout {
        ArrayLayout {
            length_offset: self.length_offset_in_bytes(),
            data_offset: self.array_base_offset_in_bytes(element_align),
            element_size,
            element_align,
        }
    }

    /// Array layout for a compressed-oop (`narrow oop`) reference array.
    #[inline]
    pub fn ref_array_layout(&self) -> ArrayLayout {
        let oop = self.mode.narrow_oop_size();
        self.array_layout(oop, oop)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_mode_layout_constants() {
        let layout = ObjLayout::default();
        // HotSpot compressed-oops layout: mark(8) + narrow klass(4),
        // length(4) -> data starts at 16 for every element kind.
        assert_eq!(layout.length_offset_in_bytes(), 12);
        for align in [1usize, 2, 4, 8] {
            assert_eq!(layout.array_base_offset_in_bytes(align), 16);
        }
        assert_eq!(layout.array_header_size_in_bytes(), 16);
    }

    #[test]
    fn uncompressed_mode_layout_constants() {
        let layout = ObjLayout::new(ObjLayoutMode::Uncompressed);
        // mark(8) + wide klass(8) = 16; length at 16, data aligned to 8 -> 24.
        assert_eq!(layout.length_offset_in_bytes(), 16);
        assert_eq!(layout.array_base_offset_in_bytes(1), 24);
        assert_eq!(layout.array_base_offset_in_bytes(8), 24);
    }

    #[test]
    fn compact_mode_layout_constants() {
        let layout = ObjLayout::new(ObjLayoutMode::Compact);
        // mark(8) contains the narrow klass; length at 8, data at 16.
        assert_eq!(layout.length_offset_in_bytes(), 8);
        assert_eq!(layout.array_base_offset_in_bytes(8), 16);
    }

    #[test]
    fn array_layout_reports_length_and_data_offsets() {
        let layout = ObjLayout::default();
        let ints = layout.array_layout(4, 4);
        assert_eq!(ints.length_offset, 12);
        assert_eq!(ints.data_offset, 16);
        assert_eq!(ints.element_size, 4);
        let longs = layout.array_layout(8, 8);
        assert_eq!(longs.data_offset, 16);
        let refs = layout.ref_array_layout();
        assert_eq!(refs.element_size, 4);
        assert_eq!(refs.data_offset, 16);
    }

    #[test]
    fn allocation_size_is_word_aligned() {
        let mode = ObjLayoutMode::Compressed;
        // 3 ints: base 16 + 12 = 28 -> align to 32.
        assert_eq!(mode.array_allocation_size_in_bytes(3, 4, 4), 32);
        // empty array still occupies the header.
        assert_eq!(mode.array_allocation_size_in_bytes(0, 4, 4), 16);
    }
}
