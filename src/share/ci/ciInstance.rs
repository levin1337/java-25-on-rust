/*
 * Port of OpenJDK 25 HotSpot ciInstance (share/ci/ciInstance.hpp/.cpp)
 *
 * ciInstance is the compiler-interface view of a heap object of a
 * non-array class: it exposes the object's klass and field values as
 * `Value`s, plus validity tracking for GC-moved/dead objects.
 *
 * In this port heap objects live in `ClassLoader::objects` addressed
 * by a stable handle (`Value::Object(handle)`); `ciInstance` stores
 * the handle and re-resolves the klass/fields on each access so a
 * stale handle reports `is_valid() == false` rather than reading
 * freed memory (the GC reuses slots via a free list).
 */

use super::ciEnv::CiEnv;
use super::ciField::CiField;
use super::ciInstanceKlass::CiInstanceKlass;
use super::ciType::CIType;
use crate::share::interpreter::bytecodeInterpreter::Value;
use std::sync::OnceLock;

/// `ciInstance`: compiler-interface view of one heap `InstanceOop`.
#[derive(Clone)]
pub struct CiInstance<'a> {
    env: CiEnv<'a>,
    /// Slot index into `ClassLoader::objects` (the `Value::Object`
    /// handle).
    handle: usize,
    /// Lazily resolved klass; `None` means the handle is out of range
    /// or no longer denotes a live instance.
    klass: OnceLock<Option<CiInstanceKlass<'a>>>,
}

impl<'a> CiInstance<'a> {
    pub(crate) fn new(env: CiEnv<'a>, handle: usize) -> Self {
        Self {
            env,
            handle,
            klass: OnceLock::new(),
        }
    }

    /// `ciInstance::handle` — the object-table slot.
    #[inline]
    pub fn handle(&self) -> usize {
        self.handle
    }

    /// `ciInstance::env`.
    #[inline]
    pub fn env(&self) -> &CiEnv<'a> {
        &self.env
    }

    /// `ciInstance::is_valid`: the handle still indexes a live object.
    pub fn is_valid(&self) -> bool {
        self.klass().is_some()
    }

    /// `ciInstance::klass`: the object's class, `None` when the handle
    /// is stale.
    pub fn klass(&self) -> Option<CiInstanceKlass<'a>> {
        self.klass
            .get_or_init(|| {
                let objects = self.env.loader().borrow_objects();
                objects
                    .get(self.handle)
                    .map(|o| CiInstanceKlass::new(self.env.clone(), std::sync::Arc::clone(&o.klass)))
            })
            .clone()
    }

    /// `ciInstance::type`: `CIType::Instance` of the object's klass.
    pub fn type_of(&self) -> Option<CIType<'a>> {
        self.klass().map(CIType::Instance)
    }

    /// Whether the object's klass is `java/lang/String` — the parser's
    /// fast path for `String` constant folding.
    pub fn is_string(&self) -> bool {
        self.klass()
            .is_some_and(|k| k.name() == b"java/lang/String")
    }

    /// `ciInstance::is_null` — handled at the `CiObject` level; kept
    /// for API parity (an instance handle is never null).
    #[inline]
    pub fn is_null(&self) -> bool {
        false
    }

    /// Field lookup by name+descriptor walking the klass hierarchy
    /// (`ciInstance::get_field` analogue): returns the raw `Value`
    /// stored in the object's field slot.
    pub fn field(&self, name: &[u8], descriptor: &[u8]) -> Option<Value<'a>> {
        let klass = self.klass()?;
        let arc = klass.instance_klass();
        let (_, info) = arc.lookup_field(name, descriptor)?;
        let objects = self.env.loader().borrow_objects();
        objects.get(self.handle)?.fields.get(info.slot).copied()
    }

    /// `ciInstance::get_field` with a resolved `CiField` (canonical
    /// holder already computed): reads the value at the field's slot.
    pub fn field_value(&self, field: &CiField<'a>) -> Option<Value<'a>> {
        let objects = self.env.loader().borrow_objects();
        objects
            .get(self.handle)?
            .fields
            .get(field.info.slot)
            .copied()
    }

    /// Field by byte offset (`getfield` fast path): searches instance
    /// fields of the object's klass hierarchy for `offset`.
    pub fn field_at_offset(&self, offset: i32) -> Option<(CiField<'a>, Value<'a>)> {
        let klass = self.klass()?;
        let field = self
            .env
            .get_field_by_offset(&klass, offset, false)?;
        let value = self.field_value(&field)?;
        Some((field, value))
    }

    /// Number of instance field slots in the object.
    pub fn field_count(&self) -> Option<usize> {
        let objects = self.env.loader().borrow_objects();
        objects.get(self.handle).map(|o| o.fields.len())
    }

    /// Raw slot access (`ciInstance::get_local_value` analogue for
    /// object fields): `fields[slot]` where `slot` is the
    /// `FieldInfo::slot` index.
    pub fn slot_value(&self, slot: usize) -> Option<Value<'a>> {
        let objects = self.env.loader().borrow_objects();
        objects.get(self.handle)?.fields.get(slot).copied()
    }
}

impl<'a> std::fmt::Debug for CiInstance<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.klass() {
            Some(k) => write!(f, "ciInstance #{} {}", self.handle, k.name_string()),
            None => write!(f, "ciInstance #{} <invalid>", self.handle),
        }
    }
}
