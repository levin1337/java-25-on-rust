/*
 * Port of OpenJDK 25 HotSpot ciField (share/ci/ciField.hpp/.cpp)
 *
 * ciField is the result of a field lookup: it carries the canonical
 * holder klass, the field's FieldInfo metadata (access flags, name,
 * descriptor, offset), and lazily resolved type/constant information.
 */

use super::ciEnv::CiEnv;
use super::ciInstanceKlass::CiInstanceKlass;
use super::ciMethod::CiMethod;
use super::ciType::{BasicType, CIType};
use crate::share::classfile::classFileParser::Constant;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::fieldInfo::FieldInfo;

/// `ciConstant`: the constant value of a static final / stable field.
#[derive(Debug, Clone, PartialEq)]
pub enum CiConstant {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(Vec<u8>),
    /// Null reference or zero primitive — HotSpot `ciConstant::is_null_or_zero`.
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
            Self::String(_) | Self::NullOrZero => BasicType::Object,
        }
    }
}

/// `FoldStableValues` product flag analogue (default true in HotSpot).
const FOLD_STABLE_VALUES: bool = true;

/// `ciField`: compiler-interface view of a resolved field.
#[derive(Clone)]
pub struct CiField<'a> {
    pub(crate) env: CiEnv<'a>,
    /// Canonical holder — the klass that actually declares the field
    /// (`fieldDescriptor::field_holder`), not the declared holder from
    /// the constant pool.
    pub(crate) holder: CiInstanceKlass<'a>,
    pub(crate) info: FieldInfo<'a>,
}

impl<'a> CiField<'a> {
    pub(crate) fn new(holder: CiInstanceKlass<'a>, info: FieldInfo<'a>) -> Self {
        Self {
            env: holder.env.clone(),
            holder,
            info,
        }
    }

    /// `ciField::holder` — canonical holder klass.
    #[inline]
    pub fn holder(&self) -> &CiInstanceKlass<'a> {
        &self.holder
    }

    /// `ciField::name`.
    #[inline]
    pub fn name(&self) -> &'a [u8] {
        self.info.name
    }

    /// `ciField::signature` (field descriptor).
    #[inline]
    pub fn signature(&self) -> &'a [u8] {
        self.info.descriptor
    }

    /// `ciField::type` — lazily resolved field type.  Primitive
    /// descriptors resolve without class loading; reference types go
    /// through the env and may fail to load, in which case `None` is
    /// returned (HotSpot defers this to compute_type/ciType::make).
    pub fn field_type(&self) -> Option<CIType<'a>> {
        CIType::from_descriptor(&self.env, self.info.descriptor)
    }

    /// `ciField::layout_type` analogue: the BasicType of the field.
    pub fn layout_type(&self) -> BasicType {
        match self.field_type() {
            Some(t) => t.basic_type(),
            None => {
                // Unresolvable reference type still occupies a word.
                match self.info.descriptor.first() {
                    Some(b'L' | b'[') => BasicType::Object,
                    _ => BasicType::Int,
                }
            }
        }
    }

    /// `ciField::size_in_bytes` (`type2aelembytes(layout_type)`).
    pub fn size_in_bytes(&self) -> usize {
        self.layout_type().size_in_bytes()
    }

    /// `ciField::offset_in_bytes`: byte offset for instance fields,
    /// static slot offset for static fields.  `-1` marks an unresolved
    /// field (HotSpot uses the same sentinel for failed lookups).
    #[inline]
    pub fn offset(&self) -> i32 {
        self.info.offset as i32
    }

    #[inline]
    pub fn offset_in_bytes(&self) -> i32 {
        self.offset()
    }

    // ---- flags -----------------------------------------------------------

    #[inline]
    pub fn access_flags(&self) -> u16 {
        self.info.access_flags
    }

    #[inline]
    pub fn is_public(&self) -> bool {
        self.info.is_public()
    }

    #[inline]
    pub fn is_private(&self) -> bool {
        self.info.is_private()
    }

    #[inline]
    pub fn is_protected(&self) -> bool {
        self.info.is_protected()
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        self.info.is_static()
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        self.info.is_final()
    }

    #[inline]
    pub fn is_volatile(&self) -> bool {
        self.info.is_volatile()
    }

    #[inline]
    pub fn is_transient(&self) -> bool {
        self.info.is_transient()
    }

    #[inline]
    pub fn is_synthetic(&self) -> bool {
        self.info.is_synthetic()
    }

    /// `ciFlags::is_stable` — FieldInfo FF_STABLE.
    #[inline]
    pub fn is_stable(&self) -> bool {
        self.info.field_flags.is_stable()
    }

    /// `ciFlags::has_initialized_final_update` — no port of
    /// FieldStatus exists yet; treated as false (final fields are not
    /// updated outside initializers).
    #[inline]
    pub fn has_initialized_final_update(&self) -> bool {
        false
    }

    // ---- constant value ---------------------------------------------------

    /// `ciField::is_constant`, ported from `initialize_from`:
    /// static+final (minus the System.in/out/err special cases), trusted
    /// non-static finals (`trust_final_non_static_fields`), stable
    /// fields, and CallSite.target.
    pub fn is_constant(&self) -> bool {
        if (self.is_final() && !self.has_initialized_final_update())
            || (FOLD_STABLE_VALUES && self.is_stable())
        {
            if self.is_static() {
                // java.lang.System.{in,out,err} are special non-constant
                // static finals.  Their offsets are layout-dependent;
                // identify them by name+descriptor instead.
                if self.holder.name() == b"java/lang/System"
                    && matches!(self.name(), b"in" | b"out" | b"err")
                    && self.signature() == b"Ljava/io/PrintStream;"
                {
                    return false;
                }
                return true;
            }
            return (FOLD_STABLE_VALUES && self.is_stable())
                || trust_final_non_static_fields(&self.holder);
        }
        // CallSite.target is a compile-time constant for the JIT.
        if self
            .holder
            .is_subclass_of_name(b"java/lang/invoke/CallSite")
            && self.name() == b"target"
        {
            return true;
        }
        false
    }

    /// `ciField::is_static_constant`.
    #[inline]
    pub fn is_static_constant(&self) -> bool {
        self.is_static() && self.is_constant() && self.constant_value().is_some()
    }

    /// `ciField::constant_value`: reads the ConstantValue attribute
    /// first (the compile-time constant stored in field metadata,
    /// available before class initialization), then falls back to the
    /// InstanceKlass static storage cell.  Returns `None` when the
    /// field is not a constant or the value is null/zero under
    /// FoldStableValues rules.
    pub fn constant_value(&self) -> Option<CiConstant> {
        if !(self.is_static() && self.is_constant()) {
            return None;
        }
        if let Some(c) = self.constant_value_attribute() {
            return Some(c);
        }
        if !self.holder.is_initialized() {
            return None;
        }
        let klass = self.holder.instance_klass();
        let cell = klass.staticField(self.name(), self.signature()).ok()?;
        let value = cell.borrow().clone();
        let constant = match value {
            Value::Int(v) => CiConstant::Int(v),
            Value::Long(v) => CiConstant::Long(v),
            Value::Float(v) => CiConstant::Float(v),
            Value::Double(v) => CiConstant::Double(v),
            Value::String(bytes) => CiConstant::String(bytes.to_vec()),
            Value::Null => CiConstant::NullOrZero,
            _ => return None,
        };
        if FOLD_STABLE_VALUES && self.is_stable() && !constant.is_valid() {
            return None;
        }
        Some(constant)
    }

    /// ConstantValue attribute of the declaring field member:
    /// `constantvalue_index` -> constant-pool literal.
    fn constant_value_attribute(&self) -> Option<CiConstant> {
        let klass = self.holder.klass();
        let member = klass
            .class
            .fields
            .iter()
            .find(|m| m.name == self.name() && m.descriptor == self.signature())?;
        let attr = member
            .attributes
            .iter()
            .find(|a| a.name == b"ConstantValue")?;
        let idx = u16::from_be_bytes([attr.bytes.first().copied()?, attr.bytes.get(1).copied()?]);
        match klass.constant(idx).ok()? {
            Constant::Integer(v) => Some(CiConstant::Int(*v)),
            Constant::Long(v) => Some(CiConstant::Long(*v)),
            Constant::Float(v) => Some(CiConstant::Float(*v)),
            Constant::Double(v) => Some(CiConstant::Double(*v)),
            Constant::String(utf8_idx) => {
                let bytes = klass.utf8(*utf8_idx).ok()?;
                Some(CiConstant::String(bytes.to_vec()))
            }
            _ => None,
        }
    }

    /// `ciField::constant_value_of` analogue: non-static final constant
    /// of a specific object instance.  Requires the object's field
    /// slot value — supplied by the caller as a `Value` (the port has
    /// no ciObject layer yet).
    pub fn constant_value_of_slot(&self, value: &Value<'a>) -> Option<CiConstant> {
        if self.is_static() || !self.is_constant() {
            return None;
        }
        let constant = match *value {
            Value::Int(v) => CiConstant::Int(v),
            Value::Long(v) => CiConstant::Long(v),
            Value::Float(v) => CiConstant::Float(v),
            Value::Double(v) => CiConstant::Double(v),
            Value::String(bytes) => CiConstant::String(bytes.to_vec()),
            Value::Null => CiConstant::NullOrZero,
            _ => return None,
        };
        if FOLD_STABLE_VALUES && self.is_stable() && !constant.is_valid() {
            return None;
        }
        Some(constant)
    }

    // ---- linkage ------------------------------------------------------------

    /// `ciField::will_link`: access check mirroring
    /// `Reflection::verify_member_access` for the common cases —
    /// public always links; private requires the same top-level class
    /// (approximated by identical holder name, since nestmate metadata
    /// is not modeled); protected requires subclass-or-same-package;
    /// package-private requires the same runtime package (same loader +
    /// same package name).  Also verifies the bytecode's static/instance
    /// expectation matches the field.
    pub fn will_link(&self, accessing_method: &CiMethod<'a>, bc: u8) -> bool {
        // Bytecode/staticness consistency (getstatic/putstatic vs
        // getfield/putfield family).
        let expects_static = matches!(bc, 0xb2 | 0xb3 | 0xb9);
        let is_field_access = matches!(bc, 0xb2..=0xb5);
        if is_field_access && expects_static != self.is_static() {
            return false;
        }

        if self.is_public() {
            return true;
        }
        let accessing_holder = accessing_method.holder();
        if self.is_private() {
            // Same top-level class approximation: identical internal name.
            return accessing_holder.name() == self.holder.name();
        }
        if self.is_protected() {
            // Accessor must be the holder, a subclass, or in the same
            // package.
            if accessing_holder.name() == self.holder.name()
                || accessing_holder.is_subclass_of(&self.holder)
            {
                return true;
            }
            return self.same_package(accessing_holder);
        }
        // Package-private.
        self.same_package(accessing_holder)
    }

    /// Same runtime package: identical package prefix and (by
    /// construction of this port) the same class loader.
    fn same_package(&self, other: &CiInstanceKlass<'a>) -> bool {
        fn package_of(name: &[u8]) -> &[u8] {
            match name.iter().rposition(|&b| b == b'/') {
                Some(pos) => &name[..pos],
                None => b"",
            }
        }
        package_of(self.holder.name()) == package_of(other.name())
    }

    /// `ciField::is_call_site_target`: the `target` field of
    /// `java.lang.invoke.CallSite` (or a subclass).
    pub fn is_call_site_target(&self) -> bool {
        self.holder
            .is_subclass_of_name(b"java/lang/invoke/CallSite")
            && self.name() == b"target"
    }

    /// `ciField::is_autobox_cache`: autoboxing cache fields inside the
    /// primitive wrapper classes (`Integer.cache`, `Character.cache`,
    /// ... — the `*Cache` holder pattern used by `Integer.valueOf`).
    /// HotSpot checks holder == the cache class and a small set of
    /// field names; the port checks the wrapper-class cache holders.
    pub fn is_autobox_cache(&self) -> bool {
        if !self.is_static() {
            return false;
        }
        let name = self.holder.name();
        let is_cache_holder = matches!(
            name,
            b"java/lang/Integer$IntegerCache"
                | b"java/lang/Character$CharacterCache"
                | b"java/lang/Byte$ByteCache"
                | b"java/lang/Short$ShortCache"
                | b"java/lang/Long$LongCache"
                | b"java/lang/Boolean"
        );
        is_cache_holder && self.name() == b"cache"
    }

    /// Constant-pool-style identity used by `ciField` equality in
    /// HotSpot (holder+name+signature).
    pub fn same_field(&self, other: &CiField<'a>) -> bool {
        self.holder.name() == other.holder.name()
            && self.name() == other.name()
            && self.signature() == other.signature()
    }
}

/// `trust_final_non_static_fields` ported from ciField.cpp.
fn trust_final_non_static_fields(holder: &CiInstanceKlass<'_>) -> bool {
    if holder.name() == b"java/lang/System" {
        return false;
    }
    const TRUSTED_PREFIXES: &[&[u8]] = &[
        b"java/lang/invoke/",
        b"sun/invoke/",
        b"java/lang/reflect/",
        b"jdk/internal/reflect/",
        b"jdk/internal/foreign/",
        b"jdk/internal/vm/vector/",
        b"jdk/incubator/vector/",
        b"java/lang/",
    ];
    if TRUSTED_PREFIXES
        .iter()
        .any(|p| holder.name().starts_with(p))
    {
        return true;
    }
    // Hidden classes cannot be expressed in this port's class model;
    // boxed classes, records, String and the Atomic*FieldUpdater impls
    // are trusted per the HotSpot list.
    if holder.is_box_klass() || holder.is_record() {
        return true;
    }
    matches!(
        holder.name(),
        b"java/lang/String"
            | b"java/util/concurrent/atomic/AtomicIntegerFieldUpdater$Impl"
            | b"java/util/concurrent/atomic/AtomicLongFieldUpdater$CASUpdater"
            | b"java/util/concurrent/atomic/AtomicLongFieldUpdater$LockedUpdater"
            | b"java/util/concurrent/atomic/AtomicReferenceFieldUpdater$Impl"
    )
}

/// `ciField::print_name_on` analogue.
pub fn field_signature_string(field: &CiField<'_>) -> String {
    format!(
        "{}:{}{}",
        String::from_utf8_lossy(field.holder().name()),
        String::from_utf8_lossy(field.name()),
        String::from_utf8_lossy(field.signature())
    )
}

/// Resolve a field constant-pool reference to (name, descriptor)
/// bytes — used by ciBytecodeStream-like consumers.
pub fn resolve_field_ref<'c>(
    constants: &'c [Constant<'c>],
    index: u16,
) -> Option<(&'c [u8], &'c [u8])> {
    let (owner, nt) = match constants.get(index as usize)? {
        Constant::FieldRef(o, n) => (*o, *n),
        _ => return None,
    };
    let _ = owner;
    let (name_idx, desc_idx) = match constants.get(nt as usize)? {
        Constant::NameAndType(n, d) => (*n, *d),
        _ => return None,
    };
    let name = match constants.get(name_idx as usize)? {
        Constant::Utf8(b) => *b,
        _ => return None,
    };
    let desc = match constants.get(desc_idx as usize)? {
        Constant::Utf8(b) => *b,
        _ => return None,
    };
    Some((name, desc))
}
