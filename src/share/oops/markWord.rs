/*
 * Port of OpenJDK 25 HotSpot markWord (share/oops/markWord.hpp, markWord.cpp)
 *
 * The markWord describes the header of an object:
 * - 2 lock bits (locked: 00, unlocked: 01, monitor: 10, marked: 11)
 * - 1 self-forward bit
 * - 4 age bits (0..15)
 * - 4 unused gap bits (reserved for Valhalla)
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

    // Bitfield specifications (markWord.hpp, LP64):
    //   lock:2 | self-fwd:1 | age:4 | unused_gap:4 | hash:31 | klass:22
    pub const LOCK_BITS: usize = 2;
    pub const SELF_FWD_BITS: usize = 1;
    pub const AGE_BITS: usize = 4;
    pub const UNUSED_GAP_BITS: usize = 4;
    pub const HASH_BITS: usize = 31;
    pub const KLASS_BITS: usize = 22;

    pub const LOCK_SHIFT: usize = 0;
    pub const SELF_FWD_SHIFT: usize = Self::LOCK_SHIFT + Self::LOCK_BITS; // 2
    pub const AGE_SHIFT: usize = Self::SELF_FWD_SHIFT + Self::SELF_FWD_BITS; // 3
    pub const HASH_SHIFT: usize = Self::AGE_SHIFT + Self::AGE_BITS + Self::UNUSED_GAP_BITS; // 11
    pub const KLASS_SHIFT: usize = Self::HASH_SHIFT + Self::HASH_BITS; // 42

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

    /// `klass_shift_at_offset`: shift used to extract the narrow klass
    /// from a mark word observed through a 4-byte (`klass_offset_in_bytes`)
    /// window (compact object headers read the upper 32 bits of the
    /// 64-bit mark word).
    pub const KLASS_OFFSET_IN_BYTES: usize = 4;
    pub const KLASS_SHIFT_AT_OFFSET: usize =
        Self::KLASS_SHIFT - Self::KLASS_OFFSET_IN_BYTES * 8;

    pub const NO_HASH: usize = 0;
    pub const NO_HASH_IN_PLACE: usize = Self::NO_HASH << Self::HASH_SHIFT;
    pub const NO_LOCK_IN_PLACE: usize = Self::UNLOCKED_VALUE;

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

    /// HotSpot `is_locked`: true for every non-unlocked lock state
    /// (locked 00, monitor 10, marked 11).  Use `is_fast_locked` /
    /// `has_locker` for the stack-lock-only check.
    #[inline]
    pub fn is_locked(&self) -> bool {
        (self.value & Self::LOCK_MASK) != Self::UNLOCKED
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

    /// Lock synchronization routines
    /// HotSpot `set_unlocked`: ORs the unlocked bits in place (callers
    /// only use it when the lock bits are already `LOCKED`).
    #[inline]
    pub fn set_unlocked(&self) -> Self {
        Self {
            value: self.value | Self::UNLOCKED_VALUE,
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

    #[inline]
    pub fn set_unmarked(&self) -> Self {
        Self {
            value: (self.value & !Self::LOCK_MASK) | Self::UNLOCKED_VALUE,
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
        self.value = (self.value & !Self::HASH_MASK_IN_PLACE)
            | ((hash & Self::HASH_MASK) << Self::HASH_SHIFT);
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
    pub fn set_narrow_klass(&self, narrow_klass: u32) -> Self {
        Self {
            value: (self.value & !Self::KLASS_MASK_IN_PLACE)
                | (((narrow_klass as usize) & Self::KLASS_MASK) << Self::KLASS_SHIFT),
        }
    }

    /// `klass_shift_at_offset` variant: extracts the narrow klass from
    /// the upper 32 bits of the mark word (the window a compact-header
    /// object header exposes).
    #[inline]
    pub fn narrow_klass_from_high_word(high_word: u32) -> u32 {
        let shifted = (high_word as usize) >> Self::KLASS_SHIFT_AT_OFFSET;
        (shifted as u32) & (Self::KLASS_MASK as u32)
    }

    // Pointer encoding / decoding for safe reference tracking
    #[inline]
    pub fn encode_pointer_as_mark(ptr_val: usize) -> Self {
        Self::from_raw(ptr_val).set_marked()
    }

    /// HotSpot `decode_pointer`: recovers the oop address stored in the
    /// mark word (forwarding pointer or monitor pointer) by clearing
    /// the lock bits.
    #[inline]
    pub fn decode_pointer(&self) -> usize {
        self.clear_lock_bits().value
    }

    /// HotSpot `forwardee`: the forwarding target of a forwarded object.
    #[inline]
    pub fn forwardee(&self) -> usize {
        self.decode_pointer()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_positions_match_hotspot_layout() {
        // lock:2 | self-fwd:1 | age:4 | gap:4 | hash:31 | klass:22
        assert_eq!(MarkWord::LOCK_SHIFT, 0);
        assert_eq!(MarkWord::SELF_FWD_SHIFT, 2);
        assert_eq!(MarkWord::AGE_SHIFT, 3);
        assert_eq!(MarkWord::HASH_SHIFT, 11);
        assert_eq!(MarkWord::KLASS_SHIFT, 42);
        assert_eq!(MarkWord::KLASS_SHIFT_AT_OFFSET, 10);
    }

    #[test]
    fn hash_roundtrip_preserves_other_fields() {
        let mut mark = MarkWord::prototype();
        mark.set_age(7);
        mark.set_hash(0x1234_5678);
        assert_eq!(mark.hash(), 0x1234_5678);
        assert_eq!(mark.age(), 7);
        assert!(mark.is_unlocked());
        // Hash occupies bits 11..42; klass bits 42..64 stay clear.
        assert_eq!(mark.narrow_klass(), 0);
    }

    #[test]
    fn lock_states() {
        let unlocked = MarkWord::prototype();
        assert!(unlocked.is_unlocked());
        assert!(!unlocked.is_locked());
        assert!(!unlocked.is_marked());
        let locked = unlocked.set_fast_locked();
        assert!(locked.is_fast_locked());
        assert!(locked.is_locked());
        let marked = unlocked.set_marked();
        assert!(marked.is_marked());
        assert!(marked.is_locked());
        let monitor = unlocked.set_has_monitor();
        assert!(monitor.is_monitored());
        assert!(monitor.is_locked());
        let unmarked = marked.set_unmarked();
        assert!(unmarked.is_unlocked());
    }

    #[test]
    fn narrow_klass_roundtrip() {
        let mark = MarkWord::prototype().set_narrow_klass(0x3ABCD);
        assert_eq!(mark.narrow_klass(), 0x3ABCD);
        // Through the 4-byte high-word window used by compact headers.
        let high = (mark.value >> 32) as u32;
        assert_eq!(MarkWord::narrow_klass_from_high_word(high), 0x3ABCD);
    }

    #[test]
    fn forwarding_pointer_roundtrip() {
        let target = 0x0000_7f00_0010_0020usize;
        let mark = MarkWord::encode_pointer_as_mark(target);
        assert!(mark.is_marked());
        assert_eq!(mark.forwardee(), target);
        assert_eq!(mark.decode_pointer(), target);
    }
}
