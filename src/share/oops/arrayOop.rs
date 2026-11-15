use super::markWord::MarkWord;
use super::objLayout::ObjLayoutMode;

#[derive(Debug, Clone)]
pub struct ArrayOopDesc {
    pub mark: MarkWord,
    pub length: i32,
}

impl ArrayOopDesc {
    pub fn new(length: i32) -> Self {
        Self {
            mark: MarkWord::prototype(),
            length,
        }
    }

    pub fn length(&self) -> i32 {
        self.length
    }

    pub fn set_length(&mut self, length: i32) {
        self.length = length;
    }

    pub fn is_within_bounds(&self, index: i32) -> bool {
        0 <= index && index < self.length
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.length <= 0
    }

    #[inline]
    pub fn header_size_in_bytes(mode: ObjLayoutMode) -> usize {
        mode.array_base_offset(4, 4)
    }

    #[inline]
    pub fn length_offset_in_bytes(mode: ObjLayoutMode) -> usize {
        mode.array_length_offset()
    }

    #[inline]
    pub fn base_offset_in_bytes(mode: ObjLayoutMode, elem_align: usize) -> usize {
        mode.array_base_offset(elem_align, elem_align)
    }

    /// Byte offset of element `index` for an element of `elem_size`/
    /// `elem_align` — the formula JIT array load/store code uses.
    #[inline]
    pub fn element_offset_in_bytes(
        mode: ObjLayoutMode,
        index: usize,
        elem_size: usize,
        elem_align: usize,
    ) -> usize {
        Self::base_offset_in_bytes(mode, elem_align) + index * elem_size
    }

    /// Total allocated size in bytes for `length` elements.
    #[inline]
    pub fn object_size_in_bytes(
        mode: ObjLayoutMode,
        length: usize,
        elem_size: usize,
        elem_align: usize,
    ) -> usize {
        mode.array_allocation_size_in_bytes(length, elem_size, elem_align)
    }

    /// Total allocated size in heap words.
    #[inline]
    pub fn object_size_in_words(
        mode: ObjLayoutMode,
        length: usize,
        elem_size: usize,
        elem_align: usize,
    ) -> usize {
        Self::object_size_in_bytes(mode, length, elem_size, elem_align) / 8
    }

    /// Maximum legal array length for an element of `elem_size`
    /// (HotSpot `arrayOop::max_array_length`).
    pub fn max_array_length(elem_size: usize) -> i32 {
        if elem_size == 0 {
            return 0;
        }
        let max_bytes = (i32::MAX as usize).saturating_sub(32);
        (max_bytes / elem_size) as i32
    }

    /// Allocated size in bytes for this descriptor under `mode`.
    pub fn allocated_size_in_bytes(&self, mode: ObjLayoutMode) -> usize {
        Self::object_size_in_bytes(mode, self.length.max(0) as usize, 4, 4)
    }
}
