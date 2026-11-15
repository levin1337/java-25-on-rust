use super::z_address::ZAddress;
use super::z_barrier::ZBarrierSet;
use super::z_heap::ZHeap;
use super::z_statistics::{ZCollectionCause, ZDriver, ZDriverRequest, ZYoungCollectionType};
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io;

pub struct ZCollectedHeap {
    heap: ZHeap,
    barrier_set: ZBarrierSet,
    driver: ZDriver,
}

impl ZCollectedHeap {
    pub fn new(capacity: usize, max_capacity: usize) -> Self {
        let heap = ZHeap::new(max_capacity.max(capacity), 1);
        Self {
            heap,
            barrier_set: ZBarrierSet::noop(),
            driver: ZDriver::default(),
        }
    }

    pub fn with_workers(capacity: usize, max_capacity: usize, nworkers: usize) -> Self {
        let heap = ZHeap::new(max_capacity.max(capacity), nworkers);
        Self {
            heap,
            barrier_set: ZBarrierSet::noop(),
            driver: ZDriver::default(),
        }
    }

    pub fn heap(&self) -> &ZHeap {
        &self.heap
    }

    pub fn heap_mut(&mut self) -> &mut ZHeap {
        &mut self.heap
    }

    pub fn barrier_set(&self) -> &ZBarrierSet {
        &self.barrier_set
    }

    pub fn driver(&self) -> &ZDriver {
        &self.driver
    }

    pub fn allocate(&mut self, size: usize) -> Option<usize> {
        self.heap.allocate(size).map(|address| address as usize)
    }

    pub fn collect_young(&mut self, roots: Vec<ZAddress>) -> bool {
        let request = ZDriverRequest {
            cause: ZCollectionCause::Proactive,
            young_type: ZYoungCollectionType::Minor,
            force_major: false,
        };
        if !self.driver.request_collection(request) {
            return false;
        }
        let result = self.heap.collect_young_auto(roots);
        self.driver.finish_collection(false);
        result
    }

    pub fn collect_old(&mut self, roots: Vec<ZAddress>) -> bool {
        let request = ZDriverRequest {
            cause: ZCollectionCause::MajorGC,
            young_type: ZYoungCollectionType::MajorFullRoots,
            force_major: true,
        };
        if !self.driver.request_collection(request) {
            return false;
        }
        let result = self.heap.collect_old(roots);
        self.driver.finish_collection(true);
        result
    }
}

impl CollectedHeap for ZCollectedHeap {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Z
    }

    fn capacity(&self) -> usize {
        self.heap.max_offset() as usize
    }

    fn used(&self) -> usize {
        self.heap.used()
    }

    fn max_capacity(&self) -> usize {
        self.capacity()
    }

    fn collect(&mut self, cause: GCCause) -> io::Result<()> {
        match cause {
            GCCause::JavaLangSystemGC | GCCause::MetadataAllocationFailure => {
                self.collect_old(Vec::new());
            }
            _ => {
                self.collect_young(Vec::new());
            }
        }
        Ok(())
    }
}
