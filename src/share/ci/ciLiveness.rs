/*
 * Port of OpenJDK 25 HotSpot MethodLiveness (compiler/methodLiveness.hpp/.cpp)
 * as used by ciMethod::liveness_at_bci / live_local_oops_at_bci.
 *
 * Two analyses run over the raw bytecode:
 *
 * 1. Backward local liveness: which local slots are live at each bci.
 *    Uses/defs are computed syntactically (loads read, stores write);
 *    exception-handler edges make every local live at a handler entry
 *    that is live anywhere inside the protected range.
 *
 * 2. Forward reference-type analysis: a may-analysis over the lattice
 *    {Num, Ref, Top} for locals and the operand stack, deciding which
 *    live locals may contain oops (`live_local_oops_at_bci`).
 *    `Top` is the conservative "unknown" and counts as a possible oop.
 */

use crate::share::classfile::classFileParser::Constant;
use crate::share::interpreter::bytecode::{BytecodeLookupswitch, BytecodeTableswitch};
use crate::share::oops::method::ExceptionTableEntry;

/// Slot lattice for the reference analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    /// Definitely not a reference.
    Num,
    /// Definitely a reference.
    Ref,
    /// Unknown / merged — treated as a possible oop.
    Top,
}

impl Ty {
    #[inline]
    fn join(self, other: Ty) -> Ty {
        if self == other { self } else { Ty::Top }
    }

    #[inline]
    fn is_maybe_oop(self) -> bool {
        !matches!(self, Ty::Num)
    }
}

/// One operand-stack word.  Category-2 values occupy two entries; the
/// second entry is flagged `second`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SE {
    ty: Ty,
    second: bool,
}

impl SE {
    const NUM: SE = SE {
        ty: Ty::Num,
        second: false,
    };
    const TOP: SE = SE {
        ty: Ty::Top,
        second: false,
    };
}

/// Abstract interpreter frame: local slot types + operand stack.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    locals: Vec<SE>,
    stack: Vec<SE>,
}

impl Frame {
    fn new(max_locals: usize) -> Self {
        Self {
            locals: vec![SE::TOP; max_locals],
            stack: Vec::new(),
        }
    }

    /// Pointwise join; `false` when nothing changed.
    fn merge(&mut self, other: &Frame) -> bool {
        let mut changed = false;
        for (a, b) in self.locals.iter_mut().zip(other.locals.iter()) {
            let joined = a.ty.join(b.ty);
            if a.ty != joined {
                *a = SE {
                    ty: joined,
                    second: false,
                };
                changed = true;
            }
        }
        let n = self.stack.len().max(other.stack.len());
        self.stack.resize(n, SE::TOP);
        for i in 0..n {
            let merged = match (self.stack.get(i), other.stack.get(i)) {
                (Some(a), Some(b)) => SE {
                    ty: a.ty.join(b.ty),
                    second: a.second || b.second,
                },
                (Some(a), None) => SE {
                    ty: a.ty.join(Ty::Top),
                    second: a.second,
                },
                (None, Some(b)) => SE {
                    ty: b.ty.join(Ty::Top),
                    second: b.second,
                },
                (None, None) => SE::TOP,
            };
            if self.stack[i] != merged {
                self.stack[i] = merged;
                changed = true;
            }
        }
        changed
    }
}

/// Bitset over local slots.
type Bits = Vec<u64>;

fn bits_new(n: usize) -> Bits {
    vec![0u64; (n + 63) / 64]
}

fn bits_set(bits: &mut Bits, i: usize) {
    if i / 64 < bits.len() {
        bits[i / 64] |= 1u64 << (i % 64);
    }
}

fn bits_get(bits: &Bits, i: usize) -> bool {
    i / 64 < bits.len() && (bits[i / 64] >> (i % 64)) & 1 == 1
}

fn bits_union_into(dst: &mut Bits, src: &Bits) -> bool {
    let mut changed = false;
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        let nv = *d | *s;
        if nv != *d {
            *d = nv;
            changed = true;
        }
    }
    changed
}

/// Result of `ciMethod::liveness_at_bci` /
/// `ciMethod::live_local_oops_at_bci`: per-bci liveness over local slots.
pub struct CiLiveness {
    instr_bcis: Vec<usize>,
    live_in: Vec<Bits>,
    oops_in: Vec<Bits>,
    max_locals: usize,
    valid: bool,
}

impl CiLiveness {
    /// Number of decoded instructions.
    pub fn instruction_count(&self) -> usize {
        self.instr_bcis.len()
    }

    /// bci of instruction `i`.
    pub fn bci_at(&self, i: usize) -> Option<usize> {
        self.instr_bcis.get(i).copied()
    }

    /// Index of the instruction starting exactly at `bci`.
    pub fn index_of_bci(&self, bci: usize) -> Option<usize> {
        self.instr_bcis.iter().position(|&b| b == bci)
    }

    /// Index of the instruction whose byte range contains `bci`
    /// (HotSpot requires an instruction boundary; interior bcis map to
    /// their containing instruction).
    pub fn index_at_bci(&self, bci: usize, code_len: usize) -> Option<usize> {
        if let Some(i) = self.index_of_bci(bci) {
            return Some(i);
        }
        let pos = self.instr_bcis.partition_point(|&b| b <= bci);
        if pos == 0 {
            return None;
        }
        let i = pos - 1;
        let start = self.instr_bcis[i];
        let end = if i + 1 < self.instr_bcis.len() {
            self.instr_bcis[i + 1]
        } else {
            code_len
        };
        if bci < end && start <= bci {
            Some(i)
        } else {
            None
        }
    }

    pub fn max_locals(&self) -> usize {
        self.max_locals
    }

    /// Whether the analysis completed.  When false the oop bitmap
    /// degrades to "every live local may be an oop".
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// `MethodLivenessResult::live`: is local slot `slot` live at the
    /// instruction containing `bci`?
    pub fn is_live(&self, bci: usize, code_len: usize, slot: usize) -> bool {
        let Some(i) = self.index_at_bci(bci, code_len) else {
            return false;
        };
        bits_get(&self.live_in[i], slot)
    }

    /// `MethodLivenessResult::live_oops` /
    /// `ciMethod::live_local_oops_at_bci`: may local slot `slot` hold
    /// a reference at `bci`?
    pub fn is_oop(&self, bci: usize, code_len: usize, slot: usize) -> bool {
        let Some(i) = self.index_at_bci(bci, code_len) else {
            return false;
        };
        bits_get(&self.oops_in[i], slot)
    }

    /// All live local slots at `bci`.
    pub fn live_slots(&self, bci: usize, code_len: usize) -> Vec<usize> {
        self.slots_at(bci, code_len, true)
    }

    /// Live local slots that may hold oops at `bci`.
    pub fn oop_slots(&self, bci: usize, code_len: usize) -> Vec<usize> {
        self.slots_at(bci, code_len, false)
    }

    fn slots_at(&self, bci: usize, code_len: usize, live: bool) -> Vec<usize> {
        let Some(i) = self.index_at_bci(bci, code_len) else {
            return Vec::new();
        };
        let bits = if live {
            &self.live_in[i]
        } else {
            &self.oops_in[i]
        };
        (0..self.max_locals)
            .filter(|&s| bits_get(bits, s))
            .collect()
    }

    /// `MethodLivenessResult` view at a specific bci.
    pub fn at(&self, bci: usize, code_len: usize) -> CiLivenessPoint<'_> {
        CiLivenessPoint {
            liveness: self,
            index: self.index_at_bci(bci, code_len),
        }
    }
}

/// `MethodLivenessResult`: liveness view at one bci.
pub struct CiLivenessPoint<'m> {
    liveness: &'m CiLiveness,
    index: Option<usize>,
}

impl<'m> CiLivenessPoint<'m> {
    /// Live local slots (any type).
    pub fn live_slots(&self) -> Vec<usize> {
        match self.index {
            Some(i) => (0..self.liveness.max_locals)
                .filter(|&s| bits_get(&self.liveness.live_in[i], s))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Live local slots that may contain oops.
    pub fn live_oops(&self) -> Vec<usize> {
        match self.index {
            Some(i) => (0..self.liveness.max_locals)
                .filter(|&s| bits_get(&self.liveness.oops_in[i], s))
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn is_live(&self, slot: usize) -> bool {
        self.index
            .is_some_and(|i| bits_get(&self.liveness.live_in[i], slot))
    }

    pub fn is_oop(&self, slot: usize) -> bool {
        self.index
            .is_some_and(|i| bits_get(&self.liveness.oops_in[i], slot))
    }
}

// ---------------------------------------------------------------------------
// Instruction decoding
// ---------------------------------------------------------------------------

struct Instr {
    bci: usize,
    len: usize,
    uses: Vec<usize>,
    defs: Vec<usize>,
    succ: Vec<usize>,
}

/// Instruction length in bytes; handles `wide`, `tableswitch` and
/// `lookupswitch` variable-length forms.
fn instr_len(code: &[u8], bci: usize) -> Option<usize> {
    let op = code[bci];
    if op == 0xaa {
        let ts = BytecodeTableswitch::new(code, bci)?;
        let low = ts.low_key()?;
        let high = ts.high_key()?;
        let n = if high >= low {
            (high - low + 1) as usize
        } else {
            0
        };
        let pad = (4 - ((bci + 1) % 4)) % 4;
        return Some(1 + pad + 12 + n * 4);
    }
    if op == 0xab {
        let ls = BytecodeLookupswitch::new(code, bci)?;
        let n = ls.number_of_pairs()?;
        let pad = (4 - ((bci + 1) % 4)) % 4;
        return Some(1 + pad + 8 + n * 8);
    }
    let l = bytes::length_at(code, bci);
    if l == 0 || bci + l > code.len() {
        return None;
    }
    Some(l)
}

mod bytes {
    pub use crate::share::interpreter::bytecodes::length_at;
}

fn read_idx(code: &[u8], pos: usize, size: usize) -> Option<usize> {
    match size {
        1 => code.get(pos).map(|&v| v as usize),
        _ => code
            .get(pos..pos + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize),
    }
}

/// Local slots read/written by the instruction at `bci`, mirroring
/// `MethodLiveness::compute_effects`.
fn collect_local_use_def(
    op: u8,
    code: &[u8],
    bci: usize,
    uses: &mut Vec<usize>,
    defs: &mut Vec<usize>,
    max_locals: usize,
) {
    // Effective opcode and index operand position/size (wide-aware).
    let (eff_op, idx_pos, idx_size) = if op == 0xc4 {
        (code.get(bci + 1).copied().unwrap_or(0), bci + 2, 2usize)
    } else {
        (op, bci + 1, 1usize)
    };

    // xload / xstore with u1/u2 index.
    match eff_op {
        0x15..=0x19 | 0x36..=0x3a | 0x84 | 0xa9 => {
            if let Some(i) = read_idx(code, idx_pos, idx_size) {
                if i >= max_locals {
                    return;
                }
                match eff_op {
                    // lload=0x16, dload=0x18 read two slots; lstore=0x37,
                    // dstore=0x39 write two slots.
                    0x15..=0x19 => {
                        uses.push(i);
                        if matches!(eff_op, 0x16 | 0x18) {
                            uses.push(i + 1);
                        }
                    }
                    0x36..=0x3a => {
                        defs.push(i);
                        if matches!(eff_op, 0x37 | 0x39) {
                            defs.push(i + 1);
                        }
                    }
                    0x84 => {
                        uses.push(i);
                        defs.push(i);
                    }
                    0xa9 => uses.push(i), // ret
                    _ => {}
                }
            }
            return;
        }
        _ => {}
    }

    if op == 0xc4 {
        return;
    }

    // _n forms.
    match op {
        0x1a..=0x2d => {
            // iload_n 0x1a-1d, lload_n 0x1e-21, fload_n 0x22-25,
            // dload_n 0x26-29, aload_n 0x2a-2d
            let group = (op - 0x1a) / 4;
            let slot = ((op - 0x1a) % 4) as usize;
            if slot < max_locals {
                uses.push(slot);
                if (group == 1 || group == 3) && slot + 1 < max_locals {
                    uses.push(slot + 1);
                }
            }
        }
        0x3b..=0x4e => {
            // istore_n 0x3b-3e, lstore_n 0x3f-42, fstore_n 0x43-46,
            // dstore_n 0x47-4a, astore_n 0x4b-4e
            let group = (op - 0x3b) / 4;
            let slot = ((op - 0x3b) % 4) as usize;
            if slot < max_locals {
                defs.push(slot);
                if (group == 1 || group == 3) && slot + 1 < max_locals {
                    defs.push(slot + 1);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// CFG successors
// ---------------------------------------------------------------------------

fn branch_target(code: &[u8], bci: usize, op: u8) -> Option<i64> {
    match op {
        0x99..=0xa8 | 0xc6 | 0xc7 => {
            let off = i16::from_be_bytes([*code.get(bci + 1)?, *code.get(bci + 2)?]) as i64;
            Some(bci as i64 + off)
        }
        0xc8 | 0xc9 => {
            let off = i32::from_be_bytes([
                *code.get(bci + 1)?,
                *code.get(bci + 2)?,
                *code.get(bci + 3)?,
                *code.get(bci + 4)?,
            ]) as i64;
            Some(bci as i64 + off)
        }
        _ => None,
    }
}

fn switch_targets(code: &[u8], bci: usize, op: u8) -> Vec<i64> {
    let mut out = Vec::new();
    if op == 0xaa {
        if let Some(ts) = BytecodeTableswitch::new(code, bci) {
            if let Some(d) = ts.default_offset() {
                out.push(bci as i64 + d as i64);
            }
            if let (Some(low), Some(high)) = (ts.low_key(), ts.high_key()) {
                if high >= low {
                    for i in 0..=(high - low) as usize {
                        if let Some(d) = ts.dest_offset_at(i) {
                            out.push(bci as i64 + d as i64);
                        }
                    }
                }
            }
        }
    } else if op == 0xab {
        if let Some(ls) = BytecodeLookupswitch::new(code, bci) {
            if let Some(d) = ls.default_offset() {
                out.push(bci as i64 + d as i64);
            }
            if let Some(n) = ls.number_of_pairs() {
                for i in 0..n {
                    if let Some(d) = ls.offset_at(i) {
                        out.push(bci as i64 + d as i64);
                    }
                }
            }
        }
    }
    out
}

/// Opcodes with no fall-through successor.
fn no_fallthrough(op: u8) -> bool {
    matches!(
        op,
        0xac..=0xb1 // ireturn..return
            | 0xbf // athrow
            | 0xa9 // ret
            | 0xa8 // jsr (subroutine call; resumes via ret)
            | 0xc9 // jsr_w
    )
}

/// Decode the instruction stream into `Instr` records with local
/// uses/defs; successors are filled in by the driver afterwards.
/// Returns `None` on malformed bytecode.
fn decode_instructions(code: &[u8], max_locals: usize) -> Option<Vec<Instr>> {
    let mut out = Vec::new();
    let mut bci = 0usize;
    while bci < code.len() {
        let len = instr_len(code, bci)?;
        let op = code[bci];
        let mut uses = Vec::new();
        let mut defs = Vec::new();
        collect_local_use_def(op, code, bci, &mut uses, &mut defs, max_locals);
        uses.sort_unstable();
        uses.dedup();
        defs.sort_unstable();
        defs.dedup();
        out.push(Instr {
            bci,
            len,
            uses,
            defs,
            succ: Vec::new(),
        });
        bci += len;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Transfer function for the reference-type analysis
// ---------------------------------------------------------------------------

struct Cp<'c> {
    constants: &'c [Constant<'c>],
}

impl<'c> Cp<'c> {
    fn utf8(&self, idx: u16) -> Option<&'c [u8]> {
        match self.constants.get(idx as usize)? {
            Constant::Utf8(b) => Some(b),
            _ => None,
        }
    }

    /// NameAndType index of a FieldRef/MethodRef/InterfaceMethodRef or
    /// InvokeDynamic/Dynamic constant.
    fn name_and_type_index(&self, cp_index: u16) -> Option<u16> {
        match self.constants.get(cp_index as usize)? {
            Constant::FieldRef(_, nt)
            | Constant::MethodRef(_, nt)
            | Constant::InterfaceMethodRef(_, nt)
            | Constant::InvokeDynamic(_, nt)
            | Constant::Dynamic(_, nt) => Some(*nt),
            _ => None,
        }
    }

    /// Descriptor (Utf8) of the NameAndType referenced by `nt_index`.
    fn descriptor_of_nt(&self, nt_index: u16) -> Option<&'c [u8]> {
        match self.constants.get(nt_index as usize)? {
            Constant::NameAndType(_, desc) => self.utf8(*desc),
            _ => None,
        }
    }

    /// Method/field descriptor of a cp index (invoke*, field*, ldc of
    /// MethodType/MethodHandle is handled separately).
    fn member_descriptor(&self, cp_index: u16) -> Option<&'c [u8]> {
        let nt = self.name_and_type_index(cp_index)?;
        self.descriptor_of_nt(nt)
    }

    /// Stack effect descriptor classification of an ldc constant:
    /// returns (is_reference, is_cat2).
    fn ldc_kind(&self, cp_index: u16) -> (bool, bool) {
        match self.constants.get(cp_index as usize) {
            Some(Constant::Long(_)) | Some(Constant::Double(_)) => (false, true),
            Some(Constant::Integer(_)) | Some(Constant::Float(_)) => (false, false),
            Some(
                Constant::String(_)
                | Constant::Class(_)
                | Constant::MethodHandle(..)
                | Constant::MethodType(_)
                | Constant::Dynamic(..)
                | Constant::InvokeDynamic(..)
                | Constant::Module(_)
                | Constant::Package(_),
            ) => (true, false),
            _ => (false, false),
        }
    }
}

/// Field-descriptor classification: (is_reference, is_cat2).
fn field_desc_kind(desc: &[u8]) -> (bool, bool) {
    match desc.first() {
        Some(b'J' | b'D') => (false, true),
        Some(b'L' | b'[') => (true, false),
        _ => (false, false),
    }
}

/// Parse a method descriptor `(params)return`; returns
/// (arg_cat1_count, arg_cat2_count, return_kind) where return_kind is
/// 0=void,1=cat1,2=cat2,3=reference.
fn method_sig_shape(sig: &[u8]) -> Option<(usize, usize, u8)> {
    if sig.first() != Some(&b'(') {
        return None;
    }
    let close = sig.iter().position(|&b| b == b')')?;
    let mut cat1 = 0usize;
    let mut cat2 = 0usize;
    let mut pos = 1usize;
    while pos < close {
        match sig[pos] {
            b'J' | b'D' => {
                cat2 += 1;
                pos += 1;
            }
            b'[' => {
                cat1 += 1;
                while pos < close && sig[pos] == b'[' {
                    pos += 1;
                }
                if pos < close && sig[pos] == b'L' {
                    pos += 1;
                    while pos < close && sig[pos] != b';' {
                        pos += 1;
                    }
                    pos += 1; // ';'
                } else if pos < close {
                    pos += 1; // primitive element
                }
            }
            b'L' => {
                cat1 += 1;
                pos += 1;
                while pos < close && sig[pos] != b';' {
                    pos += 1;
                }
                pos += 1;
            }
            _ => {
                cat1 += 1;
                pos += 1;
            }
        }
    }
    let ret = sig.get(close + 1).copied().unwrap_or(b'V');
    let ret_kind = match ret {
        b'V' => 0u8,
        b'J' | b'D' => 2u8,
        b'L' | b'[' => 3u8,
        _ => 1u8,
    };
    Some((cat1, cat2, ret_kind))
}

// ---------------------------------------------------------------------------
// Transfer function for the reference-type analysis
// ---------------------------------------------------------------------------

struct Xfer<'c> {
    cp: Cp<'c>,
}

impl<'c> Xfer<'c> {
    /// Pop one operand-stack word.
    fn pop_word(st: &mut Frame) -> Option<SE> {
        st.stack.pop()
    }

    /// Pop one value (cat1 or cat2); returns its type.
    fn pop_val(st: &mut Frame) -> Option<Ty> {
        let top = st.stack.pop()?;
        if top.second {
            let primary = st.stack.pop()?;
            Some(primary.ty.join(top.ty))
        } else {
            Some(top.ty)
        }
    }

    fn push(st: &mut Frame, ty: Ty) {
        st.stack.push(SE { ty, second: false });
    }

    fn push_cat2(st: &mut Frame) {
        st.stack.push(SE {
            ty: Ty::Num,
            second: false,
        });
        st.stack.push(SE {
            ty: Ty::Num,
            second: true,
        });
    }

    /// Pop a single word that must not be the second half of a cat2.
    fn pop_cat1_word(st: &mut Frame) -> Option<Ty> {
        let top = st.stack.pop()?;
        if top.second {
            st.stack.pop()?;
            return Some(Ty::Top);
        }
        Some(top.ty)
    }

    /// Pop a "dup unit": either a cat2 pair or two cat1 entries.
    fn pop_dup_unit(st: &mut Frame) -> Option<Vec<SE>> {
        let top = st.stack.pop()?;
        if top.second {
            let primary = st.stack.pop()?;
            Some(vec![primary, top])
        } else {
            let below = st.stack.pop()?;
            Some(vec![below, top])
        }
    }

    fn local_ty(st: &Frame, idx: usize) -> Ty {
        st.locals.get(idx).map_or(Ty::Top, |e| e.ty)
    }

    fn set_local(st: &mut Frame, idx: usize, ty: Ty, cat2: bool) {
        if idx >= st.locals.len() {
            return;
        }
        st.locals[idx] = SE { ty, second: false };
        if cat2 {
            if idx + 1 < st.locals.len() {
                st.locals[idx + 1] = SE {
                    ty: Ty::Num,
                    second: true,
                };
            }
        } else if idx + 1 < st.locals.len() && st.locals[idx + 1].second {
            st.locals[idx + 1] = SE::TOP;
        }
    }

    fn cp_u1(code: &[u8], bci: usize) -> Option<u16> {
        code.get(bci + 1).map(|&v| v as u16)
    }

    fn cp_u2(code: &[u8], bci: usize) -> Option<u16> {
        code.get(bci + 1..bci + 3)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    /// Apply the transfer for the instruction at `bci`.
    /// Returns false on malformed input.
    fn step(&self, st: &mut Frame, code: &[u8], bci: usize) -> bool {
        let op = code[bci];
        match op {
            0x00 => {}
            0x01 => Self::push(st, Ty::Ref), // aconst_null
            0x02..=0x08 => Self::push(st, Ty::Num),
            0x09 | 0x0a => Self::push_cat2(st),
            0x0b..=0x0d => Self::push(st, Ty::Num),
            0x0e | 0x0f => Self::push_cat2(st),
            0x10 | 0x11 => Self::push(st, Ty::Num),
            0x12 | 0x13 | 0x14 => {
                let idx = if op == 0x12 {
                    match Self::cp_u1(code, bci) {
                        Some(v) => v,
                        None => return false,
                    }
                } else {
                    match Self::cp_u2(code, bci) {
                        Some(v) => v,
                        None => return false,
                    }
                };
                let (is_ref, cat2) = self.cp.ldc_kind(idx);
                if is_ref {
                    Self::push(st, Ty::Ref);
                } else if cat2 {
                    Self::push_cat2(st);
                } else {
                    Self::push(st, Ty::Num);
                }
            }
            // xload with u1 index (wide handled in the 0xc4 arm).
            0x15..=0x19 => {
                let idx = match read_idx(code, bci + 1, 1) {
                    Some(i) => i,
                    None => return false,
                };
                let cat2 = matches!(op, 0x16 | 0x18); // lload, dload
                let ty = Self::local_ty(st, idx);
                if cat2 {
                    Self::push(st, ty);
                    st.stack.push(SE {
                        ty: Ty::Num,
                        second: true,
                    });
                } else {
                    Self::push(st, ty);
                }
            }
            // xload_n 0x1a..=0x2d
            0x1a..=0x2d => {
                let group = (op - 0x1a) / 4;
                let slot = ((op - 0x1a) % 4) as usize;
                let cat2 = group == 1 || group == 3; // lload_n, dload_n
                let ty = Self::local_ty(st, slot);
                if cat2 {
                    Self::push(st, ty);
                    st.stack.push(SE {
                        ty: Ty::Num,
                        second: true,
                    });
                } else {
                    Self::push(st, ty);
                }
            }
            // array loads 0x2e..=0x35: pop index + arrayref
            0x2e..=0x35 => {
                if Self::pop_val(st).is_none() || Self::pop_val(st).is_none() {
                    return false;
                }
                match op {
                    0x2f | 0x31 => Self::push_cat2(st), // laload, daload
                    0x32 => Self::push(st, Ty::Ref),    // aaload
                    _ => Self::push(st, Ty::Num),
                }
            }
            // xstore with u1 index
            0x36..=0x3a => {
                let idx = match read_idx(code, bci + 1, 1) {
                    Some(i) => i,
                    None => return false,
                };
                let cat2 = matches!(op, 0x37 | 0x39); // lstore, dstore
                let ty = Self::pop_val(st).unwrap_or(Ty::Top);
                Self::set_local(st, idx, ty, cat2);
            }
            // xstore_n 0x3b..=0x4e
            0x3b..=0x4e => {
                let group = (op - 0x3b) / 4;
                let slot = ((op - 0x3b) % 4) as usize;
                let cat2 = group == 1 || group == 3; // lstore_n, dstore_n
                let ty = Self::pop_val(st).unwrap_or(Ty::Top);
                Self::set_local(st, slot, ty, cat2);
            }
            // array stores 0x4f..=0x56: pop value, index, arrayref
            0x4f..=0x56 => {
                if Self::pop_val(st).is_none()
                    || Self::pop_val(st).is_none()
                    || Self::pop_val(st).is_none()
                {
                    return false;
                }
            }
            // stack manipulation
            0x57 => {
                if Self::pop_cat1_word(st).is_none() {
                    return false;
                }
            }
            0x58 => {
                // pop2
                let top = match Self::pop_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                if top.second {
                    if Self::pop_word(st).is_none() {
                        return false;
                    }
                } else if Self::pop_cat1_word(st).is_none() {
                    return false;
                }
            }
            0x59 => {
                // dup
                let t = match Self::pop_cat1_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                Self::push(st, t);
                Self::push(st, t);
            }
            0x5a => {
                // dup_x1
                let v1 = match Self::pop_cat1_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                let v2 = match Self::pop_cat1_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                Self::push(st, v1);
                Self::push(st, v2);
                Self::push(st, v1);
            }
            0x5b => {
                // dup_x2
                let v1 = match Self::pop_cat1_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                let e2 = match Self::pop_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                if e2.second {
                    if Self::pop_word(st).is_none() {
                        return false;
                    }
                    Self::push(st, v1);
                    st.stack.push(e2);
                    Self::push(st, v1);
                } else {
                    let v3 = match Self::pop_cat1_word(st) {
                        Some(t) => t,
                        None => return false,
                    };
                    Self::push(st, v1);
                    Self::push(st, v3);
                    Self::push(st, e2.ty);
                    Self::push(st, v1);
                }
            }
            0x5c => {
                // dup2
                let unit = match Self::pop_dup_unit(st) {
                    Some(u) => u,
                    None => return false,
                };
                for e in &unit {
                    st.stack.push(*e);
                }
                for e in &unit {
                    st.stack.push(*e);
                }
            }
            0x5d => {
                // dup2_x1
                let unit = match Self::pop_dup_unit(st) {
                    Some(u) => u,
                    None => return false,
                };
                let v3 = match Self::pop_cat1_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                for e in &unit {
                    st.stack.push(*e);
                }
                Self::push(st, v3);
                for e in &unit {
                    st.stack.push(*e);
                }
            }
            0x5e => {
                // dup2_x2
                let unit1 = match Self::pop_dup_unit(st) {
                    Some(u) => u,
                    None => return false,
                };
                let unit2 = match Self::pop_dup_unit(st) {
                    Some(u) => u,
                    None => return false,
                };
                for e in &unit1 {
                    st.stack.push(*e);
                }
                for e in &unit2 {
                    st.stack.push(*e);
                }
                for e in &unit1 {
                    st.stack.push(*e);
                }
            }
            0x5f => {
                // swap
                let v1 = match Self::pop_cat1_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                let v2 = match Self::pop_cat1_word(st) {
                    Some(t) => t,
                    None => return false,
                };
                Self::push(st, v1);
                Self::push(st, v2);
            }
            // arithmetic / conversion / comparison
            0x60..=0x83 | 0x85..=0x98 => {
                if !self.step_arith(st, op) {
                    return false;
                }
            }
            0x84 => {
                // iinc — no stack effect
            }
            // branches (pops only; CFG handled by the driver)
            0x99..=0xa6 | 0xc6 | 0xc7 => {
                let n = if matches!(op, 0x9f..=0xa6) { 2 } else { 1 };
                for _ in 0..n {
                    if Self::pop_val(st).is_none() {
                        return false;
                    }
                }
            }
            0xa7 | 0xc8 => {} // goto / goto_w
            0xa8 | 0xc9 => {
                // jsr / jsr_w: push return address (non-reference)
                Self::push(st, Ty::Num);
            }
            0xa9 => {
                // ret: discard return address
                if Self::pop_val(st).is_none() {
                    return false;
                }
            }
            0xaa | 0xab => {
                if Self::pop_val(st).is_none() {
                    return false;
                }
            }
            0xac..=0xb0 => {
                // ireturn..areturn: pop one value
                if Self::pop_val(st).is_none() {
                    return false;
                }
            }
            0xb1 => {} // return
            // field access 0xb2..=0xb5
            0xb2..=0xb5 => {
                let idx = match Self::cp_u2(code, bci) {
                    Some(v) => v,
                    None => return false,
                };
                let desc = match self.cp.member_descriptor(idx) {
                    Some(d) => d,
                    None => return false,
                };
                let (is_ref, cat2) = field_desc_kind(desc);
                match op {
                    0xb2 => {
                        // getstatic
                        if is_ref {
                            Self::push(st, Ty::Ref);
                        } else if cat2 {
                            Self::push_cat2(st);
                        } else {
                            Self::push(st, Ty::Num);
                        }
                    }
                    0xb3 => {
                        // putstatic
                        if Self::pop_val(st).is_none() {
                            return false;
                        }
                    }
                    0xb4 => {
                        // getfield
                        if Self::pop_val(st).is_none() {
                            return false;
                        }
                        if is_ref {
                            Self::push(st, Ty::Ref);
                        } else if cat2 {
                            Self::push_cat2(st);
                        } else {
                            Self::push(st, Ty::Num);
                        }
                    }
                    0xb5 => {
                        // putfield
                        if Self::pop_val(st).is_none() || Self::pop_val(st).is_none() {
                            return false;
                        }
                    }
                    _ => unreachable!(),
                }
            }
            // invokes 0xb6..=0xba
            0xb6..=0xba => {
                let idx = match Self::cp_u2(code, bci) {
                    Some(v) => v,
                    None => return false,
                };
                let desc = match self.cp.member_descriptor(idx) {
                    Some(d) => d,
                    None => return false,
                };
                let Some((cat1, cat2, ret_kind)) = method_sig_shape(desc) else {
                    return false;
                };
                for _ in 0..cat1 + cat2 {
                    if Self::pop_val(st).is_none() {
                        return false;
                    }
                }
                if !matches!(op, 0xb8 | 0xba) {
                    // receiver for non-static invokes
                    if Self::pop_val(st).is_none() {
                        return false;
                    }
                }
                match ret_kind {
                    0 => {}
                    1 => Self::push(st, Ty::Num),
                    2 => Self::push_cat2(st),
                    _ => Self::push(st, Ty::Ref),
                }
            }
            0xbb => Self::push(st, Ty::Ref), // new
            0xbc | 0xbd => {
                // newarray / anewarray
                if Self::pop_val(st).is_none() {
                    return false;
                }
                Self::push(st, Ty::Ref);
            }
            0xbe => {
                // arraylength
                if Self::pop_val(st).is_none() {
                    return false;
                }
                Self::push(st, Ty::Num);
            }
            0xbf => {
                // athrow
                if Self::pop_val(st).is_none() {
                    return false;
                }
            }
            0xc0 => {
                // checkcast
                if Self::pop_val(st).is_none() {
                    return false;
                }
                Self::push(st, Ty::Ref);
            }
            0xc1 => {
                // instanceof
                if Self::pop_val(st).is_none() {
                    return false;
                }
                Self::push(st, Ty::Num);
            }
            0xc2 | 0xc3 => {
                // monitorenter / monitorexit
                if Self::pop_val(st).is_none() {
                    return false;
                }
            }
            0xc4 => {
                // wide prefix
                let eff = code.get(bci + 1).copied().unwrap_or(0);
                match eff {
                    0x15..=0x19 => {
                        let idx = match read_idx(code, bci + 2, 2) {
                            Some(i) => i,
                            None => return false,
                        };
                        let cat2 = matches!(eff, 0x16 | 0x18);
                        let ty = Self::local_ty(st, idx);
                        if cat2 {
                            Self::push(st, ty);
                            st.stack.push(SE {
                                ty: Ty::Num,
                                second: true,
                            });
                        } else {
                            Self::push(st, ty);
                        }
                    }
                    0x36..=0x3a => {
                        let idx = match read_idx(code, bci + 2, 2) {
                            Some(i) => i,
                            None => return false,
                        };
                        let cat2 = matches!(eff, 0x37 | 0x39);
                        let ty = Self::pop_val(st).unwrap_or(Ty::Top);
                        Self::set_local(st, idx, ty, cat2);
                    }
                    0x84 => {
                        // wide iinc — no stack effect
                    }
                    0xa9 => {
                        // wide ret — discard return address
                        if Self::pop_val(st).is_none() {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
            0xc5 => {
                // multianewarray: dims u1 at bci+3
                let dims = code.get(bci + 3).copied().unwrap_or(0) as usize;
                for _ in 0..dims {
                    if Self::pop_val(st).is_none() {
                        return false;
                    }
                }
                Self::push(st, Ty::Ref);
            }
            _ => {
                // Undefined/reserved opcodes: no modeled effect.
            }
        }
        true
    }

    /// Arithmetic, conversion and comparison opcodes.
    fn step_arith(&self, st: &mut Frame, op: u8) -> bool {
        let cat2_result = matches!(
            op,
            0x61 | 0x63
                | 0x65
                | 0x67
                | 0x69
                | 0x6b
                | 0x6d
                | 0x6f
                | 0x71
                | 0x73
                | 0x75
                | 0x77
                | 0x79
                | 0x7b
                | 0x7d
                | 0x7f
                | 0x81
        );
        let is_binary = matches!(op, 0x60..=0x73 | 0x78..=0x83);
        let is_unary = matches!(op, 0x74..=0x77);
        if is_binary {
            if Self::pop_val(st).is_none() || Self::pop_val(st).is_none() {
                return false;
            }
            if cat2_result {
                Self::push_cat2(st);
            } else {
                Self::push(st, Ty::Num);
            }
        } else if is_unary {
            if Self::pop_val(st).is_none() {
                return false;
            }
            if cat2_result {
                Self::push_cat2(st);
            } else {
                Self::push(st, Ty::Num);
            }
        } else {
            // conversions 0x85..=0x93, comparisons 0x94..=0x98
            match op {
                0x85 | 0x87 | 0x8a | 0x8c | 0x8d | 0x8f => {
                    // i2l, i2d, l2d, f2l, f2d, d2l -> cat2
                    if Self::pop_val(st).is_none() {
                        return false;
                    }
                    Self::push_cat2(st);
                }
                0x86 | 0x88 | 0x89 | 0x8b | 0x8e | 0x90..=0x93 => {
                    if Self::pop_val(st).is_none() {
                        return false;
                    }
                    Self::push(st, Ty::Num);
                }
                0x94..=0x98 => {
                    // lcmp / fcmp* / dcmp*: pop two values, push int
                    if Self::pop_val(st).is_none() || Self::pop_val(st).is_none() {
                        return false;
                    }
                    Self::push(st, Ty::Num);
                }
                _ => {
                    if Self::pop_val(st).is_none() {
                        return false;
                    }
                    Self::push(st, Ty::Num);
                }
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Compute liveness for a method's bytecode.
///
/// * `code` — raw bytecode bytes (`Code::bytes`).
/// * `max_locals` — from the Code attribute.
/// * `is_static` — ACC_STATIC of the method.
/// * `signature` — method descriptor `(params)return` used to seed
///   parameter types (reference params count as potential oops).
/// * `exception_table` — parsed exception table.
/// * `constants` — the holder's constant pool (for ldc/invoke/field
///   type resolution).
pub fn compute_liveness<'c>(
    code: &[u8],
    max_locals: usize,
    is_static: bool,
    signature: &[u8],
    exception_table: &[ExceptionTableEntry],
    constants: &'c [Constant<'c>],
) -> CiLiveness {
    let xfer = Xfer {
        cp: Cp { constants },
    };

    let Some(mut instrs) = decode_instructions(code, max_locals) else {
        return CiLiveness {
            instr_bcis: Vec::new(),
            live_in: Vec::new(),
            oops_in: Vec::new(),
            max_locals,
            valid: false,
        };
    };
    let n = instrs.len();
    if n == 0 {
        return CiLiveness {
            instr_bcis: Vec::new(),
            live_in: Vec::new(),
            oops_in: Vec::new(),
            max_locals,
            valid: true,
        };
    }

    let bcis: Vec<usize> = instrs.iter().map(|i| i.bci).collect();
    let index_of = |target: i64| -> Option<usize> {
        if target < 0 {
            return None;
        }
        let t = target as usize;
        bcis.binary_search(&t).ok()
    };

    // Successors.
    let mut jsr_returns: Vec<usize> = Vec::new();
    for i in 0..n {
        let instr_bci = instrs[i].bci;
        let op = code[instr_bci];
        let mut succ = Vec::new();
        if let Some(t) = branch_target(code, instr_bci, op) {
            if let Some(j) = index_of(t) {
                succ.push(j);
            }
        }
        if op == 0xaa || op == 0xab {
            for t in switch_targets(code, instr_bci, op) {
                if let Some(j) = index_of(t) {
                    succ.push(j);
                }
            }
        }
        if matches!(op, 0xa8 | 0xc9) {
            // jsr/jsr_w resume at the following instruction via ret.
            if let Some(j) = index_of(instr_bci as i64 + instrs[i].len as i64) {
                jsr_returns.push(j);
            }
        }
        if !no_fallthrough(op) && i + 1 < n {
            succ.push(i + 1);
        }
        for e in exception_table {
            if instr_bci >= e.start_pc && instr_bci < e.end_pc {
                if let Some(j) = index_of(e.handler_pc as i64) {
                    succ.push(j);
                }
            }
        }
        succ.sort_unstable();
        succ.dedup();
        instrs[i].succ = succ;
    }
    for i in 0..n {
        if code[instrs[i].bci] == 0xa9 {
            instrs[i].succ = jsr_returns.clone();
        }
    }

    // Predecessor lists for the forward pass.
    let mut preds: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
    for i in 0..n {
        for &s in &instrs[i].succ {
            preds[s].push(i);
        }
    }

    // ---- backward liveness -------------------------------------------------
    let words = (max_locals + 63) / 64;
    let mut use_bits: Vec<Bits> = (0..n).map(|_| bits_new(max_locals)).collect();
    let mut def_bits: Vec<Bits> = (0..n).map(|_| bits_new(max_locals)).collect();
    for i in 0..n {
        for &u in &instrs[i].uses {
            bits_set(&mut use_bits[i], u);
        }
        for &d in &instrs[i].defs {
            bits_set(&mut def_bits[i], d);
        }
    }

    let mut live_in: Vec<Bits> = (0..n).map(|_| bits_new(max_locals)).collect();
    let mut handler_uses: Vec<Bits> = (0..n).map(|_| bits_new(max_locals)).collect();

    for _round in 0..16 {
        // Backward fixpoint.
        loop {
            let mut changed = false;
            for i in (0..n).rev() {
                let mut new_out = bits_new(max_locals);
                for &s in &instrs[i].succ {
                    bits_union_into(&mut new_out, &live_in[s]);
                }
                let mut new_in = new_out;
                for w in 0..words {
                    new_in[w] |= use_bits[i][w] | handler_uses[i][w];
                    new_in[w] &= !def_bits[i][w];
                }
                if new_in != live_in[i] {
                    live_in[i] = new_in;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // Refine handler uses: union of live_in over each try range.
        let mut grew = false;
        for e in exception_table {
            if let Some(h) = index_of(e.handler_pc as i64) {
                let mut u = bits_new(max_locals);
                for i in 0..n {
                    if instrs[i].bci >= e.start_pc && instrs[i].bci < e.end_pc {
                        bits_union_into(&mut u, &live_in[i]);
                    }
                }
                for w in 0..words {
                    let nv = handler_uses[h][w] | u[w];
                    if nv != handler_uses[h][w] {
                        handler_uses[h][w] = nv;
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }

    // ---- forward reference-type analysis ----------------------------------
    let mut oops_in: Vec<Bits> = (0..n).map(|_| bits_new(max_locals)).collect();
    let mut valid = true;

    // Seed parameter types.
    let mut seed = Frame::new(max_locals);
    let mut slot = 0usize;
    if !is_static && slot < max_locals {
        seed.locals[slot] = SE {
            ty: Ty::Ref,
            second: false,
        };
        slot += 1;
    }
    if let Some(close) = signature.iter().position(|&b| b == b')') {
        let mut pos = 1usize;
        while pos < close {
            match signature[pos] {
                b'[' => {
                    while pos < close && signature[pos] == b'[' {
                        pos += 1;
                    }
                    let is_ref = pos < close && signature[pos] == b'L';
                    if is_ref {
                        while pos < close && signature[pos] != b';' {
                            pos += 1;
                        }
                    }
                    pos += 1;
                    if slot < max_locals {
                        seed.locals[slot] = SE {
                            ty: if is_ref { Ty::Ref } else { Ty::Num },
                            second: false,
                        };
                        slot += 1;
                    }
                }
                b'L' => {
                    pos += 1;
                    while pos < close && signature[pos] != b';' {
                        pos += 1;
                    }
                    pos += 1;
                    if slot < max_locals {
                        seed.locals[slot] = SE {
                            ty: Ty::Ref,
                            second: false,
                        };
                        slot += 1;
                    }
                }
                b'J' | b'D' => {
                    pos += 1;
                    if slot < max_locals {
                        seed.locals[slot] = SE {
                            ty: Ty::Num,
                            second: false,
                        };
                        if slot + 1 < max_locals {
                            seed.locals[slot + 1] = SE {
                                ty: Ty::Num,
                                second: true,
                            };
                        }
                    }
                    slot += 2;
                }
                _ => {
                    pos += 1;
                    if slot < max_locals {
                        seed.locals[slot] = SE {
                            ty: Ty::Num,
                            second: false,
                        };
                        slot += 1;
                    }
                }
            }
        }
    }

    let mut in_frames: Vec<Option<Frame>> = (0..n).map(|_| None).collect();
    let mut out_frames: Vec<Option<Frame>> = (0..n).map(|_| None).collect();

    for _round in 0..64 {
        let mut changed = false;
        for i in 0..n {
            // in = merge of predecessor outs (entry uses the seed).
            let frame_in = if i == 0 {
                Some(seed.clone())
            } else {
                let mut merged: Option<Frame> = None;
                for &p in &preds[i] {
                    if let Some(out) = &out_frames[p] {
                        match &mut merged {
                            None => merged = Some(out.clone()),
                            Some(m) => {
                                m.merge(out);
                            }
                        }
                    }
                }
                merged
            };
            let Some(frame_in) = frame_in else {
                continue;
            };
            let mut frame_out = frame_in.clone();
            if !xfer.step(&mut frame_out, code, instrs[i].bci) {
                valid = false;
                frame_out.locals = vec![SE::TOP; max_locals];
                frame_out.stack.clear();
            }
            if in_frames[i].as_ref() != Some(&frame_in) {
                in_frames[i] = Some(frame_in);
                changed = true;
            }
            if out_frames[i].as_ref() != Some(&frame_out) {
                out_frames[i] = Some(frame_out);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for i in 0..n {
        if let Some(f) = &in_frames[i] {
            for s in 0..max_locals.min(f.locals.len()) {
                if f.locals[s].ty.is_maybe_oop() {
                    bits_set(&mut oops_in[i], s);
                }
            }
        } else {
            for s in 0..max_locals {
                bits_set(&mut oops_in[i], s);
            }
        }
    }
    if !valid {
        for i in 0..n {
            for w in 0..words {
                oops_in[i][w] = live_in[i][w];
            }
        }
    }

    CiLiveness {
        instr_bcis: bcis,
        live_in,
        oops_in,
        max_locals,
        valid,
    }
}
