use std::ffi::c_void;

pub const ATTRIBUTE_NO_ASAN: &str = "";

pub const fn no_asan_attribute<F>(f: F) -> F {
    f
}

#[inline(always)]
pub unsafe fn asan_poison_memory_region(_addr: *mut c_void, _size: usize) {}

#[inline(always)]
pub unsafe fn asan_unpoison_memory_region(_addr: *mut c_void, _size: usize) {}

#[inline(always)]
pub unsafe fn asan_poison_memory_region_range(_addr: *mut c_void, _size: usize) {
    asan_poison_memory_region(_addr, _size);
}

#[inline(always)]
pub unsafe fn asan_region_is_poisoned(_addr: *const c_void, _size: usize) -> bool {
    false
}
