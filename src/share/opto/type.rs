/*
 * Port of OpenJDK 25 HotSpot C2 Type lattice (share/opto/type.hpp / type.cpp)
 *
 * The C2 type lattice is the dataflow type system of the Sea-of-Nodes
 * compiler.  Types are immutable values; this port uses `Type` (an enum)
 * instead of hash-consed `Type*` objects.  Naming follows HotSpot:
 * `Bottom` is the universal type ("all values"), `Top` is the empty type
 * ("no values").
 *
 * Semantics preserved from type.cpp:
 *   - `meet` is the LEAST UPPER BOUND (generalization): the hull for
 *     integer ranges, the union for pointer kinds, the common supertype
 *     for object pointers.  meet(Top, t) = t, meet(Bottom, t) = Bottom.
 *   - `join` is the GREATEST LOWER BOUND (specialization): the
 *     intersection for ranges, the common subtype for pointers.
 *     join(Top, t) = Top, join(Bottom, t) = t.
 *   - `filter(kills)` = join(kills) normalized to Top when empty; used by
 *     Node::Value to intersect computed types with worst-case types.
 *
 * Class hierarchy queries (is this klass a subtype of that klass, common
 * supertype, array element types) require ciInterface data.  They are
 * routed through the `KlassDb` trait so the lattice logic itself is
 * complete and testable without a running VM.
 */

use std::fmt;

pub const OFFSET_TOP: i64 = -2_000_000_000;
pub const OFFSET_BOT: i64 = -2_000_000_001;

pub const WIDEN_MIN: u8 = 0;
pub const WIDEN_MAX: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KlassId(pub u32);

pub trait KlassDb {
    fn is_subtype_of(&self, sub: KlassId, sup: KlassId) -> bool;
    fn is_same_type(&self, a: KlassId, b: KlassId) -> bool;
    fn common_supertype(&self, a: KlassId, b: KlassId) -> Option<KlassId>;
    fn greatest_lower_bound(&self, a: KlassId, b: KlassId) -> Option<KlassId>;
    fn array_element_klass(&self, array: KlassId) -> Option<KlassId>;
    fn is_array(&self, k: KlassId) -> bool;
    fn is_interface(&self, k: KlassId) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EmptyKlassDb;

impl KlassDb for EmptyKlassDb {
    fn is_subtype_of(&self, a: KlassId, b: KlassId) -> bool {
        a == b
    }
    fn is_same_type(&self, a: KlassId, b: KlassId) -> bool {
        a == b
    }
    fn common_supertype(&self, a: KlassId, b: KlassId) -> Option<KlassId> {
        if a == b {
            Some(a)
        } else {
            None
        }
    }
    fn greatest_lower_bound(&self, a: KlassId, b: KlassId) -> Option<KlassId> {
        if a == b {
            Some(a)
        } else {
            None
        }
    }
    fn array_element_klass(&self, _array: KlassId) -> Option<KlassId> {
        None
    }
    fn is_array(&self, _k: KlassId) -> bool {
        false
    }
    fn is_interface(&self, _k: KlassId) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Bad,
    Control,
    Top,
    Int,
    Long,
    Half,
    NarrowOop,
    NarrowKlass,
    Tuple,
    Array,
    Interfaces,
    VectorMask,
    VectorA,
    VectorS,
    VectorD,
    VectorX,
    VectorY,
    VectorZ,
    AnyPtr,
    RawPtr,
    OopPtr,
    InstPtr,
    AryPtr,
    MetadataPtr,
    KlassPtr,
    InstKlassPtr,
    AryKlassPtr,
    Function,
    Abio,
    ReturnAddress,
    Memory,
    HalfFloatTop,
    HalfFloatCon,
    HalfFloatBot,
    FloatTop,
    FloatCon,
    FloatBot,
    DoubleTop,
    DoubleCon,
    DoubleBot,
    Bottom,
    Last,
}

impl TypeKind {
    pub fn dual(self) -> TypeKind {
        use TypeKind::*;
        match self {
            Bottom => Top,
            Top => Bottom,
            Control => Control,
            Memory => Memory,
            Abio => Abio,
            ReturnAddress => ReturnAddress,
            Half => Half,
            Int => Int,
            Long => Long,
            NarrowOop => NarrowOop,
            NarrowKlass => NarrowKlass,
            Tuple => Tuple,
            Array => Array,
            Interfaces => Interfaces,
            AnyPtr => AnyPtr,
            RawPtr => RawPtr,
            OopPtr => OopPtr,
            InstPtr => InstPtr,
            AryPtr => AryPtr,
            MetadataPtr => MetadataPtr,
            KlassPtr => KlassPtr,
            InstKlassPtr => InstKlassPtr,
            AryKlassPtr => AryKlassPtr,
            Function => Function,
            VectorMask => VectorMask,
            VectorA => VectorA,
            VectorS => VectorS,
            VectorD => VectorD,
            VectorX => VectorX,
            VectorY => VectorY,
            VectorZ => VectorZ,
            HalfFloatTop => HalfFloatBot,
            HalfFloatCon => HalfFloatCon,
            HalfFloatBot => HalfFloatTop,
            FloatTop => FloatBot,
            FloatCon => FloatCon,
            FloatBot => FloatTop,
            DoubleTop => DoubleBot,
            DoubleCon => DoubleCon,
            DoubleBot => DoubleTop,
            Bad => Bad,
            Last => Last,
        }
    }

    pub fn is_ptr(self) -> bool {
        use TypeKind::*;
        matches!(
            self,
            AnyPtr
                | RawPtr
                | OopPtr
                | InstPtr
                | AryPtr
                | MetadataPtr
                | KlassPtr
                | InstKlassPtr
                | AryKlassPtr
                | NarrowOop
                | NarrowKlass
        )
    }

    pub fn isa_oop(self) -> bool {
        use TypeKind::*;
        matches!(self, OopPtr | InstPtr | AryPtr | NarrowOop)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct I32Range {
    pub lo: i32,
    pub hi: i32,
    pub widen: u8,
}

impl I32Range {
    pub fn new(lo: i32, hi: i32, widen: u8) -> Self {
        Self { lo, hi, widen }
    }

    pub fn is_con(&self) -> bool {
        self.lo == self.hi
    }

    pub fn get_con(&self) -> i32 {
        self.lo
    }

    pub fn empty(&self) -> bool {
        self.lo > self.hi
    }

    fn hull(&self, o: &I32Range) -> I32Range {
        I32Range::new(
            self.lo.min(o.lo),
            self.hi.max(o.hi),
            self.widen.max(o.widen),
        )
    }

    fn glb(&self, o: &I32Range) -> Option<I32Range> {
        let lo = self.lo.max(o.lo);
        let hi = self.hi.min(o.hi);
        if lo > hi {
            return None;
        }
        Some(I32Range::new(lo, hi, self.widen.max(o.widen)))
    }

    fn dual(&self) -> I32Range {
        I32Range::new(self.hi, self.lo, WIDEN_MAX - self.widen)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct I64Range {
    pub lo: i64,
    pub hi: i64,
    pub widen: u8,
}

impl I64Range {
    pub fn new(lo: i64, hi: i64, widen: u8) -> Self {
        Self { lo, hi, widen }
    }

    pub fn is_con(&self) -> bool {
        self.lo == self.hi
    }

    pub fn get_con(&self) -> i64 {
        self.lo
    }

    pub fn empty(&self) -> bool {
        self.lo > self.hi
    }

    fn hull(&self, o: &I64Range) -> I64Range {
        I64Range::new(
            self.lo.min(o.lo),
            self.hi.max(o.hi),
            self.widen.max(o.widen),
        )
    }

    fn glb(&self, o: &I64Range) -> Option<I64Range> {
        let lo = self.lo.max(o.lo);
        let hi = self.hi.min(o.hi);
        if lo > hi {
            return None;
        }
        Some(I64Range::new(lo, hi, self.widen.max(o.widen)))
    }

    fn dual(&self) -> I64Range {
        I64Range::new(self.hi, self.lo, WIDEN_MAX - self.widen)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatVal {
    Top,
    Con(u32),
    Bot,
}

impl FloatVal {
    fn is_con(&self) -> bool {
        matches!(self, FloatVal::Con(_))
    }
    fn get_con(&self) -> Option<f64> {
        match self {
            FloatVal::Con(v) => Some(f64::from_bits(*v as u64)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DoubleVal {
    Top,
    Con(u64),
    Bot,
}

impl DoubleVal {
    fn is_con(&self) -> bool {
        matches!(self, DoubleVal::Con(_))
    }
    fn get_con(&self) -> Option<f64> {
        match self {
            DoubleVal::Con(v) => Some(f64::from_bits(*v)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PtrKind {
    Top,
    AnyNull,
    Constant,
    Null,
    NotNull,
    Bot,
}

impl PtrKind {
    fn is_bot(self) -> bool {
        self == PtrKind::Bot
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PtrBase {
    Any,
    Raw,
    Oop,
    Inst,
    Ary,
    Metadata,
    Klass,
    InstKlass,
    AryKlass,
    NarrowOop,
    NarrowKlass,
}

impl PtrBase {
    pub fn base_rank(self) -> u8 {
        match self {
            PtrBase::Any => 0,
            PtrBase::Oop => 1,
            PtrBase::Raw => 1,
            PtrBase::Metadata => 1,
            PtrBase::Klass => 1,
            PtrBase::Inst => 2,
            PtrBase::Ary => 2,
            PtrBase::InstKlass => 2,
            PtrBase::AryKlass => 2,
            PtrBase::NarrowOop => 3,
            PtrBase::NarrowKlass => 3,
        }
    }

    pub fn family(self) -> u8 {
        match self {
            PtrBase::Any => 0,
            PtrBase::Oop | PtrBase::Inst | PtrBase::Ary | PtrBase::NarrowOop => 1,
            PtrBase::Raw => 2,
            PtrBase::Metadata => 3,
            PtrBase::Klass | PtrBase::InstKlass | PtrBase::AryKlass | PtrBase::NarrowKlass => 4,
        }
    }

    fn common_super(self, other: PtrBase) -> Option<PtrBase> {
        if self == other {
            return Some(self);
        }
        let (f1, f2) = (self.family(), other.family());
        if f1 == f2 {
            return match self.family() {
                1 => Some(PtrBase::Oop),
                4 => Some(PtrBase::Klass),
                _ => Some(PtrBase::Any),
            };
        }
        Some(PtrBase::Any)
    }

    fn is_sub_base(self, other: PtrBase) -> bool {
        if self == other {
            return true;
        }
        match (self, other) {
            (PtrBase::Inst, PtrBase::Oop) | (PtrBase::Ary, PtrBase::Oop) => true,
            (PtrBase::InstKlass, PtrBase::Klass) | (PtrBase::AryKlass, PtrBase::Klass) => true,
            (PtrBase::NarrowOop, PtrBase::Oop) | (PtrBase::NarrowKlass, PtrBase::Klass) => true,
            (b, PtrBase::Any) => b != PtrBase::Any,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PtrType {
    pub ptr: PtrKind,
    pub base: PtrBase,
    pub klass: Option<KlassId>,
    pub interfaces: Vec<KlassId>,
    pub offset: i64,
    pub speculative: Option<Box<Type>>,
    pub inline_depth: i32,
    pub instance_id: i32,
}

impl PtrType {
    pub fn new(ptr: PtrKind, base: PtrBase) -> Self {
        Self {
            ptr,
            base,
            klass: None,
            interfaces: Vec::new(),
            offset: 0,
            speculative: None,
            inline_depth: 0,
            instance_id: 0,
        }
    }

    pub fn with_klass(mut self, k: KlassId) -> Self {
        self.klass = Some(k);
        self
    }

    pub fn with_offset(mut self, off: i64) -> Self {
        self.offset = off;
        self
    }

    pub fn with_interfaces(mut self, ifs: Vec<KlassId>) -> Self {
        self.interfaces = ifs;
        self
    }

    pub fn is_bot(&self) -> bool {
        self.ptr == PtrKind::Bot
    }

    pub fn is_null(&self) -> bool {
        self.ptr == PtrKind::Null
    }

    pub fn is_not_null(&self) -> bool {
        self.ptr == PtrKind::NotNull || self.ptr == PtrKind::Constant
    }

    pub fn may_be_null(&self) -> bool {
        !self.is_not_null()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VectorElem {
    Int8,
    UInt8,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Float32,
    Float64,
    Oop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VectorSize {
    Agnostic,
    Bits32,
    Bits64,
    Bits128,
    Bits256,
    Bits512,
}

#[derive(Clone)]
pub enum Type {
    Bad,
    Control,
    Top,
    Bottom,
    Memory,
    Abio,
    Half,
    ReturnAddress(u64),
    Int(I32Range),
    Long(I64Range),
    HalfFloat(FloatVal),
    Float(FloatVal),
    Double(DoubleVal),
    Tuple(Vec<Type>),
    Array(Box<Type>, Option<I64Range>),
    Interfaces(Vec<KlassId>),
    Function(Box<Type>, Vec<Type>),
    Vector {
        mask: bool,
        size: VectorSize,
        elem: VectorElem,
        len: u32,
    },
    Ptr(PtrType),
    Narrow(TypePtrKind),
    NarrowKlass(TypePtrKind),
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypePtrKind {
    Oop,
    Klass,
}

impl Type {
    pub fn kind(&self) -> TypeKind {
        match self {
            Type::Bad => TypeKind::Bad,
            Type::Control => TypeKind::Control,
            Type::Top => TypeKind::Top,
            Type::Bottom => TypeKind::Bottom,
            Type::Memory => TypeKind::Memory,
            Type::Abio => TypeKind::Abio,
            Type::Half => TypeKind::Half,
            Type::ReturnAddress(_) => TypeKind::ReturnAddress,
            Type::Int(_) => TypeKind::Int,
            Type::Long(_) => TypeKind::Long,
            Type::HalfFloat(v) => match v {
                FloatVal::Top => TypeKind::HalfFloatTop,
                FloatVal::Con(_) => TypeKind::HalfFloatCon,
                FloatVal::Bot => TypeKind::HalfFloatBot,
            },
            Type::Float(v) => match v {
                FloatVal::Top => TypeKind::FloatTop,
                FloatVal::Con(_) => TypeKind::FloatCon,
                FloatVal::Bot => TypeKind::FloatBot,
            },
            Type::Double(v) => match v {
                DoubleVal::Top => TypeKind::DoubleTop,
                DoubleVal::Con(_) => TypeKind::DoubleCon,
                DoubleVal::Bot => TypeKind::DoubleBot,
            },
            Type::Tuple(_) => TypeKind::Tuple,
            Type::Array(..) => TypeKind::Array,
            Type::Interfaces(_) => TypeKind::Interfaces,
            Type::Function(..) => TypeKind::Function,
            Type::Vector { mask: true, .. } => TypeKind::VectorMask,
            Type::Vector { size, .. } => match size {
                VectorSize::Agnostic => TypeKind::VectorA,
                VectorSize::Bits32 => TypeKind::VectorS,
                VectorSize::Bits64 => TypeKind::VectorD,
                VectorSize::Bits128 => TypeKind::VectorX,
                VectorSize::Bits256 => TypeKind::VectorY,
                VectorSize::Bits512 => TypeKind::VectorZ,
            },
            Type::Ptr(p) => match p.base {
                PtrBase::Any => TypeKind::AnyPtr,
                PtrBase::Raw => TypeKind::RawPtr,
                PtrBase::Oop => TypeKind::OopPtr,
                PtrBase::Inst => TypeKind::InstPtr,
                PtrBase::Ary => TypeKind::AryPtr,
                PtrBase::Metadata => TypeKind::MetadataPtr,
                PtrBase::Klass => TypeKind::KlassPtr,
                PtrBase::InstKlass => TypeKind::InstKlassPtr,
                PtrBase::AryKlass => TypeKind::AryKlassPtr,
                PtrBase::NarrowOop => TypeKind::NarrowOop,
                PtrBase::NarrowKlass => TypeKind::NarrowKlass,
            },
            Type::Narrow(_) => TypeKind::NarrowOop,
            Type::NarrowKlass(_) => TypeKind::NarrowKlass,
            Type::Last => TypeKind::Last,
        }
    }

    pub fn is_ptr(&self) -> bool {
        self.kind().is_ptr() || matches!(self, Type::Narrow(_) | Type::NarrowKlass(_))
    }

    pub fn isa_oop(&self) -> bool {
        self.kind().isa_oop() || matches!(self, Type::Narrow(_))
    }

    pub fn is_oop_ptr(&self) -> bool {
        self.isa_oop()
    }

    pub fn is_inst_ptr(&self) -> bool {
        matches!(self, Type::Ptr(p) if p.base == PtrBase::Inst)
    }

    pub fn is_ary_ptr(&self) -> bool {
        matches!(self, Type::Ptr(p) if p.base == PtrBase::Ary)
    }

    pub fn is_klass_ptr(&self) -> bool {
        matches!(self.kind(), TypeKind::KlassPtr | TypeKind::InstKlassPtr | TypeKind::AryKlassPtr)
    }

    pub fn is_int(&self) -> Option<&I32Range> {
        match self {
            Type::Int(r) => Some(r),
            _ => None,
        }
    }

    pub fn isa_int(&self) -> Option<&I32Range> {
        self.is_int()
    }

    pub fn is_long(&self) -> Option<&I64Range> {
        match self {
            Type::Long(r) => Some(r),
            _ => None,
        }
    }

    pub fn isa_long(&self) -> Option<&I64Range> {
        self.is_long()
    }

    pub fn is_tuple(&self) -> Option<&[Type]> {
        match self {
            Type::Tuple(f) => Some(f),
            _ => None,
        }
    }

    pub fn is_ary(&self) -> Option<(&Type, &Option<I64Range>)> {
        match self {
            Type::Array(e, l) => Some((e.as_ref(), l)),
            _ => None,
        }
    }

    pub fn as_ptr(&self) -> Option<&PtrType> {
        match self {
            Type::Ptr(p) => Some(p),
            _ => None,
        }
    }

    pub fn isa_ptr(&self) -> Option<&PtrType> {
        self.as_ptr()
    }

    pub fn is_float(&self) -> Option<FloatVal> {
        match self {
            Type::Float(v) | Type::HalfFloat(v) => Some(*v),
            _ => None,
        }
    }

    pub fn is_double(&self) -> Option<DoubleVal> {
        match self {
            Type::Double(v) => Some(*v),
            _ => None,
        }
    }

    pub fn empty(&self) -> bool {
        match self {
            Type::Top | Type::Bad | Type::Last => true,
            Type::Int(r) => r.lo > r.hi,
            Type::Long(r) => r.lo > r.hi,
            Type::Float(FloatVal::Top) | Type::HalfFloat(FloatVal::Top) => true,
            Type::Double(DoubleVal::Top) => true,
            Type::Ptr(p) => p.ptr == PtrKind::Bot,
            Type::Tuple(f) => f.iter().any(|t| t.empty()),
            Type::Array(e, _) => e.empty(),
            _ => false,
        }
    }

    pub fn is_finite(&self) -> bool {
        match self {
            Type::Int(_) | Type::Long(_) => true,
            Type::Float(FloatVal::Con(_)) | Type::Double(DoubleVal::Con(_)) => true,
            Type::Ptr(p) => p.ptr == PtrKind::Constant || p.ptr == PtrKind::Null,
            _ => false,
        }
    }

    pub fn singleton(&self) -> bool {
        match self {
            Type::Int(r) => r.is_con(),
            Type::Long(r) => r.is_con(),
            Type::Float(FloatVal::Con(_)) | Type::Double(DoubleVal::Con(_)) => true,
            Type::Ptr(p) => p.ptr == PtrKind::Null || p.ptr == PtrKind::Constant,
            _ => false,
        }
    }

    pub fn is_con(&self) -> bool {
        self.singleton()
    }

    pub fn get_con(&self) -> Option<i64> {
        match self {
            Type::Int(r) if r.is_con() => Some(r.lo as i64),
            Type::Long(r) if r.is_con() => Some(r.lo),
            _ => None,
        }
    }

    pub fn get_con_as_long(&self) -> Option<i64> {
        self.get_con()
    }

    pub fn geth(&self) -> Option<u16> {
        match self {
            Type::Int(r) if r.is_con() => Some(r.lo as u16),
            _ => None,
        }
    }

    pub fn getf(&self) -> Option<f32> {
        match self {
            Type::Float(FloatVal::Con(v)) | Type::HalfFloat(FloatVal::Con(v)) => {
                Some(f32::from_bits(*v))
            }
            _ => None,
        }
    }

    pub fn getd(&self) -> Option<f64> {
        match self {
            Type::Double(DoubleVal::Con(v)) => Some(f64::from_bits(*v)),
            _ => None,
        }
    }

    pub fn has_memory(&self) -> bool {
        match self {
            Type::Memory => true,
            Type::Tuple(f) => f.iter().any(|t| t.has_memory()),
            Type::Function(r, a) => r.has_memory() || a.iter().any(|t| t.has_memory()),
            _ => false,
        }
    }

    pub fn is_floatingpoint(&self) -> bool {
        matches!(
            self.kind(),
            TypeKind::FloatTop
                | TypeKind::FloatCon
                | TypeKind::FloatBot
                | TypeKind::DoubleTop
                | TypeKind::DoubleCon
                | TypeKind::DoubleBot
                | TypeKind::HalfFloatTop
                | TypeKind::HalfFloatCon
                | TypeKind::HalfFloatBot
        )
    }

    pub fn make_ptr(&self) -> Option<Type> {
        match self {
            Type::Narrow(_) => Some(Type::Ptr(PtrType::new(PtrKind::AnyNull, PtrBase::Oop))),
            Type::NarrowKlass(_) => {
                Some(Type::Ptr(PtrType::new(PtrKind::AnyNull, PtrBase::Klass)))
            }
            Type::Ptr(_) => Some(self.clone()),
            _ => None,
        }
    }

    pub fn make_narrowoop(&self) -> Type {
        match self {
            Type::Ptr(p) if matches!(p.base, PtrBase::Oop | PtrBase::Inst | PtrBase::Ary) => {
                Type::Narrow(TypePtrKind::Oop)
            }
            _ => self.clone(),
        }
    }

    pub fn make_narrowklass(&self) -> Type {
        match self {
            Type::Ptr(p)
                if matches!(
                    p.base,
                    PtrBase::Klass | PtrBase::InstKlass | PtrBase::AryKlass
                ) =>
            {
                Type::NarrowKlass(TypePtrKind::Klass)
            }
            _ => self.clone(),
        }
    }

    pub fn remove_speculative(&self) -> Type {
        match self {
            Type::Ptr(p) if p.speculative.is_some() => {
                let mut q = p.clone();
                q.speculative = None;
                Type::Ptr(q)
            }
            _ => self.clone(),
        }
    }

    pub fn higher_equal(&self, t: &Type, db: &dyn KlassDb) -> bool {
        self.meet(t, db) == *t
    }

    pub fn higher_equal_speculative(&self, t: &Type, db: &dyn KlassDb) -> bool {
        self.meet_speculative(t, db) == *t
    }

    pub fn meet(&self, t: &Type, db: &dyn KlassDb) -> Type {
        self.meet_helper(t, db, false)
    }

    pub fn meet_speculative(&self, t: &Type, db: &dyn KlassDb) -> Type {
        self.meet_helper(t, db, true)
            .cleanup_speculative()
    }

    fn maybe_remove_speculative(&self, include: bool) -> Type {
        if include {
            self.clone()
        } else {
            self.remove_speculative()
        }
    }

    fn cleanup_speculative(self) -> Type {
        self.remove_speculative()
    }

    fn meet_helper(&self, t: &Type, db: &dyn KlassDb, speculative: bool) -> Type {
        if self.is_ptr_to_narrowoop() && t.is_ptr_to_narrowoop() {
            if let (Some(a), Some(b)) = (self.make_ptr(), t.make_ptr()) {
                return a.meet(&b, db).make_narrowoop();
            }
        }
        if self.is_ptr_to_narrowklass() && t.is_ptr_to_narrowklass() {
            if let (Some(a), Some(b)) = (self.make_ptr(), t.make_ptr()) {
                return a.meet(&b, db).make_narrowklass();
            }
        }
        let a = self.maybe_remove_speculative(speculative);
        let b = t.maybe_remove_speculative(speculative);
        a.xmeet(&b, db)
    }

    fn xmeet(&self, t: &Type, db: &dyn KlassDb) -> Type {
        use Type::*;
        if std::ptr::eq(self, t) || self == t {
            return self.clone();
        }
        match (self.kind(), t.kind()) {
            (TypeKind::Bottom, _) => Bottom,
            (_, TypeKind::Bottom) => Bottom,
            (TypeKind::Top, _) => t.clone(),
            (_, TypeKind::Top) => self.clone(),
            (TypeKind::Bad, _) | (_, TypeKind::Bad) => Bad,
            (TypeKind::Last, _) | (_, TypeKind::Last) => Last,
            (TypeKind::Control, TypeKind::Control) => Control,
            (TypeKind::Memory, TypeKind::Memory) => Memory,
            (TypeKind::Abio, TypeKind::Abio) => Abio,
            (TypeKind::Half, TypeKind::Half) => Half,
            (TypeKind::ReturnAddress, TypeKind::ReturnAddress) => self.clone(),
            (TypeKind::Int, TypeKind::Int) => {
                let (ra, rb) = (self.is_int().unwrap(), t.is_int().unwrap());
                Int(ra.hull(rb))
            }
            (TypeKind::Long, TypeKind::Long) => {
                let (ra, rb) = (self.is_long().unwrap(), t.is_long().unwrap());
                Long(ra.hull(rb))
            }
            (TypeKind::HalfFloatTop, TypeKind::HalfFloatBot)
            | (TypeKind::HalfFloatBot, TypeKind::HalfFloatTop)
            | (TypeKind::HalfFloatTop, TypeKind::HalfFloatTop) => {
                HalfFloat(FloatVal::Bot)
            }
            (TypeKind::HalfFloatCon, TypeKind::HalfFloatBot)
            | (TypeKind::HalfFloatBot, TypeKind::HalfFloatCon) => HalfFloat(FloatVal::Bot),
            (TypeKind::HalfFloatCon, TypeKind::HalfFloatCon) => HalfFloat(FloatVal::Bot),
            (TypeKind::HalfFloatTop, _) | (_, TypeKind::HalfFloatTop) => self.clone_or_top(t),
            (TypeKind::FloatTop, TypeKind::FloatBot)
            | (TypeKind::FloatBot, TypeKind::FloatTop)
            | (TypeKind::FloatTop, TypeKind::FloatTop) => Float(FloatVal::Bot),
            (TypeKind::FloatCon, TypeKind::FloatBot)
            | (TypeKind::FloatBot, TypeKind::FloatCon) => Float(FloatVal::Bot),
            (TypeKind::FloatCon, TypeKind::FloatCon) => Float(FloatVal::Bot),
            (TypeKind::FloatTop, _) | (_, TypeKind::FloatTop) => self.clone_or_top(t),
            (TypeKind::DoubleTop, TypeKind::DoubleBot)
            | (TypeKind::DoubleBot, TypeKind::DoubleTop)
            | (TypeKind::DoubleTop, TypeKind::DoubleTop) => Double(DoubleVal::Bot),
            (TypeKind::DoubleCon, TypeKind::DoubleBot)
            | (TypeKind::DoubleBot, TypeKind::DoubleCon) => Double(DoubleVal::Bot),
            (TypeKind::DoubleCon, TypeKind::DoubleCon) => Double(DoubleVal::Bot),
            (TypeKind::DoubleTop, _) | (_, TypeKind::DoubleTop) => self.clone_or_top(t),
            (TypeKind::Tuple, TypeKind::Tuple) => {
                let (fa, fb) = (self.is_tuple().unwrap(), t.is_tuple().unwrap());
                if fa.len() != fb.len() {
                    return Bottom;
                }
                let fields: Vec<Type> = fa
                    .iter()
                    .zip(fb.iter())
                    .map(|(x, y)| x.meet(y, db))
                    .collect();
                Tuple(fields)
            }
            (TypeKind::Array, TypeKind::Array) => {
                let (ea, la) = match self {
                    Type::Array(e, l) => (e, l),
                    _ => unreachable!(),
                };
                let (eb, lb) = match t {
                    Type::Array(e, l) => (e, l),
                    _ => unreachable!(),
                };
                let elem = ea.meet(eb, db);
                let len = match (la, lb) {
                    (Some(a), Some(b)) => Some(a.hull(b)),
                    (Some(a), None) => Some(*a),
                    (None, Some(b)) => Some(*b),
                    (None, None) => None,
                };
                Array(Box::new(elem), len)
            }
            (TypeKind::Interfaces, TypeKind::Interfaces) => {
                let (a, b) = match (self, t) {
                    (Type::Interfaces(x), Type::Interfaces(y)) => (x, y),
                    _ => unreachable!(),
                };
                let mut union: Vec<KlassId> = a.clone();
                for k in b {
                    if !union.contains(k) {
                        union.push(*k);
                    }
                }
                union.sort();
                Interfaces(union)
            }
            (TypeKind::Function, TypeKind::Function) => {
                let (ra, aa) = match self {
                    Type::Function(r, a) => (r, a),
                    _ => unreachable!(),
                };
                let (rb, ab) = match t {
                    Type::Function(r, a) => (r, a),
                    _ => unreachable!(),
                };
                if aa.len() != ab.len() {
                    return Bottom;
                }
                let ret = ra.meet(rb, db);
                let args: Vec<Type> = aa
                    .iter()
                    .zip(ab.iter())
                    .map(|(x, y)| x.meet(y, db))
                    .collect();
                Function(Box::new(ret), args)
            }
            (ka, kb) if ka.is_ptr() || kb.is_ptr() || self.is_ptr() || t.is_ptr() => {
                self.ptr_meet(t, db)
            }
            (TypeKind::VectorA, TypeKind::VectorA)
            | (TypeKind::VectorS, TypeKind::VectorS)
            | (TypeKind::VectorD, TypeKind::VectorD)
            | (TypeKind::VectorX, TypeKind::VectorX)
            | (TypeKind::VectorY, TypeKind::VectorY)
            | (TypeKind::VectorZ, TypeKind::VectorZ)
            | (TypeKind::VectorMask, TypeKind::VectorMask) => {
                if self == t {
                    self.clone()
                } else {
                    Bottom
                }
            }
            _ => {
                if self.kind() == t.kind() {
                    self.clone()
                } else {
                    Bottom
                }
            }
        }
    }

    fn clone_or_top(&self, t: &Type) -> Type {
        if self.kind() == t.kind() {
            self.clone()
        } else {
            t.clone()
        }
    }

    fn ptr_meet(&self, t: &Type, db: &dyn KlassDb) -> Type {
        let pa = self.as_ptr_type();
        let pb = t.as_ptr_type();
        let (a, b) = match (pa, pb) {
            (Some(x), Some(y)) => (x, y),
            _ => return Type::Bottom,
        };
        if a.ptr == PtrKind::Bot {
            return Type::Ptr(a);
        }
        if b.ptr == PtrKind::Bot {
            return Type::Ptr(b);
        }
        if a.ptr == PtrKind::Top {
            return Type::Ptr(b);
        }
        if b.ptr == PtrKind::Top {
            return Type::Ptr(a);
        }

        let base = match a.base.common_super(b.base) {
            Some(b) => b,
            None => return Type::Top,
        };

        let ptr = ptr_kind_meet(a.ptr, b.ptr);

        let klass = match (a.klass, b.klass) {
            (Some(ka), Some(kb)) => {
                if ka == kb {
                    Some(ka)
                } else if db.is_subtype_of(ka, kb) {
                    Some(kb)
                } else if db.is_subtype_of(kb, ka) {
                    Some(ka)
                } else {
                    db.common_supertype(ka, kb)
                }
            }
            (Some(k), None) | (None, Some(k)) => Some(k),
            (None, None) => None,
        };

        let mut ifaces = a.interfaces.clone();
        for k in &b.interfaces {
            if !ifaces.contains(k) {
                ifaces.push(*k);
            }
        }
        ifaces.sort();
        ifaces.dedup();

        let offset = if a.offset == b.offset {
            a.offset
        } else if a.offset == OFFSET_BOT || b.offset == OFFSET_BOT {
            OFFSET_BOT
        } else if a.offset == OFFSET_TOP || b.offset == OFFSET_TOP {
            OFFSET_TOP
        } else {
            OFFSET_BOT
        };

        let speculative = if a.speculative.is_some() {
            a.speculative.clone()
        } else {
            b.speculative.clone()
        };
        Type::Ptr(PtrType {
            ptr,
            base,
            klass,
            interfaces: ifaces,
            offset,
            speculative,
            inline_depth: a.inline_depth.max(b.inline_depth),
            instance_id: a.instance_id.max(b.instance_id),
        })
    }

    fn as_ptr_type(&self) -> Option<PtrType> {
        match self {
            Type::Ptr(p) => Some(p.clone()),
            Type::Narrow(TypePtrKind::Oop) => {
                Some(PtrType::new(PtrKind::AnyNull, PtrBase::Oop))
            }
            Type::Narrow(TypePtrKind::Klass) | Type::NarrowKlass(_) => {
                Some(PtrType::new(PtrKind::AnyNull, PtrBase::Klass))
            }
            _ => None,
        }
    }

    pub fn join(&self, t: &Type, db: &dyn KlassDb) -> Type {
        self.join_helper(t, db, false)
    }

    pub fn join_speculative(&self, t: &Type, db: &dyn KlassDb) -> Type {
        self.join_helper(t, db, true).cleanup_speculative()
    }

    fn join_helper(&self, t: &Type, db: &dyn KlassDb, speculative: bool) -> Type {
        if self.is_ptr_to_narrowoop() && t.is_ptr_to_narrowoop() {
            if let (Some(a), Some(b)) = (self.make_ptr(), t.make_ptr()) {
                return a.join(&b, db).make_narrowoop();
            }
        }
        if self.is_ptr_to_narrowklass() && t.is_ptr_to_narrowklass() {
            if let (Some(a), Some(b)) = (self.make_ptr(), t.make_ptr()) {
                return a.join(&b, db).make_narrowklass();
            }
        }
        let a = self.maybe_remove_speculative(speculative);
        let b = t.maybe_remove_speculative(speculative);
        a.xjoin(&b, db)
    }

    fn xjoin(&self, t: &Type, db: &dyn KlassDb) -> Type {
        use Type::*;
        if self == t {
            return self.clone();
        }
        match (self.kind(), t.kind()) {
            (TypeKind::Top, _) => Top,
            (_, TypeKind::Top) => Top,
            (TypeKind::Bottom, _) => t.clone(),
            (_, TypeKind::Bottom) => self.clone(),
            (TypeKind::Bad, _) | (_, TypeKind::Bad) => Bad,
            (TypeKind::Last, _) | (_, TypeKind::Last) => Last,
            (TypeKind::Control, TypeKind::Control) => Control,
            (TypeKind::Memory, TypeKind::Memory) => Memory,
            (TypeKind::Abio, TypeKind::Abio) => Abio,
            (TypeKind::Half, TypeKind::Half) => Half,
            (TypeKind::ReturnAddress, TypeKind::ReturnAddress) => self.clone(),
            (TypeKind::Int, TypeKind::Int) => {
                let (ra, rb) = (self.is_int().unwrap(), t.is_int().unwrap());
                match ra.glb(rb) {
                    Some(r) => Int(r),
                    None => Top,
                }
            }
            (TypeKind::Long, TypeKind::Long) => {
                let (ra, rb) = (self.is_long().unwrap(), t.is_long().unwrap());
                match ra.glb(rb) {
                    Some(r) => Long(r),
                    None => Top,
                }
            }
            (ka, kb)
                if matches!(
                    ka,
                    TypeKind::HalfFloatTop
                        | TypeKind::HalfFloatCon
                        | TypeKind::HalfFloatBot
                        | TypeKind::FloatTop
                        | TypeKind::FloatCon
                        | TypeKind::FloatBot
                        | TypeKind::DoubleTop
                        | TypeKind::DoubleCon
                        | TypeKind::DoubleBot
                ) || matches!(
                    kb,
                    TypeKind::HalfFloatTop
                        | TypeKind::HalfFloatCon
                        | TypeKind::HalfFloatBot
                        | TypeKind::FloatTop
                        | TypeKind::FloatCon
                        | TypeKind::FloatBot
                        | TypeKind::DoubleTop
                        | TypeKind::DoubleCon
                        | TypeKind::DoubleBot
                ) =>
            {
                self.float_join(t)
            }
            (TypeKind::Tuple, TypeKind::Tuple) => {
                let (fa, fb) = (self.is_tuple().unwrap(), t.is_tuple().unwrap());
                if fa.len() != fb.len() {
                    return Top;
                }
                let mut fields = Vec::with_capacity(fa.len());
                for (x, y) in fa.iter().zip(fb.iter()) {
                    let j = x.join(y, db);
                    if j.kind() == TypeKind::Top {
                        return Top;
                    }
                    fields.push(j);
                }
                Tuple(fields)
            }
            (TypeKind::Array, TypeKind::Array) => {
                let (ea, la) = match self {
                    Type::Array(e, l) => (e, l),
                    _ => unreachable!(),
                };
                let (eb, lb) = match t {
                    Type::Array(e, l) => (e, l),
                    _ => unreachable!(),
                };
                let elem = ea.join(eb, db);
                if elem.kind() == TypeKind::Top {
                    return Top;
                }
                let len = match (la, lb) {
                    (Some(a), Some(b)) => a.glb(b),
                    _ => None,
                };
                Array(Box::new(elem), len)
            }
            (TypeKind::Interfaces, TypeKind::Interfaces) => {
                let (a, b) = match (self, t) {
                    (Type::Interfaces(x), Type::Interfaces(y)) => (x, y),
                    _ => unreachable!(),
                };
                let common: Vec<KlassId> = a
                    .iter()
                    .filter(|k| b.contains(k))
                    .copied()
                    .collect();
                Interfaces(common)
            }
            (TypeKind::Function, TypeKind::Function) => {
                let (ra, aa) = match self {
                    Type::Function(r, a) => (r, a),
                    _ => unreachable!(),
                };
                let (rb, ab) = match t {
                    Type::Function(r, a) => (r, a),
                    _ => unreachable!(),
                };
                if aa.len() != ab.len() {
                    return Top;
                }
                let ret = ra.join(rb, db);
                if ret.kind() == TypeKind::Top {
                    return Top;
                }
                let mut args = Vec::with_capacity(aa.len());
                for (x, y) in aa.iter().zip(ab.iter()) {
                    let j = x.join(y, db);
                    if j.kind() == TypeKind::Top {
                        return Top;
                    }
                    args.push(j);
                }
                Function(Box::new(ret), args)
            }
            (ka, kb) if ka.is_ptr() || kb.is_ptr() || self.is_ptr() || t.is_ptr() => {
                self.ptr_join(t, db)
            }
            (ka, kb) if ka == kb => self.clone(),
            _ => Top,
        }
    }

    fn float_join(&self, t: &Type) -> Type {
        use Type::*;
        let kind_pair = (self.kind(), t.kind());
        let same_width = matches!(
            (kind_pair.0, kind_pair.1),
            (TypeKind::HalfFloatTop, TypeKind::HalfFloatBot)
                | (TypeKind::HalfFloatCon, TypeKind::HalfFloatBot)
                | (TypeKind::HalfFloatBot, TypeKind::HalfFloatTop)
                | (TypeKind::HalfFloatCon, TypeKind::HalfFloatTop)
                | (TypeKind::HalfFloatBot, TypeKind::HalfFloatCon)
                | (TypeKind::HalfFloatTop, TypeKind::HalfFloatCon)
                | (TypeKind::HalfFloatCon, TypeKind::HalfFloatCon)
                | (TypeKind::FloatTop, TypeKind::FloatBot)
                | (TypeKind::FloatCon, TypeKind::FloatBot)
                | (TypeKind::FloatBot, TypeKind::FloatTop)
                | (TypeKind::FloatCon, TypeKind::FloatTop)
                | (TypeKind::FloatBot, TypeKind::FloatCon)
                | (TypeKind::FloatTop, TypeKind::FloatCon)
                | (TypeKind::FloatCon, TypeKind::FloatCon)
                | (TypeKind::DoubleTop, TypeKind::DoubleBot)
                | (TypeKind::DoubleCon, TypeKind::DoubleBot)
                | (TypeKind::DoubleBot, TypeKind::DoubleTop)
                | (TypeKind::DoubleCon, TypeKind::DoubleTop)
                | (TypeKind::DoubleBot, TypeKind::DoubleCon)
                | (TypeKind::DoubleTop, TypeKind::DoubleCon)
                | (TypeKind::DoubleCon, TypeKind::DoubleCon)
        );
        if !same_width {
            return Top;
        }
        let con_a = match self {
            Type::HalfFloat(FloatVal::Con(v)) | Type::Float(FloatVal::Con(v)) => Some(*v),
            _ => None,
        };
        let con_b = match t {
            Type::HalfFloat(FloatVal::Con(v)) | Type::Float(FloatVal::Con(v)) => Some(*v),
            _ => None,
        };
        let is_double = matches!(self.kind(), TypeKind::DoubleTop | TypeKind::DoubleCon | TypeKind::DoubleBot);
        if is_double {
            let da = match self {
                Type::Double(DoubleVal::Con(v)) => Some(*v),
                _ => None,
            };
            let dbv = match t {
                Type::Double(DoubleVal::Con(v)) => Some(*v),
                _ => None,
            };
            return match (da, dbv) {
                (Some(x), Some(y)) if x == y => Double(DoubleVal::Con(x)),
                (Some(_), Some(_)) => Top,
                _ => Double(DoubleVal::Top),
            };
        }
        let wide = |k: TypeKind| match k {
            TypeKind::HalfFloatTop | TypeKind::HalfFloatCon | TypeKind::HalfFloatBot => {
                HalfFloat(FloatVal::Bot)
            }
            TypeKind::DoubleTop | TypeKind::DoubleCon | TypeKind::DoubleBot => {
                Double(DoubleVal::Bot)
            }
            _ => Float(FloatVal::Bot),
        };
        let empty = |k: TypeKind| match k {
            TypeKind::HalfFloatTop | TypeKind::HalfFloatCon | TypeKind::HalfFloatBot => {
                HalfFloat(FloatVal::Top)
            }
            TypeKind::DoubleTop | TypeKind::DoubleCon | TypeKind::DoubleBot => {
                Double(DoubleVal::Top)
            }
            _ => Float(FloatVal::Top),
        };
        let is_top = |k: TypeKind| matches!(
            k,
            TypeKind::HalfFloatTop | TypeKind::FloatTop | TypeKind::DoubleTop
        );
        match (con_a, con_b) {
            (Some(x), Some(y)) if x == y => {
                if matches!(self.kind(), TypeKind::HalfFloatTop | TypeKind::HalfFloatCon | TypeKind::HalfFloatBot) {
                    HalfFloat(FloatVal::Con(x))
                } else {
                    Float(FloatVal::Con(x))
                }
            }
            (Some(_), Some(_)) => empty(self.kind()),
            (Some(_), None) => {
                if matches!(self.kind(), TypeKind::FloatCon | TypeKind::HalfFloatCon) {
                    self.clone()
                } else {
                    t.clone()
                }
            }
            (None, Some(_)) => {
                if matches!(t.kind(), TypeKind::FloatCon | TypeKind::HalfFloatCon) {
                    t.clone()
                } else {
                    self.clone()
                }
            }
            (None, None) => {
                if is_top(self.kind()) {
                    self.clone()
                } else if is_top(t.kind()) {
                    t.clone()
                } else {
                    wide(self.kind())
                }
            }
        }
    }

    fn ptr_join(&self, t: &Type, db: &dyn KlassDb) -> Type {
        let pa = self.as_ptr_type();
        let pb = t.as_ptr_type();
        let (a, b) = match (pa, pb) {
            (Some(x), Some(y)) => (x, y),
            _ => return Type::Top,
        };
        if a.ptr == PtrKind::Bot || b.ptr == PtrKind::Bot {
            return Type::Top;
        }
        if a.ptr == PtrKind::Top {
            return Type::Ptr(b);
        }
        if b.ptr == PtrKind::Top {
            return Type::Ptr(a);
        }

        let base = match a.base.common_super(b.base) {
            Some(b) => b,
            None => return Type::Top,
        };

        let ptr = ptr_kind_join(a.ptr, b.ptr);

        let klass = match (a.klass, b.klass) {
            (Some(ka), Some(kb)) => {
                if ka == kb {
                    Some(ka)
                } else if db.is_subtype_of(ka, kb) {
                    Some(ka)
                } else if db.is_subtype_of(kb, ka) {
                    Some(kb)
                } else {
                    return if a.may_be_null() && b.may_be_null() {
                        Type::Ptr(PtrType {
                            ptr: PtrKind::Null,
                            base,
                            klass: None,
                            interfaces: Vec::new(),
                            offset: 0,
                            speculative: None,
                            inline_depth: 0,
                            instance_id: 0,
                        })
                    } else {
                        Type::Ptr(PtrType {
                            ptr: PtrKind::Bot,
                            base,
                            klass: None,
                            interfaces: Vec::new(),
                            offset: OFFSET_BOT,
                            speculative: None,
                            inline_depth: 0,
                            instance_id: 0,
                        })
                    };
                }
            }
            (Some(k), None) | (None, Some(k)) => Some(k),
            (None, None) => None,
        };

        let common_ifaces: Vec<KlassId> = a
            .interfaces
            .iter()
            .filter(|k| b.interfaces.contains(k))
            .copied()
            .collect();

        let offset = if a.offset == b.offset {
            a.offset
        } else {
            OFFSET_BOT
        };

        Type::Ptr(PtrType {
            ptr,
            base,
            klass,
            interfaces: common_ifaces,
            offset,
            speculative: None,
            inline_depth: a.inline_depth.min(b.inline_depth),
            instance_id: a.instance_id.min(b.instance_id),
        })
    }

    pub fn filter(&self, kills: &Type, db: &dyn KlassDb) -> Type {
        self.filter_helper(kills, db, false)
    }

    pub fn filter_speculative(&self, kills: &Type, db: &dyn KlassDb) -> Type {
        self.filter_helper(kills, db, true).cleanup_speculative()
    }

    fn filter_helper(&self, kills: &Type, db: &dyn KlassDb, speculative: bool) -> Type {
        let ft = self.join_helper(kills, db, speculative);
        if ft.empty() {
            Type::Top
        } else {
            ft
        }
    }

    pub fn widen(&self, old: &Type, limit: &Type) -> Type {
        match (self, old) {
            (Type::Int(new), Type::Int(o)) => {
                if o.lo == o.hi {
                    return old.clone();
                }
                if new.lo <= o.lo && new.hi >= o.hi {
                    if new.widen > o.widen {
                        return self.clone();
                    }
                    if new.widen == WIDEN_MAX {
                        let (mut max, mut min) = (i32::MAX, i32::MIN);
                        if let Type::Int(l) = limit {
                            max = l.hi;
                            min = l.lo;
                        }
                        if min < new.lo && new.hi < max {
                            if new.lo >= 0
                                || ((new.lo as u32).wrapping_sub(min as u32))
                                    >= ((max as u32).wrapping_sub(new.hi as u32))
                            {
                                return Type::Int(I32Range::new(new.lo, max, WIDEN_MAX));
                            } else {
                                return Type::Int(I32Range::new(min, new.hi, WIDEN_MAX));
                            }
                        }
                        return Type::int();
                    }
                    return Type::Int(I32Range::new(new.lo, new.hi, new.widen + 1));
                }
                if o.lo <= new.lo && o.hi >= new.hi {
                    return old.clone();
                }
                Type::int()
            }
            (Type::Long(new), Type::Long(o)) => {
                if o.lo == o.hi {
                    return old.clone();
                }
                if new.lo <= o.lo && new.hi >= o.hi {
                    if new.widen > o.widen {
                        return self.clone();
                    }
                    if new.widen == WIDEN_MAX {
                        return Type::long();
                    }
                    return Type::Long(I64Range::new(new.lo, new.hi, new.widen + 1));
                }
                if o.lo <= new.lo && o.hi >= new.hi {
                    return old.clone();
                }
                Type::long()
            }
            _ => self.clone(),
        }
    }

    pub fn narrow(&self, old: &Type) -> Type {
        match (self, old) {
            (Type::Int(new), Type::Int(o)) => {
                if new.lo >= new.hi {
                    return self.clone();
                }
                if o.lo == i32::MIN && o.hi == i32::MAX {
                    return self.clone();
                }
                if new.lo < o.lo || new.hi > o.hi {
                    return self.clone();
                }
                let nrange = (new.hi as u32).wrapping_sub(new.lo as u32);
                let orange = (o.hi as u32).wrapping_sub(o.lo as u32);
                if nrange < u32::MAX - 1 && nrange > (orange >> 1) + 512 {
                    return old.clone();
                }
                self.clone()
            }
            (Type::Long(new), Type::Long(o)) => {
                if new.lo >= new.hi {
                    return self.clone();
                }
                if o.lo == i64::MIN && o.hi == i64::MAX {
                    return self.clone();
                }
                if new.lo < o.lo || new.hi > o.hi {
                    return self.clone();
                }
                self.clone()
            }
            _ => self.clone(),
        }
    }

    pub fn bottom_type(&self) -> Type {
        self.clone()
    }

    pub fn is_same_type_as(&self, other: &Type, db: &dyn KlassDb) -> bool {
        self.meet(other, db) == *other && self.join(other, db) == *self
    }

    pub fn is_meet_subtype_of(&self, other: &Type, db: &dyn KlassDb) -> bool {
        self.meet(other, db) == *self
    }

    pub fn is_meet_same_type_as(&self, other: &Type, db: &dyn KlassDb) -> bool {
        self.is_same_type_as(other, db)
    }

    pub fn is_java_subtype_of(&self, other: &Type, db: &dyn KlassDb) -> bool {
        self.is_meet_subtype_of(other, db)
    }

    pub fn maybe_java_subtype_of(&self, other: &Type, db: &dyn KlassDb) -> bool {
        self.is_meet_subtype_of(other, db)
    }

    pub fn is_ptr_to_narrowoop(&self) -> bool {
        matches!(self, Type::Ptr(p) if p.base == PtrBase::NarrowOop)
    }

    pub fn is_ptr_to_narrowklass(&self) -> bool {
        matches!(self, Type::Ptr(p) if p.base == PtrBase::NarrowKlass)
    }

    pub fn with_offset(&self, offset: i64) -> Type {
        match self {
            Type::Ptr(p) => {
                let mut q = p.clone();
                q.offset = offset;
                Type::Ptr(q)
            }
            _ => self.clone(),
        }
    }

    pub fn add_offset(&self, offset: i64) -> Type {
        match self {
            Type::Ptr(p) => {
                let mut q = p.clone();
                q.offset = if p.offset == OFFSET_BOT || p.offset == OFFSET_TOP {
                    p.offset
                } else {
                    p.offset.wrapping_add(offset)
                };
                Type::Ptr(q)
            }
            _ => self.clone(),
        }
    }

    pub fn array_element_basic_type(&self) -> BasicType {
        match self.kind() {
            TypeKind::Int => match self {
                Type::Int(r) if r.lo == 0 && r.hi == 1 => BasicType::Boolean,
                Type::Int(r) if r.lo == i8::MIN as i32 && r.hi == i8::MAX as i32 => BasicType::Byte,
                Type::Int(r) if r.lo == 0 && r.hi == u16::MAX as i32 => BasicType::Char,
                Type::Int(r) if r.lo == i16::MIN as i32 && r.hi == i16::MAX as i32 => {
                    BasicType::Short
                }
                _ => BasicType::Int,
            },
            TypeKind::Long => BasicType::Long,
            TypeKind::FloatTop | TypeKind::FloatCon | TypeKind::FloatBot => BasicType::Float,
            TypeKind::DoubleTop | TypeKind::DoubleCon | TypeKind::DoubleBot => BasicType::Double,
            _ => BasicType::Void,
        }
    }

    pub fn constructors() -> Vec<Type> {
        vec![
            Type::if_both(),
            Type::if_false(),
            Type::if_neither(),
            Type::if_true(),
            Type::loop_body(),
            Type::membar(),
            Type::store_conditional(),
            Type::int_pair(),
            Type::long_pair(),
            Type::int_cc_pair(),
            Type::long_cc_pair(),
        ]
    }

    pub fn bottom() -> Type {
        Type::Bottom
    }

    pub fn top() -> Type {
        Type::Top
    }

    pub fn control() -> Type {
        Type::Control
    }

    pub fn memory() -> Type {
        Type::Memory
    }

    pub fn abio() -> Type {
        Type::Abio
    }

    pub fn half() -> Type {
        Type::Half
    }

    pub fn int() -> Type {
        Type::Int(I32Range::new(i32::MIN, i32::MAX, WIDEN_MAX))
    }

    pub fn long() -> Type {
        Type::Long(I64Range::new(i64::MIN, i64::MAX, WIDEN_MAX))
    }

    pub fn float() -> Type {
        Type::Float(FloatVal::Bot)
    }

    pub fn double() -> Type {
        Type::Double(DoubleVal::Bot)
    }

    pub fn half_float() -> Type {
        Type::HalfFloat(FloatVal::Bot)
    }

    pub fn bool_() -> Type {
        Type::Int(I32Range::new(0, 1, WIDEN_MIN))
    }

    pub fn byte_() -> Type {
        Type::Int(I32Range::new(i8::MIN as i32, i8::MAX as i32, WIDEN_MIN))
    }

    pub fn ubyte_() -> Type {
        Type::Int(I32Range::new(0, u8::MAX as i32, WIDEN_MIN))
    }

    pub fn char_() -> Type {
        Type::Int(I32Range::new(0, u16::MAX as i32, WIDEN_MIN))
    }

    pub fn short_() -> Type {
        Type::Int(I32Range::new(i16::MIN as i32, i16::MAX as i32, WIDEN_MIN))
    }

    pub fn pos() -> Type {
        Type::Int(I32Range::new(0, i32::MAX, WIDEN_MIN))
    }

    pub fn pos1() -> Type {
        Type::Int(I32Range::new(1, i32::MAX, WIDEN_MIN))
    }

    pub fn sym_int() -> Type {
        Type::Int(I32Range::new(-i32::MAX, i32::MAX, WIDEN_MIN))
    }

    pub fn cc() -> Type {
        Self::bool_()
    }

    pub fn int_const(v: i32) -> Type {
        Type::Int(I32Range::new(v, v, WIDEN_MIN))
    }

    pub fn long_const(v: i64) -> Type {
        Type::Long(I64Range::new(v, v, WIDEN_MIN))
    }

    pub fn float_const(v: f32) -> Type {
        Type::Float(FloatVal::Con(v.to_bits()))
    }

    pub fn double_const(v: f64) -> Type {
        Type::Double(DoubleVal::Con(v.to_bits()))
    }

    pub fn tuple(fields: Vec<Type>) -> Type {
        Type::Tuple(fields)
    }

    pub fn if_both() -> Type {
        Type::Tuple(vec![Type::Control, Type::Control])
    }

    pub fn if_false() -> Type {
        Type::Tuple(vec![Type::Control, Type::Top])
    }

    pub fn if_neither() -> Type {
        Type::Tuple(vec![Type::Top, Type::Top])
    }

    pub fn if_true() -> Type {
        Type::Tuple(vec![Type::Top, Type::Control])
    }

    pub fn loop_body() -> Type {
        Type::Tuple(vec![Type::Control, Type::int()])
    }

    pub fn membar() -> Type {
        Type::Tuple(Vec::new())
    }

    pub fn store_conditional() -> Type {
        Type::Tuple(vec![Type::cc(), Type::Memory])
    }

    pub fn int_pair() -> Type {
        Type::Tuple(vec![Type::int(), Type::int()])
    }

    pub fn long_pair() -> Type {
        Type::Tuple(vec![Type::long(), Type::long()])
    }

    pub fn int_cc_pair() -> Type {
        Type::Tuple(vec![Type::int(), Type::cc()])
    }

    pub fn long_cc_pair() -> Type {
        Type::Tuple(vec![Type::long(), Type::cc()])
    }

    pub fn null_ptr() -> Type {
        Type::Ptr(PtrType {
            ptr: PtrKind::Null,
            base: PtrBase::Any,
            klass: None,
            interfaces: Vec::new(),
            offset: 0,
            speculative: None,
            inline_depth: 0,
            instance_id: 0,
        })
    }

    pub fn not_null_ptr() -> Type {
        Type::Ptr(PtrType {
            ptr: PtrKind::NotNull,
            base: PtrBase::Any,
            klass: None,
            interfaces: Vec::new(),
            offset: OFFSET_BOT,
            speculative: None,
            inline_depth: 0,
            instance_id: 0,
        })
    }

    pub fn ptr_bottom() -> Type {
        Type::Ptr(PtrType {
            ptr: PtrKind::Bot,
            base: PtrBase::Any,
            klass: None,
            interfaces: Vec::new(),
            offset: OFFSET_BOT,
            speculative: None,
            inline_depth: 0,
            instance_id: 0,
        })
    }

    pub fn any_ptr() -> Type {
        Type::Ptr(PtrType::new(PtrKind::AnyNull, PtrBase::Any))
    }

    pub fn raw_ptr() -> Type {
        Type::Ptr(PtrType::new(PtrKind::AnyNull, PtrBase::Raw))
    }

    pub fn oop_ptr() -> Type {
        Type::Ptr(PtrType::new(PtrKind::AnyNull, PtrBase::Oop))
    }

    pub fn inst_ptr(klass: Option<KlassId>) -> Type {
        Type::Ptr(PtrType::new(PtrKind::AnyNull, PtrBase::Inst).with_klass_opt(klass))
    }

    pub fn inst_ptr_not_null(klass: Option<KlassId>) -> Type {
        let mut p = PtrType::new(PtrKind::NotNull, PtrBase::Inst);
        p.klass = klass;
        Type::Ptr(p)
    }

    pub fn ary_ptr(elem: Type, klass: Option<KlassId>) -> Type {
        let mut p = PtrType::new(PtrKind::AnyNull, PtrBase::Ary);
        p.klass = klass;
        let _ = elem;
        Type::Ptr(p)
    }

    pub fn klass_ptr(klass: Option<KlassId>) -> Type {
        let mut p = PtrType::new(PtrKind::AnyNull, PtrBase::Klass);
        p.klass = klass;
        Type::Ptr(p)
    }

    pub fn metadata_ptr() -> Type {
        Type::Ptr(PtrType::new(PtrKind::AnyNull, PtrBase::Metadata))
    }

    pub fn narrow_oop() -> Type {
        Type::Narrow(TypePtrKind::Oop)
    }

    pub fn narrow_klass() -> Type {
        Type::NarrowKlass(TypePtrKind::Klass)
    }

    pub fn return_address(addr: u64) -> Type {
        Type::ReturnAddress(addr)
    }

    pub fn vector(mask: bool, size: VectorSize, elem: VectorElem, len: u32) -> Type {
        Type::Vector {
            mask,
            size,
            elem,
            len,
        }
    }

    pub fn array(elem: Type) -> Type {
        Type::Array(Box::new(elem), None)
    }

    pub fn interfaces(list: Vec<KlassId>) -> Type {
        Type::Interfaces(list)
    }

    pub fn function(ret: Type, args: Vec<Type>) -> Type {
        Type::Function(Box::new(ret), args)
    }
}

impl PtrType {
    fn with_klass_opt(mut self, k: Option<KlassId>) -> Self {
        self.klass = k;
        self
    }
}

fn ptr_kind_meet(a: PtrKind, b: PtrKind) -> PtrKind {
    use PtrKind::*;
    if a == b {
        return a;
    }
    match (a, b) {
        (Bot, _) => Bot,
        (_, Bot) => Bot,
        (Top, x) => x,
        (x, Top) => x,
        (Constant, Null) | (Null, Constant) => AnyNull,
        (Constant, NotNull) | (NotNull, Constant) => NotNull,
        (Constant, AnyNull) | (AnyNull, Constant) => AnyNull,
        (Null, NotNull) | (NotNull, Null) => AnyNull,
        (Null, AnyNull) | (AnyNull, Null) => AnyNull,
        (NotNull, AnyNull) | (AnyNull, NotNull) => AnyNull,
        _ => AnyNull,
    }
}

fn ptr_kind_join(a: PtrKind, b: PtrKind) -> PtrKind {
    use PtrKind::*;
    if a == b {
        return a;
    }
    fn leq(x: PtrKind, y: PtrKind) -> bool {
        use PtrKind::*;
        match (x, y) {
            (Bot, _) => true,
            (_, Top) => true,
            (Null, AnyNull) => true,
            (NotNull, AnyNull) => true,
            (Constant, AnyNull) | (Constant, NotNull) => true,
            _ => false,
        }
    }
    if leq(a, b) {
        return a;
    }
    if leq(b, a) {
        return b;
    }
    Bot
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BasicType {
    Void,
    Boolean,
    Char,
    Byte,
    Short,
    Int,
    Long,
    Float,
    Double,
    Object,
    Array,
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Type::Bad, Type::Bad)
            | (Type::Control, Type::Control)
            | (Type::Top, Type::Top)
            | (Type::Bottom, Type::Bottom)
            | (Type::Memory, Type::Memory)
            | (Type::Abio, Type::Abio)
            | (Type::Half, Type::Half)
            | (Type::Last, Type::Last) => true,
            (Type::ReturnAddress(a), Type::ReturnAddress(b)) => a == b,
            (Type::Int(a), Type::Int(b)) => a == b,
            (Type::Long(a), Type::Long(b)) => a == b,
            (Type::HalfFloat(a), Type::HalfFloat(b)) => a == b,
            (Type::Float(a), Type::Float(b)) => a == b,
            (Type::Double(a), Type::Double(b)) => a == b,
            (Type::Tuple(a), Type::Tuple(b)) => a == b,
            (Type::Array(ae, al), Type::Array(be, bl)) => ae == be && al == bl,
            (Type::Interfaces(a), Type::Interfaces(b)) => a == b,
            (Type::Function(ar, aa), Type::Function(br, ba)) => ar == br && aa == ba,
            (
                Type::Vector {
                    mask: m1,
                    size: s1,
                    elem: e1,
                    len: l1,
                },
                Type::Vector {
                    mask: m2,
                    size: s2,
                    elem: e2,
                    len: l2,
                },
            ) => m1 == m2 && s1 == s2 && e1 == e2 && l1 == l2,
            (Type::Ptr(a), Type::Ptr(b)) => a == b,
            (Type::Narrow(a), Type::Narrow(b)) => a == b,
            (Type::NarrowKlass(a), Type::NarrowKlass(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Type {}

impl fmt::Debug for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int(r) if r.is_con() => write!(f, "int:{}", r.lo),
            Type::Int(r) if r.lo == i32::MIN && r.hi == i32::MAX => write!(f, "int"),
            Type::Int(r) => write!(f, "int:{}..{}", r.lo, r.hi),
            Type::Long(r) if r.is_con() => write!(f, "long:{}", r.lo),
            Type::Long(r) if r.lo == i64::MIN && r.hi == i64::MAX => write!(f, "long"),
            Type::Long(r) => write!(f, "long:{}..{}", r.lo, r.hi),
            Type::Float(FloatVal::Con(v)) => write!(f, "float:{}", f32::from_bits(*v)),
            Type::Float(FloatVal::Bot) => write!(f, "float"),
            Type::Float(FloatVal::Top) => write!(f, "floattop"),
            Type::Double(DoubleVal::Con(v)) => write!(f, "double:{}", f64::from_bits(*v)),
            Type::Double(DoubleVal::Bot) => write!(f, "double"),
            Type::Double(DoubleVal::Top) => write!(f, "doubletop"),
            Type::Ptr(p) => write!(
                f,
                "ptr({:?},{:?},klass={:?},off={})",
                p.ptr, p.base, p.klass, p.offset
            ),
            Type::Tuple(fs) => write!(f, "tuple{:?}", fs),
            other => write!(f, "{:?}", other.kind()),
        }
    }
}

pub fn meet_types(a: &Type, b: &Type) -> Type {
    a.meet(b, &EmptyKlassDb)
}

pub fn join_types(a: &Type, b: &Type) -> Type {
    a.join(b, &EmptyKlassDb)
}

pub fn filter_type(t: &Type, kills: &Type) -> Type {
    t.filter(kills, &EmptyKlassDb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> EmptyKlassDb {
        EmptyKlassDb
    }

    #[test]
    fn int_meet_is_hull() {
        let a = Type::int_const(1);
        let b = Type::int_const(10);
        let m = a.meet(&b, &db());
        match m {
            Type::Int(r) => {
                assert_eq!(r.lo, 1);
                assert_eq!(r.hi, 10);
            }
            _ => panic!("expected int range, got {:?}", m),
        }
        let r = Type::Int(I32Range::new(3, 20, WIDEN_MIN));
        let m2 = a.meet(&r, &db());
        match m2 {
            Type::Int(rr) => {
                assert_eq!(rr.lo, 1);
                assert_eq!(rr.hi, 20);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn int_join_is_intersection() {
        let a = Type::Int(I32Range::new(1, 10, WIDEN_MIN));
        let b = Type::Int(I32Range::new(5, 20, WIDEN_MIN));
        match a.join(&b, &db()) {
            Type::Int(r) => {
                assert_eq!(r.lo, 5);
                assert_eq!(r.hi, 10);
            }
            other => panic!("expected int, got {:?}", other),
        }
        let disjoint = Type::Int(I32Range::new(30, 40, WIDEN_MIN));
        assert_eq!(a.join(&disjoint, &db()), Type::Top);
    }

    #[test]
    fn lattice_top_bottom() {
        let i = Type::int_const(7);
        assert_eq!(i.meet(&Type::Top, &db()), i);
        assert_eq!(i.meet(&Type::Bottom, &db()), Type::Bottom);
        assert_eq!(i.join(&Type::Top, &db()), Type::Top);
        assert_eq!(i.join(&Type::Bottom, &db()), i);
        assert!(Type::Top.empty());
        assert!(!Type::Bottom.empty());
    }

    #[test]
    fn named_ranges() {
        assert_eq!(Type::bool_(), Type::Int(I32Range::new(0, 1, WIDEN_MIN)));
        assert_eq!(Type::char_(), Type::Int(I32Range::new(0, 65535, WIDEN_MIN)));
        assert!(Type::bool_().join(&Type::byte_(), &db()) != Type::Top);
    }

    #[test]
    fn conversions_ranges() {
        let c = Type::char_();
        let b = Type::byte_();
        let j = c.join(&b, &db());
        match j {
            Type::Int(r) => {
                assert_eq!(r.lo, 0);
                assert_eq!(r.hi, 127);
            }
            _ => panic!("join should intersect, got {:?}", j),
        }
        let m = c.meet(&b, &db());
        match m {
            Type::Int(r) => {
                assert_eq!(r.lo, i8::MIN as i32);
                assert_eq!(r.hi, u16::MAX as i32);
            }
            _ => panic!("meet should hull, got {:?}", m),
        }
    }

    #[test]
    fn ptr_null_meet() {
        let nn = Type::inst_ptr_not_null(None);
        let null = Type::null_ptr();
        let m = nn.meet(&null, &db());
        assert!(m.is_ptr());
        let p = m.as_ptr().unwrap();
        assert!(p.may_be_null());
        let j = nn.join(&null, &db());
        assert_eq!(j.kind(), TypeKind::AnyPtr);
        assert_eq!(j.as_ptr().unwrap().ptr, PtrKind::Bot);
    }

    #[test]
    fn ptr_join_unrelated_klass() {
        let a = Type::inst_ptr(Some(KlassId(1)));
        let b = Type::inst_ptr(Some(KlassId(2)));
        let j = a.join(&b, &db());
        assert!(j.is_ptr());
        assert_eq!(j.as_ptr().unwrap().ptr, PtrKind::Null);
        let m = a.meet(&b, &db());
        assert!(m.is_ptr());
    }

    #[test]
    fn tuple_meet_join() {
        let t1 = Type::tuple(vec![Type::int_const(1), Type::control()]);
        let t2 = Type::tuple(vec![Type::int_const(5), Type::control()]);
        let m = t1.meet(&t2, &db());
        match m {
            Type::Tuple(f) => match &f[0] {
                Type::Int(r) => {
                    assert_eq!(r.lo, 1);
                    assert_eq!(r.hi, 5);
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
        let j = t1.join(&t2, &db());
        match j {
            Type::Tuple(f) => match &f[0] {
                Type::Int(r) => {
                    assert!(r.lo > r.hi);
                }
                _ => panic!(),
            },
            Type::Top => {}
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn filter_semantics() {
        let t = Type::Int(I32Range::new(1, 10, WIDEN_MIN));
        let kills = Type::int_const(5);
        let f = t.filter(&kills, &db());
        match f {
            Type::Int(r) => {
                assert_eq!(r.lo, 5);
                assert_eq!(r.hi, 5);
            }
            _ => panic!("filter should intersect, got {:?}", f),
        }
        let empty = t.filter(&Type::int_const(50), &db());
        assert_eq!(empty, Type::Top);
    }

    #[test]
    fn widen_narrow() {
        let old = Type::Int(I32Range::new(0, 10, WIDEN_MIN));
        let new = Type::Int(I32Range::new(0, 20, WIDEN_MIN));
        let w = new.widen(&old, &Type::int());
        match w {
            Type::Int(r) => assert!(r.hi >= 20),
            _ => panic!(),
        }
        let n = w.narrow(&old);
        assert!(n.is_int().is_some());
    }

    #[test]
    fn float_meet_join() {
        let a = Type::float_const(1.5);
        let b = Type::float();
        assert_eq!(a.meet(&b, &db()), Type::float());
        assert_eq!(a.join(&b, &db()), a);
        let c = Type::float_const(2.5);
        assert_eq!(a.join(&c, &db()).kind(), TypeKind::FloatTop);
        assert_eq!(a.meet(&c, &db()), Type::float());
    }

    #[test]
    fn higher_equal() {
        let sub = Type::bool_();
        let sup = Type::int();
        assert!(sub.higher_equal(&sup, &db()));
        assert!(!sup.higher_equal(&sub, &db()));
    }
}
