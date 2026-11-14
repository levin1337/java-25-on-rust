/*
 * Port of OpenJDK 25 HotSpot markWord (share/oops/markWord.hpp, markWord.cpp)
 *
 * The markWord describes the header of an object:
 * - 2 lock bits (locked: 00, unlocked: 01, monitor: 10, marked: 11)
 * - 1 self-forward bit
 * - 4 age bits (0..15)
 * - 1 unused gap bit
 * - 31 identity hash bits
 * - 22 compact klass bits (when compact object headers are enabled)
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkWord {
    pub value: usize,
}

impl MarkWord {
    // Lock values and masks
    pub const LOCKED: usize = 0b00;
    pub const UNLOCKED: usize = 0b01;
    pub const MONITORED: usize = 0b10;
    pub const MARKED: usize = 0b11;
    pub const LOCK_MASK: usize = 0b11;

    pub const LOCKED_VALUE: usize = Self::LOCKED;
    pub const UNLOCKED_VALUE: usize = Self::UNLOCKED;
    pub const MONITOR_VALUE: usize = Self::MONITORED;
    pub const MARKED_VALUE: usize = Self::MARKED;

    // Bitfield specifications
    pub const LOCK_BITS: usize = 2;
    pub const SELF_FWD_BITS: usize = 1;
    pub const AGE_BITS: usize = 4;
    pub const UNUSED_GAP_BITS: usize = 1;
    pub const HASH_BITS: usize = 31;
    pub const KLASS_BITS: usize = 22;

    pub const LOCK_SHIFT: usize = 0;
    pub const SELF_FWD_SHIFT: usize = Self::LOCK_SHIFT + Self::LOCK_BITS; // 2
    pub const AGE_SHIFT: usize = Self::SELF_FWD_SHIFT + Self::SELF_FWD_BITS; // 3
    pub const HASH_SHIFT: usize = 8; // Bit 7 reserved gap, hash starts at bit 8
    pub const KLASS_SHIFT: usize = 42; // Upper 22 bits for compact klass pointer

    pub const LOCK_MASK_IN_PLACE: usize = Self::LOCK_MASK;
    pub const SELF_FWD_MASK: usize = (1 << Self::SELF_FWD_BITS) - 1;
    pub const SELF_FWD_MASK_IN_PLACE: usize = Self::SELF_FWD_MASK << Self::SELF_FWD_SHIFT;
    pub const AGE_MASK: usize = (1 << Self::AGE_BITS) - 1; // 0xF
    pub const AGE_MASK_IN_PLACE: usize = Self::AGE_MASK << Self::AGE_SHIFT;
    pub const MAX_AGE: u32 = Self::AGE_MASK as u32; // 15
    pub const HASH_MASK: usize = (1 << Self::HASH_BITS) - 1; // 0x7FFF_FFFF
    pub const HASH_MASK_IN_PLACE: usize = Self::HASH_MASK << Self::HASH_SHIFT;
    pub const KLASS_MASK: usize = (1 << Self::KLASS_BITS) - 1;
    pub const KLASS_MASK_IN_PLACE: usize = Self::KLASS_MASK << Self::KLASS_SHIFT;

    pub const NO_HASH: usize = 0;

    #[inline]
    pub const fn from_raw(value: usize) -> Self {
        Self { value }
    }

    #[inline]
    pub const fn zero() -> Self {
        Self { value: 0 }
    }

    #[inline]
    pub const fn prototype() -> Self {
        Self {
            value: Self::UNLOCKED,
        }
    }

    /// Special distinguished markword value when inflating over an existing stack-lock.
    #[inline]
    pub const fn INFLATING() -> Self {
        Self::zero()
    }

    #[inline]
    pub fn is_being_inflated(&self) -> bool {
        self.value == 0
    }

    #[inline]
    pub fn is_unlocked(&self) -> bool {
        (self.value & Self::LOCK_MASK) == Self::UNLOCKED
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        (self.value & Self::LOCK_MASK) == Self::LOCKED
    }

    #[inline]
    pub fn is_marked(&self) -> bool {
        (self.value & Self::LOCK_MASK) == Self::MARKED
    }

    #[inline]
    pub fn is_monitored(&self) -> bool {
        (self.value & Self::LOCK_MASK) == Self::MONITORED
    }

    #[inline]
    pub fn is_neutral(&self) -> bool {
        self.is_unlocked()
    }

    #[inline]
    pub fn is_forwarded(&self) -> bool {
        (self.value & (Self::LOCK_MASK_IN_PLACE | Self::SELF_FWD_MASK_IN_PLACE))
            >= Self::MARKED_VALUE
    }

    #[inline]
    pub fn is_self_forwarded(&self) -> bool {
        (self.value & Self::SELF_FWD_MASK_IN_PLACE) != 0
    }

    #[inline]
    pub fn set_self_forwarded(&self) -> Self {
        Self {
            value: self.value | Self::SELF_FWD_MASK_IN_PLACE,
        }
    }

    #[inline]
    pub fn unset_self_forwarded(&self) -> Self {
        Self {
            value: self.value & !Self::SELF_FWD_MASK_IN_PLACE,
        }
    }

    #[inline]
    pub fn must_be_preserved(&self) -> bool {
        !self.is_unlocked() || !self.has_no_hash()
    }

    // Lock synchronization routines
    #[inline]
    pub fn set_unlocked(&self) -> Self {
        Self {
            value: (self.value & !Self::LOCK_MASK) | Self::UNLOCKED_VALUE,
        }
    }

    #[inline]
    pub fn set_fast_locked(&self) -> Self {
        Self {
            value: self.value & !Self::LOCK_MASK,
        }
    }

    #[inline]
    pub fn is_fast_locked(&self) -> bool {
        (self.value & Self::LOCK_MASK) == Self::LOCKED_VALUE
    }

    #[inline]
    pub fn has_locker(&self) -> bool {
        (self.value & Self::LOCK_MASK) == Self::LOCKED_VALUE
    }

    #[inline]
    pub fn has_monitor(&self) -> bool {
        (self.value & Self::LOCK_MASK) == Self::MONITOR_VALUE
    }

    #[inline]
    pub fn set_has_monitor(&self) -> Self {
        Self {
            value: (self.value & !Self::LOCK_MASK) | Self::MONITOR_VALUE,
        }
    }

    #[inline]
    pub fn clear_lock_bits(&self) -> Self {
        Self {
            value: self.value & !Self::LOCK_MASK,
        }
    }

    #[inline]
    pub fn set_marked(&self) -> Self {
        Self {
            value: (self.value & !Self::LOCK_MASK) | Self::MARKED_VALUE,
        }
    }

    // Age operations
    #[inline]
    pub fn age(&self) -> u32 {
        ((self.value >> Self::AGE_SHIFT) & Self::AGE_MASK) as u32
    }

    #[inline]
    pub fn set_age(&mut self, age: u32) {
        let age_val = (age as usize) & Self::AGE_MASK;
        self.value = (self.value & !Self::AGE_MASK_IN_PLACE) | (age_val << Self::AGE_SHIFT);
    }

    #[inline]
    pub fn incr_age(&mut self) {
        let current = self.age();
        if current < Self::MAX_AGE {
            self.set_age(current + 1);
        }
    }

    // Hash operations
    #[inline]
    pub fn hash(&self) -> usize {
        (self.value >> Self::HASH_SHIFT) & Self::HASH_MASK
    }

    #[inline]
    pub fn has_no_hash(&self) -> bool {
        self.hash() == Self::NO_HASH
    }

    #[inline]
    pub fn set_hash(&mut self, hash: usize) {
        let h = hash & Self::HASH_MASK;
        self.value = (self.value & 0xFF) | (h << Self::HASH_SHIFT);
    }

    #[inline]
    pub fn copy_set_hash(&self, hash: usize) -> Self {
        let mut copy = *self;
        copy.set_hash(hash);
        copy
    }

    // Compact headers: narrow klass pointer stored in upper bits
    #[inline]
    pub fn narrow_klass(&self) -> u32 {
        ((self.value >> Self::KLASS_SHIFT) & Self::KLASS_MASK) as u32
    }

    #[inline]
    pub fn set_narrow_klass(&mut self, narrow_klass: u32) {
        let nk = (narrow_klass as usize) & Self::KLASS_MASK;
        self.value = (self.value & !Self::KLASS_MASK_IN_PLACE) | (nk << Self::KLASS_SHIFT);
    }

    // Pointer encoding / decoding for safe reference tracking
    #[inline]
    pub fn encode_pointer_as_mark(ptr_val: usize) -> Self {
        Self::from_raw(ptr_val).set_marked()
    }

    #[inline]
    pub fn decode_pointer_value(&self) -> usize {
        self.clear_lock_bits().value
    }

    /// Safe text representation of the mark word matching HotSpot markWord::print_on.
    pub fn print_string(&self) -> String {
        if self.is_marked() {
            format!("marked(0x{:x})", self.value)
        } else if self.has_monitor() {
            format!("monitor(0x{:x})", self.value)
        } else if self.is_locked() {
            format!("locked(0x{:x})", self.value)
        } else {
            let hash_str = if self.has_no_hash() {
                "no_hash".to_string()
            } else {
                format!("hash=0x{:x}", self.hash())
            };
            format!("mark(is_unlocked {} age={})", hash_str, self.age())
        }
    }
}

impl Default for MarkWord {
    fn default() -> Self {
        Self::prototype()
    }
}
