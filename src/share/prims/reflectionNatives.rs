/*
 * Reflection native methods ported from HotSpot's reflection support
 * (share/runtime/reflection.cpp and the JVM_Reflection natives).
 *
 * Implements the VM side of java.lang.Class member enumeration
 * (getDeclaredMethods0/getDeclaredFields0/getDeclaredConstructors0 and
 * friends) plus the invocation entry points used by core reflection:
 * NativeMethodAccessorImpl.invoke0 and NativeConstructorAccessorImpl
 * .newInstance0. Member objects are real java.lang.reflect.* instances
 * whose fields are populated from parsed class-file metadata.
 */

use super::stringNatives;
use crate::share::{
    classfile::{classFileParser::{Attribute, Constant, Member}, classLoader::ClassLoader},
    interpreter::{
        bytecodeInterpreter::{self, JavaThrowable, Value},
        linkResolver::LinkResolver,
    },
    memory::oopFactory::OopFactory,
    oops::{
        instanceKlass::InstanceKlass, instanceOop::InstanceOop, objArrayOop::ObjArrayOop,
        typeArrayOop::TypeArrayOop,
    },
    runtime::{javaCalls::{JavaCallArguments, JavaCalls}, reflection::Reflection},
};
use std::io::{self, Error};
use std::sync::Arc;

type NativeResult<'a> = Option<Option<Value<'a>>>;

/// `Class.forName0` fallback for names missing from the VM's flat class
/// store: invokes `ClassLoader.loadClass(String)` on the Java loader object
/// passed as the third argument, letting URLClassLoader/custom loaders run
/// their own `findClass`. Returns `Ok(None)` when no usable loader object
/// was supplied.
fn delegate_load_class<'a>(
    loader: &ClassLoader<'a>,
    loader_arg: Value<'a>,
    class_name: &str,
) -> io::Result<Option<Value<'a>>> {
    let Value::Object(handle) = loader_arg else {
        return Ok(None);
    };
    let klass = {
        let objects = loader.borrow_objects();
        let Some(object) = objects.get(handle) else {
            return Ok(None);
        };
        Arc::clone(&object.klass)
    };
    if klass.class.name != b"java/lang/ClassLoader"
        && !crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(
            klass.as_ref(),
            b"java/lang/ClassLoader",
        )
    {
        return Ok(None);
    }
    let name = loader.storage.alloc_str(class_name);
    let mut call = JavaCallArguments::with_receiver(Value::Object(handle));
    call.push_oop(Value::HostString(name));
    let result = JavaCalls::call_virtual(
        loader,
        &klass,
        b"loadClass",
        b"(Ljava/lang/String;)Ljava/lang/Class;",
        &call,
        &mut io::sink(),
    )?;
    Ok(Some(result.unwrap_or(Value::Null)))
}

const ACC_PUBLIC: u16 = 0x0001;
const ACC_PRIVATE: u16 = 0x0002;
const ACC_STATIC: u16 = 0x0008;
const ACC_FINAL: u16 = 0x0010;
const ACC_NATIVE: u16 = 0x0100;
const ACC_INTERFACE: u16 = 0x0200;
const ACC_ABSTRACT: u16 = 0x0400;
const ACC_SYNTHETIC: u32 = 0x1000;
const ACC_RECORD: u32 = 0x10000;
const ACC_ENUM: u32 = 0x4000;

fn invalid(message: &'static str) -> Error {
    Error::other(message)
}

pub(crate) fn iae<'a>(loader: &ClassLoader<'a>, message: &str) -> Error {
    throw_java(loader, b"java/lang/IllegalArgumentException", Some(message))
}

fn npe<'a>(loader: &ClassLoader<'a>) -> Error {
    throw_java(loader, b"java/lang/NullPointerException", None)
}

/// Converts a `ClassMirror` name into a field descriptor.
fn mirror_descriptor(name: &[u8]) -> Vec<u8> {
    if name.starts_with(b":") {
        name[1..].to_vec()
    } else if name.starts_with(b"[") {
        name.to_vec()
    } else {
        let mut descriptor = Vec::with_capacity(name.len() + 2);
        descriptor.push(b'L');
        descriptor.extend_from_slice(name);
        descriptor.push(b';');
        descriptor
    }
}

/// Converts a field descriptor into the `ClassMirror` name form.
fn descriptor_mirror<'a>(descriptor: &'a [u8]) -> &'a [u8] {
    if let Some(inner) = descriptor
        .strip_prefix(b"L")
        .and_then(|d| d.strip_suffix(b";"))
    {
        inner
    } else if descriptor.len() == 1 && !descriptor.starts_with(b"[") {
        match descriptor[0] {
            b'Z' => b":Z",
            b'B' => b":B",
            b'C' => b":C",
            b'S' => b":S",
            b'I' => b":I",
            b'J' => b":J",
            b'F' => b":F",
            b'D' => b":D",
            b'V' => b":V",
            _ => descriptor,
        }
    } else {
        descriptor
    }
}

/// Splits a method descriptor into parameter descriptors and the return
/// descriptor.
fn method_descriptor(descriptor: &[u8]) -> io::Result<(Vec<&[u8]>, &[u8])> {
    let invalid = || Error::other("Invalid method descriptor");
    let body = descriptor.strip_prefix(b"(").ok_or_else(invalid)?;
    let close = body.iter().position(|byte| *byte == b')').ok_or_else(invalid)?;
    let mut parameters = Vec::new();
    let mut rest = &body[..close];
    while !rest.is_empty() {
        let len = field_descriptor_len(rest).ok_or_else(invalid)?;
        parameters.push(&rest[..len]);
        rest = &rest[len..];
    }
    let result = &body[close + 1..];
    if result.is_empty() {
        return Err(invalid());
    }
    if field_descriptor_len(result) != Some(result.len()) {
        return Err(invalid());
    }
    Ok((parameters, result))
}

fn field_descriptor_len(descriptor: &[u8]) -> Option<usize> {
    let mut index = 0;
    while descriptor.get(index) == Some(&b'[') {
        index += 1;
    }
    match *descriptor.get(index)? {
        b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' | b'V' => Some(index + 1),
        b'L' => {
            let end = descriptor[index + 1..]
                .iter()
                .position(|byte| *byte == b';')?;
            Some(index + 1 + end + 1)
        }
        _ => None,
    }
}

fn dotted_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name).replace('/', ".")
}

fn class_display_name(name: &[u8]) -> String {
    if let Some(primitive) = name.strip_prefix(b":") {
        return match primitive {
            b"Z" => "boolean".to_owned(),
            b"B" => "byte".to_owned(),
            b"C" => "char".to_owned(),
            b"S" => "short".to_owned(),
            b"I" => "int".to_owned(),
            b"J" => "long".to_owned(),
            b"F" => "float".to_owned(),
            b"D" => "double".to_owned(),
            _ => "void".to_owned(),
        };
    }
    dotted_name(name)
}

pub(crate) fn string_value<'a>(loader: &ClassLoader<'a>, text: &str) -> Value<'a> {
    Value::HostString(loader.storage.alloc_str(text))
}

fn java_string<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<String> {
    let Some(text) = stringNatives::string(loader, value)? else {
        if std::env::var_os("JVM25_TRACE_STR").is_some() {
            eprintln!(
                "[str-fail-refl] {:?}",
                match value {
                    Value::Null => "Null",
                    Value::Int(_) => "Int",
                    Value::Long(_) => "Long",
                    Value::Float(_) => "Float",
                    Value::Double(_) => "Double",
                    Value::Object(_) => "Object",
                    Value::ObjectArray(_) => "ObjectArray",
                    Value::PrimitiveArray(_) => "PrimitiveArray",
                    Value::String(_) => "String",
                    Value::HostString(_) => "HostString",
                    Value::ClassMirror(_) => "ClassMirror",
                    _ => "other",
                }
            );
        }
        return Err(invalid("Expected java.lang.String"));
    };
    Ok(text.into_owned())
}

/// Builds a `Class[]` object array holding the given internal class names.
pub(crate) fn class_array<'a>(loader: &ClassLoader<'a>, names: &[&'a [u8]]) -> io::Result<Value<'a>> {
    let mut array = ObjArrayOop::withComponent(names.len() as i32, b"java/lang/Class")?;
    for (index, name) in names.iter().enumerate() {
        array.elements[index] = Value::ClassMirror(name);
    }
    let mut arrays = loader.obj_arrays.borrow_mut();
    let handle = arrays.len();
    arrays.push(array);
    Ok(Value::ObjectArray(handle))
}

/// Allocates a real `java.lang.reflect.*` object and populates the fields
/// that identify the reflected member.
fn new_member_object<'a>(
    loader: &ClassLoader<'a>,
    class_name: &[u8],
    fields: &[(&[u8], &[u8], Value<'a>)],
) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0);
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    for &(name, descriptor, value) in fields {
        if let Ok(slot) = klass.instanceField(name, descriptor) {
            let mut objects = loader.borrow_objects_mut();
            if let Some(object) = objects.get_mut(handle)
                && slot < object.fields.len()
            {
                object.fields[slot] = value;
            }
        }
    }
    Ok(handle)
}

/// Creates a throwable object of `class_name` using the given constructor
/// descriptor/arguments.
fn new_throwable<'a>(
    loader: &ClassLoader<'a>,
    class_name: &[u8],
    ctor: &[u8],
    args: &[Value<'a>],
) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0);
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(handle));
    let result = (|| -> io::Result<()> {
        let member = klass.class.methods.iter().find(|method| {
            method.name == b"<init>"
                && method.descriptor == ctor
                && method.accessFlags & ACC_STATIC == 0
        });
        if let Some(member) = member {
            let code = crate::share::oops::method::Code::parse(member)?;
            let mut call_args = Vec::with_capacity(args.len() + 1);
            call_args.push(Value::Object(handle));
            call_args.extend_from_slice(args);
            bytecodeInterpreter::execute(
                loader,
                &klass,
                code,
                &call_args,
                &mut io::sink(),
                0,
                member.accessFlags,
            )?;
        }
        Ok(())
    })();
    let root_position = {
        let roots = loader.gc_extra_roots.borrow();
        roots
            .iter()
            .rposition(|value| matches!(value, Value::Object(root) if *root == handle))
    };
    if let Some(position) = root_position {
        loader.gc_extra_roots.borrow_mut().remove(position);
    }
    result?;
    Ok(handle)
}

/// Throws `class_name` as a Java exception, optionally with a message.
pub(crate) fn throw_java<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: Option<&str>) -> Error {
    let result = (|| -> io::Result<usize> {
        match message {
            Some(text) => {
                let message_value = string_value(loader, text);
                new_throwable(loader, class_name, b"(Ljava/lang/String;)V", &[message_value])
                    .or_else(|_| new_throwable(loader, class_name, b"()V", &[]))
            }
            None => new_throwable(loader, class_name, b"()V", &[]),
        }
    })();
    match result {
        Ok(handle) => Error::other(JavaThrowable(handle)),
        Err(_) => Error::other(dotted_name(class_name)),
    }
}

/// Wraps a throwable object handle into an InvocationTargetException and
/// returns it as a Java-level exception.
fn invocation_target<'a>(loader: &ClassLoader<'a>, target: usize) -> Error {
    match new_throwable(
        loader,
        b"java/lang/reflect/InvocationTargetException",
        b"(Ljava/lang/Throwable;)V",
        &[Value::Object(target)],
    ) {
        Ok(handle) => Error::other(JavaThrowable(handle)),
        Err(_) => Error::other(JavaThrowable(target)),
    }
}

fn member_field<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    klass: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> Option<Value<'a>> {
    let slot = klass.instanceField(name, descriptor).ok()?;
    let objects = loader.borrow_objects();
    objects
        .get(handle)
        .and_then(|object| object.fields.get(slot))
        .copied()
}

fn member_int<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    klass: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<i32> {
    match member_field(loader, handle, klass, name, descriptor) {
        Some(Value::Int(value)) => Ok(value),
        Some(Value::Long(value)) => Ok(value as i32),
        _ => Err(invalid("Invalid reflective member field")),
    }
}

fn set_member_field<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    klass: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
    value: Value<'a>,
) -> io::Result<()> {
    let slot = klass
        .instanceField(name, descriptor)
        .map_err(|_| invalid("Missing member field"))?;
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(handle)
        .ok_or_else(|| invalid("Invalid object"))?;
    if slot >= object.fields.len() {
        return Err(invalid("Member field slot out of range"));
    }
    object.fields[slot] = value;
    Ok(())
}

fn member_string<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Vec<u8>> {
    Ok(java_string(loader, value)?.into_bytes())
}

fn klass_of<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
) -> io::Result<Arc<InstanceKlass<'a>>> {
    let objects = loader.borrow_objects();
    objects
        .get(handle)
        .map(|object| Arc::clone(&object.klass))
        .ok_or_else(|| invalid("Invalid object"))
}

/// Extracts the `Class[]` mirrors stored in an executable member field.
fn class_array_field<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    klass: &InstanceKlass<'a>,
    name: &[u8],
) -> io::Result<Vec<&'a [u8]>> {
    let value = member_field(loader, handle, klass, name, b"[Ljava/lang/Class;")
        .unwrap_or(Value::Null);
    let Value::ObjectArray(array) = value else {
        return Ok(Vec::new());
    };
    let arrays = loader.obj_arrays.borrow();
    let array = arrays
        .get(array)
        .ok_or_else(|| invalid("Invalid Class array"))?;
    let mut mirrors = Vec::with_capacity(array.elements.len());
    for element in &array.elements {
        match element {
            Value::ClassMirror(name) => mirrors.push(*name),
            Value::Null => mirrors.push(b"".as_slice()),
            _ => return Err(invalid("Invalid Class array element")),
        }
    }
    Ok(mirrors)
}

struct ExecutableInfo<'a> {
    declaring: &'a [u8],
    name: Vec<u8>,
    parameters: Vec<&'a [u8]>,
    result: &'a [u8],
    modifiers: i32,
}

fn executable_info<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    expected: &[u8],
) -> io::Result<ExecutableInfo<'a>> {
    let Value::Object(handle) = value else {
        return Err(iae(loader, "Expected executable member object"));
    };
    let klass = klass_of(loader, handle)?;
    if klass.class.name != expected {
        return Err(iae(loader, "Expected reflective member object"));
    }
    let declaring = match member_field(loader, handle, &klass, b"clazz", b"Ljava/lang/Class;") {
        Some(Value::ClassMirror(name)) => name,
        _ => return Err(iae(loader, "Invalid member declaring class")),
    };
    let parameters = class_array_field(loader, handle, &klass, b"parameterTypes")?;
    let result_mirror = member_field(loader, handle, &klass, b"returnType", b"Ljava/lang/Class;");
    let modifiers = member_int(loader, handle, &klass, b"modifiers", b"I")?;
    // `java.lang.reflect.Constructor` has no `name` field; its name is the
    // constant "<init>".
    let name = match member_field(loader, handle, &klass, b"name", b"Ljava/lang/String;") {
        Some(value) => member_string(loader, value)?,
        None => Vec::new(),
    };
    let result = match result_mirror {
        Some(Value::ClassMirror(mirror)) => descriptor_mirror(mirror),
        _ => b":V".as_slice(),
    };
    Ok(ExecutableInfo {
        declaring,
        name,
        parameters,
        result,
        modifiers,
    })
}

/// Builds the method descriptor for an executable from its parameter and
/// return type mirrors.
fn executable_descriptor<'a>(info: &ExecutableInfo<'a>) -> Vec<u8> {
    let mut descriptor = Vec::new();
    descriptor.push(b'(');
    for parameter in &info.parameters {
        descriptor.extend_from_slice(&mirror_descriptor(parameter));
    }
    descriptor.push(b')');
    descriptor.extend_from_slice(&mirror_descriptor(info.result));
    descriptor
}

fn object_args<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Vec<Value<'a>>> {
    match value {
        Value::Null | Value::Empty => Ok(Vec::new()),
        Value::ObjectArray(handle) => {
            let arrays = loader.obj_arrays.borrow();
            let array = arrays
                .get(handle)
                .ok_or_else(|| invalid("Invalid argument array"))?;
            Ok(array.elements.clone())
        }
        _ => Err(iae(loader, "Expected Object[] arguments")),
    }
}

/// Unboxes a wrapper object to its primitive `value` field, returning the
/// primitive type tag and the raw value.
fn unbox<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<(u8, Value<'a>)> {
    let Value::Object(handle) = value else {
        return Err(iae(loader, "Argument is not a wrapper object"));
    };
    let (klass, name) = {
        let objects = loader.borrow_objects();
        let object = objects
            .get(handle)
            .ok_or_else(|| invalid("Invalid object"))?;
        (Arc::clone(&object.klass), object.klass.class.name)
    };
    let internal = String::from_utf8_lossy(name);
    let Some(tag) = Reflection::unbox_target_descriptor(internal.as_ref()) else {
        return Err(iae(loader, "Argument is not a wrapper object"));
    };
    let tag = tag.as_bytes()[0];
    let descriptor: &'static [u8] = match tag {
        b'Z' => b"Z",
        b'B' => b"B",
        b'C' => b"C",
        b'S' => b"S",
        b'I' => b"I",
        b'J' => b"J",
        b'F' => b"F",
        b'D' => b"D",
        _ => return Err(iae(loader, "Argument is not a wrapper object")),
    };
    let slot = klass
        .instanceField(b"value", descriptor)
        .map_err(|_| iae(loader, "Wrapper has no value field"))?;
    let objects = loader.borrow_objects();
    let raw = objects
        .get(handle)
        .and_then(|object| object.fields.get(slot))
        .copied()
        .unwrap_or(Value::Null);
    Ok((tag, raw))
}

/// Boxes a primitive value into its wrapper class instance.
fn box_value<'a>(
    loader: &ClassLoader<'a>,
    mirror: &[u8],
    value: Value<'a>,
) -> io::Result<Value<'a>> {
    let Some(tag) = mirror.strip_prefix(b":") else {
        return Ok(value);
    };
    if tag == b"V" {
        return Ok(Value::Null);
    }
    let descriptor: &'static [u8] = match tag {
        b"Z" => b"Z",
        b"B" => b"B",
        b"C" => b"C",
        b"S" => b"S",
        b"I" => b"I",
        b"J" => b"J",
        b"F" => b"F",
        b"D" => b"D",
        _ => return Ok(value),
    };
    let class_name = Reflection::box_class_for_primitive(
        std::str::from_utf8(descriptor).map_err(|_| invalid("Invalid descriptor"))?,
    )
    .ok_or_else(|| invalid("No boxing class"))?;
    let klass = loader.load(class_name.as_bytes())?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0);
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    let slot = klass
        .instanceField(b"value", descriptor)
        .map_err(|_| invalid("Wrapper has no value field"))?;
    let mut objects = loader.borrow_objects_mut();
    if let Some(object) = objects.get_mut(handle)
        && slot < object.fields.len()
    {
        object.fields[slot] = value;
    }
    Ok(Value::Object(handle))
}

/// Converts reflective call arguments to the VM representation expected by
/// the target method: unboxing wrappers, applying JLS widening and
/// reference assignability checks.
fn adapt_arguments<'a>(
    loader: &ClassLoader<'a>,
    parameters: &[&[u8]],
    arguments: &[Value<'a>],
) -> io::Result<Vec<Value<'a>>> {
    if parameters.len() != arguments.len() {
        return Err(iae(loader, "Wrong number of arguments"));
    }
    let mut adapted = Vec::with_capacity(arguments.len());
    for (&descriptor, &argument) in parameters.iter().zip(arguments) {
        let first = *descriptor.first().ok_or_else(|| invalid("Invalid descriptor"))?;
        if descriptor.len() == 1
            && matches!(first, b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z')
        {
            let (from, raw) = unbox(loader, argument)?;
            let raw = if from == first {
                raw
            } else {
                let from_desc = [from];
                let to_desc = [first];
                Reflection::widen_primitive(
                    std::str::from_utf8(&from_desc).map_err(|_| invalid("Invalid type"))?,
                    std::str::from_utf8(&to_desc).map_err(|_| invalid("Invalid type"))?,
                    raw,
                )
                .map_err(|_| iae(loader, "Argument type mismatch"))?
            };
            adapted.push(raw);
        } else {
            let target = if first == b'L' {
                &descriptor[1..descriptor.len().saturating_sub(1)]
            } else {
                descriptor
            };
            let ok = match argument {
                Value::Null => true,
                _ => loader.referenceAssignable(argument, target).unwrap_or(false),
            };
            if !ok {
                return Err(iae(loader, "Argument type mismatch"));
            }
            adapted.push(argument);
        }
    }
    Ok(adapted)
}

fn find_declared<'k, 'a>(
    klass: &'k InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> Option<&'k Member<'a>> {
    klass.class.methods.iter().find(|method| {
        method.name == name
            && method.descriptor == descriptor
            && method.accessFlags & ACC_STATIC == 0
    })
}

fn receiver_klass<'a>(
    loader: &ClassLoader<'a>,
    receiver: Value<'a>,
) -> io::Result<Arc<InstanceKlass<'a>>> {
    match receiver {
        Value::Null => Err(npe(loader)),
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            objects
                .get(handle)
                .map(|object| Arc::clone(&object.klass))
                .ok_or_else(|| npe(loader))
        }
        Value::ClassMirror(_) => loader.load(b"java/lang/Class"),
        Value::String(_) | Value::HostString(_) => loader.load(b"java/lang/String"),
        _ => Err(iae(loader, "Unsupported receiver type")),
    }
}

/// Resolves and executes a reflective method call, returning the boxed
/// result value.
fn invoke_resolved_value<'a>(
    loader: &ClassLoader<'a>,
    info: &ExecutableInfo<'a>,
    receiver: Value<'a>,
    adapted: Vec<Value<'a>>,
) -> io::Result<Value<'a>> {
    let descriptor = executable_descriptor(info);
    let name = info.name.as_slice();
    let static_method = (info.modifiers as u16 & ACC_STATIC) != 0;
    let private_method = (info.modifiers as u16 & ACC_PRIVATE) != 0;
    let result_mirror = info.result;

    let mut call_args = Vec::with_capacity(adapted.len() + 1);
    if !static_method {
        call_args.push(receiver);
    }
    call_args.extend(adapted);

    let target_klass = if static_method {
        let declaring_klass = loader.load(info.declaring)?;
        LinkResolver::resolve_static_call(&declaring_klass, name, &descriptor)
            .map(|(target, _)| target)
            .map_err(|_| iae(loader, "Cannot find static method"))?
    } else if private_method {
        let declaring_klass = loader.load(info.declaring)?;
        if find_declared(&declaring_klass, name, &descriptor).is_none() {
            return Err(iae(loader, "Cannot find private method"));
        }
        Arc::clone(&declaring_klass)
    } else {
        let receiver_klass = receiver_klass(loader, receiver)?;
        LinkResolver::resolve_virtual_call(&receiver_klass, name, &descriptor).map_err(|error| {
            if error.to_string().contains("AbstractMethodError") {
                throw_java(loader, b"java/lang/AbstractMethodError", None)
            } else {
                iae(loader, "Cannot resolve method")
            }
        })?
        .0
    };

    let member = target_klass.class.methods.iter().find(|m| {
        m.name == name
            && m.descriptor == descriptor.as_slice()
            && (m.accessFlags & ACC_STATIC != 0) == static_method
    });
    let Some(member) = member else {
        return Err(iae(loader, "Cannot find method implementation"));
    };

    bytecodeInterpreter::initialize(loader, &target_klass, &mut io::sink(), 0)?;

    if member.accessFlags & ACC_NATIVE != 0 {
        let native_args = call_args.clone();
        let result = if static_method {
            match crate::share::prims::nativeLookup::invokeStatic(
                loader,
                target_klass.class.name,
                name,
                descriptor.as_slice(),
                &native_args,
                &mut io::sink(),
                0,
            )? {
                Some(inner) => inner,
                None => {
                    return Err(throw_java(
                        loader,
                        b"java/lang/UnsatisfiedLinkError",
                        Some("Native method not implemented"),
                    ));
                }
            }
        } else {
            crate::share::prims::nativeLookup::invokeInstance(
                loader,
                target_klass.class.name,
                name,
                descriptor.as_slice(),
                &native_args,
                &mut io::sink(),
                0,
            )?
            .unwrap_or(None)
        };
        return match result {
            Some(value) => box_value(loader, result_mirror, value),
            None => {
                return Err(throw_java(
                    loader,
                    b"java/lang/UnsatisfiedLinkError",
                    Some("Native method not implemented"),
                ));
            }
        };
    }

    let code = crate::share::oops::method::Code::parse(member)?;
    match bytecodeInterpreter::execute(
        loader,
        &target_klass,
        code,
        &call_args,
        &mut io::sink(),
        0,
        member.accessFlags,
    ) {
        Ok(Some(value)) => box_value(loader, result_mirror, value),
        Ok(None) => Ok(Value::Null),
        Err(error) => {
            if let Some(target) = bytecodeInterpreter::throwable_of(&error) {
                Err(invocation_target(loader, target))
            } else {
                Err(error)
            }
        }
    }
}

fn invoke0_inner<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>]) -> io::Result<Value<'a>> {
    let [method, receiver, raw_args] = arguments else {
        return Err(invalid("Invalid invoke0 arguments"));
    };
    let info = executable_info(loader, *method, b"java/lang/reflect/Method")?;
    let args = object_args(loader, *raw_args)?;
    let descriptors: Vec<Vec<u8>> = info
        .parameters
        .iter()
        .map(|parameter| mirror_descriptor(parameter))
        .collect();
    let param_refs: Vec<&[u8]> = descriptors.iter().map(Vec::as_slice).collect();
    let adapted = adapt_arguments(loader, &param_refs, &args)?;
    if matches!(receiver, Value::Null) && (info.modifiers as u16 & ACC_STATIC) == 0 {
        return Err(npe(loader));
    }
    invoke_resolved_value(loader, &info, *receiver, adapted)
}

/// NativeConstructorAccessorImpl.newInstance0(Constructor, Object[]).
pub(crate) fn constructor_new_instance0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let [constructor, raw_args] = arguments else {
        return Err(invalid("Invalid newInstance0 arguments"));
    };
    let Value::Object(handle) = constructor else {
        return Err(iae(loader, "Expected Constructor object"));
    };
    let klass = klass_of(loader, *handle)?;
    if klass.class.name != b"java/lang/reflect/Constructor" {
        return Err(iae(loader, "Expected Constructor object"));
    }
    let declaring = match member_field(loader, *handle, &klass, b"clazz", b"Ljava/lang/Class;") {
        Some(Value::ClassMirror(name)) => name,
        _ => return Err(iae(loader, "Invalid Constructor declaring class")),
    };
    let parameters = class_array_field(loader, *handle, &klass, b"parameterTypes")?;

    let target_klass = loader.load(declaring)?;
    if (target_klass.class.accessFlags & (ACC_ABSTRACT | ACC_INTERFACE)) != 0 {
        return Err(throw_java(
            loader,
            b"java/lang/InstantiationException",
            Some(&dotted_name(declaring)),
        ));
    }

    let args = object_args(loader, *raw_args)?;
    let descriptors: Vec<Vec<u8>> = parameters
        .iter()
        .map(|parameter| mirror_descriptor(parameter))
        .collect();
    let param_refs: Vec<&[u8]> = descriptors.iter().map(Vec::as_slice).collect();
    let adapted = adapt_arguments(loader, &param_refs, &args)?;

    let mut descriptor = Vec::new();
    descriptor.push(b'(');
    for parameter in &parameters {
        descriptor.extend_from_slice(&mirror_descriptor(parameter));
    }
    descriptor.extend_from_slice(b")V");

    let member = target_klass.class.methods.iter().find(|method| {
        method.name == b"<init>"
            && method.descriptor == descriptor.as_slice()
            && method.accessFlags & ACC_STATIC == 0
    });
    let Some(member) = member else {
        return Err(iae(loader, "Cannot find constructor"));
    };

    bytecodeInterpreter::initialize(loader, &target_klass, &mut io::sink(), 0)?;

    let object = InstanceOop::new(target_klass.clone())?;
    let object_handle = loader.alloc_object(object);
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(object_handle));

    let mut call_args = Vec::with_capacity(adapted.len() + 1);
    call_args.push(Value::Object(object_handle));
    call_args.extend(adapted);

    let result = (|| -> io::Result<()> {
        let code = crate::share::oops::method::Code::parse(member)?;
        bytecodeInterpreter::execute(
            loader,
            &target_klass,
            code,
            &call_args,
            &mut io::sink(),
            0,
            member.accessFlags,
        )?;
        Ok(())
    })();

    let root_position = {
        let roots = loader.gc_extra_roots.borrow();
        roots
            .iter()
            .rposition(|value| matches!(value, Value::Object(root) if *root == object_handle))
    };
    if let Some(position) = root_position {
        loader.gc_extra_roots.borrow_mut().remove(position);
    }

    match result {
        Ok(()) => Ok(Some(Value::Object(object_handle))),
        Err(error) => {
            if let Some(target) = bytecodeInterpreter::throwable_of(&error) {
                Err(invocation_target(loader, target))
            } else {
                Err(error)
            }
        }
    }
}

/// `Class.newInstance()`: allocates the class and invokes its no-arg
/// constructor. Used by JDK internals (e.g. charset providers).
fn class_new_instance<'a>(
    loader: &ClassLoader<'a>,
    class: &[u8],
) -> io::Result<Option<Value<'a>>> {
    if class.starts_with(b":") || class.starts_with(b"[") {
        return Err(throw_java(
            loader,
            b"java/lang/InstantiationException",
            None,
        ));
    }
    let target_klass = loader.load(class)?;
    if (target_klass.class.accessFlags & (ACC_ABSTRACT | ACC_INTERFACE)) != 0 {
        return Err(throw_java(
            loader,
            b"java/lang/InstantiationException",
            Some(&dotted_name(class)),
        ));
    }
    let member = target_klass
        .class
        .methods
        .iter()
        .find(|method| {
            method.name == b"<init>"
                && method.descriptor == b"()V"
                && method.accessFlags & ACC_STATIC == 0
        })
        .cloned();
    let Some(member) = member else {
        return Err(throw_java(
            loader,
            b"java/lang/InstantiationException",
            Some(&dotted_name(class)),
        ));
    };

    bytecodeInterpreter::initialize(loader, &target_klass, &mut io::sink(), 0)?;

    let object = InstanceOop::new(target_klass.clone())?;
    let object_handle = loader.alloc_object(object);
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(object_handle));

    let result = (|| -> io::Result<()> {
        let code = crate::share::oops::method::Code::parse(&member)?;
        bytecodeInterpreter::execute(
            loader,
            &target_klass,
            code,
            &[Value::Object(object_handle)],
            &mut io::sink(),
            0,
            member.accessFlags,
        )?;
        Ok(())
    })();

    let root_position = {
        let roots = loader.gc_extra_roots.borrow();
        roots
            .iter()
            .rposition(|value| matches!(value, Value::Object(root) if *root == object_handle))
    };
    if let Some(position) = root_position {
        loader.gc_extra_roots.borrow_mut().remove(position);
    }

    match result {
        Ok(()) => Ok(Some(Value::Object(object_handle))),
        Err(error) => {
            if let Some(target) = bytecodeInterpreter::throwable_of(&error) {
                Err(invocation_target(loader, target))
            } else {
                Err(error)
            }
        }
    }
}

/// InnerClasses attribute lookup: returns (outer class name, inner simple
/// name) for the class represented by `klass`.
fn inner_class_info<'a>(
    klass: &InstanceKlass<'a>,
) -> Option<(Option<&'a [u8]>, Option<&'a [u8]>)> {
    let this_index = class_constant_index(klass)?;
    let attribute = klass
        .class
        .attributes
        .iter()
        .find(|attribute| attribute.name == b"InnerClasses")?;
    let bytes = attribute.bytes;
    if bytes.len() < 2 {
        return None;
    }
    let count = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    for entry in 0..count {
        let base = 2 + entry * 8;
        let slice = bytes.get(base..base + 8)?;
        let inner = u16::from_be_bytes([slice[0], slice[1]]);
        if inner != this_index {
            continue;
        }
        let outer = u16::from_be_bytes([slice[2], slice[3]]);
        let inner_name = u16::from_be_bytes([slice[4], slice[5]]);
        let outer_name = if outer == 0 {
            None
        } else {
            class_name_at(klass, outer)
        };
        let simple = if inner_name == 0 {
            None
        } else {
            utf8_at(klass, inner_name)
        };
        return Some((outer_name, simple));
    }
    None
}

/// Constant pool index (1-based) of the `CONSTANT_Class` naming `klass`.
fn class_constant_index<'a>(klass: &InstanceKlass<'a>) -> Option<u16> {
    for (index, constant) in klass.class.constants.iter().enumerate() {
        if let Constant::Class(name_index) = constant
            && let Some(Constant::Utf8(name)) =
                klass.class.constants.get(usize::from(*name_index))
            && *name == klass.class.name
        {
            return u16::try_from(index).ok();
        }
    }
    None
}

fn class_name_at<'a>(klass: &InstanceKlass<'a>, index: u16) -> Option<&'a [u8]> {
    match klass.class.constants.get(usize::from(index))? {
        Constant::Class(name_index) => utf8_at(klass, *name_index),
        _ => None,
    }
}

fn utf8_at<'a>(klass: &InstanceKlass<'a>, index: u16) -> Option<&'a [u8]> {
    match klass.class.constants.get(usize::from(index))? {
        Constant::Utf8(bytes) => Some(bytes),
        _ => None,
    }
}

/// Access flags from the InnerClasses self-entry (carries `static` for
/// member classes, which is not part of the class-file access_flags).
fn inner_class_access_flags<'a>(klass: &InstanceKlass<'a>) -> u16 {
    let Some(this_index) = class_constant_index(klass) else {
        return 0;
    };
    let Some(attribute) = klass
        .class
        .attributes
        .iter()
        .find(|attribute| attribute.name == b"InnerClasses")
    else {
        return 0;
    };
    let bytes = attribute.bytes;
    if bytes.len() < 2 {
        return 0;
    }
    let count = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    for entry in 0..count {
        let base = 2 + entry * 8;
        let Some(slice) = bytes.get(base..base + 8) else {
            break;
        };
        if u16::from_be_bytes([slice[0], slice[1]]) == this_index {
            return u16::from_be_bytes([slice[6], slice[7]]);
        }
    }
    0
}

/// `Class.getModule()`: resolves the `java.lang.Module` object for the
/// module that defined the class. Array classes defer to their element type;
/// primitives resolve to `java.base`. Falls back to the unnamed module when
/// no named module object has been registered yet.
pub(super) fn class_module_object<'a>(
    loader: &ClassLoader<'a>,
    class: &[u8],
) -> Option<Value<'a>> {
    let mut name = class;
    while let Some(rest) = name.strip_prefix(b"[") {
        name = rest;
    }
    let primitive = name.starts_with(b":")
        || (class.starts_with(b"[")
            && matches!(name, [b'Z' | b'B' | b'C' | b'S' | b'I' | b'J' | b'F' | b'D']));
    if name.starts_with(b"L") && name.ends_with(b";") && name.len() >= 2 {
        name = &name[1..name.len() - 1];
    }
    let module_name = if primitive {
        Some("java.base".to_owned())
    } else {
        loader.class_module_name(name)
    };
    if let Some(module_name) = module_name
        && let Some(handle) = loader.module_object(&module_name)
    {
        return Some(Value::Object(handle));
    }
    loader
        .unnamedModuleObject()
        .ok()
        .map(Value::Object)
}

#[cfg(test)]
#[test]
fn primitiveArraysBelongToJavaBase() {
    let arena = crate::share::runtime::syncBump::SyncBump::new();
    let loader = ClassLoader::open(std::path::Path::new("."), &arena).unwrap();
    loader.register_module_object("java.base", 42);

    for class in [b":I".as_slice(), b"[I", b"[[I"] {
        assert!(matches!(
            class_module_object(&loader, class),
            Some(Value::Object(42))
        ));
    }
}

/// `Class.getModifiers()` value: class access flags merged with the
/// InnerClasses entry flags (JVM_RECOGNIZED_CLASS_MODIFIERS).
pub(crate) fn class_modifiers(loader: &ClassLoader<'_>, class: &[u8]) -> io::Result<i32> {
    if class.starts_with(b":") || class.starts_with(b"[") {
        return Ok(0x0411);
    }
    let klass = loader.load(class)?;
    let inner = inner_class_access_flags(&klass);
    let flags = u32::from(klass.class.accessFlags) | u32::from(inner);
    if std::env::var_os("JVM25_TRACE").is_some() {
        eprintln!(
            "[modifiers] {} access={:#x} inner={:#x} -> {:#x}",
            String::from_utf8_lossy(class),
            klass.class.accessFlags,
            inner,
            flags & 0x761F
        );
    }
    Ok(i32::try_from(flags & 0x761F).unwrap_or(0))
}

/// Member classes declared by `klass` (InnerClasses entries whose outer
/// class is `klass` itself).
fn declared_classes<'a>(klass: &InstanceKlass<'a>) -> Vec<&'a [u8]> {
    let Some(this_index) = class_constant_index(klass) else {
        return Vec::new();
    };
    let Some(attribute) = klass
        .class
        .attributes
        .iter()
        .find(|attribute| attribute.name == b"InnerClasses")
    else {
        return Vec::new();
    };
    let bytes = attribute.bytes;
    if bytes.len() < 2 {
        return Vec::new();
    }
    let count = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    let mut names = Vec::new();
    for entry in 0..count {
        let base = 2 + entry * 8;
        let Some(slice) = bytes.get(base..base + 8) else {
            break;
        };
        let outer = u16::from_be_bytes([slice[2], slice[3]]);
        if outer == this_index
            && let Some(name) = class_name_at(klass, u16::from_be_bytes([slice[0], slice[1]]))
        {
            names.push(name);
        }
    }
    names
}

fn exception_mirrors<'a>(
    klass: &InstanceKlass<'a>,
    member: &Member<'a>,
) -> Vec<&'a [u8]> {
    member
        .exceptions()
        .iter()
        .filter_map(|index| class_name_at(klass, *index))
        .collect()
}

fn parameter_mirrors<'a>(member: &Member<'a>) -> io::Result<Vec<&'a [u8]>> {
    let (parameters, _) = method_descriptor(member.descriptor)?;
    Ok(parameters.iter().map(|parameter| descriptor_mirror(parameter)).collect())
}

fn annotationBytes<'a>(
    loader: &ClassLoader<'a>,
    attributes: &[Attribute<'a>],
    name: &[u8],
) -> io::Result<Value<'a>> {
    let Some(attribute) = attributes.iter().find(|attribute| attribute.name == name) else {
        return Ok(Value::Null);
    };
    let array = OopFactory::new_byte_array_from_bytes(attribute.bytes)?;
    Ok(Value::PrimitiveArray(loader.alloc_type_array(array)))
}

/// Creates a `java.lang.reflect.Method` object for a declared method.
pub(crate) fn new_method_object<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    member: &Member<'a>,
    slot: i32,
) -> io::Result<usize> {
    let (_, result) = method_descriptor(member.descriptor)?;
    let parameters = parameter_mirrors(member)?;
    let parameter_types = class_array(loader, &parameters)?;
    let exceptions = exception_mirrors(klass, member);
    let exception_types = class_array(loader, &exceptions)?;
    let name = Value::String(member.name);
    let fields: Vec<(&[u8], &[u8], Value<'a>)> = vec![
        (b"clazz", b"Ljava/lang/Class;", Value::ClassMirror(klass.class.name)),
        (b"slot", b"I", Value::Int(slot)),
        (b"name", b"Ljava/lang/String;", name),
        (
            b"returnType",
            b"Ljava/lang/Class;",
            Value::ClassMirror(descriptor_mirror(result)),
        ),
        (b"parameterTypes", b"[Ljava/lang/Class;", parameter_types),
        (b"exceptionTypes", b"[Ljava/lang/Class;", exception_types),
        (b"modifiers", b"I", Value::Int(i32::from(member.accessFlags))),
        (b"annotations", b"[B", annotationBytes(loader, &member.attributes, b"RuntimeVisibleAnnotations")?),
        (b"parameterAnnotations", b"[B", annotationBytes(loader, &member.attributes, b"RuntimeVisibleParameterAnnotations")?),
        (b"annotationDefault", b"[B", annotationBytes(loader, &member.attributes, b"AnnotationDefault")?),
    ];
    new_member_object(loader, b"java/lang/reflect/Method", &fields)
}

/// Creates a `java.lang.reflect.Field` object for a declared field.
pub(crate) fn new_field_object<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    member: &Member<'a>,
) -> io::Result<usize> {
    let name = string_value(loader, &String::from_utf8_lossy(member.name));
    let is_final = (member.accessFlags & ACC_FINAL) != 0;
    let trusted_final = is_final && loader.hasClass(klass.class.name).unwrap_or(false);
    let slot = klass
        .instance_field_infos
        .iter()
        .chain(klass.static_field_infos.iter())
        .find(|info| info.name == member.name && info.descriptor == member.descriptor)
        .map(|info| i32::try_from(info.offset).unwrap_or(0))
        .unwrap_or(0);
    let fields: Vec<(&[u8], &[u8], Value<'a>)> = vec![
        (b"clazz", b"Ljava/lang/Class;", Value::ClassMirror(klass.class.name)),
        (b"slot", b"I", Value::Int(slot)),
        (b"name", b"Ljava/lang/String;", name),
        (
            b"type",
            b"Ljava/lang/Class;",
            Value::ClassMirror(descriptor_mirror(member.descriptor)),
        ),
        (b"modifiers", b"I", Value::Int(i32::from(member.accessFlags))),
        (b"trustedFinal", b"Z", Value::Int(i32::from(trusted_final))),
        (b"annotations", b"[B", annotationBytes(loader, &member.attributes, b"RuntimeVisibleAnnotations")?),
    ];
    new_member_object(loader, b"java/lang/reflect/Field", &fields)
}

/// Creates a `java.lang.reflect.Constructor` object.
pub(crate) fn new_constructor_object<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    member: &Member<'a>,
    slot: i32,
) -> io::Result<usize> {
    let parameters = parameter_mirrors(member)?;
    let parameter_types = class_array(loader, &parameters)?;
    let exceptions = exception_mirrors(klass, member);
    let exception_types = class_array(loader, &exceptions)?;
    let fields: Vec<(&[u8], &[u8], Value<'a>)> = vec![
        (b"clazz", b"Ljava/lang/Class;", Value::ClassMirror(klass.class.name)),
        (b"slot", b"I", Value::Int(slot)),
        (b"parameterTypes", b"[Ljava/lang/Class;", parameter_types),
        (b"exceptionTypes", b"[Ljava/lang/Class;", exception_types),
        (b"modifiers", b"I", Value::Int(i32::from(member.accessFlags))),
        (b"annotations", b"[B", annotationBytes(loader, &member.attributes, b"RuntimeVisibleAnnotations")?),
        (b"parameterAnnotations", b"[B", annotationBytes(loader, &member.attributes, b"RuntimeVisibleParameterAnnotations")?),
    ];
    new_member_object(loader, b"java/lang/reflect/Constructor", &fields)
}

/// Builds an object array of reflect member objects.
fn member_array<'a>(
    loader: &ClassLoader<'a>,
    component: &'a [u8],
    handles: Vec<usize>,
) -> io::Result<Value<'a>> {
    let mut array = ObjArrayOop::withComponent(handles.len() as i32, component)?;
    for (index, handle) in handles.into_iter().enumerate() {
        array.elements[index] = Value::Object(handle);
    }
    let mut arrays = loader.obj_arrays.borrow_mut();
    let handle = arrays.len();
    arrays.push(array);
    Ok(Value::ObjectArray(handle))
}

fn public_only(arguments: &[Value<'_>], index: usize) -> bool {
    matches!(arguments.get(index), Some(Value::Int(flags)) if *flags != 0)
}

/// Mirror name of the immediate array component type, mirroring
/// `Class.getComponentType()`.
fn array_component<'b>(class: &'b [u8]) -> Option<&'b [u8]> {
    let component = class.strip_prefix(b"[")?;
    if component.len() == 1 {
        return Some(match component[0] {
            b'Z' => b":Z",
            b'B' => b":B",
            b'C' => b":C",
            b'S' => b":S",
            b'I' => b":I",
            b'J' => b":J",
            b'F' => b":F",
            b'D' => b":D",
            _ => return None,
        });
    }
    if let Some(inner) = component.strip_prefix(b"L")
        && let Some(inner) = inner.strip_suffix(b";")
    {
        return Some(inner);
    }
    if component.starts_with(b"[") {
        return Some(component);
    }
    None
}

/// Innermost (non-array) element type mirror name.
fn array_element(class: &[u8]) -> &[u8] {
    let mut current = class;
    while let Some(component) = current.strip_prefix(b"[") {
        current = component;
    }
    if current.starts_with(b"L")
        && let Some(inner) = current.strip_suffix(b";")
    {
        return inner;
    }
    current
}

fn array_dimensions(class: &[u8]) -> usize {
    class.iter().take_while(|byte| **byte == b'[').count()
}

fn has_attribute(klass: &InstanceKlass<'_>, name: &[u8]) -> bool {
    klass.class.attributes.iter().any(|attribute| attribute.name == name)
}

/// InnerClasses self-entry: (outer class name, simple name) or `None` when
/// the class has no InnerClasses entry for itself (top-level classes).
fn inner_entry<'a>(
    klass: &InstanceKlass<'a>,
) -> Option<(Option<&'a [u8]>, Option<&'a [u8]>)> {
    inner_class_info(klass)
}

/// Extracts a `Class[]` argument into field descriptors for
/// `getMethod`-style member lookups.
fn parameter_classes<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    index: usize,
) -> io::Result<Vec<Vec<u8>>> {
    let mut parameters = Vec::new();
    let Some(Value::ObjectArray(handle)) = arguments.get(index).copied() else {
        return Ok(parameters);
    };
    let arrays = loader.obj_arrays.borrow();
    let Some(array) = arrays.get(handle) else {
        return Err(invalid("Invalid parameter type array"));
    };
    for element in &array.elements {
        match element {
            Value::ClassMirror(name) => parameters.push(mirror_descriptor(name)),
            Value::Null => return Err(npe(loader)),
            _ => return Err(iae(loader, "Argument is not a Class")),
        }
    }
    Ok(parameters)
}

fn member_name<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    index: usize,
) -> io::Result<Vec<u8>> {
    java_string(loader, arguments.get(index).copied().unwrap_or(Value::Null))
        .map(String::into_bytes)
}

fn method_matches(member: &Member<'_>, parameters: &[Vec<u8>]) -> bool {
    let Ok((params, _)) = method_descriptor(member.descriptor) else {
        return false;
    };
    params.len() == parameters.len()
        && params
            .iter()
            .zip(parameters.iter())
            .all(|(expected, actual)| *expected == actual.as_slice())
}

#[allow(dead_code)]
fn is_synthetic(member: &Member<'_>) -> bool {
    u32::from(member.accessFlags) & ACC_SYNTHETIC != 0
}

/// Finds a declared (non-constructor) method by name and erased parameter
/// types. Returns the index into `klass.class.methods`.
fn find_declared_method(
    klass: &InstanceKlass<'_>,
    name: &[u8],
    parameters: &[Vec<u8>],
    public_only: bool,
) -> Option<usize> {
    klass.class.methods.iter().position(|member| {
        member.name == name
            && member.name != b"<init>"
            && member.name != b"<clinit>"
            && (!public_only || member.accessFlags & ACC_PUBLIC != 0)
            && method_matches(member, parameters)
    })
}

/// `Class.getMethod`/`getMethods` lookup: searches the class, its
/// superclasses and its interfaces for a public method.
fn find_public_method<'a>(
    loader: &ClassLoader<'a>,
    class: &'a [u8],
    name: &[u8],
    parameters: &[Vec<u8>],
) -> io::Result<Option<(Arc<InstanceKlass<'a>>, usize)>> {
    let root = if class.starts_with(b"[") || class.starts_with(b":") {
        loader.load(b"java/lang/Object")?
    } else {
        loader.load(class)?
    };
    let mut current = Some(Arc::clone(&root));
    while let Some(klass) = current {
        if let Some(index) = find_declared_method(&klass, name, parameters, true) {
            return Ok(Some((klass, index)));
        }
        current = if klass.class.accessFlags & ACC_INTERFACE == 0 {
            klass.super_klass()
        } else {
            None
        };
    }
    // Interface methods (including defaults) are only visible when no
    // class in the hierarchy provides the method.
    let mut pending: Vec<Arc<InstanceKlass<'a>>> = root.interfaces.clone();
    let mut seen: std::collections::HashSet<&'a [u8]> = std::collections::HashSet::new();
    while let Some(iface) = pending.pop() {
        if !seen.insert(iface.class.name) {
            continue;
        }
        pending.extend(iface.interfaces.iter().cloned());
        if let Some(index) = find_declared_method(&iface, name, parameters, true) {
            return Ok(Some((iface, index)));
        }
    }
    Ok(None)
}

/// Collects every public method visible on `class` (declared, inherited
/// and interface methods), deduplicated by name+descriptor.
fn public_methods<'a>(
    loader: &ClassLoader<'a>,
    class: &'a [u8],
) -> io::Result<Vec<(Arc<InstanceKlass<'a>>, usize)>> {
    let root = if class.starts_with(b"[") || class.starts_with(b":") {
        loader.load(b"java/lang/Object")?
    } else {
        loader.load(class)?
    };
    let mut result: Vec<(Arc<InstanceKlass<'a>>, usize)> = Vec::new();
    let mut seen: std::collections::HashSet<(Vec<u8>, Vec<u8>)> =
        std::collections::HashSet::new();
    let push = |klass: &Arc<InstanceKlass<'a>>,
                    result: &mut Vec<(Arc<InstanceKlass<'a>>, usize)>,
                    seen: &mut std::collections::HashSet<(Vec<u8>, Vec<u8>)>| {
        for (index, member) in klass.class.methods.iter().enumerate() {
            if member.name == b"<init>"
                || member.name == b"<clinit>"
                || member.accessFlags & ACC_PUBLIC == 0
            {
                continue;
            }
            if seen.insert((member.name.to_vec(), member.descriptor.to_vec())) {
                result.push((Arc::clone(klass), index));
            }
        }
    };
    let mut current = Some(Arc::clone(&root));
    while let Some(klass) = current {
        push(&klass, &mut result, &mut seen);
        current = if klass.class.accessFlags & ACC_INTERFACE == 0 {
            klass.super_klass()
        } else {
            None
        };
    }
    let mut pending: Vec<Arc<InstanceKlass<'a>>> = root.interfaces.clone();
    let mut visited: std::collections::HashSet<&'a [u8]> = std::collections::HashSet::new();
    while let Some(iface) = pending.pop() {
        if !visited.insert(iface.class.name) {
            continue;
        }
        pending.extend(iface.interfaces.iter().cloned());
        push(&iface, &mut result, &mut seen);
    }
    Ok(result)
}

fn declared_methods<'a>(
    loader: &ClassLoader<'a>,
    klass: &Arc<InstanceKlass<'a>>,
    public_only: bool,
) -> io::Result<Value<'a>> {
    let mut handles = Vec::new();
    for (index, member) in klass.class.methods.iter().enumerate() {
        if member.name == b"<init>"
            || member.name == b"<clinit>"
            || (public_only && member.accessFlags & ACC_PUBLIC == 0)
        {
            continue;
        }
        handles.push(new_method_object(loader, klass, member, index as i32)?);
    }
    member_array(loader, b"java/lang/reflect/Method", handles)
}

/// Finds a declared field by name. When several fields share the name the
/// first declaration wins, matching `Class.getDeclaredField`.
fn find_declared_field(
    klass: &InstanceKlass<'_>,
    name: &[u8],
    public_only: bool,
) -> Option<usize> {
    klass.class.fields.iter().position(|field| {
        field.name == name
            && (!public_only || field.accessFlags & ACC_PUBLIC != 0)
    })
}

/// `Class.getField` lookup: public fields of the class, its superclasses
/// and its interfaces.
fn find_public_field<'a>(
    loader: &ClassLoader<'a>,
    class: &'a [u8],
    name: &[u8],
) -> io::Result<Option<(Arc<InstanceKlass<'a>>, usize)>> {
    let root = if class.starts_with(b"[") || class.starts_with(b":") {
        loader.load(b"java/lang/Object")?
    } else {
        loader.load(class)?
    };
    let mut current = Some(Arc::clone(&root));
    while let Some(klass) = current {
        if let Some(index) = find_declared_field(&klass, name, true) {
            return Ok(Some((klass, index)));
        }
        current = klass.super_klass();
    }
    let mut pending: Vec<Arc<InstanceKlass<'a>>> = root.interfaces.clone();
    let mut seen: std::collections::HashSet<&'a [u8]> = std::collections::HashSet::new();
    while let Some(iface) = pending.pop() {
        if !seen.insert(iface.class.name) {
            continue;
        }
        pending.extend(iface.interfaces.iter().cloned());
        if let Some(index) = find_declared_field(&iface, name, true) {
            return Ok(Some((iface, index)));
        }
    }
    Ok(None)
}

fn public_fields<'a>(
    loader: &ClassLoader<'a>,
    class: &'a [u8],
) -> io::Result<Vec<(Arc<InstanceKlass<'a>>, usize)>> {
    let root = if class.starts_with(b"[") || class.starts_with(b":") {
        loader.load(b"java/lang/Object")?
    } else {
        loader.load(class)?
    };
    let mut result: Vec<(Arc<InstanceKlass<'a>>, usize)> = Vec::new();
    let mut seen: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    let push = |klass: &Arc<InstanceKlass<'a>>,
                    result: &mut Vec<(Arc<InstanceKlass<'a>>, usize)>,
                    seen: &mut std::collections::HashSet<Vec<u8>>| {
        for (index, field) in klass.class.fields.iter().enumerate() {
            if field.accessFlags & ACC_PUBLIC == 0 {
                continue;
            }
            if seen.insert(field.name.to_vec()) {
                result.push((Arc::clone(klass), index));
            }
        }
    };
    let mut current = Some(Arc::clone(&root));
    while let Some(klass) = current {
        push(&klass, &mut result, &mut seen);
        current = klass.super_klass();
    }
    let mut pending: Vec<Arc<InstanceKlass<'a>>> = root.interfaces.clone();
    let mut visited: std::collections::HashSet<&'a [u8]> = std::collections::HashSet::new();
    while let Some(iface) = pending.pop() {
        if !visited.insert(iface.class.name) {
            continue;
        }
        pending.extend(iface.interfaces.iter().cloned());
        push(&iface, &mut result, &mut seen);
    }
    Ok(result)
}

fn declared_fields<'a>(
    loader: &ClassLoader<'a>,
    klass: &Arc<InstanceKlass<'a>>,
    public_only: bool,
) -> io::Result<Value<'a>> {
    let mut handles = Vec::new();
    for (index, member) in klass.class.fields.iter().enumerate() {
        if public_only && member.accessFlags & ACC_PUBLIC == 0 {
            continue;
        }
        let _ = index;
        handles.push(new_field_object(loader, klass, member)?);
    }
    member_array(loader, b"java/lang/reflect/Field", handles)
}

fn find_constructor<'a>(
    _loader: &ClassLoader<'a>,
    klass: &Arc<InstanceKlass<'a>>,
    parameters: &[Vec<u8>],
    public_only: bool,
) -> Option<usize> {
    klass.class.methods.iter().position(|member| {
        member.name == b"<init>"
            && (!public_only || member.accessFlags & ACC_PUBLIC != 0)
            && method_matches(member, parameters)
    })
}

fn constructors_array<'a>(
    loader: &ClassLoader<'a>,
    klass: &Arc<InstanceKlass<'a>>,
    public_only: bool,
) -> io::Result<Value<'a>> {
    let mut handles = Vec::new();
    for (index, member) in klass.class.methods.iter().enumerate() {
        if member.name != b"<init>" {
            continue;
        }
        if public_only && member.accessFlags & ACC_PUBLIC == 0 {
            continue;
        }
        handles.push(new_constructor_object(loader, klass, member, index as i32)?);
    }
    member_array(loader, b"java/lang/reflect/Constructor", handles)
}

/// `java.lang.Class` instance methods implemented by the VM because the
/// receiver is a `ClassMirror` sentinel rather than a heap object.
fn class_java_method<'a>(
    loader: &ClassLoader<'a>,
    class: &'a [u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let is_array = class.starts_with(b"[");
    let is_primitive = class.starts_with(b":");
    let result = match (name, descriptor) {
        (b"getName", b"()Ljava/lang/String;") => {
            Some(string_value(loader, &class_display_name(class)))
        }
        (b"getPackageName", b"()Ljava/lang/String;") => {
            let package = if is_array {
                let element = array_element(class);
                if element.starts_with(b":") || element.len() == 1 {
                    "java.lang".to_owned()
                } else {
                    let dotted = dotted_name(element);
                    match dotted.rfind('.') {
                        Some(dot) => dotted[..dot].to_owned(),
                        None => String::new(),
                    }
                }
            } else if is_primitive {
                "java.lang".to_owned()
            } else {
                let dotted = dotted_name(class);
                match dotted.rfind('.') {
                    Some(dot) => dotted[..dot].to_owned(),
                    None => String::new(),
                }
            };
            Some(string_value(loader, &package))
        }
        (b"getSimpleName", b"()Ljava/lang/String;") => {
            let simple = if is_primitive {
                class_display_name(class)
            } else if is_array {
                let mut result = String::new();
                let mut current = class;
                while let Some(component) = current.strip_prefix(b"[") {
                    current = component;
                    result.push_str("[]");
                }
                let _ = result;
                // Build "Element[][]..." by recursion on the component.
                fn simple_of<'b>(loader: &ClassLoader<'b>, class: &[u8]) -> String {
                    if let Some(component) = class.strip_prefix(b"[") {
                        format!("{}[]", simple_of(loader, component))
                    } else if let Some(primitive) = class.strip_prefix(b":") {
                        match primitive {
                            b"Z" => "boolean".to_owned(),
                            b"B" => "byte".to_owned(),
                            b"C" => "char".to_owned(),
                            b"S" => "short".to_owned(),
                            b"I" => "int".to_owned(),
                            b"J" => "long".to_owned(),
                            b"F" => "float".to_owned(),
                            b"D" => "double".to_owned(),
                            _ => "void".to_owned(),
                        }
                    } else if let Some(inner) =
                        loader.load(class).ok().and_then(|k| inner_entry(&k)).and_then(|(_, s)| s)
                    {
                        String::from_utf8_lossy(inner).into_owned()
                    } else {
                        let dotted = dotted_name(class);
                        dotted.rsplit('.').next().unwrap_or(&dotted).to_owned()
                    }
                }
                simple_of(loader, class)
            } else if let Some(klass) = loader.load(class).ok()
                && let Some((_, Some(simple))) = inner_entry(&klass)
            {
                String::from_utf8_lossy(simple).into_owned()
            } else {
                let dotted = dotted_name(class);
                dotted.rsplit('.').next().unwrap_or(&dotted).to_owned()
            };
            Some(string_value(loader, &simple))
        }
        (b"getTypeName", b"()Ljava/lang/String;") => {
            if is_array {
                let dimensions = array_dimensions(class);
                let element = array_element(class);
                let element_name = if element.starts_with(b"L") || element.contains(&b'/') {
                    dotted_name(element)
                } else if element.len() == 1 {
                    class_display_name(&[b':', element[0]])
                } else {
                    dotted_name(element)
                };
                Some(string_value(
                    loader,
                    &format!("{element_name}{}", "[]".repeat(dimensions)),
                ))
            } else {
                Some(string_value(loader, &class_display_name(class)))
            }
        }
        (b"getCanonicalName", b"()Ljava/lang/String;") => {
            if is_primitive {
                Some(string_value(loader, &class_display_name(class)))
            } else if is_array {
                match array_component(class).and_then(|c| canonical_name_of(loader, c)) {
                    Some(text) => Some(string_value(loader, &format!("{text}[]"))),
                    None => Some(Value::Null),
                }
            } else {
                canonical_name_of(loader, class).map(|text| string_value(loader, &text))
            }
        }
        (b"getComponentType", b"()Ljava/lang/Class;") => {
            if is_array {
                array_component(class).map(Value::ClassMirror)
            } else {
                Some(Value::Null)
            }
        }
        (b"isAnnotation", b"()Z") => Some(Value::Int(i32::from(
            !is_array
                && !is_primitive
                && loader
                    .load(class)
                    .map(|klass| u32::from(klass.class.accessFlags) & 0x2000 != 0)
                    .unwrap_or(false),
        ))),
        (b"isEnum", b"()Z") => Some(Value::Int(i32::from(
            !is_array
                && !is_primitive
                && loader
                    .load(class)
                    .map(|klass| {
                        u32::from(klass.class.accessFlags) & ACC_ENUM != 0
                            && klass
                                .super_klass()
                                .is_some_and(|super_klass| super_klass.class.name == b"java/lang/Enum")
                    })
                    .unwrap_or(false),
        ))),
        (b"isUnnamedClass", b"()Z") => Some(Value::Int(0)),
        (b"desiredAssertionStatus", b"()Z") => Some(Value::Int(0)),
        (b"getModule", b"()Ljava/lang/Module;") => class_module_object(loader, class),
        (b"getPackage", b"()Ljava/lang/Package;") => {
            // Same package-name derivation as `getPackageName` above.
            let package = if is_array {
                let element = array_element(class);
                if element.starts_with(b":") || element.len() == 1 {
                    "java.lang".to_owned()
                } else {
                    let dotted = dotted_name(element);
                    match dotted.rfind('.') {
                        Some(dot) => dotted[..dot].to_owned(),
                        None => String::new(),
                    }
                }
            } else if is_primitive {
                "java.lang".to_owned()
            } else {
                let dotted = dotted_name(class);
                match dotted.rfind('.') {
                    Some(dot) => dotted[..dot].to_owned(),
                    None => String::new(),
                }
            };
            if package.is_empty() {
                Some(Value::Null)
            } else if let Some(handle) = loader.package_object(&package) {
                Some(Value::Object(handle))
            } else {
                // Lazily create the `java.lang.Package` object: `name` must
                // be a real String (Package.getName() returns it to Java).
                let name_value = string_value(loader, &package);
                let mut sink = io::sink();
                let real_name = match stringNatives::materialize(loader, name_value, &mut sink, 0) {
                    Ok(Some(value)) => value,
                    _ => name_value,
                };
                // `Package.versionInfo` must be a real `Package$VersionInfo`
                // object — getters dereference it unconditionally.
                let version_info = new_member_object(
                    loader,
                    b"java/lang/Package$VersionInfo",
                    &[],
                )
                .ok();
                let mut fields: Vec<(&[u8], &[u8], Value<'a>)> = vec![
                    (b"name", b"Ljava/lang/String;", real_name),
                ];
                if let Some(vi) = version_info {
                    fields.push((
                        b"versionInfo",
                        b"Ljava/lang/Package$VersionInfo;",
                        Value::Object(vi),
                    ));
                }
                if let Some(module) = class_module_object(loader, class) {
                    fields.push((b"module", b"Ljava/lang/Module;", module));
                }
                match new_member_object(loader, b"java/lang/Package", &fields) {
                    Ok(handle) => {
                        loader.register_package_object(&package, handle);
                        Some(Value::Object(handle))
                    }
                    Err(_) => Some(Value::Null),
                }
            }
        }
        (b"getNestHost", b"()Ljava/lang/Class;") => {
            let host = if !is_array
                && !is_primitive
                && let Ok(klass) = loader.load(class)
                && let Some(host) = klass.class.nest_host()
            {
                host
            } else {
                class
            };
            Some(Value::ClassMirror(host))
        }
        (b"getNestMembers", b"()[Ljava/lang/Class;") => {
            if !is_array && !is_primitive && let Ok(klass) = loader.load(class) {
                let mut names: Vec<&[u8]> = Vec::new();
                for index in klass.class.nest_members() {
                    if let Some(name) = class_name_at(&klass, index) {
                        names.push(name);
                    }
                }
                if !names.iter().any(|name| *name == klass.class.name) {
                    names.push(klass.class.name);
                }
                class_array(loader, &names).ok()
            } else {
                class_array(loader, &[class]).ok()
            }
        }
        (b"getRecordComponents", b"()[Ljava/lang/reflect/RecordComponent;") => {
            class_array(loader, &[]).ok()
        }
        // NOTE: getTypeParameters is intentionally NOT intercepted — the JDK
        // implementation parses the Signature attribute via ClassRepository
        // and must return real TypeVariable objects for generics reification.
        (b"getAnnotatedSuperclass", b"()Ljava/lang/reflect/AnnotatedType;") => Some(Value::Null),
        (b"getAnnotatedInterfaces", b"()[Ljava/lang/reflect/AnnotatedType;") => {
            member_array(loader, b"java/lang/reflect/AnnotatedType", Vec::new()).ok()
        }
        (b"getAnnotations", b"()[Ljava/lang/annotation/Annotation;")
        | (b"getDeclaredAnnotations", b"()[Ljava/lang/annotation/Annotation;") => {
            member_array(loader, b"java/lang/annotation/Annotation", Vec::new()).ok()
        }
        (b"getEnumConstants", b"()[Ljava/lang/Object;")
        | (b"getEnumConstantsShared", b"()[Ljava/lang/Object;") => {
            enum_constants(loader, class).ok()
        }
        (b"getMethod", b"(Ljava/lang/String;[Ljava/lang/Class;)Ljava/lang/reflect/Method;") => {
            let name = member_name(loader, arguments, 1)?;
            let parameters = parameter_classes(loader, arguments, 2)?;
            match find_public_method(loader, class, &name, &parameters)? {
                Some((method_klass, index)) => {
                    let member = method_klass.class.methods[index].clone();
                    Some(Value::Object(new_method_object(
                        loader,
                        &method_klass,
                        &member,
                        index as i32,
                    )?))
                }
                None => {
                    return Err(no_such_method(loader, class, &name, &parameters));
                }
            }
        }
        (b"getDeclaredMethod", b"(Ljava/lang/String;[Ljava/lang/Class;)Ljava/lang/reflect/Method;") => {
            let name = member_name(loader, arguments, 1)?;
            let parameters = parameter_classes(loader, arguments, 2)?;
            if is_array || is_primitive {
                return Err(no_such_method(loader, class, &name, &parameters));
            }
            let method_klass = loader.load(class)?;
            match find_declared_method(&method_klass, &name, &parameters, false) {
                Some(index) => {
                    let member = method_klass.class.methods[index].clone();
                    Some(Value::Object(new_method_object(
                        loader,
                        &method_klass,
                        &member,
                        index as i32,
                    )?))
                }
                None => return Err(no_such_method(loader, class, &name, &parameters)),
            }
        }
        (b"getMethods", b"()[Ljava/lang/reflect/Method;") => {
            let methods = public_methods(loader, class)?;
            let mut handles = Vec::with_capacity(methods.len());
            for (method_klass, index) in methods {
                let member = method_klass.class.methods[index].clone();
                handles.push(new_method_object(loader, &method_klass, &member, index as i32)?);
            }
            Some(member_array(loader, b"java/lang/reflect/Method", handles)?)
        }
        (b"getDeclaredMethods", b"()[Ljava/lang/reflect/Method;") => {
            if is_array || is_primitive {
                Some(member_array(loader, b"java/lang/reflect/Method", Vec::new())?)
            } else {
                let method_klass = loader.load(class)?;
                Some(declared_methods(loader, &method_klass, false)?)
            }
        }
        // Internal helper used by Class.getMethods(); returning an empty
        // list is sufficient for bootstrap paths that only probe members.
        (b"getDeclaredPublicMethods", b"(Ljava/lang/String;[Ljava/lang/Class;)Ljava/util/List;") => {
            let klass = loader.load(b"java/util/Collections$EmptyList")?;
            let object = InstanceOop::new(klass)?;
            Some(Value::Object(loader.alloc_object(object)))
        }
        (b"getField", b"(Ljava/lang/String;)Ljava/lang/reflect/Field;") => {
            let name = member_name(loader, arguments, 1)?;
            match find_public_field(loader, class, &name)? {
                Some((field_klass, index)) => {
                    let member = field_klass.class.fields[index].clone();
                    Some(Value::Object(new_field_object(loader, &field_klass, &member)?))
                }
                None => {
                    return Err(throw_java(
                        loader,
                        b"java/lang/NoSuchFieldException",
                        Some(&String::from_utf8_lossy(&name)),
                    ));
                }
            }
        }
        (b"getDeclaredField", b"(Ljava/lang/String;)Ljava/lang/reflect/Field;") => {
            let name = member_name(loader, arguments, 1)?;
            if is_array || is_primitive {
                return Err(throw_java(
                    loader,
                    b"java/lang/NoSuchFieldException",
                    Some(&String::from_utf8_lossy(&name)),
                ));
            }
            let field_klass = loader.load(class)?;
            match find_declared_field(&field_klass, &name, false) {
                Some(index) => {
                    let member = field_klass.class.fields[index].clone();
                    Some(Value::Object(new_field_object(loader, &field_klass, &member)?))
                }
                None => {
                    return Err(throw_java(
                        loader,
                        b"java/lang/NoSuchFieldException",
                        Some(&String::from_utf8_lossy(&name)),
                    ));
                }
            }
        }
        (b"getFields", b"()[Ljava/lang/reflect/Field;") => {
            let fields = public_fields(loader, class)?;
            let mut handles = Vec::with_capacity(fields.len());
            for (field_klass, index) in fields {
                let member = field_klass.class.fields[index].clone();
                handles.push(new_field_object(loader, &field_klass, &member)?);
            }
            Some(member_array(loader, b"java/lang/reflect/Field", handles)?)
        }
        (b"getDeclaredFields", b"()[Ljava/lang/reflect/Field;") => {
            if is_array || is_primitive {
                Some(member_array(loader, b"java/lang/reflect/Field", Vec::new())?)
            } else {
                let field_klass = loader.load(class)?;
                Some(declared_fields(loader, &field_klass, false)?)
            }
        }
        (b"newInstance", b"()Ljava/lang/Object;") => class_new_instance(loader, class)?,
        (b"getConstructor", b"([Ljava/lang/Class;)Ljava/lang/reflect/Constructor;") => {
            let parameters = parameter_classes(loader, arguments, 1)?;
            if is_array || is_primitive {
                return Err(no_such_method(loader, class, b"<init>", &parameters));
            }
            let ctor_klass = loader.load(class)?;
            match find_constructor(loader, &ctor_klass, &parameters, true) {
                Some(index) => {
                    let member = ctor_klass.class.methods[index].clone();
                    Some(Value::Object(new_constructor_object(
                        loader,
                        &ctor_klass,
                        &member,
                        index as i32,
                    )?))
                }
                None => return Err(no_such_method(loader, class, b"<init>", &parameters)),
            }
        }
        (b"getDeclaredConstructor", b"([Ljava/lang/Class;)Ljava/lang/reflect/Constructor;") => {
            let parameters = parameter_classes(loader, arguments, 1)?;
            if is_array || is_primitive {
                return Err(no_such_method(loader, class, b"<init>", &parameters));
            }
            let ctor_klass = loader.load(class)?;
            match find_constructor(loader, &ctor_klass, &parameters, false) {
                Some(index) => {
                    let member = ctor_klass.class.methods[index].clone();
                    Some(Value::Object(new_constructor_object(
                        loader,
                        &ctor_klass,
                        &member,
                        index as i32,
                    )?))
                }
                None => return Err(no_such_method(loader, class, b"<init>", &parameters)),
            }
        }
        (b"getConstructors", b"()[Ljava/lang/reflect/Constructor;") => {
            if is_array || is_primitive {
                Some(member_array(loader, b"java/lang/reflect/Constructor", Vec::new())?)
            } else {
                let ctor_klass = loader.load(class)?;
                Some(constructors_array(loader, &ctor_klass, true)?)
            }
        }
        (b"getDeclaredConstructors", b"()[Ljava/lang/reflect/Constructor;") => {
            if is_array || is_primitive {
                Some(member_array(loader, b"java/lang/reflect/Constructor", Vec::new())?)
            } else {
                let ctor_klass = loader.load(class)?;
                Some(constructors_array(loader, &ctor_klass, false)?)
            }
        }
        (b"asSubclass", b"(Ljava/lang/Class;)Ljava/lang/Class;") => {
            let target = match arguments.get(1).copied().unwrap_or(Value::Null) {
                Value::ClassMirror(target) => target,
                Value::Null => return Err(npe(loader)),
                _ => return Err(iae(loader, "Argument is not a Class")),
            };
            let assignable = if class == target {
                true
            } else if is_primitive || target.starts_with(b":") {
                false
            } else if target.starts_with(b"[") {
                loader.classAssignable(class, target).unwrap_or(false)
            } else {
                let source_klass = loader.load(class)?;
                crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(
                    &source_klass, target,
                ) || crate::share::classfile::systemDictionary::SystemDictionary::implements_interface(
                    &source_klass, target,
                )
            };
            if !assignable {
                return Err(throw_java(
                    loader,
                    b"java/lang/ClassCastException",
                    Some(&format!(
                        "Cannot cast {} to {}",
                        class_display_name(class),
                        class_display_name(target)
                    )),
                ));
            }
            Some(Value::ClassMirror(class))
        }
        (b"cast", b"(Ljava/lang/Object;)Ljava/lang/Object;") => {
            let object = arguments.get(1).copied().unwrap_or(Value::Null);
            let instance = !matches!(object, Value::Null)
                && loader.referenceAssignable(object, class).unwrap_or(false);
            if !instance {
                return Err(throw_java(
                    loader,
                    b"java/lang/ClassCastException",
                    Some(&format!(
                        "Cannot cast {} to {}",
                        match object {
                            Value::Null => "null".to_owned(),
                            _ => "object".to_owned(),
                        },
                        class_display_name(class)
                    )),
                ));
            }
            Some(object)
        }
        _ => None,
    };
    Ok(result)
}

fn no_such_method<'a>(
    loader: &ClassLoader<'a>,
    class: &[u8],
    name: &[u8],
    parameters: &[Vec<u8>],
) -> Error {
    let mut signature = String::new();
    signature.push_str(&class_display_name(class));
    signature.push('.');
    signature.push_str(&String::from_utf8_lossy(name));
    signature.push('(');
    for parameter in parameters {
        signature.push_str(&class_display_name(parameter));
    }
    signature.push(')');
    throw_java(
        loader,
        b"java/lang/NoSuchMethodException",
        Some(&signature),
    )
}

fn canonical_name_of(loader: &ClassLoader<'_>, class: &[u8]) -> Option<String> {
    if class.starts_with(b":") {
        return Some(class_display_name(class));
    }
    if class.starts_with(b"[") {
        let component = array_component(class)?;
        let inner = canonical_name_of(loader, component)?;
        return Some(format!("{inner}[]"));
    }
    let klass = loader.load(class).ok()?;
    // Local and anonymous classes have no canonical name.
    if has_attribute(&klass, b"EnclosingMethod")
        || matches!(inner_entry(&klass), Some((None, _)))
    {
        return None;
    }
    match inner_entry(&klass) {
        Some((Some(outer), Some(simple))) => {
            let outer_canonical = canonical_name_of(loader, outer)?;
            Some(format!(
                "{outer_canonical}.{}",
                String::from_utf8_lossy(simple)
            ))
        }
        Some((Some(_), None)) => None,
        _ => Some(dotted_name(class)),
    }
}

/// Collects the enum constant objects declared by an enum class.
fn enum_constants<'a>(loader: &ClassLoader<'a>, class: &'a [u8]) -> io::Result<Value<'a>> {
    let mut handles = Vec::new();
    let mut is_enum = false;
    if !class.starts_with(b"[")
        && !class.starts_with(b":")
        && let Ok(klass) = loader.load(class)
        && u32::from(klass.class.accessFlags) & ACC_ENUM != 0
        && klass
            .super_klass()
            .is_some_and(|super_klass| super_klass.class.name == b"java/lang/Enum")
    {
        // Enum constant fields are populated by <clinit>; a bare `Level.class`
        // literal does not initialize the class, so run it before reading the
        // static fields (JVMS: values() would trigger initialization).
        let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
            loader,
            &klass,
            &mut io::sink(),
            0,
        );
        is_enum = true;
        let self_descriptor = format!("L{};", String::from_utf8_lossy(class));
        let self_descriptor = loader.storage.alloc_str(&self_descriptor);
        for field in &klass.class.fields {
            if field.descriptor == self_descriptor.as_bytes()
                && field.accessFlags & ACC_STATIC != 0
                && field.accessFlags & ACC_FINAL != 0
            {
                if let Ok(cell) = klass.staticField(field.name, field.descriptor) {
                    if let Value::Object(handle) = cell.get() {
                        handles.push(handle);
                    }
                }
            }
        }
    }
    if !is_enum {
        return Ok(Value::Null);
    }
    member_array(loader, class, handles)
}

fn primitive_array_kind(descriptor: &[u8]) -> Option<u8> {
    // Tags follow the interpreter's `opcode - 0x2e` scheme that
    // `TypeArrayOop::load/store` expects: 0=int, 1=long, 2=float, 3=double,
    // 5=byte/boolean, 6=char, 7=short.
    match descriptor {
        b"[Z" | b"[B" => Some(5),
        b"[C" => Some(6),
        b"[S" => Some(7),
        b"[I" => Some(0),
        b"[J" => Some(1),
        b"[F" => Some(2),
        b"[D" => Some(3),
        _ => None,
    }
}

fn primitive_component_mirror(descriptor: &[u8]) -> Option<&'static [u8]> {
    match descriptor {
        b"[Z" => Some(b":Z"),
        b"[B" => Some(b":B"),
        b"[C" => Some(b":C"),
        b"[S" => Some(b":S"),
        b"[I" => Some(b":I"),
        b"[J" => Some(b":J"),
        b"[F" => Some(b":F"),
        b"[D" => Some(b":D"),
        _ => None,
    }
}

fn array_aioobe<'a>(loader: &ClassLoader<'a>) -> Error {
    throw_java(loader, b"java/lang/ArrayIndexOutOfBoundsException", None)
}

fn array_iae<'a>(loader: &ClassLoader<'a>, message: &str) -> Error {
    throw_java(loader, b"java/lang/IllegalArgumentException", Some(message))
}

fn new_array_value<'a>(
    loader: &ClassLoader<'a>,
    component: &'a [u8],
    length: i32,
) -> io::Result<Value<'a>> {
    if length < 0 {
        return Err(throw_java(
            loader,
            b"java/lang/NegativeArraySizeException",
            None,
        ));
    }
    if let Some(tag) = component.strip_prefix(b":") {
        let kind = match tag {
            b"Z" => 4,
            b"C" => 5,
            b"F" => 6,
            b"D" => 7,
            b"B" => 8,
            b"S" => 9,
            b"I" => 10,
            b"J" => 11,
            _ => return Err(array_iae(loader, "Invalid component type")),
        };
        let array = TypeArrayOop::new(kind, length)?;
        return Ok(Value::PrimitiveArray(loader.alloc_type_array(array)));
    }
    let component = if let Some(inner) = component
        .strip_prefix(b"L")
        .and_then(|inner| inner.strip_suffix(b";"))
    {
        inner
    } else {
        component
    };
    let array = ObjArrayOop::withComponent(length, component)?;
    Ok(Value::ObjectArray(loader.alloc_obj_array(array)))
}

fn array_length<'a>(loader: &ClassLoader<'a>, array: Value<'a>) -> io::Result<i32> {
    match array {
        Value::PrimitiveArray(handle) => Ok(loader
            .arrays
            .borrow()
            .get(handle)
            .map(|array| array.len() as i32)
            .unwrap_or(0)),
        Value::ObjectArray(handle) => Ok(loader
            .obj_arrays
            .borrow()
            .get(handle)
            .map(|array| array.len() as i32)
            .unwrap_or(0)),
        _ => Err(array_iae(loader, "Argument is not an array")),
    }
}

/// `java.lang.Class` instance native methods.
pub(crate) fn class_invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    if std::env::var_os("JVM25_TRACE").is_some() {
        eprintln!(
            "[class_invoke] {} {} args={}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor),
            arguments.len()
        );
    }
    let receiver = arguments.first().copied().unwrap_or(Value::Null);
    // Heap `java.lang.Class` objects carry a populated `protectionDomain`
    // field; keep the handle so natives like `getProtectionDomain0` can
    // read it back without re-materializing.
    let receiver_object = match receiver {
        Value::Object(handle) => Some(handle),
        _ => None,
    };
    // Accept materialized `java.lang.Class` objects as well as `ClassMirror`
    // literals by recovering the internal class name.
    let receiver = match receiver {
        Value::Object(handle) => {
            let is_class_object = {
                let objects = loader.borrow_objects();
                objects.get(handle).is_some_and(|object| {
                    object.klass.class.name == b"java/lang/Class"
                })
            };
            if is_class_object
                && let Some(name) = loader.class_object_names.borrow().get(&handle)
            {
                Value::ClassMirror(*name)
            } else {
                receiver
            }
        }
        _ => receiver,
    };
    let Value::ClassMirror(class) = receiver else {
        return Err(invalid("Expected java.lang.Class receiver"));
    };
    let is_array = class.starts_with(b"[");
    let is_primitive = class.starts_with(b":");
    let klass = if is_array || is_primitive {
        None
    } else {
        Some(loader.load(class)?)
    };

    if let Some(value) = class_java_method(loader, class, name, descriptor, arguments)? {
        return Ok(Some(value));
    }

    let value = match (name, descriptor) {
        (b"initClassName", b"()Ljava/lang/String;") => {
            return Ok(Some(string_value(loader, &class_display_name(class))));
        }
        (b"getSuperclass", b"()Ljava/lang/Class;") => {
            if is_array {
                Value::ClassMirror(b"java/lang/Object")
            } else if is_primitive {
                Value::Null
            } else {
                let klass = klass.as_ref().unwrap();
                if klass.is_interface() || klass.class.name == b"java/lang/Object" {
                    Value::Null
                } else {
                    match klass.super_klass() {
                        Some(super_klass) => Value::ClassMirror(super_klass.class.name),
                        None => Value::Null,
                    }
                }
            }
        }
        (b"getInterfaces0", b"()[Ljava/lang/Class;") => {
            if is_array {
                let cloneable = loader.storage.alloc_slice_copy(b"java/lang/Cloneable");
                let serializable = loader.storage.alloc_slice_copy(b"java/io/Serializable");
                let names: Vec<&[u8]> = vec![cloneable, serializable];
                class_array(loader, &names)?
            } else if is_primitive {
                class_array(loader, &[])?
            } else {
                let klass = klass.as_ref().unwrap();
                let names: Vec<&[u8]> = klass
                    .interfaces
                    .iter()
                    .map(|interface| interface.class.name)
                    .collect();
                class_array(loader, &names)?
            }
        }
        (b"getModifiers", b"()I") => Value::Int(class_modifiers(loader, class)?),
        (b"isInstance", b"(Ljava/lang/Object;)Z") => {
            let arg = arguments.get(1).copied().unwrap_or(Value::Null);
            let result = !matches!(arg, Value::Null)
                && loader.referenceAssignable(arg, class).unwrap_or(false);
            return Ok(Some(Value::Int(i32::from(result))));
        }
        (b"isAssignableFrom", b"(Ljava/lang/Class;)Z") => {
            let source = match arguments.get(1).copied().unwrap_or(Value::Null) {
                Value::ClassMirror(source) => source,
                Value::Null => return Err(npe(loader)),
                _ => return Ok(Some(Value::Int(0))),
            };
            let assignable = if class == source {
                true
            } else if is_primitive || source.starts_with(b":") {
                false
            } else if source.starts_with(b"[") {
                loader.classAssignable(source, class).unwrap_or(false)
            } else {
                let source_klass = loader.load(source)?;
                crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(
                    &source_klass, class,
                ) || crate::share::classfile::systemDictionary::SystemDictionary::implements_interface(
                    &source_klass, class,
                )
            };
            return Ok(Some(Value::Int(i32::from(assignable))));
        }
        (b"isInterface", b"()Z") => Value::Int(i32::from(
            !is_array && !is_primitive && klass.as_ref().unwrap().is_interface(),
        )),
        (b"isArray", b"()Z") => Value::Int(i32::from(is_array)),
        (b"isPrimitive", b"()Z") => Value::Int(i32::from(is_primitive)),
        (b"descriptorString", b"()Ljava/lang/String;") => {
            let descriptor = mirror_descriptor(class);
            return Ok(Some(string_value(
                loader,
                &String::from_utf8_lossy(&descriptor),
            )));
        }
        (b"isHidden", b"()Z") => Value::Int(0),
        (b"isRecord0", b"()Z") => Value::Int(i32::from(
            klass
                .as_ref()
                .is_some_and(|klass| u32::from(klass.class.accessFlags) & ACC_RECORD != 0),
        )),
        (b"getSigners", b"()[Ljava/lang/Object;") => class_array(loader, &[])?,
        (b"setSigners", b"([Ljava/lang/Object;)V") => return Ok(Some(Value::Null)),
        (b"getEnclosingMethod0", b"()[Ljava/lang/Object;") => Value::Null,
        (b"getDeclaringClass0", b"()Ljava/lang/Class;")
        | (b"getDeclaringClass", b"()Ljava/lang/Class;") => {
            if is_array || is_primitive {
                Value::Null
            } else {
                match inner_class_info(klass.as_ref().unwrap()).and_then(|(outer, _)| outer) {
                    Some(outer) => Value::ClassMirror(outer),
                    None => Value::Null,
                }
            }
        }
        (b"getSimpleBinaryName0", b"()Ljava/lang/String;") => {
            if is_array || is_primitive {
                Value::Null
            } else {
                match inner_class_info(klass.as_ref().unwrap()).and_then(|(_, inner)| inner) {
                    Some(inner) => string_value(loader, &String::from_utf8_lossy(inner)),
                    None => Value::Null,
                }
            }
        }
        (b"getProtectionDomain0", b"()Ljava/security/ProtectionDomain;") => {
            // `java.lang.Class.protectionDomain` is populated when the
            // mirror object is materialized; reuse the receiver object
            // when the caller already has one, otherwise materialize.
            let class_object = match receiver_object {
                Some(handle) => Value::Object(handle),
                None => match crate::share::prims::classNatives::materialize_class(
                    loader,
                    receiver,
                )? {
                    Some(value) => value,
                    None => Value::Null,
                },
            };
            let handle = match class_object {
                Value::Object(handle) => handle,
                _ => return Ok(Some(Value::Null)),
            };
            let objects = loader.borrow_objects();
            let field_value = match objects.get(handle) {
                Some(object) => match object.klass.instanceField(
                    b"protectionDomain",
                    b"Ljava/security/ProtectionDomain;",
                ) {
                    Ok(slot) if slot < object.fields.len() => object.fields[slot],
                    _ => Value::Null,
                },
                None => Value::Null,
            };
            if !matches!(field_value, Value::Null) {
                return Ok(Some(field_value));
            }
            if let Some(klass) = klass.as_ref()
                && let Some(pd) = loader.class_protection_domain(klass.class.name)
            {
                return Ok(Some(Value::Object(pd)));
            }
            if let Some(klass) = klass.as_ref() {
                eprintln!(
                    "[pd-null] {} recorded={} origin={}",
                    String::from_utf8_lossy(klass.class.name),
                    loader
                        .class_protection_domain(klass.class.name)
                        .is_some(),
                    loader.class_origin(klass.class.name).is_some()
                );
            }
            Value::Null
        }
        (b"getGenericSignature0", b"()Ljava/lang/String;") => {
            if is_array || is_primitive {
                Value::Null
            } else {
                if std::env::var_os("JVM25_TRACE").is_some() {
                    let names: Vec<String> = klass.as_ref().unwrap().class.attributes.iter()
                        .map(|a| String::from_utf8_lossy(a.name).into_owned()).collect();
                    eprintln!("[gsig0] class={} attrs={names:?}", String::from_utf8_lossy(class));
                }
                let signature = klass.as_ref().unwrap().class.attributes.iter().find_map(|attribute| {
                    if attribute.name != b"Signature" || attribute.bytes.len() < 2 {
                        return None;
                    }
                    let index = u16::from_be_bytes([attribute.bytes[0], attribute.bytes[1]]);
                    utf8_at(klass.as_ref().unwrap(), index)
                });
                match signature {
                    Some(signature) => {
                        if std::env::var_os("JVM25_DEBUG_SIG").is_some() {
                            eprintln!(
                                "[gsig] {} -> [{}]",
                                String::from_utf8_lossy(class),
                                String::from_utf8_lossy(signature)
                            );
                        }
                        string_value(loader, &String::from_utf8_lossy(signature))
                    }
                    None => {
                        if std::env::var_os("JVM25_DEBUG_SIG").is_some() {
                            eprintln!(
                                "[gsig-null] {} attrs={}",
                                String::from_utf8_lossy(class),
                                klass
                                    .as_ref()
                                    .map(|k| k.class.attributes.len())
                                    .unwrap_or(0)
                            );
                        }
                        Value::Null
                    }
                }
            }
        }
        (b"getRawAnnotations", b"()[B") => match &klass {
            Some(klass) => annotationBytes(loader, &klass.class.attributes, b"RuntimeVisibleAnnotations")?,
            None => Value::Null,
        },
        (b"getRawTypeAnnotations", b"()[B") => Value::Null,
        (b"getConstantPool", b"()Ljava/lang/constant/ConstantPool;")
        | (b"getConstantPool", b"()Ljdk/internal/reflect/ConstantPool;") => {
            if is_array || is_primitive {
                Value::Null
            } else {
                new_member_object(
                    loader,
                    b"jdk/internal/reflect/ConstantPool",
                    &[(
                        b"constantPoolOop",
                        b"Ljava/lang/Object;",
                        Value::ClassMirror(class),
                    )],
                )
                .map(Value::Object)?
            }
        }
        (b"getRecordComponents0", b"()[Ljava/lang/reflect/RecordComponent;") => Value::Null,
        (b"getNestHost0", b"()Ljava/lang/Class;") => {
            if is_array || is_primitive {
                Value::Null
            } else {
                match klass
                    .as_ref()
                    .and_then(|klass| klass.class.nest_host())
                {
                    Some(host) => Value::ClassMirror(host),
                    None => Value::ClassMirror(class),
                }
            }
        }
        (b"getNestMembers0", b"()[Ljava/lang/Class;") => {
            if is_array || is_primitive {
                class_array(loader, &[])?
            } else {
                let klass = klass.as_ref().unwrap();
                let mut names: Vec<&[u8]> = Vec::new();
                for index in klass.class.nest_members() {
                    if let Some(name) = class_name_at(klass, index) {
                        names.push(name);
                    }
                }
                if !names.iter().any(|name| *name == klass.class.name) {
                    names.push(klass.class.name);
                }
                class_array(loader, &names)?
            }
        }
        (b"getPermittedSubclasses0", b"()[Ljava/lang/Class;") => {
            if std::env::var_os("JVM25_TRACE_PERMIT").is_some()
                && let Some(k) = klass.as_ref()
            {
                eprintln!(
                    "[permit] {} attrs={:?}",
                    String::from_utf8_lossy(&k.class.name),
                    k.class
                        .attributes
                        .iter()
                        .map(|a| String::from_utf8_lossy(a.name).into_owned())
                        .collect::<Vec<_>>()
                );
            }
            let entries = klass.as_ref().and_then(|klass| {
                let attribute = klass.class.attributes.iter().find(|attribute| attribute.name == b"PermittedSubclasses")?;
                let bytes = attribute.bytes.get(2..)?;
                Some(bytes.chunks_exact(2)
                    .filter_map(|index| class_name_at(klass, u16::from_be_bytes([index[0], index[1]])))
                    .collect::<Vec<_>>())
            });
            match entries {
                Some(entries) => class_array(loader, &entries)?,
                None => Value::Null,
            }
        }
        (b"getClassFileVersion0", b"()I") => Value::Int(i32::from(
            klass.as_ref().map_or(0, |klass| klass.class.majorVersion),
        )),
        (b"getClassAccessFlagsRaw0", b"()I") => Value::Int(i32::from(
            klass.as_ref().map_or(0x0411, |klass| klass.class.accessFlags),
        )),
        (b"getDeclaredClasses0", b"()[Ljava/lang/Class;") => {
            if is_array || is_primitive {
                class_array(loader, &[])?
            } else {
                let names = declared_classes(klass.as_ref().unwrap());
                class_array(loader, &names)?
            }
        }
        (b"getDeclaredFields0", b"(Z)[Ljava/lang/reflect/Field;") => {
            let only_public = public_only(arguments, 1);
            let Some(klass) = &klass else {
                return Ok(Some(class_array(loader, &[])?));
            };
            let mut handles = Vec::new();
            for member in &klass.class.fields {
                if only_public && member.accessFlags & ACC_PUBLIC == 0 {
                    continue;
                }
                handles.push(new_field_object(loader, klass, member)?);
            }
            member_array(loader, b"java/lang/reflect/Field", handles)?
        }
        (b"getDeclaredMethods0", b"(Z)[Ljava/lang/reflect/Method;") => {
            let only_public = public_only(arguments, 1);
            let Some(klass) = &klass else {
                return Ok(Some(class_array(loader, &[])?));
            };
            let mut handles = Vec::new();
            for (index, member) in klass.class.methods.iter().enumerate() {
                if member.name == b"<init>"
                    || member.name == b"<clinit>"
                    || (only_public && member.accessFlags & ACC_PUBLIC == 0)
                {
                    continue;
                }
                handles.push(new_method_object(loader, klass, member, index as i32)?);
            }
            member_array(loader, b"java/lang/reflect/Method", handles)?
        }
        (b"getDeclaredConstructors0", b"(Z)[Ljava/lang/reflect/Constructor;") => {
            let only_public = public_only(arguments, 1);
            let Some(klass) = &klass else {
                return Ok(Some(class_array(loader, &[])?));
            };
            let mut handles = Vec::new();
            for (index, member) in klass.class.methods.iter().enumerate() {
                if member.name != b"<init>"
                    || (only_public && member.accessFlags & ACC_PUBLIC == 0)
                {
                    continue;
                }
                handles.push(new_constructor_object(loader, klass, member, index as i32)?);
            }
            member_array(loader, b"java/lang/reflect/Constructor", handles)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

/// Converts a reflective `Array.set` argument to a primitive element,
/// unboxing wrappers and applying JLS widening conversions.
fn value_to_element<'a>(
    loader: &ClassLoader<'a>,
    tag: u8,
    value: Value<'a>,
) -> io::Result<crate::share::oops::typeArrayOop::Element> {
    let (from, raw) = match value {
        Value::Object(_) => unbox(loader, value)?,
        Value::Int(v) => (b'I', Value::Int(v)),
        Value::Long(v) => (b'J', Value::Long(v)),
        Value::Float(v) => (b'F', Value::Float(v)),
        Value::Double(v) => (b'D', Value::Double(v)),
        _ => return Err(array_iae(loader, "Array element type mismatch")),
    };
    if from != tag {
        let widened = Reflection::widen_primitive(
            std::str::from_utf8(&[from]).map_err(|_| invalid("Invalid type"))?,
            std::str::from_utf8(&[tag]).map_err(|_| invalid("Invalid type"))?,
            raw,
        )
        .map_err(|_| array_iae(loader, "Array element type mismatch"))?;
        return value_to_element(loader, tag, widened);
    }
    match (tag, raw) {
        (b'Z' | b'B' | b'S' | b'C' | b'I', Value::Int(v)) => {
            Ok(crate::share::oops::typeArrayOop::Element::Int(v))
        }
        (b'J', Value::Long(v)) => Ok(crate::share::oops::typeArrayOop::Element::Long(v)),
        (b'F', Value::Float(v)) => Ok(crate::share::oops::typeArrayOop::Element::Float(v)),
        (b'D', Value::Double(v)) => Ok(crate::share::oops::typeArrayOop::Element::Double(v)),
        _ => Err(array_iae(loader, "Array element type mismatch")),
    }
}

/// Converts a typed `Array.setXxx` argument without widening.
fn typed_value_to_element<'a>(
    loader: &ClassLoader<'a>,
    tag: u8,
    value: Value<'a>,
) -> io::Result<crate::share::oops::typeArrayOop::Element> {
    let raw = match value {
        Value::Object(_) => unbox(loader, value)?.1,
        raw => raw,
    };
    match (tag, raw) {
        (b'Z' | b'B' | b'S' | b'C' | b'I', Value::Int(v)) => {
            Ok(crate::share::oops::typeArrayOop::Element::Int(v))
        }
        (b'J', Value::Long(v)) => Ok(crate::share::oops::typeArrayOop::Element::Long(v)),
        (b'F', Value::Float(v)) => Ok(crate::share::oops::typeArrayOop::Element::Float(v)),
        (b'D', Value::Double(v)) => Ok(crate::share::oops::typeArrayOop::Element::Double(v)),
        _ => Err(array_iae(loader, "Array element type mismatch")),
    }
}

fn array_get_typed<'a>(
    loader: &ClassLoader<'a>,
    array: Value<'a>,
    index: usize,
) -> io::Result<Value<'a>> {
    let Value::PrimitiveArray(handle) = array else {
        return Err(array_iae(loader, "Argument is not a primitive array"));
    };
    let arrays = loader.arrays.borrow();
    let Some(array) = arrays.get(handle) else {
        return Err(invalid("Invalid array reference"));
    };
    let descriptor = array.descriptor();
    let Some(kind) = primitive_array_kind(descriptor) else {
        return Err(invalid("Invalid primitive array"));
    };
    let element = array.load(kind, index).map_err(|_| array_aioobe(loader))?;
    Ok(match element {
        crate::share::oops::typeArrayOop::Element::Int(v) => Value::Int(v),
        crate::share::oops::typeArrayOop::Element::Long(v) => Value::Long(v),
        crate::share::oops::typeArrayOop::Element::Float(v) => Value::Float(v),
        crate::share::oops::typeArrayOop::Element::Double(v) => Value::Double(v),
    })
}

fn array_set_typed<'a>(
    loader: &ClassLoader<'a>,
    array: Value<'a>,
    index: usize,
    value: Value<'a>,
) -> io::Result<()> {
    let Value::PrimitiveArray(handle) = array else {
        return Err(array_iae(loader, "Argument is not a primitive array"));
    };
    let (kind, tag) = {
        let arrays = loader.arrays.borrow();
        let Some(array) = arrays.get(handle) else {
            return Err(invalid("Invalid array reference"));
        };
        let descriptor = array.descriptor();
        let Some(kind) = primitive_array_kind(descriptor) else {
            return Err(invalid("Invalid primitive array"));
        };
        (kind, descriptor[1])
    };
    let element = typed_value_to_element(loader, tag, value)?;
    let mut arrays = loader.arrays.borrow_mut();
    let array = arrays
        .get_mut(handle)
        .ok_or_else(|| invalid("Invalid array reference"))?;
    array
        .store(kind, index, element)
        .map_err(|_| array_aioobe(loader))
}

/// Mirror name of `component` wrapped one array dimension deeper.
fn prepend_array_name(name: &[u8]) -> Vec<u8> {
    if name.starts_with(b":") {
        vec![b'[', name[1]]
    } else if name.starts_with(b"[") {
        let mut wrapped = Vec::with_capacity(name.len() + 1);
        wrapped.push(b'[');
        wrapped.extend_from_slice(name);
        wrapped
    } else {
        let mut wrapped = Vec::with_capacity(name.len() + 3);
        wrapped.extend_from_slice(b"[L");
        wrapped.extend_from_slice(name);
        wrapped.push(b';');
        wrapped
    }
}

fn multi_new_array<'a>(
    loader: &ClassLoader<'a>,
    component: &'a [u8],
    dims: &[i32],
    level: usize,
) -> io::Result<Value<'a>> {
    let length = dims[level];
    if level + 1 == dims.len() {
        return new_array_value(loader, component, length);
    }
    let mut name = component.to_vec();
    for _ in 0..(dims.len() - level - 1) {
        name = prepend_array_name(&name);
    }
    let component_name = loader.storage.alloc_slice_copy(&name);
    let array = ObjArrayOop::withComponent(length, component_name)?;
    let handle = loader.alloc_obj_array(array);
    for index in 0..length.max(0) as usize {
        let sub = multi_new_array(loader, component, dims, level + 1)?;
        let mut arrays = loader.obj_arrays.borrow_mut();
        if let Some(array) = arrays.get_mut(handle) {
            array.elements[index] = sub;
        }
    }
    Ok(Value::ObjectArray(handle))
}

fn array_get<'a>(loader: &ClassLoader<'a>, array: Value<'a>, index: usize) -> io::Result<Value<'a>> {
    match array {
        Value::PrimitiveArray(handle) => {
            let (descriptor, element) = {
                let arrays = loader.arrays.borrow();
                let Some(array) = arrays.get(handle) else {
                    return Err(invalid("Invalid array reference"));
                };
                let descriptor = array.descriptor();
                let Some(kind) = primitive_array_kind(descriptor) else {
                    return Err(invalid("Invalid primitive array"));
                };
                let element = array.load(kind, index).map_err(|_| array_aioobe(loader))?;
                (descriptor, element)
            };
            let raw: Value<'a> = match element {
                crate::share::oops::typeArrayOop::Element::Int(v) => Value::Int(v),
                crate::share::oops::typeArrayOop::Element::Long(v) => Value::Long(v),
                crate::share::oops::typeArrayOop::Element::Float(v) => Value::Float(v),
                crate::share::oops::typeArrayOop::Element::Double(v) => Value::Double(v),
            };
            match primitive_component_mirror(descriptor) {
                Some(mirror) => box_value(loader, mirror, raw),
                None => Ok(raw),
            }
        }
        Value::ObjectArray(handle) => {
            let arrays = loader.obj_arrays.borrow();
            let Some(array) = arrays.get(handle) else {
                return Err(invalid("Invalid object array reference"));
            };
            array.elements.get(index).copied().ok_or_else(|| array_aioobe(loader))
        }
        _ => Err(array_iae(loader, "Argument is not an array")),
    }
}

fn array_set<'a>(
    loader: &ClassLoader<'a>,
    array: Value<'a>,
    index: usize,
    value: Value<'a>,
) -> io::Result<()> {
    match array {
        Value::PrimitiveArray(handle) => {
            let (kind, tag) = {
                let arrays = loader.arrays.borrow();
                let Some(array) = arrays.get(handle) else {
                    return Err(invalid("Invalid array reference"));
                };
                let descriptor = array.descriptor();
                let Some(kind) = primitive_array_kind(descriptor) else {
                    return Err(invalid("Invalid primitive array"));
                };
                (kind, descriptor[1])
            };
            let element = value_to_element(loader, tag, value)?;
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            array
                .store(kind, index, element)
                .map_err(|_| array_aioobe(loader))
        }
        Value::ObjectArray(handle) => {
            let mut arrays = loader.obj_arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid object array reference"))?;
            if index >= array.elements.len() {
                return Err(array_aioobe(loader));
            }
            if !matches!(value, Value::Null)
                && !loader.referenceAssignable(value, array.component)?
            {
                return Err(array_iae(loader, "Array element type mismatch"));
            }
            array.elements[index] = value;
            Ok(())
        }
        _ => Err(array_iae(loader, "Argument is not an array")),
    }
}

fn dims_values<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Vec<i32>> {
    let Value::PrimitiveArray(handle) = value else {
        return Err(array_iae(loader, "Argument is not an int array"));
    };
    let arrays = loader.arrays.borrow();
    let Some(array) = arrays.get(handle) else {
        return Err(invalid("Invalid array reference"));
    };
    if array.descriptor() != b"[I" {
        return Err(array_iae(loader, "Argument is not an int array"));
    }
    let kind = primitive_array_kind(b"[I").unwrap();
    let mut dims = Vec::with_capacity(array.len());
    for index in 0..array.len() {
        match array.load(kind, index).map_err(|_| array_aioobe(loader))? {
            crate::share::oops::typeArrayOop::Element::Int(value) => dims.push(value),
            _ => return Err(array_iae(loader, "Argument is not an int array")),
        }
    }
    Ok(dims)
}

/// Static native methods used by reflection and class lookup.
/// `Executable.getParameters0()`: builds the real `Parameter[]` from the
/// method's `MethodParameters` attribute (JVMS §4.7.18) and descriptor,
/// stores `Executable$ParameterData` on the executable, and returns the
/// array. Parameter names absent from the attribute are synthesized as
/// `argN`, matching HotSpot.
fn executable_parameters0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let exec = arguments.first().copied().unwrap_or(Value::Null);
    let Value::Object(handle) = exec else {
        return Err(iae(loader, "Expected executable member object"));
    };
    let exec_klass = klass_of(loader, handle)?;
    let expected: &[u8] = match exec_klass.class.name {
        b"java/lang/reflect/Method" => b"java/lang/reflect/Method",
        b"java/lang/reflect/Constructor" => b"java/lang/reflect/Constructor",
        _ => return Err(iae(loader, "Expected executable member object")),
    };
    let info = executable_info(loader, exec, expected)?;
    let descriptor = executable_descriptor(&info);
    let declaring = loader.load(info.declaring)?;
    let member_name: &[u8] = if expected == b"java/lang/reflect/Constructor" {
        b"<init>"
    } else {
        info.name.as_slice()
    };
    let member = declaring
        .class
        .methods
        .iter()
        .find(|member| member.name == member_name && member.descriptor == descriptor.as_slice());

    // MethodParameters attribute: u1 count, then {u2 name_index, u2 flags}.
    let mut entries: Vec<(Option<Vec<u8>>, u16)> = Vec::new();
    let mut is_real = false;
    if let Some(member) = member
        && let Some(attribute) = member
            .attributes
            .iter()
            .find(|attribute| attribute.name == b"MethodParameters")
    {
        let bytes = attribute.bytes;
        if let Some(&count) = bytes.first() {
            let mut offset = 1;
            for _ in 0..count {
                let Some(slice) = bytes.get(offset..offset + 4) else {
                    break;
                };
                let name_index = u16::from_be_bytes([slice[0], slice[1]]);
                let flags = u16::from_be_bytes([slice[2], slice[3]]);
                offset += 4;
                let name = if name_index == 0 {
                    None
                } else {
                    utf8_at(&declaring, name_index).map(<[u8]>::to_vec)
                };
                entries.push((name, flags));
            }
            is_real = true;
        }
    }

    if std::env::var_os("JVM25_TRACE").is_some() {
        eprintln!(
            "[params0] member={:?} desc={} entries={} real={}",
            member.map(|m| m.name),
            String::from_utf8_lossy(&descriptor),
            entries.len(),
            is_real
        );
    }
    let count = info.parameters.len();
    let mut handles = Vec::with_capacity(count);
    for index in 0..count {
        let (name_bytes, flags) = entries.get(index).cloned().unwrap_or((None, 0));
        let name_bytes = name_bytes.unwrap_or_else(|| format!("arg{index}").into_bytes());
        let name_text = String::from_utf8_lossy(&name_bytes);
        let name_value = string_value(loader, &name_text);
        // JVMS parameter access flags: FINAL(0x0010) | SYNTHETIC(0x1000) |
        // MANDATED(0x8000).
        let modifiers = (flags & 0x9010) as i32;
        let parameter = new_member_object(
            loader,
            b"java/lang/reflect/Parameter",
            &[
                (b"name", b"Ljava/lang/String;", name_value),
                (b"modifiers", b"I", Value::Int(modifiers)),
                (
                    b"executable",
                    b"Ljava/lang/reflect/Executable;",
                    exec,
                ),
                (b"index", b"I", Value::Int(index as i32)),
            ],
        )?;
        handles.push(parameter);
    }
    let parameters = member_array(loader, b"java/lang/reflect/Parameter", handles)?;

    // `parameterData` = new ParameterData(parameters, isReal).
    let parameter_data = new_member_object(
        loader,
        b"java/lang/reflect/Executable$ParameterData",
        &[
            (
                b"parameters",
                b"[Ljava/lang/reflect/Parameter;",
                parameters,
            ),
            (b"isReal", b"Z", Value::Int(is_real as i32)),
        ],
    )?;
    set_member_field(
        loader,
        handle,
        &exec_klass,
        b"parameterData",
        b"Ljava/lang/reflect/Executable$ParameterData;",
        Value::Object(parameter_data),
    )?;
    Ok(parameters)
}

/// Instance-side natives declared on `java.lang.reflect.Executable` and
/// `java.lang.reflect.Field` (resolved to their declaring class by
/// `native_declaring_owner`).
pub(crate) fn member_invoke<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    match (owner, name, descriptor) {
        (
            b"java/lang/reflect/Executable",
            b"getParameters0",
            b"()[Ljava/lang/reflect/Parameter;",
        ) => Ok(Some(Some(executable_parameters0(loader, arguments)?))),
        // Type-use annotation bytes are not parsed by this VM yet; the JDK
        // treats a null result as "no annotations", consistent with
        // Class.getRawAnnotations/getRawTypeAnnotations.
        (b"java/lang/reflect/Executable", b"getTypeAnnotationBytes0", b"()[B")
        | (b"java/lang/reflect/Field", b"getTypeAnnotationBytes0", b"()[B") => {
            Ok(Some(Some(Value::Null)))
        }
        _ => Ok(None),
    }
}

pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<NativeResult<'a>> {
    match (owner, name, descriptor) {
        (
            b"java/lang/Class",
            b"forName0",
            b"(Ljava/lang/String;ZLjava/lang/ClassLoader;Ljava/lang/Class;)Ljava/lang/Class;",
        ) => {
            let class_name =
                java_string(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let initialize = matches!(arguments.get(1), Some(Value::Int(flags)) if *flags != 0);
            let internal = class_name.replace('.', "/");
            if class_name.contains('=') || class_name.contains(':') {
                eprintln!(
                    "[forName0-suspect] name={:?} site={}",
                    class_name,
                    bytecodeInterpreter::current_frame_site()
                );
            }
            if internal.starts_with('[') {
                let mirror = loader.storage.alloc_slice_copy(internal.as_bytes());
                return Ok(Some(Some(Value::ClassMirror(mirror))));
            }
            if matches!(
                internal.as_str(),
                "boolean"
                    | "byte"
                    | "char"
                    | "short"
                    | "int"
                    | "long"
                    | "float"
                    | "double"
                    | "void"
            ) {
                return Err(throw_java(
                    loader,
                    b"java/lang/ClassNotFoundException",
                    Some(&class_name),
                ));
            }
            let klass = match loader.load(internal.as_bytes()) {
                Ok(klass) => klass,
                Err(load_error) => {
                    // Custom Java loaders (URLClassLoader and friends) resolve
                    // classes through their own `findClass`; give the loader
                    // object a chance via `ClassLoader.loadClass` before
                    // reporting ClassNotFoundException.
                    let loader_arg = arguments.get(2).copied().unwrap_or(Value::Null);
                    if let Some(value) =
                        delegate_load_class(loader, loader_arg, &class_name)?
                    {
                        return Ok(Some(Some(value)));
                    }
                    let _ = load_error;
                    return Err(throw_java(
                        loader,
                        b"java/lang/ClassNotFoundException",
                        Some(&class_name),
                    ));
                }
            };
            if initialize {
                bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0)?;
            }
            Ok(Some(Some(Value::ClassMirror(klass.class.name))))
        }
        (b"java/lang/Class", b"desiredAssertionStatus0", b"(Ljava/lang/Class;)Z") => {
            Ok(Some(Some(Value::Int(0))))
        }
        (
            b"jdk/internal/reflect/NativeMethodAccessorImpl",
            b"invoke0",
            b"(Ljava/lang/reflect/Method;Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;",
        )
        | (
            b"jdk/internal/reflect/DirectMethodHandleAccessor$NativeAccessor",
            b"invoke0",
            b"(Ljava/lang/reflect/Method;Ljava/lang/Object;[Ljava/lang/Object;)Ljava/lang/Object;",
        ) => {
            let value = invoke0_inner(loader, arguments)?;
            Ok(Some(Some(value)))
        }
        (
            b"jdk/internal/reflect/NativeConstructorAccessorImpl",
            b"newInstance0",
            b"(Ljava/lang/reflect/Constructor;[Ljava/lang/Object;)Ljava/lang/Object;",
        )
        | (
            b"jdk/internal/reflect/DirectConstructorHandleAccessor$NativeAccessor",
            b"newInstance0",
            b"(Ljava/lang/reflect/Constructor;[Ljava/lang/Object;)Ljava/lang/Object;",
        ) => constructor_new_instance0(loader, arguments)
            .map(|result| result.map(Some)),
        (b"java/lang/reflect/Array", b"getLength", b"(Ljava/lang/Object;)I") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            Ok(Some(Some(Value::Int(array_length(loader, array)?))))
        }
        (b"java/lang/reflect/Array", b"get", b"(Ljava/lang/Object;I)Ljava/lang/Object;") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            let index = match arguments.get(1) {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                Some(Value::Int(_)) => return Err(array_aioobe(loader)),
                _ => return Err(array_iae(loader, "Invalid index")),
            };
            let value = array_get(loader, array, index)?;
            Ok(Some(Some(value)))
        }
        (b"java/lang/reflect/Array", b"set", b"(Ljava/lang/Object;ILjava/lang/Object;)V") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            let index = match arguments.get(1) {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                Some(Value::Int(_)) => return Err(array_aioobe(loader)),
                _ => return Err(array_iae(loader, "Invalid index")),
            };
            let value = arguments.get(2).copied().unwrap_or(Value::Null);
            array_set(loader, array, index, value)?;
            Ok(Some(None))
        }
        (b"java/lang/reflect/Array", b"getBoolean", b"(Ljava/lang/Object;I)Z")
        | (b"java/lang/reflect/Array", b"getByte", b"(Ljava/lang/Object;I)B")
        | (b"java/lang/reflect/Array", b"getChar", b"(Ljava/lang/Object;I)C")
        | (b"java/lang/reflect/Array", b"getShort", b"(Ljava/lang/Object;I)S")
        | (b"java/lang/reflect/Array", b"getInt", b"(Ljava/lang/Object;I)I") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            let index = match arguments.get(1) {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                Some(Value::Int(_)) => return Err(array_aioobe(loader)),
                _ => return Err(array_iae(loader, "Invalid index")),
            };
            let value = array_get_typed(loader, array, index)?;
            Ok(Some(Some(match value {
                Value::Int(v) if name == b"getBoolean" => Value::Int(v & 1),
                other => other,
            })))
        }
        (b"java/lang/reflect/Array", b"getLong", b"(Ljava/lang/Object;I)J") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            let index = match arguments.get(1) {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                Some(Value::Int(_)) => return Err(array_aioobe(loader)),
                _ => return Err(array_iae(loader, "Invalid index")),
            };
            Ok(Some(Some(array_get_typed(loader, array, index)?)))
        }
        (b"java/lang/reflect/Array", b"getFloat", b"(Ljava/lang/Object;I)F") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            let index = match arguments.get(1) {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                Some(Value::Int(_)) => return Err(array_aioobe(loader)),
                _ => return Err(array_iae(loader, "Invalid index")),
            };
            Ok(Some(Some(array_get_typed(loader, array, index)?)))
        }
        (b"java/lang/reflect/Array", b"getDouble", b"(Ljava/lang/Object;I)D") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            let index = match arguments.get(1) {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                Some(Value::Int(_)) => return Err(array_aioobe(loader)),
                _ => return Err(array_iae(loader, "Invalid index")),
            };
            Ok(Some(Some(array_get_typed(loader, array, index)?)))
        }
        (b"java/lang/reflect/Array", b"setBoolean", b"(Ljava/lang/Object;IZ)V")
        | (b"java/lang/reflect/Array", b"setByte", b"(Ljava/lang/Object;IB)V")
        | (b"java/lang/reflect/Array", b"setChar", b"(Ljava/lang/Object;IC)V")
        | (b"java/lang/reflect/Array", b"setShort", b"(Ljava/lang/Object;IS)V")
        | (b"java/lang/reflect/Array", b"setInt", b"(Ljava/lang/Object;II)V")
        | (b"java/lang/reflect/Array", b"setLong", b"(Ljava/lang/Object;IJ)V")
        | (b"java/lang/reflect/Array", b"setFloat", b"(Ljava/lang/Object;IF)V")
        | (b"java/lang/reflect/Array", b"setDouble", b"(Ljava/lang/Object;ID)V") => {
            let array = arguments.first().copied().unwrap_or(Value::Null);
            let index = match arguments.get(1) {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                Some(Value::Int(_)) => return Err(array_aioobe(loader)),
                _ => return Err(array_iae(loader, "Invalid index")),
            };
            let value = arguments.get(2).copied().unwrap_or(Value::Null);
            array_set_typed(loader, array, index, value)?;
            Ok(Some(None))
        }
        (b"java/lang/reflect/Array", b"newArray", b"(Ljava/lang/Class;I)Ljava/lang/Object;") => {
            let component = match arguments.first() {
                Some(Value::ClassMirror(component)) => Some(*component),
                Some(Value::Object(handle)) => loader.class_object_names.borrow().get(handle).copied(),
                _ => None,
            }.ok_or_else(|| array_iae(loader, "Argument is not a Class"))?;
            let length = match arguments.get(1) {
                Some(Value::Int(length)) => *length,
                _ => return Err(array_iae(loader, "Invalid length")),
            };
            let value = new_array_value(loader, component, length)?;
            Ok(Some(Some(value)))
        }
        (b"java/lang/reflect/Array", b"multiNewArray", b"(Ljava/lang/Class;[I)Ljava/lang/Object;") => {
            let component = match arguments.first() {
                Some(Value::ClassMirror(component)) => Some(*component),
                Some(Value::Object(handle)) => loader.class_object_names.borrow().get(handle).copied(),
                _ => None,
            }.ok_or_else(|| array_iae(loader, "Argument is not a Class"))?;
            let dims = dims_values(loader, arguments.get(1).copied().unwrap_or(Value::Null))?;
            if dims.is_empty() {
                return Err(array_iae(loader, "Empty dimensions array"));
            }
            let value = multi_new_array(loader, component, &dims, 0)?;
            Ok(Some(Some(value)))
        }
        _ => Ok(None),
    }
}
