/*
 * Port of OpenJDK 25 HotSpot WhiteBox API (share/prims/whitebox.hpp / whitebox.cpp)
 *
 * Implements WhiteBox testing interface for GC control and heap diagnostics in safe Rust.
 */

use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io;

pub struct WhiteBox;

impl WhiteBox {
    /// Forces a full GC via System.gc() cause
    pub fn force_gc(heap: &mut impl CollectedHeap) -> io::Result<()> {
        heap.collect(GCCause::JavaLangSystemGC)
    }

    /// Triggers a Full GC
    pub fn full_gc(heap: &mut impl CollectedHeap) -> io::Result<()> {
        heap.collect(GCCause::JavaLangSystemGC)
    }

    /// Triggers a Young GC (Allocation Failure cause)
    pub fn young_gc(heap: &mut impl CollectedHeap) -> io::Result<()> {
        heap.collect(GCCause::AllocationFailure)
    }

    /// Returns a tuple of (used_bytes, capacity_bytes) for the heap
    pub fn get_heap_usage(heap: &impl CollectedHeap) -> (usize, usize) {
        (heap.used(), heap.capacity())
    }

    /// Returns current heap used bytes
    pub fn heap_used(heap: &impl CollectedHeap) -> usize {
        heap.used()
    }

    /// Returns current heap total capacity in bytes
    pub fn heap_capacity(heap: &impl CollectedHeap) -> usize {
        heap.capacity()
    }

    /// Checks if a specified GC algorithm is supported
    pub fn is_gc_supported(kind: CollectedHeapKind) -> bool {
        matches!(
            kind,
            CollectedHeapKind::Serial
                | CollectedHeapKind::Parallel
                | CollectedHeapKind::G1
                | CollectedHeapKind::Epsilon
        )
    }
}
