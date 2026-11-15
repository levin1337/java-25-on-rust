use super::fieldInfo::{descriptor_kind, FieldInfo, FieldKind};
use super::instanceKlass::InstanceKlass;
use crate::share::interpreter::bytecodeInterpreter::Value;
use std::{io, sync::Arc};

pub struct InstanceOop<'a> {
    pub klass: Arc<InstanceKlass<'a>>,
    pub fields: Vec<Value<'a>>,
}

impl<'a> InstanceOop<'a> {
    pub fn new(klass: Arc<InstanceKlass<'a>>) -> io::Result<Self> {
        let total = klass.total_instance_fields;
        let mut fields = Vec::with_capacity(total);
        fields.resize(total, Value::Null);
        let mut current = Some(Arc::clone(&klass));
        while let Some(owner) = current {
            for field in &owner.instance_field_infos {
                fields[field.slot] = match field.descriptor {
                    b"Z" | b"B" | b"C" | b"S" | b"I" => Value::Int(0),
                    b"J" => Value::Long(0),
                    b"F" => Value::Float(0.0),
                    b"D" => Value::Double(0.0),
                    _ => Value::Null,
                };
            }
            current = owner.super_klass();
        }
        Ok(Self { klass, fields })
    }

    /// Number of instance field slots.
    #[inline]
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// `FieldInfo` declared at byte `offset` in this object's layout,
    /// searching the whole superclass chain (instance fields only).
    /// Returns the declaring class's field info plus the slot index.
    pub fn field_info_by_offset(&self, offset: u32) -> Option<(&InstanceKlass<'a>, &FieldInfo<'a>)> {
        let mut current = Some(self.klass.as_ref());
        while let Some(klass) = current {
            for info in &klass.instance_field_infos {
                if info.offset == offset {
                    return Some((klass, info));
                }
            }
            current = klass.super_klass.as_ref().map(|arc| arc.as_ref());
        }
        None
    }

    /// Slot index of the instance field at byte `offset`, if any.
    pub fn field_slot_by_offset(&self, offset: u32) -> Option<usize> {
        self.field_info_by_offset(offset).map(|(_, info)| info.slot)
    }

    /// Validates that `value` is storable in a field of `kind` and
    /// narrows integer values to the field's descriptor width
    /// (Z/B/C/S semantics, same coercion the interpreter applies).
    fn coerce_value(&self, descriptor: &[u8], kind: FieldKind, value: Value<'a>) -> io::Result<Value<'a>> {
        match kind {
            FieldKind::Int32 => {
                let Value::Int(v) = value else {
                    return Err(io::Error::other("Invalid int32 field value"));
                };
                Ok(match descriptor {
                    b"Z" => Value::Int(v & 1),
                    b"B" => Value::Int(i32::from(v as i8)),
                    b"C" => Value::Int(i32::from(v as u16)),
                    b"S" => Value::Int(i32::from(v as i16)),
                    _ => Value::Int(v),
                })
            }
            FieldKind::Int64 => match value {
                Value::Long(v) => Ok(Value::Long(v)),
                Value::Int(v) => Ok(Value::Long(i64::from(v))),
                _ => Err(io::Error::other("Invalid int64 field value")),
            },
            FieldKind::Float32 => match value {
                Value::Float(v) => Ok(Value::Float(v)),
                Value::Int(v) => Ok(Value::Float(v as f32)),
                _ => Err(io::Error::other("Invalid float32 field value")),
            },
            FieldKind::Float64 => match value {
                Value::Double(v) => Ok(Value::Double(v)),
                Value::Float(v) => Ok(Value::Double(f64::from(v))),
                Value::Int(v) => Ok(Value::Double(v as f64)),
                _ => Err(io::Error::other("Invalid float64 field value")),
            },
            FieldKind::Ref => match value {
                Value::Int(_)
                | Value::Long(_)
                | Value::Float(_)
                | Value::Double(_)
                | Value::Empty
                | Value::Output
                | Value::ReturnAddress(_) => {
                    Err(io::Error::other("Invalid reference field value"))
                }
                other => Ok(other),
            },
        }
    }

    /// Reads the instance field at byte `offset` (HotSpot
    /// `oopDesc::obj_field_addr` + typed load analogue used by JIT
    /// stubs).  `kind` selects the expected machine type; a
    /// `descriptor_kind` mismatch against the field's declared
    /// descriptor is an error.
    pub fn get_field_at_offset(&self, offset: u32, kind: FieldKind) -> io::Result<Value<'a>> {
        let (_, info) = self
            .field_info_by_offset(offset)
            .ok_or_else(|| io::Error::other("No instance field at offset"))?;
        let declared = descriptor_kind(info.descriptor)
            .ok_or_else(|| io::Error::other("Invalid field descriptor"))?;
        if declared != kind {
            return Err(io::Error::other("Field kind mismatch"));
        }
        self.fields
            .get(info.slot)
            .copied()
            .ok_or_else(|| io::Error::other("Invalid instance field slot"))
    }

    /// Writes the instance field at byte `offset` with kind checking
    /// and descriptor-width coercion.
    pub fn set_field_at_offset(
        &mut self,
        offset: u32,
        kind: FieldKind,
        value: Value<'a>,
    ) -> io::Result<()> {
        let (_, info) = self
            .field_info_by_offset(offset)
            .ok_or_else(|| io::Error::other("No instance field at offset"))?;
        let declared = descriptor_kind(info.descriptor)
            .ok_or_else(|| io::Error::other("Invalid field descriptor"))?;
        if declared != kind {
            return Err(io::Error::other("Field kind mismatch"));
        }
        let coerced = self.coerce_value(info.descriptor, kind, value)?;
        *self
            .fields
            .get_mut(info.slot)
            .ok_or_else(|| io::Error::other("Invalid instance field slot"))? = coerced;
        Ok(())
    }

    /// Reads the instance field identified by `name`/`descriptor`,
    /// walking the superclass chain (`InstanceKlass::instanceField`
    /// analogue returning the stored `Value`).
    pub fn get_field(&self, name: &[u8], descriptor: &[u8]) -> io::Result<Value<'a>> {
        let slot = self.klass.instanceField(name, descriptor)?;
        self.fields
            .get(slot)
            .copied()
            .ok_or_else(|| io::Error::other("Invalid instance field slot"))
    }

    /// Writes the instance field identified by `name`/`descriptor`.
    pub fn set_field(
        &mut self,
        name: &[u8],
        descriptor: &[u8],
        value: Value<'a>,
    ) -> io::Result<()> {
        let slot = self.klass.instanceField(name, descriptor)?;
        let kind = descriptor_kind(descriptor)
            .ok_or_else(|| io::Error::other("Invalid field descriptor"))?;
        let coerced = self.coerce_value(descriptor, kind, value)?;
        *self
            .fields
            .get_mut(slot)
            .ok_or_else(|| io::Error::other("Invalid instance field slot"))? = coerced;
        Ok(())
    }

    /// Typed read by `FieldInfo` (JIT/runtime stub path): resolves the
    /// slot from the resolved field entry's byte offset.
    pub fn get_field_by_offset_info(&self, offset: u32) -> io::Result<Value<'a>> {
        let kind = self
            .field_info_by_offset(offset)
            .and_then(|(_, info)| info.kind())
            .ok_or_else(|| io::Error::other("Unknown field offset"))?;
        self.get_field_at_offset(offset, kind)
    }
}
