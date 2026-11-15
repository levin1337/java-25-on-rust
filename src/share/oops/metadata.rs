/*
 * Port of OpenJDK 25 HotSpot Metadata (share/oops/metadata.hpp, metadata.cpp)
 *
 * Base metadata trait and enumeration for HotSpot Metaspace metadata objects:
 * Klass, Method, ConstMethod, MethodCounters, ConstantPool, ConstantPoolCache.
 *
 * Also contains the Rust-side structural analogue of Metaspace: fixed-size
 * virtual-space chunks with a free-list allocator and per-type allocation
 * statistics (`Metaspace`, `MetaChunk`, `MetaspaceStats`).  Instead of
 * reserving address space, chunks hand out block indices; the accounting
 * (committed/reserved bytes, free blocks, per-type totals) mirrors what
 * `Metaspace::report_metadata_type` and `ChunkManager` track.
 */

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataType {
    Klass,
    Method,
    ConstMethod,
    MethodData,
    ConstantPool,
    ConstantPoolCache,
    MethodCounters,
    RecordComponent,
}

impl MetadataType {
    /// All metadata types in declaration order (matches the
    /// `MetaspaceObj::Type` numbering used by `Metaspace::report_*`).
    pub const ALL: [MetadataType; 8] = [
        MetadataType::Klass,
        MetadataType::Method,
        MetadataType::ConstMethod,
        MetadataType::MethodData,
        MetadataType::ConstantPool,
        MetadataType::ConstantPoolCache,
        MetadataType::MethodCounters,
        MetadataType::RecordComponent,
    ];

    #[inline]
    pub fn index(self) -> usize {
        match self {
            MetadataType::Klass => 0,
            MetadataType::Method => 1,
            MetadataType::ConstMethod => 2,
            MetadataType::MethodData => 3,
            MetadataType::ConstantPool => 4,
            MetadataType::ConstantPoolCache => 5,
            MetadataType::MethodCounters => 6,
            MetadataType::RecordComponent => 7,
        }
    }

    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            MetadataType::Klass => "Klass",
            MetadataType::Method => "Method",
            MetadataType::ConstMethod => "ConstMethod",
            MetadataType::MethodData => "MethodData",
            MetadataType::ConstantPool => "ConstantPool",
            MetadataType::ConstantPoolCache => "ConstantPoolCache",
            MetadataType::MethodCounters => "MethodCounters",
            MetadataType::RecordComponent => "RecordComponent",
        }
    }
}

pub trait Metadata {
    fn metadata_type(&self) -> MetadataType;
    fn size_in_words(&self) -> usize;
    fn internal_name(&self) -> &str;

    fn is_klass(&self) -> bool {
        self.metadata_type() == MetadataType::Klass
    }

    fn is_method(&self) -> bool {
        self.metadata_type() == MetadataType::Method
    }

    fn is_constant_pool(&self) -> bool {
        self.metadata_type() == MetadataType::ConstantPool
    }

    fn is_method_counters(&self) -> bool {
        self.metadata_type() == MetadataType::MethodCounters
    }
}

// ---------------------------------------------------------------------------
// Metaspace analogue: chunked metadata allocator with statistics
// ---------------------------------------------------------------------------

/// One virtual-space chunk of metaspace.  Blocks are addressed by their
/// offset within the chunk; `free_blocks` keeps `(offset, size)` pairs
/// sorted by offset for first-fit allocation and coalescing.
#[derive(Debug, Clone)]
pub struct MetaChunk {
    capacity: usize,
    used_bytes: usize,
    free_blocks: BTreeMap<usize, usize>,
}

impl MetaChunk {
    pub fn new(capacity: usize) -> Self {
        let mut free_blocks = BTreeMap::new();
        free_blocks.insert(0, capacity);
        Self {
            capacity,
            used_bytes: 0,
            free_blocks,
        }
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    #[inline]
    pub fn free_bytes(&self) -> usize {
        self.free_blocks.values().sum()
    }

    #[inline]
    pub fn num_free_blocks(&self) -> usize {
        self.free_blocks.len()
    }

    /// First-fit allocation honoring `align` (a power of two).  Returns
    /// the block offset inside the chunk, or `None` when no free block is
    /// large enough.
    pub fn alloc(&mut self, size: usize, align: usize) -> Option<usize> {
        debug_assert!(align.is_power_of_two());
        for (&offset, &block_size) in self.free_blocks.iter() {
            let aligned = (offset + align - 1) & !(align - 1);
            let padding = aligned - offset;
            if block_size >= padding + size {
                if padding == 0 {
                    self.free_blocks.remove(&offset);
                } else {
                    self.free_blocks.insert(offset, padding);
                }
                let block_offset = aligned;
                let tail = block_offset + size;
                let tail_size = block_size - padding - size;
                if tail_size > 0 {
                    self.free_blocks.insert(tail, tail_size);
                }
                self.used_bytes += padding + size;
                return Some(block_offset);
            }
        }
        None
    }

    /// Returns a previously allocated block; adjacent free blocks are
    /// coalesced (mirrors `VirtualSpaceNode` free-list merging).
    pub fn free(&mut self, offset: usize, size: usize) {
        debug_assert!(offset + size <= self.capacity);
        self.used_bytes = self.used_bytes.saturating_sub(size);
        self.free_blocks.insert(offset, size);
        // Merge with the previous block if adjacent.
        let mut start = offset;
        let mut end = offset + size;
        if let Some((&prev_off, &prev_size)) = self
            .free_blocks
            .range(..offset)
            .next_back()
            .filter(|(&prev_off, &prev_size)| prev_off + prev_size == offset)
        {
            self.free_blocks.remove(&prev_off);
            start = prev_off;
            end = prev_off + prev_size + size;
        }
        // Merge with the next block if adjacent.
        if let Some((&next_off, &next_size)) = self
            .free_blocks
            .range((std::ops::Bound::Excluded(end), std::ops::Bound::Unbounded))
            .next()
            .filter(|(&next_off, _)| end == next_off)
        {
            self.free_blocks.remove(&next_off);
            end = next_off + next_size;
        }
        self.free_blocks.remove(&offset);
        self.free_blocks.insert(start, end - start);
    }

    /// Whether the chunk has no allocated blocks.
    pub fn is_empty(&self) -> bool {
        self.used_bytes == 0
    }
}

/// Per-type allocation totals, matching the counters
/// `Metaspace::report_metadata_type` prints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeStats {
    pub num_allocated: u64,
    pub allocated_bytes: u64,
    pub capacity_bytes: u64,
}

/// Aggregate metaspace statistics (`Metaspace::print_usage` /
/// `chunk_manager()->statistics()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetaspaceStats {
    pub capacity_bytes: u64,
    pub committed_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub num_chunks: u64,
    pub num_free_blocks: u64,
    pub by_type: [TypeStats; 8],
}

impl MetaspaceStats {
    #[inline]
    pub fn is_exhausted(&self) -> bool {
        self.free_bytes == 0
    }

    /// Percentage of committed space in use (0..=100).
    #[inline]
    pub fn used_percent(&self) -> u64 {
        if self.committed_bytes == 0 {
            0
        } else {
            self.used_bytes.saturating_mul(100) / self.committed_bytes
        }
    }
}

/// A handle to an allocated metadata block inside a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataBlock {
    pub chunk_index: usize,
    pub offset: usize,
    pub size: usize,
}

/// The metaspace analogue: a set of chunks plus allocation accounting
/// per metadata type.  Allocation never fails for metadata objects that
/// fit a chunk; `alloc` returns `None` only when every chunk is full
/// (callers should treat this as `OutOfMetaspace` / class unloading).
#[derive(Debug)]
pub struct Metaspace {
    chunks: Vec<MetaChunk>,
    by_type: [TypeStats; 8],
    next_chunk_size: usize,
}

impl Metaspace {
    /// Default chunk size, mirroring `ChunkSize` growth for metadata
    /// (256KB initial virtual-space node).
    pub const DEFAULT_CHUNK_SIZE: usize = 256 * 1024;

    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            by_type: [TypeStats::default(); 8],
            next_chunk_size: Self::DEFAULT_CHUNK_SIZE,
        }
    }

    /// Metaspace with a single preallocated chunk.
    pub fn with_chunk(chunk_size: usize) -> Self {
        let mut space = Self::new();
        space.grow(chunk_size);
        space
    }

    /// Adds a new chunk (`VirtualSpaceNode` analogue).  Returns its index.
    pub fn grow(&mut self, size: usize) -> usize {
        let index = self.chunks.len();
        self.chunks.push(MetaChunk::new(size));
        self.next_chunk_size = size.saturating_mul(2).max(Self::DEFAULT_CHUNK_SIZE);
        index
    }

    #[inline]
    pub fn num_chunks(&self) -> usize {
        self.chunks.len()
    }

    #[inline]
    pub fn chunk(&self, index: usize) -> Option<&MetaChunk> {
        self.chunks.get(index)
    }

    /// Allocates `size` bytes aligned to `align` for `ty`, searching all
    /// chunks first-fit and growing a fresh chunk when needed.
    pub fn alloc(&mut self, ty: MetadataType, size: usize, align: usize) -> Option<MetadataBlock> {
        let align = align.max(8);
        // Try existing chunks in order.
        for (index, chunk) in self.chunks.iter_mut().enumerate() {
            if let Some(offset) = chunk.alloc(size, align) {
                self.by_type[ty.index()].num_allocated += 1;
                self.by_type[ty.index()].allocated_bytes += size as u64;
                self.by_type[ty.index()].capacity_bytes += size as u64;
                return Some(MetadataBlock {
                    chunk_index: index,
                    offset,
                    size,
                });
            }
        }
        // Grow and retry once.
        let index = self.grow(self.next_chunk_size);
        let offset = self.chunks[index].alloc(size, align)?;
        self.by_type[ty.index()].num_allocated += 1;
        self.by_type[ty.index()].allocated_bytes += size as u64;
        self.by_type[ty.index()].capacity_bytes += size as u64;
        Some(MetadataBlock {
            chunk_index: index,
            offset,
            size,
        })
    }

    /// Frees a block allocated by [`Self::alloc`].
    pub fn free(&mut self, ty: MetadataType, block: MetadataBlock) {
        if let Some(chunk) = self.chunks.get_mut(block.chunk_index) {
            chunk.free(block.offset, block.size);
        }
        let stats = &mut self.by_type[ty.index()];
        stats.num_allocated = stats.num_allocated.saturating_sub(1);
        stats.allocated_bytes = stats.allocated_bytes.saturating_sub(block.size as u64);
    }

    /// Aggregate statistics across all chunks (`Metaspace::statistics`).
    pub fn stats(&self) -> MetaspaceStats {
        let mut stats = MetaspaceStats {
            num_chunks: self.chunks.len() as u64,
            by_type: self.by_type,
            ..MetaspaceStats::default()
        };
        for chunk in &self.chunks {
            stats.capacity_bytes += chunk.capacity() as u64;
            stats.used_bytes += chunk.used_bytes() as u64;
            stats.free_bytes += chunk.free_bytes() as u64;
            stats.num_free_blocks += chunk.num_free_blocks() as u64;
        }
        stats.committed_bytes = stats.capacity_bytes;
        stats
    }

    /// Per-type statistics (`Metaspace::report_metadata_type`).
    pub fn type_stats(&self, ty: MetadataType) -> TypeStats {
        self.by_type[ty.index()]
    }
}

impl Default for Metaspace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_alloc_and_free_coalesces() {
        let mut chunk = MetaChunk::new(64);
        let a = chunk.alloc(16, 8).unwrap();
        let b = chunk.alloc(16, 8).unwrap();
        assert_eq!(chunk.used_bytes(), 32);
        assert_eq!(chunk.free_bytes(), 32);
        chunk.free(a, 16);
        chunk.free(b, 16);
        assert_eq!(chunk.num_free_blocks(), 1);
        assert_eq!(chunk.free_bytes(), 64);
        assert!(chunk.is_empty());
    }

    #[test]
    fn metaspace_alloc_tracks_type_stats() {
        let mut space = Metaspace::with_chunk(256);
        let block = space
            .alloc(MetadataType::Klass, 48, 8)
            .expect("first alloc fits");
        let stats = space.stats();
        assert_eq!(stats.used_bytes, 48);
        assert_eq!(stats.num_chunks, 1);
        assert_eq!(space.type_stats(MetadataType::Klass).num_allocated, 1);
        space.free(MetadataType::Klass, block);
        assert_eq!(space.stats().used_bytes, 0);
        assert_eq!(space.type_stats(MetadataType::Klass).allocated_bytes, 0);
    }

    #[test]
    fn metaspace_grows_when_full() {
        let mut space = Metaspace::new();
        space.grow(64);
        // Fill the chunk completely.
        let _b1 = space.alloc(MetadataType::Method, 32, 8).unwrap();
        let _b2 = space.alloc(MetadataType::Method, 32, 8).unwrap();
        // Third allocation grows a new chunk.
        let _b3 = space.alloc(MetadataType::Method, 16, 8).unwrap();
        assert_eq!(space.num_chunks(), 2);
        assert_eq!(space.type_stats(MetadataType::Method).num_allocated, 3);
    }
}
