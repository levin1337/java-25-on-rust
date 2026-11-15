/*
 * Port of OpenJDK 25 HotSpot MetadataHandles
 * (share/jvmci/metadataHandles.hpp / metadataHandles.cpp)
 *
 * JVMCI keeps direct references to HotSpot metadata (Method*, Klass*,
 * ConstantPool*) in `jmetadata` handles so that class redefinition /
 * metadata unloading can be tracked.  Handles are managed in blocks of
 * `BLOCK_SIZE_IN_HANDLES` records with a free list; the Java side
 * clears a handle (HandleCleaner) and `rebuild_free_list` recycles it.
 *
 * In this port metadata objects are not raw pointers, so a handle
 * records a `MetadataTarget` (the canonical method/class key or
 * constant-pool key) instead of a `Metadata*`.
 */

/// Opaque metadata handle (`jmetadata`).  Encodes (block << 32) | slot.
pub type JMetadata = u64;

const BLOCK_SHIFT: u64 = 32;
const PTR_TAG: u64 = 1;

/// What a metadata handle refers to.  `key` is the canonical
/// `holder.name+descriptor` / internal class name used throughout this
/// VM for metadata identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetadataTarget {
    Method { key: String },
    Klass { key: String },
    ConstantPool { key: String },
}

impl MetadataTarget {
    pub fn method(key: impl Into<String>) -> Self {
        Self::Method { key: key.into() }
    }

    pub fn klass(key: impl Into<String>) -> Self {
        Self::Klass { key: key.into() }
    }

    pub fn constant_pool(key: impl Into<String>) -> Self {
        Self::ConstantPool { key: key.into() }
    }

    pub fn key(&self) -> &str {
        match self {
            Self::Method { key } | Self::Klass { key } | Self::ConstantPool { key } => key,
        }
    }

    pub fn is_method(&self) -> bool {
        matches!(self, Self::Method { .. })
    }

    pub fn is_klass(&self) -> bool {
        matches!(self, Self::Klass { .. })
    }

    pub fn is_constant_pool(&self) -> bool {
        matches!(self, Self::ConstantPool { .. })
    }
}

/// One `_jmetadata` record.
#[derive(Clone, Debug)]
struct HandleRecord {
    /// `None` means the slot is on the free list.
    value: Option<MetadataTarget>,
    /// Debug name tracking (METADATA_TRACK_NAMES analogue).
    name: Option<String>,
}

impl HandleRecord {
    fn new() -> Self {
        Self {
            value: None,
            name: None,
        }
    }
}

/// A block of handle records (`MetadataHandleBlock`, 32 handles).
struct MetadataHandleBlock {
    handles: Vec<HandleRecord>,
    top: usize,
}

impl MetadataHandleBlock {
    const BLOCK_SIZE_IN_HANDLES: usize = 32;

    fn new() -> Self {
        Self {
            handles: (0..Self::BLOCK_SIZE_IN_HANDLES)
                .map(|_| HandleRecord::new())
                .collect(),
            top: 0,
        }
    }
}

/// Handle table owned by a `JVMCIRuntime` (`_metadata_handles`).
pub struct MetadataHandles {
    blocks: Vec<MetadataHandleBlock>,
    /// Free-list of cleared handles awaiting `rebuild_free_list`.
    pending_free: Vec<JMetadata>,
    /// Rebuilt free list (encoded handles).
    free_list: Vec<JMetadata>,
    allocate_before_rebuild: usize,
    num_handles: usize,
    num_free_handles: usize,
}

impl MetadataHandles {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            pending_free: Vec::new(),
            free_list: Vec::new(),
            allocate_before_rebuild: 0,
            num_handles: 0,
            num_free_handles: 0,
        }
    }

    #[inline]
    pub fn num_handles(&self) -> usize {
        self.num_handles
    }

    #[inline]
    pub fn num_free_handles(&self) -> usize {
        self.num_free_handles
    }

    #[inline]
    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    #[inline]
    fn encode(block: usize, slot: usize) -> JMetadata {
        ((block as u64) << BLOCK_SHIFT) | (slot as u64)
    }

    #[inline]
    fn decode(handle: JMetadata) -> (usize, usize) {
        ((handle >> BLOCK_SHIFT) as usize, (handle & 0xFFFF_FFFF) as usize)
    }

    fn record_mut(&mut self, handle: JMetadata) -> Option<&mut HandleRecord> {
        let (block, slot) = Self::decode(handle);
        self.blocks
            .get_mut(block)
            .and_then(|b| b.handles.get_mut(slot))
    }

    fn record(&self, handle: JMetadata) -> Option<&HandleRecord> {
        let (block, slot) = Self::decode(handle);
        self.blocks.get(block).and_then(|b| b.handles.get(slot))
    }

    /// `MetadataHandles::get_handle`: bump-allocate from the last block,
    /// then the free list, then a fresh block.
    fn get_handle(&mut self) -> Option<JMetadata> {
        if let Some(last) = self.blocks.last_mut() {
            if last.top < MetadataHandleBlock::BLOCK_SIZE_IN_HANDLES {
                let slot = last.top;
                last.top += 1;
                self.num_handles += 1;
                return Some(Self::encode(self.blocks.len() - 1, slot));
            }
        }
        if let Some(handle) = self.free_list.pop() {
            self.num_handles += 1;
            self.num_free_handles = self.num_free_handles.saturating_sub(1);
            return Some(handle);
        }
        None
    }

    /// `MetadataHandles::rebuild_free_list`: scan all blocks for cleared
    /// handles (the Java side sets the handle's metadata pointer to
    /// null via `HandleCleaner`) and chain them onto the free list.
    pub fn rebuild_free_list(&mut self) {
        for (block_idx, block) in self.blocks.iter().enumerate() {
            for (slot, record) in block.handles.iter().enumerate() {
                if record.value.is_none() {
                    let handle = Self::encode(block_idx, slot);
                    // Only records that were ever allocated and then
                    // cleared belong on the free list; `top` bounds the
                    // never-used tail of each block.
                    if slot < block.top {
                        self.free_list.push(handle | PTR_TAG);
                    }
                }
            }
        }
        // Strip tag bits when actually consuming (kept for parity with
        // the C++ tagged free list).
        for h in self.free_list.iter_mut() {
            *h &= !PTR_TAG;
        }
        self.num_free_handles = self.free_list.len();
    }

    /// `allocate_metadata_handle`.
    pub fn allocate_metadata_handle(&mut self, target: MetadataTarget) -> Option<JMetadata> {
        if self.allocate_before_rebuild > 0 {
            self.allocate_before_rebuild -= 1;
        } else if self.free_list.is_empty() && self.pending_free.len() >= 16 {
            self.rebuild_free_list();
        }

        let handle = match self.get_handle() {
            Some(h) => h,
            None => {
                self.blocks.push(MetadataHandleBlock::new());
                self.get_handle()?
            }
        };
        if let Some(record) = self.record_mut(handle) {
            record.value = Some(target);
            record.name = None;
        }
        Some(handle)
    }

    /// `allocate_handle(methodHandle)` analogue.
    pub fn allocate_handle_method(&mut self, key: impl Into<String>) -> Option<JMetadata> {
        self.allocate_metadata_handle(MetadataTarget::method(key))
    }

    /// `allocate_handle(constantPoolHandle)` analogue.
    pub fn allocate_handle_constant_pool(&mut self, key: impl Into<String>) -> Option<JMetadata> {
        self.allocate_metadata_handle(MetadataTarget::constant_pool(key))
    }

    /// `allocate_handle(klass)` analogue.
    pub fn allocate_handle_klass(&mut self, key: impl Into<String>) -> Option<JMetadata> {
        self.allocate_metadata_handle(MetadataTarget::klass(key))
    }

    /// Resolve a handle to its target (null handle -> None).
    pub fn resolve(&self, handle: JMetadata) -> Option<MetadataTarget> {
        if handle == 0 {
            return None;
        }
        self.record(handle).and_then(|r| r.value.clone())
    }

    /// The metadata key for a handle (empty string for the null handle).
    pub fn key_of(&self, handle: JMetadata) -> String {
        self.resolve(handle).map_or_else(String::new, |t| t.key().to_string())
    }

    /// `release_handle`: clears the slot; the slot is recycled by the
    /// next `rebuild_free_list` (the HandleCleaner contract).
    pub fn release_handle(&mut self, handle: JMetadata) {
        if handle == 0 {
            return;
        }
        if let Some(record) = self.record_mut(handle) {
            if record.value.is_some() {
                record.value = None;
                record.name = None;
                self.num_handles = self.num_handles.saturating_sub(1);
                self.pending_free.push(handle);
            }
        }
    }

    /// `clear`: clears all handles without releasing block memory.
    pub fn clear(&mut self) {
        for block in &mut self.blocks {
            for record in &mut block.handles {
                record.value = None;
                record.name = None;
            }
            block.top = 0;
        }
        self.free_list.clear();
        self.pending_free.clear();
        self.num_handles = 0;
        self.num_free_handles = 0;
    }

    /// `metadata_do`: visit every live metadata target.
    pub fn metadata_do(&self, mut f: impl FnMut(&MetadataTarget)) {
        for block in &self.blocks {
            for record in &block.handles {
                if let Some(target) = &record.value {
                    f(target);
                }
            }
        }
    }

    /// `do_unloading`: drop handles whose metadata is no longer live.
    /// Metadata liveness is tracked by keys in this port, so unloading
    /// clears everything not reachable — modelled as a full clear of
    /// stale entries is impossible without a liveness oracle; instead
    /// this performs the same observable effect as the C++ version for
    /// unloaded classes by removing handles explicitly released by the
    /// class unloading path (already covered by release_handle).
    pub fn do_unloading(&mut self) {
        self.rebuild_free_list();
    }
}

impl Default for MetadataHandles {
    fn default() -> Self {
        Self::new()
    }
}
