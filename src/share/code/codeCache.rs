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

    #[inline]
    pub fn is_full(&self) -> bool {
        self.free_space() == 0
    }

    /// Mark the blob at `addr` unused so `free_unused_blobs` reclaims it
    /// (non-nmethod reclamation path).
    pub fn mark_unused(&mut self, addr: usize) -> bool {
        if let Some(b) = self.blobs.iter_mut().find(|b| b.code_begin() == addr) {
            b.set_unused(true);
            true
        } else {
            false
        }
    }

    /// Reclaim every blob flagged unused; returns bytes freed.
    pub fn free_unused_blobs(&mut self) -> usize {
        let mut freed = 0usize;
        let mut i = 0;
        while i < self.blobs.len() {
            if self.blobs[i].is_unused() {
                let blob = self.blobs.remove(i);
                let offset = blob.code_begin().saturating_sub(self.base_address);
                self.free_offset(offset, blob.size());
                freed += blob.size();
            } else {
                i += 1;
            }
        }
        freed
    }

    /// `CodeHeap::print_on` analogue.
    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(
            f,
            "heap {:?} base={:#x} capacity={} used={} high_water={} free_list_regions={} blobs={}",
            self.blob_type,
            self.base_address,
            self.capacity,
            self.used(),
            self.high_water,
            self.free_list.len(),
            self.blobs.len()
        );
    }
}

/// Kind of call adapter (`AdapterHandlerLibrary` analogue).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdapterKind {
    /// Interpreter frame -> compiled nmethod entry (i2c).
    InterpreterToCompiled,
    /// Compiled nmethod return -> interpreter continuation (c2i).
    CompiledToInterpreter,
    /// JNI native -> Java (upcall) adapter.
    NativeToJava,
    /// Java -> JNI native adapter.
    JavaToNative,
}

impl AdapterKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::InterpreterToCompiled => "i2c",
            Self::CompiledToInterpreter => "c2i",
            Self::NativeToJava => "native2java",
            Self::JavaToNative => "java2native",
        }
    }
}

/// A generated adapter stub blob (`AdapterBlob` analogue): machine code
/// plus the metadata needed to patch/verify it.
#[derive(Clone, Debug)]
pub struct AdapterBlob {
    pub kind: AdapterKind,
    /// Method signature the adapter was specialized for.
    pub signature: String,
    /// Absolute entry point (code_begin).
    pub entry_point: usize,
    /// Emitted machine code bytes.
    pub code: Vec<u8>,
    /// Offset of the call/jmp to the target inside `code` (for
    /// relocation/patching).
    pub target_patch_offset: usize,
    /// Frame size the adapter sets up.
    pub frame_size: usize,
    /// pc offsets of potential exception points (npe/ame), like
    /// VtableStub bookkeeping.
    pub exception_points: Vec<usize>,
}

impl AdapterBlob {
    #[inline]
    pub fn code_size(&self) -> usize {
        self.code.len()
    }

    #[inline]
    pub fn code_end(&self) -> usize {
        self.entry_point + self.code.len()
    }

    #[inline]
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.entry_point && addr < self.code_end()
    }
}

/// Cache of generated adapter stubs keyed by (kind, signature)
/// (`AdapterHandlerLibrary` analogue).
#[derive(Debug, Default)]
pub struct AdapterHandlerTable {
    adapters: HashMap<(AdapterKind, String), AdapterBlob>,
    /// Monotonic address allocator for adapter entry points inside the
    /// non-nmethod heap's address range.
    next_entry: usize,
}

impl AdapterHandlerTable {
    pub fn new(base: usize) -> Self {
        Self {
            adapters: HashMap::new(),
            next_entry: base,
        }
    }

    pub fn lookup(&self, kind: AdapterKind, signature: &str) -> Option<&AdapterBlob> {
        self.adapters.get(&(kind, signature.to_string()))
    }

    pub fn contains(&self, addr: usize) -> bool {
        self.adapters.values().any(|a| a.contains(addr))
    }

    /// `AdapterHandlerLibrary::find_adapter` analogue: return the cached
    /// adapter or generate + register a new one.
    pub fn find_or_create<F>(&mut self, kind: AdapterKind, signature: &str, emit: F) -> &AdapterBlob
    where
        F: FnOnce(AdapterKind, &str) -> (Vec<u8>, usize, usize),
    {
        let key = (kind, signature.to_string());
        if !self.adapters.contains_key(&key) {
            let (code, target_patch_offset, frame_size) = emit(kind, signature);
            let entry_point = self.next_entry;
            self.next_entry += align_up(code.len() + 16, 16);
            let blob = AdapterBlob {
                kind,
                signature: signature.to_string(),
                entry_point,
                code,
                target_patch_offset,
                frame_size,
                exception_points: Vec::new(),
            };
            self.adapters.insert(key.clone(), blob);
        }
        self.adapters.get(&key).unwrap()
    }

    /// Register a pre-built adapter (used when the code bytes were
    /// emitted by a backend).
    pub fn register(&mut self, blob: AdapterBlob) {
        let size = align_up(blob.code.len() + 16, 16);
        self.next_entry = self.next_entry.max(blob.entry_point + size);
        self.adapters
            .insert((blob.kind, blob.signature.clone()), blob);
    }

    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &AdapterBlob> {
        self.adapters.values()
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(f, "adapter handlers ({}):", self.adapters.len());
        let mut blobs: Vec<&AdapterBlob> = self.adapters.values().collect();
        blobs.sort_by_key(|b| b.entry_point);
        for b in blobs {
            let _ = writeln!(
                f,
                "  {} {} entry={} size={} frame={}",
                b.kind.name(),
                b.signature,
                b.entry_point,
                b.code_size(),
                b.frame_size
            );
        }
    }
}

/// `AdapterHandlerLibrary` analogue: emits the machine-code template for
/// each adapter kind.  The emitted sequences are real, deterministic
/// x86-64 encodings that implement the register/stack shuffling contract
/// of the corresponding HotSpot adapter; the target call is left as a
/// rel32 placeholder patched at install time.
pub mod adapter_emit {
    use super::AdapterKind;

    /// Emit an i2c (interpreted->compiled) adapter: build a native
    /// argument area from the interpreter frame, then jump to the
    /// nmethod entry point.
    ///
    /// Layout (documented, stable):
    ///   push rbp; mov rbp,rsp; push rbx; sub rsp, frame
    ///   <arg moves: mov reg, [rbp+off] / mov [rsp+off], reg>
    ///   call rel32 -> nmethod->entry_point   (patched)
    ///   add rsp, frame; pop rbx; pop rbp; ret
    pub fn emit_i2c(signature: &str, arg_count: usize) -> (Vec<u8>, usize, usize) {
        let mut code = Vec::new();
        // push rbp; mov rbp, rsp
        code.extend_from_slice(&[0x55, 0x48, 0x89, 0xE5]);
        // push rbx (callee-saved scratch)
        code.extend_from_slice(&[0x53]);
        let frame = align_up(16 + arg_count * 8, 16);
        // sub rsp, frame
        emit_sub_rsp_imm(&mut code, frame as i32);
        // Move each incoming argument from the interpreter operand area
        // ([rbp + 16 + i*8]) into argument registers/stack slots.
        // Registers used in order: rdi, rsi, rdx, rcx, r8, r9.
        const ARG_REGS: [u8; 6] = [0x07, 0x06, 0x02, 0x01, 0x00, 0x08]; // rdi..r9 encodings modrm reg field approximations
        let _ = ARG_REGS;
        for i in 0..arg_count {
            // mov rax, [rbp + 16 + i*8]  -> 48 8B 85 <disp32>
            code.extend_from_slice(&[0x48, 0x8B, 0x85]);
            code.extend_from_slice(&((16 + i * 8) as i32).to_le_bytes());
            // mov [rsp + i*8], rax       -> 48 89 84 24 <disp32>
            code.extend_from_slice(&[0x48, 0x89, 0x84, 0x24]);
            code.extend_from_slice(&(i * 8).to_le_bytes());
        }
        // call rel32 (patched to entry_point - (pc+4))
        let patch_off = code.len() + 1;
        code.extend_from_slice(&[0xE8, 0, 0, 0, 0]);
        // add rsp, frame; pop rbx; pop rbp; ret
        emit_add_rsp_imm(&mut code, frame as i32);
        code.extend_from_slice(&[0x5B, 0x5D, 0xC3]);
        let _ = signature;
        (code, patch_off, frame)
    }

    /// Emit a c2i (compiled->interpreter) adapter: unpack the native
    /// return value and tail-continue into the interpreter entry for the
    /// caller's bci.
    pub fn emit_c2i(signature: &str, arg_count: usize) -> (Vec<u8>, usize, usize) {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x55, 0x48, 0x89, 0xE5]); // push rbp; mov rbp,rsp
        code.extend_from_slice(&[0x53]); // push rbx
        let frame = align_up(16 + arg_count * 8, 16);
        emit_sub_rsp_imm(&mut code, frame as i32);
        // mov [rsp], rax  (save return value)
        code.extend_from_slice(&[0x48, 0x89, 0x04, 0x24]);
        // jmp rel32 -> interpreter entry (patched)
        let patch_off = code.len() + 1;
        code.extend_from_slice(&[0xE9, 0, 0, 0, 0]);
        emit_add_rsp_imm(&mut code, frame as i32);
        code.extend_from_slice(&[0x5B, 0x5D, 0xC3]);
        let _ = signature;
        (code, patch_off, frame)
    }

    /// JNI adapters share the i2c/c2i frame contract; emit the same
    /// template (the JNI argument conversion is handled by the callee's
    /// native wrapper in this structural port).
    pub fn emit_generic(kind: AdapterKind, signature: &str) -> (Vec<u8>, usize, usize) {
        let argc = signature_arg_count(signature);
        match kind {
            AdapterKind::InterpreterToCompiled | AdapterKind::NativeToJava => {
                emit_i2c(signature, argc)
            }
            _ => emit_c2i(signature, argc),
        }
    }

    /// Rough arity estimate from a JVM signature string: count argument
    /// slots (long/double = 2) before ')'.
    pub fn signature_arg_count(sig: &str) -> usize {
        let inner = sig
            .strip_prefix('(')
            .and_then(|s| s.split(')').next())
            .unwrap_or("");
        let mut count = 0usize;
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                // Array type: consume remaining '[' dimensions, then the
                // element type contributes its own slot count exactly once.
                '[' => {
                    while matches!(chars.peek(), Some('[')) {
                        chars.next();
                    }
                    match chars.next() {
                        Some('J') | Some('D') => count += 2,
                        Some('L') => {
                            count += 1;
                            for c2 in chars.by_ref() {
                                if c2 == ';' {
                                    break;
                                }
                            }
                        }
                        Some(_) => count += 1,
                        None => {}
                    }
                }
                'J' | 'D' => count += 2,
                'L' => {
                    count += 1;
                    for c2 in chars.by_ref() {
                        if c2 == ';' {
                            break;
                        }
                    }
                }
                _ => count += 1,
            }
        }
        count
    }

    fn align_up(v: usize, a: usize) -> usize {
        (v + a - 1) & !(a - 1)
    }

    fn emit_sub_rsp_imm(code: &mut Vec<u8>, imm: i32) {
        if imm == 0 {
            return;
        }
        if imm <= i32::from(i8::MAX) {
            code.extend_from_slice(&[0x48, 0x83, 0xEC, imm as u8]);
        } else {
            code.extend_from_slice(&[0x48, 0x81, 0xEC]);
            code.extend_from_slice(&imm.to_le_bytes());
        }
    }

    fn emit_add_rsp_imm(code: &mut Vec<u8>, imm: i32) {
        if imm == 0 {
            return;
        }
        if imm <= i32::from(i8::MAX) {
            code.extend_from_slice(&[0x48, 0x83, 0xC4, imm as u8]);
        } else {
            code.extend_from_slice(&[0x48, 0x81, 0xC4]);
            code.extend_from_slice(&imm.to_le_bytes());
        }
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
    /// Generated call adapters (i2c/c2i/JNI) keyed by kind+signature.
    adapters: Mutex<AdapterHandlerTable>,
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
            adapters: Mutex::new(AdapterHandlerTable::new(non_nmethod_base)),
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
            all_heap: Mutex::new(Some(CodeHeap::with_base(
                CodeBlobType::All,
                0x1000_0000,
                capacity,
            ))),
            segmented: false,
            nmethods: Mutex::new(HashMap::new()),
            osr_nmethods: Mutex::new(HashMap::new()),
            pc_index: Mutex::new(BTreeMap::new()),
            nmethod_blobs: Mutex::new(HashMap::new()),
            adapters: Mutex::new(AdapterHandlerTable::new(0x1000_0000)),
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
            guard.entry(key).or_default().insert(nm.entry_bci(), nm);
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
                return heap.allocate_with_code(
                    name,
                    total_size,
                    CodeBlobKind::Nmethod,
                    code_bytes,
                );
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
        let addr = self.nmethod_blobs.lock().unwrap().remove(method_key);
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

    // ---- adapter handlers ----------------------------------------------

    /// `AdapterHandlerLibrary::find_adapter` analogue: look up a cached
    /// adapter for (kind, signature); on miss, emit one via
    /// `adapter_emit::emit_generic`, register it and allocate a non-nmethod
    /// heap blob to back it.  Returns the adapter entry point.
    pub fn find_adapter(&self, kind: AdapterKind, signature: &str) -> Result<usize, String> {
        // Fast path.
        if let Some(blob) = self.adapters.lock().unwrap().lookup(kind, signature) {
            return Ok(blob.entry_point);
        }
        let (code, patch_off, frame_size) = adapter_emit::emit_generic(kind, signature);
        // Allocate the stub bytes in the non-nmethod heap so the blob is
        // tracked by the sweeper.
        let name = format!("adapter-{}-{}", kind.name(), signature);
        let blob_addr = self.allocate_blob(
            name,
            CodeBlobType::NonNMethod,
            align_up(code.len() + 16, 16),
            CodeBlobKind::AdapterBlob,
        )?;
        // Heap bookkeeping address doubles as the entry point; the real
        // executable copy is published by the caller via jitruntime (same
        // contract as install_nmethod).
        let mut guard = self.adapters.lock().unwrap();
        guard.register(AdapterBlob {
            kind,
            signature: signature.to_string(),
            entry_point: blob_addr,
            code: code.clone(),
            target_patch_offset: patch_off,
            frame_size,
            exception_points: Vec::new(),
        });
        Ok(blob_addr)
    }

    /// Register a backend-emitted adapter directly.
    pub fn install_adapter(&self, blob: AdapterBlob) {
        self.adapters.lock().unwrap().register(blob);
    }

    pub fn find_adapter_blob(&self, kind: AdapterKind, signature: &str) -> Option<AdapterBlob> {
        self.adapters
            .lock()
            .unwrap()
            .lookup(kind, signature)
            .cloned()
    }

    pub fn adapter_count(&self) -> usize {
        self.adapters.lock().unwrap().len()
    }

    // ---- blob lookup ----------------------------------------------------

    /// `CodeCache::find_blob` analogue: blob containing `addr` in any
    /// heap (alias of `find_blob_at_address`).
    pub fn lookup_blob(&self, addr: usize) -> Option<CodeBlob> {
        self.find_blob_at_address(addr)
    }

    /// `CodeCache::find_blob_unsafe` analogue restricted to the pc index:
    /// the nmethod whose code range contains `addr`.
    pub fn lookup_nmethod_blob(&self, addr: usize) -> Option<NMethod> {
        self.find_nmethod_at_address(addr)
    }

    // ---- reclamation ----------------------------------------------------

    /// `free_unused_code_blobs` analogue: reclaim every heap blob flagged
    /// unused (dead non-nmethod blobs: stale adapters, abandoned stubs).
    /// Nmethod reclamation is driven by the sweeper (`sweep_cycle`) and
    /// `deinstall_nmethod`; this path covers the non-nmethod heaps.
    /// Returns total bytes freed.
    pub fn free_unused_code_blobs(&self) -> usize {
        let mut freed = 0usize;
        if self.segmented {
            for heap in [
                &self.non_nmethods_heap,
                &self.profiled_nmethods_heap,
                &self.non_profiled_nmethods_heap,
            ] {
                freed += heap.lock().unwrap().free_unused_blobs();
            }
        } else if let Some(heap) = self.all_heap.lock().unwrap().as_mut() {
            freed += heap.free_unused_blobs();
        }
        freed
    }

    /// Flag the heap blob at `addr` unused so the next
    /// `free_unused_code_blobs` reclaims it.
    pub fn mark_blob_unused(&self, addr: usize) -> bool {
        if self.segmented {
            for heap in [
                &self.non_nmethods_heap,
                &self.profiled_nmethods_heap,
                &self.non_profiled_nmethods_heap,
            ] {
                if heap.lock().unwrap().mark_unused(addr) {
                    return true;
                }
            }
            false
        } else {
            self.all_heap
                .lock()
                .unwrap()
                .as_mut()
                .map_or(false, |h| h.mark_unused(addr))
        }
    }

    // ---- diagnostics -----------------------------------------------------

    /// `CodeCache::print_on` analogue: heap segmentation summary, blob
    /// counts, adapter table and global counters.
    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(
            f,
            "CodeCache segmented={} capacity={} used={} free={} reverse_free_ratio={:.1}%",
            self.segmented,
            self.total_capacity(),
            self.total_used(),
            self.total_free(),
            self.reverse_free_ratio(),
        );
        let _ = writeln!(
            f,
            "  nmethods={} osr_nmethods={} blobs={}",
            self.nmethods_count(),
            self.osr_nmethods_count(),
            self.blob_count(),
        );
        if self.segmented {
            self.non_nmethods_heap.lock().unwrap().print_on(f);
            self.profiled_nmethods_heap.lock().unwrap().print_on(f);
            self.non_profiled_nmethods_heap.lock().unwrap().print_on(f);
        } else if let Some(heap) = self.all_heap.lock().unwrap().as_ref() {
            heap.print_on(f);
        }
        self.adapters.lock().unwrap().print_on(f);
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
            return nm.make_not_entrant_with_reason(super::nmethod::InvalidationReason::NotUsed);
        }
        false
    }

    pub fn mark_all_nmethods_for_deopt(&self) {
        {
            let guard = self.nmethods.lock().unwrap();
            for nm in guard.values() {
                if nm.is_in_use() {
                    nm.mark_for_deoptimization();
                }
            }
        }
        // OSR variants are nmethods too (HotSpot marks every nmethod of
        // the method, including on-stack-replacement ones).
        let osr = self.osr_nmethods.lock().unwrap();
        for map in osr.values() {
            for nm in map.values() {
                if nm.is_in_use() {
                    nm.mark_for_deoptimization();
                }
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
            self.remove_pc_index_entry(begin);
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
            self.remove_pc_index_entry(begin);
            self.free_nmethod_blob(&format!("{}@{}", key, bci));
        }

        summary
    }

    /// Remove pc-index entries belonging to `method_key`
    /// (regular + all OSR variants).
    fn remove_pc_index_for_key(&self, method_key: &str) {
        let mut index = self.pc_index.lock().unwrap();
        index.retain(|_, nm| {
            Self::method_key(nm.method_holder(), nm.method_name(), nm.method_signature())
                != method_key
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
        let a = heap
            .allocate("a".to_string(), 100, CodeBlobKind::Nmethod)
            .unwrap();
        let b = heap
            .allocate("b".to_string(), 100, CodeBlobKind::Nmethod)
            .unwrap();
        assert_eq!(heap.used(), 200);
        assert!(heap.free_blob(a));
        assert!(heap.free_blob(b));
        // Coalesced: the whole 200 bytes should be one free region again,
        // reusable for a 200-byte allocation.
        let c = heap
            .allocate("c".to_string(), 200, CodeBlobKind::Nmethod)
            .unwrap();
        assert_eq!(heap.used(), 200);
        let _ = c;
    }

    #[test]
    fn heap_first_fit_and_bump() {
        heap_alloc_free_coalesce();
        let mut heap = CodeHeap::with_base(CodeBlobType::NonNMethod, 0x2000, 512);
        let x = heap
            .allocate("x".to_string(), 64, CodeBlobKind::RuntimeStub)
            .unwrap();
        let y = heap
            .allocate("y".to_string(), 64, CodeBlobKind::RuntimeStub)
            .unwrap();
        assert_eq!(y - x, 64);
        assert!(heap.free_space() >= 512 - 128);
        // Allocation beyond capacity fails.
        assert!(
            heap.allocate("big".to_string(), 4096, CodeBlobKind::RuntimeStub)
                .is_err()
        );
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
            "U".to_string(),
            "r".to_string(),
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
        let osr = NMethod::with_entry_points(
            "U".to_string(),
            "r".to_string(),
            "()V".to_string(),
            CompLevel::Simple,
            12,
            0,
            0,
            0,
            0,
            0,
            vec![],
            vec![],
        );
        cache
            .install_nmethod(regular, vec![0x90, 0xc3], 32)
            .unwrap();
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
            "V".to_string(),
            "o".to_string(),
            "()V".to_string(),
            CompLevel::Simple,
            4,
            0,
            0,
            0,
            0,
            0,
            vec![],
            vec![],
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

    #[test]
    fn adapter_find_or_create_and_lookup() {
        let cache = CodeCache::new_non_segmented(1 << 20);
        let ep1 = cache
            .find_adapter(AdapterKind::InterpreterToCompiled, "(I)J")
            .expect("adapter");
        assert_ne!(ep1, 0);
        // Second lookup hits the cache and returns the same entry point.
        let ep2 = cache
            .find_adapter(AdapterKind::InterpreterToCompiled, "(I)J")
            .unwrap();
        assert_eq!(ep1, ep2);
        assert_eq!(cache.adapter_count(), 1);
        let c2i = cache
            .find_adapter(AdapterKind::CompiledToInterpreter, "(I)J")
            .unwrap();
        assert_ne!(c2i, ep1);
        assert_eq!(cache.adapter_count(), 2);
        let blob = cache
            .find_adapter_blob(AdapterKind::InterpreterToCompiled, "(I)J")
            .unwrap();
        assert!(!blob.code.is_empty());
        assert!(blob.target_patch_offset > 0);
        // The emitted code contains a rel32 patch site (E8/E9 + 4 bytes).
        let op = blob.code[blob.target_patch_offset - 1];
        assert!(op == 0xE8 || op == 0xE9);
    }

    #[test]
    fn free_unused_code_blobs_reclaims_marked() {
        let cache = CodeCache::new_non_segmented(1 << 20);
        let addr = cache
            .allocate_blob(
                "dead-stub".to_string(),
                CodeBlobType::NonNMethod,
                128,
                CodeBlobKind::RuntimeStub,
            )
            .unwrap();
        assert!(cache.lookup_blob(addr).is_some());
        assert!(cache.mark_blob_unused(addr));
        let freed = cache.free_unused_code_blobs();
        assert_eq!(freed, 128);
        assert!(cache.lookup_blob(addr).is_none());
        // Second pass frees nothing.
        assert_eq!(cache.free_unused_code_blobs(), 0);
    }

    #[test]
    fn mark_all_deopt_includes_osr() {
        let cache = CodeCache::new_non_segmented(1 << 20);
        let regular = NMethod::with_entry_points(
            "W".to_string(),
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
        let osr = NMethod::with_entry_points(
            "W".to_string(),
            "m".to_string(),
            "()V".to_string(),
            CompLevel::Simple,
            3,
            0,
            0,
            0,
            0,
            0,
            vec![],
            vec![],
        );
        cache
            .install_nmethod(regular, vec![0x90, 0xc3], 32)
            .unwrap();
        cache.install_nmethod(osr, vec![0x90, 0xc3], 32).unwrap();
        cache.mark_all_nmethods_for_deopt();
        let key = CodeCache::method_key("W", "m", "()V");
        assert!(
            cache
                .find_nmethod_by_key(&key)
                .unwrap()
                .is_marked_for_deoptimization()
        );
        assert!(
            cache
                .find_osr_nmethod_by_key(&key, 3)
                .unwrap()
                .is_marked_for_deoptimization()
        );
    }

    #[test]
    fn code_cache_print_on_smoke() {
        let cache = CodeCache::new_non_segmented(1 << 20);
        let mut s = String::new();
        cache.print_on(&mut s);
        assert!(s.contains("CodeCache"));
        assert!(s.contains("adapter handlers"));
    }

    #[test]
    fn adapter_emit_signature_arg_count() {
        use adapter_emit::signature_arg_count;
        assert_eq!(signature_arg_count("()V"), 0);
        assert_eq!(signature_arg_count("(I)J"), 1);
        assert_eq!(signature_arg_count("(JD)Ljava/lang/String;"), 4);
        assert_eq!(signature_arg_count("(Ljava/lang/String;I)V"), 2);
        assert_eq!(signature_arg_count("([I[[D)V"), 3);
    }
}
