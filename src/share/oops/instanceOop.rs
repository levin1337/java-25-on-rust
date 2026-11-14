use super::instanceKlass::InstanceKlass;
use crate::share::interpreter::bytecodeInterpreter::Value;
use std::{io, rc::Rc};

pub struct InstanceOop<'a> {
    pub klass: Rc<InstanceKlass<'a>>,
    pub fields: Vec<Value<'a>>,
}

impl<'a> InstanceOop<'a> {
    pub fn new(klass: Rc<InstanceKlass<'a>>) -> io::Result<Self> {
        let total = klass.total_instance_fields;
        let mut fields = Vec::with_capacity(total);
        fields.resize(total, Value::Null);
        let mut current = Some(Rc::clone(&klass));
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
}
