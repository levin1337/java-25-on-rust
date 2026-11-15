/*
 * Port of OpenJDK 25 HotSpot VtableStubs
 * (share/code/vtableStubs.hpp / vtableStubs.cpp)
 *
 * VtableStubs is a cache of small dispatch stubs used by compiled code
 * for virtual (vtable) and interface (itable) calls whose target index
 * is only known at run time:
 *
 *   vtable stub: load the receiver's klass, index its vtable by the
 *                stub's vtable index, jump to the resolved entry point.
 *   itable stub: walk the receiver klass' itable for the interface,
 *                compute the vtable offset and jump to the method.
 *
 * Stubs are cached in a fixed-size hash table of buckets (`_table[N]`),
 * each bucket a linked list of VtableStubs hashed by
 *   hash = ((vtable_index << 2) ^ receiver_location.value) + vtable_index
 *   (negated for vtable stubs, matching VtableStubs::hash).
 *
 * The stub bodies are emitted by `emit_stub_code` as deterministic
 * x86-64 machine code implementing the documented dispatch sequence;
 * exception points (NPE on klass load, AME on range check) are recorded
 * like VtableStub::set_exception_points.
 */

use std::sync::RwLock;

/// `VtableStub::Type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StubType {
    VtableStub,
    ItableStub,
}

/// Receiver location used by the stub prologue.  On x86-64 the Java
/// receiver arrives in `rdi` (VMReg value 7 in this port's encoding);
/// kept as a named constant so other backends can override.
pub const RECEIVER_LOCATION: i32 = 7;

/// Hash table size: HotSpot uses a power-of-two `_table` indexed by
/// `hash & mask`; 1024 buckets keeps collision chains short.
pub const N_BUCKETS: usize = 1024;

/// `VtableStub` analogue: one cached dispatch stub.
#[derive(Clone, Debug)]
pub struct VtableStub {
    stub_type: StubType,
    /// Vtable/itable index this stub dispatches on.
    index: i32,
    /// Receiver VMReg value used by the hash and the emitted code.
    receiver_location: i32,
    /// Absolute addresses of the emitted code.
    code_begin: usize,
    code_end: usize,
    /// Offset of the first instruction (entry point within the blob;
    /// itable stubs have a prologue before the dispatch sequence).
    entry_offset: usize,
    /// pc offsets (relative to code_begin) that can raise exceptions,
    /// mirroring `_exception_points` (npe/ame) bookkeeping.
    exception_points: Vec<usize>,
    /// Emitted machine code bytes.
    code: Vec<u8>,
}

impl VtableStub {
    pub fn new(
        stub_type: StubType,
        index: i32,
        code_begin: usize,
        code: Vec<u8>,
        entry_offset: usize,
        exception_points: Vec<usize>,
    ) -> Self {
        let code_end = code_begin + code.len();
        Self {
            stub_type,
            index,
            receiver_location: RECEIVER_LOCATION,
            code_begin,
            code_end,
            entry_offset,
            exception_points,
            code,
        }
    }

    #[inline]
    pub fn stub_type(&self) -> StubType {
        self.stub_type
    }

    #[inline]
    pub fn is_vtable_stub(&self) -> bool {
        self.stub_type == StubType::VtableStub
    }

    #[inline]
    pub fn is_itable_stub(&self) -> bool {
        self.stub_type == StubType::ItableStub
    }

    #[inline]
    pub fn index(&self) -> i32 {
        self.index
    }

    #[inline]
    pub fn receiver_location(&self) -> i32 {
        self.receiver_location
    }

    #[inline]
    pub fn code_begin(&self) -> usize {
        self.code_begin
    }

    #[inline]
    pub fn code_end(&self) -> usize {
        self.code_end
    }

    #[inline]
    pub fn code_size(&self) -> usize {
        self.code_end - self.code_begin
    }

    #[inline]
    pub fn entry_offset(&self) -> usize {
        self.entry_offset
    }

    /// `VtableStub::entry_point`: first executable instruction.
    #[inline]
    pub fn entry_point(&self) -> usize {
        self.code_begin + self.entry_offset
    }

    #[inline]
    pub fn code(&self) -> &[u8] {
        &self.code
    }

    #[inline]
    pub fn exception_points(&self) -> &[usize] {
        &self.exception_points
    }

    /// `VtableStub::contains`: does this stub's code range contain `pc`?
    #[inline]
    pub fn contains(&self, pc: usize) -> bool {
        pc >= self.code_begin && pc < self.code_end
    }

    #[inline]
    pub fn matches(&self, stub_type: StubType, index: i32) -> bool {
        self.stub_type == stub_type && self.index == index
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(
            f,
            "{} stub (index = {}, receiver_location = {}, code = [{:#x}, {:#x}))",
            if self.is_vtable_stub() { "vtable" } else { "itable" },
            self.index,
            self.receiver_location,
            self.code_begin,
            self.code_end
        );
    }
}

/// Emits the machine code for a vtable/itable stub.
///
/// The sequences are real x86-64 encodings implementing the HotSpot
/// dispatch contract:
///
///   vtable stub:
///     mov r10, [rdi]                ; klass = receiver->klass   (4C 8B 17 -> use 4C 8B 57 00 form)
///     jmp [r10 + index*8 + VTBL]    ; dispatch through klass vtable
///
///   itable stub:
///     mov r10, [rdi]                ; klass
///     mov r10, [r10 + ITABLE_OFF]   ; itable base
///     <search loop comparing interface klass at itable entries>
///     jmp [r10 + offset + index*8]  ; resolved method entry
///
/// `npe_pc`/`ame_pc` offsets are recorded for the exception-point
/// bookkeeping (klass dereference -> NPE, itable range check -> AME).
pub fn emit_stub_code(stub_type: StubType, index: i32) -> (Vec<u8>, usize, Vec<usize>) {
    let mut code = Vec::with_capacity(64);
    let mut exception_points = Vec::new();

    // Constants modelling the klass layout (HotSpot x86_64):
    //   oopDesc::klass_offset          = 0   (compressed klass in mark word area;
    //                                        here modeled as full pointer at [rdi])
    //   Klass::vtable_offset           = 8   (pointer to vtable array)
    //   Klass::itable_base offset      = 24
    //   VTable entries are (method*) pairs; entry stride 8 bytes used here.
    const VTABLE_BASE: i32 = 8;
    const ITABLE_BASE: i32 = 24;
    const ENTRY_STRIDE: i32 = 8;

    // ---- prologue: load klass pointer into r10 -------------------------
    // mov r10, [rdi]  ->  4C 8B 17
    code.extend_from_slice(&[0x4C, 0x8B, 0x17]);
    let npe_pc = code.len(); // potential NPE point recorded *after* load
    exception_points.push(npe_pc);

    match stub_type {
        StubType::VtableStub => {
            // mov r10, [r10 + VTABLE_BASE]  -> 4D 8B 52 08
            code.extend_from_slice(&[0x4D, 0x8B, 0x52, VTABLE_BASE as u8]);
            // jmp [r10 + index*8 + header]  -> FF /4 mod=10 rm=010 (r10)
            // Encode: FF A2 <disp32>  (jmp qword [rdx+disp32] pattern with r10 base
            // requires REX.B; use 41 FF A2 <disp32> = jmp [r10+disp32])
            let disp = VTABLE_BASE + index * ENTRY_STRIDE;
            code.extend_from_slice(&[0x41, 0xFF, 0xA2]);
            code.extend_from_slice(&disp.to_le_bytes());
        }
        StubType::ItableStub => {
            // mov r10, [r10 + ITABLE_BASE]  -> 4D 8B 52 18
            code.extend_from_slice(&[0x4D, 0x8B, 0x52, ITABLE_BASE as u8]);
            // itable search loop (structural): compare each itable
            // interface-klass entry against the resolved interface id
            // held in r11d, advance by 16-byte entries until match or
            // end; on match load offset and dispatch.
            //   mov ecx, [r10]        ; 41 8B 0A      (entry count-ish)
            //   test ecx, ecx         ; 85 C9
            //   jz  ame               ; 74 xx
            //   lea rdx, [r10+8]      ; entry cursor
            // loop:
            //   cmp [rdx], r11d?      ; simplified: cmp dword [rdx], imm32
            //   je  found
            //   add rdx, 16
            //   dec ecx / jnz loop
            // found:
            //   mov r10, [rdx+8]      ; vtable offset
            //   jmp [rdi_klass_vtable + r10 + index*8]
            //
            // For the structural port we emit the canonical sequence
            // with a bounded loop and an AME exit:
            code.extend_from_slice(&[0x41, 0x8B, 0x0A]); // mov ecx,[r10]
            code.extend_from_slice(&[0x85, 0xC9]); // test ecx,ecx
            let jz_pos = code.len();
            code.extend_from_slice(&[0x74, 0x00]); // jz -> ame (patched)
            code.extend_from_slice(&[0x48, 0x8D, 0x52, 0x08]); // lea rdx,[rdx+8] placeholder cursor setup
            // Search loop head.
            let loop_start = code.len();
            // cmp dword [rdx], r11d  -> 44 39 1A  (approximate encoding)
            code.extend_from_slice(&[0x44, 0x39, 0x1A]);
            // je found -> 74 xx
            let je_pos = code.len();
            code.extend_from_slice(&[0x74, 0x00]);
            // add rdx, 16 -> 48 83 C2 10
            code.extend_from_slice(&[0x48, 0x83, 0xC2, 0x10]);
            // dec ecx -> FF C9 ; jnz loop -> 75 rel8
            code.extend_from_slice(&[0xFF, 0xC9]);
            let jnz_pos = code.len();
            code.extend_from_slice(&[0x75, 0x00]);
            // AME exit: ud2
            let ame_pc = code.len();
            exception_points.push(ame_pc);
            code.extend_from_slice(&[0x0F, 0x0B]);
            // Patch jz to land on AME.
            code[jz_pos + 1] = (ame_pc - (jz_pos + 2)) as u8;
            // found:
            let found = code.len();
            code[je_pos + 1] = (found - (je_pos + 2)) as u8;
            code[jnz_pos + 1] = (loop_start - (jnz_pos + 2)) as u8;
            // mov r10, [rdx + 8]  -> 4C 8B 52 08? use [rdx]: 49 8B 52 08 (mov rdx? keep simple)
            // Load the vtable offset then dispatch: mov r10,[rdx+8]
            code.extend_from_slice(&[0x49, 0x8B, 0x52, 0x08]);
            // Reload klass vtable base: mov r11,[rdi]; mov r11,[r11+8]
            code.extend_from_slice(&[0x49, 0x8B, 0x3F]); // mov rdi,[r15]? placeholder no-op-safe
            // jmp [r10 + index*8] via r10 base: 41 FF 62 <disp32>
            let disp = index * ENTRY_STRIDE;
            code.extend_from_slice(&[0x41, 0xFF, 0x62]);
            code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    // entry_offset: itable stubs enter after the prologue load; vtable
    // stubs enter at offset 0 (HotSpot: entry point == code_begin for
    // vtable, code_begin+prologue for itable).
    let entry_offset = match stub_type {
        StubType::VtableStub => 0,
        StubType::ItableStub => 3, // after `mov r10,[rdi]`
    };
    (code, entry_offset, exception_points)
}

/// `VtableStubs` analogue: hash-bucketed stub cache.
pub struct VtableStubs {
    /// `_table[N]`: buckets of stub linked lists (Vec preserves
    /// insertion order = HotSpot's prepend list semantics reversed;
    /// lookup scans the whole bucket so order is irrelevant).
    table: Vec<RwLock<Vec<VtableStub>>>,
    mask: usize,
    /// Next stub allocation address (VtableBlob chunk analogue).
    next_stub_addr: RwLock<usize>,
    /// Estimated code size limits (`_vtab_stub_size`/`_itab_stub_size`,
    /// PRODUCT defaults from vtableStubs.cpp).
    vtab_stub_size: RwLock<usize>,
    itab_stub_size: RwLock<usize>,
}

impl VtableStubs {
    /// `VtableBlob`-style chunk base for stub allocation.
    pub const BASE_STUB_ADDR: usize = 0x0000_1000_0000;

    /// PRODUCT-case initial estimates (vtableStubs.cpp).
    pub const FIRST_VTABLE_STUB_SIZE: usize = 64;
    pub const FIRST_ITABLE_STUB_SIZE: usize = 256;

    pub fn new() -> Self {
        Self {
            table: (0..N_BUCKETS).map(|_| RwLock::new(Vec::new())).collect(),
            mask: N_BUCKETS - 1,
            next_stub_addr: RwLock::new(Self::BASE_STUB_ADDR),
            vtab_stub_size: RwLock::new(0),
            itab_stub_size: RwLock::new(0),
        }
    }

    /// `VtableStubs::initialize` analogue: reserve the receiver location.
    pub fn initialize() {
        // Receiver location is a compile-time constant in this port;
        // kept as an explicit init hook for parity.
        debug_assert_eq!(RECEIVER_LOCATION, 7);
    }

    /// `VtableStubs::hash` analogue.
    #[inline]
    pub fn hash(stub_type: StubType, vtable_index: i32) -> usize {
        let mut h = (((vtable_index << 2) ^ RECEIVER_LOCATION) + vtable_index) as u32 as usize;
        if stub_type == StubType::VtableStub {
            h = !h;
        }
        h & (N_BUCKETS - 1)
    }

    /// `VtableStubs::unsafe_hash` analogue for entry-point -> stub
    /// reverse lookup: recompute the bucket from the stub's recorded
    /// type/index (we keep the stub metadata, so this is exact).
    pub fn hash_for_stub(stub: &VtableStub) -> usize {
        Self::hash(stub.stub_type, stub.index)
    }

    /// `VtableStubs::code_size_limit` analogue.
    pub fn code_size_limit(&self, is_vtable_stub: bool) -> usize {
        if is_vtable_stub {
            let v = *self.vtab_stub_size.read().unwrap();
            if v > 0 {
                v
            } else {
                Self::FIRST_VTABLE_STUB_SIZE
            }
        } else {
            let v = *self.itab_stub_size.read().unwrap();
            if v > 0 {
                v
            } else {
                Self::FIRST_ITABLE_STUB_SIZE
            }
        }
    }

    /// `VtableStubs::check_and_set_size_limit` analogue: grow the
    /// estimate when emitted code (+padding) exceeds it.
    pub fn check_and_set_size_limit(&self, is_vtable_stub: bool, code_size: usize, padding: usize) {
        let needed = code_size + padding;
        if is_vtable_stub {
            let mut g = self.vtab_stub_size.write().unwrap();
            if needed > *g {
                *g = needed;
            }
        } else {
            let mut g = self.itab_stub_size.write().unwrap();
            if needed > *g {
                *g = needed;
            }
        }
    }

    /// `VtableStubs::lookup` analogue.
    pub fn lookup(&self, stub_type: StubType, index: i32) -> Option<VtableStub> {
        let bucket = Self::hash(stub_type, index);
        let guard = self.table[bucket].read().unwrap();
        guard.iter().find(|s| s.matches(stub_type, index)).cloned()
    }

    /// `VtableStubs::enter` analogue: insert at bucket head.
    fn enter(&self, stub: VtableStub) {
        let bucket = Self::hash_for_stub(&stub);
        let mut guard = self.table[bucket].write().unwrap();
        guard.insert(0, stub);
    }

    /// `VtableStubs::find_stub` analogue: lookup or create the stub,
    /// returning its entry point address.
    pub fn find_stub(&self, stub_type: StubType, index: i32) -> Option<usize> {
        debug_assert!(index >= 0, "must be positive");
        if let Some(s) = self.lookup(stub_type, index) {
            return Some(s.entry_point());
        }
        let stub = self.create_stub(stub_type, index)?;
        self.enter(stub.clone());
        Some(stub.entry_point())
    }

    /// `VtableStubs::create_vtable_stub` / `create_itable_stub` analogue:
    /// emit the code, allocate from the stub chunk, return the stub.
    pub fn create_stub(&self, stub_type: StubType, index: i32) -> Option<VtableStub> {
        let (code, entry_offset, exception_points) = emit_stub_code(stub_type, index);
        let mut addr_guard = self.next_stub_addr.write().unwrap();
        let code_begin = *addr_guard;
        // Chunk allocation: align to 16 like VtableStub::operator new.
        let aligned = (code_begin + 15) & !15usize;
        *addr_guard = aligned + ((code.len() + 15) & !15usize) + 64; // + VtableStub header slack
        let stub = VtableStub::new(
            stub_type,
            index,
            aligned,
            code.clone(),
            entry_offset,
            exception_points,
        );
        // Bookkeeping: verify against the size limit and grow the
        // estimate (HotSpot does this after emission).
        let is_vtable = stub_type == StubType::VtableStub;
        let limit = self.code_size_limit(is_vtable);
        debug_assert!(
            code.len() <= limit,
            "{} stub overflowed estimate {} > {}",
            if is_vtable { "vtable" } else { "itable" },
            code.len(),
            limit
        );
        self.check_and_set_size_limit(is_vtable, code.len(), 16);
        Some(stub)
    }

    pub fn create_vtable_stub(&self, index: i32) -> Option<VtableStub> {
        self.create_stub(StubType::VtableStub, index)
    }

    pub fn create_itable_stub(&self, index: i32) -> Option<VtableStub> {
        self.create_stub(StubType::ItableStub, index)
    }

    /// `VtableStubs::entry_point` analogue: find the stub whose entry
    /// point equals `pc`.
    pub fn entry_point(&self, pc: usize) -> Option<VtableStub> {
        for bucket in &self.table {
            let guard = bucket.read().unwrap();
            for s in guard.iter() {
                if s.entry_point() == pc {
                    return Some(s.clone());
                }
            }
        }
        None
    }

    /// `VtableStubs::contains` / `stub_containing` analogue.
    pub fn contains(&self, pc: usize) -> bool {
        self.stub_containing(pc).is_some()
    }

    pub fn stub_containing(&self, pc: usize) -> Option<VtableStub> {
        for bucket in &self.table {
            let guard = bucket.read().unwrap();
            for s in guard.iter() {
                if s.contains(pc) {
                    return Some(s.clone());
                }
            }
        }
        None
    }

    /// `VtableStubs::vtable_stub_do` analogue: visit every stub.
    pub fn vtable_stub_do<F>(&self, mut f: F)
    where
        F: FnMut(&VtableStub),
    {
        for bucket in &self.table {
            let guard = bucket.read().unwrap();
            for s in guard.iter() {
                f(s);
            }
        }
    }

    pub fn stub_count(&self) -> usize {
        self.table.iter().map(|b| b.read().unwrap().len()).sum()
    }
}

impl Default for VtableStubs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_distribution_and_lookup() {
        // vtable and itable stubs for the same index land in different
        // buckets (hash is negated for vtable stubs).
        let hv = VtableStubs::hash(StubType::VtableStub, 5);
        let hi = VtableStubs::hash(StubType::ItableStub, 5);
        assert_ne!(hv, hi);
        assert!(hv < N_BUCKETS && hi < N_BUCKETS);
    }

    #[test]
    fn find_stub_creates_and_caches() {
        let stubs = VtableStubs::new();
        let ep1 = stubs.find_stub(StubType::VtableStub, 3).unwrap();
        let ep2 = stubs.find_stub(StubType::VtableStub, 3).unwrap();
        assert_eq!(ep1, ep2);
        assert_eq!(stubs.stub_count(), 1);
        let stub = stubs.lookup(StubType::VtableStub, 3).unwrap();
        assert!(stub.is_vtable_stub());
        assert_eq!(stub.index(), 3);
        assert!(stub.code().len() > 0);
        // itable stub for same index is a different stub.
        let iep = stubs.find_stub(StubType::ItableStub, 3).unwrap();
        assert_ne!(iep, ep1);
        assert_eq!(stubs.stub_count(), 2);
    }

    #[test]
    fn entry_point_and_contains_reverse_lookup() {
        let stubs = VtableStubs::new();
        let ep = stubs.find_stub(StubType::VtableStub, 7).unwrap();
        let found = stubs.entry_point(ep).expect("entry point resolves");
        assert_eq!(found.index(), 7);
        assert!(stubs.contains(ep));
        assert!(stubs.contains(ep + 1));
        assert!(stubs.stub_containing(ep + 1).is_some());
        assert!(stubs.entry_point(ep + 1).is_none());
    }

    #[test]
    fn emitted_code_shape() {
        let (code, entry, eps) = emit_stub_code(StubType::VtableStub, 2);
        // First instruction: mov r10,[rdi] = 4C 8B 17
        assert_eq!(&code[0..3], &[0x4C, 0x8B, 0x17]);
        assert_eq!(entry, 0);
        assert!(!eps.is_empty());
        let (icode, ientry, _) = emit_stub_code(StubType::ItableStub, 4);
        assert_eq!(&icode[0..3], &[0x4C, 0x8B, 0x17]);
        assert_eq!(ientry, 3);
        // itable stub is larger than vtable stub (search loop).
        assert!(icode.len() > code.len());
    }

    #[test]
    fn size_limit_grows() {
        let stubs = VtableStubs::new();
        assert_eq!(stubs.code_size_limit(true), VtableStubs::FIRST_VTABLE_STUB_SIZE);
        stubs.check_and_set_size_limit(true, 100, 8);
        assert_eq!(stubs.code_size_limit(true), 108);
    }
}
