/*
 * Port of OpenJDK 25 HotSpot share/code/relocInfo.hpp
 *
 * Relocation types and the relocation table attached to a CodeBuffer.
 * HotSpot packs relocInfo entries as (pc_offset, type, format, value);
 * here RelocEntry mirrors that record and RelocTable is the ordered set
 * of entries that RelocIterator walks over.
 */

use std::fmt;

/// relocation type codes, mirrors relocInfo::relocType (JDK 25 numbering).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RelocType {
    None = 0,
    Oop = 1,                  // oop_type
    VirtualCall = 2,          // virtual_call_type
    OptVirtualCall = 3,       // opt_virtual_call_type
    StaticCall = 4,           // static_call_type
    StaticStub = 5,           // static_stub_type
    RuntimeCall = 6,          // runtime_call_type
    ExternalWord = 7,         // external_word_type
    InternalWord = 8,         // internal_word_type
    SectionWord = 9,          // section_word_type
    Poll = 10,                // poll_type
    PollReturn = 11,          // poll_return_type
    Metadata = 12,            // metadata_type
    TrampolineStub = 13,      // trampoline_stub_type
    RuntimeCallWcp = 14,      // runtime_call_w_cp_type
    DataPrefixTag = 15,       // data_prefix_tag
    PostCallNop = 16,         // post_call_nop_type
    EntryGuard = 17,          // entry_guard_type
    Barrier = 18,             // barrier_type
}

impl RelocType {
    pub const TYPE_MASK: u8 = 31;

    pub fn from_code(code: u8) -> Option<RelocType> {
        match code {
            0 => Some(RelocType::None),
            1 => Some(RelocType::Oop),
            2 => Some(RelocType::VirtualCall),
            3 => Some(RelocType::OptVirtualCall),
            4 => Some(RelocType::StaticCall),
            5 => Some(RelocType::StaticStub),
            6 => Some(RelocType::RuntimeCall),
            7 => Some(RelocType::ExternalWord),
            8 => Some(RelocType::InternalWord),
            9 => Some(RelocType::SectionWord),
            10 => Some(RelocType::Poll),
            11 => Some(RelocType::PollReturn),
            12 => Some(RelocType::Metadata),
            13 => Some(RelocType::TrampolineStub),
            14 => Some(RelocType::RuntimeCallWcp),
            15 => Some(RelocType::DataPrefixTag),
            16 => Some(RelocType::PostCallNop),
            17 => Some(RelocType::EntryGuard),
            18 => Some(RelocType::Barrier),
            _ => None,
        }
    }

    pub fn code(self) -> u8 {
        self as u8
    }

    /// Types that patch a 32-bit displacement/word field of an instruction.
    pub fn is_call_or_branch(self) -> bool {
        matches!(
            self,
            RelocType::StaticCall
                | RelocType::StaticStub
                | RelocType::RuntimeCall
                | RelocType::TrampolineStub
                | RelocType::VirtualCall
                | RelocType::OptVirtualCall
                | RelocType::RuntimeCallWcp
        )
    }

    /// Types carrying an embedded oop / metadata reference.
    pub fn is_data_ref(self) -> bool {
        matches!(self, RelocType::Oop | RelocType::Metadata)
    }
}

/// Width of the field the relocation patches (mirrors the format bits
/// stored in relocInfo::_value).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RelocWidth {
    /// 32-bit word / displacement (x86 call/jcc/disp32).
    Word32 = 0,
    /// 64-bit absolute word (movabs).
    Word64 = 1,
    /// 8-bit short branch displacement.
    Byte8 = 2,
}

impl RelocWidth {
    pub fn size(self) -> usize {
        match self {
            RelocWidth::Word32 => 4,
            RelocWidth::Word64 => 8,
            RelocWidth::Byte8 => 1,
        }
    }
}

/// Value carried by a relocation (Relocation::value() equivalent).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RelocValue {
    /// No payload (poll sites, barriers store their data inline).
    None,
    /// Embedded oop (compressed or full, raw handle/address).
    Oop(u64),
    /// Metadata reference (klass/method handle).
    Metadata(u64),
    /// Reference inside the same code blob: target pc offset + addend.
    Internal { target: u64, addend: i32 },
    /// Fixed external symbol: name + addend.
    External { name: String, addend: i32 },
    /// Runtime call whose target comes from the constant pool.
    RuntimeCallWcp { method_index: u32, cp_index: u32 },
    /// Section reference (stub section target).
    Section { section: u8, offset: u64 },
    /// Poll site metadata.
    Poll { data: u64 },
    /// GC barrier descriptor.
    Barrier { data: u64 },
    /// Raw absolute address (runtime stub entry point).
    Address(u64),
}

/// One relocation record, equivalent to a relocInfo at a pc offset.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RelocEntry {
    /// Offset of the instruction start (pc offset from insts_begin).
    pub pc_offset: u32,
    /// Offset of the patched field within the instruction (HotSpot
    /// `relocInfo::addr()` points at the instruction start; the field
    /// offset is `format`-dependent).  We store the field offset for
    /// unambiguous patching.
    pub field_offset: u32,
    pub rtype: RelocType,
    pub width: RelocWidth,
    pub value: RelocValue,
}

impl RelocEntry {
    pub fn new(pc_offset: u32, rtype: RelocType, width: RelocWidth, value: RelocValue) -> Self {
        Self { pc_offset, field_offset: 0, rtype, width, value }
    }

    /// Binary encoding for blob relocation tables (little-endian).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(32);
        v.push(self.rtype.code());
        v.push(self.width as u8);
        v.extend_from_slice(&self.pc_offset.to_le_bytes());
        v.extend_from_slice(&self.field_offset.to_le_bytes());
        match &self.value {
            RelocValue::None => v.push(0),
            RelocValue::Oop(x) => {
                v.push(1);
                v.extend_from_slice(&x.to_le_bytes());
            }
            RelocValue::Metadata(x) => {
                v.push(2);
                v.extend_from_slice(&x.to_le_bytes());
            }
            RelocValue::Internal { target, addend } => {
                v.push(3);
                v.extend_from_slice(&target.to_le_bytes());
                v.extend_from_slice(&addend.to_le_bytes());
            }
            RelocValue::External { name, addend } => {
                v.push(4);
                v.extend_from_slice(&(name.len() as u32).to_le_bytes());
                v.extend_from_slice(name.as_bytes());
                v.extend_from_slice(&addend.to_le_bytes());
            }
            RelocValue::RuntimeCallWcp { method_index, cp_index } => {
                v.push(5);
                v.extend_from_slice(&method_index.to_le_bytes());
                v.extend_from_slice(&cp_index.to_le_bytes());
            }
            RelocValue::Section { section, offset } => {
                v.push(6);
                v.push(*section);
                v.extend_from_slice(&offset.to_le_bytes());
            }
            RelocValue::Poll { data } => {
                v.push(7);
                v.extend_from_slice(&data.to_le_bytes());
            }
            RelocValue::Barrier { data } => {
                v.push(8);
                v.extend_from_slice(&data.to_le_bytes());
            }
            RelocValue::Address(x) => {
                v.push(9);
                v.extend_from_slice(&x.to_le_bytes());
            }
        }
        v
    }

    pub fn decode(buf: &[u8]) -> Result<(RelocEntry, usize), RelocError> {
        fn need(buf: &[u8], at: usize, n: usize) -> Result<(), RelocError> {
            if buf.len() < at + n {
                return Err(RelocError::Truncated);
            }
            Ok(())
        }
        fn u8at(buf: &[u8], at: usize) -> u8 { buf[at] }
        fn u32at(buf: &[u8], at: usize) -> u32 {
            u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
        }
        fn u64at(buf: &[u8], at: usize) -> u64 {
            u64::from_le_bytes([
                buf[at], buf[at + 1], buf[at + 2], buf[at + 3],
                buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7],
            ])
        }
        need(buf, 0, 12)?;
        let mut at = 0usize;
        let rcode = u8at(buf, at);
        at += 1;
        let rtype = RelocType::from_code(rcode).ok_or(RelocError::BadType(rcode))?;
        let wcode = u8at(buf, at);
        at += 1;
        let width = match wcode {
            0 => RelocWidth::Word32,
            1 => RelocWidth::Word64,
            2 => RelocWidth::Byte8,
            _ => return Err(RelocError::BadWidth(wcode)),
        };
        let pc_offset = u32at(buf, at);
        at += 4;
        let field_offset = u32at(buf, at);
        at += 4;
        need(buf, at, 1)?;
        let vkind = u8at(buf, at);
        at += 1;
        let value = match vkind {
            0 => RelocValue::None,
            1 => {
                need(buf, at, 8)?;
                let x = u64at(buf, at);
                at += 8;
                RelocValue::Oop(x)
            }
            2 => {
                need(buf, at, 8)?;
                let x = u64at(buf, at);
                at += 8;
                RelocValue::Metadata(x)
            }
            3 => {
                need(buf, at, 12)?;
                let target = u64at(buf, at);
                at += 8;
                let addend = u32at(buf, at) as i32;
                at += 4;
                RelocValue::Internal { target, addend }
            }
            4 => {
                need(buf, at, 4)?;
                let len = u32at(buf, at) as usize;
                at += 4;
                need(buf, at, len + 4)?;
                let name = String::from_utf8(buf[at..at + len].to_vec())
                    .map_err(|_| RelocError::BadUtf8)?;
                at += len;
                let addend = u32at(buf, at) as i32;
                at += 4;
                RelocValue::External { name, addend }
            }
            5 => {
                need(buf, at, 8)?;
                let method_index = u32at(buf, at);
                at += 4;
                let cp_index = u32at(buf, at);
                at += 4;
                RelocValue::RuntimeCallWcp { method_index, cp_index }
            }
            6 => {
                need(buf, at, 9)?;
                let section = u8at(buf, at);
                at += 1;
                let offset = u64at(buf, at);
                at += 8;
                RelocValue::Section { section, offset }
            }
            7 => {
                need(buf, at, 8)?;
                let data = u64at(buf, at);
                at += 8;
                RelocValue::Poll { data }
            }
            8 => {
                need(buf, at, 8)?;
                let data = u64at(buf, at);
                at += 8;
                RelocValue::Barrier { data }
            }
            9 => {
                need(buf, at, 8)?;
                let x = u64at(buf, at);
                at += 8;
                RelocValue::Address(x)
            }
            _ => return Err(RelocError::BadValueKind(vkind)),
        };
        Ok((
            RelocEntry { pc_offset, field_offset, rtype, width, value },
            at,
        ))
    }
}

#[derive(Debug)]
pub enum RelocError {
    Truncated,
    BadType(u8),
    BadWidth(u8),
    BadValueKind(u8),
    BadUtf8,
    InvalidOffset,
}

impl fmt::Display for RelocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelocError::Truncated => write!(f, "relocation table truncated"),
            RelocError::BadType(t) => write!(f, "bad relocation type {}", t),
            RelocError::BadWidth(w) => write!(f, "bad relocation width {}", w),
            RelocError::BadValueKind(k) => write!(f, "bad relocation value kind {}", k),
            RelocError::BadUtf8 => write!(f, "bad utf8 in external reloc name"),
            RelocError::InvalidOffset => write!(f, "invalid relocation offset"),
        }
    }
}

impl std::error::Error for RelocError {}

/// Ordered relocation table attached to a CodeBuffer.  Entries are kept
/// sorted by pc_offset, like relocInfo records sorted over the blob.
#[derive(Clone, Debug, Default)]
pub struct RelocTable {
    entries: Vec<RelocEntry>,
}

impl RelocTable {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, RelocEntry> {
        self.entries.iter()
    }

    /// Insert keeping the table sorted by pc_offset.
    pub fn add(&mut self, entry: RelocEntry) {
        let pos = self
            .entries
            .partition_point(|e| e.pc_offset < entry.pc_offset || (e.pc_offset == entry.pc_offset && e.field_offset <= entry.field_offset));
        self.entries.insert(pos, entry);
    }

    /// First entry with pc_offset >= offset (RelocIterator::next lower bound).
    pub fn lower_bound(&self, offset: u32) -> Option<usize> {
        let i = self.entries.partition_point(|e| e.pc_offset < offset);
        if i < self.entries.len() { Some(i) } else { None }
    }

    /// Entries covering [start, end) pc offsets.
    pub fn in_range(&self, start: u32, end: u32) -> impl Iterator<Item = &RelocEntry> {
        self.entries
            .iter()
            .skip_while(move |e| e.pc_offset < start)
            .take_while(move |e| e.pc_offset < end)
    }

    pub fn entry(&self, index: usize) -> Option<&RelocEntry> {
        self.entries.get(index)
    }

    pub fn remove_at(&mut self, index: usize) -> Option<RelocEntry> {
        if index < self.entries.len() {
            Some(self.entries.remove(index))
        } else {
            None
        }
    }

    /// Serialize the whole table; decode with `decode_table`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.entries.len() * 16);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            out.extend_from_slice(&e.encode());
        }
        out
    }

    pub fn decode_table(buf: &[u8]) -> Result<RelocTable, RelocError> {
        if buf.len() < 4 {
            return Err(RelocError::Truncated);
        }
        let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let mut table = RelocTable::new();
        let mut at = 4usize;
        for _ in 0..count {
            let (entry, used) = RelocEntry::decode(&buf[at..])?;
            at += used;
            table.entries.push(entry);
        }
        if at != buf.len() {
            return Err(RelocError::Truncated);
        }
        Ok(table)
    }

    /// Total serialized size, like relocInfo::size / CodeBuffer
    /// `total_relocation_size`.
    pub fn total_size(&self) -> usize {
        self.entries.iter().map(|e| e.encode().len()).sum::<usize>() + 4
    }
}
