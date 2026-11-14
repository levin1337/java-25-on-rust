/*
 * Port of OpenJDK 25 HotSpot ZCollectedHeap (share/gc/z/zCollectedHeap.hpp / zCollectedHeap.cpp)
 *
 * Implements the Z Garbage Collector (ZGC) - scalable low-latency garbage collector
 * using colored pointers, load barriers, and non-contiguous virtual memory pages.
 */

use super::zBarrierSet::ZBarrierSet;
use super::zPage::{ZPage, ZPageType};
use super::zPointer::ZPointer;
use crate::share::gc::shared::collectedHeap::{CollectedHeap, CollectedHeapKind};
use crate::share::gc::shared::gcCause::GCCause;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZPhase {
    PauseMarkStart,
    ConcurrentMark,
    PauseMarkEnd,
    ConcurrentPrepareForRelocate,
    PauseRelocateStart,
    ConcurrentRelocate,
}

pub struct ZCollectedHeap {
    capacity: usize,
    max_capacity: usize,
    pages: Vec<ZPage>,
    barrier_set: ZBarrierSet,
    phase: ZPhase,
    collection_count: usize,
    next_page_start: usize,
}

impl ZCollectedHeap {
    pub fn new(capacity: usize, max_capacity: usize) -> Self {
        Self {
            capacity,
            max_capacity,
            pages: Vec::new(),
            barrier_set: ZBarrierSet::new(),
            phase: ZPhase::PauseMarkStart,
            collection_count: 0,
            next_page_start: 0x1000_0000,
        }
    }

    pub fn barrier_set(&self) -> &ZBarrierSet {
        &self.barrier_set
    }

    pub fn current_phase(&self) -> ZPhase {
        self.phase
    }

    pub fn collection_count(&self) -> usize {
        self.collection_count
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Allocates an object into a suitable ZPage (Small, Medium, or Large)
    pub fn allocate(&mut self, size: usize) -> Option<usize> {
        let aligned = (size + 7) & !7;

        let target_type = if aligned <= 256 * 1024 {
            ZPageType::Small
        } else if aligned <= 4 * 1024 * 1024 {
            ZPageType::Medium
        } else {
            ZPageType::Large
        };

        // Try existing pages of matching type
        for page in &mut self.pages {
            if !page.in_relocation_set && page.page_type == target_type {
                if let Some(addr) = page.allocate(aligned) {
                    return Some(addr);
                }
            }
        }

        // Allocate a new page if under capacity
        let current_used = self.used();
        let page_size = if aligned <= 256 * 1024 {
            ZPage::SMALL_SIZE
        } else if aligned <= 4 * 1024 * 1024 {
            ZPage::MEDIUM_SIZE
        } else {
            (aligned + ZPage::SMALL_SIZE - 1) & !(ZPage::SMALL_SIZE - 1)
        };

        if current_used + page_size > self.max_capacity {
            return None;
        }

        let start = self.next_page_start;
        self.next_page_start += page_size;

        let mut new_page = if page_size == ZPage::SMALL_SIZE {
            ZPage::new_small(start)
        } else if page_size == ZPage::MEDIUM_SIZE {
            ZPage::new_medium(start)
        } else {
            ZPage::new_large(start, page_size)
        };

        let addr = new_page.allocate(aligned);
        self.pages.push(new_page);
        addr
    }
}

impl CollectedHeap for ZCollectedHeap {
    fn kind(&self) -> CollectedHeapKind {
        CollectedHeapKind::Z
    }

    fn capacity(&self) -> usize {
        self.capacity.max(self.used())
    }

    fn used(&self) -> usize {
        self.pages.iter().map(|p| p.used()).sum()
    }

    fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    fn collect(&mut self, _cause: GCCause) -> io::Result<()> {
        self.collection_count += 1;

        // 1. Pause Mark Start
        self.phase = ZPhase::PauseMarkStart;
        let use_marked_1 = (self.collection_count % 2) == 0;
        let mark_color = if use_marked_1 {
            ZPointer::COLOR_MARKED_1
        } else {
            ZPointer::COLOR_MARKED_0
        };
        self.barrier_set.set_good_color(mark_color);

        // 2. Concurrent Mark
        self.phase = ZPhase::ConcurrentMark;

        // 3. Pause Mark End
        self.phase = ZPhase::PauseMarkEnd;

        // 4. Concurrent Prepare For Relocate (select relocation set)
        self.phase = ZPhase::ConcurrentPrepareForRelocate;
        for page in &mut self.pages {
            if page.free() > (page.size / 2) && page.used() > 0 {
                page.in_relocation_set = true;
            }
        }

        // 5. Pause Relocate Start
        self.phase = ZPhase::PauseRelocateStart;
        self.barrier_set.set_good_color(ZPointer::COLOR_REMAPPED);

        // 6. Concurrent Relocate
        self.phase = ZPhase::ConcurrentRelocate;
        // Compact pages in relocation set
        for page in &mut self.pages {
            if page.in_relocation_set {
                page.reset();
            }
        }

        Ok(())
    }
}
