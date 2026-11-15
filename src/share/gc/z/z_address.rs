use std::sync::atomic::{AtomicU64, Ordering};

pub const Z_GRANULE_SIZE_SHIFT: u32 = 21;
pub const Z_GRANULE_SIZE: u64 = 1 << Z_GRANULE_SIZE_SHIFT;
pub const Z_MARK_STRIPES_MAX: usize = 16;
pub const Z_MARK_CACHE_SIZE: usize = 1024;
pub const Z_NAME: &str = "The Z Garbage Collector";

const RESERVED_SHIFT: u32 = 0;
const RESERVED_BITS: u32 = 4;
const REMEMBERED_SHIFT: u32 = RESERVED_SHIFT + RESERVED_BITS;
const REMEMBERED_BITS: u32 = 2;
const MARKED_SHIFT: u32 = REMEMBERED_SHIFT + REMEMBERED_BITS;
const MARKED_BITS: u32 = 6;
const REMAPPED_SHIFT: u32 = MARKED_SHIFT + MARKED_BITS;
const REMAPPED_BITS: u32 = 4;

const fn mask(shift: u32, bits: u32) -> u64 {
    (((1u64 << bits) - 1) << shift)
}

const fn bit(shift: u32, offset: u32) -> u64 {
    1u64 << (shift + offset)
}

pub const Z_POINTER_RESERVED_MASK: u64 = mask(RESERVED_SHIFT, RESERVED_BITS);
pub const Z_POINTER_REMEMBERED_MASK: u64 = mask(REMEMBERED_SHIFT, REMEMBERED_BITS);
pub const Z_POINTER_MARKED_MASK: u64 = mask(MARKED_SHIFT, MARKED_BITS);
pub const Z_POINTER_REMAPPED_MASK: u64 = mask(REMAPPED_SHIFT, REMAPPED_BITS);
pub const Z_POINTER_LOAD_METADATA_MASK: u64 = Z_POINTER_REMAPPED_MASK;
pub const Z_POINTER_MARK_METADATA_MASK: u64 = Z_POINTER_LOAD_METADATA_MASK | Z_POINTER_MARKED_MASK;
pub const Z_POINTER_STORE_METADATA_MASK: u64 = Z_POINTER_MARK_METADATA_MASK | Z_POINTER_REMEMBERED_MASK;
pub const Z_POINTER_ALL_METADATA_MASK: u64 = Z_POINTER_STORE_METADATA_MASK;

pub const Z_POINTER_REMEMBERED_0: u64 = bit(REMEMBERED_SHIFT, 0);
pub const Z_POINTER_REMEMBERED_1: u64 = bit(REMEMBERED_SHIFT, 1);
pub const Z_POINTER_FINALIZABLE_0: u64 = bit(MARKED_SHIFT, 0);
pub const Z_POINTER_FINALIZABLE_1: u64 = bit(MARKED_SHIFT, 1);
pub const Z_POINTER_MARKED_YOUNG_0: u64 = bit(MARKED_SHIFT, 2);
pub const Z_POINTER_MARKED_YOUNG_1: u64 = bit(MARKED_SHIFT, 3);
pub const Z_POINTER_MARKED_OLD_0: u64 = bit(MARKED_SHIFT, 4);
pub const Z_POINTER_MARKED_OLD_1: u64 = bit(MARKED_SHIFT, 5);
pub const Z_POINTER_REMAPPED_00: u64 = bit(REMAPPED_SHIFT, 0);
pub const Z_POINTER_REMAPPED_01: u64 = bit(REMAPPED_SHIFT, 1);
pub const Z_POINTER_REMAPPED_10: u64 = bit(REMAPPED_SHIFT, 2);
pub const Z_POINTER_REMAPPED_11: u64 = bit(REMAPPED_SHIFT, 3);

const LOAD_SHIFT_TABLE: [u32; 16] = [
    (REMAPPED_SHIFT + REMAPPED_SHIFT) as u32,
    (REMAPPED_SHIFT + 1) as u32,
    (REMAPPED_SHIFT + 2) as u32,
    0,
    (REMAPPED_SHIFT + 3) as u32,
    0,
    0,
    0,
    (REMAPPED_SHIFT + 4) as u32,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];

static ADDRESS_OFFSET_BITS: AtomicU64 = AtomicU64::new(44);
static ADDRESS_HEAP_BASE_SHIFT: AtomicU64 = AtomicU64::new(44);
static REMAPPED_YOUNG_MASK: AtomicU64 = AtomicU64::new(Z_POINTER_REMAPPED_10 | Z_POINTER_REMAPPED_00);
static REMAPPED_OLD_MASK: AtomicU64 = AtomicU64::new(Z_POINTER_REMAPPED_01 | Z_POINTER_REMAPPED_00);
static REMAPPED: AtomicU64 = AtomicU64::new(Z_POINTER_REMAPPED_00);
static MARKED_YOUNG: AtomicU64 = AtomicU64::new(Z_POINTER_MARKED_YOUNG_0);
static MARKED_OLD: AtomicU64 = AtomicU64::new(Z_POINTER_MARKED_OLD_0);
static FINALIZABLE: AtomicU64 = AtomicU64::new(Z_POINTER_FINALIZABLE_0);
static REMEMBERED: AtomicU64 = AtomicU64::new(Z_POINTER_REMEMBERED_0);
static LOAD_GOOD_MASK: AtomicU64 = AtomicU64::new(0);
static LOAD_BAD_MASK: AtomicU64 = AtomicU64::new(0);
static MARK_GOOD_MASK: AtomicU64 = AtomicU64::new(0);
static MARK_BAD_MASK: AtomicU64 = AtomicU64::new(0);
static STORE_GOOD_MASK: AtomicU64 = AtomicU64::new(0);
static STORE_BAD_MASK: AtomicU64 = AtomicU64::new(0);
static INITIALIZED: AtomicU64 = AtomicU64::new(0);

fn load(ord: Ordering) -> u64 {
    REMEMBERED.load(ord)
}

fn set_good_masks() {
    let remapped = REMAPPED_YOUNG_MASK
        .load(Ordering::Acquire)
        & REMAPPED_OLD_MASK.load(Ordering::Acquire);
    REMAPPED.store(remapped, Ordering::Release);

    let load_good = remap_bits(remapped);
    let mark_good = load_good | MARKED_YOUNG.load(Ordering::Acquire) | MARKED_OLD.load(Ordering::Acquire);
    let store_good = mark_good | load(Ordering::Acquire);

    LOAD_GOOD_MASK.store(load_good, Ordering::Release);
    MARK_GOOD_MASK.store(mark_good, Ordering::Release);
    STORE_GOOD_MASK.store(store_good, Ordering::Release);
    LOAD_BAD_MASK.store(load_good ^ Z_POINTER_LOAD_METADATA_MASK, Ordering::Release);
    MARK_BAD_MASK.store(mark_good ^ Z_POINTER_MARK_METADATA_MASK, Ordering::Release);
    STORE_BAD_MASK.store(store_good ^ Z_POINTER_STORE_METADATA_MASK, Ordering::Release);
}

pub fn z_globals_initialize(offset_bits: u64) {
    let bits = offset_bits.clamp(42, 44);
    ADDRESS_OFFSET_BITS.store(bits, Ordering::Release);
    ADDRESS_HEAP_BASE_SHIFT.store(bits, Ordering::Release);
    INITIALIZED.store(1, Ordering::Release);
    set_good_masks();
}

pub fn ensure_initialized() {
    if INITIALIZED.load(Ordering::Acquire) == 0 {
        z_globals_initialize(44);
    }
}

pub fn address_offset_bits() -> u64 {
    ensure_initialized();
    ADDRESS_OFFSET_BITS.load(Ordering::Acquire)
}

pub fn address_heap_base_shift() -> u64 {
    ensure_initialized();
    ADDRESS_HEAP_BASE_SHIFT.load(Ordering::Acquire)
}

pub fn address_heap_base() -> u64 {
    1u64 << address_heap_base_shift()
}

pub fn address_offset_mask() -> u64 {
    (1u64 << address_offset_bits()) - 1
}

pub fn address_offset_max() -> u64 {
    1u64 << address_offset_bits()
}

pub fn remapped() -> u64 {
    ensure_initialized();
    REMAPPED.load(Ordering::Acquire)
}

pub fn remapped_young_mask() -> u64 {
    ensure_initialized();
    REMAPPED_YOUNG_MASK.load(Ordering::Acquire)
}

pub fn remapped_old_mask() -> u64 {
    ensure_initialized();
    REMAPPED_OLD_MASK.load(Ordering::Acquire)
}

pub fn marked_young() -> u64 {
    ensure_initialized();
    MARKED_YOUNG.load(Ordering::Acquire)
}

pub fn marked_old() -> u64 {
    ensure_initialized();
    MARKED_OLD.load(Ordering::Acquire)
}

pub fn finalizable() -> u64 {
    ensure_initialized();
    FINALIZABLE.load(Ordering::Acquire)
}

pub fn remembered() -> u64 {
    ensure_initialized();
    REMEMBERED.load(Ordering::Acquire)
}

pub fn load_good_mask() -> u64 {
    ensure_initialized();
    LOAD_GOOD_MASK.load(Ordering::Acquire)
}

pub fn load_bad_mask() -> u64 {
    ensure_initialized();
    LOAD_BAD_MASK.load(Ordering::Acquire)
}

pub fn mark_good_mask() -> u64 {
    ensure_initialized();
    MARK_GOOD_MASK.load(Ordering::Acquire)
}

pub fn mark_bad_mask() -> u64 {
    ensure_initialized();
    MARK_BAD_MASK.load(Ordering::Acquire)
}

pub fn store_good_mask() -> u64 {
    ensure_initialized();
    STORE_GOOD_MASK.load(Ordering::Acquire)
}

pub fn store_bad_mask() -> u64 {
    ensure_initialized();
    STORE_BAD_MASK.load(Ordering::Acquire)
}

pub fn flip_young_mark_start() {
    ensure_initialized();
    MARKED_YOUNG.fetch_xor(Z_POINTER_MARKED_YOUNG_0 | Z_POINTER_MARKED_YOUNG_1, Ordering::AcqRel);
    REMEMBERED.fetch_xor(Z_POINTER_REMEMBERED_0 | Z_POINTER_REMEMBERED_1, Ordering::AcqRel);
    set_good_masks();
}

pub fn flip_young_relocate_start() {
    ensure_initialized();
    REMAPPED_YOUNG_MASK.fetch_xor(Z_POINTER_REMAPPED_MASK, Ordering::AcqRel);
    set_good_masks();
}

pub fn flip_old_mark_start() {
    ensure_initialized();
    MARKED_OLD.fetch_xor(Z_POINTER_MARKED_OLD_0 | Z_POINTER_MARKED_OLD_1, Ordering::AcqRel);
    FINALIZABLE.fetch_xor(Z_POINTER_FINALIZABLE_0 | Z_POINTER_FINALIZABLE_1, Ordering::AcqRel);
    set_good_masks();
}

pub fn flip_old_relocate_start() {
    ensure_initialized();
    REMAPPED_OLD_MASK.fetch_xor(Z_POINTER_REMAPPED_MASK, Ordering::AcqRel);
    set_good_masks();
}

pub fn remap_bits(colored: u64) -> u64 {
    colored & Z_POINTER_REMAPPED_MASK
}

pub fn load_shift_lookup_index(value: u64) -> usize {
    ((value >> REMAPPED_SHIFT) & ((1u64 << REMAPPED_BITS) - 1)) as usize
}

pub fn load_shift_lookup(value: u64) -> u32 {
    LOAD_SHIFT_TABLE[load_shift_lookup_index(value) & 0xf]
}

pub type ZOffset = u64;
pub type ZAddress = u64;
pub type ZAddressUnsafe = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ZPointer(pub u64);

impl ZPointer {
    pub const NULL: Self = Self(0);

    pub fn raw(self) -> u64 {
        self.0
    }

    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    pub fn is_null_any(self) -> bool {
        (self.0 & !Z_POINTER_ALL_METADATA_MASK) == 0
    }

    pub fn is_load_bad(self) -> bool {
        (self.0 & load_bad_mask()) != 0
    }

    pub fn is_load_good(self) -> bool {
        !self.is_load_bad() && !self.is_null()
    }

    pub fn is_load_good_or_null(self) -> bool {
        !self.is_load_bad()
    }

    pub fn is_mark_bad(self) -> bool {
        (self.0 & mark_bad_mask()) != 0
    }

    pub fn is_mark_good(self) -> bool {
        !self.is_mark_bad() && !self.is_null()
    }

    pub fn is_mark_good_or_null(self) -> bool {
        !self.is_mark_bad()
    }

    pub fn is_store_bad(self) -> bool {
        (self.0 & store_bad_mask()) != 0
    }

    pub fn is_store_good(self) -> bool {
        !self.is_store_bad() && !self.is_null()
    }

    pub fn is_store_good_or_null(self) -> bool {
        !self.is_store_bad()
    }

    pub fn is_store_good_or_null_any(self) -> bool {
        !self.is_store_bad() || self.is_null_any()
    }

    pub fn is_young_load_good(self) -> bool {
        (remap_bits(self.0) & remapped_young_mask()) != 0
    }

    pub fn is_old_load_good(self) -> bool {
        (remap_bits(self.0) & remapped_old_mask()) != 0
    }

    pub fn is_marked_finalizable(self) -> bool {
        (self.0 & finalizable()) != 0
    }

    pub fn is_marked_young(self) -> bool {
        (self.0 & marked_young()) != 0
    }

    pub fn is_marked_old(self) -> bool {
        (self.0 & marked_old()) != 0
    }

    pub fn is_marked_any_old(self) -> bool {
        (self.0 & (marked_old() | finalizable())) != 0
    }

    pub fn is_remapped(self) -> bool {
        (remap_bits(self.0) & remapped()) != 0
    }

    pub fn is_remembered_exact(self) -> bool {
        (self.0 & remembered()) == remembered()
    }

    pub fn uncolor(self) -> ZAddress {
        self.0 >> load_shift_lookup(self.0)
    }

    pub fn uncolor_unsafe(self) -> ZAddressUnsafe {
        self.uncolor()
    }

    pub fn offset(self) -> ZOffset {
        z_address_offset(self.uncolor())
    }
}

pub fn offset_to_address(offset: ZOffset) -> ZAddress {
    offset | address_heap_base()
}

pub fn offset_to_address_unsafe(offset: ZOffset) -> ZAddressUnsafe {
    offset_to_address(offset)
}

pub fn address_to_offset(address: ZAddress) -> ZOffset {
    address & address_offset_mask()
}

pub fn offset_to_backing_index(offset: ZOffset) -> u32 {
    (offset >> Z_GRANULE_SIZE_SHIFT) as u32
}

pub fn backing_index_to_offset(index: u32) -> ZOffset {
    (index as u64) << Z_GRANULE_SIZE_SHIFT
}

pub fn z_address_offset(address: ZAddress) -> ZOffset {
    address_to_offset(address)
}

pub fn z_pointer_color(address: ZAddress, color: u64) -> ZPointer {
    ZPointer(((address as u64) << load_shift_lookup(color)) | color)
}

pub fn z_pointer_color_unsafe(address: ZAddressUnsafe, color: u64) -> ZPointer {
    z_pointer_color(address, color)
}

pub fn z_pointer_uncolor(ptr: ZPointer) -> ZAddress {
    ptr.uncolor()
}

pub fn z_pointer_uncolor_store_good(ptr: ZPointer) -> ZAddress {
    ptr.uncolor()
}

pub fn z_pointer_uncolor_unsafe(ptr: ZPointer) -> ZAddressUnsafe {
    ptr.uncolor_unsafe()
}

pub fn color_null() -> ZPointer {
    z_pointer_color(0, store_good_mask() | Z_POINTER_REMEMBERED_MASK)
}

pub fn load_good(address: ZAddress, prev: ZPointer) -> ZPointer {
    if prev.is_null_any() {
        return color_null();
    }
    let non_load_bits_mask = Z_POINTER_LOAD_METADATA_MASK ^ Z_POINTER_ALL_METADATA_MASK;
    let non_load_prev_bits = prev.0 & non_load_bits_mask;
    z_pointer_color(address, load_good_mask() | non_load_prev_bits | Z_POINTER_REMEMBERED_MASK)
}

pub fn finalizable_good(address: ZAddress, prev: ZPointer) -> ZPointer {
    if prev.is_null_any() {
        return color_null();
    }
    z_pointer_color(address, load_good_mask() | marked_young() | finalizable() | Z_POINTER_REMEMBERED_MASK)
}

pub fn mark_good(address: ZAddress, prev: ZPointer) -> ZPointer {
    if prev.is_null_any() {
        return color_null();
    }
    z_pointer_color(address, load_good_mask() | marked_young() | marked_old() | Z_POINTER_REMEMBERED_MASK)
}

pub fn mark_old_good(address: ZAddress, prev: ZPointer) -> ZPointer {
    if prev.is_null_any() {
        return color_null();
    }
    let young_marked = prev.0 & (Z_POINTER_MARKED_YOUNG_0 | Z_POINTER_MARKED_YOUNG_1);
    z_pointer_color(address, load_good_mask() | marked_old() | young_marked | Z_POINTER_REMEMBERED_MASK)
}

pub fn mark_young_good(address: ZAddress, prev: ZPointer) -> ZPointer {
    if prev.is_null_any() {
        return color_null();
    }
    let old_marked = prev.0 & (Z_POINTER_MARKED_MASK ^ (Z_POINTER_MARKED_YOUNG_0 | Z_POINTER_MARKED_YOUNG_1));
    z_pointer_color(address, load_good_mask() | marked_young() | old_marked | Z_POINTER_REMEMBERED_MASK)
}

pub fn store_good(address: ZAddress) -> ZPointer {
    z_pointer_color(address, store_good_mask())
}

pub fn store_good_or_null(address: ZAddress) -> ZPointer {
    if address == 0 { ZPointer::NULL } else { store_good(address) }
}

pub fn is_valid_pointer(ptr: ZPointer) -> bool {
    if ptr.is_null() {
        return true;
    }
    let remap = remap_bits(ptr.0);
    if !remap.is_power_of_two() {
        return false;
    }
    let young = ptr.0 & (Z_POINTER_MARKED_YOUNG_0 | Z_POINTER_MARKED_YOUNG_1);
    let old = ptr.0 & (Z_POINTER_MARKED_OLD_0 | Z_POINTER_MARKED_OLD_1 | Z_POINTER_FINALIZABLE_0 | Z_POINTER_FINALIZABLE_1);
    let remembered_bits = ptr.0 & Z_POINTER_REMEMBERED_MASK;
    young.is_power_of_two() && old.is_power_of_two() && remembered_bits != 0 && (ptr.0 & Z_POINTER_RESERVED_MASK) == 0
}

pub fn is_valid_address(address: ZAddress) -> bool {
    address == 0 || ((address & 0x7) == 0 && (address & address_heap_base()) != 0 && address < address_heap_base() + address_offset_max())
}

pub fn min_address_offset_request(max_heap_size: u64, numa_count: u32) -> u64 {
    let desired = max_heap_size.saturating_mul(16);
    let multiplier = if numa_count > 1 { 2 } else { 1 };
    desired.saturating_mul(multiplier).next_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_round_trip() {
        ensure_initialized();
        let address = offset_to_address(0x12345678);
        let ptr = store_good(address);
        assert!(ptr.is_store_good());
        assert_eq!(ptr.uncolor(), address);
        assert_eq!(ptr.offset(), 0x12345678);
    }

    #[test]
    fn pointer_flips_change_good_mask() {
        ensure_initialized();
        let before = store_good_mask();
        flip_young_mark_start();
        assert_ne!(store_good_mask(), before);
        flip_young_mark_start();
        assert_eq!(store_good_mask(), before);
    }
}
