/*
 * Port of OpenJDK 25 HotSpot CompressedOops (share/oops/compressedOops.hpp / compressedOops.cpp)
 *
 * Implements 32-bit narrow OOP encoding and decoding for heaps up to 32GB (ZeroBased),
 * heaps with disjoint base, or unscaled 32-bit heaps.
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NarrowOopMode {
    Unscaled32Bit,
    ZeroBased,
    DisjointBase,
    HeapBased,
}

#[derive(Clone, Debug)]
pub struct CompressedOops {
    base: u64,
    shift: u32,
    mode: NarrowOopMode,
}

impl CompressedOops {
    pub const DEFAULT_SHIFT: u32 = 3; // LogMinObjAlignmentInBytes (8 bytes alignment)

    pub fn new(base: u64, shift: u32, mode: NarrowOopMode) -> Self {
        Self { base, shift, mode }
    }

    /// Automatically configures narrow OOP mode and base based on heap range
    pub fn for_heap(heap_base: u64, heap_size: usize) -> Self {
        let heap_end = heap_base + heap_size as u64;

        if heap_end <= (4 * 1024 * 1024 * 1024) && heap_base == 0 {
            // Unscaled 32-bit (< 4GB)
            Self::new(0, 0, NarrowOopMode::Unscaled32Bit)
        } else if heap_end <= (32 * 1024 * 1024 * 1024) && heap_base == 0 {
            // Zero-based (< 32GB)
            Self::new(0, Self::DEFAULT_SHIFT, NarrowOopMode::ZeroBased)
        } else if (heap_base & ((1 << (32 + Self::DEFAULT_SHIFT)) - 1)) == 0 {
            // Disjoint base
            Self::new(heap_base, Self::DEFAULT_SHIFT, NarrowOopMode::DisjointBase)
        } else {
            // General heap-based
            Self::new(heap_base, Self::DEFAULT_SHIFT, NarrowOopMode::HeapBased)
        }
    }

    pub fn mode(&self) -> NarrowOopMode {
        self.mode
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn shift(&self) -> u32 {
        self.shift
    }

    /// Compresses a 64-bit oop address into a 32-bit narrow oop
    #[inline]
    pub fn encode(&self, oop: usize) -> u32 {
        if oop == 0 {
            return 0;
        }
        let addr = oop as u64;
        match self.mode {
            NarrowOopMode::Unscaled32Bit => addr as u32,
            NarrowOopMode::ZeroBased => (addr >> self.shift) as u32,
            NarrowOopMode::DisjointBase | NarrowOopMode::HeapBased => {
                ((addr.wrapping_sub(self.base)) >> self.shift) as u32
            }
        }
    }

    /// Decompresses a 32-bit narrow oop back into a 64-bit oop address
    #[inline]
    pub fn decode(&self, narrow: u32) -> usize {
        if narrow == 0 {
            return 0;
        }
        let val = narrow as u64;
        let addr = match self.mode {
            NarrowOopMode::Unscaled32Bit => val,
            NarrowOopMode::ZeroBased => val << self.shift,
            NarrowOopMode::DisjointBase | NarrowOopMode::HeapBased => {
                (val << self.shift).wrapping_add(self.base)
            }
        };
        addr as usize
    }
}

impl Default for CompressedOops {
    fn default() -> Self {
        Self::for_heap(0, 512 * 1024 * 1024)
    }
}
