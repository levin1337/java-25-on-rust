/*
 * Port of OpenJDK 25 HotSpot CodeBlob (share/code/codeBlob.hpp / codeBlob.cpp)
 *
 * CodeBlob is the base structure representing machine code and native executable
 * routines emitted into the CodeCache (JIT nmethods, runtime stubs, adapters).
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeBlobType {
    MethodNonProfiled = 0, // Level 1 and 4 compiled code (non-profiled)
    MethodProfiled = 1,    // Level 2 and 3 compiled code (profiled)
    NonNMethod = 2,        // Runtime stubs, adapters, and buffer blobs
    All = 3,               // Non-segmented code cache
}

impl CodeBlobType {
    #[inline]
    pub fn is_nmethod(&self) -> bool {
        matches!(self, Self::MethodNonProfiled | Self::MethodProfiled)
    }

    #[inline]
    pub fn is_profiled(&self) -> bool {
        matches!(self, Self::MethodProfiled)
    }

    #[inline]
    pub fn is_non_nmethod(&self) -> bool {
        matches!(self, Self::NonNMethod)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeBlobKind {
    None,
    Nmethod,
    BufferBlob,
    AdapterBlob,
    VtableBlob,
    MethodHandlesAdapterBlob,
    RuntimeStub,
    DeoptimizationBlob,
    SafepointBlob,
    ExceptionBlob,
    UncommonTrapBlob,
    UpcallStub,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlob {
    name: String,
    blob_type: CodeBlobType,
    blob_kind: CodeBlobKind,
    size: usize,
    header_size: usize,
    content_offset: usize,
    code_offset: usize,
    data_offset: usize,
    frame_size: usize,
    frame_complete_offset: i16,
    code_begin: usize,
    code_end: usize,
    content_begin: usize,
    content_end: usize,
    data_begin: usize,
    data_end: usize,
    code_bytes: Vec<u8>,
}

impl CodeBlob {
    pub const FRAME_NEVER_SAFE: i16 = -1;

    pub fn new(
        name: String,
        blob_type: CodeBlobType,
        blob_kind: CodeBlobKind,
        offset: usize,
        size: usize,
    ) -> Self {
        let code_begin = offset;
        let code_end = offset + size;
        Self {
            name,
            blob_type,
            blob_kind,
            size,
            header_size: 0,
            content_offset: 0,
            code_offset: 0,
            data_offset: size,
            frame_size: 0,
            frame_complete_offset: 0,
            code_begin,
            code_end,
            content_begin: code_begin,
            content_end: code_end,
            data_begin: code_end,
            data_end: code_end,
            code_bytes: Vec::new(),
        }
    }

    pub fn with_code(
        name: String,
        blob_type: CodeBlobType,
        blob_kind: CodeBlobKind,
        offset: usize,
        size: usize,
        code_bytes: Vec<u8>,
    ) -> Self {
        let mut blob = Self::new(name, blob_type, blob_kind, offset, size);
        blob.code_bytes = code_bytes;
        blob
    }

    pub fn with_layout(
        name: String,
        blob_type: CodeBlobType,
        blob_kind: CodeBlobKind,
        offset: usize,
        size: usize,
        header_size: usize,
        code_offset: usize,
        data_offset: usize,
        frame_size: usize,
        frame_complete_offset: i16,
    ) -> Self {
        let content_begin = offset;
        let content_end = offset + data_offset;
        let code_begin = offset + code_offset;
        let code_end = content_end;
        let data_begin = content_end;
        let data_end = offset + size;

        Self {
            name,
            blob_type,
            blob_kind,
            size,
            header_size,
            content_offset: 0,
            code_offset,
            data_offset,
            frame_size,
            frame_complete_offset,
            code_begin,
            code_end,
            content_begin,
            content_end,
            data_begin,
            data_end,
            code_bytes: Vec::new(),
        }
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn blob_type(&self) -> CodeBlobType {
        self.blob_type
    }

    #[inline]
    pub fn blob_kind(&self) -> CodeBlobKind {
        self.blob_kind
    }

    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    #[inline]
    pub fn header_size(&self) -> usize {
        self.header_size
    }

    #[inline]
    pub fn content_offset(&self) -> usize {
        self.content_offset
    }

    #[inline]
    pub fn code_offset(&self) -> usize {
        self.code_offset
    }

    #[inline]
    pub fn data_offset(&self) -> usize {
        self.data_offset
    }

    #[inline]
    pub fn frame_size(&self) -> usize {
        self.frame_size
    }

    #[inline]
    pub fn frame_complete_offset(&self) -> i16 {
        self.frame_complete_offset
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
    pub fn content_begin(&self) -> usize {
        self.content_begin
    }

    #[inline]
    pub fn content_end(&self) -> usize {
        self.content_end
    }

    #[inline]
    pub fn data_begin(&self) -> usize {
        self.data_begin
    }

    #[inline]
    pub fn data_end(&self) -> usize {
        self.data_end
    }

    #[inline]
    pub fn code_bytes(&self) -> &[u8] {
        &self.code_bytes
    }

    #[inline]
    pub fn set_code_bytes(&mut self, bytes: Vec<u8>) {
        self.code_bytes = bytes;
    }

    #[inline]
    pub fn is_nmethod(&self) -> bool {
        self.blob_kind == CodeBlobKind::Nmethod
    }

    #[inline]
    pub fn is_buffer_blob(&self) -> bool {
        self.blob_kind == CodeBlobKind::BufferBlob
    }

    #[inline]
    pub fn is_runtime_stub(&self) -> bool {
        self.blob_kind == CodeBlobKind::RuntimeStub
    }

    #[inline]
    pub fn is_deoptimization_blob(&self) -> bool {
        self.blob_kind == CodeBlobKind::DeoptimizationBlob
    }

    #[inline]
    pub fn is_safepoint_blob(&self) -> bool {
        self.blob_kind == CodeBlobKind::SafepointBlob
    }

    #[inline]
    pub fn is_adapter_blob(&self) -> bool {
        self.blob_kind == CodeBlobKind::AdapterBlob
    }

    #[inline]
    pub fn is_vtable_blob(&self) -> bool {
        self.blob_kind == CodeBlobKind::VtableBlob
    }

    #[inline]
    pub fn is_exception_blob(&self) -> bool {
        self.blob_kind == CodeBlobKind::ExceptionBlob
    }

    #[inline]
    pub fn is_uncommon_trap_blob(&self) -> bool {
        self.blob_kind == CodeBlobKind::UncommonTrapBlob
    }

    #[inline]
    pub fn is_upcall_stub(&self) -> bool {
        self.blob_kind == CodeBlobKind::UpcallStub
    }

    #[inline]
    pub fn blob_contains(&self, addr: usize) -> bool {
        addr >= self.content_begin && addr < self.data_end
    }

    #[inline]
    pub fn code_contains(&self, addr: usize) -> bool {
        addr >= self.code_begin && addr < self.code_end
    }

    #[inline]
    pub fn content_contains(&self, addr: usize) -> bool {
        addr >= self.content_begin && addr < self.content_end
    }

    #[inline]
    pub fn is_frame_complete_at(&self, addr: usize) -> bool {
        self.frame_complete_offset != Self::FRAME_NEVER_SAFE
            && self.code_contains(addr)
            && addr >= self.code_begin + (self.frame_complete_offset.max(0) as usize)
    }

    pub fn adjust_size(&mut self, used: usize) {
        self.size = used;
        self.code_end = self.code_begin + used;
        self.content_end = self.code_end;
        self.data_begin = self.code_end;
        self.data_end = self.code_end;
    }
}
