/*
 * Port of OpenJDK 25 HotSpot ciObject / ciObjArray / ciTypeArray
 * (share/ci/ciObject.hpp, ciObjArray.hpp, ciTypeArray.hpp)
 *
 * `CiObject` is the tagged union over heap objects the compiler
 * interface can see: instances, primitive arrays and object arrays.
 * Each variant carries a stable handle into the class loader's object
 * stores; resolution is deferred to access time so GC slot reuse is
 * detected through `is_valid()`.
 */

use super::ciEnv::CiEnv;
use super::ciInstance::CiInstance;
use super::ciInstanceKlass::CiInstanceKlass;
use super::ciType::CIType;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;

/// Which object store a `CiObject` handle indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CiObjectKind {
    Instance,
    TypeArray,
    ObjArray,
}

/// A reference to one heap object: kind + store handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CiObjectRef {
    pub kind: CiObjectKind,
    pub handle: usize,
}

impl CiObjectRef {
    /// `ciObject` reference for `Value::Object(h)`.
    #[inline]
    pub fn instance(handle: usize) -> Self {
        Self {
            kind: CiObjectKind::Instance,
            handle,
        }
    }

    /// `ciObject` reference for `Value::PrimitiveArray(h)`.
    #[inline]
    pub fn type_array(handle: usize) -> Self {
        Self {
            kind: CiObjectKind::TypeArray,
            handle,
        }
    }

    /// `ciObject` reference for `Value::ObjectArray(h)`.
    #[inline]
    pub fn obj_array(handle: usize) -> Self {
        Self {
            kind: CiObjectKind::ObjArray,
            handle,
        }
    }

    /// Extract an object reference from an interpreter `Value`;
    /// `None` for non-reference values.
    pub fn from_value(value: &Value<'_>) -> Option<Self> {
        match value {
            Value::Object(h) => Some(Self::instance(*h)),
            Value::PrimitiveArray(h) => Some(Self::type_array(*h)),
            Value::ObjectArray(h) => Some(Self::obj_array(*h)),
            _ => None,
        }
    }
}

/// `ciTypeArray`: compiler-interface view of a primitive array oop.
#[derive(Clone)]
pub struct CiTypeArray<'a> {
    env: CiEnv<'a>,
    handle: usize,
    /// Cached element ciType (primitive).
    element: CIType<'a>,
}

impl<'a> CiTypeArray<'a> {
    pub(crate) fn new(env: CiEnv<'a>, handle: usize, element: CIType<'a>) -> Self {
        Self {
            env,
            handle,
            element,
        }
    }

    #[inline]
    pub fn handle(&self) -> usize {
        self.handle
    }

    /// `ciTypeArray::is_valid`.
    pub fn is_valid(&self) -> bool {
        self.env.loader().arrays.borrow().get(self.handle).is_some()
    }

    /// Array length in elements.
    pub fn length(&self) -> usize {
        match self.env.loader().arrays.borrow().get(self.handle) {
            Some(TypeArrayOop::Boolean(v)) => v.len(),
            Some(TypeArrayOop::Char(v)) => v.len(),
            Some(TypeArrayOop::Float(v)) => v.len(),
            Some(TypeArrayOop::Double(v)) => v.len(),
            Some(TypeArrayOop::Byte(v)) => v.len(),
            Some(TypeArrayOop::Short(v)) => v.len(),
            Some(TypeArrayOop::Int(v)) => v.len(),
            Some(TypeArrayOop::Long(v)) => v.len(),
            None => 0,
        }
    }

    /// `ciTypeArray::element_type`.
    #[inline]
    pub fn element_type(&self) -> &CIType<'a> {
        &self.element
    }

    /// Raw element access (`ciTypeArray::get_elem` analogue).
    /// Returns the widened `Value` for the element kind.
    pub fn element_at(&self, index: usize) -> Option<Value<'a>> {
        let arrays = self.env.loader().arrays.borrow();
        let array = arrays.get(self.handle)?;
        Some(match array {
            TypeArrayOop::Boolean(v) => Value::Int(*v.get(index)? as i32),
            TypeArrayOop::Char(v) => Value::Int(*v.get(index)? as i32),
            TypeArrayOop::Byte(v) => Value::Int(*v.get(index)? as i32),
            TypeArrayOop::Short(v) => Value::Int(*v.get(index)? as i32),
            TypeArrayOop::Int(v) => Value::Int(*v.get(index)?),
            TypeArrayOop::Long(v) => Value::Long(*v.get(index)?),
            TypeArrayOop::Float(v) => Value::Float(*v.get(index)?),
            TypeArrayOop::Double(v) => Value::Double(*v.get(index)?),
        })
    }
}

/// `ciObjArray`: compiler-interface view of an object array oop.
#[derive(Clone)]
pub struct CiObjArray<'a> {
    env: CiEnv<'a>,
    handle: usize,
    /// Component klass internal name (descriptor form `[I` arrays are
    /// `CiTypeArray`, not here).
    component: Vec<u8>,
}

impl<'a> CiObjArray<'a> {
    pub(crate) fn new(env: CiEnv<'a>, handle: usize, component: &[u8]) -> Self {
        Self {
            env,
            handle,
            component: component.to_vec(),
        }
    }

    #[inline]
    pub fn handle(&self) -> usize {
        self.handle
    }

    /// `ciObjArray::is_valid`.
    pub fn is_valid(&self) -> bool {
        self.env
            .loader()
            .obj_arrays
            .borrow()
            .get(self.handle)
            .is_some()
    }

    /// Array length.
    pub fn length(&self) -> usize {
        self.env
            .loader()
            .obj_arrays
            .borrow()
            .get(self.handle)
            .map_or(0, |a| a.len())
    }

    /// `ciObjArray::component_type` internal name.
    #[inline]
    pub fn component_name(&self) -> &[u8] {
        &self.component
    }

    /// `ciObjArray::component_type` resolved to a klass.
    pub fn component_klass(&self) -> Option<CiInstanceKlass<'a>> {
        self.env.get_klass(&self.component)
    }

    /// `ciObjArray::get_elem`.
    pub fn element_at(&self, index: usize) -> Option<Value<'a>> {
        let arrays = self.env.loader().obj_arrays.borrow();
        arrays.get(self.handle)?.get(index).ok()
    }
}

/// `ciObject`: tagged union over heap objects visible to the compiler.
#[derive(Clone)]
pub enum CiObject<'a> {
    /// The `null` reference.
    Null,
    /// `ciInstance` (plain object).
    Instance(CiInstance<'a>),
    /// `ciTypeArray` (primitive array).
    TypeArray(CiTypeArray<'a>),
    /// `ciObjArray` (reference array).
    ObjArray(CiObjArray<'a>),
}

impl<'a> CiObject<'a> {
    /// `ciObject::is_null`.
    #[inline]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// `ciObject::is_valid` — for `Null` always true; for heap objects
    /// the handle must still index a live slot.
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Null => true,
            Self::Instance(i) => i.is_valid(),
            Self::TypeArray(a) => a.is_valid(),
            Self::ObjArray(a) => a.is_valid(),
        }
    }

    /// `ciObject::klass` — the ci klass of the object's type.
    /// `None` for `null` and stale handles.
    pub fn klass(&self) -> Option<CiInstanceKlass<'a>> {
        match self {
            Self::Null => None,
            Self::Instance(i) => i.klass(),
            Self::TypeArray(a) => a.element_type().as_instance_klass().cloned(),
            Self::ObjArray(a) => a.component_klass(),
        }
    }

    /// `ciObject::type` — the ciType of the object.
    pub fn type_of(&self) -> Option<CIType<'a>> {
        match self {
            Self::Null => None,
            Self::Instance(i) => i.type_of(),
            Self::TypeArray(a) => {
                Some(CIType::Array(super::ciArrayKlass::CiArrayKlass::new(
                    a.element_type().clone(),
                    1,
                )))
            }
            Self::ObjArray(a) => {
                let elem = CIType::from_descriptor(&a.env, &array_descriptor(&a.component))?;
                Some(CIType::Array(super::ciArrayKlass::CiArrayKlass::new(elem, 1)))
            }
        }
    }

    /// `ciObject::as_instance`.
    #[inline]
    pub fn as_instance(&self) -> Option<&CiInstance<'a>> {
        match self {
            Self::Instance(i) => Some(i),
            _ => None,
        }
    }

    #[inline]
    pub fn as_type_array(&self) -> Option<&CiTypeArray<'a>> {
        match self {
            Self::TypeArray(a) => Some(a),
            _ => None,
        }
    }

    #[inline]
    pub fn as_obj_array(&self) -> Option<&CiObjArray<'a>> {
        match self {
            Self::ObjArray(a) => Some(a),
            _ => None,
        }
    }
}

/// Build the `[T` descriptor for an object-array component name
/// (`java/lang/String` -> `[Ljava/lang/String;`, `I` -> `[I`).
fn array_descriptor(component: &[u8]) -> Vec<u8> {
    let mut d = Vec::with_capacity(component.len() + 3);
    d.push(b'[');
    if component.first() == Some(&b'[')
        || matches!(component, b"Z" | b"B" | b"C" | b"S" | b"I" | b"J" | b"F" | b"D")
    {
        d.extend_from_slice(component);
    } else {
        d.push(b'L');
        d.extend_from_slice(component);
        d.push(b';');
    }
    d
}

impl<'a> std::fmt::Debug for CiObject<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => write!(f, "ciObject null"),
            Self::Instance(i) => std::fmt::Debug::fmt(i, f),
            Self::TypeArray(a) => write!(f, "ciTypeArray #{}", a.handle()),
            Self::ObjArray(a) => write!(f, "ciObjArray #{}", a.handle()),
        }
    }
}

