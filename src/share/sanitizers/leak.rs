use std::ffi::c_void;

#[inline(always)]
pub unsafe fn lsan_register_root_region(_addr: *const c_void, _size: usize) {}

#[inline(always)]
pub unsafe fn lsan_unregister_root_region(_addr: *const c_void, _size: usize) {}

#[inline(always)]
pub unsafe fn lsan_ignore_object(_object: *const c_void) {}

#[inline(always)]
pub fn lsan_do_leak_check() {}

#[inline(always)]
pub fn lsan_do_recoverable_leak_check() -> i32 {
    0
}
