/*
 * Port of OpenJDK 25 HotSpot ScopeDesc (share/code/scopeDesc.hpp / scopeDesc.cpp)
 * and the ScopeValue family (share/code/debugInfo.hpp / debugInfo.cpp)
 *
 * ScopeDesc describes debug and deoptimization information for JIT-compiled
 * frames: method identity, bytecode indices (BCI), inlining call tree
 * (sender chain), locals, expressions, object values (escape-analyzed
 * allocations) and lock monitors.
 *
 * In HotSpot the data lives in the nmethod's "scopes data" blob and is read
 * through DebugInfoReadStream.  Here the same structure is serialized with
 * the UNSIGNED5 compressed stream (code::compressedStream) produced by
 * ScopeDescWriter and decoded by ScopeDescReader — a real, round-trippable
 * encoding rather than a placeholder.  Scope-value wire codes mirror the
 * debugInfo.cpp constants (LOCATION_CODE .. OBJECT_MERGE_CODE).
 */

use crate::share::code::compressedStream::{CompressedReadStream, CompressedWriteStream};

pub const NO_SENDER_DECODE_OFFSET: i32 = -1;
/// DebugInformationRecorder::serialized_null == 0; byte offset 0 is
/// reserved so real records always start at >= 1.
pub const SERIALIZED_NULL: i32 = 0;
/// InvocationEntryBci (HotSpot constant, used by the sentinel scope).
pub const INVOCATION_ENTRY_BCI: i32 = -1;

// ---------------------------------------------------------------------------
// Location (share/code/location.hpp)
// ---------------------------------------------------------------------------

/// Where a location lives: on the stack or in a register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocationWhere {
    OnStack,
    InRegister,
}

/// Type of value held at a location (Location::Type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocationType {
    Invalid,
    Normal,
    Oop,
    IntInLong,
    Lng,
    FloatInDbl,
    Dbl,
    Vector,
    Addr,
    NarrowOop,
}

impl LocationType {
    fn to_u32(self) -> u32 {
        match self {
            Self::Invalid => 0,
            Self::Normal => 1,
            Self::Oop => 2,
            Self::IntInLong => 3,
            Self::Lng => 4,
            Self::FloatInDbl => 5,
            Self::Dbl => 6,
            Self::Vector => 7,
            Self::Addr => 8,
            Self::NarrowOop => 9,
        }
    }

    fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Invalid,
            1 => Self::Normal,
            2 => Self::Oop,
            3 => Self::IntInLong,
            4 => Self::Lng,
            5 => Self::FloatInDbl,
            6 => Self::Dbl,
            7 => Self::Vector,
            8 => Self::Addr,
            9 => Self::NarrowOop,
            _ => return None,
        })
    }
}

/// Bit-packed machine location (Location analogue): type in the low 4 bits,
/// the where bit at bit 4, and the 4-byte-aligned offset shifted left by 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    value: u32,
}

impl Location {
    const WHERE_SHIFT: u32 = 4;
    const OFFSET_SHIFT: u32 = 5;

    /// Stack location factory.  `offset_in_bytes` must be 4-byte aligned.
    pub fn new_stk_loc(type_: LocationType, offset_in_bytes: i32) -> Self {
        let offset_words = (offset_in_bytes >> 2) as u32;
        Self::pack(LocationWhere::OnStack, type_, offset_words)
    }

    /// Register location factory; `reg` is the VMReg value.
    pub fn new_reg_loc(type_: LocationType, reg: i32) -> Self {
        Self::pack(LocationWhere::InRegister, type_, reg as u32)
    }

    fn pack(where_: LocationWhere, type_: LocationType, offset: u32) -> Self {
        let where_bit = match where_ {
            LocationWhere::OnStack => 0u32,
            LocationWhere::InRegister => 1u32,
        };
        let value = (type_.to_u32() & 0xF)
            | (where_bit << Self::WHERE_SHIFT)
            | ((offset << Self::OFFSET_SHIFT) & 0xFFFF_FFE0);
        Self { value }
    }

    #[inline]
    pub fn where_(&self) -> LocationWhere {
        if (self.value & (1 << Self::WHERE_SHIFT)) != 0 {
            LocationWhere::InRegister
        } else {
            LocationWhere::OnStack
        }
    }

    #[inline]
    pub fn type_(&self) -> Option<LocationType> {
        LocationType::from_u32(self.value & 0xF)
    }

    #[inline]
    pub fn offset(&self) -> u32 {
        (self.value & 0xFFFF_FFE0) >> Self::OFFSET_SHIFT
    }

    #[inline]
    pub fn is_register(&self) -> bool {
        self.where_() == LocationWhere::InRegister
    }

    #[inline]
    pub fn is_stack(&self) -> bool {
        self.where_() == LocationWhere::OnStack
    }

    /// Byte offset for stack locations.
    #[inline]
    pub fn stack_offset(&self) -> i32 {
        (self.offset() as i32) << 2
    }

    /// Register number for register locations.
    #[inline]
    pub fn register_number(&self) -> i32 {
        self.offset() as i32
    }

    /// Whether this location may contain an oop that the GC must visit.
    #[inline]
    pub fn is_oop_location(&self) -> bool {
        matches!(
            self.type_(),
            Some(LocationType::Oop | LocationType::NarrowOop)
        )
    }

    pub fn write_on(&self, stream: &mut CompressedWriteStream) {
        stream.write_int(self.value);
    }

    pub fn read_from(stream: &mut CompressedReadStream) -> Option<Self> {
        stream.read_int().map(|v| Self { value: v })
    }
}

impl Default for Location {
    fn default() -> Self {
        // Location() : on_stack, invalid, 0
        Self { value: 0 }
    }
}

// ---------------------------------------------------------------------------
// ScopeValue (share/code/debugInfo.hpp)
// ---------------------------------------------------------------------------

/// Wire codes, mirroring debugInfo.cpp.
mod sv_code {
    pub const LOCATION_CODE: u32 = 0;
    pub const CONSTANT_INT_CODE: u32 = 1;
    pub const CONSTANT_OOP_CODE: u32 = 2;
    pub const CONSTANT_LONG_CODE: u32 = 3;
    pub const CONSTANT_DOUBLE_CODE: u32 = 4;
    pub const OBJECT_CODE: u32 = 5;
    pub const OBJECT_ID_CODE: u32 = 6;
    pub const AUTO_BOX_OBJECT_CODE: u32 = 7;
    pub const MARKER_CODE: u32 = 8;
    pub const OBJECT_MERGE_CODE: u32 = 9;
    /// Extension for 32-bit float constants (HotSpot stores them inside
    /// ConstantDoubleValue/ConstantIntValue depending on backend).
    pub const CONSTANT_FLOAT_CODE: u32 = 10;
}

/// An object eliminated by escape analysis (ObjectValue analogue):
/// described by its class, a list of field values and identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectValue {
    id: i32,
    klass: Option<Box<ScopeValue>>,
    field_values: Vec<ScopeValue>,
    is_scalar_replaced: bool,
    is_root: bool,
    is_auto_box: bool,
}

impl ObjectValue {
    pub fn new(id: i32) -> Self {
        Self {
            id,
            klass: None,
            field_values: Vec::new(),
            is_scalar_replaced: true,
            is_root: true,
            is_auto_box: false,
        }
    }

    #[inline]
    pub fn id(&self) -> i32 {
        self.id
    }

    #[inline]
    pub fn klass(&self) -> Option<&ScopeValue> {
        self.klass.as_deref()
    }

    #[inline]
    pub fn set_klass(&mut self, klass: ScopeValue) {
        self.klass = Some(Box::new(klass));
    }

    #[inline]
    pub fn field_values(&self) -> &[ScopeValue] {
        &self.field_values
    }

    #[inline]
    pub fn field_at(&self, i: usize) -> Option<&ScopeValue> {
        self.field_values.get(i)
    }

    pub fn set_field_values(&mut self, fields: Vec<ScopeValue>) {
        self.field_values = fields;
    }

    #[inline]
    pub fn field_size(&self) -> usize {
        self.field_values.len()
    }

    #[inline]
    pub fn is_scalar_replaced(&self) -> bool {
        self.is_scalar_replaced
    }

    #[inline]
    pub fn set_is_scalar_replaced(&mut self, scd: bool) {
        self.is_scalar_replaced = scd;
    }

    #[inline]
    pub fn is_root(&self) -> bool {
        self.is_root
    }

    #[inline]
    pub fn set_root(&mut self, root: bool) {
        self.is_root = root;
    }

    #[inline]
    pub fn is_auto_box(&self) -> bool {
        self.is_auto_box
    }

    #[inline]
    pub fn set_auto_box(&mut self, auto_box: bool) {
        self.is_auto_box = auto_box;
    }
}

/// Objects that were Phi inputs in C2 where at least one input was scalar
/// replaced (ObjectMergeValue analogue).
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectMergeValue {
    id: i32,
    merge_pointer: Option<Box<ScopeValue>>,
    selector: Option<Box<ScopeValue>>,
    possible_objects: Vec<ObjectValue>,
    selected: Option<usize>,
}

impl ObjectMergeValue {
    pub fn new(id: i32) -> Self {
        Self {
            id,
            merge_pointer: None,
            selector: None,
            possible_objects: Vec::new(),
            selected: None,
        }
    }

    #[inline]
    pub fn id(&self) -> i32 {
        self.id
    }

    #[inline]
    pub fn merge_pointer(&self) -> Option<&ScopeValue> {
        self.merge_pointer.as_deref()
    }

    #[inline]
    pub fn set_merge_pointer(&mut self, v: ScopeValue) {
        self.merge_pointer = Some(Box::new(v));
    }

    #[inline]
    pub fn selector(&self) -> Option<&ScopeValue> {
        self.selector.as_deref()
    }

    #[inline]
    pub fn set_selector(&mut self, v: ScopeValue) {
        self.selector = Some(Box::new(v));
    }

    #[inline]
    pub fn possible_objects(&self) -> &[ObjectValue] {
        &self.possible_objects
    }

    pub fn add_possible_object(&mut self, obj: ObjectValue) {
        self.possible_objects.push(obj);
    }

    #[inline]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    #[inline]
    pub fn set_selected(&mut self, idx: Option<usize>) {
        self.selected = idx;
    }

    /// `ObjectMergeValue::select` analogue: resolves which object the
    /// selector value picks.  A selector of -1 (as a constant int)
    /// selects the merge pointer; otherwise it indexes possible_objects.
    pub fn select(&mut self) -> Option<usize> {
        let sel = match self.selector.as_deref() {
            Some(ScopeValue::ConstantInt(v)) => *v,
            _ => -2,
        };
        if sel == -1 {
            self.selected = None;
            None
        } else if sel >= 0 && (sel as usize) < self.possible_objects.len() {
            let idx = sel as usize;
            self.selected = Some(idx);
            Some(idx)
        } else {
            self.selected = None;
            None
        }
    }
}

/// A decoded value in a scope (ScopeValue analogue).  Variants correspond
/// to the debugInfo.cpp wire codes; `ConstantFloat` is an extension for
/// 32-bit float constants.
#[derive(Clone, Debug, PartialEq)]
pub enum ScopeValue {
    /// Value held at a machine location (LocationValue).
    Location(Location),
    /// Integer constant (ConstantIntValue).
    ConstantInt(i32),
    /// Float constant (raw bits) — extension code.
    ConstantFloat(u32),
    /// Oop constant, identified by its symbol/name
    /// (ConstantOopReadValue/ConstantOopWriteValue analogue).
    ConstantOop(String),
    /// Long constant (ConstantLongValue).
    ConstantLong(i64),
    /// Double constant (raw bits) (ConstantDoubleValue).
    ConstantDouble(u64),
    /// Escape-analyzed object (ObjectValue / AutoBoxObjectValue).
    Object(ObjectValue),
    /// Reference to a previously decoded object by id
    /// (OBJECT_ID_CODE / get_cached_object analogue).  When decoding with
    /// an object pool the reference is resolved to `Object` directly.
    ObjectId(i32),
    /// Object allocation merge (ObjectMergeValue).
    ObjectMerge(ObjectMergeValue),
    /// Placeholder with no concrete meaning (MarkerValue).
    Marker,
}

impl ScopeValue {
    // ---- testers ----------------------------------------------------
    #[inline]
    pub fn is_location(&self) -> bool {
        matches!(self, Self::Location(_))
    }

    #[inline]
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    #[inline]
    pub fn is_object_merge(&self) -> bool {
        matches!(self, Self::ObjectMerge(_))
    }

    #[inline]
    pub fn is_auto_box(&self) -> bool {
        matches!(self, Self::Object(o) if o.is_auto_box())
    }

    #[inline]
    pub fn is_marker(&self) -> bool {
        matches!(self, Self::Marker)
    }

    #[inline]
    pub fn is_constant_int(&self) -> bool {
        matches!(self, Self::ConstantInt(_))
    }

    #[inline]
    pub fn is_constant_double(&self) -> bool {
        matches!(self, Self::ConstantDouble(_))
    }

    #[inline]
    pub fn is_constant_long(&self) -> bool {
        matches!(self, Self::ConstantLong(_))
    }

    #[inline]
    pub fn is_constant_oop(&self) -> bool {
        matches!(self, Self::ConstantOop(_))
    }

    #[inline]
    pub fn as_location(&self) -> Option<Location> {
        match self {
            Self::Location(l) => Some(*l),
            _ => None,
        }
    }

    #[inline]
    pub fn as_object(&self) -> Option<&ObjectValue> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }

    #[inline]
    pub fn as_object_merge(&self) -> Option<&ObjectMergeValue> {
        match self {
            Self::ObjectMerge(o) => Some(o),
            _ => None,
        }
    }

    // ---- serialization ------------------------------------------------

    // Wire layout mirrors ObjectValue::write_on / read_object: code, id,
    // is_root, klass (optional), field count, field values.  The object is
    // registered in the pool *before* its fields are decoded so that
    // OBJECT_ID_CODE self/cyclic references resolve (HotSpot pushes the
    // ObjectValue into _obj_pool prior to read_object).
    fn write_object_value(
        stream: &mut CompressedWriteStream,
        obj: &ObjectValue,
        code: u32,
        pool: &mut Vec<ObjectValue>,
    ) {
        stream.write_int(code);
        stream.write_signed_int(obj.id);
        stream.write_bool(obj.is_root);
        match &obj.klass {
            Some(k) => {
                stream.write_bool(true);
                k.write_on_with_pool(stream, pool);
            }
            None => stream.write_bool(false),
        }
        stream.write_int(obj.field_values.len() as u32);
        for f in &obj.field_values {
            f.write_on_with_pool(stream, pool);
        }
    }

    fn read_object_value(
        stream: &mut CompressedReadStream,
        auto_box: bool,
        pool: &mut Vec<ObjectValue>,
    ) -> Option<ObjectValue> {
        let id = stream.read_signed_int()?;
        // Register a placeholder before reading fields so OBJECT_ID_CODE
        // references inside the field list resolve to this object.
        pool.push(ObjectValue::new(id));
        let is_root = stream.read_bool()?;
        let has_klass = stream.read_bool()?;
        let klass = if has_klass {
            Some(Box::new(ScopeValue::read_from(stream, Some(&mut *pool))?))
        } else {
            None
        };
        let field_count = stream.read_int()? as usize;
        let mut field_values = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            field_values.push(ScopeValue::read_from(stream, Some(&mut *pool))?);
        }
        let mut obj = ObjectValue::new(id);
        obj.klass = klass;
        obj.field_values = field_values;
        obj.is_scalar_replaced = true;
        obj.is_root = is_root;
        obj.is_auto_box = auto_box;
        // Replace the placeholder with the fully decoded object.
        if let Some(slot) = pool.iter_mut().find(|o| o.id == id) {
            *slot = obj.clone();
        }
        Some(obj)
    }

    /// `get_cached_object` analogue: resolve an object id against the
    /// object pool accumulated during decode_object_values.
    fn get_cached_object(pool: &[ObjectValue], id: i32) -> Option<ObjectValue> {
        pool.iter().find(|o| o.id == id).cloned()
    }

    fn write_on(&self, stream: &mut CompressedWriteStream) {
        self.write_on_with_pool(stream, &mut Vec::new());
    }

    fn write_on_with_pool(&self, stream: &mut CompressedWriteStream, pool: &mut Vec<ObjectValue>) {
        match self {
            Self::Location(l) => {
                stream.write_int(sv_code::LOCATION_CODE);
                l.write_on(stream);
            }
            Self::ConstantInt(v) => {
                stream.write_int(sv_code::CONSTANT_INT_CODE);
                stream.write_signed_int(*v);
            }
            Self::ConstantFloat(v) => {
                stream.write_int(sv_code::CONSTANT_FLOAT_CODE);
                stream.write_int(*v);
            }
            Self::ConstantOop(s) => {
                stream.write_int(sv_code::CONSTANT_OOP_CODE);
                let bytes = s.as_bytes();
                stream.write_int(bytes.len() as u32);
                for &b in bytes {
                    stream.write(b);
                }
            }
            Self::ConstantLong(v) => {
                stream.write_int(sv_code::CONSTANT_LONG_CODE);
                stream.write_signed_int(*v as i32);
                stream.write_signed_int((*v >> 32) as i32);
            }
            Self::ConstantDouble(v) => {
                stream.write_int(sv_code::CONSTANT_DOUBLE_CODE);
                stream.write_int((*v >> 32) as u32);
                stream.write_int(*v as u32);
            }
            Self::Object(o) => {
                // HotSpot ObjectValue::write_on: a second write of the
                // same object id emits OBJECT_ID_CODE instead of the full
                // record.
                if pool.iter().any(|p| p.id == o.id) {
                    stream.write_int(sv_code::OBJECT_ID_CODE);
                    stream.write_signed_int(o.id);
                } else {
                    pool.push(o.clone());
                    let code = if o.is_auto_box {
                        sv_code::AUTO_BOX_OBJECT_CODE
                    } else {
                        sv_code::OBJECT_CODE
                    };
                    Self::write_object_value(stream, o, code, pool);
                }
            }
            Self::ObjectId(id) => {
                stream.write_int(sv_code::OBJECT_ID_CODE);
                stream.write_signed_int(*id);
            }
            Self::ObjectMerge(m) => {
                stream.write_int(sv_code::OBJECT_MERGE_CODE);
                stream.write_signed_int(m.id);
                match &m.selector {
                    Some(s) => {
                        stream.write_bool(true);
                        s.write_on_with_pool(stream, pool);
                    }
                    None => stream.write_bool(false),
                }
                match &m.merge_pointer {
                    Some(s) => {
                        stream.write_bool(true);
                        s.write_on_with_pool(stream, pool);
                    }
                    None => stream.write_bool(false),
                }
                stream.write_int(m.possible_objects.len() as u32);
                for o in &m.possible_objects {
                    // Candidates go through the regular write path so
                    // visited objects collapse to OBJECT_ID_CODE.
                    Self::Object(o.clone()).write_on_with_pool(stream, pool);
                }
            }
            Self::Marker => {
                stream.write_int(sv_code::MARKER_CODE);
            }
        }
    }

    /// `ScopeValue::read_from` analogue.  When `pool` is `Some`, object
    /// values are registered and OBJECT_ID_CODE references resolve to the
    /// cached `Object`; otherwise OBJECT_ID_CODE decodes to `ObjectId`.
    fn read_from(
        stream: &mut CompressedReadStream,
        mut pool: Option<&mut Vec<ObjectValue>>,
    ) -> Option<Self> {
        let code = stream.read_int()?;
        match code {
            sv_code::LOCATION_CODE => Some(Self::Location(Location::read_from(stream)?)),
            sv_code::CONSTANT_INT_CODE => Some(Self::ConstantInt(stream.read_signed_int()?)),
            sv_code::CONSTANT_FLOAT_CODE => Some(Self::ConstantFloat(stream.read_int()?)),
            sv_code::CONSTANT_OOP_CODE => {
                let len = stream.read_int()? as usize;
                let mut bytes = Vec::with_capacity(len);
                for _ in 0..len {
                    bytes.push(stream.read()?);
                }
                Some(Self::ConstantOop(
                    String::from_utf8_lossy(&bytes).into_owned(),
                ))
            }
            sv_code::CONSTANT_LONG_CODE => {
                let lo = stream.read_signed_int()? as i64;
                let hi = stream.read_signed_int()? as i64;
                Some(Self::ConstantLong(
                    (hi << 32) | (lo as u64 & 0xFFFF_FFFF) as i64,
                ))
            }
            sv_code::CONSTANT_DOUBLE_CODE => {
                let hi = stream.read_int()? as u64;
                let lo = stream.read_int()? as u64;
                Some(Self::ConstantDouble((hi << 32) | lo))
            }
            sv_code::OBJECT_CODE | sv_code::AUTO_BOX_OBJECT_CODE => {
                // Use the caller's pool when present so OBJECT_ID_CODE
                // references resolve; otherwise decode into a scratch
                // pool (ids then stay unresolved ObjectId references).
                let mut owned;
                let pool_ref: &mut Vec<ObjectValue> = match pool.as_deref_mut() {
                    Some(p) => p,
                    None => {
                        owned = Vec::new();
                        &mut owned
                    }
                };
                let obj = Self::read_object_value(
                    stream,
                    code == sv_code::AUTO_BOX_OBJECT_CODE,
                    pool_ref,
                )?;
                Some(Self::Object(obj))
            }
            sv_code::OBJECT_ID_CODE => {
                let id = stream.read_signed_int()?;
                match pool.as_deref() {
                    Some(p) => match Self::get_cached_object(p, id) {
                        Some(o) => Some(Self::Object(o)),
                        None => Some(Self::ObjectId(id)),
                    },
                    None => Some(Self::ObjectId(id)),
                }
            }
            sv_code::OBJECT_MERGE_CODE => {
                let id = stream.read_signed_int()?;
                let mut merge = ObjectMergeValue::new(id);
                if stream.read_bool()? {
                    merge.selector = Some(Box::new(Self::read_from(stream, pool.as_deref_mut())?));
                }
                if stream.read_bool()? {
                    merge.merge_pointer =
                        Some(Box::new(Self::read_from(stream, pool.as_deref_mut())?));
                }
                let count = stream.read_int()? as usize;
                for _ in 0..count {
                    // Possible objects are full ObjectValues written via
                    // the regular path (OBJECT_CODE / OBJECT_ID_CODE).
                    let obj = Self::read_from(stream, pool.as_deref_mut())?;
                    match obj {
                        ScopeValue::Object(o) => merge.add_possible_object(o),
                        ScopeValue::ObjectId(i) => {
                            // Reference to an already-seen object: keep a
                            // marker candidate with that id.
                            let mut o = ObjectValue::new(i);
                            o.set_is_scalar_replaced(false);
                            merge.add_possible_object(o);
                        }
                        _ => return None,
                    }
                }
                Some(Self::ObjectMerge(merge))
            }
            sv_code::MARKER_CODE => Some(Self::Marker),
            _ => None,
        }
    }

    /// Structural equality ignoring pool-resolution differences between
    /// `ObjectId` and resolved `Object` (HotSpot `equals` analogue is
    /// strict; this is a debug helper).
    pub fn resolved_equals(&self, other: &ScopeValue) -> bool {
        match (self, other) {
            (Self::ObjectId(a), Self::Object(o)) | (Self::Object(o), Self::ObjectId(a)) => {
                o.id() == *a
            }
            _ => self == other,
        }
    }
}

/// A locked monitor recorded in a scope (MonitorValue analogue).  Wire
// order follows HotSpot: basic_lock Location, owner ScopeValue,
// eliminated bool.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorValue {
    pub owner: ScopeValue,
    pub basic_lock: Location,
    pub eliminated: bool,
}

impl MonitorValue {
    pub fn new(owner: ScopeValue, basic_lock: Location) -> Self {
        Self {
            owner,
            basic_lock,
            eliminated: false,
        }
    }

    pub fn eliminated(owner: ScopeValue, basic_lock: Location) -> Self {
        Self {
            owner,
            basic_lock,
            eliminated: true,
        }
    }

    fn write_on(&self, stream: &mut CompressedWriteStream) {
        self.basic_lock.write_on(stream);
        self.owner.write_on(stream);
        stream.write_bool(self.eliminated);
    }

    fn read_from(stream: &mut CompressedReadStream) -> Option<Self> {
        let basic_lock = Location::read_from(stream)?;
        let owner = ScopeValue::read_from(stream, None)?;
        let eliminated = stream.read_bool()?;
        Some(Self {
            owner,
            basic_lock,
            eliminated,
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
    objects_decode_offset: i32,
    locals: Vec<ScopeValue>,
    expressions: Vec<ScopeValue>,
    monitors: Vec<MonitorValue>,
    objects: Vec<ObjectValue>,
    // Flags normally taken from the enclosing PcDesc (HotSpot stores them
    // there, not in the scope record); settable via apply_pc_desc_flags.
    reexecute: bool,
    return_oop: bool,
    rethrow_exception: bool,
    has_ea_local_in_scope: bool,
    arg_escape: bool,
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
            objects_decode_offset: SERIALIZED_NULL,
            locals: Vec::new(),
            expressions: Vec::new(),
            monitors: Vec::new(),
            objects: Vec::new(),
            reexecute: false,
            return_oop: false,
            rethrow_exception: false,
            has_ea_local_in_scope: false,
            arg_escape: false,
        }
    }

    /// Sentinel scope decoded for serialized_null / missing decode offset
    /// (decode_body analogue): represents the nmethod's own method at
    /// InvocationEntryBci with empty bodies.
    pub fn sentinel(method_holder: String, method_name: String, method_signature: String) -> Self {
        Self::new(
            method_holder,
            method_name,
            method_signature,
            INVOCATION_ENTRY_BCI,
            false,
        )
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
            objects_decode_offset: SERIALIZED_NULL,
            locals,
            expressions,
            monitors,
            objects: Vec::new(),
            reexecute: false,
            return_oop: false,
            rethrow_exception: false,
            has_ea_local_in_scope: false,
            arg_escape: false,
        }
    }

    /// Apply the PcDesc flags that HotSpot reads from the pc descriptor
    /// (reexecute / return_oop / rethrow / has_ea_local / arg_escape).
    pub fn apply_pc_desc_flags(
        &mut self,
        reexecute: bool,
        return_oop: bool,
        rethrow_exception: bool,
        has_ea_local_in_scope: bool,
        arg_escape: bool,
    ) {
        self.reexecute = reexecute;
        self.return_oop = return_oop;
        self.rethrow_exception = rethrow_exception;
        self.has_ea_local_in_scope = has_ea_local_in_scope;
        self.arg_escape = arg_escape;
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

    /// `ScopeDesc::method()` analogue: formatted method identity.
    pub fn method_string(&self) -> String {
        format!(
            "{}.{}{}",
            self.method_holder, self.method_name, self.method_signature
        )
    }

    #[inline]
    pub fn bci(&self) -> i32 {
        self.bci
    }

    #[inline]
    pub fn set_bci(&mut self, bci: i32) {
        self.bci = bci;
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
    pub fn locals_decode_offset(&self) -> i32 {
        self.locals_decode_offset
    }

    #[inline]
    pub fn expressions_decode_offset(&self) -> i32 {
        self.expressions_decode_offset
    }

    #[inline]
    pub fn monitors_decode_offset(&self) -> i32 {
        self.monitors_decode_offset
    }

    #[inline]
    pub fn objects_decode_offset(&self) -> i32 {
        self.objects_decode_offset
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

    /// All object values decoded for this scope (root and nested).
    #[inline]
    pub fn objects(&self) -> &[ObjectValue] {
        &self.objects
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

    #[inline]
    pub fn has_ea_local_in_scope(&self) -> bool {
        self.has_ea_local_in_scope
    }

    #[inline]
    pub fn arg_escape(&self) -> bool {
        self.arg_escape
    }

    /// `ScopeDesc::objects_to_rematerialize` analogue: the subset of
    /// decoded objects actually referenced by locals, expressions or
    /// monitors (the "roots" of the rematerialization graph).  Object
    /// ids referenced through OBJECT_ID_CODE are resolved against the
    /// scope's object pool.
    pub fn objects_to_rematerialize(&self) -> Vec<ObjectValue> {
        fn collect_ids(v: &ScopeValue, ids: &mut Vec<i32>) {
            match v {
                ScopeValue::Object(o) => ids.push(o.id()),
                ScopeValue::ObjectId(id) => ids.push(*id),
                ScopeValue::ObjectMerge(m) => {
                    ids.push(m.id());
                    for o in m.possible_objects() {
                        ids.push(o.id());
                    }
                }
                _ => {}
            }
        }
        let mut ids = Vec::new();
        for v in &self.locals {
            collect_ids(v, &mut ids);
        }
        for v in &self.expressions {
            collect_ids(v, &mut ids);
        }
        for m in &self.monitors {
            collect_ids(&m.owner, &mut ids);
        }
        let mut out = Vec::new();
        for id in ids {
            if let Some(o) = self.objects.iter().find(|o| o.id() == id) {
                if o.is_root() && !out.iter().any(|q: &ObjectValue| q.id() == o.id()) {
                    out.push(o.clone());
                }
            }
        }
        out
    }

    /// Structural verification of the decoded scope (debug builds only in
    /// HotSpot; here always available).  Returns a list of problems.
    pub fn verify(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.bci < INVOCATION_ENTRY_BCI {
            problems.push(format!("invalid bci {}", self.bci));
        }
        // Object ids referenced by locals/expressions/monitors must exist
        // in the object pool (or be resolvable constants).
        for (i, v) in self.locals.iter().enumerate() {
            self.verify_value_refs(v, &format!("local[{}]", i), &mut problems);
        }
        for (i, v) in self.expressions.iter().enumerate() {
            self.verify_value_refs(v, &format!("expression[{}]", i), &mut problems);
        }
        for (i, m) in self.monitors.iter().enumerate() {
            self.verify_value_refs(&m.owner, &format!("monitor[{}].owner", i), &mut problems);
            if m.basic_lock.type_().is_none() {
                problems.push(format!("monitor[{}] has invalid basic_lock", i));
            }
        }
        for o in &self.objects {
            for (i, f) in o.field_values().iter().enumerate() {
                self.verify_value_refs(
                    f,
                    &format!("object[{}].field[{}]", o.id(), i),
                    &mut problems,
                );
            }
        }
        problems
    }

    fn verify_value_refs(&self, v: &ScopeValue, ctx: &str, problems: &mut Vec<String>) {
        match v {
            ScopeValue::ObjectId(id) => {
                if !self.objects.iter().any(|o| o.id() == *id) {
                    problems.push(format!("{}: unresolved object id {}", ctx, id));
                }
            }
            ScopeValue::Object(o) => {
                for (i, f) in o.field_values().iter().enumerate() {
                    self.verify_value_refs(f, &format!("{}.field[{}]", ctx, i), problems);
                }
            }
            ScopeValue::ObjectMerge(m) => {
                for (i, o) in m.possible_objects().iter().enumerate() {
                    for (j, f) in o.field_values().iter().enumerate() {
                        self.verify_value_refs(
                            f,
                            &format!("{}.merge[{}].field[{}]", ctx, i, j),
                            problems,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// `ScopeDesc::print_on` analogue (text dump).
    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        let _ = writeln!(
            f,
            "scope {} decode_offset={} bci={}",
            self.method_string(),
            self.decode_offset,
            self.bci
        );
        if self.is_inlined {
            let _ = writeln!(
                f,
                "  inlined, sender_decode_offset={}",
                self.sender_decode_offset
            );
        }
        if self.reexecute {
            let _ = writeln!(f, "  reexecute");
        }
        if self.return_oop {
            let _ = writeln!(f, "  return_oop");
        }
        if self.rethrow_exception {
            let _ = writeln!(f, "  rethrow_exception");
        }
        if self.has_ea_local_in_scope {
            let _ = writeln!(f, "  has_ea_local_in_scope");
        }
        if self.arg_escape {
            let _ = writeln!(f, "  arg_escape");
        }
        ScopeValue::print_values(f, "locals", &self.locals);
        ScopeValue::print_values(f, "expressions", &self.expressions);
        if !self.monitors.is_empty() {
            let _ = writeln!(f, "  monitors:");
            for m in &self.monitors {
                let _ = writeln!(f, "    monitor{{ owner=",);
                m.owner.print_on(f);
                let _ = writeln!(
                    f,
                    ", lock={} }}{}",
                    m.basic_lock.stack_offset(),
                    if m.eliminated { " (eliminated)" } else { "" }
                );
            }
        }
        if !self.objects.is_empty() {
            let _ = writeln!(f, "  objects: {}", self.objects.len());
        }
    }
}

impl ScopeValue {
    fn print_values(f: &mut dyn std::fmt::Write, label: &str, values: &[ScopeValue]) {
        if values.is_empty() {
            return;
        }
        let _ = writeln!(f, "  {}:", label);
        for (i, v) in values.iter().enumerate() {
            let _ = write!(f, "    [{}] ", i);
            v.print_on(f);
            let _ = writeln!(f);
        }
    }

    /// `ScopeValue::print_on` analogue.
    pub fn print_on(&self, f: &mut dyn std::fmt::Write) {
        match self {
            Self::Location(l) => {
                let _ = write!(f, "location{{");
                match l.where_() {
                    LocationWhere::OnStack => {
                        let _ = write!(f, "stack[{}]", l.stack_offset());
                    }
                    LocationWhere::InRegister => {
                        let _ = write!(f, "reg{}", l.register_number());
                    }
                }
                if let Some(t) = l.type_() {
                    let _ = write!(f, ",{:?}", t);
                }
                let _ = write!(f, "}}");
            }
            Self::ConstantInt(v) => {
                let _ = write!(f, "int {}", v);
            }
            Self::ConstantFloat(v) => {
                let _ = write!(f, "float {:#x}", v);
            }
            Self::ConstantOop(s) => {
                let _ = write!(f, "oop {}", s);
            }
            Self::ConstantLong(v) => {
                let _ = write!(f, "long {}", v);
            }
            Self::ConstantDouble(v) => {
                let _ = write!(f, "double {:#x}", v);
            }
            Self::Object(o) => {
                let _ = write!(f, "object#{}", o.id);
                if o.is_auto_box {
                    let _ = write!(f, "(auto_box)");
                }
                if !o.is_scalar_replaced {
                    let _ = write!(f, "(not_scalar_replaced)");
                }
                if !o.is_root {
                    let _ = write!(f, "(not_root)");
                }
            }
            Self::ObjectId(id) => {
                let _ = write!(f, "object_id#{}", id);
            }
            Self::ObjectMerge(m) => {
                let _ = write!(f, "object_merge#{}", m.id);
            }
            Self::Marker => {
                let _ = write!(f, "marker");
            }
        }
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
    /// Object ids registered while writing resolve through a shared pool.
    pub fn write_scope_values(&mut self, values: &[ScopeValue]) -> i32 {
        let offset = self.stream.position() as i32;
        let mut pool = Vec::new();
        self.stream.write_int(values.len() as u32);
        for v in values {
            v.write_on_with_pool(&mut self.stream, &mut pool);
        }
        offset
    }

    /// Encode object values (`decode_object_values` analogue); returns
    /// the decode offset.  One pool is shared across the whole list so
    /// OBJECT_ID_CODE references resolve and repeated objects collapse
    /// (HotSpot is_visited semantics).
    pub fn write_object_values(&mut self, objects: &[ObjectValue]) -> i32 {
        let offset = self.stream.position() as i32;
        let mut pool = Vec::new();
        self.stream.write_int(objects.len() as u32);
        for o in objects {
            ScopeValue::Object(o.clone()).write_on_with_pool(&mut self.stream, &mut pool);
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
    /// monitors/objects), matching the HotSpot layout where decode_body
    /// reads the header then lazily decodes bodies at their offsets.
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
        objects_decode_offset: i32,
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
        self.stream.write_signed_int(objects_decode_offset);
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
    /// `decode_monitor_values` / `decode_object_values`.
    ///
    /// A decode_offset of serialized_null (0) yields the sentinel scope
    /// only when the caller supplies the fallback method identity via
    /// `decode_body_with_fallback`.
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
        let objs_off = self.stream.read_signed_int()?;

        let mut desc = ScopeDesc::new(
            holder,
            name,
            sig,
            bci,
            sender != NO_SENDER_DECODE_OFFSET && sender != SERIALIZED_NULL,
        );
        desc.sender_decode_offset = sender;
        desc.decode_offset = decode_offset;
        desc.locals_decode_offset = locals_off;
        desc.expressions_decode_offset = exprs_off;
        desc.monitors_decode_offset = mons_off;
        desc.objects_decode_offset = objs_off;
        Some(desc)
    }

    /// `ScopeDesc::decode_body` analogue including the serialized_null
    /// sentinel case: `fallback` supplies the nmethod's own method
    /// identity used for the sentinel scope.  Only an actual
    /// serialized_null position (0) produces the sentinel — a malformed
    /// non-zero record yields None (HotSpot asserts in this case).
    pub fn decode_body(&mut self, fallback: Option<(&str, &str, &str)>) -> Option<ScopeDesc> {
        if self.stream.pos as i32 == SERIALIZED_NULL {
            return fallback
                .map(|(h, n, s)| ScopeDesc::sentinel(h.to_string(), n.to_string(), s.to_string()));
        }
        self.decode_scope_header()
    }

    /// Decode a scope-value list at `decode_offset`
    /// (`ScopeDesc::decode_scope_values` analogue).
    pub fn decode_scope_values(&mut self, decode_offset: i32) -> Option<Vec<ScopeValue>> {
        if decode_offset == SERIALIZED_NULL {
            return None;
        }
        self.stream.pos = decode_offset as usize;
        let length = self.stream.read_int()? as usize;
        let mut pool = Vec::new();
        let mut result = Vec::with_capacity(length);
        for _ in 0..length {
            result.push(ScopeValue::read_from(&mut self.stream, Some(&mut pool))?);
        }
        Some(result)
    }

    /// Decode object values at `decode_offset`
    /// (`ScopeDesc::decode_object_values` analogue).  Object ids inside
    /// the list resolve through the shared object pool; the returned
    /// values keep `Object`/`ObjectId` forms as decoded.
    pub fn decode_object_values(&mut self, decode_offset: i32) -> Option<Vec<ObjectValue>> {
        if decode_offset == SERIALIZED_NULL {
            return None;
        }
        self.stream.pos = decode_offset as usize;
        let length = self.stream.read_int()? as usize;
        // One pool shared across the whole list and returned as the
        // result: HotSpot passes the `result` GrowableArray itself as the
        // object pool, so nested field objects appear in objects() too,
        // in decode order (placeholder pushed before fields are read).
        let mut pool: Vec<ObjectValue> = Vec::with_capacity(length);
        for _ in 0..length {
            // Each entry is a full ObjectValue (OBJECT_CODE form).
            let sv = ScopeValue::read_from(&mut self.stream, Some(&mut pool))?;
            match sv {
                ScopeValue::Object(_) | ScopeValue::ObjectId(_) => {}
                _ => return None,
            }
        }
        Some(pool)
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

    /// Full decode: header + bodies, exactly like `ScopeDesc::decode_body`
    /// plus lazy body decoding.  `obj_decode_offset` is the PcDesc's
    /// object decode offset (0 when absent).
    pub fn decode_full(&mut self, obj_decode_offset: i32) -> Option<ScopeDesc> {
        let mut desc = self.decode_scope_header()?;
        let data_len_guard = self.stream.data.len();
        desc.locals = self
            .decode_scope_values(desc.locals_decode_offset)
            .unwrap_or_default();
        desc.expressions = self
            .decode_scope_values(desc.expressions_decode_offset)
            .unwrap_or_default();
        desc.monitors = self
            .decode_monitor_values(desc.monitors_decode_offset)
            .unwrap_or_default();
        desc.objects = self
            .decode_object_values(obj_decode_offset)
            .or_else(|| {
                if desc.objects_decode_offset != SERIALIZED_NULL {
                    self.decode_object_values(desc.objects_decode_offset)
                } else {
                    None
                }
            })
            .unwrap_or_default();
        // Keep the stream bounded to the blob.
        if self.stream.pos > data_len_guard {
            self.stream.pos = data_len_guard;
        }
        Some(desc)
    }

    /// Decode the sender scope (`ScopeDesc::sender()` analogue).
    pub fn decode_sender(
        &mut self,
        parent: &ScopeDesc,
        obj_decode_offset: i32,
    ) -> Option<ScopeDesc> {
        if parent.sender_decode_offset == NO_SENDER_DECODE_OFFSET
            || parent.sender_decode_offset == SERIALIZED_NULL
        {
            return None;
        }
        let mut reader = ScopeDescReader::at(self.stream.data, parent.sender_decode_offset);
        let mut desc = reader.decode_full(obj_decode_offset)?;
        // Non-top scopes never carry the reexecute flag (HotSpot clears
        // it for inlined frames).
        desc.reexecute = false;
        Some(desc)
    }
}

/// Convenience: decode the complete inlined-scope chain for a pc, walking
/// sender decode offsets like `ScopeDesc::sender()`.
pub fn decode_scope_chain(data: &[u8], decode_offset: i32) -> Vec<ScopeDesc> {
    decode_scope_chain_with_objects(data, decode_offset, SERIALIZED_NULL)
}

/// `decode_scope_chain` with an explicit PcDesc object decode offset.
pub fn decode_scope_chain_with_objects(
    data: &[u8],
    decode_offset: i32,
    obj_decode_offset: i32,
) -> Vec<ScopeDesc> {
    let mut chain = Vec::new();
    let mut offset = decode_offset;
    let mut depth = 0;
    // Real decode offsets are >= 1; 0 (serialized_null) and -1
    // (no sender) terminate the chain.
    while offset > 0 && depth < 256 {
        let mut reader = ScopeDescReader::at(data, offset);
        match reader.decode_full(obj_decode_offset) {
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
pub fn encode_scope_desc(desc: &ScopeDesc) -> (Vec<u8>, i32) {
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
    let objs: Vec<ObjectValue> = desc
        .objects
        .iter()
        .cloned()
        .chain(
            desc.objects_to_rematerialize()
                .into_iter()
                .filter(|o| !desc.objects.iter().any(|p| p.id() == o.id())),
        )
        .collect();
    let objs_off = if objs.is_empty() {
        SERIALIZED_NULL
    } else {
        w.write_object_values(&objs)
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
        objs_off,
    );
    (w.finish(), top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_bit_packing() {
        let l = Location::new_stk_loc(LocationType::Oop, 64);
        assert!(l.is_stack());
        assert_eq!(l.type_(), Some(LocationType::Oop));
        assert_eq!(l.stack_offset(), 64);
        let r = Location::new_reg_loc(LocationType::Lng, 5);
        assert!(r.is_register());
        assert_eq!(r.register_number(), 5);
        assert!(r.is_oop_location() == false);
        let narrow = Location::new_stk_loc(LocationType::NarrowOop, 8);
        assert!(narrow.is_oop_location());
        // Default location is on_stack/invalid/0.
        assert_eq!(Location::default().type_(), Some(LocationType::Invalid));
    }

    #[test]
    fn scope_value_round_trip() {
        let values = vec![
            ScopeValue::ConstantInt(-7),
            ScopeValue::ConstantLong(0x1234_5678_9ABC_DEF0u64 as i64),
            ScopeValue::ConstantFloat(1.25f32.to_bits()),
            ScopeValue::ConstantDouble((-3.5f64).to_bits()),
            ScopeValue::ConstantOop("java/lang/Object".to_string()),
            ScopeValue::Location(Location::new_stk_loc(LocationType::Normal, 16)),
            ScopeValue::Location(Location::new_reg_loc(LocationType::Dbl, 3)),
        ];
        let mut w = ScopeDescWriter::new();
        let off = w.write_scope_values(&values);
        let data = w.finish();
        let mut r = ScopeDescReader::new(&data);
        let decoded = r.decode_scope_values(off).unwrap();
        assert_eq!(decoded.len(), values.len());
        for (a, b) in decoded.iter().zip(values.iter()) {
            assert!(a.resolved_equals(b), "{:?} != {:?}", a, b);
        }
    }

    #[test]
    fn object_value_round_trip_with_id_ref() {
        let mut obj = ObjectValue::new(1);
        obj.set_klass(ScopeValue::ConstantOop("pkg/Klass".to_string()));
        obj.set_field_values(vec![
            ScopeValue::ConstantInt(42),
            ScopeValue::Location(Location::new_reg_loc(LocationType::Oop, 2)),
        ]);
        obj.set_root(true);
        // An OBJECT_ID_CODE reference to object 1 inside locals.
        let values = vec![ScopeValue::Object(obj.clone()), ScopeValue::ObjectId(1)];
        let mut w = ScopeDescWriter::new();
        let off = w.write_scope_values(&values);
        let data = w.finish();
        let mut r = ScopeDescReader::new(&data);
        let decoded = r.decode_scope_values(off).unwrap();
        assert_eq!(decoded.len(), 2);
        match &decoded[1] {
            ScopeValue::Object(o) => assert_eq!(o.id(), 1),
            other => panic!("expected resolved object, got {:?}", other),
        }
    }

    #[test]
    fn object_merge_round_trip_and_select() {
        let mut m = ObjectMergeValue::new(7);
        m.set_selector(ScopeValue::ConstantInt(0));
        m.set_merge_pointer(ScopeValue::Location(Location::new_stk_loc(
            LocationType::Oop,
            32,
        )));
        let mut o0 = ObjectValue::new(8);
        o0.set_is_scalar_replaced(true);
        o0.set_field_values(vec![ScopeValue::ConstantInt(1)]);
        let mut o1 = ObjectValue::new(9);
        o1.set_is_scalar_replaced(true);
        o1.set_field_values(vec![ScopeValue::ConstantInt(2)]);
        m.add_possible_object(o0);
        m.add_possible_object(o1);
        let values = vec![ScopeValue::ObjectMerge(m)];
        let mut w = ScopeDescWriter::new();
        let off = w.write_scope_values(&values);
        let data = w.finish();
        let mut r = ScopeDescReader::new(&data);
        let decoded = r.decode_scope_values(off).unwrap();
        let mut merge = match &decoded[0] {
            ScopeValue::ObjectMerge(m) => m.clone(),
            other => panic!("expected merge, got {:?}", other),
        };
        assert_eq!(merge.possible_objects().len(), 2);
        assert_eq!(merge.select(), Some(0));
    }

    #[test]
    fn monitor_round_trip_hotspot_order() {
        let mon = MonitorValue::eliminated(
            ScopeValue::ConstantOop("monitor".to_string()),
            Location::new_stk_loc(LocationType::Normal, 16),
        );
        let mut w = ScopeDescWriter::new();
        let off = w.write_monitors(&[mon.clone()]);
        let data = w.finish();
        let mut r = ScopeDescReader::new(&data);
        let decoded = r.decode_monitor_values(off).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].basic_lock.stack_offset(), 16);
        assert!(decoded[0].eliminated);
    }

    #[test]
    fn encode_decode_scope_desc_round_trip() {
        let mut desc = ScopeDesc::with_details(
            "pkg/Klass".to_string(),
            "method".to_string(),
            "(I)J".to_string(),
            42,
            false,
            NO_SENDER_DECODE_OFFSET,
            vec![
                ScopeValue::ConstantInt(1),
                ScopeValue::Location(Location::new_reg_loc(LocationType::Oop, 3)),
            ],
            vec![ScopeValue::ConstantLong(99)],
            vec![MonitorValue::new(
                ScopeValue::Location(Location::new_stk_loc(LocationType::Oop, 8)),
                Location::new_stk_loc(LocationType::Normal, 0),
            )],
        );
        desc.apply_pc_desc_flags(true, false, true, false, true);
        let (data, top) = encode_scope_desc(&desc);
        let chain = decode_scope_chain(&data, top);
        assert_eq!(chain.len(), 1);
        let d = &chain[0];
        assert_eq!(d.method_holder(), "pkg/Klass");
        assert_eq!(d.method_name(), "method");
        assert_eq!(d.method_signature(), "(I)J");
        assert_eq!(d.bci(), 42);
        assert_eq!(d.locals().len(), 2);
        assert_eq!(d.expressions().len(), 1);
        assert_eq!(d.monitors().len(), 1);
        // Flags are PcDesc data in HotSpot; a fresh decode leaves them
        // unset unless apply_pc_desc_flags is used.
        assert!(!d.should_reexecute());
        assert!(!d.rethrow_exception());
        assert!(!d.return_oop());
        assert!(d.is_top());
        assert!(d.verify().is_empty());
        // print_on smoke test
        let mut s = String::new();
        d.print_on(&mut s);
        assert!(s.contains("pkg/Klass.method(I)J"));
    }

    #[test]
    fn inline_chain_decode_order() {
        let mut w = ScopeDescWriter::new();
        // Inner (callee) scope first, then outer with sender -> inner.
        let inner_locals = w.write_scope_values(&[ScopeValue::ConstantInt(42)]);
        let inner = w.write_scope(
            NO_SENDER_DECODE_OFFSET,
            "Inner",
            "in",
            "()V",
            5,
            inner_locals,
            SERIALIZED_NULL,
            SERIALIZED_NULL,
            SERIALIZED_NULL,
        );
        let outer_locals = w.write_scope_values(&[ScopeValue::ConstantOop("this".to_string())]);
        let outer = w.write_scope(
            inner,
            "Outer",
            "out",
            "()V",
            10,
            outer_locals,
            SERIALIZED_NULL,
            SERIALIZED_NULL,
            SERIALIZED_NULL,
        );
        let data = w.finish();
        let chain = decode_scope_chain(&data, outer);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].method_name(), "out");
        assert_eq!(chain[1].method_name(), "in");
        assert!(chain[1].is_top());
    }

    #[test]
    fn objects_to_rematerialize_filters_roots() {
        let mut root = ObjectValue::new(1);
        root.set_root(true);
        root.set_field_values(vec![ScopeValue::ObjectId(2)]);
        let mut nested = ObjectValue::new(2);
        nested.set_root(false);
        nested.set_field_values(vec![ScopeValue::ConstantInt(7)]);
        let desc = ScopeDesc::with_details(
            "K".to_string(),
            "m".to_string(),
            "()V".to_string(),
            3,
            false,
            NO_SENDER_DECODE_OFFSET,
            vec![
                ScopeValue::Object(root.clone()),
                ScopeValue::Object(nested.clone()),
            ],
            vec![],
            vec![],
        );
        let mut desc = desc;
        desc.objects = vec![root.clone(), nested.clone()];
        let roots = desc.objects_to_rematerialize();
        // Root object 1 referenced by locals; object 2 is only reachable
        // as a field of object 1 (not a JVM-visible root).
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id(), 1);
        assert!(desc.verify().is_empty());
    }

    #[test]
    fn verify_detects_dangling_object_id() {
        let desc = ScopeDesc::with_details(
            "K".to_string(),
            "m".to_string(),
            "()V".to_string(),
            3,
            false,
            NO_SENDER_DECODE_OFFSET,
            vec![ScopeValue::ObjectId(99)],
            vec![],
            vec![],
        );
        let problems = desc.verify();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("unresolved object id 99"));
    }

    #[test]
    fn sentinel_scope_decode() {
        // A blob whose first byte is 0 (serialized_null) decodes to the
        // sentinel scope at InvocationEntryBci.
        let w = ScopeDescWriter::new();
        // Only the reserved byte 0 was written; no records follow.
        let data = w.finish();
        let mut r = ScopeDescReader::new(&data);
        // Force position to 0 and attempt header decode -> fails -> sentinel.
        r.stream.pos = 0;
        let d = r
            .decode_body(Some(("Holder", "main", "()V")))
            .expect("sentinel");
        assert_eq!(d.bci(), INVOCATION_ENTRY_BCI);
        assert_eq!(d.method_holder(), "Holder");
        assert!(d.is_top());
    }
}
