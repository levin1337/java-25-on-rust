/*
 * Port of OpenJDK 25 HotSpot JVMCI code installer
 * (share/jvmci/jvmciCodeInstaller.hpp / jvmciCodeInstaller.cpp)
 *
 * Decodes a serialized `HotSpotCompiledCode`/`HotSpotCompiledNmethod`
 * stream produced by the JVMCI compiler (`jdk.vm.ci.hotspot.
 * HotSpotCompiledCodeStream` wire format) and converts it into the
 * code-cache request consumed by `JVMCIRuntime::register_method`.
 *
 * Stream layout: the buffer is a chain of chunks, each
 *
 *   u64 next_off   // offset of next chunk header from buffer start, 0 = end
 *   u32 size       // payload size in bytes
 *   u32 reserved
 *   u8  data[size]
 *
 * HotSpot links chunks with raw pointers; this port serializes the
 * link as a buffer offset so the stream is self-contained.
 *
 * With `with_type_info` every typed read is preceded by
 * `check_data(size, name)` (u1 size, u1 name-len, name bytes) exactly
 * like the C++ consumer.
 */

use crate::share::ci::ciType::BasicType;
use crate::share::code::dependencies::{
    Dependency, DependencyArgument, DependencySet, DepType,
};
use crate::share::code::exceptionHandlerTable::{
    ExceptionHandlerTable, HandlerTableEntry, PcRangeHandlerTable,
};
use crate::share::code::nmethod::DeoptLiveMap;
use crate::share::code::pcDesc::PcDesc;
use crate::share::code::scopeDesc::{
    Location, LocationType, MonitorValue, ObjectValue, ScopeDesc, ScopeDescWriter,
    ScopeValue,
};
use crate::share::compiler::compilerDefinitions::MethodCompilation;
use crate::share::compiler::oopMap::{OopMap, OopMapSet, OopMapValue, VMReg};
use crate::share::jvmci::jvmci::CodeInstallResult;
use crate::share::jvmci::jvmciEnv::JvmciEnv;
use crate::share::jvmci::jvmciExceptions::{JvmciException, JvmciResult};
use crate::share::jvmci::jvmciGlobals;
use crate::share::jvmci::jvmciObject::{self, JvmciHeapValue, JvmciObject};
use crate::share::jvmci::jvmciRuntime::FailedSpeculation;
use crate::share::jvmci::metadataHandles::MetadataTarget;

// ---------------------------------------------------------------------------
// BCI sentinels (HotSpot ScopeDesc/BytecodeFrame constants; the shared
// scopeDesc port does not export them, so they live here)
// ---------------------------------------------------------------------------

pub const BEFORE_BCI: i32 = MethodCompilation::BEFORE_BCI;
pub const AFTER_BCI: i32 = MethodCompilation::AFTER_BCI;
pub const UNWIND_BCI: i32 = MethodCompilation::UNWIND_BCI;
pub const AFTER_EXCEPTION_BCI: i32 = MethodCompilation::AFTER_EXCEPTION_BCI;
pub const UNKNOWN_BCI: i32 = MethodCompilation::UNKNOWN_BCI;
pub const INVALID_FRAMESTATE_BCI: i32 = MethodCompilation::INVALID_FRAME_STATE_BCI;
/// HotSpot `SynchronizationEntryBCI`.
pub const SYNCHRONIZATION_ENTRY_BCI: i32 = -7;

// ---------------------------------------------------------------------------
// Wire constants (mirror jdk.vm.ci.hotspot.HotSpotCompiledCodeStream.Tag)
// ---------------------------------------------------------------------------

pub mod tag {
    pub const ILLEGAL: u8 = 0;
    pub const REGISTER_PRIMITIVE: u8 = 1;
    pub const REGISTER_OOP: u8 = 2;
    pub const REGISTER_NARROW_OOP: u8 = 3;
    pub const REGISTER_VECTOR: u8 = 4;
    pub const STACK_SLOT_PRIMITIVE: u8 = 5;
    pub const STACK_SLOT_OOP: u8 = 6;
    pub const STACK_SLOT_NARROW_OOP: u8 = 7;
    pub const STACK_SLOT_VECTOR: u8 = 8;
    pub const STACK_SLOT4_PRIMITIVE: u8 = 9;
    pub const STACK_SLOT4_OOP: u8 = 10;
    pub const STACK_SLOT4_NARROW_OOP: u8 = 11;
    pub const STACK_SLOT4_VECTOR: u8 = 12;
    pub const VIRTUAL_OBJECT_ID: u8 = 13;
    pub const VIRTUAL_OBJECT_ID2: u8 = 14;
    pub const NULL_CONSTANT: u8 = 15;
    pub const RAW_CONSTANT: u8 = 16;
    pub const PRIMITIVE_0: u8 = 17;
    pub const PRIMITIVE4: u8 = 18;
    pub const PRIMITIVE8: u8 = 19;
    pub const JOBJECT: u8 = 20;
    pub const OBJECT_ID: u8 = 21;
    pub const OBJECT_ID2: u8 = 22;

    pub const NO_FINALIZABLE_SUBCLASS: u8 = 23;
    pub const CONCRETE_SUBTYPE: u8 = 24;
    pub const LEAF_TYPE: u8 = 25;
    pub const CONCRETE_METHOD: u8 = 26;
    pub const CALLSITE_TARGET_VALUE: u8 = 27;

    pub const PATCH_OBJECT_ID: u8 = 28;
    pub const PATCH_OBJECT_ID2: u8 = 29;
    pub const PATCH_NARROW_OBJECT_ID: u8 = 30;
    pub const PATCH_NARROW_OBJECT_ID2: u8 = 31;
    pub const PATCH_JOBJECT: u8 = 32;
    pub const PATCH_NARROW_JOBJECT: u8 = 33;
    pub const PATCH_KLASS: u8 = 34;
    pub const PATCH_NARROW_KLASS: u8 = 35;
    pub const PATCH_METHOD: u8 = 36;
    pub const PATCH_DATA_SECTION_REFERENCE: u8 = 37;

    pub const SITE_CALL: u8 = 38;
    pub const SITE_FOREIGN_CALL: u8 = 39;
    pub const SITE_FOREIGN_CALL_NO_DEBUG_INFO: u8 = 40;
    pub const SITE_SAFEPOINT: u8 = 41;
    pub const SITE_INFOPOINT: u8 = 42;
    pub const SITE_IMPLICIT_EXCEPTION: u8 = 43;
    pub const SITE_IMPLICIT_EXCEPTION_DISPATCH: u8 = 44;
    pub const SITE_MARK: u8 = 45;
    pub const SITE_DATA_PATCH: u8 = 46;
    pub const SITE_EXCEPTION_HANDLER: u8 = 47;
}

/// `CodeInstaller::MarkId` (site mark identifiers).
pub mod mark {
    pub const INVALID_MARK: u8 = 0;
    pub const VERIFIED_ENTRY: u8 = 1;
    pub const UNVERIFIED_ENTRY: u8 = 2;
    pub const OSR_ENTRY: u8 = 3;
    pub const EXCEPTION_HANDLER_ENTRY: u8 = 4;
    pub const DEOPT_HANDLER_ENTRY: u8 = 5;
    pub const FRAME_COMPLETE: u8 = 6;
    pub const ENTRY_BARRIER_PATCH: u8 = 7;
    pub const INVOKEINTERFACE: u8 = 8;
    pub const INVOKEVIRTUAL: u8 = 9;
    pub const INVOKESTATIC: u8 = 10;
    pub const INVOKESPECIAL: u8 = 11;
    pub const INLINE_INVOKE: u8 = 12;
    pub const POLL_NEAR: u8 = 13;
    pub const POLL_RETURN_NEAR: u8 = 14;
    pub const POLL_FAR: u8 = 15;
    pub const POLL_RETURN_FAR: u8 = 16;
    pub const CARD_TABLE_ADDRESS: u8 = 17;
    pub const CARD_TABLE_SHIFT: u8 = 18;
    pub const HEAP_TOP_ADDRESS: u8 = 19;
    pub const HEAP_END_ADDRESS: u8 = 20;
    pub const NARROW_KLASS_BASE_ADDRESS: u8 = 21;
    pub const NARROW_OOP_BASE_ADDRESS: u8 = 22;
    pub const CRC_TABLE_ADDRESS: u8 = 23;
    pub const LOG_OF_HEAP_REGION_GRAIN_BYTES: u8 = 24;
    pub const INLINE_CONTIGUOUS_ALLOCATION_SUPPORTED: u8 = 25;
    pub const DEOPT_MH_HANDLER_ENTRY: u8 = 26;
    pub const VERIFY_OOPS: u8 = 27;
    pub const VERIFY_OOP_BITS: u8 = 28;
    pub const VERIFY_OOP_MASK: u8 = 29;
    pub const VERIFY_OOP_COUNT_ADDRESS: u8 = 30;
}

/// `HotSpotCompiledCodeFlags`.
pub mod hcc {
    pub const IS_NMETHOD: u8 = 0x01;
    pub const HAS_ASSUMPTIONS: u8 = 0x02;
    pub const HAS_METHODS: u8 = 0x04;
    pub const HAS_DEOPT_RESCUE_SLOT: u8 = 0x08;
    pub const HAS_COMMENTS: u8 = 0x10;
}

/// `DebugInfoFlags`.
pub mod dif {
    pub const HAS_REFERENCE_MAP: u8 = 0x01;
    pub const HAS_CALLEE_SAVE_INFO: u8 = 0x02;
    pub const HAS_FRAMES: u8 = 0x04;
}

/// `DebugInfoFrameFlags`.
pub mod frame_flag {
    pub const HAS_LOCALS: u8 = 0x01;
    pub const HAS_STACK: u8 = 0x02;
    pub const HAS_LOCKS: u8 = 0x04;
    pub const DURING_CALL: u8 = 0x08;
    pub const RETHROW_EXCEPTION: u8 = 0x10;
}

/// `NO_REGISTER` sentinel for the JVMCI register number space.
pub const NO_REGISTER: u16 = 0xFFFF;

fn is_set(flags: u8, bit: u8) -> bool {
    flags & bit != 0
}

// ---------------------------------------------------------------------------
// HotSpotCompiledCodeStream
// ---------------------------------------------------------------------------

struct ChunkRef {
    /// Absolute offset of the payload inside `buf`.
    data_off: usize,
    size: usize,
}

/// Decoder for the serialized HotSpotCompiledCode stream.
pub struct HotSpotCompiledCodeStream<'a> {
    buf: &'a [u8],
    chunks: Vec<ChunkRef>,
    chunk_index: usize,
    pos: usize,
    with_type_info: bool,
    object_pool: Vec<JvmciObject>,
    virtual_objects: Vec<ScopeValue>,
    code_desc: String,
}

impl<'a> HotSpotCompiledCodeStream<'a> {
    /// Parse the chunk chain at `buffer`.
    pub fn new(
        buffer: &'a [u8],
        with_type_info: bool,
        object_pool: Vec<JvmciObject>,
    ) -> Result<Self, JvmciException> {
        let mut chunks = Vec::new();
        let mut off = 0usize;
        while off + 16 <= buffer.len() {
            let next_off = u64::from_le_bytes(buffer[off..off + 8].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(buffer[off + 8..off + 12].try_into().unwrap()) as usize;
            let data_off = off + 16;
            if data_off + size > buffer.len() {
                return Err(JvmciException::internal_error(format!(
                    "HotSpotCompiledCodeStream: chunk at {off} overruns buffer (size {size}, len {})",
                    buffer.len()
                )));
            }
            chunks.push(ChunkRef { data_off, size });
            if next_off == 0 {
                break;
            }
            if next_off <= off || next_off >= buffer.len() {
                return Err(JvmciException::internal_error(format!(
                    "HotSpotCompiledCodeStream: bad next chunk offset {next_off}"
                )));
            }
            off = next_off;
        }
        if chunks.is_empty() {
            return Err(JvmciException::internal_error(
                "HotSpotCompiledCodeStream: empty buffer",
            ));
        }
        let first_data_off = chunks[0].data_off;
        Ok(Self {
            buf: buffer,
            chunks,
            chunk_index: 0,
            pos: first_data_off,
            with_type_info,
            object_pool,
            virtual_objects: Vec::new(),
            code_desc: "<unknown>".to_string(),
        })
    }

    /// `code_desc`.
    pub fn code_desc(&self) -> &str {
        &self.code_desc
    }

    /// `set_code_desc`.
    pub fn set_code_desc(&mut self, name: Option<&str>, method_key: Option<&str>) {
        if let Some(n) = name {
            self.code_desc = n.to_string();
        } else if let Some(k) = method_key {
            self.code_desc = k.to_string();
        }
    }

    /// `offset`: bytes consumed across all chunks so far.
    pub fn offset(&self) -> u32 {
        let mut res = 0u32;
        for (i, c) in self.chunks.iter().enumerate() {
            if i == self.chunk_index {
                res += (self.pos - c.data_off) as u32;
                break;
            }
            res += c.size as u32;
        }
        res
    }

    /// `available`: bytes remaining in the stream.
    pub fn available(&self) -> usize {
        let cur = &self.chunks[self.chunk_index];
        let mut rem = cur.data_off + cur.size - self.pos;
        for c in &self.chunks[self.chunk_index + 1..] {
            rem += c.size;
        }
        rem
    }

    fn before_read(&mut self, size: usize, name: &str) -> JvmciResult<()> {
        let cur = &self.chunks[self.chunk_index];
        let end = cur.data_off + cur.size;
        if self.pos + size > end {
            let next = self.chunk_index + 1;
            if next >= self.chunks.len() || size > self.chunks[next].size {
                return Err(JvmciException::internal_error(format!(
                    "{}: reading {} bytes ('{}') overflows buffer at offset {}",
                    self.code_desc,
                    size,
                    name,
                    self.offset()
                )));
            }
            self.chunk_index = next;
            self.pos = self.chunks[next].data_off;
        }
        Ok(())
    }

    /// `check_data`.
    fn check_data(&mut self, expect_size: usize, expect_name: &str) -> JvmciResult<()> {
        let actual_size = self.get_u1_raw()? as usize;
        let ascii_len = self.get_u1_raw()? as usize;
        let name_start = self.pos;
        let name_end = name_start + ascii_len;
        let cur = &self.chunks[self.chunk_index];
        if name_end > cur.data_off + cur.size {
            return Err(JvmciException::internal_error(format!(
                "{}: type-info name overruns chunk",
                self.code_desc
            )));
        }
        let actual_name = String::from_utf8_lossy(&self.buf[name_start..name_end]).into_owned();
        self.pos = name_end;
        if actual_name != expect_name {
            return Err(JvmciException::internal_error(format!(
                "{}: expected \"{expect_name}\" in HotSpotCompiledCode stream, got \"{actual_name}\"",
                self.code_desc
            )));
        }
        if actual_size != expect_size {
            return Err(JvmciException::internal_error(format!(
                "{}: expected \"{expect_name}\" to have size {expect_size}, got {actual_size}",
                self.code_desc
            )));
        }
        Ok(())
    }

    fn get_u1_raw(&mut self) -> JvmciResult<u8> {
        self.before_read(1, "u1")?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn typed<T>(
        &mut self,
        size: usize,
        name: &str,
        read: impl FnOnce(&[u8]) -> T,
    ) -> JvmciResult<T> {
        if self.with_type_info {
            self.check_data(size, name)?;
        }
        self.before_read(size, name)?;
        let end = self.pos + size;
        let v = read(&self.buf[self.pos..end]);
        self.pos = end;
        Ok(v)
    }

    pub fn read_u1(&mut self, name: &str) -> JvmciResult<u8> {
        self.typed(1, name, |b| b[0])
    }
    pub fn read_u2(&mut self, name: &str) -> JvmciResult<u16> {
        self.typed(2, name, |b| u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn read_u4(&mut self, name: &str) -> JvmciResult<u32> {
        self.typed(4, name, |b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn read_u8(&mut self, name: &str) -> JvmciResult<u64> {
        self.typed(8, name, |b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn read_s2(&mut self, name: &str) -> JvmciResult<i16> {
        self.typed(2, name, |b| i16::from_le_bytes([b[0], b[1]]))
    }
    pub fn read_s4(&mut self, name: &str) -> JvmciResult<i32> {
        self.typed(4, name, |b| i32::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn read_s8(&mut self, name: &str) -> JvmciResult<i64> {
        self.typed(8, name, |b| i64::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn read_bool(&mut self, name: &str) -> JvmciResult<bool> {
        Ok(self.read_u1(name)? != 0)
    }

    /// `read_utf8`: s4 length (-1 = null) + bytes + NUL terminator.
    pub fn read_utf8(&mut self, name: &str) -> JvmciResult<Option<String>> {
        let utf_len = self.read_s4(name)?;
        if utf_len == -1 {
            return Ok(None);
        }
        if utf_len < 0 {
            return Err(JvmciException::internal_error(format!(
                "{}: bad utf_len {utf_len}",
                self.code_desc
            )));
        }
        let len = utf_len as usize;
        self.before_read(len + 1, name)?;
        let start = self.pos;
        let end = start + len;
        if self.buf[end] != 0 {
            return Err(JvmciException::internal_error(format!(
                "{}: UTF8 string of length {utf_len} missing 0 terminator",
                self.code_desc
            )));
        }
        let s = String::from_utf8_lossy(&self.buf[start..end]).into_owned();
        self.pos = end + 1;
        Ok(Some(s))
    }

    /// `read_method`: u64 jmetadata handle resolving to a method key.
    pub fn read_method(
        &mut self,
        env: &mut JvmciEnv,
        name: &str,
    ) -> JvmciResult<Option<String>> {
        let handle = self.read_u8(name)?;
        if handle == 0 {
            return Ok(None);
        }
        let rt = env.runtime()?;
        match rt.resolve_metadata_handle(handle) {
            Some(MetadataTarget::Method { key }) => Ok(Some(key)),
            Some(other) => Err(JvmciException::internal_error(format!(
                "{}: expected method metadata handle, got {:?}",
                self.code_desc, other
            ))),
            None => Err(JvmciException::internal_error(format!(
                "{}: unknown method metadata handle {}",
                self.code_desc, handle
            ))),
        }
    }

    /// `read_klass`: u64 jmetadata handle resolving to a klass key.
    pub fn read_klass(
        &mut self,
        env: &mut JvmciEnv,
        name: &str,
    ) -> JvmciResult<Option<String>> {
        let handle = self.read_u8(name)?;
        if handle == 0 {
            return Ok(None);
        }
        let rt = env.runtime()?;
        match rt.resolve_metadata_handle(handle) {
            Some(MetadataTarget::Klass { key }) => Ok(Some(key)),
            Some(other) => Err(JvmciException::internal_error(format!(
                "{}: expected klass metadata handle, got {:?}",
                self.code_desc, other
            ))),
            None => Err(JvmciException::internal_error(format!(
                "{}: unknown klass metadata handle {}",
                self.code_desc, handle
            ))),
        }
    }

    /// `get_oop`: index into the object pool.
    pub fn get_oop(&self, id: usize) -> JvmciResult<JvmciObject> {
        match self.object_pool.get(id) {
            Some(o) => Ok(*o),
            None => Err(JvmciException::internal_error(format!(
                "{}: unknown direct object id {}",
                self.code_desc, id
            ))),
        }
    }

    pub fn set_virtual_objects(&mut self, objs: Vec<ScopeValue>) {
        self.virtual_objects = objs;
    }

    pub fn virtual_object_at(&self, id: usize) -> JvmciResult<ScopeValue> {
        self.virtual_objects
            .get(id)
            .cloned()
            .ok_or_else(|| {
                JvmciException::internal_error(format!(
                    "{}: invalid virtual object id {}",
                    self.code_desc, id
                ))
            })
    }
}

/// Split a `holder.name(signature)` method key into components.
pub fn split_method_key(key: &str) -> (String, String, String) {
    match key.rfind('.') {
        Some(dot) => {
            let holder = key[..dot].to_string();
            let rest = &key[dot + 1..];
            match rest.find('(') {
                Some(p) => (holder, rest[..p].to_string(), rest[p..].to_string()),
                None => (holder, rest.to_string(), String::new()),
            }
        }
        None => (String::new(), key.to_string(), String::new()),
    }
}

// ---------------------------------------------------------------------------
// CodeInstallRequest
// ---------------------------------------------------------------------------

/// Payload handed from `CodeInstaller::install` to
/// `JVMCIRuntime::register_method`.
#[derive(Debug)]
pub struct CodeInstallRequest {
    /// `holder.name(signature)` method key.
    pub method_key: String,
    pub entry_bci: i32,
    pub code_bytes: Vec<u8>,
    pub data_section: Vec<u8>,
    pub total_frame_size: i32,
    pub stack_slots: i32,
    /// PcDesc records including lower/upper sentinels.
    pub pc_descs: Vec<PcDesc>,
    pub scopes: Vec<ScopeDesc>,
    pub scope_data: Vec<u8>,
    pub exception_table: ExceptionHandlerTable,
    pub pc_handlers: PcRangeHandlerTable,
    pub oop_maps: OopMapSet,
    pub implicit_exceptions: Vec<(i32, i32)>,
    pub live_maps: Vec<DeoptLiveMap>,
    pub metadata_size: usize,
    pub nmethod_mirror_index: i32,
    pub nmethod_entry_patch_offset: i32,
    pub nmethod_mirror_name: Option<String>,
    pub failed_speculations: Vec<FailedSpeculation>,
    pub dependencies: DependencySet,
    pub offsets: CodeOffsets,
    pub has_unsafe_access: bool,
    pub has_scoped_access: bool,
    pub has_monitors: bool,
    pub has_wide_vector: bool,
    pub has_auto_box: bool,
    pub compile_id: i32,
    pub is_stub: bool,
    pub stub_name: Option<String>,
}

/// `CodeBuffer::CodeOffsets` analogue.
#[derive(Clone, Debug, Default)]
pub struct CodeOffsets {
    pub entry: i32,
    pub verified_entry: i32,
    pub osr_entry: i32,
    pub exceptions: i32,
    pub deopt: i32,
    pub deopt_mh: i32,
    pub frame_complete: i32,
}

impl CodeOffsets {
    pub fn set(&mut self, which: OffsetKind, pc_offset: i32) {
        match which {
            OffsetKind::Entry => self.entry = pc_offset,
            OffsetKind::VerifiedEntry => self.verified_entry = pc_offset,
            OffsetKind::OsrEntry => self.osr_entry = pc_offset,
            OffsetKind::Exceptions => self.exceptions = pc_offset,
            OffsetKind::Deopt => self.deopt = pc_offset,
            OffsetKind::DeoptMh => self.deopt_mh = pc_offset,
            OffsetKind::FrameComplete => self.frame_complete = pc_offset,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffsetKind {
    Entry,
    VerifiedEntry,
    OsrEntry,
    Exceptions,
    Deopt,
    DeoptMh,
    FrameComplete,
}

// ---------------------------------------------------------------------------
// Pending patches and implicit exception table
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum PendingPatch {
    /// Patch into `data_section` (constants) or `instructions` at `offset`.
    Metadata {
        offset: usize,
        kind: MetadataPatchKind,
        in_constants: bool,
        key: String,
    },
    Oop {
        offset: usize,
        in_constants: bool,
        narrow: bool,
        pool_index: usize,
    },
    /// PC-relative reference into the data section (foreign call target
    /// placeholder / data section reference).
    DataSectionRef { code_offset: usize, data_offset: usize },
    /// Foreign call destination recorded for relocation.
    ForeignCall { code_offset: usize, target: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetadataPatchKind {
    Klass,
    NarrowKlass,
    Method,
}

/// `ImplicitExceptionTable` analogue.
#[derive(Clone, Debug, Default)]
pub struct ImplicitExceptionTable {
    /// (pc_offset, dispatch_offset); dispatch_offset < 0 marks a plain
    /// deoptimization entry (`add_deoptimize`).
    entries: Vec<(i32, i32)>,
}

impl ImplicitExceptionTable {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn append(&mut self, pc_offset: i32, dispatch_offset: i32) {
        self.entries.push((pc_offset, dispatch_offset));
    }
    pub fn add_deoptimize(&mut self, pc_offset: i32) {
        self.entries.push((pc_offset, -1));
    }
    pub fn entries(&self) -> &[(i32, i32)] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------
// CodeInstaller
// ---------------------------------------------------------------------------

/// `CodeInstaller` — decodes the stream and produces a
/// `CodeInstallRequest`.
pub struct CodeInstaller<'a> {
    env: &'a mut JvmciEnv,
    offsets: CodeOffsets,
    nmethod_entry_patch_offset: i32,
    code_size: i32,
    total_frame_size: i32,
    orig_pc_offset: i32,
    parameter_count: i32,
    constants_size: u32,
    has_monitors: bool,
    has_wide_vector: bool,
    has_auto_box: bool,
    next_call_type: u8,
    invoke_mark_pc: i64,
    instructions: Vec<u8>,
    constants: Vec<u8>,
    /// Recorded oop identities (dedup like OopRecorder::find_index).
    oop_recorder: Vec<String>,
    dependencies: DependencySet,
    exception_handler_table: ExceptionHandlerTable,
    implicit_exception_table: ImplicitExceptionTable,
    sites_count: i32,
    pending_patches: Vec<PendingPatch>,
    scope_writer: Option<ScopeDescWriter>,
    /// Offset of the last scope record written by the writer
    /// (innermost frame of the most recent record_scope).
    last_scope_offset: i32,
    decoded_scopes: Vec<ScopeDesc>,
    pc_descs: Vec<PcDesc>,
    oop_map_set: OopMapSet,
    live_maps: Vec<DeoptLiveMap>,
    constants_alignment: u8,
}

impl<'a> CodeInstaller<'a> {
    /// `CodeInstaller(jvmci_env)`.
    pub fn new(env: &'a mut JvmciEnv) -> Self {
        Self {
            env,
            offsets: CodeOffsets::default(),
            nmethod_entry_patch_offset: -1,
            code_size: 0,
            total_frame_size: 0,
            orig_pc_offset: -1,
            parameter_count: 0,
            constants_size: 0,
            has_monitors: false,
            has_wide_vector: false,
            has_auto_box: false,
            next_call_type: mark::INVALID_MARK,
            invoke_mark_pc: -1,
            instructions: Vec::new(),
            constants: Vec::new(),
            oop_recorder: Vec::new(),
            dependencies: DependencySet::new(),
            exception_handler_table: ExceptionHandlerTable::new(8),
            implicit_exception_table: ImplicitExceptionTable::new(),
            sites_count: 0,
            pending_patches: Vec::new(),
            scope_writer: None,
            last_scope_offset: 0,
            decoded_scopes: Vec::new(),
            pc_descs: Vec::new(),
            oop_map_set: OopMapSet::new(),
            live_maps: Vec::new(),
            constants_alignment: 8,
        }
    }

    // ---- VMReg helpers -------------------------------------------------

    /// `getVMRegFromLocation`.
    fn get_vm_reg_from_location(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
    ) -> JvmciResult<VMReg> {
        let reg = stream.read_u2("register")?;
        let offset = stream.read_u2("offset")?;
        if reg != NO_REGISTER {
            let hotspot_reg = Self::get_hotspot_reg(reg as i32)?;
            if offset % 4 == 0 {
                let n = (offset / 4) as i32;
                return Ok(if hotspot_reg.is_reg() {
                    VMReg::reg(hotspot_reg.value() + n)
                } else {
                    VMReg::stack_slot(hotspot_reg.reg2stack() + n)
                });
            }
            return Err(JvmciException::internal_error(format!(
                "unaligned subregister offset {} in oop map",
                offset
            )));
        }
        if offset % 4 == 0 {
            let vm_reg = VMReg::stack2reg((offset / 4) as i32);
            if !OopMapValue::legal_vm_reg_name(vm_reg) {
                return Err(JvmciException::internal_error(format!(
                    "stack offset {} is too large to be encoded in OopMap",
                    offset
                )));
            }
            Ok(vm_reg)
        } else {
            Err(JvmciException::internal_error(format!(
                "unaligned stack offset {} in oop map",
                offset
            )))
        }
    }

    /// `CodeInstaller::get_hotspot_reg`.
    pub fn get_hotspot_reg(jvmci_reg: i32) -> JvmciResult<VMReg> {
        if jvmci_reg < 0 {
            return Err(JvmciException::internal_error(format!(
                "invalid JVMCI register number {}",
                jvmci_reg
            )));
        }
        Ok(VMReg::reg(jvmci_reg))
    }

    /// `is_general_purpose_reg`.
    pub fn is_general_purpose_reg(vm_reg: VMReg) -> bool {
        vm_reg.is_reg() && vm_reg.value() <= 15
    }

    /// `create_oop_map`.
    fn create_oop_map(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        debug_info_flags: u8,
    ) -> JvmciResult<OopMap> {
        debug_assert!(is_set(debug_info_flags, dif::HAS_REFERENCE_MAP));
        let max_register_size = stream.read_u2("maxRegisterSize")?;
        if !self.has_wide_vector && is_wide_vector(max_register_size) {
            self.has_wide_vector = true;
        }
        let length = stream.read_u2("referenceMap:length")? as usize;
        let mut map =
            OopMap::new(self.total_frame_size.max(0) as usize, self.parameter_count.max(0) as usize);
        for _ in 0..length {
            let has_derived = stream.read_bool("hasDerived")?;
            let bytes = stream.read_u2("sizeInBytes")?;
            let vm_reg = self.get_vm_reg_from_location(stream)?;
            if has_derived {
                if bytes == 8 {
                    let base_reg = self.get_vm_reg_from_location(stream)?;
                    map.set_derived_oop(vm_reg, base_reg);
                } else {
                    return Err(JvmciException::internal_error(format!(
                        "invalid derived oop size in ReferenceMap: {}",
                        bytes
                    )));
                }
            } else if bytes == 8 {
                map.set_oop(vm_reg);
            } else if bytes == 4 {
                map.set_narrowoop(vm_reg);
            } else {
                return Err(JvmciException::internal_error(format!(
                    "invalid oop size in ReferenceMap: {}",
                    bytes
                )));
            }
        }
        if is_set(debug_info_flags, dif::HAS_CALLEE_SAVE_INFO) {
            let length = stream.read_u2("calleeSaveInfo:length")? as usize;
            for _ in 0..length {
                let jvmci_reg = stream.read_u2("register")? as i32;
                let hotspot_reg = Self::get_hotspot_reg(jvmci_reg)?;
                let jvmci_slot = stream.read_u2("slot")?;
                let hotspot_slot = jvmci_slot * 2;
                let slot_reg = VMReg::stack2reg(hotspot_slot as i32);
                map.set_callee_saved(slot_reg, hotspot_reg);
                let slot_reg_hi = VMReg::stack2reg(hotspot_slot as i32 + 1);
                map.set_callee_saved(slot_reg_hi, hotspot_reg);
            }
        }
        Ok(map)
    }

    // ---- scope value decoding -------------------------------------------

    /// `to_primitive_value`.
    fn to_primitive_value(
        &mut self,
        raw: i64,
        type_: BasicType,
    ) -> JvmciResult<(ScopeValue, Option<ScopeValue>)> {
        match type_ {
            BasicType::Int | BasicType::Float => Ok((
                ScopeValue::ConstantInt(raw as i32),
                None,
            )),
            BasicType::Long | BasicType::Double => Ok((
                ScopeValue::ConstantLong(raw),
                Some(ScopeValue::ConstantInt(1)),
            )),
            _ => Err(JvmciException::internal_error(format!(
                "unexpected primitive constant type {type_:?}"
            ))),
        }
    }

    /// `read_oop`: resolve an oop reference from the stream.
    fn read_oop(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        tag_: u8,
    ) -> JvmciResult<JvmciObject> {
        match tag_ {
            tag::OBJECT_ID => {
                let id = stream.read_u1("id")? as usize;
                stream.get_oop(id)
            }
            tag::OBJECT_ID2 => {
                let id = stream.read_u2("id:2")? as usize;
                stream.get_oop(id)
            }
            tag::JOBJECT => {
                let handle = stream.read_u8("jobject")? as i64;
                self.env
                    .resolve_oop_handle(handle)
                    .map(|o| jvmciObject::alloc_object(true, JvmciHeapValue::Oop(o)))
                    .ok_or_else(|| {
                        JvmciException::internal_error("constant was unexpectedly null")
                    })
            }
            _ => Err(JvmciException::internal_error(format!(
                "unexpected oop tag: {}",
                tag_
            ))),
        }
    }

    /// `get_scope_value`.
    fn get_scope_value(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        tag_: u8,
        type_: BasicType,
    ) -> JvmciResult<(ScopeValue, Option<ScopeValue>)> {
        use tag::*;
        match tag_ {
            ILLEGAL => {
                if type_ != BasicType::Illegal {
                    return Err(JvmciException::internal_error(format!(
                        "unexpected illegal value, expected {type_:?}"
                    )));
                }
                Ok((ScopeValue::Marker, None))
            }
            REGISTER_PRIMITIVE | REGISTER_NARROW_OOP | REGISTER_OOP | REGISTER_VECTOR => {
                let number = stream.read_u2("register")? as i32;
                let hotspot_reg = Self::get_hotspot_reg(number)?;
                if Self::is_general_purpose_reg(hotspot_reg) {
                    let location_type = if type_ == BasicType::Object {
                        if tag_ == REGISTER_NARROW_OOP {
                            LocationType::NarrowOop
                        } else {
                            LocationType::Oop
                        }
                    } else if type_ == BasicType::Long {
                        LocationType::Lng
                    } else if matches!(
                        type_,
                        BasicType::Int
                            | BasicType::Float
                            | BasicType::Short
                            | BasicType::Char
                            | BasicType::Byte
                            | BasicType::Boolean
                    ) {
                        LocationType::IntInLong
                    } else {
                        return Err(JvmciException::internal_error(format!(
                            "unexpected type {type_:?} in CPU register"
                        )));
                    };
                    let value = ScopeValue::Location(Location::new_reg_loc(
                        location_type,
                        hotspot_reg.value(),
                    ));
                    if type_ == BasicType::Long {
                        return Ok((value.clone(), Some(value)));
                    }
                    Ok((value, None))
                } else {
                    let location_type = if type_ == BasicType::Float {
                        LocationType::Normal
                    } else if type_ == BasicType::Double {
                        LocationType::Dbl
                    } else if type_ == BasicType::Object && tag_ == REGISTER_VECTOR {
                        LocationType::Vector
                    } else {
                        return Err(JvmciException::internal_error(format!(
                            "unexpected type {type_:?} in floating point register"
                        )));
                    };
                    let value = ScopeValue::Location(Location::new_reg_loc(
                        location_type,
                        hotspot_reg.value(),
                    ));
                    if type_ == BasicType::Double {
                        return Ok((value.clone(), Some(value)));
                    }
                    Ok((value, None))
                }
            }
            STACK_SLOT4_PRIMITIVE | STACK_SLOT4_NARROW_OOP | STACK_SLOT4_OOP
            | STACK_SLOT4_VECTOR => {
                let offset = stream.read_s4("offset4")?;
                self.stack_slot_value(stream, offset, tag_, type_)
            }
            STACK_SLOT_PRIMITIVE | STACK_SLOT_NARROW_OOP | STACK_SLOT_OOP
            | STACK_SLOT_VECTOR => {
                let offset = stream.read_s2("offset")? as i32;
                self.stack_slot_value(stream, offset, tag_, type_)
            }
            NULL_CONSTANT => Ok((ScopeValue::ConstantOop("null".to_string()), None)),
            RAW_CONSTANT => {
                let raw = stream.read_u8("primitive")? as i64;
                Ok((ScopeValue::ConstantLong(raw), None))
            }
            PRIMITIVE_0 => self.to_primitive_value(0, type_),
            PRIMITIVE4 => {
                let raw = stream.read_s4("primitive4")? as i64;
                self.to_primitive_value(raw, type_)
            }
            PRIMITIVE8 => {
                let raw = stream.read_s8("primitive8")?;
                self.to_primitive_value(raw, type_)
            }
            VIRTUAL_OBJECT_ID => {
                let id = stream.read_u1("id")? as usize;
                Ok((stream.virtual_object_at(id)?, None))
            }
            VIRTUAL_OBJECT_ID2 => {
                let id = stream.read_u2("id:2")? as usize;
                Ok((stream.virtual_object_at(id)?, None))
            }
            OBJECT_ID | OBJECT_ID2 | JOBJECT => {
                let obj = self.read_oop(stream, tag_)?;
                let idx = self.record_oop(obj);
                Ok((ScopeValue::ConstantOop(format!("oop#{}", idx)), None))
            }
            _ => Err(JvmciException::internal_error(format!(
                "unexpected tag in scope: {}",
                tag_
            ))),
        }
    }

    fn stack_slot_value(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        mut offset: i32,
        tag_: u8,
        type_: BasicType,
    ) -> JvmciResult<(ScopeValue, Option<ScopeValue>)> {
        if stream.read_bool("addRawFrameSize")? {
            offset += self.total_frame_size;
        }
        let location_type = if type_ == BasicType::Object {
            if tag_ == tag::STACK_SLOT_VECTOR || tag_ == tag::STACK_SLOT4_VECTOR {
                LocationType::Vector
            } else if tag_ == tag::STACK_SLOT_NARROW_OOP
                || tag_ == tag::STACK_SLOT4_NARROW_OOP
            {
                LocationType::NarrowOop
            } else {
                LocationType::Oop
            }
        } else if type_ == BasicType::Long {
            LocationType::Lng
        } else if type_ == BasicType::Double {
            LocationType::Dbl
        } else if matches!(
            type_,
            BasicType::Int
                | BasicType::Float
                | BasicType::Short
                | BasicType::Char
                | BasicType::Byte
                | BasicType::Boolean
        ) {
            LocationType::Normal
        } else {
            return Err(JvmciException::internal_error(format!(
                "unexpected type {type_:?} in stack slot"
            )));
        };
        let value = ScopeValue::Location(Location::new_stk_loc(location_type, offset));
        if type_ == BasicType::Double || type_ == BasicType::Long {
            return Ok((value.clone(), Some(value)));
        }
        Ok((value, None))
    }

    /// `record_object_value`.
    fn record_object_value(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        sv: &mut ObjectValue,
    ) -> JvmciResult<()> {
        let klass_desc = sv
            .klass()
            .map(|k| format!("{:?}", k))
            .unwrap_or_default();
        let is_long_array = klass_desc.contains("[J") || klass_desc.contains("long");
        let is_byte_array = klass_desc.contains("[B") || klass_desc.contains("byte");

        let length = stream.read_u2("values:length")? as usize;
        let mut fields = Vec::with_capacity(length * 2);
        for _ in 0..length {
            let type_ = basic_type_from_wire(stream.read_u1("basicType")?)?;
            let tag_ = stream.read_u1("tag")?;
            let mut second = None;
            let value = if tag_ == tag::ILLEGAL {
                if is_byte_array && type_ == BasicType::Illegal {
                    ScopeValue::Marker
                } else {
                    if type_ == BasicType::Double || type_ == BasicType::Long {
                        second = Some(ScopeValue::Marker);
                    }
                    ScopeValue::Marker
                }
            } else {
                let (v, s) = self.get_scope_value(stream, tag_, type_)?;
                second = s;
                v
            };
            if is_long_array && second.is_none() {
                second = Some(ScopeValue::ConstantInt(0));
            }
            if is_byte_array
                && second.is_some()
                && (type_ == BasicType::Double || type_ == BasicType::Long)
            {
                second = None;
            }
            if let Some(s) = second {
                fields.push(s);
            }
            fields.push(value);
        }
        sv.set_field_values(fields);
        Ok(())
    }

    /// `read_local_or_stack_values`.
    fn read_local_or_stack_values(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        frame_flags: u8,
        is_locals: bool,
    ) -> JvmciResult<Vec<ScopeValue>> {
        let want = if is_locals {
            frame_flag::HAS_LOCALS
        } else {
            frame_flag::HAS_STACK
        };
        if !is_set(frame_flags, want) {
            return Ok(Vec::new());
        }
        let length = if is_locals {
            stream.read_u2("numLocals")?
        } else {
            stream.read_u2("numStack")?
        } as usize;
        let mut values = Vec::with_capacity(length * 2);
        for _ in 0..length {
            let type_ = basic_type_from_wire(stream.read_u1("basicType")?)?;
            let tag_ = stream.read_u1("tag")?;
            let (first, second) = self.get_scope_value(stream, tag_, type_)?;
            if let Some(second) = second {
                let _bt = stream.read_u1("basicType")?;
                let t2 = stream.read_u1("tag")?;
                if t2 != tag::ILLEGAL {
                    return Err(JvmciException::internal_error(
                        "double-slot value not followed by Value.ILLEGAL",
                    ));
                }
                values.push(second);
            }
            values.push(first);
        }
        Ok(values)
    }

    /// `read_monitor_values`.
    fn read_monitor_values(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        frame_flags: u8,
    ) -> JvmciResult<Vec<MonitorValue>> {
        if !is_set(frame_flags, frame_flag::HAS_LOCKS) {
            return Ok(Vec::new());
        }
        self.has_monitors = true;
        let length = stream.read_u2("numLocks")? as usize;
        let mut monitors = Vec::with_capacity(length);
        for _ in 0..length {
            let eliminated = stream.read_bool("isEliminated")?;
            let owner_tag = stream.read_u1("tag")?;
            let (owner_value, _) = self.get_scope_value(stream, owner_tag, BasicType::Object)?;
            let lock_tag = stream.read_u1("tag")?;
            let (lock_value, _) = self.get_scope_value(stream, lock_tag, BasicType::Long)?;
            let lock_data_loc = match lock_value {
                ScopeValue::Location(l) => l,
                _ => return Err(JvmciException::internal_error("invalid monitor location")),
            };
            let mv = if eliminated {
                MonitorValue::eliminated(owner_value, lock_data_loc)
            } else {
                MonitorValue::new(owner_value, lock_data_loc)
            };
            monitors.push(mv);
        }
        Ok(monitors)
    }

    /// `read_virtual_objects`.
    fn read_virtual_objects(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
    ) -> JvmciResult<()> {
        let length = stream.read_u2("virtualObjects:length")? as usize;
        if length == 0 {
            stream.set_virtual_objects(Vec::new());
            return Ok(());
        }
        let mut objects: Vec<ObjectValue> = Vec::with_capacity(length);
        for id in 0..length {
            let klass_key = stream.read_klass(self.env, "type")?;
            let is_auto_box = stream.read_bool("isAutoBox")?;
            if is_auto_box {
                self.has_auto_box = true;
            }
            let klass_sv = ScopeValue::ConstantOop(
                klass_key.unwrap_or_else(|| "java/lang/Object".to_string()),
            );
            let mut sv = ObjectValue::new(id as i32);
            sv.set_klass(klass_sv);
            sv.set_auto_box(is_auto_box);
            objects.push(sv);
        }
        for i in 0..length {
            let mut sv = objects.swap_remove(i);
            self.record_object_value(stream, &mut sv)?;
            objects.push(sv);
        }
        stream.set_virtual_objects(objects.into_iter().map(ScopeValue::Object).collect());
        Ok(())
    }

    /// `map_jvmci_bci`.
    fn map_jvmci_bci(bci_: i32) -> i32 {
        if bci_ < 0 {
            match bci_ {
                AFTER_EXCEPTION_BCI => crate::share::compiler::compilerDefinitions::MethodCompilation::AFTER_EXCEPTION_BCI,
                _ => bci_,
            }
        } else {
            bci_
        }
    }

    /// `record_scope`.
    #[allow(clippy::too_many_arguments)]
    fn record_scope(
        &mut self,
        pc_offset: i32,
        stream: &mut HotSpotCompiledCodeStream,
        debug_info_flags: u8,
        full_info: bool,
        is_mh_invoke: bool,
        return_oop: bool,
        pc_flags: i32,
    ) -> JvmciResult<()> {
        if full_info {
            self.read_virtual_objects(stream)?;
        }
        // Take the writer out so `self` methods can run while it is in
        // use; restore it before returning (on error the install aborts
        // anyway).
        let mut writer = self
            .scope_writer
            .take()
            .unwrap_or_else(ScopeDescWriter::new);
        let result = (|| -> JvmciResult<()> {
            let mut sender_offset =
                crate::share::code::scopeDesc::NO_SENDER_DECODE_OFFSET;
            let mut frame_index = 0usize;
            if is_set(debug_info_flags, dif::HAS_FRAMES) {
                let depth = stream.read_u2("depth")? as usize;
                for _ in 0..depth {
                    let method_key = stream.read_method(self.env, "method")?;
                    let mut bci_ = stream.read_s4("bci")?;
                    // HotSpot BytecodeFrame BCI constants map onto the
                    // same negative sentinels.
                    if bci_ == BEFORE_BCI {
                        bci_ = SYNCHRONIZATION_ENTRY_BCI;
                    }
                    bci_ = Self::map_jvmci_bci(bci_);
                    let (holder, name, sig) = method_key
                        .as_deref()
                        .map(split_method_key)
                        .unwrap_or_else(|| {
                            (String::new(), "<unknown>".to_string(), String::new())
                        });

                    let mut rethrow = false;
                    let mut reexecute = false;
                    let mut locals_off = crate::share::code::scopeDesc::SERIALIZED_NULL;
                    let mut exprs_off = crate::share::code::scopeDesc::SERIALIZED_NULL;
                    let mut mons_off = crate::share::code::scopeDesc::SERIALIZED_NULL;
                    let objs_off = crate::share::code::scopeDesc::SERIALIZED_NULL;

                    if full_info {
                        let frame_flags = stream.read_u1("flags")?;
                        rethrow = is_set(frame_flags, frame_flag::RETHROW_EXCEPTION);
                        if bci_ >= 0 {
                            reexecute = !is_set(frame_flags, frame_flag::DURING_CALL);
                        }
                        let locals =
                            self.read_local_or_stack_values(stream, frame_flags, true)?;
                        let exprs =
                            self.read_local_or_stack_values(stream, frame_flags, false)?;
                        let monitors = self.read_monitor_values(stream, frame_flags)?;
                        locals_off = writer.write_scope_values(&locals);
                        exprs_off = writer.write_scope_values(&exprs);
                        mons_off = writer.write_monitors(&monitors);
                    }

                    let rec_offset = writer.write_scope(
                        sender_offset,
                        &holder,
                        &name,
                        &sig,
                        bci_,
                        locals_off,
                        exprs_off,
                        mons_off,
                        objs_off,
                    );
                    sender_offset = rec_offset;
                    self.last_scope_offset = rec_offset;
                    frame_index += 1;

                    let mut desc = ScopeDesc::new(holder, name, sig, bci_, frame_index > 1);
                    desc.apply_pc_desc_flags(
                        reexecute && frame_index == 1,
                        return_oop && frame_index == 1,
                        rethrow,
                        false,
                        false,
                    );
                    let _ = is_mh_invoke;
                    self.decoded_scopes.push(desc);
                }
            }
            let scope_off = if is_set(debug_info_flags, dif::HAS_FRAMES) {
                self.last_scope_offset
            } else {
                0
            };
            let mut pd = PcDesc::new(pc_offset, scope_off, pc_flags);
            pd.set_bci(PcDesc::UNKNOWN_BCI);
            self.pc_descs.push(pd);
            Ok(())
        })();
        self.scope_writer = Some(writer);
        result
    }

    // ---- dependencies ------------------------------------------------------

    /// `initialize_dependencies`.
    fn initialize_dependencies(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        code_flags: u8,
    ) -> JvmciResult<()> {
        if is_set(code_flags, hcc::HAS_ASSUMPTIONS) {
            let length = stream.read_u2("assumptions:length")? as usize;
            for _ in 0..length {
                let tag_ = stream.read_u1("tag")?;
                match tag_ {
                    tag::NO_FINALIZABLE_SUBCLASS => {
                        if let Some(k) = stream.read_klass(self.env, "receiverType")? {
                            self.dependencies.add(Dependency::new_unchecked(
                                DepType::NoFinalizableSubclasses,
                                vec![DependencyArgument::Klass(k)],
                            ));
                        }
                    }
                    tag::CONCRETE_SUBTYPE => {
                        let context = stream.read_klass(self.env, "context")?;
                        let subtype = stream.read_klass(self.env, "subtype")?;
                        if let (Some(c), Some(s)) = (context, subtype) {
                            self.dependencies.add(Dependency::new_unchecked(
                                DepType::AbstractWithUniqueConcreteSubtype,
                                vec![
                                    DependencyArgument::Klass(c),
                                    DependencyArgument::Klass(s),
                                ],
                            ));
                        }
                    }
                    tag::LEAF_TYPE => {
                        if let Some(k) = stream.read_klass(self.env, "context")? {
                            self.dependencies.add_leaf_type(&k);
                        }
                    }
                    tag::CONCRETE_METHOD => {
                        let context = stream.read_klass(self.env, "context")?;
                        let impl_key = stream.read_method(self.env, "impl")?;
                        if let (Some(c), Some(m)) = (context, impl_key) {
                            let (h, n, s) = split_method_key(&m);
                            self.dependencies
                                .add_unique_concrete_method(&h, &n, &s, &c);
                        }
                    }
                    tag::CALLSITE_TARGET_VALUE => {
                        let obj_tag = stream.read_u1("tag")?;
                        let _call_site = self.read_oop(stream, obj_tag)?;
                        let mh_tag = stream.read_u1("tag")?;
                        let mh = self.read_oop(stream, mh_tag)?;
                        let desc = format!("{:?}", mh);
                        self.dependencies.add_call_site_target(&desc);
                    }
                    other => {
                        return Err(JvmciException::internal_error(format!(
                            "unexpected assumption tag {}",
                            other
                        )))
                    }
                }
            }
        }
        if is_set(code_flags, hcc::HAS_METHODS) {
            let length = stream.read_u2("methods:length")? as usize;
            for _ in 0..length {
                if let Some(m) = stream.read_method(self.env, "method")? {
                    let (h, n, s) = split_method_key(&m);
                    self.dependencies.add_evol_method(&h, &n, &s);
                }
            }
        }
        Ok(())
    }

    // ---- buffer initialization --------------------------------------------

    /// `initialize_fields`.
    fn initialize_fields(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        code_flags: u8,
    ) -> JvmciResult<()> {
        self.sites_count = stream.read_s4("sites:length")?;
        self.code_size = stream.read_s4("targetCodeSize")?;
        self.total_frame_size = stream.read_s4("totalFrameSize")?;
        if !is_set(code_flags, hcc::HAS_DEOPT_RESCUE_SLOT) {
            self.orig_pc_offset = -1;
        } else {
            let mut off = stream.read_s4("offset")?;
            if stream.read_bool("addRawFrameSize")? {
                off += self.total_frame_size;
            }
            if off < 0 {
                return Err(JvmciException::internal_error(format!(
                    "invalid deopt rescue slot: {}",
                    off
                )));
            }
            self.orig_pc_offset = off;
        }
        self.constants_size = stream.read_u4("dataSectionSize")?;
        self.constants_alignment = stream.read_u1("dataSectionAlignment")?;
        if self.constants_alignment == 0 || (8usize % self.constants_alignment as usize) != 0
        {
            return Err(JvmciException::internal_error(format!(
                "invalid data section alignment: {}",
                self.constants_alignment
            )));
        }
        self.next_call_type = mark::INVALID_MARK;
        self.has_monitors = false;
        self.has_wide_vector = false;
        self.nmethod_entry_patch_offset = -1;
        Ok(())
    }

    /// `estimate_stubs_size`.
    fn estimate_stubs_size(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
    ) -> JvmciResult<i64> {
        let static_call_stubs = stream.read_u2("numStaticCallStubs")? as i64;
        let trampoline_stubs = stream.read_u2("numTrampolineStubs")? as i64;
        // CompiledDirectCall::to_interp_stub_size / to_trampoline_stub_size
        // are architecture constants; use conservative x86-64 sizes.
        Ok(static_call_stubs * 32 + trampoline_stubs * 24)
    }

    /// `as_read_oop_tag`.
    fn as_read_oop_tag(&mut self, patch_object_tag: u8) -> JvmciResult<u8> {
        match patch_object_tag {
            tag::PATCH_OBJECT_ID | tag::PATCH_NARROW_OBJECT_ID => Ok(tag::OBJECT_ID),
            tag::PATCH_OBJECT_ID2 | tag::PATCH_NARROW_OBJECT_ID2 => Ok(tag::OBJECT_ID2),
            tag::PATCH_NARROW_JOBJECT | tag::PATCH_JOBJECT => Ok(tag::JOBJECT),
            other => Err(JvmciException::internal_error(format!(
                "unknown object patch tag: {}",
                other
            ))),
        }
    }

    /// `initialize_buffer`.
    fn initialize_buffer(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        compiled_code: JvmciObject,
        code_flags: u8,
    ) -> JvmciResult<CodeInstallResult> {
        let stubs_size = self.estimate_stubs_size(stream)?;
        let align = |v: usize, a: usize| (v + a - 1) & !(a - 1);
        let total_size = align(self.constants_size as usize, 16)
            + align(self.code_size.max(0) as usize, 16)
            + stubs_size.max(0) as usize;
        let limit = jvmciGlobals::snapshot().jvmci_nmethod_size_limit.max(0) as usize;
        if total_size > limit {
            return Ok(CodeInstallResult::CodeTooLarge);
        }

        let (data_bytes, code_bytes) = read_compiled_code_payloads(self.env, compiled_code)?;
        self.constants = data_bytes;
        if self.constants.len() < self.constants_size as usize {
            return Err(JvmciException::internal_error(format!(
                "data section too small: {} < {}",
                self.constants.len(),
                self.constants_size
            )));
        }
        self.constants.truncate(self.constants_size as usize);
        self.instructions = code_bytes;
        if self.instructions.len() < self.code_size as usize {
            return Err(JvmciException::internal_error(format!(
                "target code too small: {} < {}",
                self.instructions.len(),
                self.code_size
            )));
        }
        self.instructions.truncate(self.code_size as usize);

        // Data-section patches.
        let length = stream.read_u2("dataSectionPatches:length")? as usize;
        for _ in 0..length {
            let pc_offset = stream.read_u4("patch:pcOffset")? as usize;
            if pc_offset + 8 > self.constants.len() {
                return Err(JvmciException::internal_error(format!(
                    "constant patch offset {} outside data section (size {})",
                    pc_offset,
                    self.constants.len()
                )));
            }
            let tag_ = stream.read_u1("tag")?;
            match tag_ {
                tag::PATCH_METHOD | tag::PATCH_KLASS => {
                    let key = if tag_ == tag::PATCH_METHOD {
                        stream.read_method(self.env, "patch:method")?
                    } else {
                        stream.read_klass(self.env, "patch:klass")?
                    };
                    let kind = if tag_ == tag::PATCH_METHOD {
                        MetadataPatchKind::Method
                    } else {
                        MetadataPatchKind::Klass
                    };
                    self.pending_patches.push(PendingPatch::Metadata {
                        offset: pc_offset,
                        kind,
                        in_constants: true,
                        key: key.unwrap_or_default(),
                    });
                }
                tag::PATCH_NARROW_KLASS => {
                    let key = stream.read_klass(self.env, "patch:klass")?;
                    self.pending_patches.push(PendingPatch::Metadata {
                        offset: pc_offset,
                        kind: MetadataPatchKind::NarrowKlass,
                        in_constants: true,
                        key: key.unwrap_or_default(),
                    });
                }
                tag::PATCH_OBJECT_ID
                | tag::PATCH_OBJECT_ID2
                | tag::PATCH_NARROW_OBJECT_ID
                | tag::PATCH_NARROW_OBJECT_ID2
                | tag::PATCH_JOBJECT
                | tag::PATCH_NARROW_JOBJECT => {
                    let narrow = matches!(
                        tag_,
                        tag::PATCH_NARROW_OBJECT_ID
                            | tag::PATCH_NARROW_OBJECT_ID2
                            | tag::PATCH_NARROW_JOBJECT
                    );
                    let read_tag = self.as_read_oop_tag(tag_)?;
                    self.record_oop_patch(stream, pc_offset, read_tag, narrow, true)?;
                }
                other => {
                    return Err(JvmciException::internal_error(format!(
                        "invalid constant tag: {}",
                        other
                    )))
                }
            }
        }

        // Sites.
        for _ in 0..self.sites_count.max(0) {
            let pc_offset = stream.read_s4("site:pcOffset")?;
            let site_tag = stream.read_u1("tag")?;
            match site_tag {
                tag::SITE_FOREIGN_CALL
                | tag::SITE_FOREIGN_CALL_NO_DEBUG_INFO
                | tag::SITE_CALL => self.site_call(stream, site_tag, pc_offset)?,
                tag::SITE_SAFEPOINT
                | tag::SITE_IMPLICIT_EXCEPTION
                | tag::SITE_IMPLICIT_EXCEPTION_DISPATCH => {
                    self.site_safepoint(stream, pc_offset, site_tag)?
                }
                tag::SITE_INFOPOINT => self.site_infopoint(stream, pc_offset)?,
                tag::SITE_MARK => self.site_mark(stream, pc_offset)?,
                tag::SITE_DATA_PATCH => self.site_data_patch(stream, pc_offset)?,
                tag::SITE_EXCEPTION_HANDLER => {
                    self.site_exception_handler(pc_offset, stream)?
                }
                other => {
                    return Err(JvmciException::internal_error(format!(
                        "unexpected site tag at offset {}: {}",
                        stream.offset(),
                        other
                    )))
                }
            }
        }

        if is_set(code_flags, hcc::HAS_COMMENTS) {
            let length = stream.read_u2("comments:length")? as usize;
            for _ in 0..length {
                let _pc = stream.read_u4("comment:pcOffset")?;
                let _text = stream.read_utf8("comment:text")?;
            }
        }

        if self.has_auto_box {
            crate::share::jvmci::jvmci::JVMCI::ensure_box_caches_initialized()
                .map_err(|e| JvmciException::internal_error(e.describe()))?;
        }
        Ok(CodeInstallResult::Ok)
    }

    // ---- site handlers ------------------------------------------------------

    /// `site_ExceptionHandler`.
    fn site_exception_handler(
        &mut self,
        pc_offset: i32,
        stream: &mut HotSpotCompiledCodeStream,
    ) -> JvmciResult<()> {
        let handler_offset = stream.read_u4("site:handlerPos")? as i32;
        self.exception_handler_table
            .add_entry(HandlerTableEntry::new(1, pc_offset, 0));
        self.exception_handler_table
            .add_entry(HandlerTableEntry::new(-1, handler_offset, 0));
        Ok(())
    }

    /// `site_Safepoint`.
    fn site_safepoint(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        pc_offset: i32,
        tag_: u8,
    ) -> JvmciResult<()> {
        let flags = stream.read_u1("debugInfo:flags")?;
        let map = self.create_oop_map(stream, flags)?;
        self.oop_map_set.add_gc_map(pc_offset, map);
        self.record_scope(pc_offset, stream, flags, true, false, false, 0)?;
        if self.orig_pc_offset < 0 {
            return Err(JvmciException::internal_error(
                "method contains safepoint, but has no deopt rescue slot",
            ));
        }
        if tag_ == tag::SITE_IMPLICIT_EXCEPTION_DISPATCH {
            let dispatch = stream.read_s4("dispatchOffset")?;
            self.implicit_exception_table.append(pc_offset, dispatch);
        } else if tag_ == tag::SITE_IMPLICIT_EXCEPTION {
            self.implicit_exception_table.add_deoptimize(pc_offset);
        }
        Ok(())
    }

    /// `site_Infopoint`.
    fn site_infopoint(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        pc_offset: i32,
    ) -> JvmciResult<()> {
        let flags = stream.read_u1("debugInfo:flags")?;
        self.record_scope(pc_offset, stream, flags, false, false, false, 0)?;
        Ok(())
    }

    /// `site_Call`.
    fn site_call(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        tag_: u8,
        pc_offset: i32,
    ) -> JvmciResult<()> {
        let target = stream.read_u8("target")? as i64;
        let mut method_key: Option<String> = None;
        let mut direct_call = false;
        if tag_ == tag::SITE_CALL {
            method_key = self.resolve_call_target(target);
            direct_call = stream.read_bool("direct")?;
            if method_key.is_none() {
                return Err(JvmciException::null_pointer("call target is null"));
            }
        }
        // pd_next_offset analogue: instruction length at pc_offset.
        let next_pc_offset = pc_offset + call_site_size(&self.instructions, pc_offset as usize);

        if tag_ != tag::SITE_FOREIGN_CALL_NO_DEBUG_INFO {
            let flags = stream.read_u1("debugInfo:flags")?;
            let map = self.create_oop_map(stream, flags)?;
            self.oop_map_set.add_gc_map(next_pc_offset, map);
            let is_mh_invoke = method_key
                .as_deref()
                .map(|k| k.contains("invoke") || k.contains("LambdaForm"))
                .unwrap_or(false)
                && direct_call;
            let return_oop = method_key
                .as_deref()
                .and_then(|k| k.rsplit(')').next().map(|r| r.to_string()))
                .map(|r| !matches!(r.as_str(), "V" | "I" | "J" | "F" | "D" | "Z" | "B" | "C" | "S" | ""))
                .unwrap_or(false);
            self.record_scope(
                next_pc_offset,
                stream,
                flags,
                true,
                is_mh_invoke,
                return_oop,
                0,
            )?;
        }

        if tag_ != tag::SITE_CALL {
            self.pending_patches.push(PendingPatch::ForeignCall {
                code_offset: pc_offset as usize,
                target,
            });
        } else if self.next_call_type == mark::INVOKESTATIC
            || self.next_call_type == mark::INVOKESPECIAL
        {
            // to_interp stub space was reserved by estimate_stubs_size.
        }
        self.next_call_type = mark::INVALID_MARK;
        Ok(())
    }

    /// `site_DataPatch`.
    fn site_data_patch(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        pc_offset: i32,
    ) -> JvmciResult<()> {
        let tag_ = stream.read_u1("tag")?;
        match tag_ {
            tag::PATCH_OBJECT_ID
            | tag::PATCH_OBJECT_ID2
            | tag::PATCH_NARROW_OBJECT_ID
            | tag::PATCH_NARROW_OBJECT_ID2
            | tag::PATCH_JOBJECT
            | tag::PATCH_NARROW_JOBJECT => {
                let narrow = matches!(
                    tag_,
                    tag::PATCH_NARROW_OBJECT_ID
                        | tag::PATCH_NARROW_OBJECT_ID2
                        | tag::PATCH_NARROW_JOBJECT
                );
                let read_tag = self.as_read_oop_tag(tag_)?;
                self.record_oop_patch(stream, pc_offset as usize, read_tag, narrow, false)?;
            }
            tag::PATCH_METHOD | tag::PATCH_KLASS | tag::PATCH_NARROW_KLASS => {
                let key = if tag_ == tag::PATCH_METHOD {
                    stream.read_method(self.env, "patch:method")?
                } else {
                    stream.read_klass(self.env, "patch:klass")?
                };
                let kind = match tag_ {
                    tag::PATCH_METHOD => MetadataPatchKind::Method,
                    tag::PATCH_NARROW_KLASS => MetadataPatchKind::NarrowKlass,
                    _ => MetadataPatchKind::Klass,
                };
                self.pending_patches.push(PendingPatch::Metadata {
                    offset: pc_offset as usize,
                    kind,
                    in_constants: false,
                    key: key.unwrap_or_default(),
                });
            }
            tag::PATCH_DATA_SECTION_REFERENCE => {
                let data_offset = stream.read_u4("data:offset")? as usize;
                if data_offset >= self.constants_size as usize {
                    return Err(JvmciException::internal_error(format!(
                        "data offset {} points outside data section (size {})",
                        data_offset, self.constants_size
                    )));
                }
                self.pending_patches.push(PendingPatch::DataSectionRef {
                    code_offset: pc_offset as usize,
                    data_offset,
                });
            }
            other => {
                return Err(JvmciException::internal_error(format!(
                    "unknown data patch tag: {}",
                    other
                )))
            }
        }
        Ok(())
    }

    /// `site_Mark`.
    fn site_mark(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        pc_offset: i32,
    ) -> JvmciResult<()> {
        let id = stream.read_u1("mark:id")?;
        match id {
            mark::UNVERIFIED_ENTRY => self.offsets.entry = pc_offset,
            mark::VERIFIED_ENTRY => self.offsets.verified_entry = pc_offset,
            mark::OSR_ENTRY => self.offsets.osr_entry = pc_offset,
            mark::EXCEPTION_HANDLER_ENTRY => self.offsets.exceptions = pc_offset,
            mark::DEOPT_HANDLER_ENTRY => self.offsets.deopt = pc_offset,
            mark::DEOPT_MH_HANDLER_ENTRY => self.offsets.deopt_mh = pc_offset,
            mark::FRAME_COMPLETE => self.offsets.frame_complete = pc_offset,
            mark::ENTRY_BARRIER_PATCH => self.nmethod_entry_patch_offset = pc_offset,
            mark::INVOKEVIRTUAL | mark::INVOKEINTERFACE | mark::INLINE_INVOKE
            | mark::INVOKESTATIC | mark::INVOKESPECIAL => {
                self.next_call_type = id;
                self.invoke_mark_pc = pc_offset as i64;
            }
            mark::POLL_NEAR
            | mark::POLL_RETURN_NEAR
            | mark::POLL_FAR
            | mark::POLL_RETURN_FAR
            | mark::CARD_TABLE_ADDRESS
            | mark::CARD_TABLE_SHIFT
            | mark::HEAP_TOP_ADDRESS
            | mark::HEAP_END_ADDRESS
            | mark::NARROW_KLASS_BASE_ADDRESS
            | mark::NARROW_OOP_BASE_ADDRESS
            | mark::CRC_TABLE_ADDRESS
            | mark::LOG_OF_HEAP_REGION_GRAIN_BYTES
            | mark::INLINE_CONTIGUOUS_ALLOCATION_SUPPORTED
            | mark::VERIFY_OOPS
            | mark::VERIFY_OOP_BITS
            | mark::VERIFY_OOP_MASK
            | mark::VERIFY_OOP_COUNT_ADDRESS => {}
            other => {
                return Err(JvmciException::internal_error(format!(
                    "invalid mark id: {}",
                    other
                )))
            }
        }
        Ok(())
    }

    // ---- helpers ------------------------------------------------------------

    /// `record_oop`: dedup like `OopRecorder::find_index`; returns index.
    fn record_oop(&mut self, obj: JvmciObject) -> usize {
        let key = format!("{:?}", obj);
        if let Some(i) = self.oop_recorder.iter().position(|k| *k == key) {
            return i;
        }
        self.oop_recorder.push(key);
        self.oop_recorder.len() - 1
    }

    /// `record_oop_patch`.
    fn record_oop_patch(
        &mut self,
        stream: &mut HotSpotCompiledCodeStream,
        dest: usize,
        read_tag: u8,
        narrow: bool,
        in_constants: bool,
    ) -> JvmciResult<()> {
        let obj = self.read_oop(stream, read_tag)?;
        let idx = self.record_oop(obj);
        self.pending_patches.push(PendingPatch::Oop {
            offset: dest,
            in_constants,
            narrow,
            pool_index: idx,
        });
        Ok(())
    }

    /// `site_Call` target resolution: jmetadata handle -> method key.
    fn resolve_call_target(&mut self, target: i64) -> Option<String> {
        let rt = self.env.runtime().ok()?;
        let handle = target as u32 as i64;
        if handle > 0 {
            if let Some(MetadataTarget::Method { key }) = rt.resolve_metadata_handle(handle as u64)
            {
                return Some(key);
            }
        }
        None
    }

    // ---- top-level install ---------------------------------------------------

    /// `CodeInstaller::install`.
    pub fn install(
        &mut self,
        compiled_code_buffer: &[u8],
        with_type_info: bool,
        object_pool: Vec<JvmciObject>,
        compiled_code: JvmciObject,
        installed_code: JvmciObject,
        speculations: &[u8],
    ) -> JvmciResult<(CodeInstallResult, Option<CodeInstallRequest>)> {
        self.env.check_init()?;
        let mut stream =
            HotSpotCompiledCodeStream::new(compiled_code_buffer, with_type_info, object_pool)?;

        let code_flags = stream.read_u1("code:flags")?;
        let is_nmethod = is_set(code_flags, hcc::IS_NMETHOD);
        let name = stream.read_utf8("name")?;

        let mut method_key: Option<String> = None;
        let mut entry_bci = -1i32;
        let mut has_unsafe_access = false;
        let mut has_scoped_access = false;
        let mut compile_id = -1i32;

        if is_nmethod {
            method_key = stream.read_method(self.env, "method")?;
            entry_bci = stream.read_s4("entryBCI")?;
            let _compile_state = stream.read_u8("compileState")?;
            has_unsafe_access = stream.read_bool("hasUnsafeAccess")?;
            has_scoped_access = stream.read_bool("hasScopedAccess")?;
            compile_id = stream.read_s4("id")?;
        }
        stream.set_code_desc(name.as_deref(), method_key.as_deref());

        self.initialize_dependencies(&mut stream, code_flags)?;
        self.initialize_fields(&mut stream, code_flags)?;
        let result = self.initialize_buffer(&mut stream, compiled_code, code_flags)?;

        let available = stream.available();
        if result == CodeInstallResult::Ok && available != 0 {
            return Err(JvmciException::internal_error(format!(
                "{} bytes remaining in stream",
                available
            )));
        }
        if result != CodeInstallResult::Ok {
            return Ok((result, None));
        }

        self.apply_pending_patches()?;

        let scope_data = self
            .scope_writer
            .take()
            .map(|w| w.finish())
            .unwrap_or_default();

        let mut pc_descs = Vec::with_capacity(self.pc_descs.len() + 2);
        pc_descs.push(PcDesc::new(PcDesc::LOWER_OFFSET_LIMIT, 0, 0));
        pc_descs.extend(self.pc_descs.iter().cloned());
        pc_descs.push(PcDesc::new(PcDesc::UPPER_OFFSET_LIMIT, 0, 0));

        let metadata_size = pc_descs.len() * 32
            + self.decoded_scopes.len() * 128
            + scope_data.len()
            + self.exception_handler_table.size_in_bytes()
            + self
                .oop_map_set
                .maps()
                .iter()
                .map(|m| m.data_size() + 16)
                .sum::<usize>()
            + self.live_maps.iter().map(|m| m.encoded_size()).sum::<usize>();

        // HotSpotNmethod mirror fields (mirrorIndex / name).
        let mirror_index = jvmciObject::get_object_field(installed_code, "mirrorIndex")
            .and_then(|v| v.as_i32())
            .unwrap_or(-1);
        let mirror_name = jvmciObject::get_object_field(installed_code, "name")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .or_else(|| name.clone());

        let failed_speculations = decode_speculations(speculations);

        let request = CodeInstallRequest {
            method_key: method_key.clone().unwrap_or_default(),
            entry_bci,
            code_bytes: std::mem::take(&mut self.instructions),
            data_section: std::mem::take(&mut self.constants),
            total_frame_size: self.total_frame_size,
            stack_slots: self.total_frame_size / 8,
            pc_descs,
            scopes: std::mem::take(&mut self.decoded_scopes),
            scope_data,
            exception_table: self.exception_handler_table.clone(),
            pc_handlers: PcRangeHandlerTable::new(),
            oop_maps: self.oop_map_set.clone(),
            implicit_exceptions: self
                .implicit_exception_table
                .entries()
                .iter()
                .map(|&(p, d)| (p, d))
                .collect(),
            live_maps: std::mem::take(&mut self.live_maps),
            metadata_size,
            nmethod_mirror_index: mirror_index,
            nmethod_entry_patch_offset: self.nmethod_entry_patch_offset,
            nmethod_mirror_name: mirror_name,
            failed_speculations,
            dependencies: self.dependencies.clone(),
            offsets: self.offsets.clone(),
            has_unsafe_access,
            has_scoped_access,
            has_monitors: self.has_monitors,
            has_wide_vector: self.has_wide_vector,
            has_auto_box: self.has_auto_box,
            compile_id,
            is_stub: !is_nmethod,
            stub_name: if is_nmethod { None } else { name.clone() },
        };
        Ok((CodeInstallResult::Ok, Some(request)))
    }

    /// `apply_pending_patches` analogue: resolve recorded patches into
    /// concrete bytes where the target representation is known.
    fn apply_pending_patches(&mut self) -> JvmciResult<()> {
        let patches = std::mem::take(&mut self.pending_patches);
        for p in patches {
            match p {
                PendingPatch::Metadata {
                    offset,
                    kind,
                    in_constants,
                    key,
                } => {
                    let handle = self.metadata_handle_for(&key, kind)?;
                    let bytes = match kind {
                        MetadataPatchKind::NarrowKlass => {
                            (handle as u32).to_le_bytes().to_vec()
                        }
                        _ => (handle as u64).to_le_bytes().to_vec(),
                    };
                    self.write_patch_bytes(offset, in_constants, &bytes)?;
                }
                PendingPatch::Oop {
                    offset,
                    in_constants,
                    narrow,
                    pool_index,
                } => {
                    let key = self
                        .oop_recorder
                        .get(pool_index)
                        .cloned()
                        .unwrap_or_default();
                    // The recorded "oop" is a symbolic key; encode its
                    // stable hash as the patch payload (real oop
                    // relocation requires a live heap).
                    let value = stable_hash(&key);
                    let bytes = if narrow {
                        (value as u32).to_le_bytes().to_vec()
                    } else {
                        value.to_le_bytes().to_vec()
                    };
                    self.write_patch_bytes(offset, in_constants, &bytes)?;
                }
                PendingPatch::DataSectionRef {
                    code_offset,
                    data_offset,
                } => {
                    let rel = data_offset as i64
                        - (self.constants_size as i64 + code_offset as i64 + 4);
                    if code_offset + 4 <= self.instructions.len() {
                        self.instructions[code_offset..code_offset + 4]
                            .copy_from_slice(&(rel as i32).to_le_bytes());
                    }
                }
                PendingPatch::ForeignCall { code_offset, target } => {
                    // Relocate the call target: write the absolute
                    // target address into the instruction stream where a
                    // rel32/absolute call was emitted.
                    if code_offset + 8 <= self.instructions.len() {
                        self.instructions[code_offset..code_offset + 8]
                            .copy_from_slice(&target.to_le_bytes());
                    }
                }
            }
        }
        Ok(())
    }

    fn write_patch_bytes(
        &mut self,
        offset: usize,
        in_constants: bool,
        bytes: &[u8],
    ) -> JvmciResult<()> {
        let buf = if in_constants {
            &mut self.constants
        } else {
            &mut self.instructions
        };
        if offset + bytes.len() > buf.len() {
            return Err(JvmciException::internal_error(format!(
                "patch offset {} outside {} section (len {})",
                offset,
                if in_constants { "data" } else { "code" },
                buf.len()
            )));
        }
        buf[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    /// Resolve a metadata key to a jmetadata handle in the runtime's
    /// handle table.
    fn metadata_handle_for(
        &mut self,
        key: &str,
        kind: MetadataPatchKind,
    ) -> JvmciResult<u64> {
        if key.is_empty() {
            return Ok(0);
        }
        let rt = self.env.runtime()?;
        match kind {
            MetadataPatchKind::Method => rt
                .allocate_handle_method(key)
                .ok_or_else(|| {
                    JvmciException::internal_error("metadata handle allocation failed")
                }),
            _ => rt
                .allocate_handle_klass(key)
                .ok_or_else(|| {
                    JvmciException::internal_error("metadata handle allocation failed")
                }),
        }
    }
}

/// Read `dataSection`/`targetCode` byte arrays off the
/// HotSpotCompiledCode JVMCI object.
fn read_compiled_code_payloads(
    env: &mut JvmciEnv,
    compiled_code: JvmciObject,
) -> JvmciResult<(Vec<u8>, Vec<u8>)> {
    let _ = env;
    let mut data_bytes = Vec::new();
    let mut code_bytes = Vec::new();
    for (field, out) in [
        ("dataSection", &mut data_bytes),
        ("targetCode", &mut code_bytes),
    ] {
        if let Some(obj) = jvmciObject::get_object_field(compiled_code, field)
            .and_then(|v| v.as_object())
        {
            *out = read_byte_array(obj);
        }
    }
    Ok((data_bytes, code_bytes))
}

fn read_byte_array(obj: JvmciObject) -> Vec<u8> {
    match jvmciObject::resolve_object(obj) {
        Some(JvmciHeapValue::ByteArray(b)) => b,
        Some(JvmciHeapValue::Object { fields, .. }) => match fields.get("bytes") {
            Some(crate::share::jvmci::jvmciObject::JvmciValue::ByteArray(b)) => b.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Whether a register size denotes a vector wider than the runtime
/// baseline (`SharedRuntime::is_wide_vector` analogue).
fn is_wide_vector(max_register_size: u16) -> bool {
    max_register_size > 16
}

/// Best-effort call instruction size for `pd_next_offset`.
fn call_site_size(instructions: &[u8], pc_offset: usize) -> i32 {
    if pc_offset < instructions.len() {
        match instructions[pc_offset] {
            0xE8 | 0xE9 => return 5,
            0xFF => return 6,
            _ => {}
        }
    }
    5
}

/// `BasicType` wire value -> `ci::BasicType` (JavaKind ordinal order).
fn basic_type_from_wire(v: u8) -> JvmciResult<BasicType> {
    Ok(match v {
        0 => BasicType::Illegal,
        1 => BasicType::Boolean,
        2 => BasicType::Byte,
        3 => BasicType::Char,
        4 => BasicType::Short,
        5 => BasicType::Int,
        6 => BasicType::Float,
        7 => BasicType::Long,
        8 => BasicType::Double,
        9 => BasicType::Object,
        10 => BasicType::Array,
        11 => BasicType::Void,
        _ => {
            return Err(JvmciException::internal_error(format!(
                "unexpected basic type wire value {}",
                v
            )))
        }
    })
}

/// Decode the failed-speculations blob:
/// (u2 key_len, key bytes, s4 bci, i64 speculation) records.
fn decode_speculations(bytes: &[u8]) -> Vec<FailedSpeculation> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 2 <= bytes.len() {
        let klen = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;
        if pos + klen + 12 > bytes.len() {
            break;
        }
        let key = String::from_utf8_lossy(&bytes[pos..pos + klen]).into_owned();
        pos += klen;
        let bci = i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap());
        let spec = i64::from_le_bytes(bytes[pos + 4..pos + 12].try_into().unwrap());
        pos += 12;
        out.push(FailedSpeculation {
            method_key: key,
            bci,
            speculation: spec,
        });
    }
    out
}

/// Stable non-cryptographic hash for symbolic oop patch payloads.
fn stable_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
