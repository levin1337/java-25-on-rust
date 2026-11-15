/*
 * Port of OpenJDK 25 HotSpot ciObjectFactory (share/ci/ciObjectFactory.hpp/.cpp)
 *
 * `ciObjectFactory::get(oop)` returns the ciObject facade for a heap
 * object.  In this port oops are stable handles into the class
 * loader's object stores, so the factory is a thin resolver; results
 * are cached per handle to keep `ciObject*` identity stable like
 * HotSpot's factory does.
 */

use super::ciEnv::CiEnv;
use super::ciInstance::CiInstance;
use super::ciObject::{CiObject, CiObjectRef};
use super::ciType::{CIType, BasicType};
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::oop::Oop;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use std::cell::RefCell;
use std::collections::HashMap;

/// `ciObjectFactory`: resolves heap-object handles to `CiObject`s.
pub struct CiObjectFactory<'a> {
    env: CiEnv<'a>,
    /// handle -> resolved object (identity cache, HotSpot factory
    /// analogue).  Entries may go stale after GC slot reuse; callers
    /// must check `CiObject::is_valid`.
    cache: RefCell<HashMap<CiObjectRef, CiObject<'a>>>,
}

impl<'a> CiObjectFactory<'a> {
    pub fn new(env: CiEnv<'a>) -> Self {
        Self {
            env,
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// `ciObjectFactory::get(oop)`: resolve an `oops::oop::Oop` handle.
    pub fn get_oop(&self, oop: Oop) -> CiObject<'a> {
        match oop {
            Oop::Null => CiObject::Null,
            Oop::Instance(handle) => self.get_instance(handle),
            Oop::TypeArray(handle) => self.get_type_array(handle),
            Oop::ObjArray(handle) => self.get_obj_array(handle),
        }
    }

    /// `ciObjectFactory::get` for an interpreter `Value`: reference
    /// values resolve to objects, everything else yields `Null`.
    pub fn get_value(&self, value: &Value<'a>) -> CiObject<'a> {
        match CiObjectRef::from_value(value) {
            Some(r) => self.get_ref(r),
            None => CiObject::Null,
        }
    }

    /// `ciObjectFactory::get` for a `CiObjectRef`.
    pub fn get_ref(&self, reference: CiObjectRef) -> CiObject<'a> {
        if let Some(obj) = self.cache.borrow().get(&reference) {
            return obj.clone();
        }
        let obj = match reference.kind {
            super::ciObject::CiObjectKind::Instance => {
                CiObject::Instance(CiInstance::new(self.env.clone(), reference.handle))
            }
            super::ciObject::CiObjectKind::TypeArray => {
                let element = self.type_array_element(reference.handle).unwrap_or(CIType::Int);
                CiObject::TypeArray(super::ciObject::CiTypeArray::new(
                    self.env.clone(),
                    reference.handle,
                    element,
                ))
            }
            super::ciObject::CiObjectKind::ObjArray => {
                let component = self.obj_array_component(reference.handle).unwrap_or_default();
                CiObject::ObjArray(super::ciObject::CiObjArray::new(
                    self.env.clone(),
                    reference.handle,
                    &component,
                ))
            }
        };
        self.cache.borrow_mut().insert(reference, obj.clone());
        obj
    }

    /// Convenience: `Value::Object` handle -> `CiObject::Instance`.
    pub fn get_instance(&self, handle: usize) -> CiObject<'a> {
        self.get_ref(CiObjectRef::instance(handle))
    }

    /// `Value::PrimitiveArray` handle -> `CiObject::TypeArray`.
    pub fn get_type_array(&self, handle: usize) -> CiObject<'a> {
        self.get_ref(CiObjectRef::type_array(handle))
    }

    /// `Value::ObjectArray` handle -> `CiObject::ObjArray`.
    pub fn get_obj_array(&self, handle: usize) -> CiObject<'a> {
        self.get_ref(CiObjectRef::obj_array(handle))
    }

    /// The JVM null object.
    #[inline]
    pub fn null(&self) -> CiObject<'a> {
        CiObject::Null
    }

    /// Element primitive type of a `TypeArrayOop` slot.
    fn type_array_element(&self, handle: usize) -> Option<CIType<'a>> {
        let arrays = self.env.loader().arrays.borrow();
        let array = arrays.get(handle)?;
        let bt = match array {
            TypeArrayOop::Boolean(_) => BasicType::Boolean,
            TypeArrayOop::Char(_) => BasicType::Char,
            TypeArrayOop::Float(_) => BasicType::Float,
            TypeArrayOop::Double(_) => BasicType::Double,
            TypeArrayOop::Byte(_) => BasicType::Byte,
            TypeArrayOop::Short(_) => BasicType::Short,
            TypeArrayOop::Int(_) => BasicType::Int,
            TypeArrayOop::Long(_) => BasicType::Long,
        };
        Some(match bt {
            BasicType::Boolean => CIType::Boolean,
            BasicType::Byte => CIType::Byte,
            BasicType::Char => CIType::Char,
            BasicType::Short => CIType::Short,
            BasicType::Int => CIType::Int,
            BasicType::Long => CIType::Long,
            BasicType::Float => CIType::Float,
            BasicType::Double => CIType::Double,
            _ => CIType::Int,
        })
    }

    /// Component name of an `ObjArrayOop` slot.
    fn obj_array_component(&self, handle: usize) -> Option<Vec<u8>> {
        let arrays = self.env.loader().obj_arrays.borrow();
        arrays.get(handle).map(|a| a.component.to_vec())
    }
}

