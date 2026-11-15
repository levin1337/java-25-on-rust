/*
 * Port of OpenJDK 25 HotSpot ciConstant (share/ci/ciConstant.hpp/.cpp)
 *
 * ciConstant wraps the constant-pool entries the C2 parser can fold:
 * int/long/float/double literals, String constants and class constants.
 * `CiConstantPool::constant_at` is the `ciConstant::make` analogue
 * resolving a pool index for the parser.
 */

use super::ciInstanceKlass::CiInstanceKlass;
use super::ciType::BasicType;
use crate::share::classfile::classFileParser::Constant;
use crate::share::oops::instanceKlass::InstanceKlass;
use std::sync::Arc;

/// `ciConstant`: a compile-time constant usable by the optimizers.
#[derive(Debug, Clone, PartialEq)]
pub enum CiConstant {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(Vec<u8>),
    /// Internal name of a `CONSTANT_Class` entry (`java/lang/String`).
    Klass(Vec<u8>),
    /// Utf8 payload of a `CONSTANT_Utf8` or descriptor entry.
    Utf8(Vec<u8>),
    /// Null reference or zero primitive — HotSpot
    /// `ciConstant::is_null_or_zero`.
    NullOrZero,
}

impl CiConstant {
    #[inline]
    pub fn is_valid(&self) -> bool {
        !matches!(self, Self::NullOrZero)
    }

    #[inline]
    pub fn basic_type(&self) -> BasicType {
        match self {
            Self::Int(_) => BasicType::Int,
            Self::Long(_) => BasicType::Long,
            Self::Float(_) => BasicType::Float,
            Self::Double(_) => BasicType::Double,
            Self::String(_) | Self::Klass(_) | Self::Utf8(_) | Self::NullOrZero => {
                BasicType::Object
            }
        }
    }

    /// `ciConstant::is_null` — the JVM null reference.
    #[inline]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::NullOrZero)
    }

    /// `ciConstant::as_int` for `Int` constants.
    #[inline]
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// `ciConstant::as_object` analogue for String constants: the
    /// utf8 payload of a `CONSTANT_String`.
    #[inline]
    pub fn as_string(&self) -> Option<&[u8]> {
        match self {
            Self::String(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// `ciConstant::as_klass`: internal name for `Klass` constants.
    #[inline]
    pub fn as_klass_name(&self) -> Option<&[u8]> {
        match self {
            Self::Klass(name) => Some(name),
            _ => None,
        }
    }

    /// `ciConstant::make` for a self-contained `Constant` pool entry.
    /// Indirect entries (`CONSTANT_String`, `CONSTANT_Class`) need the
    /// pool for resolution — use `CiConstantPool::constant_at`.
    pub fn from_pool_entry(entry: &Constant<'_>) -> Option<Self> {
        Some(match entry {
            Constant::Integer(v) => Self::Int(*v),
            Constant::Long(v) => Self::Long(*v),
            Constant::Float(v) => Self::Float(*v),
            Constant::Double(v) => Self::Double(*v),
            Constant::Utf8(bytes) => Self::Utf8((*bytes).to_vec()),
            _ => return None,
        })
    }
}

impl std::fmt::Display for CiConstant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Long(v) => write!(f, "{v}L"),
            Self::Float(v) => write!(f, "{v}F"),
            Self::Double(v) => write!(f, "{v}D"),
            Self::String(bytes) => write!(f, "\"{}\"", String::from_utf8_lossy(bytes)),
            Self::Klass(name) => write!(f, "class {}", String::from_utf8_lossy(name)),
            Self::Utf8(bytes) => write!(f, "\"{}\"", String::from_utf8_lossy(bytes)),
            Self::NullOrZero => write!(f, "null"),
        }
    }
}

/// Constant-pool view over one holder klass: answers
/// `constant_at(index)` / `member_ref(index)` queries like the parser's
/// constant-pool access in HotSpot (`ciInstanceKlass::get_field`,
/// `ciConstant::make`).
pub struct CiConstantPool<'a> {
    holder: CiInstanceKlass<'a>,
    klass: Arc<InstanceKlass<'a>>,
}

impl<'a> CiConstantPool<'a> {
    pub fn new(holder: CiInstanceKlass<'a>) -> Self {
        let klass = holder.instance_klass();
        Self { holder, klass }
    }

    /// `ciConstantPool::constant_at`: resolve pool index `index` to a
    /// foldable constant.  `CONSTANT_String` entries resolve through
    /// their utf8 payload; `CONSTANT_Class` to the internal name;
    /// numeric entries directly.  Returns `None` for symbolic entries
    /// (Fieldref/Methodref/InvokeDynamic/... are not constants).
    pub fn constant_at(&self, index: u16) -> Option<CiConstant> {
        let constants = &self.klass.class.constants;
        match constants.get(usize::from(index))? {
            Constant::String(utf8_index) => match constants.get(usize::from(*utf8_index))? {
                Constant::Utf8(bytes) => Some(CiConstant::String(bytes.to_vec())),
                _ => None,
            },
            Constant::Class(name_index) => match constants.get(usize::from(*name_index))? {
                Constant::Utf8(name) => Some(CiConstant::Klass(name.to_vec())),
                _ => None,
            },
            other => CiConstant::from_pool_entry(other),
        }
    }

    /// `ciConstantPool::constant_at` convenience for `ldc`/`ldc_w`
    /// operands, including `CONSTANT_Class` -> klass constant.
    pub fn ldc_constant(&self, index: u16) -> Option<CiConstant> {
        self.constant_at(index)
    }

    /// `ldc2_w` operand: only Long/Double entries are valid.
    pub fn ldc2_constant(&self, index: u16) -> Option<CiConstant> {
        match self.constant_at(index)? {
            c @ (CiConstant::Long(_) | CiConstant::Double(_)) => Some(c),
            _ => None,
        }
    }

    /// The `(owner, name, descriptor)` triple of a Fieldref/Methodref
    /// constant (`ciEnv::resolve_method_at` companion).
    pub fn member_ref(&self, index: u16, is_field: bool) -> Option<[&'a [u8]; 3]> {
        self.holder.member_ref(index, is_field)
    }

    /// `ciConstantPool::is_valid_index` analogue.
    #[inline]
    pub fn is_valid_index(&self, index: u16) -> bool {
        usize::from(index) < self.klass.class.constants.len()
    }
}
