/*
 * Port of OpenJDK 25 HotSpot ParallelScavengeHeap (share/gc/parallel/parallelScavengeHeap.hpp)
 *
 * Implements Parallel GC (Throughput Collector) with Parallel Scavenge Young
 * and Parallel Old Tenured generations.
 */

use super::psOldGen::PSOldGen;
use super::psYoungGen::PSYoungGen;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io::{self, Error};

pub struct ParallelScavengeHeap {
    young_gen: PSYoungGen,
    old_gen: PSOldGen,
    scavenge_count: usize,
    full_gc_count: usize,
}

impl ParallelScavengeHeap {
    pub fn new(young_size: usize, old_size: usize) -> Self {
        Self {
            young_gen: PSYoungGen::new(0, young_size),
            old_gen: PSOldGen::new(young_size, old_size),
            scavenge_count: 0,
            full_gc_count: 0,
        }
    }

    #[inline]
    pub fn young_gen(&self) -> &PSYoungGen {
        &self.young_gen
    }

    #[inline]
    pub fn old_gen(&self) -> &PSOldGen {
        &self.old_gen
    }

    #[inline]
    pub fn scavenge_count(&self) -> usize {
        self.scavenge_count
    }

    #[inline]
    pub fn full_gc_count(&self) -> usize {
        self.full_gc_count
    }

    pub fn allocate(&mut self, size: usize) -> io::Result<usize> {
        // Try Young generation
        if let Some(addr) = self.young_gen.allocate(size) {
            return Ok(addr);
        }

        // Young space full: invoke young scavenge
        self.do_scavenge();

        if let Some(addr) = self.young_gen.allocate(size) {
            return Ok(addr);
        }

        // Try allocating in Old generation
        if let Some(addr) = self.old_gen.allocate(size) {
            return Ok(addr);
        }

        // Trigger Full GC
        self.do_full_gc();

        if let Some(addr) = self.young_gen.allocate(size) {
            Ok(addr)
        } else if let Some(addr) = self.old_gen.allocate(size) {
            Ok(addr)
        } else {
            Err(Error::other(
                "OutOfMemoryError: Java heap space (Parallel GC)",
            ))
        }
    }

    pub fn do_scavenge(&mut self) {
        self.scavenge_count += 1;
        self.young_gen.reset_young();
    }

    pub fn do_full_gc(&mut self) {
        self.full_gc_count += 1;
        self.young_gen.reset_young();
        self.old_gen.clear();
    }
}

impl CollectedHeap for ParallelScavengeHeap {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Parallel
    }

    fn capacity(&self) -> usize {
        self.young_gen.capacity() + self.old_gen.capacity()
    }

    fn used(&self) -> usize {
        self.young_gen.used() + self.old_gen.used()
    }

    fn max_capacity(&self) -> usize {
        self.capacity()
    }

    fn collect(&mut self, cause: GCCause) -> io::Result<()> {
        match cause {
            GCCause::JavaLangSystemGC | GCCause::FullGCAlot => {
                self.do_full_gc();
            }
            _ => {
                self.do_scavenge();
            }
        }
        Ok(())
    }
}
