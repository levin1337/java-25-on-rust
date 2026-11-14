/*
 * Port of OpenJDK 25 HotSpot ContinuationEntry (share/runtime/continuationEntry.hpp)
 *
 * ContinuationEntry marks the boundary on the execution stack between carrier thread frames
 * and virtual thread (continuation) frames, supporting nesting of continuations.
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationEntry {
    entry_sp: usize,
    cookie: u64,
    parent: Option<Box<ContinuationEntry>>,
}

impl ContinuationEntry {
    pub const DEFAULT_COOKIE: u64 = 0xC001_C001_CAFE_BABE;

    pub fn new(entry_sp: usize) -> Self {
        Self {
            entry_sp,
            cookie: Self::DEFAULT_COOKIE,
            parent: None,
        }
    }

    pub fn with_parent(entry_sp: usize, parent: ContinuationEntry) -> Self {
        Self {
            entry_sp,
            cookie: Self::DEFAULT_COOKIE,
            parent: Some(Box::new(parent)),
        }
    }

    #[inline]
    pub fn entry_sp(&self) -> usize {
        self.entry_sp
    }

    #[inline]
    pub fn cookie(&self) -> u64 {
        self.cookie
    }

    #[inline]
    pub fn is_valid(&self) -> bool {
        self.cookie == Self::DEFAULT_COOKIE
    }

    #[inline]
    pub fn parent(&self) -> Option<&ContinuationEntry> {
        self.parent.as_deref()
    }
}
