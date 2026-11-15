/*
 * Port of OpenJDK 25 HotSpot share/asm/assembler.hpp (+ .inline.hpp)
 *
 * Platform-independent assembler base: owns the CodeBuffer, the label
 * table with patch lists, emit_int* helpers, relocate() and the
 * simm/uimm predicates.  Instruction encodings live in x86_64.rs.
 */

use crate::share::asm::code_buffer::{CodeBuffer, CodeBufferError, SectionId};
use crate::share::asm::reloc::{RelocError, RelocTable, RelocType, RelocWidth, RelocValue};

/// How a pending label patch is encoded, mirrors the platform
/// patch_instruction dispatch (rel8 / rel32 / absolute).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchKind {
    /// rel8 displacement (jcc short, jmp short, loop).
    Rel8,
    /// rel32 displacement (jmp/call/jcc near).
    Rel32,
    /// 32-bit absolute word (internal_word data reference).
    Abs32,
    /// 64-bit absolute word (movabs).
    Abs64,
}

impl PatchKind {
    pub fn width(self) -> usize {
        match self {
            PatchKind::Rel8 => 1,
            PatchKind::Rel32 | PatchKind::Abs32 => 4,
            PatchKind::Abs64 => 8,
        }
    }
}

/// One pending patch site: where the displacement field lives and how wide.
#[derive(Clone, Debug)]
pub struct PatchSite {
    /// Offset of the instruction start in its section.
    pub inst_offset: usize,
    /// Offset of the displacement/word field within the section.
    pub field_offset: usize,
    /// Which section the patch applies to.
    pub section: SectionId,
    pub kind: PatchKind,
    /// For Abs* patches: relocation to record once bound.
    pub reloc: Option<RelocSpec>,
}

/// Relocation spec attached to a patch (HotSpot RelocationHolder).
#[derive(Clone, Debug)]
pub struct RelocSpec {
    pub rtype: RelocType,
    pub width: RelocWidth,
    pub value: RelocValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelId(pub u32);

impl LabelId {
    pub const NONE: LabelId = LabelId(u32::MAX);
}

/// State of one label, mirrors assembler.hpp Label.
#[derive(Debug)]
pub struct LabelState {
    /// Bound code offset (section-relative); None while unbound.
    pub loc: Option<usize>,
    /// Section the label is bound in.
    pub section: SectionId,
    /// Pending patch sites referencing this label.
    pub patches: Vec<usize>, // indices into LabelTable::patches
    /// Whether label may be patched as "near" only.
    pub near_only: bool,
}

/// The label/patch table owned by an Assembler.
#[derive(Debug)]
pub struct LabelTable {
    labels: Vec<LabelState>,
    patches: Vec<PatchSite>,
    /// Reusable label slots (HotSpot reuses labels via init/reset).
    free: Vec<u32>,
}

impl Default for LabelTable {
    fn default() -> Self {
        Self::new()
    }
}

impl LabelTable {
    pub fn new() -> Self {
        Self { labels: Vec::new(), patches: Vec::new(), free: Vec::new() }
    }

    pub fn alloc(&mut self) -> LabelId {
        if let Some(id) = self.free.pop() {
            self.labels[id as usize] = LabelState {
                loc: None,
                section: SectionId::Insts,
                patches: Vec::new(),
                near_only: false,
            };
            return LabelId(id);
        }
        self.labels.push(LabelState {
            loc: None,
            section: SectionId::Insts,
            patches: Vec::new(),
            near_only: false,
        });
        LabelId((self.labels.len() - 1) as u32)
    }

    pub fn reset(&mut self, label: LabelId) {
        let st = &mut self.labels[label.0 as usize];
        st.loc = None;
        st.patches.clear();
        self.free.push(label.0);
    }

    pub fn loc(&self, label: LabelId) -> Option<usize> {
        self.labels.get(label.0 as usize).and_then(|l| l.loc)
    }

    pub fn is_bound(&self, label: LabelId) -> bool {
        self.loc(label).is_some()
    }

    pub fn section_of(&self, label: LabelId) -> SectionId {
        self.labels[label.0 as usize].section
    }

    /// Bind label to `offset` in `section`; resolves all pending patches.
    /// Returns the patch sites that had to be written (for diagnostics).
    pub fn bind(
        &mut self,
        code: &mut CodeBuffer,
        label: LabelId,
        offset: usize,
        section: SectionId,
    ) -> Result<Vec<usize>, AssemblerError> {
        {
            let st = &mut self.labels[label.0 as usize];
            if st.loc.is_some() {
                return Err(AssemblerError::LabelAlreadyBound(label));
            }
            st.loc = Some(offset);
            st.section = section;
        }
        // Drain the label's patch list; resolve each against the buffer.
        let patch_indices: Vec<usize> = self.labels[label.0 as usize].patches.clone();
        let mut resolved = Vec::with_capacity(patch_indices.len());
        for pi in patch_indices {
            let site = self.patches[pi];
            Self::apply_patch(code, &site, offset)?;
            // Record pending reloc for Abs* patches once bound.
            if let Some(spec) = &site.reloc {
                let target = offset as u64;
                let value = match &spec.value {
                    RelocValue::Internal { addend, .. } => {
                        RelocValue::Internal { target, addend: *addend }
                    }
                    RelocValue::None => RelocValue::Internal { target, addend: 0 },
                    other => other.clone(),
                };
                code.section(site.section).add_reloc(
                    site.inst_offset as u32,
                    (site.field_offset - site.inst_offset) as u32,
                    spec.rtype,
                    spec.width,
                    value,
                );
            }
            self.labels[label.0 as usize].patches.retain(|&x| x != pi);
            resolved.push(pi);
        }
        Ok(resolved)
    }

    /// Register a patch site for an unbound label (called by emitters).
    pub fn add_patch(&mut self, label: LabelId, site: PatchSite) -> usize {
        let pi = self.patches.len();
        self.patches.push(site);
        self.labels[label.0 as usize].patches.push(pi);
        pi
    }

    /// Immediate patch for a bound label (HotSpot pd_patch_instruction).
    /// `branch_section`/`field_offset` locate the displacement field;
    /// `target_section` is the section the label is bound in (normally
    /// the same section for intra-blob branches).
    pub fn patch_bound(
        code: &mut CodeBuffer,
        labels: &LabelTable,
        label: LabelId,
        branch_section: SectionId,
        field_offset: usize,
        kind: PatchKind,
    ) -> Result<(), AssemblerError> {
        let target = labels
            .loc(label)
            .ok_or(AssemblerError::UnboundLabel(label))?;
        let disp_end = field_offset + kind.width();
        let disp = target as i64 - disp_end as i64;
        match kind {
            PatchKind::Rel8 => {
                if !(-128..=127).contains(&disp) {
                    return Err(AssemblerError::BranchOutOfRange(disp));
                }
                code.section(branch_section)
                    .patch(field_offset, &[(disp as i8) as u8])
                    .map_err(AssemblerError::CodeBuffer)?;
            }
            PatchKind::Rel32 => {
                if !Assembler::is_simm(disp, 32) {
                    return Err(AssemblerError::BranchOutOfRange(disp));
                }
                code.section(branch_section)
                    .patch(field_offset, &(disp as i32).to_le_bytes())
                    .map_err(AssemblerError::CodeBuffer)?;
            }
            PatchKind::Abs32 => {
                code.section(branch_section)
                    .patch(field_offset, &(target as u32).to_le_bytes())
                    .map_err(AssemblerError::CodeBuffer)?;
            }
            PatchKind::Abs64 => {
                code.section(branch_section)
                    .patch(field_offset, &(target as u64).to_le_bytes())
                    .map_err(AssemblerError::CodeBuffer)?;
            }
        }
        Ok(())
    }

    /// Apply a pending patch site once the label is bound.
    fn apply_patch(code: &mut CodeBuffer, site: &PatchSite, target: usize) -> Result<(), AssemblerError> {
        let disp_end = site.field_offset + site.kind.width();
        let disp = target as i64 - disp_end as i64;
        match site.kind {
            PatchKind::Rel8 => {
                if !(-128..=127).contains(&disp) {
                    return Err(AssemblerError::BranchOutOfRange(disp));
                }
                code.section(site.section)
                    .patch(site.field_offset, &[(disp as i8) as u8])
                    .map_err(AssemblerError::CodeBuffer)?;
            }
            PatchKind::Rel32 => {
                if !Assembler::is_simm(disp, 32) {
                    return Err(AssemblerError::BranchOutOfRange(disp));
                }
                code.section(site.section)
                    .patch(site.field_offset, &(disp as i32).to_le_bytes())
                    .map_err(AssemblerError::CodeBuffer)?;
            }
            PatchKind::Abs32 => {
                code.section(site.section)
                    .patch(site.field_offset, &(target as u32).to_le_bytes())
                    .map_err(AssemblerError::CodeBuffer)?;
            }
            PatchKind::Abs64 => {
                code.section(site.section)
                    .patch(site.field_offset, &(target as u64).to_le_bytes())
                    .map_err(AssemblerError::CodeBuffer)?;
            }
        }
        Ok(())
    }
}

/// Error type for assembler operations.
#[derive(Debug)]
pub enum AssemblerError {
    CodeBuffer(CodeBufferError),
    Reloc(RelocError),
    UnboundLabel(LabelId),
    LabelAlreadyBound(LabelId),
    BranchOutOfRange(i64),
    InvalidOperand(String),
}

impl std::fmt::Display for AssemblerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssemblerError::CodeBuffer(e) => write!(f, "code buffer: {}", e),
            AssemblerError::Reloc(e) => write!(f, "reloc: {}", e),
            AssemblerError::UnboundLabel(l) => write!(f, "unbound label {:?}", l),
            AssemblerError::LabelAlreadyBound(l) => write!(f, "label {:?} already bound", l),
            AssemblerError::BranchOutOfRange(d) => write!(f, "branch displacement {} out of range", d),
            AssemblerError::InvalidOperand(s) => write!(f, "invalid operand: {}", s),
        }
    }
}
impl std::error::Error for AssemblerError {}
impl From<CodeBufferError> for AssemblerError {
    fn from(e: CodeBufferError) -> Self { AssemblerError::CodeBuffer(e) }
}
impl From<RelocError> for AssemblerError {
    fn from(e: RelocError) -> Self { AssemblerError::Reloc(e) }
}

/// Base assembler, mirrors share/asm/assembler.hpp class Assembler.
#[derive(Debug)]
pub struct Assembler {
    pub code: CodeBuffer,
    pub labels: LabelTable,
    /// Emit position cache: pc of the start of the current instruction
    /// (HotSpot uses inst_mark).
    inst_start: usize,
}

impl Assembler {
    pub fn new(code: CodeBuffer) -> Self {
        Self { code, labels: LabelTable::new(), inst_start: 0 }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(CodeBuffer::new(capacity))
    }

    // ---- position ------------------------------------------------------

    /// pc(): current emission address (as offset into insts section).
    #[inline]
    pub fn pc(&self) -> usize {
        self.code.pos()
    }

    /// offset(): same as pc but in insts section terms.
    #[inline]
    pub fn offset(&self) -> usize {
        self.pc()
    }

    /// begin(): start of insts section.
    #[inline]
    pub fn begin(&self) -> usize {
        0
    }

    /// inst_mark / set_inst_mark / clear_inst_mark.
    #[inline]
    pub fn inst_mark(&self) -> usize {
        self.inst_start
    }
    #[inline]
    pub fn set_inst_mark(&mut self) {
        self.inst_start = self.pc();
    }
    #[inline]
    pub fn clear_inst_mark(&mut self) {
        self.inst_start = self.pc();
    }

    /// code_fill_byte: byte used to pad code (x86 nop-ish fill = 0x90 is
    /// emitted by align(); generic fill is 0).
    pub fn code_fill_byte(&self) -> u8 {
        0
    }

    // ---- emit helpers (emit_int8..emit_int64) ---------------------------

    #[inline]
    pub fn emit_byte(&mut self, x: u8) {
        self.code.code_section().emit_u8(x);
    }

    #[inline]
    pub fn emit_bytes(&mut self, xs: &[u8]) {
        self.code.code_section().emit_bytes(xs);
    }

    #[inline]
    pub fn emit_word(&mut self, x: u16) {
        self.code.code_section().emit_u16(x);
    }

    #[inline]
    pub fn emit_dword(&mut self, x: u32) {
        self.code.code_section().emit_u32(x);
    }

    #[inline]
    pub fn emit_qword(&mut self, x: u64) {
        self.code.code_section().emit_u64(x);
    }

    #[inline]
    pub fn emit_f32(&mut self, x: f32) {
        self.code.code_section().emit_f32(x);
    }

    #[inline]
    pub fn emit_f64(&mut self, x: f64) {
        self.code.code_section().emit_f64(x);
    }

    #[inline]
    pub fn emit_i8(&mut self, x: i8) {
        self.emit_byte(x as u8);
    }

    #[inline]
    pub fn emit_i16(&mut self, x: i16) {
        self.emit_word(x as u16);
    }

    #[inline]
    pub fn emit_i32(&mut self, x: i32) {
        self.emit_dword(x as u32);
    }

    #[inline]
    pub fn emit_i64(&mut self, x: i64) {
        self.emit_qword(x as u64);
    }

    // ---- predicates ------------------------------------------------------

    /// is_simm(x, w): x fits in a signed w-bit immediate.
    pub fn is_simm(x: i64, w: u32) -> bool {
        let min = -(1i64 << (w - 1));
        let max = (1i64 << (w - 1)) - 1;
        x >= min && x <= max
    }

    pub fn is_simm8(x: i64) -> bool {
        Self::is_simm(x, 8)
    }

    pub fn is_simm16(x: i64) -> bool {
        Self::is_simm(x, 16)
    }

    pub fn is_simm32(x: i64) -> bool {
        Self::is_simm(x, 32)
    }

    /// is_uimm(x, w): x fits in an unsigned w-bit immediate.
    pub fn is_uimm(x: u64, w: u32) -> bool {
        x < (1u64 << w)
    }

    pub fn is_uimm8(x: u64) -> bool {
        Self::is_uimm(x, 8)
    }

    pub fn is_uimm16(x: u64) -> bool {
        Self::is_uimm(x, 16)
    }

    pub fn is_uimm32(x: u64) -> bool {
        Self::is_uimm(x, 32)
    }

    pub fn is_shift_count(x: i64) -> bool {
        (0..32).contains(&x)
    }

    pub fn is_byte(x: i64) -> bool {
        (0..256).contains(&x)
    }

    // ---- relocation ------------------------------------------------------

    /// relocate(rtype, format): record relocation at current pc.
    /// `field_offset` is the offset of the patched field relative to the
    /// instruction start (pc - inst_start gives the instruction offset).
    pub fn relocate(&mut self, rtype: RelocType, width: RelocWidth, value: RelocValue, field_offset: u32) {
        let pc = self.pc() as u32;
        let inst_off = self.inst_start as u32;
        self.code
            .section(self.code.current)
            .add_reloc(pc, field_offset, rtype, width, value);
        let _ = inst_off;
    }

    /// relocate at an explicit address offset.
    pub fn relocate_at(
        &mut self,
        at: usize,
        rtype: RelocType,
        width: RelocWidth,
        value: RelocValue,
        field_offset: u32,
    ) {
        let section = self.code.current;
        self.code
            .section(section)
            .add_reloc(at as u32, field_offset, rtype, width, value);
    }

    /// Simplified HotSpot-style: record reloc for the field that ends at
    /// current pc (used right after emitting a dword/qword).
    pub fn relocate_here(&mut self, rtype: RelocType, width: RelocWidth, value: RelocValue) {
        let field_end = self.pc() as u32;
        let field_off = field_end - width.size() as u32;
        let section = self.code.current;
        let inst_start = self.inst_start as u32;
        self.code.section(section).add_reloc(
            inst_start,
            field_off - inst_start,
            rtype,
            width,
            value,
        );
    }

    // ---- labels -----------------------------------------------------------

    pub fn new_label(&mut self) -> LabelId {
        self.labels.alloc()
    }

    /// bind(L): bind label to current code position.
    pub fn bind(&mut self, label: LabelId) -> Result<(), AssemblerError> {
        let section = self.code.current;
        let pos = self.pc();
        self.labels.bind(&mut self.code, label, pos, section)?;
        Ok(())
    }

    /// bind at explicit offset in current section.
    pub fn bind_at(&mut self, label: LabelId, offset: usize) -> Result<(), AssemblerError> {
        let section = self.code.current;
        self.labels.bind(&mut self.code, label, offset, section)
    }

    pub fn label_loc(&self, label: LabelId) -> Option<usize> {
        self.labels.loc(label)
    }

    pub fn is_bound(&self, label: LabelId) -> bool {
        self.labels.is_bound(label)
    }

    /// target(L): bound address or branch pc for unbound label.
    /// Returns None if unbound (caller must register a patch).
    pub fn target(&self, label: LabelId) -> Option<usize> {
        self.labels.loc(label)
    }

    /// Register a pending patch for `label` at (inst_offset, field_offset).
    pub fn add_label_patch(&mut self, label: LabelId, inst_offset: usize, field_offset: usize, kind: PatchKind) {
        let section = self.code.current;
        self.labels.add_patch(
            label,
            PatchSite { inst_offset, field_offset, section, kind, reloc: None },
        );
    }

    /// Register a pending patch carrying a relocation (Abs32/Abs64 with
    /// internal_word style reloc recorded on bind).
    pub fn add_label_patch_with_reloc(
        &mut self,
        label: LabelId,
        inst_offset: usize,
        field_offset: usize,
        kind: PatchKind,
        spec: RelocSpec,
    ) {
        let section = self.code.current;
        self.labels.add_patch(
            label,
            PatchSite { inst_offset, field_offset, section, kind, reloc: Some(spec) },
        );
    }

    /// patch_instruction for an already-bound label.  `inst_offset` is the
    /// instruction start; the displacement field offset is derived from it
    /// and the patch kind.
    pub fn patch_label(
        &mut self,
        label: LabelId,
        inst_offset: usize,
        field_offset: usize,
        kind: PatchKind,
    ) -> Result<(), AssemblerError> {
        let section = self.code.current;
        LabelTable::patch_bound(&mut self.code, &self.labels, label, section, field_offset, kind)
    }

    // ---- constants ---------------------------------------------------------

    pub fn int_constant(&mut self, c: i32) -> u64 {
        self.code.int_constant(c)
    }

    pub fn long_constant(&mut self, c: i64) -> u64 {
        self.code.long_constant(c)
    }

    pub fn double_constant(&mut self, c: f64) -> u64 {
        self.code.double_constant(c)
    }

    pub fn float_constant(&mut self, c: f32) -> u64 {
        self.code.float_constant(c)
    }

    /// start_a_stub / end_a_stub / start_a_const / end_a_const.
    pub fn start_a_stub(&mut self) {
        self.code.start_a_stub();
    }
    pub fn end_a_stub(&mut self) {
        self.code.end_a_stub();
    }
    pub fn start_a_const(&mut self, space: usize, align: usize) {
        self.code.start_a_const(space, align);
    }
    pub fn end_a_const(&mut self) {
        self.code.end_a_const();
    }

    // ---- finalize ------------------------------------------------------------

    /// flush(): prepare buffer for blob creation.
    pub fn flush(&mut self) {
        self.code.finalize();
    }

    /// Access the underlying CodeBuffer.
    pub fn code_buffer(&self) -> &CodeBuffer {
        &self.code
    }

    pub fn code_buffer_mut(&mut self) -> &mut CodeBuffer {
        &mut self.code
    }

    /// Take ownership of the CodeBuffer.
    pub fn into_code(self) -> CodeBuffer {
        self.code
    }
}
