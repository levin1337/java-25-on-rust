/*
 * Port of OpenJDK 25 HotSpot CodeCache (share/code/codeCache.hpp / codeCache.cpp)
 *
 * CodeCache manages segmented executable memory heaps for compiled Java methods (nmethods),
 * runtime stubs, and interpreter adapters. It implements segmented code cache
 * architecture (profiled, non-profiled, non-nmethods) and sweeper heuristics.
 */

use super::codeBlob::{CodeBlob, CodeBlobKind, CodeBlobType};
use super::nmethod::{NMethod, NMethodState};
use crate::share::compiler::compilerDefinitions::CompLevel;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug)]
pub struct CodeHeap {
    blob_type: CodeBlobType,
    capacity: usize,
    used: usize,
    base_address: usize,
    blobs: Vec<CodeBlob>,
}

impl CodeHeap {
    pub fn new(blob_type: CodeBlobType, capacity: usize) -> Self {
        Self {
            blob_type,
            capacity,
            used: 0,
            base_address: 0,
            blobs: Vec::new(),
        }
    }

    pub fn with_base(blob_type: CodeBlobType, base_address: usize, capacity: usize) -> Self {
        Self {
            blob_type,
            capacity,
            used: 0,
            base_address,
            blobs: Vec::new(),
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

    #[inline]
    pub fn used(&self) -> usize {
        self.used
    }

    #[inline]
    pub fn free_space(&self) -> usize {
        self.capacity.saturating_sub(self.used)
    }

    #[inline]
    pub fn base_address(&self) -> usize {
        self.base_address
    }

    pub fn allocate(
        &mut self,
        name: String,
        size: usize,
        kind: CodeBlobKind,
    ) -> Result<usize, String> {
        if self.used + size > self.capacity {
            return Err("CodeHeap out of memory".to_string());
        }
        let offset = self.base_address + self.used;
        let blob = CodeBlob::new(name, self.blob_type, kind, offset, size);
        self.blobs.push(blob);
        self.used += size;
        Ok(offset)
    }

    pub fn allocate_with_code(
        &mut self,
        name: String,
        size: usize,
        kind: CodeBlobKind,
        code_bytes: Vec<u8>,
    ) -> Result<usize, String> {
        if self.used + size > self.capacity {
            return Err("CodeHeap out of memory".to_string());
        }
        let offset = self.base_address + self.used;
        let blob = CodeBlob::with_code(name, self.blob_type, kind, offset, size, code_bytes);
        self.blobs.push(blob);
        self.used += size;
        Ok(offset)
    }

    pub fn free_blob(&mut self, offset: usize) -> bool {
        if let Some(pos) = self.blobs.iter().position(|b| b.code_begin() == offset) {
            let blob = self.blobs.remove(pos);
            self.used = self.used.saturating_sub(blob.size());
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
    nmethods: Mutex<HashMap<String, NMethod>>,
    osr_nmethods: Mutex<HashMap<String, Vec<NMethod>>>,
    sweeper_cycle: AtomicU64,
}

static GLOBAL_CODE_CACHE: OnceLock<CodeCache> = OnceLock::new();

impl CodeCache {
    pub const DEFAULT_RESERVED_SIZE: usize = 240 * 1024 * 1024; // 240MB
    pub const DEFAULT_NON_NMETHOD_SIZE: usize = 8 * 1024 * 1024; // 8MB
    pub const DEFAULT_PROFILED_SIZE: usize = 116 * 1024 * 1024; // 116MB
    pub const DEFAULT_NON_PROFILED_SIZE: usize = 116 * 1024 * 1024; // 116MB

    pub fn new() -> Self {
        let non_nmethod_base = 0x1000_0000;
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

    pub fn add_nmethod(&self, nm: NMethod) {
        let key = Self::method_key(nm.method_holder(), nm.method_name(), nm.method_signature());
        if nm.is_osr_method() {
            let mut guard = self.osr_nmethods.lock().unwrap();
            guard.entry(key.clone()).or_default().push(nm.clone());
        }
        let mut guard = self.nmethods.lock().unwrap();
        guard.insert(key, nm);
    }

    pub fn find_nmethod(&self, holder: &str, name: &str, sig: &str) -> Option<NMethod> {
        let key = Self::method_key(holder, name, sig);
        let guard = self.nmethods.lock().unwrap();
        guard.get(&key).cloned()
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
        guard
            .get(&key)
            .and_then(|list| list.iter().find(|nm| nm.entry_bci() == bci).cloned())
    }

    pub fn find_nmethod_at_address(&self, addr: usize) -> Option<NMethod> {
        let guard = self.nmethods.lock().unwrap();
        for nm in guard.values() {
            if addr >= nm.code_begin() && addr < nm.code_end() {
                return Some(nm.clone());
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
                    // Fallback to non-profiled heap
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

    pub fn nmethods_count(&self) -> usize {
        self.nmethods.lock().unwrap().len()
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
        let mut guard = self.nmethods.lock().unwrap();
        if let Some(nm) = guard.get_mut(&key) {
            nm.make_not_entrant();
            true
        } else {
            false
        }
    }

    pub fn mark_all_nmethods_for_deopt(&self) {
        let mut guard = self.nmethods.lock().unwrap();
        for nm in guard.values_mut() {
            if nm.is_in_use() {
                nm.mark_for_deoptimization();
            }
        }
    }

    /// Pure sweeper implementation: removes zombie nmethods and reclaims their slot.
    pub fn sweep_zombies(&self) -> usize {
        let mut guard = self.nmethods.lock().unwrap();
        let initial = guard.len();
        guard.retain(|_, nm| nm.state() != NMethodState::Zombie);
        let swept = initial.saturating_sub(guard.len());

        // Also sweep OSR nmethods
        let mut osr_guard = self.osr_nmethods.lock().unwrap();
        for list in osr_guard.values_mut() {
            list.retain(|nm| nm.state() != NMethodState::Zombie);
        }
        osr_guard.retain(|_, list| !list.is_empty());

        swept
    }

    /// Full sweeper cycle implementing HotSpot sweeper heuristics:
    /// 1. Transition NotEntrant methods to Zombie if they are not active on execution stacks.
    /// 2. Purge Zombie methods and reclaim code cache memory.
    pub fn sweep_cycle(&self, active_stack_keys: &[&str]) -> SweepSummary {
        self.sweeper_cycle.fetch_add(1, Ordering::Relaxed);
        let mut summary = SweepSummary::default();

        let mut guard = self.nmethods.lock().unwrap();

        // Pass 1: Transition NotEntrant to Zombie if no stack activations remain
        for (key, nm) in guard.iter_mut() {
            if nm.is_not_entrant() {
                let on_stack = active_stack_keys.contains(&key.as_str());
                if !on_stack {
                    nm.make_zombie();
                    summary.transitioned_to_zombie += 1;
                }
            }
        }

        // Pass 2: Sweep Zombies and track reclaimed memory
        let initial = guard.len();
        guard.retain(|_, nm| {
            if nm.is_zombie() {
                summary.total_freed_bytes += nm.code_size();
                false
            } else {
                true
            }
        });
        summary.swept_zombies = initial.saturating_sub(guard.len());

        // Clean OSR methods
        let mut osr_guard = self.osr_nmethods.lock().unwrap();
        for list in osr_guard.values_mut() {
            list.retain(|nm| nm.state() != NMethodState::Zombie);
        }
        osr_guard.retain(|_, list| !list.is_empty());

        summary
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

impl Default for CodeCache {
    fn default() -> Self {
        Self::new()
    }
}
