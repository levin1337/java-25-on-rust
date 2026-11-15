/*
 * Port of OpenJDK 25 HotSpot ciType (share/ci/ciType.hpp, ciType.cpp)
 *
 * ciType is the compiler-interface representation of a Java type.
 * Primitive types are singleton values; reference types are backed by
 * ciInstanceKlass / ciArrayKlass handles resolved through the ciEnv.
 */

use super::ciArrayKlass::CiArrayKlass;
use super::ciEnv::CiEnv;
use super::ciInstanceKlass::CiInstanceKlass;

/// HotSpot `BasicType` (globalDefinitions.hpp) restricted to the values
/// the ci layer needs for type queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BasicType {
    Boolean = 0,
    Char = 1,
    Float = 2,
    Double = 3,
    Byte = 4,
    Short = 5,
    Int = 6,
    Long = 7,
    Object = 8,
    Array = 9,
    Void = 10,
    Illegal = 11,
}

impl BasicType {
    /// `type2size`: size in stack/local slots (0 for void).
    #[inline]
    pub fn size(self) -> usize {
        match self {
            Self::Long | Self::Double => 2,
            Self::Void | Self::Illegal => 0,
            _ => 1,
        }
    }

    /// `type2aelembytes`-style byte size used by field layout.
    #[inline]
    pub fn size_in_bytes(self) -> usize {
        match self {
            Self::Long | Self::Double => 8,
            Self::Float | Self::Int => 4,
            Self::Char | Self::Short => 2,
            Self::Byte | Self::Boolean => 1,
            Self::Object | Self::Array => 4,
            Self::Void | Self::Illegal => 0,
        }
    }

    #[inline]
    pub fn is_reference(self) -> bool {
        matches!(self, Self::Object | Self::Array)
    }
}

/// `ciType`: a Java type known to the compiler interface.
///
/// Mirrors the HotSpot class hierarchy where `ciInstanceKlass` and
/// `ciArrayKlass` derive from `ciKlass`/`ciType`.  Primitive variants are
/// plain values; reference variants carry the resolved klass facade.
#[derive(Clone)]
pub enum CIType<'a> {
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Void,
    Instance(CiInstanceKlass<'a>),
    Array(CiArrayKlass<'a>),
}

impl<'a> std::fmt::Debug for CIType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Boolean => write!(f, "boolean"),
            Self::Byte => write!(f, "byte"),
            Self::Char => write!(f, "char"),
            Self::Short => write!(f, "short"),
            Self::Int => write!(f, "int"),
            Self::Long => write!(f, "long"),
            Self::Float => write!(f, "float"),
            Self::Double => write!(f, "double"),
            Self::Void => write!(f, "void"),
            Self::Instance(k) => write!(f, "class {}", k.name_string()),
            Self::Array(a) => write!(f, "{}[]", a.element_type().descriptor()),
        }
    }
}

impl<'a> PartialEq for CIType<'a> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Boolean, Self::Boolean)
            | (Self::Byte, Self::Byte)
            | (Self::Char, Self::Char)
            | (Self::Short, Self::Short)
            | (Self::Int, Self::Int)
            | (Self::Long, Self::Long)
            | (Self::Float, Self::Float)
            | (Self::Double, Self::Double)
            | (Self::Void, Self::Void) => true,
            (Self::Instance(a), Self::Instance(b)) => a.name() == b.name(),
            (Self::Array(a), Self::Array(b)) => a == b,
            _ => false,
        }
    }
}

impl<'a> Eq for CIType<'a> {}

impl<'a> CIType<'a> {
    /// `ciType::basic_type`.
    #[inline]
    pub fn basic_type(&self) -> BasicType {
        match self {
            Self::Boolean => BasicType::Boolean,
            Self::Byte => BasicType::Byte,
            Self::Char => BasicType::Char,
            Self::Short => BasicType::Short,
            Self::Int => BasicType::Int,
            Self::Long => BasicType::Long,
            Self::Float => BasicType::Float,
            Self::Double => BasicType::Double,
            Self::Void => BasicType::Void,
            Self::Instance(_) => BasicType::Object,
            Self::Array(_) => BasicType::Array,
        }
    }

    /// `ciType::is_primitive` — not void, not a reference.
    #[inline]
    pub fn is_primitive(&self) -> bool {
        !matches!(self, Self::Void | Self::Instance(_) | Self::Array(_))
    }

    /// `ciType::is_reference`.
    #[inline]
    pub fn is_reference(&self) -> bool {
        matches!(self, Self::Instance(_) | Self::Array(_))
    }

    #[inline]
    pub fn is_void(&self) -> bool {
        matches!(self, Self::Void)
    }

    #[inline]
    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }

    /// `ciType::is_instance_klass`-style query on the type itself.
    #[inline]
    pub fn as_instance_klass(&self) -> Option<&CiInstanceKlass<'a>> {
        match self {
            Self::Instance(k) => Some(k),
            _ => None,
        }
    }

    #[inline]
    pub fn as_array_klass(&self) -> Option<&CiArrayKlass<'a>> {
        match self {
            Self::Array(k) => Some(k),
            _ => None,
        }
    }

    /// Slot size in locals/stack words (`size()` in HotSpot terms).
    #[inline]
    pub fn size(&self) -> usize {
        self.basic_type().size()
    }

    #[inline]
    pub fn is_one_word(&self) -> bool {
        self.size() == 1
    }

    #[inline]
    pub fn is_two_word(&self) -> bool {
        self.size() == 2
    }

    /// Field-descriptor form: `I`, `J`, `Ljava/lang/String;`, `[I`, ...
    pub fn descriptor(&self) -> String {
        match self {
            Self::Boolean => "Z".to_string(),
            Self::Byte => "B".to_string(),
            Self::Char => "C".to_string(),
            Self::Short => "S".to_string(),
            Self::Int => "I".to_string(),
            Self::Long => "J".to_string(),
            Self::Float => "F".to_string(),
            Self::Double => "D".to_string(),
            Self::Void => "V".to_string(),
            Self::Instance(k) => format!("L{};", k.name_string()),
            Self::Array(a) => a.descriptor(),
        }
    }

    /// Primitive type for a single-letter field descriptor.
    fn primitive_from_char(c: u8) -> Option<Self> {
        Some(match c {
            b'Z' => Self::Boolean,
            b'B' => Self::Byte,
            b'C' => Self::Char,
            b'S' => Self::Short,
            b'I' => Self::Int,
            b'J' => Self::Long,
            b'F' => Self::Float,
            b'D' => Self::Double,
            b'V' => Self::Void,
            _ => return None,
        })
    }

    /// `ciType::make`-style resolution of a single field descriptor
    /// (`Z`, `I`, `[I`, `[[Ljava/lang/String;`, `Ljava/lang/Object;`).
    ///
    /// Returns `None` when a referenced class cannot be loaded through
    /// the environment's class loader.
    pub fn from_descriptor(env: &CiEnv<'a>, desc: &[u8]) -> Option<Self> {
        let (&first, rest) = desc.split_first()?;
        if first == b'[' {
            let mut dims = 1usize;
            let mut elem = rest;
            while let Some((&b'[', tail)) = elem.split_first() {
                dims += 1;
                elem = tail;
            }
            let element = Self::from_descriptor(env, elem)?;
            return Some(Self::Array(CiArrayKlass::new(element, dims as u32)));
        }
        if first == b'L' {
            let inner = rest.strip_suffix(b";")?;
            let klass = env.get_klass(inner)?;
            return Some(Self::Instance(klass));
        }
        if rest.is_empty() {
            return Self::primitive_from_char(first);
        }
        None
    }

    /// Parses a method/field descriptor list `(params)return` used by
    /// `ciSignature`.  Returns `(param_types, return_type)`.
    pub fn parse_method_descriptor(
        env: &CiEnv<'a>,
        sig: &[u8],
    ) -> Option<(Vec<CIType<'a>>, CIType<'a>)> {
        if sig.first() != Some(&b'(') {
            return None;
        }
        let close = sig.iter().position(|&b| b == b')')?;
        let mut params = Vec::new();
        let mut pos = 1usize;
        while pos < close {
            let (t, len) = Self::parse_field_type_at(env, &sig[pos..close])?;
            params.push(t);
            pos += len;
        }
        let (ret, _) = Self::parse_field_type_at(env, &sig[close + 1..])?;
        Some((params, ret))
    }

    /// Parses one field type starting at `desc[0]`; returns the type and
    /// the number of bytes consumed.
    pub fn parse_field_type_at(env: &CiEnv<'a>, desc: &[u8]) -> Option<(CIType<'a>, usize)> {
        let (&first, rest) = desc.split_first()?;
        if first == b'[' {
            let mut dims = 1usize;
            let mut elem = rest;
            while let Some((&b'[', tail)) = elem.split_first() {
                dims += 1;
                elem = tail;
            }
            let (element, used) = Self::parse_field_type_at(env, elem)?;
            debug_assert_eq!(used, elem.len());
            return Some((
                CIType::Array(CiArrayKlass::new(element, dims as u32)),
                desc.len() - elem.len() + used,
            ));
        }
        if first == b'L' {
            let rel_end = rest.iter().position(|&b| b == b';')?;
            let used = 1 + rel_end + 1;
            let klass = env.get_klass(&rest[..rel_end])?;
            return Some((CIType::Instance(klass), used));
        }
        let t = Self::primitive_from_char(first)?;
        Some((t, 1))
    }

    /// `ciSignature::size` analogue: total argument slot count of a
    /// `(params)return` descriptor (receiver NOT included).
    pub fn signature_slot_size(env: &CiEnv<'a>, sig: &[u8]) -> Option<usize> {
        let (params, _) = Self::parse_method_descriptor(env, sig)?;
        Some(params.iter().map(|t| t.size()).sum())
    }

    /// Whether two types are assignment-compatible in the ci sense:
    /// identical types, or `other` is a subtype of `self` for references.
    /// This is the lightweight form of `ciType::is_subtype_of` used by
    /// the typer before a full `ciTypeFlow` exists.
    pub fn is_subtype_of(&self, other: &CIType<'a>, env: &CiEnv<'a>) -> bool {
        if self == other {
            return true;
        }
        match (self, other) {
            (Self::Instance(a), Self::Instance(b)) => {
                b.is_subclass_of(a) || b.implements_interface(a)
            }
            (Self::Array(a), Self::Array(b)) => {
                a.dimension() == b.dimension()
                    && a.element_type().is_subtype_of(b.element_type(), env)
            }
            // Arrays are subtypes of Object and Clone/Serializable; we only
            // model the Object relation here.
            (Self::Array(_), Self::Instance(k)) => {
                k.name() == b"java/lang/Object" || k.name() == b"java/lang/Clone"
            }
            (Self::Instance(k), Self::Array(_)) => false,
            _ => false,
        }
    }

    /// `ciType::is_instance` — true for non-array reference types.
    #[inline]
    pub fn is_instance(&self) -> bool {
        matches!(self, Self::Instance(_))
    }
}
