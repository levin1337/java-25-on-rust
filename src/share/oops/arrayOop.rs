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

    pub fn header_size_in_bytes(mode: ObjLayoutMode) -> usize {
        mode.array_base_offset(4, 4)
    }

    pub fn length_offset_in_bytes(mode: ObjLayoutMode) -> usize {
        mode.array_length_offset()
    }

    pub fn base_offset_in_bytes(mode: ObjLayoutMode, elem_align: usize) -> usize {
        mode.array_base_offset(elem_align, elem_align)
    }

    pub fn max_array_length(elem_size: usize) -> i32 {
        if elem_size == 0 {
            return 0;
        }
        let max_bytes = (i32::MAX as usize).saturating_sub(32);
        (max_bytes / elem_size) as i32
    }
}
