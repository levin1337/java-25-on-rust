/*
 * Port of OpenJDK 25 HotSpot Shenandoah Forwarding
 * (share/gc/shenandoah/shenandoahForwarding.hpp / shenandoahForwarding.inline.hpp,
 *  on top of share/oops/markWord.hpp)
 *
 * During evacuation the mark word of a from-space object is turned into
 * a forwarding pointer: markWord::encode_pointer_as_mark stores the
 * to-space address with the marked lock pattern (0b11) in the low bits.
 */

use std::sync::atomic::{AtomicU64, Ordering};

pub const LOCK_BITS: u32 = 2;
pub const LOCK_SHIFT: u32 = 0;
pub const SELF_FWD_SHIFT: u32 = LOCK_SHIFT + LOCK_BITS;
pub const AGE_SHIFT: u32 = SELF_FWD_SHIFT + 1;
pub const HASH_SHIFT: u32 = AGE_SHIFT + 4 + 4;
pub const LOCK_MASK: u64 = (1u64 << LOCK_BITS) - 1;
pub const SELF_FWD_MASK: u64 = 1u64 << SELF_FWD_SHIFT;
pub const AGE_MASK: u64 = 0xF;
pub const LOCKED_VALUE: u64 = 0;
pub const UNLOCKED_VALUE: u64 = 1;
pub const MONITOR_VALUE: u64 = 2;
pub const MARKED_VALUE: u64 = 3;
pub const MAX_AGE: u64 = AGE_MASK;
pub const POINTER_MASK: u64 = !LOCK_MASK;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkWord {
    value: u64,
}

impl MarkWord {
    pub fn new(value: u64) -> Self {
        Self { value }
    }

    pub fn from_pointer(ptr: u64) -> Self {
        Self { value: ptr }
    }

    pub fn zero() -> Self {
        Self { value: 0 }
    }

    #[inline]
    pub fn value(&self) -> u64 {
        self.value
    }

    #[inline]
    fn lock_bits(&self) -> u64 {
        self.value & LOCK_MASK
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.lock_bits() != UNLOCKED_VALUE
    }

    #[inline]
    pub fn is_unlocked(&self) -> bool {
        self.lock_bits() == UNLOCKED_VALUE
    }

    #[inline]
    pub fn is_marked(&self) -> bool {
        self.lock_bits() == MARKED_VALUE
    }

    #[inline]
    pub fn is_forwarded(&self) -> bool {
        (self.value & (LOCK_MASK | SELF_FWD_MASK)) >= MARKED_VALUE
    }

    #[inline]
    pub fn is_neutral(&self) -> bool {
        self.lock_bits() == UNLOCKED_VALUE
    }

    #[inline]
    pub fn is_being_inflated(&self) -> bool {
        self.value == 0
    }

    #[inline]
    pub fn age(&self) -> u64 {
        (self.value >> AGE_SHIFT) & AGE_MASK
    }

    #[inline]
    pub fn set_age(&self, age: u64) -> Self {
        Self {
            value: (self.value & !(AGE_MASK << AGE_SHIFT)) | ((age & AGE_MASK) << AGE_SHIFT),
        }
    }

    #[inline]
    pub fn has_hash(&self) -> bool {
        self.hash() != 0
    }

    #[inline]
    pub fn hash(&self) -> u64 {
        (self.value >> HASH_SHIFT) & ((1u64 << 31) - 1)
    }

    #[inline]
    pub fn set_marked(&self) -> Self {
        Self {
            value: (self.value & !LOCK_MASK) | MARKED_VALUE,
        }
    }

    #[inline]
    pub fn set_unmarked(&self) -> Self {
        Self {
            value: (self.value & !LOCK_MASK) | UNLOCKED_VALUE,
        }
    }

    #[inline]
    pub fn set_unlocked(&self) -> Self {
        self.set_unmarked()
    }

    #[inline]
    pub fn clear_lock_bits(&self) -> Self {
        Self {
            value: self.value & !LOCK_MASK,
        }
    }

    #[inline]
    pub fn to_pointer(&self) -> u64 {
        self.value
    }

    #[inline]
    pub fn decode_pointer(&self) -> u64 {
        self.clear_lock_bits().value
    }

    #[inline]
    pub fn encode_pointer_as_mark(ptr: u64) -> Self {
        Self::from_pointer(ptr).set_marked()
    }
}

#[inline]
fn mask_bits(value: u64, mask: u64) -> u64 {
    value & mask
}

pub struct ShenandoahForwarding;

impl ShenandoahForwarding {
    #[inline]
    pub fn get_forwardee_raw_unchecked(mark: MarkWord, self_addr: u64) -> u64 {
        if mark.is_marked() {
            let fwdptr = mark.clear_lock_bits().to_pointer();
            if fwdptr != 0 {
                return fwdptr;
            }
        }
        self_addr
    }

    #[inline]
    pub fn get_forwardee_raw(mark: MarkWord, self_addr: u64) -> u64 {
        debug_assert!(self_addr != 0);
        Self::get_forwardee_raw_unchecked(mark, self_addr)
    }

    #[inline]
    pub fn get_forwardee_mutator(mark: MarkWord, self_addr: u64) -> u64 {
        debug_assert!(self_addr != 0);
        if mark.is_marked() {
            let fwdptr = mark.clear_lock_bits().to_pointer();
            debug_assert!(fwdptr != 0, "forwarding pointer is never null here");
            fwdptr
        } else {
            self_addr
        }
    }

    #[inline]
    pub fn get_forwardee(mark: MarkWord, self_addr: u64) -> u64 {
        debug_assert!(self_addr != 0);
        Self::get_forwardee_raw_unchecked(mark, self_addr)
    }

    #[inline]
    pub fn is_forwarded(mark: MarkWord) -> bool {
        mark.is_marked()
    }

    #[inline]
    pub fn try_update_forwardee(header: &AtomicU64, self_addr: u64, update: u64) -> u64 {
        debug_assert!(self_addr != 0);
        debug_assert!(update != self_addr);
        let old_mark = MarkWord::new(header.load(Ordering::Acquire));
        if old_mark.is_marked() {
            return old_mark.clear_lock_bits().to_pointer();
        }
        let new_mark = MarkWord::encode_pointer_as_mark(update);
        match header.compare_exchange(
            old_mark.value(),
            new_mark.value(),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => update,
            Err(prev) => MarkWord::new(prev).clear_lock_bits().to_pointer(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_word_forwarding() {
        let obj = 0x7f00_0000_1000u64;
        let fwd = 0x7f00_0000_8000u64;
        let header = AtomicU64::new(MarkWord::zero().value());

        assert!(!ShenandoahForwarding::is_forwarded(MarkWord::zero()));
        let result = ShenandoahForwarding::try_update_forwardee(&header, obj, fwd);
        assert_eq!(result, fwd);

        let mark = MarkWord::new(header.load(Ordering::Relaxed));
        assert!(mark.is_marked());
        assert!(ShenandoahForwarding::is_forwarded(mark));
        assert_eq!(ShenandoahForwarding::get_forwardee(mark, obj), fwd);
        assert_eq!(ShenandoahForwarding::get_forwardee_mutator(mark, obj), fwd);
        assert_eq!(ShenandoahForwarding::get_forwardee_raw(mark, obj), fwd);

        let other = 0x7f00_0000_9000u64;
        let result2 = ShenandoahForwarding::try_update_forwardee(&header, obj, other);
        assert_eq!(result2, fwd);
        assert_eq!(
            MarkWord::new(header.load(Ordering::Relaxed)).decode_pointer(),
            fwd
        );
    }

    #[test]
    fn unmarked_returns_self() {
        let obj = 0x7f00_0000_1000u64;
        let header = AtomicU64::new(MarkWord::zero().set_unlocked().value());
        let mark = MarkWord::new(header.load(Ordering::Relaxed));
        assert_eq!(ShenandoahForwarding::get_forwardee(mark, obj), obj);
        assert_eq!(ShenandoahForwarding::get_forwardee_mutator(mark, obj), obj);
        assert_eq!(ShenandoahForwarding::get_forwardee_raw(mark, obj), obj);
        assert!(!ShenandoahForwarding::is_forwarded(mark));
    }

    #[test]
    fn age_bits() {
        let m = MarkWord::zero().set_age(7);
        assert_eq!(m.age(), 7);
        assert_eq!(m.set_age(20).age(), 4);
    }
}
