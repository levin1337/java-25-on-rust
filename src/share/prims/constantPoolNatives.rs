//! Native methods behind `jdk.internal.reflect.ConstantPool`, ported from
//! HotSpot `JVM_ConstantPool*` entries in share/prims/jvm.cpp.
//!
//! `Class.getConstantPool()` stores the owning class mirror in the
//! `constantPoolOop` field; every native below resolves that mirror back to
//! the parsed `InstanceKlass` and reads its constant pool.

use super::reflectionNatives;
use crate::share::{
    classfile::{
        classFileParser::{ClassFile, Constant, Member},
        classLoader::ClassLoader,
    },
    interpreter::bytecodeInterpreter::Value,
    oops::instanceKlass::InstanceKlass,
};
use std::io::{self, Error};
use std::sync::Arc;

fn invalid(message: &'static str) -> Error {
    Error::other(message)
}

fn iae<'a>(loader: &ClassLoader<'a>, message: &str) -> Error {
    reflectionNatives::throw_java(loader, b"java/lang/IllegalArgumentException", Some(message))
}

fn bounds<'a>(loader: &ClassLoader<'a>, index: usize, len: usize) -> io::Result<()> {
    if index == 0 || index >= len {
        return Err(reflectionNatives::throw_java(
            loader,
            b"java/lang/IndexOutOfBoundsException",
            None,
        ));
    }
    Ok(())
}

fn utf8_at<'a>(class: &ClassFile<'a>, index: u16) -> Option<&'a [u8]> {
    match class.constants.get(usize::from(index))? {
        Constant::Utf8(bytes) => Some(bytes),
        _ => None,
    }
}

fn class_name_at<'a>(class: &ClassFile<'a>, index: u16) -> Option<&'a [u8]> {
    let Constant::Class(name_index) = class.constants.get(usize::from(index))? else {
        return None;
    };
    utf8_at(class, *name_index)
}

fn name_and_type<'a>(
    class: &ClassFile<'a>,
    index: u16,
) -> Option<(&'a [u8], &'a [u8])> {
    let Constant::NameAndType(name_index, descriptor_index) =
        class.constants.get(usize::from(index))?
    else {
        return None;
    };
    Some((utf8_at(class, *name_index)?, utf8_at(class, *descriptor_index)?))
}

/// `(class_index, name_and_type_index)` shared by all reference constants.
fn ref_parts(constant: &Constant<'_>) -> Option<(u16, u16)> {
    match constant {
        Constant::FieldRef(class_index, name_and_type)
        | Constant::MethodRef(class_index, name_and_type)
        | Constant::InterfaceMethodRef(class_index, name_and_type) => {
            Some((*class_index, *name_and_type))
        }
        _ => None,
    }
}

fn tag_of(constant: &Constant<'_>) -> i32 {
    match constant {
        Constant::Unusable => 0,
        Constant::Utf8(_) => 1,
        Constant::Integer(_) => 3,
        Constant::Float(_) => 4,
        Constant::Long(_) => 5,
        Constant::Double(_) => 6,
        Constant::Class(_) => 7,
        Constant::String(_) => 8,
        Constant::FieldRef(..) => 9,
        Constant::MethodRef(..) => 10,
        Constant::InterfaceMethodRef(..) => 11,
        Constant::NameAndType(..) => 12,
        Constant::MethodHandle(..) => 15,
        Constant::MethodType(..) => 16,
        Constant::Dynamic(..) | Constant::InvokeDynamic(..) => 18,
        Constant::Module(_) => 19,
        Constant::Package(_) => 20,
    }
}

fn pool<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Arc<InstanceKlass<'a>>> {
    let name = match arguments.get(1).copied() {
        Some(Value::ClassMirror(name)) => name,
        Some(Value::Object(handle)) => loader.class_object_names.borrow().get(&handle).copied()
            .ok_or_else(|| invalid("Invalid constant pool object"))?,
        _ => return Err(invalid("Invalid constant pool object")),
    };
    loader.load(name)
}

fn index_of(arguments: &[Value<'_>]) -> usize {
    match arguments.get(2).copied().unwrap_or(Value::Null) {
        Value::Int(value) => usize::try_from(value).unwrap_or(0),
        Value::Long(value) => usize::try_from(value).unwrap_or(0),
        _ => 0,
    }
}

fn string_array<'a>(
    loader: &ClassLoader<'a>,
    texts: &[&[u8]],
) -> io::Result<Value<'a>> {
    let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
        texts.len() as i32,
        b"java/lang/String",
    )?;
    for (index, text) in texts.iter().enumerate() {
        let text = String::from_utf8_lossy(text).into_owned();
        array.elements[index] = Value::HostString(loader.storage.alloc_str(&text));
    }
    let mut arrays = loader.obj_arrays.borrow_mut();
    let handle = arrays.len();
    arrays.push(array);
    Ok(Value::ObjectArray(handle))
}

fn member_at<'a>(
    loader: &ClassLoader<'a>,
    class: &ClassFile<'a>,
    index: usize,
    force: bool,
) -> io::Result<Option<(Arc<InstanceKlass<'a>>, Member<'a>, bool)>> {
    let Some(constant) = class.constants.get(index) else {
        return Err(invalid("Invalid constant pool index"));
    };
    let is_method = matches!(
        constant,
        Constant::MethodRef(..) | Constant::InterfaceMethodRef(..)
    );
    let is_field = matches!(constant, Constant::FieldRef(..));
    if !is_method && !is_field {
        return Err(iae(
            loader,
            "Wrong type at constant pool index",
        ));
    }
    let Some((class_index, nt_index)) = ref_parts(constant) else {
        return Err(invalid("Invalid reference constant"));
    };
    let Some(target_name) = class_name_at(class, class_index) else {
        return Err(invalid("Invalid class reference"));
    };
    if !force && !loader.hasClass(target_name)? {
        return Ok(None);
    }
    let target = loader.load(target_name)?;
    let Some((name, descriptor)) = name_and_type(class, nt_index) else {
        return Err(invalid("Invalid name and type"));
    };
    if is_method {
        if let Some((holder, method_index)) = target.lookup_method(name, descriptor) {
            let member = holder.class.methods[method_index].clone();
            return Ok(Some((holder, member, is_method)));
        }
        // Interface method references resolve through itables, not the
        // superclass chain; search declared interfaces as a fallback.
        for interface in &target.class.interfaces {
            if let Ok(interface_klass) = loader.load(*interface)
                && let Some((holder, method_index)) =
                    interface_klass.lookup_method(name, descriptor)
            {
                let member = holder.class.methods[method_index].clone();
                return Ok(Some((holder, member, true)));
            }
        }
        return Err(reflectionNatives::throw_java(
            loader,
            b"java/lang/RuntimeException",
            Some("Unable to look up method in target class"),
        ));
    }
    let mut current = Some(Arc::clone(&target));
    while let Some(klass) = current {
        if let Some(member) = klass.declared_field(name, descriptor) {
            let member = member.clone();
            return Ok(Some((klass, member, false)));
        }
        current = klass.super_klass();
    }
    Err(reflectionNatives::throw_java(
        loader,
        b"java/lang/RuntimeException",
        Some("Unable to look up field in target class"),
    ))
}

fn member_value<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    member: &Member<'a>,
    slot: i32,
    is_method: bool,
) -> io::Result<Value<'a>> {
    if !is_method {
        let handle = reflectionNatives::new_field_object(loader, klass, member)?;
        return Ok(Value::Object(handle));
    }
    if member.name == b"<init>" {
        let handle = reflectionNatives::new_constructor_object(loader, klass, member, slot)?;
        return Ok(Value::Object(handle));
    }
    let handle = reflectionNatives::new_method_object(loader, klass, member, slot)?;
    Ok(Value::Object(handle))
}

/// `jdk.internal.reflect.ConstantPool` instance natives. `arguments` holds
/// `[receiver, constantPoolOop, index]` (`getSize0` omits the index).
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let _ = descriptor;
    if !name.ends_with(b"0") {
        return Ok(None);
    }
    let klass = pool(loader, arguments)?;
    let class = &klass.class;
    let index = index_of(arguments);
    let value = match name {
        b"getSize0" => Value::Int(class.constants.len() as i32),
        b"getTagAt0" => {
            bounds(loader, index, class.constants.len())?;
            Value::Int(tag_of(&class.constants[index]))
        }
        b"getClassAt0" | b"getClassAtIfLoaded0" => {
            bounds(loader, index, class.constants.len())?;
            let force = name == b"getClassAt0";
            let Some(constant) = class.constants.get(index) else {
                return Err(invalid("Invalid constant pool index"));
            };
            let is_klass = matches!(constant, Constant::Class(_));
            if !is_klass {
                return Err(iae(loader, "Wrong type at constant pool index"));
            }
            let Constant::Class(name_index) = constant else {
                return Err(invalid("Invalid class constant"));
            };
            let Some(class_name) = utf8_at(class, *name_index) else {
                return Err(invalid("Invalid class name"));
            };
            if !force && !loader.hasClass(class_name)? {
                Value::Null
            } else {
                Value::ClassMirror(class_name)
            }
        }
        b"getClassRefIndexAt0" => {
            bounds(loader, index, class.constants.len())?;
            let Some(constant) = class.constants.get(index) else {
                return Err(invalid("Invalid constant pool index"));
            };
            let Some((class_index, _)) = ref_parts(constant) else {
                return Err(iae(loader, "Wrong type at constant pool index"));
            };
            Value::Int(i32::from(class_index))
        }
        b"getMethodAt0" | b"getMethodAtIfLoaded0" => {
            bounds(loader, index, class.constants.len())?;
            match member_at(loader, class, index, name == b"getMethodAt0")? {
                Some((holder, member, is_method)) => {
                    let slot = holder
                        .class
                        .methods
                        .iter()
                        .position(|m| m.name == member.name && m.descriptor == member.descriptor)
                        .unwrap_or(0) as i32;
                    member_value(loader, &holder, &member, slot, is_method)?
                }
                None => Value::Null,
            }
        }
        b"getFieldAt0" | b"getFieldAtIfLoaded0" => {
            bounds(loader, index, class.constants.len())?;
            match member_at(loader, class, index, name == b"getFieldAt0")? {
                Some((holder, member, is_method)) => {
                    member_value(loader, &holder, &member, 0, is_method)?
                }
                None => Value::Null,
            }
        }
        b"getMemberRefInfoAt0" => {
            bounds(loader, index, class.constants.len())?;
            let Some(constant) = class.constants.get(index) else {
                return Err(invalid("Invalid constant pool index"));
            };
            let Some((class_index, nt_index)) = ref_parts(constant) else {
                return Err(iae(loader, "Wrong type at constant pool index"));
            };
            let Some(class_name) = class_name_at(class, class_index) else {
                return Err(invalid("Invalid class reference"));
            };
            let Some((member_name, member_sig)) = name_and_type(class, nt_index) else {
                return Err(invalid("Invalid name and type"));
            };
            string_array(loader, &[class_name, member_name, member_sig])?
        }
        b"getNameAndTypeRefIndexAt0" => {
            bounds(loader, index, class.constants.len())?;
            let Some(constant) = class.constants.get(index) else {
                return Err(invalid("Invalid constant pool index"));
            };
            let nt_index = match constant {
                Constant::FieldRef(_, nt)
                | Constant::MethodRef(_, nt)
                | Constant::InterfaceMethodRef(_, nt)
                | Constant::InvokeDynamic(_, nt)
                | Constant::Dynamic(_, nt) => *nt,
                _ => {
                    return Err(iae(loader, "Wrong type at constant pool index"));
                }
            };
            Value::Int(i32::from(nt_index))
        }
        b"getNameAndTypeRefInfoAt0" => {
            bounds(loader, index, class.constants.len())?;
            let Some(Constant::NameAndType(..)) = class.constants.get(index) else {
                return Err(iae(loader, "Wrong type at constant pool index"));
            };
            let Some((member_name, member_sig)) =
                name_and_type(class, u16::try_from(index).map_err(|_| invalid("Index too large"))?)
            else {
                return Err(invalid("Invalid name and type"));
            };
            string_array(loader, &[member_name, member_sig])?
        }
        b"getIntAt0" => {
            bounds(loader, index, class.constants.len())?;
            match class.constants[index] {
                Constant::Integer(value) => Value::Int(value),
                _ => return Err(iae(loader, "Wrong type at constant pool index")),
            }
        }
        b"getLongAt0" => {
            bounds(loader, index, class.constants.len())?;
            match class.constants[index] {
                Constant::Long(value) => Value::Long(value),
                _ => return Err(iae(loader, "Wrong type at constant pool index")),
            }
        }
        b"getFloatAt0" => {
            bounds(loader, index, class.constants.len())?;
            match class.constants[index] {
                Constant::Float(value) => Value::Float(value),
                _ => return Err(iae(loader, "Wrong type at constant pool index")),
            }
        }
        b"getDoubleAt0" => {
            bounds(loader, index, class.constants.len())?;
            match class.constants[index] {
                Constant::Double(value) => Value::Double(value),
                _ => return Err(iae(loader, "Wrong type at constant pool index")),
            }
        }
        b"getStringAt0" => {
            bounds(loader, index, class.constants.len())?;
            match class.constants[index] {
                Constant::String(utf8_index) => {
                    let Some(text) = utf8_at(class, utf8_index) else {
                        return Err(invalid("Invalid string constant"));
                    };
                    let text = String::from_utf8_lossy(text).into_owned();
                    Value::HostString(loader.storage.alloc_str(&text))
                }
                _ => return Err(iae(loader, "Wrong type at constant pool index")),
            }
        }
        b"getUTF8At0" => {
            bounds(loader, index, class.constants.len())?;
            match class.constants[index] {
                Constant::Utf8(text) => {
                    let text = String::from_utf8_lossy(text).into_owned();
                    Value::HostString(loader.storage.alloc_str(&text))
                }
                _ => return Err(iae(loader, "Wrong type at constant pool index")),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

pub(crate) fn is_owner(owner: &[u8]) -> bool {
    owner == b"jdk/internal/reflect/ConstantPool"
}
