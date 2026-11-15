/*
 * Port of OpenJDK 25 HotSpot G1OopStarChunkedList
 * (share/gc/g1/g1OopStarChunkedList.hpp) — the chunked, overflowable
 * work list used by parallel marking and evacuation.
 *
 * In HotSpot the list is a linked list of fixed-size chunks so that
 * pushing is allocation-free in the common case and iteration order
 * approximates DFS. The Rust port uses a `Vec<Vec<T>>` chunk stack
 * with the same LIFO-within-chunk semantics.
 */

/// `G1OopStarChunkedList`: chunked LIFO work list.
#[derive(Debug)]
pub struct G1OopStarChunkedList<T> {
    /// Chunks of `chunk_size` elements; the last chunk may be partial.
    chunks: Vec<Vec<T>>,
    chunk_size: usize,
    /// Total elements across all chunks.
    len: usize,
}

impl<T> G1OopStarChunkedList<T> {
    /// `chunk_size` mirrors `OopChunkedList::OOM_ELEMENT_SIZE`-style
    /// ergonomics; HotSpot uses 64-oop chunks for scan stacks.
    pub fn new(chunk_size: usize) -> Self {
        Self { chunks: Vec::new(), chunk_size: chunk_size.max(1), len: 0 }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// `push`: appends an element; allocates a new chunk when the
    /// current one is full.
    pub fn push(&mut self, item: T) {
        if self.chunks.last().map_or(true, |c| c.len() >= self.chunk_size) {
            self.chunks.push(Vec::with_capacity(self.chunk_size));
        }
        self.chunks.last_mut().unwrap().push(item);
        self.len += 1;
    }

    /// `pop`: removes and returns the most recently pushed element.
    pub fn pop(&mut self) -> Option<T> {
        loop {
            let last = self.chunks.last_mut()?;
            if let Some(item) = last.pop() {
                self.len -= 1;
                if last.is_empty() {
                    self.chunks.pop();
                }
                return Some(item);
            }
            self.chunks.pop();
        }
    }

    /// `top`: peek at the next element without removing.
    pub fn top(&self) -> Option<&T> {
        self.chunks.last().and_then(|c| c.last())
    }

    /// Iterate all elements in pop order (LIFO) without removing.
    pub fn iter_rev(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().rev().flat_map(|c| c.iter().rev())
    }

    /// `clear` / `reset`.
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.len = 0;
    }

    /// `size` in elements; `mem_size` approximation in bytes.
    pub fn mem_size(&self) -> usize {
        self.len * std::mem::size_of::<T>()
            + self.chunks.len() * self.chunk_size * std::mem::size_of::<T>()
    }

    /// Drain all elements into `out` in pop order.
    pub fn drain_into<F: FnMut(T)>(&mut self, mut f: F) {
        while let Some(item) = self.pop() {
            f(item);
        }
    }
}

impl<T> Default for G1OopStarChunkedList<T> {
    fn default() -> Self {
        Self::new(64)
    }
}
