//! Thread-safe bump allocator.
//!
//! `bumpalo::Bump` is `!Sync` because chunk bookkeeping uses `Cell`s.
//! Allocations never move or free existing memory, so serializing every
//! `alloc*` call behind a mutex is sufficient: the returned `&'a mut`
//! references stay valid for `'a` (the `SyncBump` lifetime) because bump
//! chunks are append-only and never relocated.

use bumpalo::Bump;
use std::sync::Mutex;

pub struct SyncBump {
    inner: Mutex<Bump>,
}

// SAFETY: all access to the inner `Bump` goes through `inner.lock()`;
// `SyncBump` itself is only reached via shared references.
unsafe impl Sync for SyncBump {}
unsafe impl Send for SyncBump {}

impl SyncBump {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Bump::new()),
        }
    }

    /// Runs `f` with `&'a Bump` where `'a` is the `SyncBump` borrow.
    ///
    /// # Safety contract
    /// `f` must not retain the `&Bump` beyond the call in a way that
    /// mutates allocator state without going through this mutex — in
    /// practice callers only use `alloc*` methods which we re-expose.
    fn with_bump<'a, R>(&'a self, f: impl FnOnce(&'a Bump) -> R) -> R {
        let guard = self.inner.lock().unwrap();
        // SAFETY: `guard` borrows `self.inner`, which lives as long as
        // `self` ('a). The Bump's address is stable inside the Mutex.
        // The lock is held for the duration of `f`, serializing allocs.
        let bump: &'a Bump = unsafe { std::mem::transmute(&*guard) };
        f(bump)
    }

    pub fn alloc_str<'a>(&'a self, value: &str) -> &'a mut str {
        self.with_bump(|bump| bump.alloc_str(value))
    }

    pub fn alloc_slice_copy<'a>(&'a self, value: &[u8]) -> &'a mut [u8] {
        self.with_bump(|bump| bump.alloc_slice_copy(value))
    }

    pub fn alloc<'a, T>(&'a self, value: T) -> &'a mut T {
        self.with_bump(|bump| bump.alloc(value))
    }

    pub fn alloc_slice_fill_copy<'a, T: Copy>(&'a self, len: usize, value: T) -> &'a mut [T] {
        self.with_bump(|bump| bump.alloc_slice_fill_copy(len, value))
    }

    pub fn alloc_slice_fill_clone<'a, T: Clone>(&'a self, len: usize, value: T) -> &'a mut [T] {
        self.with_bump(|bump| bump.alloc_slice_fill_with(len, |_| value.clone()))
    }

    /// Total bytes currently allocated inside the bump (diagnostics).
    pub fn allocated_bytes(&self) -> usize {
        self.inner.lock().unwrap().allocated_bytes()
    }
}

impl Default for SyncBump {
    fn default() -> Self {
        Self::new()
    }
}
