use crate::share::gc::shared::gcCause::GCCause;
use std::io::{self, Error};

pub struct MemAllocator;

impl MemAllocator {
    pub fn allocate_or_retry<F, R>(
        mut alloc: F,
        mut gc: impl FnMut(GCCause) -> io::Result<()>,
    ) -> io::Result<R>
    where
        F: FnMut() -> Option<R>,
    {
        if let Some(res) = alloc() {
            return Ok(res);
        }
        gc(GCCause::AllocationFailure)?;
        alloc().ok_or_else(|| Error::other("OutOfMemoryError: Java heap space"))
    }
}
