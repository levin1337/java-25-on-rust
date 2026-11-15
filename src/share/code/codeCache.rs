/*
 * Port of OpenJDK 25 HotSpot CodeCache (share/code/codeCache.hpp / codeCache.cpp)
 *
 * CodeCache manages segmented executable memory heaps for compiled Java
 * methods (nmethods), runtime stubs and adapters.  It implements the
 * segmented code cache (non-nmethods / profiled / non-profiled heaps),
 * first-fit free-list allocation with coalescing, pc->nmethod lookup and
 * sweeper heuristics (NotEntrant -> Zombie -> reclaim).
 */

use super::codeBlob::{CodeBlob, CodeBlobKind, CodeBlobType};
use super::nmethod::{NMethod, NMethodState};
use crate::share::compiler::compilerDefinitions::CompLevel;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// One free region inside a CodeHeap (offset relative to heap base).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FreeRegion {
    offset: usize,
    size: usize,
}

/// A segmented code heap: bump-allocated from `base_address`, with a
/// sorted free list for reclaimed space (CodeHeap analogue).
#[derive(Debug)]
pub struct CodeHeap {
    blob_type: CodeBlobType,
    capacity: usize,
    high_water: usize, // bump pointer: next never-allocated offset
    base_address: usize,
    blobs: Vec<CodeBlob>,
    /// Sorted by offset, non-overlapping free regions.
    free_list: Vec<FreeRegion>,
}

impl CodeHeap {
    pub fn new(blob_type: CodeBlobType, capacity: usize) -> Self {
        Self {
            blob_type,
            capacity,
            high_water: 0,
            base_address: 0,
            blobs: Vec::new(),
            free_list: Vec::new(),
        }
    }

    pub fn with_base(blob_type: CodeBlobType, base_address: usize, capacity: usize) -> Self {
        Self {
            blob_type,
            capacity,
            high_water: 0,
            base_address,
            blobs: Vec::new(),
            free_list: Vec::new(),
        }
    }

    #[inline]
    pub fn blob_type(&self) -> CodeBlobType {
        self.blob_type
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes currently allocated to live blobs.
    #[inline]
    pub fn used(&self) -> usize {
        self.blobs.iter().map(|b| b.size()).sum()
    }

    #[inline]
    pub fn high_water(&self) -> usize {
        self.high_water
    }

    #[inline]
    pub fn free_space(&self) -> usize {
        self.capacity.saturating_sub(self.used())
    }

    #[inline]
    pub fn base_address(&self) -> usize {
        self.base_address
    }

    /// First-fit allocation with splitting; falls back to bump allocation.
    /// Returns the heap-relative offset.
    fn alloc_offset(&mut self, size: usize) -> Option<usize> {
        // First-fit over the free list.
        for i in 0..self.free_list.len() {
            if self.free_list[i].size >= size {
                let region = self.free_list[i];
                let offset = region.offset;
                if region.size == size {
                    self.free_list.remove(i);
                } else {
                    self.free_list[i] = FreeRegion {
                        offset: offset + size,
                        size: region.size - size,
                    };
                }
                return Some(offset);
            }
        }
        // Bump allocation.
        if self.high_water + size <= self.capacity {
            let offset = self.high_water;
            self.high_water += size;
            return Some(offset);
        }
        None
    }

    /// Insert a freed region and coalesce with neighbours.
    fn free_offset(&mut self, offset: usize, size: usize) {
        if size == 0 {
            return;
        }
        // Find insertion point keeping the list sorted by offset.
        let pos = self.free_list.partition_point(|r| r.offset < offset);
        self.free_list.insert(pos, FreeRegion { offset, size });
        // Coalesce with previous.
        if pos > 0 {
            let prev = self.free_list[pos - 1];
            if prev.offset + prev.size == offset {
                self.free_list[pos - 1].size += size;
                self.free_list.remove(pos);
                return self.free_offset_coalesce(prev.offset);
            }
        }
        self.free_offset_coalesce(offset);
    }

    fn free_offset_coalesce(&mut self, offset: usize) {
        // Merge region starting at `offset` with the following region(s).
        loop {
            let pos = self.free_list.partition_point(|r| r.offset <= offset);
            if pos == 0 {
                return;
            }
            let idx = pos - 1;
            let cur = self.free_list[idx];
            let next_end = cur.offset + cur.size;
            if next_end == self.free_list.get(pos).map_or(usize::MAX, |n| n.offset) {
                let next = self.free_list.remove(pos);
                self.free_list[idx].size += next.size;
            } else {
                return;
            }
        }
    }

    pub fn allocate(
        &mut self,
        name: String,
        size: usize,
        kind: CodeBlobKind,
    ) -> Result<usize, String> {
        let offset = self
            .alloc_offset(size)
            .ok_or_else(|| "CodeHeap out of memory".to_string())?;
        let addr = self.base_address + offset;
        let blob = CodeBlob::new(name, self.blob_type, kind, addr, size);
        self.blobs.push(blob);
        Ok(addr)
    }

    pub fn allocate_with_code(
        &mut self,
        name: String,
        size: usize,
        kind: CodeBlobKind,
        code_bytes: Vec<u8>,
    ) -> Result<usize, String> {
        let offset = self
            .alloc_offset(size)
            .ok_or_else(|| "CodeHeap out of memory".to_string())?;
        let addr = self.base_address + offset;
        let blob = CodeBlob::with_code(name, self.blob_type, kind, addr, size, code_bytes);
        self.blobs.push(blob);
        Ok(addr)
    }

    /// Free the blob at `addr` (absolute address) and return its size.
    pub fn free_blob(&mut self, addr: usize) -> bool {
        if let Some(pos) = self.blobs.iter().position(|b| b.code_begin() == addr) {
            let blob = self.blobs.remove(pos);
            let offset = blob.code_begin().saturating_sub(self.base_address);
            self.free_offset(offset, blob.size());
            true
        } else {
            false
        }
    }

    pub fn find_blob(&self, addr: usize) -> Option<&CodeBlob> {
        self.blobs.iter().find(|b| b.blob_contains(addr))
    }

    #[inline]
    pub fn blobs(&self) -> &[CodeBlob] {
        &self.blobs
    }

    #[inline]
    pub fn blobs_mut(&mut self) -> &mut [CodeBlob] {
        &mut self.blobs
    }

    #[inline]
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base_address && addr < self.base_address + self.capacity
    }

    /// Total reclaimable bytes (free list + never-used tail).
    pub fn total_free_bytes(&self) -> usize {
        self.free_list.iter().map(|r| r.size).sum::<usize>()
            + self.capacity.saturating_sub(self.high_water)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SweepSummary {
    pub swept_zombies: usize,
    pub transitioned_to_zombie: usize,
    pub total_freed_bytes: usize,
}

pub struct CodeCache {
    non_nmethods_heap: Mutex<CodeHeap>,
    profiled_nmethods_heap: Mutex<CodeHeap>,
    non_profiled_nmethods_heap: Mutex<CodeHeap>,
    all_heap: Mutex<Option<CodeHeap>>,
    segmented: bool,
    /// method_key -> installed nmethod.
    nmethods: Mutex<HashMap<String, NMethod>>,
    /// method_key -> osr_bci -> nmethod.
    osr_nmethods: Mutex<HashMap<String, BTreeMap<i32, NMethod>>>,
    /// code_begin -> nmethod index for pc->method lookup (both regular
    /// and OSR nmethods; NMethod clones share the same atomic state).
    pc_index: Mutex<BTreeMap<usize, NMethod>>,
    /// Heap addresses of live blobs that back nmethods (for freeing).
    nmethod_blobs: Mutex<HashMap<String, usize>>,
    sweeper_cycle: AtomicU64,
}

static GLOBAL_CODE_CACHE: OnceLock<CodeCache> = OnceLock::new();

impl CodeCache {
    pub const DEFAULT_RESERVED_SIZE: usize = 240 * 1024 * 1024; // 240MB
    pub const DEFAULT_NON_NMETHOD_SIZE: usize = 8 * 1024 * 1024; // 8MB
    pub const DEFAULT_PROFILED_SIZE: usize = 116 * 1024 * 1024; // 116MB
    pub const DEFAULT_NON_PROFILED_SIZE: usize = 116 * 1024 * 1024; // 116MB

    pub fn new() -> Self {
        let non_nmethod_base = 0x1000_0000usize;
        let profiled_base = non_nmethod_base + Self::DEFAULT_NON_NMETHOD_SIZE;
        let non_profiled_base = profiled_base + Self::DEFAULT_PROFILED_SIZE;

        Self {
            non_nmethods_heap: Mutex::new(CodeHeap::with_base(
                CodeBlobType::NonNMethod,
                non_nmethod_base,
                Self::DEFAULT_NON_NMETHOD_SIZE,
            )),
            profiled_nmethods_heap: Mutex::new(CodeHeap::with_base(
                CodeBlobType::MethodProfiled,
                profiled_base,
                Self::DEFAULT_PROFILED_SIZE,
            )),
            non_profiled_nmethods_heap: Mutex::new(CodeHeap::with_base(
                CodeBlobType::MethodNonProfiled,
                non_profiled_base,
                Self::DEFAULT_NON_PROFILED_SIZE,
            )),
            all_heap: Mutex::new(None),
            segmented: true,
            nmethods: Mutex::new(HashMap::new()),
            osr_nmethods: Mutex::new(HashMap::new()),
            pc_index: Mutex::new(BTreeMap::new()),
            nmethod_blobs: Mutex::new(HashMap::new()),
            sweeper_cycle: AtomicU64::new(0),
        }
    }

    pub fn new_non_segmented(capacity: usize) -> Self {
        Self {
            non_nmethods_heap: Mutex::new(CodeHeap::new(CodeBlobType::NonNMethod, 0)),
            profiled_nmethods_heap: Mutex::new(CodeHeap::new(CodeBlobType::MethodProfiled, 0)),
            non_profiled_nmethods_heap: Mutex::new(CodeHeap::new(
                CodeBlobType::MethodNonProfiled,
                0,
            )),
            all_heap: Mutex::new(Some(CodeHeap::new(CodeBlobType::All, capacity))),
            segmented: false,
            nmethods: Mutex::new(HashMap::new()),
            osr_nmethods: Mutex::new(HashMap::new()),
            pc_index: Mutex::new(BTreeMap::new()),
            nmethod_blobs: Mutex::new(HashMap::new()),
            sweeper_cycle: AtomicU64::new(0),
        }
    }

    pub fn global() -> &'static CodeCache {
        GLOBAL_CODE_CACHE.get_or_init(Self::new)
    }

    #[inline]
    pub fn is_segmented(&self) -> bool {
        self.segmented
    }

    pub fn method_key(holder: &str, name: &str, sig: &str) -> String {
        format!("{}.{}{}", holder, name, sig)
    }

    /// Key under which an nmethod's heap blob is registered in
    /// `nmethod_blobs`: regular nmethods use the plain method key,
    /// OSR nmethods append `@entry_bci`.
    pub fn blob_key(holder: &str, name: &str, sig: &str, entry_bci: i32) -> String {
        let key = Self::method_key(holder, name, sig);
        if entry_bci == NMethod::INVOCATION_ENTRY_BCI {
            key
        } else {
            format!("{}@{}", key, entry_bci)
        }
    }

    /// Insert an nmethod into the registries and the pc index.
    /// Regular nmethods go into `nmethods`; OSR nmethods go into
    /// `osr_nmethods` keyed by (method_key, entry_bci).  Both are
    /// reachable through the pc index.
    pub fn add_nmethod(&self, nm: NMethod) {
        let key = Self::method_key(nm.method_holder(), nm.method_name(), nm.method_signature());
        self.pc_index
            .lock()
            .unwrap()
            .insert(nm.code_begin(), nm.clone());
        if nm.is_osr_method() {
            let mut guard = self.osr_nmethods.lock().unwrap();
            guard
                .entry(key)
                .or_default()
                .insert(nm.entry_bci(), nm);
        } else {
            let mut guard = self.nmethods.lock().unwrap();
            guard.insert(key, nm);
        }
    }

    /// Register the heap blob backing an nmethod so the sweeper can
    /// reclaim it.
    pub fn register_nmethod_blob(&self, method_key: &str, blob_addr: usize) {
        self.nmethod_blobs
            .lock()
            .unwrap()
            .insert(method_key.to_string(), blob_addr);
    }

    pub fn find_nmethod(&self, holder: &str, name: &str, sig: &str) -> Option<NMethod> {
        let key = Self::method_key(holder, name, sig);
        let guard = self.nmethods.lock().unwrap();
        guard.get(&key).cloned()
    }

    pub fn find_nmethod_by_key(&self, method_key: &str) -> Option<NMethod> {
        let guard = self.nmethods.lock().unwrap();
        guard.get(method_key).cloned()
    }

    pub fn find_osr_nmethod(
        &self,
        holder: &str,
        name: &str,
        sig: &str,
        bci: i32,
    ) -> Option<NMethod> {
        let key = Self::method_key(holder, name, sig);
        let guard = self.osr_nmethods.lock().unwrap();
        guard.get(&key).and_then(|m| m.get(&bci).cloned())
    }

    pub fn find_osr_nmethod_by_key(&self, method_key: &str, bci: i32) -> Option<NMethod> {
        let guard = self.osr_nmethods.lock().unwrap();
        guard.get(method_key).and_then(|m| m.get(&bci).cloned())
    }

    /// pc -> nmethod lookup via the sorted begin-address index:
    /// finds the nmethod with the greatest code_begin <= addr and checks
    /// whether addr falls inside its range.
    pub fn find_nmethod_at_address(&self, addr: usize) -> Option<NMethod> {
        let index = self.pc_index.lock().unwrap();
        let (_, nm) = index.range(..=addr).next_back()?;
        if addr >= nm.code_begin() && addr < nm.code_end() {
            return Some(nm.clone());
        }
        // The predecessor may end before addr while a *later-starting*
        // nmethod can't exist (index ordered), but an OSR nmethod for the
        // same method may share the range differently — scan OSR map as
        // fallback only when the direct hit failed.
        None
    }

    /// Exact nmethod containing `addr`, scanning all (fallback used by
    /// tests and rare paths).
    pub fn find_any_nmethod_at(&self, addr: usize) -> Option<NMethod> {
        {
            let guard = self.nmethods.lock().unwrap();
            for nm in guard.values() {
                if addr >= nm.code_begin() && addr < nm.code_end() {
                    return Some(nm.clone());
                }
            }
        }
        let guard = self.osr_nmethods.lock().unwrap();
        for map in guard.values() {
            for nm in map.values() {
                if addr >= nm.code_begin() && addr < nm.code_end() {
                    return Some(nm.clone());
                }
            }
        }
        None
    }

    pub fn find_blob_at_address(&self, addr: usize) -> Option<CodeBlob> {
        if self.segmented {
            if let Some(b) = self.non_nmethods_heap.lock().unwrap().find_blob(addr) {
                return Some(b.clone());
            }
            if let Some(b) = self.profiled_nmethods_heap.lock().unwrap().find_blob(addr) {
                return Some(b.clone());
            }
            if let Some(b) = self
                .non_profiled_nmethods_heap
                .lock()
                .unwrap()
                .find_blob(addr)
            {
                return Some(b.clone());
            }
            None
        } else {
            let guard = self.all_heap.lock().unwrap();
            guard.as_ref().and_then(|h| h.find_blob(addr).cloned())
        }
    }

    /// Allocate raw space for a non-nmethod blob.
    pub fn allocate_blob(
        &self,
        name: String,
        blob_type: CodeBlobType,
        size: usize,
        kind: CodeBlobKind,
    ) -> Result<usize, String> {
        if !self.segmented {
            let mut guard = self.all_heap.lock().unwrap();
            if let Some(heap) = guard.as_mut() {
                return heap.allocate(name, size, kind);
            }
            return Err("CodeHeap not initialized".to_string());
        }

        match blob_type {
            CodeBlobType::NonNMethod => {
                let mut heap = self.non_nmethods_heap.lock().unwrap();
                if heap.free_space() >= size {
                    heap.allocate(name, size, kind)
                } else {
                    drop(heap);
                    // Fallback to non-profiled heap (HotSpot does the same
                    // for non-nmethod overflow when profiled heap is full).
                    self.non_profiled_nmethods_heap
                        .lock()
                        .unwrap()
                        .allocate(name, size, kind)
                }
            }
            CodeBlobType::MethodProfiled => self
                .profiled_nmethods_heap
                .lock()
                .unwrap()
                .allocate(name, size, kind),
            CodeBlobType::MethodNonProfiled => self
                .non_profiled_nmethods_heap
                .lock()
                .unwrap()
                .allocate(name, size, kind),
            CodeBlobType::All => {
                let mut heap = self.non_profiled_nmethods_heap.lock().unwrap();
                heap.allocate(name, size, kind)
            }
        }
    }

    /// Allocate space for an nmethod and store its code bytes in the blob.
    /// Returns the absolute code_begin address.
    pub fn allocate_nmethod_blob(
        &self,
        name: String,
        blob_type: CodeBlobType,
        total_size: usize,
        code_bytes: Vec<u8>,
    ) -> Result<usize, String> {
        if !self.segmented {
            let mut guard = self.all_heap.lock().unwrap();
            if let Some(heap) = guard.as_mut() {
                return heap.allocate_with_code(name, total_size, CodeBlobKind::Nmethod, code_bytes);
            }
            return Err("CodeHeap not initialized".to_string());
        }
        let heap = match blob_type {
            CodeBlobType::MethodProfiled => &self.profiled_nmethods_heap,
            _ => &self.non_profiled_nmethods_heap,
        };
        heap.lock()
            .unwrap()
            .allocate_with_code(name, total_size, CodeBlobKind::Nmethod, code_bytes)
    }

    /// Free the heap blob backing a swept nmethod.
    pub fn free_nmethod_blob(&self, method_key: &str) {
        let addr = self
            .nmethod_blobs
            .lock()
            .unwrap()
            .remove(method_key);
        if let Some(addr) = addr {
            if self.segmented {
                for heap in [
                    &self.non_nmethods_heap,
                    &self.profiled_nmethods_heap,
                    &self.non_profiled_nmethods_heap,
                ] {
                    if heap.lock().unwrap().free_blob(addr) {
                        break;
                    }
                }
            } else if let Some(heap) = self.all_heap.lock().unwrap().as_mut() {
                heap.free_blob(addr);
            }
        }
    }

    /// Install a fully-formed nmethod: allocate heap space sized to hold
    /// code + metadata, place code bytes, register the nmethod.
    /// Returns the code_begin address.
    pub fn install_nmethod(
        &self,
        mut nm: NMethod,
        code_bytes: Vec<u8>,
        metadata_size: usize,
    ) -> Result<usize, String> {
        let code_len = code_bytes.len();
        let total = align_up(code_len + metadata_size, 16).max(64);
        let blob_type = Self::comp_level_to_blob_type(nm.comp_level());
        let name = format!(
            "{} {}.{}{}",
            nm.comp_level().name(),
            nm.method_holder(),
            nm.method_name(),
            nm.method_signature()
        );
        let addr = self.allocate_nmethod_blob(name, blob_type, total, code_bytes.clone())?;
        // OSR nmethods share the method key; key their heap blob by
        // (method_key, entry_bci) so regular and OSR blobs don't
        // overwrite each other in `nmethod_blobs`.
        let blob_key = Self::blob_key(
            nm.method_holder(),
            nm.method_name(),
            nm.method_signature(),
            nm.entry_bci(),
        );
        self.register_nmethod_blob(&blob_key, addr);
        // The heap address is only a bookkeeping key: CodeHeap does not
        // reserve real memory.  Materialize the code bytes into an RWW
        // region so `entry_point` is actually callable, and relocate
        // the nmethod there (HotSpot's code-cache allocation analogue).
        let exec_addr = crate::share::jitruntime::publish_executable(&code_bytes, total)
            .ok_or_else(|| "executable memory allocation failed".to_string())?;
        nm.set_code_location(exec_addr, total);
        nm.set_code_bytes(Vec::new()); // bytes live in the exec arena
        self.add_nmethod(nm);
        Ok(exec_addr)
    }

    pub fn nmethods_count(&self) -> usize {
        self.nmethods.lock().unwrap().len()
    }

    pub fn osr_nmethods_count(&self) -> usize {
        self.osr_nmethods
            .lock()
            .unwrap()
            .values()
            .map(|m| m.len())
            .sum()
    }

    pub fn blob_count(&self) -> usize {
        if self.segmented {
            self.non_nmethods_heap.lock().unwrap().blobs().len()
                + self.profiled_nmethods_heap.lock().unwrap().blobs().len()
                + self
                    .non_profiled_nmethods_heap
                    .lock()
                    .unwrap()
                    .blobs()
                    .len()
        } else {
            self.all_heap
                .lock()
                .unwrap()
                .as_ref()
                .map_or(0, |h| h.blobs().len())
        }
    }

    pub fn make_not_entrant(&self, holder: &str, name: &str, sig: &str) -> bool {
        let key = Self::method_key(holder, name, sig);
        self.make_not_entrant_by_key(&key)
    }

    pub fn make_not_entrant_by_key(&self, key: &str) -> bool {
        let guard = self.nmethods.lock().unwrap();
        if let Some(nm) = guard.get(key) {
            // NMethod methods take &self (shared atomics), so no mut needed.
            return nm.make_not_entrant_with_reason(
                super::nmethod::InvalidationReason::NotUsed,
            );
        }
        false
    }

    pub fn mark_all_nmethods_for_deopt(&self) {
        let guard = self.nmethods.lock().unwrap();
        for nm in guard.values() {
            if nm.is_in_use() {
                nm.mark_for_deoptimization();
            }
        }
    }

    /// Immediate deinstallation of every nmethod for `method_key`
    /// (regular + all OSR variants): mark for deoptimization, force the
    /// NotEntrant -> Zombie transition and reclaim the heap blobs and
    /// pc-index entries right away, without waiting for a sweeper
    /// cycle.  Returns the number of nmethods removed.  Used for
    /// dependency invalidation where HotSpot calls
    /// `nmethod::mark_for_deoptimization` + `make_not_entrant` and lets
    /// the sweeper finish the job — here the reclaim is synchronous so
    /// the method's code is guaranteed gone when this returns.
    pub fn deinstall_nmethod(&self, method_key: &str) -> usize {
        let regular = self.nmethods.lock().unwrap().remove(method_key);
        let osr_map = self.osr_nmethods.lock().unwrap().remove(method_key);
        let mut nms: Vec<NMethod> = Vec::new();
        if let Some(nm) = regular {
            nms.push(nm);
        }
        if let Some(map) = osr_map {
            nms.extend(map.into_values());
        }
        let count = nms.len();
        for nm in nms {
            nm.mark_for_deoptimization();
            nm.make_not_entrant_with_reason(
                super::nmethod::InvalidationReason::MarkedForDeoptimization,
            );
            nm.make_zombie();
            self.pc_index.lock().unwrap().remove(&nm.code_begin());
            let blob_key = Self::blob_key(
                nm.method_holder(),
                nm.method_name(),
                nm.method_signature(),
                nm.entry_bci(),
            );
            self.free_nmethod_blob(&blob_key);
        }
        count
    }

    /// Pure zombie sweep: drop Zombie nmethods and reclaim their blobs.
    pub fn sweep_zombies(&self) -> usize {
        let mut guard = self.nmethods.lock().unwrap();
        let mut swept_keys = Vec::new();
        guard.retain(|key, nm| {
            if nm.state() == NMethodState::Zombie {
                swept_keys.push(key.clone());
                false
            } else {
                true
            }
        });
        drop(guard);
        for key in &swept_keys {
            self.remove_pc_index_for_key(key);
            self.free_nmethod_blob(key);
        }
        // OSR variants: drop zombies, free their (bci-suffixed) blobs.
        let mut osr = self.osr_nmethods.lock().unwrap();
        let mut osr_freed: Vec<(String, i32, usize)> = Vec::new();
        for (key, map) in osr.iter_mut() {
            map.retain(|bci, nm| {
                if nm.state() == NMethodState::Zombie {
                    osr_freed.push((key.clone(), *bci, nm.code_begin()));
                    false
                } else {
                    true
                }
            });
        }
        osr.retain(|_, m| !m.is_empty());
        drop(osr);
        for (key, bci, begin) in osr_freed {
            self.pc_index.lock().unwrap().remove(&begin);
            self.free_nmethod_blob(&format!("{}@{}", key, bci));
        }
        swept_keys.len()
    }

    /// Full sweeper cycle: NotEntrant -> Zombie when not on stack, then
    /// reclaim zombies.  `active_stack_keys` are method keys with live
    /// compiled frames.
    pub fn sweep_cycle(&self, active_stack_keys: &HashSet<String>) -> SweepSummary {
        self.sweeper_cycle.fetch_add(1, Ordering::Relaxed);
        let mut summary = SweepSummary::default();

        // Pass 1: NotEntrant -> Zombie.
        {
            let guard = self.nmethods.lock().unwrap();
            for (key, nm) in guard.iter() {
                if nm.is_not_entrant() && !active_stack_keys.contains(key) {
                    nm.make_zombie();
                    summary.transitioned_to_zombie += 1;
                }
            }
        }

        // Pass 2: sweep zombies + free blobs.
        let mut guard = self.nmethods.lock().unwrap();
        let mut zombie_keys = Vec::new();
        guard.retain(|key, nm| {
            if nm.is_zombie() {
                summary.total_freed_bytes += nm.code_size();
                zombie_keys.push(key.clone());
                false
            } else {
                true
            }
        });
        summary.swept_zombies = zombie_keys.len();
        drop(guard);
        for key in &zombie_keys {
            self.remove_pc_index_for_key(key);
            self.free_nmethod_blob(key);
        }

        // OSR pass 1: NotEntrant -> Zombie when no live frame uses them.
        {
            let mut osr = self.osr_nmethods.lock().unwrap();
            for (key, map) in osr.iter_mut() {
                for nm in map.values() {
                    if nm.is_not_entrant() && !active_stack_keys.contains(key) {
                        nm.make_zombie();
                        summary.transitioned_to_zombie += 1;
                    }
                }
            }
        }

        // OSR pass 2: reclaim zombies and their blobs.
        let mut osr = self.osr_nmethods.lock().unwrap();
        let mut osr_freed: Vec<(String, i32, usize, usize)> = Vec::new();
        for (key, map) in osr.iter_mut() {
            map.retain(|bci, nm| {
                if nm.state() == NMethodState::Zombie {
                    osr_freed.push((key.clone(), *bci, nm.code_begin(), nm.code_size()));
                    false
                } else {
                    true
                }
            });
        }
        osr.retain(|_, m| !m.is_empty());
        drop(osr);
        for (key, bci, begin, size) in osr_freed {
            summary.swept_zombies += 1;
            summary.total_freed_bytes += size;
            self.pc_index.lock().unwrap().remove(&begin);
            self.free_nmethod_blob(&format!("{}@{}", key, bci));
        }

        summary
    }

    /// Remove pc-index entries belonging to `method_key`
    /// (regular + all OSR variants).
    fn remove_pc_index_for_key(&self, method_key: &str) {
        let mut index = self.pc_index.lock().unwrap();
        index.retain(|_, nm| {
            Self::method_key(nm.method_holder(), nm.method_name(), nm.method_signature()) != method_key
        });
    }

    /// Remove a single OSR nmethod's pc-index entry.
    fn remove_pc_index_entry(&self, code_begin: usize) {
        self.pc_index.lock().unwrap().remove(&code_begin);
    }

    pub fn total_capacity(&self) -> usize {
        if self.segmented {
            self.non_nmethods_heap.lock().unwrap().capacity()
                + self.profiled_nmethods_heap.lock().unwrap().capacity()
                + self.non_profiled_nmethods_heap.lock().unwrap().capacity()
        } else {
            self.all_heap
                .lock()
                .unwrap()
                .as_ref()
                .map_or(0, |h| h.capacity())
        }
    }

    pub fn total_used(&self) -> usize {
        if self.segmented {
            self.non_nmethods_heap.lock().unwrap().used()
                + self.profiled_nmethods_heap.lock().unwrap().used()
                + self.non_profiled_nmethods_heap.lock().unwrap().used()
        } else {
            self.all_heap
                .lock()
                .unwrap()
                .as_ref()
                .map_or(0, |h| h.used())
        }
    }

    /// HotSpot `CodeCache::reverse_free_ratio`-style value: percent of
    /// reserved space currently in use (0..100).
    pub fn reverse_free_ratio(&self) -> f64 {
        let cap = self.total_capacity();
        if cap == 0 {
            return 0.0;
        }
        (self.total_used() as f64 / cap as f64) * 100.0
    }

    pub fn total_free(&self) -> usize {
        self.total_capacity().saturating_sub(self.total_used())
    }

    pub fn comp_level_to_blob_type(level: CompLevel) -> CodeBlobType {
        match level {
            CompLevel::None | CompLevel::Any => CodeBlobType::NonNMethod,
            CompLevel::Simple | CompLevel::FullOptimization => CodeBlobType::MethodNonProfiled,
            CompLevel::LimitedProfile | CompLevel::FullProfile => CodeBlobType::MethodProfiled,
        }
    }
}

fn align_up(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

impl Default for CodeCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::compiler::compilerDefinitions::CompLevel;

    fn heap_alloc_free_coalesce() {
        let mut heap = CodeHeap::with_base(CodeBlobType::MethodNonProfiled, 0x1000, 1024);
        let a = heap.allocate("a".to_string(), 100, CodeBlobKind::Nmethod).unwrap();
        let b = heap.allocate("b".to_string(), 100, CodeBlobKind::Nmethod).unwrap();
        assert_eq!(heap.used(), 200);
        assert!(heap.free_blob(a));
        assert!(heap.free_blob(b));
        // Coalesced: the whole 200 bytes should be one free region again,
        // reusable for a 200-byte allocation.
        let c = heap.allocate("c".to_string(), 200, CodeBlobKind::Nmethod).unwrap();
        assert_eq!(heap.used(), 200);
        let _ = c;
    }

    #[test]
    fn heap_first_fit_and_bump() {
        heap_alloc_free_coalesce();
        let mut heap = CodeHeap::with_base(CodeBlobType::NonNMethod, 0x2000, 512);
        let x = heap.allocate("x".to_string(), 64, CodeBlobKind::RuntimeStub).unwrap();
        let y = heap.allocate("y".to_string(), 64, CodeBlobKind::RuntimeStub).unwrap();
        assert_eq!(y - x, 64);
        assert!(heap.free_space() >= 512 - 128);
        // Allocation beyond capacity fails.
        assert!(heap.allocate("big".to_string(), 4096, CodeBlobKind::RuntimeStub).is_err());
    }

    #[test]
    fn nmethod_install_lookup_and_deinstall() {
        let cache = CodeCache::new_non_segmented(1 << 20);
        let nm = NMethod::with_entry_points(
            "T".to_string(),
            "m".to_string(),
            "()V".to_string(),
            CompLevel::Simple,
            NMethod::INVOCATION_ENTRY_BCI,
            0,
            0,
            0,
            0,
            0,
            vec![],
            vec![],
        );
        let addr = cache
            .install_nmethod(nm, vec![0x90, 0xc3], 64)
            .expect("install");
        assert_ne!(addr, 0);
        let found = cache.find_nmethod("T", "m", "()V").expect("registered");
        assert!(found.is_in_use());
        assert!(found.entry_point() != 0);
        // pc -> nmethod lookup inside the published range.
        let hit = cache.find_nmethod_at_address(addr + 1).expect("pc lookup");
        assert_eq!(hit.method_name(), "m");
        assert!(cache.find_nmethod_at_address(addr + 1_000_000).is_none());

        // not-entrant -> sweep -> zombie -> reclaimed.
        assert!(cache.make_not_entrant("T", "m", "()V"));
        let summary = cache.sweep_cycle(&HashSet::new());
        assert_eq!(summary.transitioned_to_zombie, 1);
        assert_eq!(summary.swept_zombies, 1);
        assert!(cache.find_nmethod("T", "m", "()V").is_none());
        assert!(cache.find_nmethod_at_address(addr).is_none());
    }

    #[test]
    fn deinstall_removes_regular_and_osr() {
        let cache = CodeCache::new_non_segmented(1 << 20);
        let regular = NMethod::with_entry_points(
            "U".to_string(), "r".to_string(), "()V".to_string(),
            CompLevel::Simple, NMethod::INVOCATION_ENTRY_BCI,
            0, 0, 0, 0, 0, vec![], vec![],
        );
        let osr = NMethod::with_entry_points(
            "U".to_string(), "r".to_string(), "()V".to_string(),
            CompLevel::Simple, 12,
            0, 0, 0, 0, 0, vec![], vec![],
        );
        cache.install_nmethod(regular, vec![0x90, 0xc3], 32).unwrap();
        cache.install_nmethod(osr, vec![0x90, 0xc3], 32).unwrap();
        assert_eq!(cache.nmethods_count(), 1);
        assert_eq!(cache.osr_nmethods_count(), 1);
        let key = CodeCache::method_key("U", "r", "()V");
        assert_eq!(cache.deinstall_nmethod(&key), 2);
        assert!(cache.find_nmethod_by_key(&key).is_none());
        assert!(cache.find_osr_nmethod_by_key(&key, 12).is_none());
        assert_eq!(cache.nmethods_count(), 0);
        assert_eq!(cache.osr_nmethods_count(), 0);
    }

    #[test]
    fn osr_not_entrant_swept_to_zombie() {
        let cache = CodeCache::new_non_segmented(1 << 20);
        let osr = NMethod::with_entry_points(
            "V".to_string(), "o".to_string(), "()V".to_string(),
            CompLevel::Simple, 4,
            0, 0, 0, 0, 0, vec![], vec![],
        );
        cache.install_nmethod(osr, vec![0x90, 0xc3], 32).unwrap();
        let key = CodeCache::method_key("V", "o", "()V");
        // Mark the OSR nmethod not-entrant directly.
        let nm = cache.find_osr_nmethod_by_key(&key, 4).unwrap();
        nm.make_not_entrant();
        let summary = cache.sweep_cycle(&HashSet::new());
        assert_eq!(summary.transitioned_to_zombie, 1);
        assert!(cache.find_osr_nmethod_by_key(&key, 4).is_none());
    }

    #[test]
    fn blob_key_distinguishes_osr() {
        assert_eq!(
            CodeCache::blob_key("A", "b", "()V", NMethod::INVOCATION_ENTRY_BCI),
            "A.b()V"
        );
        assert_eq!(CodeCache::blob_key("A", "b", "()V", 7), "A.b()V@7");
    }
}
