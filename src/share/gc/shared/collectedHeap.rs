use super::gcCause::GCCause;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectedHeapKind {
    Epsilon,
    Serial,
    Parallel,
    G1,
    Shenandoah,
    Z,
}

pub trait CollectedHeap {
    fn kind(&self) -> CollectedHeapKind;
    fn capacity(&self) -> usize;
    fn used(&self) -> usize;
    fn max_capacity(&self) -> usize;
    fn collect(&mut self, cause: GCCause) -> io::Result<()>;
}
