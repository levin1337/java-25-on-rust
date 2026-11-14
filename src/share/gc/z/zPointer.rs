/*
 * Port of OpenJDK 25 HotSpot ZGC Colored Pointers (share/gc/z/zAddress.hpp / zPointer.hpp)
 *
 * Implements 64-bit colored pointers with metadata bits for concurrent marking,
 * relocation, and remap tracking.
 *
 * Layout:
 * [63..48]: Unused / Virtual address bits
 * [47]: Finalizable bit
 * [46]: Remapped bit
 * [45]: Marked1 bit
 * [44]: Marked0 bit
 * [43..0]: Object address / offset inside heap
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ZPointer(pub u64);

impl ZPointer {
    pub const OFFSET_MASK: u64 = (1 << 44) - 1;

    pub const COLOR_MARKED_0: u64 = 1 << 44;
    pub const COLOR_MARKED_1: u64 = 1 << 45;
    pub const COLOR_REMAPPED: u64 = 1 << 46;
    pub const COLOR_FINALIZABLE: u64 = 1 << 47;
    pub const COLOR_MASK: u64 = Self::COLOR_MARKED_0
        | Self::COLOR_MARKED_1
        | Self::COLOR_REMAPPED
        | Self::COLOR_FINALIZABLE;

    pub fn new(address: usize, color: u64) -> Self {
        let addr = (address as u64) & Self::OFFSET_MASK;
        Self(addr | (color & Self::COLOR_MASK))
    }

    pub fn uncolored_address(self) -> usize {
        (self.0 & Self::OFFSET_MASK) as usize
    }

    pub fn color(self) -> u64 {
        self.0 & Self::COLOR_MASK
    }

    pub fn is_marked(self) -> bool {
        (self.0 & (Self::COLOR_MARKED_0 | Self::COLOR_MARKED_1)) != 0
    }

    pub fn is_remapped(self) -> bool {
        (self.0 & Self::COLOR_REMAPPED) != 0
    }

    pub fn is_finalizable(self) -> bool {
        (self.0 & Self::COLOR_FINALIZABLE) != 0
    }

    pub fn as_remapped(self) -> Self {
        let uncolored = self.0 & Self::OFFSET_MASK;
        Self(uncolored | Self::COLOR_REMAPPED)
    }

    pub fn as_marked(self, use_marked_1: bool) -> Self {
        let uncolored = self.0 & Self::OFFSET_MASK;
        let mark_bit = if use_marked_1 {
            Self::COLOR_MARKED_1
        } else {
            Self::COLOR_MARKED_0
        };
        Self(uncolored | mark_bit)
    }
}
