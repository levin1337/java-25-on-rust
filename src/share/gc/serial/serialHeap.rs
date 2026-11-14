use super::defNewGeneration::DefNewGeneration;
use super::tenuredGeneration::TenuredGeneration;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io;

pub struct SerialHeap {
    pub young: DefNewGeneration,
    pub old: TenuredGeneration,
    pub young_gc_count: usize,
    pub full_gc_count: usize,
}

impl SerialHeap {
    pub fn new(young_size: usize, old_size: usize) -> Self {
        Self {
            young: DefNewGeneration::new(young_size * 8 / 10, young_size / 10),
            old: TenuredGeneration::new(old_size),
            young_gc_count: 0,
            full_gc_count: 0,
        }
    }

    pub fn do_full_gc(&mut self) {
        self.full_gc_count += 1;
        self.young.reset_young();
    }

    pub fn do_young_gc(&mut self) {
        self.young_gc_count += 1;
        self.young.reset_young();
    }
}

impl CollectedHeap for SerialHeap {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Serial
    }

    fn capacity(&self) -> usize {
        self.young.capacity() + self.old.capacity()
    }

    fn used(&self) -> usize {
        self.young.used() + self.old.used()
    }

    fn max_capacity(&self) -> usize {
        self.capacity()
    }

    fn collect(&mut self, cause: GCCause) -> io::Result<()> {
        match cause {
            GCCause::JavaLangSystemGC | GCCause::FullGCAlot => self.do_full_gc(),
            _ => self.do_young_gc(),
        }
        Ok(())
    }
}
