/*
 * Port of OpenJDK 25 HotSpot ZGC Page (share/gc/z/zPage.hpp / zPage.cpp)
 *
 * Represents a virtual memory page in ZGC heap: Small (2MB), Medium (32MB), or Large.
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZPageType {
    Small,
    Medium,
    Large,
}

#[derive(Clone, Debug)]
pub struct ZPage {
    pub page_type: ZPageType,
    pub start: usize,
    pub size: usize,
    pub top: usize,
    pub live_objects: usize,
    pub in_relocation_set: bool,
}

impl ZPage {
    pub const SMALL_SIZE: usize = 2 * 1024 * 1024; // 2MB
    pub const MEDIUM_SIZE: usize = 32 * 1024 * 1024; // 32MB

    pub fn new(page_type: ZPageType, start: usize, size: usize) -> Self {
        Self {
            page_type,
            start,
            size,
            top: start,
            live_objects: 0,
            in_relocation_set: false,
        }
    }

    pub fn new_small(start: usize) -> Self {
        Self::new(ZPageType::Small, start, Self::SMALL_SIZE)
    }

    pub fn new_medium(start: usize) -> Self {
        Self::new(ZPageType::Medium, start, Self::MEDIUM_SIZE)
    }

    pub fn new_large(start: usize, size: usize) -> Self {
        Self::new(ZPageType::Large, start, size)
    }

    pub fn allocate(&mut self, size: usize) -> Option<usize> {
        let aligned_size = (size + 7) & !7;
        if self.top + aligned_size <= self.start + self.size {
            let addr = self.top;
            self.top += aligned_size;
            self.live_objects += 1;
            Some(addr)
        } else {
            None
        }
    }

    pub fn used(&self) -> usize {
        self.top - self.start
    }

    pub fn free(&self) -> usize {
        self.size - self.used()
    }

    pub fn reset(&mut self) {
        self.top = self.start;
        self.live_objects = 0;
        self.in_relocation_set = false;
    }
}
