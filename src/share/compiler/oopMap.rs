/*
 * Port of OpenJDK 25 HotSpot OopMap (share/compiler/oopMap.hpp / oopMap.cpp).
 *
 * OopMaps describe which VM locations (registers and stack slots) hold live
 * OOP references at a safepoint pc-offset.  Each map serializes a stream of
 * OopMapValue entries through a CompressedWriteStream, exactly like HotSpot:
 *   - type   : 2 low bits of the u16 value (oop / narrowoop / callee_saved /
 *              derived_oop)
 *   - reg    : 14 high bits = VMReg encoding of the location
 *   - content_reg : extra VMReg written only for callee_saved and derived_oop
 *
 * VMReg encoding: values below STACK_REG_BASE are machine registers; stack
 * slots are encoded as STACK_REG_BASE + slot index (analogous to
 * VMRegImpl::stack2reg).
 */

use crate::share::code::compressedStream::{CompressedReadStream, CompressedWriteStream};
use std::cmp::Ordering;

pub const STACK_REG_BASE: i32 = 1 << 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VMReg(i32);

impl VMReg {
    pub const BAD: VMReg = VMReg(-1);

    #[inline]
    pub fn reg(n: i32) -> Self {
        VMReg(n)
    }

    #[inline]
    pub fn stack_slot(slot: i32) -> Self {
        VMReg(STACK_REG_BASE + slot)
    }

    #[inline]
    pub fn stack2reg(slot: i32) -> Self {
        Self::stack_slot(slot)
    }

    #[inline]
    pub fn value(self) -> i32 {
        self.0
    }

    #[inline]
    pub fn is_valid(self) -> bool {
        self.0 >= 0
    }

    #[inline]
    pub fn is_reg(self) -> bool {
        self.0 >= 0 && self.0 < STACK_REG_BASE
    }

    #[inline]
    pub fn is_stack(self) -> bool {
        self.0 >= STACK_REG_BASE
    }

    #[inline]
    pub fn reg2stack(self) -> i32 {
        self.0 - STACK_REG_BASE
    }

    /// Frame slot offset in bytes (4-byte stack slots like HotSpot LP64).
    #[inline]
    pub fn stack_slot_offset(self) -> i32 {
        if self.is_stack() { self.reg2stack() * 4 } else { 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum OopMapType {
    Oop = 0,
    NarrowOop = 1,
    CalleeSaved = 2,
    DerivedOop = 3,
}

impl OopMapType {
    pub fn from_bits(bits: u16) -> Self {
        match bits & 0b11 {
            0 => Self::Oop,
            1 => Self::NarrowOop,
            2 => Self::CalleeSaved,
            3 => Self::DerivedOop,
            _ => unreachable!(),
        }
    }
}

/// One serialized entry; `value` packs type (2 bits) and register (14 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OopMapValue {
    value: u16,
    content_reg: i16,
}

impl OopMapValue {
    pub const TYPE_BITS: u32 = 2;
    pub const REGISTER_BITS: u32 = 16 - Self::TYPE_BITS;
    pub const TYPE_SHIFT: u32 = 0;
    pub const REGISTER_SHIFT: u32 = Self::TYPE_BITS;
    pub const TYPE_MASK: u16 = 0b11;
    pub const REGISTER_MASK: u16 = (1 << Self::REGISTER_BITS) - 1;

    pub fn new() -> Self {
        Self {
            value: 0,
            content_reg: VMReg::BAD.value() as i16,
        }
    }

    pub fn of(reg: VMReg, t: OopMapType, content: VMReg) -> Self {
        let mut v = Self::new();
        v.set_reg_type(reg, t);
        v.set_content_reg(content);
        v
    }

    #[inline]
    pub fn value(&self) -> u16 {
        self.value
    }

    #[inline]
    pub fn content_reg(&self) -> VMReg {
        VMReg(self.content_reg as i32)
    }

    #[inline]
    pub fn map_type(&self) -> OopMapType {
        OopMapType::from_bits(self.value)
    }

    #[inline]
    pub fn reg(&self) -> VMReg {
        VMReg(((self.value >> Self::REGISTER_SHIFT) & Self::REGISTER_MASK) as i32)
    }

    #[inline]
    fn set_value(&mut self, v: u16) {
        self.value = v;
    }

    fn set_reg_type(&mut self, reg: VMReg, t: OopMapType) {
        let reg_bits = (reg.value() as u16) & Self::REGISTER_MASK;
        self.set_value((reg_bits << Self::REGISTER_SHIFT) | (t as u16));
    }

    fn set_content_reg(&mut self, reg: VMReg) {
        self.content_reg = reg.value() as i16;
    }

    pub fn is_oop(&self) -> bool {
        self.map_type() == OopMapType::Oop
    }

    pub fn is_narrowoop(&self) -> bool {
        self.map_type() == OopMapType::NarrowOop
    }

    pub fn is_callee_saved(&self) -> bool {
        self.map_type() == OopMapType::CalleeSaved
    }

    pub fn is_derived_oop(&self) -> bool {
        self.map_type() == OopMapType::DerivedOop
    }

    /// HotSpot `legal_vm_reg_name`: the register must fit in 14 bits.
    pub fn legal_vm_reg_name(reg: VMReg) -> bool {
        reg.value() >= 0 && (reg.value() as u32) <= Self::REGISTER_MASK as u32
    }

    /// Serialize: u32 value, plus content_reg for callee_saved/derived_oop.
    pub fn write_on(&self, stream: &mut CompressedWriteStream) {
        stream.write_int(self.value as u32);
        if self.is_callee_saved() || self.is_derived_oop() {
            stream.write_int(self.content_reg as u32);
        }
    }

    pub fn read_from(stream: &mut CompressedReadStream) -> Option<Self> {
        let value = stream.read_int()? as u16;
        let mut v = Self {
            value,
            content_reg: VMReg::BAD.value() as i16,
        };
        if v.is_callee_saved() || v.is_derived_oop() {
            let cr = stream.read_int()? as i32;
            v.set_content_reg(VMReg(cr));
        }
        Some(v)
    }
}

/// Cost model used by OopMapSort: registers are cheapest, then stack slots.
fn reg_cost(reg: VMReg) -> i32 {
    if reg.is_reg() {
        0
    } else {
        reg.reg2stack() * 4
    }
}

fn derived_cost(v: &OopMapValue) -> i32 {
    // Derived entries sort after their base; cost is dominated by location.
    reg_cost(v.reg())
}

/// OopMapSort equivalent: orders entries so that all derived oops appear
/// after the entries they derive from, and cheaper locations come first.
fn sorted_entries(map: &OopMap) -> Vec<OopMapValue> {
    let mut entries: Vec<OopMapValue> = map.entries.clone();
    entries.sort_by(|a, b| match (a.is_derived_oop(), b.is_derived_oop()) {
        (false, true) => Ordering::Less,
        (true, false) => Ordering::Greater,
        _ => reg_cost(a.reg()).cmp(&reg_cost(b.reg())),
    });
    // Stable pass: ensure each derived_oop sits after its content base.
    let mut out: Vec<OopMapValue> = Vec::with_capacity(entries.len());
    for v in entries {
        if v.is_derived_oop() {
            let base = v.content_reg();
            // Move past any entries whose reg is not the base that precede
            // other derived entries of the same base with lower cost.
            let pos = out
                .iter()
                .rposition(|e| e.reg() == base || e.content_reg() == base);
            let insert_at = pos.map(|p| p + 1).unwrap_or(out.len());
            out.insert(insert_at.min(out.len()), v);
        } else {
            out.push(v);
        }
    }
    // Suppress unused-warning path for derived_cost helper parity.
    let _ = derived_cost(&OopMapValue::new());
    out
}

/// A single frame map for one pc offset.
#[derive(Clone, Debug, PartialEq)]
pub struct OopMap {
    pub pc_offset: i32,
    omv_count: usize,
    num_oops: usize,
    has_derived_oops: bool,
    pub index: i32,
    entries: Vec<OopMapValue>,
    stream: CompressedWriteStream,
    used: Vec<bool>,
}

impl OopMap {
    pub fn new(frame_size: usize, _arg_count: usize) -> Self {
        Self {
            pc_offset: 0,
            omv_count: 0,
            num_oops: 0,
            has_derived_oops: false,
            index: -1,
            entries: Vec::new(),
            stream: CompressedWriteStream::with_capacity(32),
            used: vec![false; frame_size.max(64)],
        }
    }

    /// Deep-copy constructor (OopMap::DeepCopyToken analogue).
    pub fn deep_copy(&self) -> Self {
        let mut copy = Self::new(self.used.len(), 0);
        copy.pc_offset = self.pc_offset;
        copy.num_oops = self.num_oops;
        copy.has_derived_oops = self.has_derived_oops;
        for v in &self.entries {
            v.write_on(&mut copy.stream);
            copy.omv_count += 1;
        }
        copy.entries = self.entries.clone();
        copy
    }

    pub fn offset(&self) -> i32 {
        self.pc_offset
    }

    pub fn set_offset(&mut self, o: i32) {
        self.pc_offset = o;
    }

    pub fn count(&self) -> usize {
        self.omv_count
    }

    pub fn omv_count(&self) -> usize {
        self.omv_count
    }

    pub fn num_oops(&self) -> usize {
        self.num_oops
    }

    pub fn has_derived_oops(&self) -> bool {
        self.has_derived_oops
    }

    pub fn data_size(&self) -> usize {
        self.stream.position()
    }

    pub fn data(&self) -> &[u8] {
        self.stream.buffer()
    }

    pub fn entries(&self) -> &[OopMapValue] {
        &self.entries
    }

    fn set_xxx(&mut self, reg: VMReg, t: OopMapType, optional: VMReg) {
        debug_assert!(OopMapValue::legal_vm_reg_name(reg));
        let idx = reg.value() as usize;
        if idx < self.used.len() {
            debug_assert!(!self.used[idx], "cannot insert twice");
            self.used[idx] = true;
        }
        let omv = OopMapValue::of(reg, t, optional);
        omv.write_on(&mut self.stream);
        self.omv_count += 1;
        self.entries.push(omv);
        match t {
            OopMapType::Oop | OopMapType::NarrowOop => self.num_oops += 1,
            OopMapType::DerivedOop => self.has_derived_oops = true,
            OopMapType::CalleeSaved => {}
        }
    }

    pub fn set_oop(&mut self, reg: VMReg) {
        self.set_xxx(reg, OopMapType::Oop, VMReg::BAD);
    }

    pub fn set_narrowoop(&mut self, reg: VMReg) {
        self.set_xxx(reg, OopMapType::NarrowOop, VMReg::BAD);
    }

    pub fn set_callee_saved(&mut self, reg: VMReg, caller_machine_register: VMReg) {
        self.set_xxx(reg, OopMapType::CalleeSaved, caller_machine_register);
    }

    pub fn set_derived_oop(&mut self, reg: VMReg, derived_from_local_register: VMReg) {
        if reg == derived_from_local_register {
            self.set_oop(reg);
        } else {
            self.set_xxx(reg, OopMapType::DerivedOop, derived_from_local_register);
        }
    }

    /// Iterator over the serialized (and OopMapSort-ordered) entries.
    pub fn sorted_entries(&self) -> Vec<OopMapValue> {
        sorted_entries(self)
    }

    /// Serialize the sorted entry stream, like `copy_and_sort_data_to`.
    pub fn copy_and_sort_data_to(&self) -> Vec<u8> {
        let mut stream = CompressedWriteStream::new();
        for v in self.sorted_entries() {
            v.write_on(&mut stream);
        }
        stream.into_buffer()
    }

    /// Iterate entries of a serialized map (OopMapStream analogue).
    pub fn stream_entries(data: &[u8]) -> Vec<OopMapValue> {
        let mut s = CompressedReadStream::new(data);
        let mut out = Vec::new();
        while !s.eof() {
            match OopMapValue::read_from(&mut s) {
                Some(v) => out.push(v),
                None => break,
            }
        }
        out
    }

    pub fn heap_size(&self) -> usize {
        self.data_size()
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(
            f,
            "OopMap[pc={} oops={} derived={} entries={}]",
            self.pc_offset, self.num_oops, self.has_derived_oops, self.omv_count
        );
        for v in self.entries.iter() {
            let _ = writeln!(
                f,
                "  {:?} reg={:?} content={:?}",
                v.map_type(),
                v.reg(),
                v.content_reg()
            );
        }
    }
}

/// A set of OopMaps attached to a CodeBlob / nmethod, indexed by pc offset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OopMapSet {
    maps: Vec<OopMap>,
}

impl OopMapSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// `OopMapSet::add_gc_map`: assigns the pc offset and appends the map.
    /// Maps are expected in non-decreasing pc-offset order (like HotSpot).
    pub fn add_gc_map(&mut self, pc_offset: i32, map: OopMap) -> usize {
        let mut map = map;
        map.set_offset(pc_offset);
        debug_assert!(
            self.maps.last().map_or(true, |last| last.offset() <= map.offset()),
            "OopMaps must be added in pc-offset order"
        );
        self.maps.push(map);
        self.maps.len() - 1
    }

    /// Add a map that is already offset-tagged.
    pub fn add(&mut self, map: OopMap) -> usize {
        self.maps.push(map);
        self.maps.len() - 1
    }

    pub fn size(&self) -> usize {
        self.maps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }

    pub fn at(&self, index: usize) -> Option<&OopMap> {
        self.maps.get(index)
    }

    pub fn maps(&self) -> &[OopMap] {
        &self.maps
    }

    /// Exact-offset lookup (like `ImmutableOopMapSet::find_map_at_offset`,
    /// which requires an exact pc-offset match).
    pub fn find_map_at_offset(&self, pc_offset: i32) -> Option<&OopMap> {
        self.maps.iter().find(|m| m.offset() == pc_offset)
    }

    /// `CodeBlob::oop_map_for_return_address` semantics: the map covering
    /// `pc_offset` is the one with the greatest offset <= pc_offset.
    pub fn map_for_pc(&self, pc_offset: i32) -> Option<&OopMap> {
        // Maps are sorted by pc_offset; binary search for the last <= target.
        let idx = self
            .maps
            .partition_point(|m| m.offset() <= pc_offset);
        if idx == 0 {
            None
        } else {
            Some(&self.maps[idx - 1])
        }
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        for m in &self.maps {
            let _ = m.print_on(f);
        }
    }
}
