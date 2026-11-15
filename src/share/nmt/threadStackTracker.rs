/*
 * Port of OpenJDK 25 HotSpot ThreadStackTracker
 * (share/nmt/threadStackTracker.hpp/.cpp)
 *
 * Counts live thread stacks and registers their reserved regions with
 * the VirtualMemoryTracker under mtThreadStack.
 */

use super::memTag::MemTag;
use super::nativeCallStack::NativeCallStack;
use super::virtualMemoryTracker::VirtualMemoryTracker;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct ThreadStackTracker;

static THREAD_COUNT: AtomicUsize = AtomicUsize::new(0);

impl ThreadStackTracker {
    /// Number of currently tracked thread stacks.
    pub fn thread_count() -> usize {
        THREAD_COUNT.load(Ordering::Relaxed)
    }

    /// Align stack boundaries inward to page granularity (AIX/Alpine have
    /// unaligned stack bounds; the VM tracker needs page-aligned regions).
    fn align_boundaries_inward(base: &mut usize, size: &mut usize) {
        #[cfg(windows)]
        let page = crate::share::platform::windows::OsWindows::vm_page_size();
        #[cfg(not(windows))]
        let page = 4096usize;
        let base_aligned = base.div_ceil(page) * page;
        let size_aligned = (*size / page) * page;
        debug_assert!(size_aligned > 0, "stack size less than a page?");
        *base = base_aligned;
        *size = size_aligned;
    }

    /// Register a new thread stack [base, base+size).
    pub fn new_thread_stack(base: usize, size: usize, stack: &NativeCallStack) {
        debug_assert!(base != 0, "Should have been filtered");
        let mut base = base;
        let mut size = size;
        Self::align_boundaries_inward(&mut base, &mut size);
        VirtualMemoryTracker::global().add_reserved_region(base, size, *stack, MemTag::MtThreadStack);
        THREAD_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    /// Remove a thread stack.
    pub fn delete_thread_stack(base: usize, size: usize) {
        debug_assert!(base != 0, "Should have been filtered");
        let mut base = base;
        let mut size = size;
        Self::align_boundaries_inward(&mut base, &mut size);
        VirtualMemoryTracker::global().remove_released_region(base, size);
        THREAD_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}
