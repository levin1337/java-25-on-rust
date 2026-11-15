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

/// Narrow class pointer: 22 bits with compact object headers, 32 bits
/// otherwise (`narrowKlass` in HotSpot).
pub type NarrowKlass = u32;

/// Encoding geometry for compressed class pointers
/// (`CompressedKlassPointers` in share/oops/compressedKlass.hpp).
///
/// The narrow klass encodes `klass_address - base` shifted right by
/// `shift`; decoding is `base + (narrow << shift)`.  Unlike narrow oops,
/// there is no mode: klasses always live in a dedicated class space and
/// always use base+shift encoding (`+UseCompressedClassPointers`).
#[derive(Clone, Debug)]
pub struct CompressedKlassPointers {
    base: u64,
    shift: u32,
    /// `narrow_klass_pointer_bits()`: 22 with compact object headers
    /// (klass stored in the mark word), 32 otherwise.
    narrow_bits: u32,
    /// Start of the range of addresses encodable with this scheme.
    klass_range_start: u64,
    /// End (exclusive) of the encodable klass range.
    klass_range_end: u64,
    lowest_valid_narrow_klass_id: NarrowKlass,
    highest_valid_narrow_klass_id: NarrowKlass,
}

impl CompressedKlassPointers {
    pub const NARROW_KLASS_POINTER_BITS: u32 = 22;
    pub const NARROW_KLASS_POINTER_BITS_NONCOH: u32 = 32;
    /// Maximum usable shift: 10 with compact object headers (klass
    /// alignment up to 1KB), 3 otherwise.
    pub const MAX_SHIFT_COH: u32 = 10;
    pub const MAX_SHIFT_NONCOH: u32 = 3;

    /// Creates the encoder for a klass range `[klass_range_start,
    /// klass_range_end)` at the given shift (0..=`max_shift`).
    /// `compact_headers` selects the 22-bit (true) or 32-bit (false)
    /// narrow klass geometry.
    pub fn new(
        klass_range_start: u64,
        klass_range_end: u64,
        shift: u32,
        compact_headers: bool,
    ) -> Self {
        let narrow_bits = if compact_headers {
            Self::NARROW_KLASS_POINTER_BITS
        } else {
            Self::NARROW_KLASS_POINTER_BITS_NONCOH
        };
        let max_shift = if compact_headers {
            Self::MAX_SHIFT_COH
        } else {
            Self::MAX_SHIFT_NONCOH
        };
        assert!(shift <= max_shift, "klass encoding shift out of range");
        // Base is aligned down to 1 << shift; HotSpot may choose a base
        // below `klass_range_start` (e.g. zero for zero-based encoding).
        let base = klass_range_start & !(((1u64 << shift) - 1));
        let lowest = ((klass_range_start - base) >> shift) as NarrowKlass;
        let highest = ((klass_range_end - base) >> shift) as NarrowKlass;
        Self {
            base,
            shift,
            narrow_bits,
            klass_range_start,
            klass_range_end,
            lowest_valid_narrow_klass_id: lowest,
            highest_valid_narrow_klass_id: highest,
        }
    }

    /// Zero-based encoding for klass space starting at `klass_range_start`.
    pub fn zero_based(klass_range_start: u64, klass_range_end: u64, shift: u32) -> Self {
        Self::new(klass_range_start, klass_range_end, shift, false)
            .with_base(0)
    }

    fn with_base(mut self, base: u64) -> Self {
        self.base = base & !(((1u64 << self.shift) - 1));
        self.lowest_valid_narrow_klass_id =
            ((self.klass_range_start.wrapping_sub(self.base)) >> self.shift) as NarrowKlass;
        self.highest_valid_narrow_klass_id =
            ((self.klass_range_end.wrapping_sub(self.base)) >> self.shift) as NarrowKlass;
        self
    }

    #[inline]
    pub fn base(&self) -> u64 {
        self.base
    }

    #[inline]
    pub fn shift(&self) -> u32 {
        self.shift
    }

    #[inline]
    pub fn narrow_klass_pointer_bits(&self) -> u32 {
        self.narrow_bits
    }

    /// `klass_alignment_in_bytes()`: klasses are always at least 8-byte
    /// aligned, so the result is `max(8, 1 << shift)`.
    #[inline]
    pub fn klass_alignment_in_bytes(&self) -> u64 {
        1u64 << self.shift.max(3)
    }

    #[inline]
    pub fn klass_range_start(&self) -> u64 {
        self.klass_range_start
    }

    #[inline]
    pub fn klass_range_end(&self) -> u64 {
        self.klass_range_end
    }

    #[inline]
    pub fn encoding_range_end(&self) -> u64 {
        self.base.wrapping_add(1u64 << (self.narrow_bits + self.shift))
    }

    /// Whether `addr` lies inside the encodable klass range and is
    /// aligned for the current shift (HotSpot `is_encodable`).
    #[inline]
    pub fn is_encodable(&self, addr: u64) -> bool {
        addr >= self.klass_range_start
            && addr < self.klass_range_end
            && (addr & (self.klass_alignment_in_bytes() - 1)) == 0
    }

    #[inline]
    fn is_valid_narrow_klass_id(&self, nk: NarrowKlass) -> bool {
        nk > 0
            && nk >= self.lowest_valid_narrow_klass_id
            && nk <= self.highest_valid_narrow_klass_id
    }

    /// `decode_not_null_without_asserts`: `base + (narrow << shift)`.
    #[inline]
    pub fn decode_not_null_without_asserts(&self, narrow: NarrowKlass) -> usize {
        (self.base.wrapping_add((narrow as u64) << self.shift)) as usize
    }

    /// Decodes a narrow klass pointer; `0` (the null narrow klass) maps
    /// to `None`.
    #[inline]
    pub fn decode(&self, narrow: NarrowKlass) -> Option<usize> {
        if narrow == 0 {
            None
        } else {
            Some(self.decode_not_null(narrow))
        }
    }

    /// `decode_not_null`: asserts the narrow id is nonzero and in range.
    #[inline]
    pub fn decode_not_null(&self, narrow: NarrowKlass) -> usize {
        debug_assert!(narrow != 0, "narrow klass value can never be zero");
        debug_assert!(
            self.is_valid_narrow_klass_id(narrow),
            "narrowKlass ID out of range ({narrow})"
        );
        self.decode_not_null_without_asserts(narrow)
    }

    /// Decodes without range/alignment checks.
    #[inline]
    pub fn decode_without_asserts(&self, narrow: NarrowKlass) -> Option<usize> {
        if narrow == 0 {
            None
        } else {
            Some(self.decode_not_null_without_asserts(narrow))
        }
    }

    /// `encode_not_null_without_asserts`: `(klass - base) >> shift`.
    #[inline]
    pub fn encode_not_null_without_asserts(&self, klass: usize) -> NarrowKlass {
        ((klass as u64).wrapping_sub(self.base) >> self.shift) as NarrowKlass
    }

    /// Encodes a klass address; `0` encodes to the null narrow klass.
    /// Returns `None` when the address is not encodable.
    #[inline]
    pub fn encode(&self, klass: usize) -> Option<NarrowKlass> {
        if klass == 0 {
            return Some(0);
        }
        if !self.is_encodable(klass as u64) {
            return None;
        }
        Some(self.encode_not_null(klass))
    }

    /// `encode_not_null`: asserts encodability and reversibility.
    #[inline]
    pub fn encode_not_null(&self, klass: usize) -> NarrowKlass {
        debug_assert!(klass != 0, "klass value can never be zero");
        debug_assert!(
            self.is_encodable(klass as u64),
            "Address {klass:#x} is not encodable"
        );
        let nk = self.encode_not_null_without_asserts(klass);
        debug_assert_eq!(
            self.decode_not_null_without_asserts(nk),
            klass,
            "encode/decode not reversible"
        );
        debug_assert!(self.is_valid_narrow_klass_id(nk));
        debug_assert!(
            (nk as u64) < (1u64 << self.narrow_bits),
            "narrow klass id bit spillover ({nk})"
        );
        nk
    }
}

/// Convenience: decode a narrow klass pointer read from an object header
/// under compact object headers, where `markWord::narrow_klass()` yields
/// the raw bits.
#[inline]
pub fn decode_narrow_klass(ckp: &CompressedKlassPointers, narrow: NarrowKlass) -> Option<usize> {
    ckp.decode(narrow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_oops_modes_roundtrip() {
        for mode in [
            NarrowOopMode::Unscaled32Bit,
            NarrowOopMode::ZeroBased,
            NarrowOopMode::DisjointBase,
            NarrowOopMode::HeapBased,
        ] {
            let co = CompressedOops::new(0x1_0000_0000, 3, mode);
            let addr = 0x1_0010_0040usize;
            let narrow = co.encode(addr);
            assert_eq!(co.decode(narrow), addr);
            assert_eq!(co.decode(0), 0);
        }
    }

    #[test]
    fn klass_pointer_roundtrip() {
        // 4GB class space at 0x4_0000_0000, zero-based encoding, shift 3.
        let ckp = CompressedKlassPointers::zero_based(0x4_0000_0000, 0x5_0000_0000, 3);
        assert_eq!(ckp.base(), 0);
        let klass = 0x4_0000_0040usize;
        let nk = ckp.encode(klass).unwrap();
        // (0x4_0000_0040 - 0) >> 3 = 0x8000_0008.
        assert_eq!(nk, 0x8000_0008);
        assert_eq!(ckp.decode(nk), Some(klass));
        assert_eq!(ckp.decode(0), None);
        // Out-of-range address is not encodable.
        assert!(ckp.encode(0x1_0000usize).is_none());
        // Misaligned address is not encodable (klass alignment >= 8).
        assert!(ckp.encode(0x4_0000_0041).is_none());
    }

    #[test]
    fn klass_pointer_disjoint_base() {
        let ckp = CompressedKlassPointers::new(0x7_0000_0000, 0x8_0000_0000, 0, false);
        let klass = 0x7_0000_0100usize;
        let nk = ckp.encode(klass).unwrap();
        assert_eq!(nk, 0x100);
        assert_eq!(ckp.decode(nk), Some(klass));
        assert_eq!(ckp.klass_alignment_in_bytes(), 8);
    }
}
