/*
 * ciInterface boundary for the C2 port (share/ci/ci*.hpp analog).
 *
 * HotSpot's optimizer consumes compiler-interface objects — ciMethod,
 * ciType, ciInstanceKlass, ciField, ciConstant — that wrap metadata
 * (klass hierarchies, method signatures, constant pool entries, bytecode
 * flow types).  Those objects are produced by the VM's compiler
 * interface and are not part of src/share/opto.  This module declares
 * the trait surface the opto phases need so phases can be written and
 * tested against synthetic implementations, exactly as HotSpot phases
 * take `Compile*`/`PhaseGVN&` plus implicit `ciEnv` access.
 *
 * Rule: opto code never fabricates ci data; it queries through these
 * traits.  A `CiEnv` bundles the per-compilation services (klass db,
 * method, signature lookup, intrinsics).
 */

use super::r#type::{BasicType, KlassDb, KlassId, Type};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CiMethodId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CiFieldId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CiTypeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CiConstantId(pub u32);

pub trait CiType: Send + Sync {
    fn id(&self) -> CiTypeId;
    fn basic_type(&self) -> BasicType;
    fn is_primitive(&self) -> bool {
        !matches!(self.basic_type(), BasicType::Object | BasicType::Array)
    }
    fn is_array(&self) -> bool {
        self.basic_type() == BasicType::Array
    }
    fn is_instance_klass(&self) -> bool {
        self.basic_type() == BasicType::Object
    }
    fn klass(&self) -> Option<KlassId> {
        None
    }
    fn as_klass(&self) -> Option<&dyn CiKlass> {
        None
    }
    fn array_element(&self) -> Option<Box<dyn CiType>> {
        None
    }
    fn is_subtype_of(&self, _other: &dyn CiType, _db: &dyn KlassDb) -> bool {
        false
    }
    fn least_upper_bound(&self, _other: &dyn CiType, _db: &dyn KlassDb) -> Option<Type> {
        None
    }
}

pub trait CiKlass: Send + Sync {
    fn id(&self) -> KlassId;
    fn is_subtype_of(&self, _super_: &dyn CiKlass, _db: &dyn KlassDb) -> bool {
        false
    }
    fn is_interface(&self) -> bool {
        false
    }
    fn is_array(&self) -> bool {
        false
    }
    fn super_kClass(&self) -> Option<KlassId> {
        None
    }
    fn element_klass(&self) -> Option<KlassId> {
        None
    }
}

pub trait CiField: Send + Sync {
    fn id(&self) -> CiFieldId;
    fn offset(&self) -> i64;
    fn type_(&self) -> Type;
    fn is_static(&self) -> bool;
    fn holder(&self) -> KlassId;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiSignatureKind {
    Void,
    Int,
    Long,
    Float,
    Double,
    Object,
}

pub struct CiSignature {
    pub args: Vec<CiSignatureKind>,
    pub ret: CiSignatureKind,
    pub has_receiver: bool,
}

impl CiSignature {
    pub fn parse(desc: &str) -> Option<Self> {
        let b = desc.as_bytes();
        if b.first() != Some(&b'(') {
            return None;
        }
        let mut i = 1;
        let mut args = Vec::new();
        while i < b.len() && b[i] != b')' {
            let k = match b[i] {
                b'V' => CiSignatureKind::Void,
                b'J' => CiSignatureKind::Long,
                b'F' => CiSignatureKind::Float,
                b'D' => CiSignatureKind::Double,
                b'L' => {
                    let end = desc[i..].find(';')? + i;
                    i = end;
                    CiSignatureKind::Object
                }
                _ => CiSignatureKind::Int,
            };
            args.push(k);
            i += 1;
        }
        if i >= b.len() || b[i] != b')' {
            return None;
        }
        let ret = match b.get(i + 1) {
            Some(b'V') => CiSignatureKind::Void,
            Some(b'J') => CiSignatureKind::Long,
            Some(b'F') => CiSignatureKind::Float,
            Some(b'D') => CiSignatureKind::Double,
            Some(b'L') => CiSignatureKind::Object,
            _ => CiSignatureKind::Int,
        };
        Some(Self {
            args,
            ret,
            has_receiver: true,
        })
    }

    pub fn result_type(&self) -> Type {
        match self.ret {
            CiSignatureKind::Void => Type::Top,
            CiSignatureKind::Int => Type::int(),
            CiSignatureKind::Long => Type::long(),
            CiSignatureKind::Float => Type::float(),
            CiSignatureKind::Double => Type::double(),
            CiSignatureKind::Object => Type::inst_ptr(None),
        }
    }
}

pub trait CiMethod: Send + Sync {
    fn id(&self) -> CiMethodId;
    fn name(&self) -> &str;
    fn signature(&self) -> CiSignature;
    fn holder(&self) -> KlassId;
    fn is_static(&self) -> bool {
        false
    }
    fn is_native(&self) -> bool {
        false
    }
    fn is_accessor(&self) -> bool {
        false
    }
    fn intrinsic_id(&self) -> Option<u32> {
        None
    }
    fn max_locals(&self) -> usize {
        0
    }
    fn code_size(&self) -> usize {
        0
    }
}

pub trait CiConstant: Send + Sync {
    fn id(&self) -> CiConstantId;
    fn type_(&self) -> Type;
    fn string_value(&self) -> Option<String> {
        None
    }
    fn int_value(&self) -> Option<i32> {
        None
    }
    fn long_value(&self) -> Option<i64> {
        None
    }
    fn float_value(&self) -> Option<f32> {
        None
    }
    fn double_value(&self) -> Option<f64> {
        None
    }
    fn klass(&self) -> Option<KlassId> {
        None
    }
}

pub struct CiEnv<'a> {
    pub klass_db: &'a dyn KlassDb,
    pub method: Option<&'a dyn CiMethod>,
    pub types: Option<&'a dyn CiTypeTable>,
    pub fields: Option<&'a dyn CiFieldTable>,
    pub methods: Option<&'a dyn CiMethodTable>,
    pub constants: Option<&'a dyn CiConstantTable>,
}

pub trait CiTypeTable {
    fn find(&self, id: CiTypeId) -> Option<Box<dyn CiType>>;
    fn from_basic(&self, bt: BasicType) -> Option<Box<dyn CiType>>;
}

pub trait CiFieldTable {
    fn find(&self, id: CiFieldId) -> Option<Box<dyn CiField>>;
}

pub trait CiMethodTable {
    fn find(&self, id: CiMethodId) -> Option<Box<dyn CiMethod>>;
}

pub trait CiConstantTable {
    fn find(&self, id: CiConstantId) -> Option<Box<dyn CiConstant>>;
}

#[derive(Debug)]
pub struct MissingCiData(pub &'static str);

impl std::fmt::Display for MissingCiData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ciInterface data not available: {}", self.0)
    }
}

impl std::error::Error for MissingCiData {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_parse() {
        let s = CiSignature::parse("(IIJ)Ljava/lang/String;").unwrap();
        assert_eq!(s.args.len(), 3);
        assert_eq!(s.ret, CiSignatureKind::Object);
        assert_eq!(s.args[2], CiSignatureKind::Long);
        assert_eq!(CiSignature::parse("()V").unwrap().ret, CiSignatureKind::Void);
        assert!(CiSignature::parse("garbage").is_none());
    }

    #[test]
    fn result_types() {
        let s = CiSignature::parse("(I)J").unwrap();
        assert_eq!(s.result_type(), Type::long());
    }
}
