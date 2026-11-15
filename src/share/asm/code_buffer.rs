/*
 * Port of OpenJDK 25 HotSpot share/asm/codeBuffer.hpp
 *
 * CodeBuffer is the growable code emission buffer: it owns the
 * instruction/stub/constant sections, the relocation table, oop maps,
 * scope descriptors and the exception handler table, and can be
 * finalized into a CodeBlob via copy_codes().
 */

use crate::share::asm::reloc::{RelocEntry, RelocError, RelocTable, RelocType, RelocWidth, RelocValue};

/// Code section identifier, mirrors CodeBuffer::SECT_*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SectionId {
    Insts = 0,
    Stubs = 1,
    Consts = 2,
}

impl SectionId {
    pub fn from_code(c: u8) -> Option<SectionId> {
        match c {
            0 => Some(SectionId::Insts),
            1 => Some(SectionId::Stubs),
            2 => Some(SectionId::Consts),
            _ => None,
        }
    }
}

/// One emit section (insts, stubs or consts), mirrors CodeBuffer::CodeSection.
#[derive(Debug)]
pub struct CodeSection {
    /// Raw emitted bytes.  `end` is the current emission point; bytes past
    /// `end` up to capacity are uninitialized (HotSpot limit/start model).
    pub bytes: Vec<u8>,
    pub end: usize,
    /// Inst mark, usually an instruction boundary.
    pub mark: Option<usize>,
    /// Instruction end address used by fattening logic (position of last
    /// emitted instruction end); equals `end` unless set otherwise.
    pub inst_end: Option<usize>,
    /// Relocations recorded while emitting into this section.
    pub relocs: RelocTable,
    /// Scratch emission: buffer is used for scratch emit, don't relocate.
    pub scratch_emit: bool,
    /// Section index (SECT_INSTS/SECT_STUBS/SECT_CONSTS).
    pub index: u8,
    /// Skipped instructions size (for scratch emit alignment).
    pub skipped_instructions_size: usize,
}

impl CodeSection {
    pub fn new(index: u8, capacity: usize) -> Self {
        Self {
            bytes: vec![0u8; capacity],
            end: 0,
            mark: None,
            inst_end: None,
            relocs: RelocTable::new(),
            scratch_emit: false,
            index,
            skipped_instructions_size: 0,
        }
    }

    #[inline]
    pub fn start(&self) -> usize {
        0
    }

    #[inline]
    pub fn end_pos(&self) -> usize {
        self.end
    }

    #[inline]
    pub fn limit(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.end
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.end)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.end == 0
    }

    #[inline]
    pub fn contains(&self, pc: usize) -> bool {
        pc < self.end
    }

    /// Bytes actually emitted.
    pub fn code(&self) -> &[u8] {
        &self.bytes[..self.end]
    }

    /// Ensure `amount` bytes of capacity remain.
    pub fn ensure_capacity(&mut self, amount: usize) {
        if self.remaining() < amount {
            let need = self.end + amount;
            let new_cap = self.bytes.len().max(need).max(need * 2);
            self.bytes.resize(new_cap, 0);
        }
    }

    pub fn emit_u8(&mut self, x: u8) {
        self.ensure_capacity(1);
        self.bytes[self.end] = x;
        self.end += 1;
    }

    pub fn emit_bytes(&mut self, xs: &[u8]) {
        self.ensure_capacity(xs.len());
        self.bytes[self.end..self.end + xs.len()].copy_from_slice(xs);
        self.end += xs.len();
    }

    pub fn emit_u16(&mut self, x: u16) {
        self.emit_bytes(&x.to_le_bytes());
    }

    pub fn emit_u24(&mut self, x: u32) {
        let b = x.to_le_bytes();
        self.emit_bytes(&b[..3]);
    }

    pub fn emit_u32(&mut self, x: u32) {
        self.emit_bytes(&x.to_le_bytes());
    }

    pub fn emit_u64(&mut self, x: u64) {
        self.emit_bytes(&x.to_le_bytes());
    }

    pub fn emit_f32(&mut self, x: f32) {
        self.emit_u32(x.to_bits());
    }

    pub fn emit_f64(&mut self, x: f64) {
        self.emit_u64(x.to_bits());
    }

    /// Patch a little-endian field at absolute section offset `at`.
    pub fn patch(&mut self, at: usize, bytes: &[u8]) -> Result<(), CodeBufferError> {
        if at + bytes.len() > self.end {
            return Err(CodeBufferError::InvalidPatch(at));
        }
        self.bytes[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    /// Read a little-endian field at section offset `at`.
    pub fn read_field(&self, at: usize, len: usize) -> Result<u64, CodeBufferError> {
        if at + len > self.end || len > 8 {
            return Err(CodeBufferError::InvalidRead(at));
        }
        let mut v: u64 = 0;
        for (i, &b) in self.bytes[at..at + len].iter().enumerate() {
            v |= (b as u64) << (8 * i);
        }
        Ok(v)
    }

    pub fn alignment(&self) -> usize {
        // CodeEntryAlignment
        CODE_ENTRY_ALIGNMENT
    }

    /// Align emission point up to `align` boundary with fill byte.
    pub fn align_at(&mut self, align: usize, fill: u8) {
        let target = self.end.div_ceil(align) * align;
        while self.end < target {
            self.emit_u8(fill);
        }
    }

    /// relocate() for this section; appends a reloc entry at pc_offset.
    pub fn add_reloc(
        &mut self,
        pc_offset: u32,
        field_offset: u32,
        rtype: RelocType,
        width: RelocWidth,
        value: RelocValue,
    ) {
        self.relocs.add(RelocEntry { pc_offset, field_offset, rtype, width, value });
    }
}

pub const CODE_ENTRY_ALIGNMENT: usize = 32;

/// Padding used at the end of a buffer (CodeBuffer::end_slop).
pub const END_SLOP: usize = CODE_ENTRY_ALIGNMENT;

#[derive(Debug)]
pub enum CodeBufferError {
    OutOfSpace,
    InvalidSeek(usize),
    InvalidPatch(usize),
    InvalidRead(usize),
    Reloc(RelocError),
    SectionNotFound(SectionId),
}

use std::fmt;
impl fmt::Display for CodeBufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodeBufferError::OutOfSpace => write!(f, "code buffer out of space"),
            CodeBufferError::InvalidSeek(o) => write!(f, "invalid seek offset {}", o),
            CodeBufferError::InvalidPatch(o) => write!(f, "invalid patch offset {}", o),
            CodeBufferError::InvalidRead(o) => write!(f, "invalid read offset {}", o),
            CodeBufferError::Reloc(e) => write!(f, "relocation error: {}", e),
            CodeBufferError::SectionNotFound(s) => write!(f, "section not found {:?}", s),
        }
    }
}
impl std::error::Error for CodeBufferError {}
impl From<RelocError> for CodeBufferError {
    fn from(e: RelocError) -> Self { CodeBufferError::Reloc(e) }
}

/// Oop map: which stack slots / registers contain oops at a safepoint pc.
/// Mirrors OopMap (content sets flattened to offset lists).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OopMap {
    /// Pc offset (relative to insts section start).
    pub pc_offset: u32,
    /// Frame/stack offsets holding plain oops.
    pub oops: Vec<u16>,
    /// Frame/stack offsets holding derived oops.
    pub derived_oops: Vec<u16>,
    /// Register bitmask (GPRs) holding oops.
    pub oop_regs: u64,
    /// Register bitmask holding derived oops.
    pub derived_regs: u64,
}

impl OopMap {
    pub fn new(pc_offset: u32) -> Self {
        Self {
            pc_offset,
            oops: Vec::new(),
            derived_oops: Vec::new(),
            oop_regs: 0,
            derived_regs: 0,
        }
    }

    pub fn add_oop(&mut self, frame_offset: i32) {
        if frame_offset >= 0 {
            self.oops.push(frame_offset as u16);
        } else {
            self.oop_regs |= 1u64 << ((-frame_offset) as u32 & 63);
        }
    }

    pub fn add_derived(&mut self, frame_offset: i32) {
        if frame_offset >= 0 {
            self.derived_oops.push(frame_offset as u16);
        } else {
            self.derived_regs |= 1u64 << ((-frame_offset) as u32 & 63);
        }
    }
}

/// Set of oop maps, mirrors OopMapSet (sorted by pc offset).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OopMapSet {
    maps: Vec<OopMap>,
}

impl OopMapSet {
    pub fn new() -> Self {
        Self { maps: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.maps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, OopMap> {
        self.maps.iter()
    }

    /// Record an oop map at pc_offset, keeping sorted order.
    pub fn add(&mut self, map: OopMap) {
        let pos = self.maps.partition_point(|m| m.pc_offset < map.pc_offset);
        self.maps.insert(pos, map);
    }

    /// OopMapSet::find_map: last map with pc_offset <= offset.
    pub fn find_map(&self, offset: u32) -> Option<&OopMap> {
        let i = self.maps.partition_point(|m| m.pc_offset <= offset);
        if i == 0 { None } else { self.maps.get(i - 1) }
    }

    /// Serialize count + entries.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(self.maps.len() as u32).to_le_bytes());
        for m in &self.maps {
            v.extend_from_slice(&m.pc_offset.to_le_bytes());
            v.extend_from_slice(&(m.oops.len() as u32).to_le_bytes());
            for &o in &m.oops { v.extend_from_slice(&o.to_le_bytes()); }
            v.extend_from_slice(&(m.derived_oops.len() as u32).to_le_bytes());
            for &o in &m.derived_oops { v.extend_from_slice(&o.to_le_bytes()); }
            v.extend_from_slice(&m.oop_regs.to_le_bytes());
            v.extend_from_slice(&m.derived_regs.to_le_bytes());
        }
        v
    }

    pub fn decode(buf: &[u8]) -> Result<OopMapSet, CodeBufferError> {
        let mut at = 0usize;
        let rd_u32 = |at: &mut usize| -> Result<u32, CodeBufferError> {
            if buf.len() < *at + 4 { return Err(CodeBufferError::Reloc(RelocError::Truncated)); }
            let v = u32::from_le_bytes([buf[*at], buf[*at + 1], buf[*at + 2], buf[*at + 3]]);
            *at += 4;
            Ok(v)
        };
        let rd_u16 = |at: &mut usize| -> Result<u16, CodeBufferError> {
            if buf.len() < *at + 2 { return Err(CodeBufferError::Reloc(RelocError::Truncated)); }
            let v = u16::from_le_bytes([buf[*at], buf[*at + 1]]);
            *at += 2;
            Ok(v)
        };
        let rd_u64 = |at: &mut usize| -> Result<u64, CodeBufferError> {
            if buf.len() < *at + 8 { return Err(CodeBufferError::Reloc(RelocError::Truncated)); }
            let v = u64::from_le_bytes(buf[*at..*at + 8].try_into().unwrap());
            *at += 8;
            Ok(v)
        };
        let count = rd_u32(&mut at)? as usize;
        let mut set = OopMapSet::new();
        for _ in 0..count {
            let pc_offset = rd_u32(&mut at)?;
            let mut m = OopMap::new(pc_offset);
            let n = rd_u32(&mut at)? as usize;
            for _ in 0..n { m.oops.push(rd_u16(&mut at)?); }
            let n = rd_u32(&mut at)? as usize;
            for _ in 0..n { m.derived_oops.push(rd_u16(&mut at)?); }
            m.oop_regs = rd_u64(&mut at)?;
            m.derived_regs = rd_u64(&mut at)?;
            set.maps.push(m);
        }
        if at != buf.len() {
            return Err(CodeBufferError::Reloc(RelocError::Truncated));
        }
        Ok(set)
    }
}

/// A value describing one stack slot / register content at a safepoint.
/// Mirrors ScopeValue (tagged union flattened into an enum).
#[derive(Clone, Debug, PartialEq)]
pub enum ScopeConst {
    Invalid,
    IntConst(i32),
    LongConst(i64),
    FloatConst(f32),
    DoubleConst(f64),
    OopConst(u64),
    /// Raw address constant.
    AddrConst(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocBase {
    /// Normal frame (frame pointer based).
    Normal,
    /// Interpreter frame.
    InterpreterFrame,
    /// Compiled (optimized) frame.
    CompiledFrame,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScopeValueKind {
    Const(ScopeConst),
    /// Location: base register class + byte offset.
    Loc { base: LocBase, offset: i32 },
    /// Derived pointer: base value + byte offset (HotSpot derived pointer).
    Derived { base: Box<ScopeValue>, offset: i32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScopeValue {
    pub kind: ScopeValueKind,
}

impl ScopeValue {
    pub fn invalid() -> Self {
        Self { kind: ScopeValueKind::Const(ScopeConst::Invalid) }
    }
    pub fn int_const(v: i32) -> Self {
        Self { kind: ScopeValueKind::Const(ScopeConst::IntConst(v)) }
    }
    pub fn oop_const(v: u64) -> Self {
        Self { kind: ScopeValueKind::Const(ScopeConst::OopConst(v)) }
    }
    pub fn normal(offset: i32) -> Self {
        Self { kind: ScopeValueKind::Loc { base: LocBase::Normal, offset } }
    }
}

/// Monitor (lock) value in a scope descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorValue {
    pub owner: ScopeValue,
    pub basic_lock: ScopeValue,
    pub eliminated: bool,
}

/// Object value recorded in debug info (fields of an allocated object).
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectValue {
    pub klass: u64,
    pub fields: Vec<ScopeValue>,
}

/// Scope descriptor for one pc, mirrors scopeDesc.hpp.
#[derive(Clone, Debug)]
pub struct ScopeDesc {
    /// Pc offset relative to insts section start.
    pub pc_offset: u32,
    /// Sender (caller) scope descriptor; None at outermost frame.
    pub sender: Option<Box<ScopeDesc>>,
    /// Method handle (Method* as u64).
    pub method: u64,
    /// Bytecode index.
    pub bci: i32,
    pub locals: Vec<ScopeValue>,
    pub expressions: Vec<ScopeValue>,
    pub monitors: Vec<MonitorValue>,
    pub objects: Vec<ObjectValue>,
}

impl ScopeDesc {
    pub fn new(pc_offset: u32, method: u64, bci: i32) -> Self {
        Self {
            pc_offset,
            sender: None,
            method,
            bci,
            locals: Vec::new(),
            expressions: Vec::new(),
            monitors: Vec::new(),
            objects: Vec::new(),
        }
    }
}

/// Exception handler table entry, mirrors exceptionTable.hpp
/// ExceptionHandlerTable::HandlerInfo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExceptionHandlerEntry {
    /// Start pc offset (inclusive).
    pub pc_start: u32,
    /// End pc offset (exclusive).
    pub pc_end: u32,
    /// Handler pc offset.
    pub handler_pc: u32,
    /// Catch type (klass handle; 0 = catch all / finally).
    pub catch_type: u64,
    /// Link to next handler with same pc range (0 = none), used when
    /// multiple catch clauses overlap.
    pub next: u32,
}

/// Exception handler table, mirrors ExceptionHandlerTable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExceptionHandlerTable {
    entries: Vec<ExceptionHandlerEntry>,
}

impl ExceptionHandlerTable {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, ExceptionHandlerEntry> {
        self.entries.iter()
    }

    pub fn add(
        &mut self,
        pc_start: u32,
        pc_end: u32,
        handler_pc: u32,
        catch_type: u64,
    ) -> u32 {
        let index = self.entries.len() as u32;
        self.entries.push(ExceptionHandlerEntry {
            pc_start,
            pc_end,
            handler_pc,
            catch_type,
            next: 0,
        });
        index
    }

    /// Find handler covering `pc`; returns the chain head index.
    pub fn find_handler(&self, pc: u32) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.pc_start <= pc && pc < e.pc_end)
    }

    /// Serialize.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            v.extend_from_slice(&e.pc_start.to_le_bytes());
            v.extend_from_slice(&e.pc_end.to_le_bytes());
            v.extend_from_slice(&e.handler_pc.to_le_bytes());
            v.extend_from_slice(&e.catch_type.to_le_bytes());
            v.extend_from_slice(&e.next.to_le_bytes());
        }
        v
    }

    pub fn decode(buf: &[u8]) -> Result<ExceptionHandlerTable, CodeBufferError> {
        if buf.len() < 4 {
            return Err(CodeBufferError::Reloc(RelocError::Truncated));
        }
        let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let mut t = ExceptionHandlerTable::new();
        let mut at = 4usize;
        for _ in 0..count {
            if buf.len() < at + 24 {
                return Err(CodeBufferError::Reloc(RelocError::Truncated));
            }
            let pc_start = u32::from_le_bytes(buf[at..at + 4].try_into().unwrap());
            let pc_end = u32::from_le_bytes(buf[at + 4..at + 8].try_into().unwrap());
            let handler_pc = u32::from_le_bytes(buf[at + 8..at + 12].try_into().unwrap());
            let catch_type = u64::from_le_bytes(buf[at + 12..at + 20].try_into().unwrap());
            let next = u32::from_le_bytes(buf[at + 20..at + 24].try_into().unwrap());
            t.entries.push(ExceptionHandlerEntry { pc_start, pc_end, handler_pc, catch_type, next });
            at += 24;
        }
        if at != buf.len() {
            return Err(CodeBufferError::Reloc(RelocError::Truncated));
        }
        Ok(t)
    }
}

/// CodeBuffer flags, mirrors CodeBuffer::CB_*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeBufferFlag {
    /// Buffer is optimized for size (short branches etc.).
    OptimizedForSize,
    /// Do not optimize for size.
    NotOptimized,
    /// Buffer contains relocation records (always true in practice).
    HasRelocations,
}

/// The main code emission buffer, mirrors share/asm/codeBuffer.hpp CodeBuffer.
#[derive(Debug)]
pub struct CodeBuffer {
    /// Instruction section (SECT_INSTS).
    pub insts: CodeSection,
    /// Stub section (SECT_STUBS).
    pub stubs: CodeSection,
    /// Constant section (SECT_CONSTS).
    pub consts: CodeSection,
    /// Currently active emit section.
    pub current: SectionId,
    /// Oop maps.
    pub oopmaps: OopMapSet,
    /// Scope descriptors, sorted by pc offset.
    pub scope_descs: Vec<ScopeDesc>,
    /// Exception handler table.
    pub exception_table: ExceptionHandlerTable,
    /// Code buffer flags.
    pub flags: u32,
    /// Buffer name for diagnostics.
    pub name: String,
}

const FLAG_OPTIMIZED_FOR_SIZE: u32 = 1 << 0;
const FLAG_NOT_OPTIMIZED: u32 = 1 << 1;

impl CodeBuffer {
    pub fn new(initial_size: usize) -> Self {
        let size = initial_size.max(END_SLOP);
        Self {
            insts: CodeSection::new(0, size),
            stubs: CodeSection::new(1, 0),
            consts: CodeSection::new(2, 0),
            current: SectionId::Insts,
            oopmaps: OopMapSet::new(),
            scope_descs: Vec::new(),
            exception_table: ExceptionHandlerTable::new(),
            flags: 0,
            name: String::new(),
        }
    }

    pub fn with_name(initial_size: usize, name: &str) -> Self {
        let mut b = Self::new(initial_size);
        b.name = name.to_string();
        b
    }

    // ---- section access -------------------------------------------------

    pub fn section(&mut self, id: SectionId) -> &mut CodeSection {
        match id {
            SectionId::Insts => &mut self.insts,
            SectionId::Stubs => &mut self.stubs,
            SectionId::Consts => &mut self.consts,
        }
    }

    pub fn section_ref(&self, id: SectionId) -> &CodeSection {
        match id {
            SectionId::Insts => &self.insts,
            SectionId::Stubs => &self.stubs,
            SectionId::Consts => &self.consts,
        }
    }

    /// Assembler::_code accessor: currently active section.
    pub fn code_section(&mut self) -> &mut CodeSection {
        self.section(self.current)
    }

    pub fn code_section_ref(&self) -> &CodeSection {
        self.section_ref(self.current)
    }

    pub fn set_code_section(&mut self, id: SectionId) {
        self.current = id;
    }

    // ---- flags ----------------------------------------------------------

    pub fn set_optimized_for_size(&mut self) {
        self.flags |= FLAG_OPTIMIZED_FOR_SIZE;
        self.flags &= !FLAG_NOT_OPTIMIZED;
    }

    pub fn set_not_optimized_for_size(&mut self) {
        self.flags |= FLAG_NOT_OPTIMIZED;
        self.flags &= !FLAG_OPTIMIZED_FOR_SIZE;
    }

    pub fn optimize_for_size(&self) -> bool {
        self.flags & FLAG_OPTIMIZED_FOR_SIZE != 0
    }

    // ---- position -------------------------------------------------------

    pub fn pos(&self) -> usize {
        self.code_section_ref().end
    }

    pub fn end_pos(&self) -> usize {
        self.pos()
    }

    pub fn mark(&self) -> Option<usize> {
        self.code_section_ref().mark
    }

    pub fn set_mark(&mut self) {
        let p = self.pos();
        self.code_section().mark = Some(p);
    }

    pub fn clear_mark(&mut self) {
        self.code_section().mark = None;
    }

    /// inst_mark(): position of last instruction mark in insts section.
    pub fn inst_mark(&self) -> usize {
        self.insts.mark.unwrap_or(0)
    }

    pub fn set_inst_mark(&mut self) {
        self.insts.mark = Some(self.insts.end);
    }

    pub fn clear_inst_mark(&mut self) {
        self.insts.mark = None;
    }

    /// Seek current section to absolute offset (CodeBuffer::seek).
    pub fn seek(&mut self, offset: usize) -> Result<(), CodeBufferError> {
        let sec = self.code_section();
        if offset > sec.end {
            return Err(CodeBufferError::InvalidSeek(offset));
        }
        sec.end = offset;
        Ok(())
    }

    /// set_end: extend current section end to `offset` (bytes must exist).
    pub fn set_end(&mut self, offset: usize) -> Result<(), CodeBufferError> {
        let sec = self.code_section();
        if offset > sec.bytes.len() {
            return Err(CodeBufferError::InvalidSeek(offset));
        }
        if offset < sec.end {
            sec.end = offset;
        } else {
            sec.end = offset;
        }
        Ok(())
    }

    // ---- size accounting -------------------------------------------------

    /// total used size across sections (CodeBuffer::size).  Only the
    /// insts section counts toward blob size in HotSpot; stubs/consts are
    /// appended after.  We report insts size like HotSpot CodeBuffer::size().
    pub fn size(&self) -> usize {
        self.insts.size()
    }

    /// buffer_size(): total allocated bytes in all sections.
    pub fn buffer_size(&self) -> usize {
        self.insts.bytes.len() + self.stubs.bytes.len() + self.consts.bytes.len()
    }

    /// max_used(): highest used offset across sections.
    pub fn max_used(&self) -> usize {
        self.insts.end.max(self.stubs.end).max(self.consts.end)
    }

    /// total_relocation_size(): serialized relocation bytes (approximation
    /// of relocInfo::size used for blob layout).
    pub fn total_relocation_size(&self) -> usize {
        self.reloc_table().map(|t| t.total_size()).unwrap_or(0)
    }

    pub fn total_oopmap_size(&self) -> usize {
        self.oopmaps.encode().len()
    }

    pub fn total_scope_descs_size(&self) -> usize {
        // Each ScopeDesc serializes method/bci + value lists; estimate
        // conservatively with a fixed per-desc record plus payload.
        self.scope_descs.iter().map(|s| 32 + (s.locals.len() + s.expressions.len()) * 16).sum()
    }

    pub fn total_exception_handler_table_size(&self) -> usize {
        self.exception_table.encode().len()
    }

    /// Merged relocation table across all sections, rebased to offsets
    /// relative to insts section start, mirroring CodeBuffer::relocInfo
    /// ordering (insts first, then stubs, then consts).
    pub fn reloc_table(&self) -> Result<RelocTable, CodeBufferError> {
        let mut table = RelocTable::new();
        let insts_base = 0usize;
        let stubs_base = self.insts.end.div_ceil(CODE_ENTRY_ALIGNMENT) * CODE_ENTRY_ALIGNMENT;
        let consts_base = (stubs_base + self.stubs.end).div_ceil(CODE_ENTRY_ALIGNMENT) * CODE_ENTRY_ALIGNMENT;
        let bases = [(insts_base, &self.insts), (stubs_base, &self.stubs), (consts_base, &self.consts)];
        for (base, sec) in bases {
            for e in sec.relocs.iter() {
                let mut ne = e.clone();
                ne.pc_offset = (base + e.pc_offset as usize) as u32;
                table.add(ne);
            }
        }
        Ok(table)
    }

    /// record_relocation: add reloc entry at current pc in active section.
    pub fn record_relocation(
        &mut self,
        rtype: RelocType,
        width: RelocWidth,
        value: RelocValue,
        field_offset: u32,
    ) {
        let pc = self.pos() as u32;
        self.code_section().add_reloc(pc, field_offset, rtype, width, value);
    }

    // ---- oop maps / scope descs / exception table ------------------------

    /// OopMapSet::add_gc_map equivalent: record oop map at pc offset.
    pub fn add_oopmap(&mut self, map: OopMap) {
        self.oopmaps.add(map);
    }

    pub fn record_oopmap(
        &mut self,
        pc_offset: u32,
        oops: &[i32],
        derived_oops: &[i32],
    ) {
        let mut m = OopMap::new(pc_offset);
        for &o in oops { m.add_oop(o); }
        for &o in derived_oops { m.add_derived(o); }
        self.oopmaps.add(m);
    }

    /// Add a scope descriptor; kept sorted by pc offset.
    pub fn add_scope_desc(&mut self, desc: ScopeDesc) {
        let pos = self.scope_descs.partition_point(|d| d.pc_offset < desc.pc_offset);
        self.scope_descs.insert(pos, desc);
    }

    /// Add exception handler range.
    pub fn add_exception_handler(
        &mut self,
        pc_start: u32,
        pc_end: u32,
        handler_pc: u32,
        catch_type: u64,
    ) -> u32 {
        self.exception_table.add(pc_start, pc_end, handler_pc, catch_type)
    }

    // ---- stub / const helpers ---------------------------------------------

    /// start_a_stub: switch emission to stubs section.
    pub fn start_a_stub(&mut self) {
        self.set_code_section(SectionId::Stubs);
    }

    /// end_a_stub: switch back to insts.
    pub fn end_a_stub(&mut self) {
        self.set_code_section(SectionId::Insts);
    }

    /// start_a_const: switch to consts section with alignment.
    pub fn start_a_const(&mut self, required_space: usize, required_align: usize) {
        self.set_code_section(SectionId::Consts);
        let align = required_align.max(1);
        self.code_section().align_at(align, 0);
        let _ = required_space;
    }

    /// end_a_const: switch back to insts.
    pub fn end_a_const(&mut self) {
        self.set_code_section(SectionId::Insts);
    }

    /// int_constant: emit a jint into consts, return its offset.
    pub fn int_constant(&mut self, c: i32) -> u64 {
        self.start_a_const(4, 4);
        let off = self.pos() as u64;
        self.code_section().emit_u32(c as u32);
        self.end_a_const();
        off
    }

    /// long_constant: emit a jlong into consts.
    pub fn long_constant(&mut self, c: i64) -> u64 {
        self.start_a_const(8, 8);
        let off = self.pos() as u64;
        self.code_section().emit_u64(c as u64);
        self.end_a_const();
        off
    }

    /// double_constant: emit a jdouble into consts.
    pub fn double_constant(&mut self, c: f64) -> u64 {
        self.start_a_const(8, 8);
        let off = self.pos() as u64;
        self.code_section().emit_f64(c);
        self.end_a_const();
        off
    }

    /// float_constant: emit a jfloat into consts.
    pub fn float_constant(&mut self, c: f32) -> u64 {
        self.start_a_const(4, 4);
        let off = self.pos() as u64;
        self.code_section().emit_f32(c);
        self.end_a_const();
        off
    }

    // ---- finalize / copy -----------------------------------------------

    /// Section base offsets inside the final blob (insts at 0).
    pub fn section_bases(&self) -> (usize, usize, usize) {
        let stubs_base = self.insts.end.div_ceil(CODE_ENTRY_ALIGNMENT) * CODE_ENTRY_ALIGNMENT;
        let consts_base = (stubs_base + self.stubs.end).div_ceil(CODE_ENTRY_ALIGNMENT) * CODE_ENTRY_ALIGNMENT;
        (0, stubs_base, consts_base)
    }

    /// finalize(): perform end_slop padding and prepare for blob copy.
    /// Mirrors CodeBuffer::before_finalize/flush (no-op beyond alignment
    /// since Rust Vec manages capacity).
    pub fn finalize(&mut self) {
        // Ensure end slop exists for safe over-reads by the VM.
        self.insts.ensure_capacity(END_SLOP);
        self.stubs.ensure_capacity(END_SLOP);
        self.consts.ensure_capacity(END_SLOP);
    }

    /// copy_codes_into: flatten sections into a contiguous image and merge
    /// relocation tables rebased to blob offsets.  Returns (image,
    /// relocations, section bases).
    pub fn copy_codes_into(&self) -> Result<(Vec<u8>, RelocTable, (usize, usize, usize)), CodeBufferError> {
        let (ib, sb, cb) = self.section_bases();
        let total = cb + self.consts.end + END_SLOP;
        let mut image = vec![0u8; total];
        image[..self.insts.end].copy_from_slice(&self.insts.bytes[..self.insts.end]);
        image[sb..sb + self.stubs.end].copy_from_slice(&self.stubs.bytes[..self.stubs.end]);
        image[cb..cb + self.consts.end].copy_from_slice(&self.consts.bytes[..self.consts.end]);
        let table = self.reloc_table()?;
        Ok((image, table, (ib, sb, cb)))
    }

    /// copy_codes: build a CodeBlob from this buffer.
    pub fn copy_codes(&self, blob_type: CodeBlobType, name: &str) -> Result<CodeBlob, CodeBufferError> {
        let (image, table, (_ib, sb, cb)) = self.copy_codes_into()?;
        Ok(CodeBlob {
            blob_type,
            name: name.to_string(),
            header_size: 0,
            instructions: image,
            insts_size: self.insts.end,
            stubs_offset: sb,
            stubs_size: self.stubs.end,
            consts_offset: cb,
            consts_size: self.consts.end,
            relocations: table,
            oopmaps: self.oopmaps.clone(),
            scope_descs: self.scope_descs.clone(),
            exception_table: self.exception_table.clone(),
            frame_size: 0,
            is_alive: true,
            is_nmethod: matches!(blob_type, CodeBlobType::NMethod),
        })
    }

    /// find_blob: locate the blob containing pc, like
    /// CodeCache::find_blob.  In this port blobs are owned by the caller,
    /// so we check whether `pc` (an offset or absolute address) falls in
    /// the blob's instruction range.
    pub fn find_blob_in<'a>(blob: &'a CodeBlob, pc: u64) -> Option<&'a CodeBlob> {
        if blob.contains_pc(pc) { Some(blob) } else { None }
    }
}

/// CodeBlob type, mirrors CodeBlob::Type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeBlobType {
    NMethod,
    RuntimeStub,
    OsrNMethod,
    Other,
}

/// Finalized code blob, mirrors share/code/codeBlob.hpp CodeBlob.
#[derive(Debug)]
pub struct CodeBlob {
    pub blob_type: CodeBlobType,
    pub name: String,
    /// Header size in bytes before instructions begin.
    pub header_size: usize,
    /// Contiguous code image (insts + stubs + consts + end slop).
    pub instructions: Vec<u8>,
    /// Instruction section size.
    pub insts_size: usize,
    /// Offset of stubs section inside `instructions`.
    pub stubs_offset: usize,
    pub stubs_size: usize,
    /// Offset of consts section inside `instructions`.
    pub consts_offset: usize,
    pub consts_size: usize,
    pub relocations: RelocTable,
    pub oopmaps: OopMapSet,
    pub scope_descs: Vec<ScopeDesc>,
    pub exception_table: ExceptionHandlerTable,
    /// Compiled frame size (set by compilers).
    pub frame_size: usize,
    pub is_alive: bool,
    pub is_nmethod: bool,
}

impl CodeBlob {
    /// insts_begin / insts_end as slices.
    pub fn insts(&self) -> &[u8] {
        &self.instructions[..self.insts_size]
    }

    pub fn insts_end(&self) -> usize {
        self.header_size + self.insts_size
    }

    /// Absolute pc of instruction start within the image (header + 0).
    pub fn insts_begin_offset(&self) -> usize {
        self.header_size
    }

    /// Does pc (image offset or absolute address) fall inside instructions?
    pub fn contains_pc(&self, pc: u64) -> bool {
        let start = self.insts_begin_offset() as u64;
        let end = self.insts_begin_offset() as u64 + self.insts_size as u64;
        pc >= start && pc < end
    }

    /// pc_offset: offset of pc relative to insts_begin.
    pub fn pc_offset(&self, pc: u64) -> Option<u32> {
        let start = self.insts_begin_offset() as u64;
        if pc >= start && pc < start + self.insts_size as u64 {
            Some((pc - start) as u32)
        } else {
            None
        }
    }

    /// total_size(): total blob size.
    pub fn total_size(&self) -> usize {
        self.instructions.len()
    }

    /// code_begin / code_end offsets within image.
    pub fn code_begin(&self) -> usize {
        self.insts_begin_offset()
    }

    pub fn code_end(&self) -> usize {
        self.insts_end()
    }

    /// Lookup exception handler for pc offset.
    pub fn exception_handler(&self, pc_offset: u32) -> Option<ExceptionHandlerEntry> {
        self.exception_table
            .iter()
            .find(|e| e.pc_start <= pc_offset && pc_offset < e.pc_end)
            .copied()
    }

    /// OopMapSet::find_map for a pc offset.
    pub fn oop_map_at(&self, pc_offset: u32) -> Option<&OopMap> {
        self.oopmaps.find_map(pc_offset)
    }

    /// Relocations covering a pc offset range.
    pub fn relocations_in(&self, start: u32, end: u32) -> Vec<&RelocEntry> {
        self.relocations.in_range(start, end).collect()
    }
}
