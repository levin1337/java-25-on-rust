/*
 * Port of OpenJDK 25 HotSpot ScopeDesc (share/code/scopeDesc.hpp / scopeDesc.cpp)
 *
 * ScopeDesc describes debug and deoptimization information for JIT-compiled
 * frames: method identity, bytecode indices (BCI), inlining call tree
 * (sender chain), locals, expressions and lock monitors.
 *
 * In HotSpot the data lives in the nmethod's "scopes data" blob and is read
 * through DebugInfoReadStream.  Here the same structure is serialized with
 * the UNSIGNED5 compressed stream (code::compressedStream) produced by
 * ScopeDescWriter and decoded by ScopeDescReader — a real, round-trippable
 * encoding rather than a placeholder.
 */

use crate::share::code::compressedStream::{CompressedReadStream, CompressedWriteStream};

pub const NO_SENDER_DECODE_OFFSET: i32 = -1;
/// DebugInformationRecorder::serialized_null == 0; byte offset 0 is
/// reserved so real records always start at >= 1.
pub const SERIALIZED_NULL: i32 = 0;

/// A decoded value in a scope: either a constant or a machine location.
#[derive(Clone, Debug, PartialEq)]
pub enum ScopeValue {
    /// Integer constant.
    ConstantInt(i32),
    /// Long constant (stored as two i32 halves like HotSpot longs).
    ConstantLong(i64),
    /// Float constant (raw bits).
    ConstantFloat(u32),
    /// Double constant (raw bits).
    ConstantDouble(u64),
    /// Oop constant, identified by its symbol/name.
    ConstantOop(String),
    /// Value held at a machine location.
    /// `location` is a VMReg-style encoding: >= 0 register index,
    /// < 0 encodes a stack slot as `-(slot + 2)` (slot 0 -> -2).
    Location { location: i32, offset_in_location: i32 },
}

impl ScopeValue {
    fn write_on(&self, stream: &mut CompressedWriteStream) {
        match self {
            Self::ConstantInt(v) => {
                stream.write(0);
                stream.write_signed_int(*v);
            }
            Self::ConstantLong(v) => {
                stream.write(1);
                stream.write_signed_int(*v as i32);
                stream.write_signed_int((*v >> 32) as i32);
            }
            Self::ConstantFloat(v) => {
                stream.write(2);
                stream.write_int(*v);
            }
            Self::ConstantDouble(v) => {
                stream.write(3);
                stream.write_int(*v as u32);
                stream.write_int((*v >> 32) as u32);
            }
            Self::ConstantOop(s) => {
                stream.write(4);
                let bytes = s.as_bytes();
                stream.write_int(bytes.len() as u32);
                for &b in bytes {
                    stream.write(b);
                }
            }
            Self::Location {
                location,
                offset_in_location,
            } => {
                stream.write(5);
                stream.write_signed_int(*location);
                stream.write_signed_int(*offset_in_location);
            }
        }
    }

    fn read_from(stream: &mut CompressedReadStream) -> Option<Self> {
        let tag = stream.read()?;
        match tag {
            0 => Some(Self::ConstantInt(stream.read_signed_int()?)),
            1 => {
                let lo = stream.read_signed_int()? as i64;
                let hi = stream.read_signed_int()? as i64;
                Some(Self::ConstantLong((hi << 32) | (lo as u64 & 0xFFFF_FFFF) as i64))
            }
            2 => Some(Self::ConstantFloat(stream.read_int()?)),
            3 => {
                let lo = stream.read_int()? as u64;
                let hi = stream.read_int()? as u64;
                Some(Self::ConstantDouble((hi << 32) | lo))
            }
            4 => {
                let len = stream.read_int()? as usize;
                let mut bytes = Vec::with_capacity(len);
                for _ in 0..len {
                    bytes.push(stream.read()?);
                }
                Some(Self::ConstantOop(String::from_utf8_lossy(&bytes).into_owned()))
            }
            5 => Some(Self::Location {
                location: stream.read_signed_int()?,
                offset_in_location: stream.read_signed_int()?,
            }),
            _ => None,
        }
    }
}

/// A locked monitor recorded in a scope.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorValue {
    pub owner: ScopeValue,
    pub location: ScopeValue,
    pub eliminated: bool,
}

impl MonitorValue {
    fn write_on(&self, stream: &mut CompressedWriteStream) {
        self.owner.write_on(stream);
        self.location.write_on(stream);
        stream.write_bool(self.eliminated);
    }

    fn read_from(stream: &mut CompressedReadStream) -> Option<Self> {
        Some(Self {
            owner: ScopeValue::read_from(stream)?,
            location: ScopeValue::read_from(stream)?,
            eliminated: stream.read_bool()?,
        })
    }
}

/// Decoded scope information for one (possibly inlined) frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeDesc {
    method_holder: String,
    method_name: String,
    method_signature: String,
    bci: i32,
    is_inlined: bool,
    sender_decode_offset: i32,
    decode_offset: i32,
    locals_decode_offset: i32,
    expressions_decode_offset: i32,
    monitors_decode_offset: i32,
    locals: Vec<ScopeValue>,
    expressions: Vec<ScopeValue>,
    monitors: Vec<MonitorValue>,
    reexecute: bool,
    return_oop: bool,
    rethrow_exception: bool,
}

impl ScopeDesc {
    pub fn new(
        method_holder: String,
        method_name: String,
        method_signature: String,
        bci: i32,
        is_inlined: bool,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            bci,
            is_inlined,
            sender_decode_offset: NO_SENDER_DECODE_OFFSET,
            decode_offset: SERIALIZED_NULL,
            locals_decode_offset: SERIALIZED_NULL,
            expressions_decode_offset: SERIALIZED_NULL,
            monitors_decode_offset: SERIALIZED_NULL,
            locals: Vec::new(),
            expressions: Vec::new(),
            monitors: Vec::new(),
            reexecute: false,
            return_oop: false,
            rethrow_exception: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_details(
        method_holder: String,
        method_name: String,
        method_signature: String,
        bci: i32,
        is_inlined: bool,
        sender_decode_offset: i32,
        locals: Vec<ScopeValue>,
        expressions: Vec<ScopeValue>,
        monitors: Vec<MonitorValue>,
        reexecute: bool,
        return_oop: bool,
        rethrow_exception: bool,
    ) -> Self {
        Self {
            method_holder,
            method_name,
            method_signature,
            bci,
            is_inlined,
            sender_decode_offset,
            decode_offset: SERIALIZED_NULL,
            locals_decode_offset: SERIALIZED_NULL,
            expressions_decode_offset: SERIALIZED_NULL,
            monitors_decode_offset: SERIALIZED_NULL,
            locals,
            expressions,
            monitors,
            reexecute,
            return_oop,
            rethrow_exception,
        }
    }

    #[inline]
    pub fn method_holder(&self) -> &str {
        &self.method_holder
    }

    #[inline]
    pub fn method_name(&self) -> &str {
        &self.method_name
    }

    #[inline]
    pub fn method_signature(&self) -> &str {
        &self.method_signature
    }

    #[inline]
    pub fn bci(&self) -> i32 {
        self.bci
    }

    #[inline]
    pub fn is_inlined(&self) -> bool {
        self.is_inlined
    }

    #[inline]
    pub fn sender_decode_offset(&self) -> i32 {
        self.sender_decode_offset
    }

    #[inline]
    pub fn decode_offset(&self) -> i32 {
        self.decode_offset
    }

    #[inline]
    pub fn is_top(&self) -> bool {
        self.sender_decode_offset == NO_SENDER_DECODE_OFFSET
    }

    #[inline]
    pub fn locals(&self) -> &[ScopeValue] {
        &self.locals
    }

    #[inline]
    pub fn expressions(&self) -> &[ScopeValue] {
        &self.expressions
    }

    #[inline]
    pub fn monitors(&self) -> &[MonitorValue] {
        &self.monitors
    }

    #[inline]
    pub fn should_reexecute(&self) -> bool {
        self.reexecute
    }

    #[inline]
    pub fn return_oop(&self) -> bool {
        self.return_oop
    }

    #[inline]
    pub fn rethrow_exception(&self) -> bool {
        self.rethrow_exception
    }
}

/// Writes scope records and scope-value lists into a compressed stream,
/// returning byte offsets usable as decode offsets (DebugInformationRecorder
/// analogue).
#[derive(Default)]
pub struct ScopeDescWriter {
    stream: CompressedWriteStream,
}

impl ScopeDescWriter {
    pub fn new() -> Self {
        // Reserve byte 0: decode offset 0 is serialized_null.
        let mut stream = CompressedWriteStream::with_capacity(256);
        stream.write(0);
        Self { stream }
    }

    pub fn position(&self) -> usize {
        self.stream.position()
    }

    /// Encode a list of scope values; returns the decode offset.
    pub fn write_scope_values(&mut self, values: &[ScopeValue]) -> i32 {
        let offset = self.stream.position() as i32;
        self.stream.write_int(values.len() as u32);
        for v in values {
            v.write_on(&mut self.stream);
        }
        offset
    }

    /// Encode a list of monitors; returns the decode offset.
    pub fn write_monitors(&mut self, monitors: &[MonitorValue]) -> i32 {
        let offset = self.stream.position() as i32;
        self.stream.write_int(monitors.len() as u32);
        for m in monitors {
            m.write_on(&mut self.stream);
        }
        offset
    }

    fn write_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.stream.write_int(bytes.len() as u32);
        for &b in bytes {
            self.stream.write(b);
        }
    }

    /// Encode one scope record; returns the decode offset of the record.
    /// Body offsets must already have been written (locals/expressions/
    /// monitors), matching the HotSpot layout where decode_body reads the
    /// header then lazily decodes bodies at their offsets.
    #[allow(clippy::too_many_arguments)]
    pub fn write_scope(
        &mut self,
        sender_decode_offset: i32,
        method_holder: &str,
        method_name: &str,
        method_signature: &str,
        bci: i32,
        locals_decode_offset: i32,
        expressions_decode_offset: i32,
        monitors_decode_offset: i32,
    ) -> i32 {
        let offset = self.stream.position() as i32;
        self.stream.write_signed_int(sender_decode_offset);
        self.write_string(method_holder);
        self.write_string(method_name);
        self.write_string(method_signature);
        self.stream.write_signed_int(bci);
        self.stream.write_signed_int(locals_decode_offset);
        self.stream.write_signed_int(expressions_decode_offset);
        self.stream.write_signed_int(monitors_decode_offset);
        offset
    }

    pub fn finish(self) -> Vec<u8> {
        self.stream.into_buffer()
    }
}

/// Reads scope records and scope-value lists from a compressed stream
/// (`DebugInfoReadStream` analogue).
pub struct ScopeDescReader<'a> {
    stream: CompressedReadStream<'a>,
}

impl<'a> ScopeDescReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            stream: CompressedReadStream::new(data),
        }
    }

    pub fn at(data: &'a [u8], decode_offset: i32) -> Self {
        let pos = if decode_offset < 0 {
            0
        } else {
            decode_offset as usize
        };
        Self {
            stream: CompressedReadStream::new_at(data, pos),
        }
    }

    fn read_string(&mut self) -> Option<String> {
        let len = self.stream.read_int()? as usize;
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            bytes.push(self.stream.read()?);
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// `ScopeDesc::decode_body` analogue: reads the scope header at
    /// `decode_offset` and returns a partially decoded ScopeDesc whose
    /// bodies must be filled via `decode_scope_values` /
    /// `decode_monitor_values`.
    pub fn decode_scope_header(&mut self) -> Option<ScopeDesc> {
        let decode_offset = self.stream.pos as i32;
        let sender = self.stream.read_signed_int()?;
        let holder = self.read_string()?;
        let name = self.read_string()?;
        let sig = self.read_string()?;
        let bci = self.stream.read_signed_int()?;
        let locals_off = self.stream.read_signed_int()?;
        let exprs_off = self.stream.read_signed_int()?;
        let mons_off = self.stream.read_signed_int()?;

        let mut desc = ScopeDesc::new(holder, name, sig, bci, sender != NO_SENDER_DECODE_OFFSET);
        desc.sender_decode_offset = sender;
        desc.decode_offset = decode_offset;
        desc.locals_decode_offset = locals_off;
        desc.expressions_decode_offset = exprs_off;
        desc.monitors_decode_offset = mons_off;
        Some(desc)
    }

    /// Decode a scope-value list at `decode_offset`
    /// (`ScopeDesc::decode_scope_values` analogue).
    pub fn decode_scope_values(&mut self, decode_offset: i32) -> Option<Vec<ScopeValue>> {
        if decode_offset == SERIALIZED_NULL {
            return None;
        }
        self.stream.pos = decode_offset as usize;
        let length = self.stream.read_int()? as usize;
        let mut result = Vec::with_capacity(length);
        for _ in 0..length {
            result.push(ScopeValue::read_from(&mut self.stream)?);
        }
        Some(result)
    }

    /// Decode object values at `decode_offset`
    /// (`ScopeDesc::decode_object_values` analogue).
    pub fn decode_object_values(&mut self, decode_offset: i32) -> Option<Vec<ScopeValue>> {
        self.decode_scope_values(decode_offset)
    }

    /// Decode a monitor list at `decode_offset`
    /// (`ScopeDesc::decode_monitor_values` analogue).
    pub fn decode_monitor_values(&mut self, decode_offset: i32) -> Option<Vec<MonitorValue>> {
        if decode_offset == SERIALIZED_NULL {
            return None;
        }
        self.stream.pos = decode_offset as usize;
        let length = self.stream.read_int()? as usize;
        let mut result = Vec::with_capacity(length);
        for _ in 0..length {
            result.push(MonitorValue::read_from(&mut self.stream)?);
        }
        Some(result)
    }

    /// Full decode: header + bodies, exactly like `ScopeDesc::decode_body`.
    pub fn decode_full(&mut self) -> Option<ScopeDesc> {
        let mut desc = self.decode_scope_header()?;
        let data_len_guard = self.stream.data.len();
        desc.locals = self.decode_scope_values(desc.locals_decode_offset).unwrap_or_default();
        desc.expressions = self
            .decode_scope_values(desc.expressions_decode_offset)
            .unwrap_or_default();
        desc.monitors = self
            .decode_monitor_values(desc.monitors_decode_offset)
            .unwrap_or_default();
        // Keep the stream bounded to the blob.
        if self.stream.pos > data_len_guard {
            self.stream.pos = data_len_guard;
        }
        Some(desc)
    }
}

/// Convenience: decode the complete inlined-scope chain for a pc, walking
/// sender decode offsets like `ScopeDesc::sender()`.
pub fn decode_scope_chain(data: &[u8], decode_offset: i32) -> Vec<ScopeDesc> {
    let mut chain = Vec::new();
    let mut offset = decode_offset;
    let mut depth = 0;
    // Real decode offsets are >= 1; 0 (serialized_null) and -1
    // (no sender) terminate the chain.
    while offset > 0 && depth < 256 {
        let mut reader = ScopeDescReader::at(data, offset);
        match reader.decode_full() {
            Some(desc) => {
                offset = desc.sender_decode_offset;
                chain.push(desc);
            }
            None => break,
        }
        depth += 1;
    }
    chain
}

/// Encode a whole `ScopeDesc` (with already-encoded body offsets unknown)
/// into a fresh blob: bodies are written first, then the header.
/// Returns `(blob, decode_offset_of_top_scope)`.
pub fn encode_scope_desc(
    desc: &ScopeDesc,
) -> (Vec<u8>, i32) {
    let mut w = ScopeDescWriter::new();
    let locals_off = if desc.locals.is_empty() {
        SERIALIZED_NULL
    } else {
        w.write_scope_values(&desc.locals)
    };
    let exprs_off = if desc.expressions.is_empty() {
        SERIALIZED_NULL
    } else {
        w.write_scope_values(&desc.expressions)
    };
    let mons_off = if desc.monitors.is_empty() {
        SERIALIZED_NULL
    } else {
        w.write_monitors(&desc.monitors)
    };
    let top = w.write_scope(
        desc.sender_decode_offset,
        &desc.method_holder,
        &desc.method_name,
        &desc.method_signature,
        desc.bci,
        locals_off,
        exprs_off,
        mons_off,
    );
    (w.finish(), top)
}
