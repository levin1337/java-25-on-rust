/*
 * Port of OpenJDK java.lang.invoke MethodHandleNatives native entry points
 * (java.base/java/lang/invoke/MethodHandleNatives.java).
 *
 * Implements MemberName resolution, field offsets, CallSite target updates,
 * the signature-polymorphic MethodHandle.invoke* family and the VarHandle
 * access-mode family used by java.util.concurrent.atomic and friends.
 */

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Error};
use std::sync::Arc;

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::{
        bytecodeInterpreter::{self, JavaThrowable, Value},
    },
    oops::{instanceKlass::InstanceKlass, instanceOop::InstanceOop, method::Code},
    prims::{nativeLookup, stringNatives},
    runtime::signature,
};

const ACC_STATIC: u16 = 0x0008;
const ACC_NATIVE: u16 = 0x0100;
const ACC_ABSTRACT: u16 = 0x0400;

const IS_METHOD: u32 = 0x10000;
const IS_CONSTRUCTOR: u32 = 0x20000;
const IS_FIELD: u32 = 0x40000;
const IS_TYPE: u32 = 0x80000;
const TRUSTED_FINAL: u32 = 0x200000;
const ALL_KINDS: u32 = 0xF0000;
const HIGH_BITS: u32 = 0x300000;
const REF_KIND_SHIFT: u32 = 24;

const REF_GET_FIELD: u32 = 1;
const REF_GET_STATIC: u32 = 2;
const REF_PUT_FIELD: u32 = 3;
const REF_PUT_STATIC: u32 = 4;
const REF_INVOKE_VIRTUAL: u32 = 5;
const REF_INVOKE_STATIC: u32 = 6;
const REF_INVOKE_SPECIAL: u32 = 7;
const REF_NEW_INVOKE_SPECIAL: u32 = 8;
const REF_INVOKE_INTERFACE: u32 = 9;

fn invalid(message: &'static str) -> Error {
    Error::other(message)
}

fn dotted_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name).replace('/', ".")
}

fn string_value<'a>(loader: &ClassLoader<'a>, text: &str) -> Value<'a> {
    Value::HostString(loader.storage.alloc_str(text))
}

fn java_string<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<String> {
    let Some(text) = stringNatives::string(loader, value)? else {
        if std::env::var_os("JVM25_TRACE_STR").is_some() { eprintln!("[str-fail-mh]"); }
        return Err(invalid("Expected java.lang.String"));
    };
    Ok(text.into_owned())
}

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
            let code = Code::parse(member)?;
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

fn throw_java<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: Option<&str>) -> Error {
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

fn class_not_found<'a>(loader: &ClassLoader<'a>, name: &[u8]) -> Error {
    let message = format!("{}", dotted_name(name));
    throw_java(loader, b"java/lang/ClassNotFoundException", Some(&message))
}

fn wrong_method_type<'a>(loader: &ClassLoader<'a>) -> Error {
    throw_java(loader, b"java/lang/invoke/WrongMethodTypeException", None)
}

fn null_pointer<'a>(loader: &ClassLoader<'a>) -> Error {
    throw_java(loader, b"java/lang/NullPointerException", None)
}

fn no_such_method<'a>(loader: &ClassLoader<'a>, name: &[u8], desc: &[u8]) -> Error {
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(name),
        String::from_utf8_lossy(desc)
    );
    throw_java(loader, b"java/lang/NoSuchMethodError", Some(&message))
}

fn no_such_field<'a>(loader: &ClassLoader<'a>, name: &[u8], desc: &[u8]) -> Error {
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(name),
        String::from_utf8_lossy(desc)
    );
    throw_java(loader, b"java/lang/NoSuchFieldError", Some(&message))
}

fn array_index_oob<'a>(loader: &ClassLoader<'a>) -> Error {
    throw_java(loader, b"java/lang/ArrayIndexOutOfBoundsException", None)
}

fn class_cast<'a>(loader: &ClassLoader<'a>, message: &str) -> Error {
    throw_java(loader, b"java/lang/ClassCastException", Some(message))
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

/// Reads an instance field of a heap object by slot name.
pub(super) fn object_field<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<Value<'a>> {
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid object reference"))?;
    let slot = object.klass.instanceField(name, descriptor)?;
    Ok(object.fields.get(slot).copied().unwrap_or(Value::Empty))
}

pub(super) fn set_object_field<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    name: &[u8],
    descriptor: &[u8],
    value: Value<'a>,
) -> io::Result<()> {
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(handle)
        .ok_or_else(|| invalid("Invalid object reference"))?;
    let slot = object.klass.instanceField(name, descriptor)?;
    if let Some(cell) = object.fields.get_mut(slot) {
        *cell = value;
    }
    Ok(())
}

pub(super) fn object_klass<'a>(loader: &ClassLoader<'a>, handle: usize) -> io::Result<Arc<InstanceKlass<'a>>> {
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid object reference"))?;
    Ok(Arc::clone(&object.klass))
}

/// Extracts the internal class name from a `java.lang.Class` value.
fn class_name<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Vec<u8>> {
    match value {
        Value::ClassMirror(name) => Ok(name.to_vec()),
        Value::Object(handle) => loader
            .class_object_names
            .borrow()
            .get(&handle)
            .map(|name| name.to_vec())
            .ok_or_else(|| invalid("Expected java.lang.Class")),
        _ => Err(invalid("Expected java.lang.Class")),
    }
}

/// Builds the method descriptor for a `java.lang.invoke.MethodType` object.
pub(super) fn method_type_descriptor<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Vec<u8>> {
    let Value::Object(handle) = value else {
        return Err(invalid("Expected java.lang.invoke.MethodType"));
    };
    let (rtype, ptypes) = {
        let objects = loader.borrow_objects();
        let object = objects
            .get(handle)
            .ok_or_else(|| invalid("Invalid object reference"))?;
        let rtype_slot = object.klass.instanceField(b"rtype", b"Ljava/lang/Class;")?;
        let ptypes_slot = object.klass.instanceField(b"ptypes", b"[Ljava/lang/Class;")?;
        (object.fields[rtype_slot], object.fields[ptypes_slot])
    };
    let mut descriptor = Vec::from(&b"("[..]);
    if let Value::ObjectArray(array_handle) = ptypes {
        let arrays = loader.obj_arrays.borrow();
        let array = arrays
            .get(array_handle)
            .ok_or_else(|| invalid("Invalid object array"))?;
        for element in &array.elements {
            let name = class_name(loader, *element)?;
            descriptor.extend_from_slice(&mirror_descriptor(&name));
        }
    }
    descriptor.push(b')');
    let rtype_name = class_name(loader, rtype)?;
    descriptor.extend_from_slice(&mirror_descriptor(&rtype_name));
    Ok(descriptor)
}

/// Parses a method descriptor into parameter kinds plus the return kind.
fn descriptor_kinds(descriptor: &[u8]) -> io::Result<(Vec<u8>, Option<u8>)> {
    let (params, ret) = signature::method(descriptor)?;
    let mut kinds = Vec::new();
    for param in params {
        kinds.push(param?);
    }
    Ok((kinds, ret))
}

/// Reads the fields of a `java.lang.invoke.MemberName` object.
struct MemberState<'a> {
    clazz: Option<Vec<u8>>,
    name: Vec<u8>,
    flags: u32,
    r#type: Value<'a>,
}

fn read_member_state<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
) -> io::Result<MemberState<'a>> {
    let flags_value = object_field(loader, handle, b"flags", b"I")?;
    let flags = flags_value.int()? as u32;
    let clazz_value = object_field(loader, handle, b"clazz", b"Ljava/lang/Class;")?;
    let clazz = match clazz_value {
        Value::ClassMirror(name) => Some(name.to_vec()),
        Value::Null | Value::Empty => None,
        _ => Some(class_name(loader, clazz_value)?),
    };
    let name_value = object_field(loader, handle, b"name", b"Ljava/lang/String;")?;
    let name = match name_value {
        Value::Null | Value::Empty => Vec::new(),
        _ => java_string(loader, name_value)?.into_bytes(),
    };
    let r#type = object_field(loader, handle, b"type", b"Ljava/lang/Object;")?;
    Ok(MemberState {
        clazz,
        name,
        flags,
        r#type,
    })
}

fn member_ref_kind(flags: u32) -> u32 {
    (flags >> REF_KIND_SHIFT) & 0xF
}

fn member_is_field(state: &MemberState<'_>) -> bool {
    let kind = state.flags & ALL_KINDS;
    if kind == IS_FIELD {
        return true;
    }
    if kind == IS_METHOD || kind == IS_CONSTRUCTOR {
        return false;
    }
    member_ref_kind(state.flags) <= REF_PUT_STATIC
}

/// Computes the member descriptor (method descriptor or field descriptor)
/// for a MemberName's `type` field.
fn member_descriptor<'a>(loader: &ClassLoader<'a>, state: &MemberState<'a>) -> io::Result<Vec<u8>> {
    if member_is_field(state) {
        let name = class_name(loader, state.r#type)?;
        Ok(mirror_descriptor(&name))
    } else {
        method_type_descriptor(loader, state.r#type)
    }
}

#[derive(Clone)]
enum SnapshotType {
    Object(usize),
    ClassName(Vec<u8>),
    Empty,
}

#[derive(Clone)]
struct MemberSnapshot {
    clazz: Vec<u8>,
    name: Vec<u8>,
    flags: u32,
    r#type: SnapshotType,
}

thread_local! {
    static MEMBER_SNAPSHOTS: RefCell<HashMap<usize, MemberSnapshot>> = RefCell::new(HashMap::new());
}

fn snapshot_type(value: Value<'_>) -> SnapshotType {
    match value {
        Value::Object(handle) => SnapshotType::Object(handle),
        Value::ClassMirror(name) => SnapshotType::ClassName(name.to_vec()),
        _ => SnapshotType::Empty,
    }
}

fn record_snapshot(loader: &ClassLoader<'_>, handle: usize) -> io::Result<()> {
    let state = read_member_state(loader, handle)?;
    let snapshot = MemberSnapshot {
        clazz: state.clazz.map(|name| name.to_vec()).unwrap_or_default(),
        name: state.name,
        flags: state.flags,
        r#type: snapshot_type(state.r#type),
    };
    MEMBER_SNAPSHOTS.with(|snapshots| {
        snapshots.borrow_mut().insert(handle, snapshot);
    });
    Ok(())
}

fn find_method_member<'k, 'a>(
    klass: &'k Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
    want_static: bool,
) -> Option<(Arc<InstanceKlass<'a>>, usize)> {
    let mut current = Some(Arc::clone(klass));
    while let Some(k) = current {
        for (index, method) in k.class.methods.iter().enumerate() {
            if method.name == name
                && method.descriptor == descriptor
                && (method.accessFlags & ACC_STATIC != 0) == want_static
            {
                return Some((Arc::clone(&k), index));
            }
        }
        current = k.super_klass();
    }
    None
}

/// Finds a signature-polymorphic method (declared as
/// `([Ljava/lang/Object;)...`) by name. Used as a fallback in `resolve`
/// because the MemberName carries the polymorphic call-site type rather than
/// the erased varargs descriptor.
fn find_polymorphic_method<'a>(
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
) -> Option<(Arc<InstanceKlass<'a>>, usize)> {
    let mut current = Some(Arc::clone(klass));
    while let Some(k) = current {
        for (index, method) in k.class.methods.iter().enumerate() {
            if method.name == name && method.descriptor.starts_with(b"([Ljava/lang/Object;)") {
                return Some((Arc::clone(&k), index));
            }
        }
        current = k.super_klass();
    }
    None
}

fn find_interface_method<'a>(
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> Option<(Arc<InstanceKlass<'a>>, usize)> {
    let mut pending: Vec<Arc<InstanceKlass<'a>>> = klass.interfaces.clone();
    let mut visited = std::collections::HashSet::new();
    while let Some(iface) = pending.pop() {
        if !visited.insert(Arc::as_ptr(&iface) as usize) {
            continue;
        }
        pending.extend(iface.interfaces.iter().cloned());
        for (index, method) in iface.class.methods.iter().enumerate() {
            if method.name == name
                && method.descriptor == descriptor
                && method.accessFlags & ACC_STATIC == 0
                && method.accessFlags & ACC_ABSTRACT == 0
            {
                return Some((Arc::clone(&iface), index));
            }
        }
    }
    None
}

fn find_instance_field<'a>(
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> Option<Arc<InstanceKlass<'a>>> {
    let mut current = Some(Arc::clone(klass));
    while let Some(k) = current {
        if k.instance_field_infos
            .iter()
            .any(|info| info.name == name && info.descriptor == descriptor)
        {
            return Some(Arc::clone(&k));
        }
        current = k.super_klass();
    }
    None
}

fn find_static_field<'a>(
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> Option<Arc<InstanceKlass<'a>>> {
    let mut current = Some(Arc::clone(klass));
    while let Some(k) = current {
        if k.static_field_infos
            .iter()
            .any(|info| info.name == name && info.descriptor == descriptor)
        {
            return Some(Arc::clone(&k));
        }
        current = k.super_klass();
    }
    None
}

/// Locates a field by name+descriptor for offset computation. Returns the
/// declaring klass and whether the field is static.
fn locate_field<'a>(
    loader: &ClassLoader<'a>,
    clazz: &[u8],
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<(Arc<InstanceKlass<'a>>, bool)> {
    let klass = loader.load(clazz).map_err(|_| class_not_found(loader, clazz))?;
    if let Some(owner) = find_instance_field(&klass, name, descriptor) {
        return Ok((owner, false));
    }
    if let Some(owner) = find_static_field(&klass, name, descriptor) {
        return Ok((owner, true));
    }
    Err(no_such_field(loader, name, descriptor))
}

fn field_info_offset<'a>(
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
    want_static: bool,
) -> io::Result<u32> {
    let infos = if want_static {
        &klass.static_field_infos
    } else {
        &klass.instance_field_infos
    };
    infos
        .iter()
        .find(|info| info.name == name && info.descriptor == descriptor)
        .map(|info| info.offset)
        .ok_or_else(|| invalid("Field not found in declaring class"))
}

fn natives_init<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>]) -> io::Result<()> {
    let (Some(&Value::Object(mn)), Some(&reflect)) = (arguments.first(), arguments.get(1)) else {
        return Err(invalid("MethodHandleNatives.init arguments"));
    };
    let reflect_klass = match reflect {
        Value::Object(handle) => object_klass(loader, handle)?,
        Value::ClassMirror(_) => {
            // MemberName(Class): a type MemberName.
            set_object_field(loader, mn, b"clazz", b"Ljava/lang/Class;", reflect)?;
            set_object_field(loader, mn, b"flags", b"I", Value::Int(IS_TYPE as i32))?;
            set_object_field(loader, mn, b"resolution", b"Ljava/lang/Object;", Value::Null)?;
            record_snapshot(loader, mn)?;
            return Ok(());
        }
        _ => return Err(invalid("MethodHandleNatives.init reflect object")),
    };
    let reflect_name = reflect_klass.class.name;
    let is_field = reflect_name == b"java/lang/reflect/Field";
    let is_method = matches!(
        reflect_name,
        b"java/lang/reflect/Method" | b"java/lang/reflect/Constructor"
    );
    if !is_field && !is_method {
        return Err(invalid("Unsupported MemberName.init reflect object"));
    }
    let handle = match reflect {
        Value::Object(handle) => handle,
        _ => unreachable!(),
    };
    let clazz_value = object_field(loader, handle, b"clazz", b"Ljava/lang/Class;")?;
    set_object_field(loader, mn, b"clazz", b"Ljava/lang/Class;", clazz_value)?;
    let modifiers = object_field(loader, handle, b"modifiers", b"I")?.int()? as u32;
    let mut flags = modifiers & 0xFFFF;
    if is_field {
        flags |= IS_FIELD;
        flags |= (if modifiers & u32::from(ACC_STATIC) != 0 {
            REF_GET_STATIC
        } else {
            REF_GET_FIELD
        }) << REF_KIND_SHIFT;
        if let Ok(trusted) = object_field(loader, handle, b"trustedFinal", b"Z") {
            if trusted.int()? != 0 {
                flags |= TRUSTED_FINAL;
            }
        }
        let type_value = object_field(loader, handle, b"type", b"Ljava/lang/Class;")?;
        set_object_field(loader, mn, b"type", b"Ljava/lang/Object;", type_value)?;
    } else {
        let name_value = if reflect_name == b"java/lang/reflect/Constructor" {
            string_value(loader, "<init>")
        } else {
            object_field(loader, handle, b"name", b"Ljava/lang/String;")?
        };
        set_object_field(loader, mn, b"name", b"Ljava/lang/String;", name_value)?;
        flags |= if reflect_name == b"java/lang/reflect/Constructor" {
            IS_CONSTRUCTOR
        } else {
            IS_METHOD
        };
        // Build the MethodType via MethodType.methodType(rtype, ptypes).
        let rtype = if reflect_name == b"java/lang/reflect/Constructor" {
            clazz_value
        } else {
            object_field(loader, handle, b"returnType", b"Ljava/lang/Class;")?
        };
        let ptypes = object_field(loader, handle, b"parameterTypes", b"[Ljava/lang/Class;")?;
        let method_type = {
            let mt_klass = loader.load(b"java/lang/invoke/MethodType")?;
            let _ = bytecodeInterpreter::initialize(loader, &mt_klass, &mut io::sink(), 0);
            let member = mt_klass.class.methods.iter().find(|method| {
                method.name == b"methodType"
                    && method.descriptor == b"(Ljava/lang/Class;[Ljava/lang/Class;)Ljava/lang/invoke/MethodType;"
                    && method.accessFlags & ACC_STATIC != 0
            });
            let Some(member) = member else {
                return Err(invalid("MethodType.methodType not found"));
            };
            let code = Code::parse(member)?;
            let result = bytecodeInterpreter::execute(
                loader,
                &mt_klass,
                code,
                &[rtype, ptypes],
                &mut io::sink(),
                0,
                member.accessFlags,
            )?;
            result.ok_or_else(|| invalid("MethodType.methodType returned void"))?
        };
        set_object_field(loader, mn, b"type", b"Ljava/lang/Object;", method_type)?;
    }
    set_object_field(loader, mn, b"flags", b"I", Value::Int(flags as i32))?;
    set_object_field(loader, mn, b"resolution", b"Ljava/lang/Object;", Value::Null)?;
    record_snapshot(loader, mn)?;
    Ok(())
}

fn natives_expand<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>]) -> io::Result<()> {
    let Some(&Value::Object(mn)) = arguments.first() else {
        return Err(invalid("MethodHandleNatives.expand arguments"));
    };
    let snapshot = MEMBER_SNAPSHOTS.with(|snapshots| snapshots.borrow().get(&mn).cloned());
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    if !snapshot.clazz.is_empty() {
        let text = String::from_utf8_lossy(&snapshot.clazz).into_owned();
        let name: &'a str = loader.storage.alloc_str(&text);
        set_object_field(
            loader,
            mn,
            b"clazz",
            b"Ljava/lang/Class;",
            Value::ClassMirror(name.as_bytes()),
        )?;
    }
    if !snapshot.name.is_empty() {
        let text = String::from_utf8_lossy(&snapshot.name).into_owned();
        let name: &'a str = loader.storage.alloc_str(&text);
        set_object_field(
            loader,
            mn,
            b"name",
            b"Ljava/lang/String;",
            Value::HostString(name),
        )?;
    }
    let type_value = match &snapshot.r#type {
        SnapshotType::Object(handle) => Value::Object(*handle),
        SnapshotType::ClassName(name) => {
            let text = String::from_utf8_lossy(name).into_owned();
            let name: &'a str = loader.storage.alloc_str(&text);
            Value::ClassMirror(name.as_bytes())
        }
        SnapshotType::Empty => Value::Null,
    };
    set_object_field(loader, mn, b"type", b"Ljava/lang/Object;", type_value)?;
    set_object_field(loader, mn, b"flags", b"I", Value::Int(snapshot.flags as i32))?;
    Ok(())
}

fn natives_resolve<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>]) -> io::Result<Value<'a>> {
    let Some(&Value::Object(mn)) = arguments.first() else {
        return Err(invalid("MethodHandleNatives.resolve arguments"));
    };
    let state = read_member_state(loader, mn)?;
    let Some(clazz_name) = state.clazz.clone() else {
        return Err(no_such_field(loader, b"<init>", b""));
    };
    let descriptor = member_descriptor(loader, &state)?;
    let is_field = member_is_field(&state);
    let want_static = match member_ref_kind(state.flags) {
        REF_GET_STATIC | REF_PUT_STATIC | REF_INVOKE_STATIC => true,
        REF_GET_FIELD | REF_PUT_FIELD | REF_INVOKE_VIRTUAL | REF_INVOKE_SPECIAL
        | REF_NEW_INVOKE_SPECIAL | REF_INVOKE_INTERFACE => false,
        _ => {
            if is_field {
                // Unknown reference kind: locate the field and derive
                // staticness from the class metadata.
                let (owner, static_field) =
                    locate_field(loader, &clazz_name, &state.name, &descriptor)?;
                write_resolved_field(loader, mn, &state, &owner, &descriptor, static_field)?;
                record_snapshot(loader, mn)?;
                return Ok(Value::Object(mn));
            }
            false
        }
    };
    let klass = loader
        .load(&clazz_name)
        .map_err(|_| class_not_found(loader, &clazz_name))?;
    if is_field {
        let Some(owner) = (if want_static {
            find_static_field(&klass, &state.name, &descriptor)
        } else {
            find_instance_field(&klass, &state.name, &descriptor)
        }) else {
            return Err(no_such_field(loader, &state.name, &descriptor));
        };
        write_resolved_field(loader, mn, &state, &owner, &descriptor, want_static)?;
    } else {
        let mut found = find_method_member(&klass, &state.name, &descriptor, want_static)
            .or_else(|| {
                if member_ref_kind(state.flags) == REF_INVOKE_INTERFACE {
                    find_interface_method(&klass, &state.name, &descriptor)
                } else {
                    None
                }
            });
        if found.is_none()
            && matches!(
                clazz_name.as_slice(),
                b"java/lang/invoke/MethodHandle" | b"java/lang/invoke/VarHandle"
            )
        {
            found = find_polymorphic_method(&klass, &state.name);
        }
        let Some((owner, index)) = found else {
            return Err(no_such_method(loader, &state.name, &descriptor));
        };
        let member = &owner.class.methods[index];
        let kind_bits = if state.name == b"<init>" {
            IS_CONSTRUCTOR
        } else {
            IS_METHOD
        };
        let mut flags = (state.flags & !(0xFFFF | ALL_KINDS | HIGH_BITS))
            | u32::from(member.accessFlags)
            | kind_bits;
        if member.accessFlags & ACC_STATIC != 0 && member_ref_kind(state.flags) == 0 {
            flags |= (REF_INVOKE_STATIC as u32) << REF_KIND_SHIFT;
        }
        set_object_field(
            loader,
            mn,
            b"clazz",
            b"Ljava/lang/Class;",
            Value::ClassMirror(owner.class.name),
        )?;
        set_object_field(loader, mn, b"flags", b"I", Value::Int(flags as i32))?;
        let token = {
            let token_klass = loader.load(b"java/lang/invoke/ResolvedMethodName")?;
            let _ = bytecodeInterpreter::initialize(loader, &token_klass, &mut io::sink(), 0);
            loader.alloc_object(InstanceOop::new(token_klass)?)
        };
        set_object_field(
            loader,
            mn,
            b"method",
            b"Ljava/lang/invoke/ResolvedMethodName;",
            Value::Object(token),
        )?;
        set_object_field(loader, mn, b"resolution", b"Ljava/lang/Object;", Value::Null)?;
    }
    record_snapshot(loader, mn)?;
    Ok(Value::Object(mn))
}

fn write_resolved_field<'a>(
    loader: &ClassLoader<'a>,
    mn: usize,
    state: &MemberState<'a>,
    owner: &Arc<InstanceKlass<'a>>,
    descriptor: &[u8],
    want_static: bool,
) -> io::Result<()> {
    let infos = if want_static {
        &owner.static_field_infos
    } else {
        &owner.instance_field_infos
    };
    let info = infos
        .iter()
        .find(|info| info.name == state.name && info.descriptor == descriptor)
        .ok_or_else(|| invalid("Field not found in declaring class"))?;
    let mut flags = (state.flags & !(0xFFFF | ALL_KINDS | HIGH_BITS))
        | u32::from(info.access_flags)
        | IS_FIELD;
    let trusted = info.is_final()
        && (info.is_private() || owner.class.name.starts_with(b"java/lang/invoke/"));
    if trusted {
        flags |= TRUSTED_FINAL;
    }
    set_object_field(
        loader,
        mn,
        b"clazz",
        b"Ljava/lang/Class;",
        Value::ClassMirror(owner.class.name),
    )?;
    set_object_field(loader, mn, b"flags", b"I", Value::Int(flags as i32))?;
    set_object_field(loader, mn, b"resolution", b"Ljava/lang/Object;", Value::Null)?;
    Ok(())
}

fn member_field_offset<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<i64> {
    let Some(&Value::Object(mn)) = arguments.first() else {
        return Err(invalid("MethodHandleNatives field offset arguments"));
    };
    let state = read_member_state(loader, mn)?;
    let Some(clazz_name) = state.clazz.clone() else {
        return Err(no_such_field(loader, b"<init>", b""));
    };
    let descriptor = member_descriptor(loader, &state)?;
    let (owner, _) = locate_field(loader, &clazz_name, &state.name, &descriptor)?;
    // Prefer the reference kind to pick instance vs static storage; fall back
    // to whichever list actually contains the field.
    let ref_kind = member_ref_kind(state.flags);
    let want_static = match ref_kind {
        REF_GET_STATIC | REF_PUT_STATIC => true,
        REF_GET_FIELD | REF_PUT_FIELD => false,
        _ => owner
            .static_field_infos
            .iter()
            .any(|info| info.name == state.name && info.descriptor == descriptor),
    };
    let offset = field_info_offset(&owner, &state.name, &descriptor, want_static)
        .or_else(|_| field_info_offset(&owner, &state.name, &descriptor, !want_static))?;
    Ok(i64::from(offset))
}

fn natives_static_field_base<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let Some(&Value::Object(mn)) = arguments.first() else {
        return Err(invalid("MethodHandleNatives.staticFieldBase arguments"));
    };
    let state = read_member_state(loader, mn)?;
    let Some(clazz_name) = state.clazz.clone() else {
        return Err(no_such_field(loader, b"<init>", b""));
    };
    let descriptor = member_descriptor(loader, &state)?;
    let (owner, _) = locate_field(loader, &clazz_name, &state.name, &descriptor)?;
    Ok(Value::ClassMirror(owner.class.name))
}

fn natives_member_vm_info<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let _ = arguments;
    let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
        2,
        b"java/lang/Object",
    )?;
    array.elements[0] = Value::Long(0);
    array.elements[1] = Value::Long(0);
    Ok(Value::ObjectArray(loader.alloc_obj_array(array)))
}

fn natives_set_call_site_target<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<()> {
    let (Some(&Value::Object(call_site)), Some(&target)) =
        (arguments.first(), arguments.get(1))
    else {
        return Err(invalid("MethodHandleNatives.setCallSiteTarget arguments"));
    };
    set_object_field(
        loader,
        call_site,
        b"target",
        b"Ljava/lang/invoke/MethodHandle;",
        target,
    )
}

/// Returns the `java.lang.invoke.VarHandle` receiver handle for a
/// signature-polymorphic access-mode call, or `None` when the receiver is
/// not a VarHandle.
fn var_handle_receiver<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<usize>> {
    let Some(&Value::Object(handle)) = arguments.first() else {
        return Ok(None);
    };
    let klass = object_klass(loader, handle)?;
    let mut current = Some(klass);
    while let Some(k) = current {
        if k.class.name == b"java/lang/invoke/VarHandle" {
            return Ok(Some(handle));
        }
        current = k.super_klass();
    }
    Ok(None)
}

/// Returns the `java.lang.invoke.MethodHandle` receiver handle for a
/// signature-polymorphic invoke call, or `None`.
fn method_handle_receiver<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<usize>> {
    let Some(&Value::Object(handle)) = arguments.first() else {
        return Ok(None);
    };
    let klass = object_klass(loader, handle)?;
    let mut current = Some(klass);
    while let Some(k) = current {
        if k.class.name == b"java/lang/invoke/MethodHandle" {
            return Ok(Some(handle));
        }
        current = k.super_klass();
    }
    Ok(None)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AccessOp {
    Get,
    Set,
    CompareAndSet,
    CompareAndExchange,
    WeakCompareAndSet,
    GetAndSet,
    GetAndAdd,
    GetAndBitwiseOr,
    GetAndBitwiseAnd,
    GetAndBitwiseXor,
}

fn access_op(name: &[u8]) -> Option<AccessOp> {
    Some(match name {
        b"get" | b"getVolatile" | b"getOpaque" | b"getAcquire" => AccessOp::Get,
        b"set" | b"setVolatile" | b"setOpaque" | b"setRelease" => AccessOp::Set,
        b"compareAndSet" => AccessOp::CompareAndSet,
        b"compareAndExchange" | b"compareAndExchangeAcquire" | b"compareAndExchangeRelease" => {
            AccessOp::CompareAndExchange
        }
        b"weakCompareAndSetPlain" | b"weakCompareAndSet" | b"weakCompareAndSetAcquire"
        | b"weakCompareAndSetRelease" => AccessOp::WeakCompareAndSet,
        b"getAndSet" | b"getAndSetAcquire" | b"getAndSetRelease" => AccessOp::GetAndSet,
        b"getAndAdd" | b"getAndAddAcquire" | b"getAndAddRelease" => AccessOp::GetAndAdd,
        b"getAndBitwiseOr" | b"getAndBitwiseOrAcquire" | b"getAndBitwiseOrRelease" => {
            AccessOp::GetAndBitwiseOr
        }
        b"getAndBitwiseAnd" | b"getAndBitwiseAndAcquire" | b"getAndBitwiseAndRelease" => {
            AccessOp::GetAndBitwiseAnd
        }
        b"getAndBitwiseXor" | b"getAndBitwiseXorAcquire" | b"getAndBitwiseXorRelease" => {
            AccessOp::GetAndBitwiseXor
        }
        _ => return None,
    })
}

/// Resolves the VarHandle's target: an instance field slot, a static field
/// cell, or an array element accessor.
enum VarTarget<'a> {
    /// (receiver object handle, slot, field kind byte)
    InstanceField { receiver: usize, slot: usize, kind: u8 },
    /// (declaring klass, static_fields index, kind byte)
    StaticField {
        klass: Arc<InstanceKlass<'a>>,
        index: usize,
        kind: u8,
    },
    /// Primitive array handle; element kind comes from the TypeArrayOop variant.
    PrimitiveArray(usize),
    /// Object array handle.
    ObjectArray(usize),
}

fn find_instance_slot(object: &crate::share::oops::instanceOop::InstanceOop<'_>, offset: i64) -> io::Result<(usize, u8)> {
    let mut current = Some(object.klass.as_ref());
    while let Some(klass) = current {
        for field in &klass.instance_field_infos {
            if i64::from(field.offset) == offset {
                let kind = field
                    .descriptor
                    .first()
                    .copied()
                    .ok_or_else(|| invalid("Invalid field descriptor"))?;
                return Ok((field.slot, kind));
            }
        }
        current = klass.super_klass.as_deref();
    }
    Err(invalid("Invalid VarHandle field offset"))
}

fn resolve_var_target<'a>(
    loader: &ClassLoader<'a>,
    vh: usize,
    mode_args: &[Value<'a>],
) -> io::Result<(VarTarget<'a>, usize)> {
    // VarHandle wrappers that delegate to a resolved target VarHandle
    // (IndirectVarHandle, LazyInitializingVarHandle, ...).
    let klass = object_klass(loader, vh)?;
    if klass.class.name == b"java/lang/invoke/IndirectVarHandle" {
        let target = object_field(loader, vh, b"target", b"Ljava/lang/invoke/VarHandle;")?;
        let Value::Object(target_handle) = target else {
            return Err(invalid("IndirectVarHandle without target"));
        };
        return resolve_var_target(loader, target_handle, mode_args);
    }
    if let Ok(Value::Object(target_handle)) =
        object_field(loader, vh, b"target", b"Ljava/lang/invoke/VarHandle;")
    {
        return resolve_var_target(loader, target_handle, mode_args);
    }
    // Array-shaped VarHandles carry `abase`/`ashift`.
    if object_field(loader, vh, b"abase", b"I").is_ok() {
        let (Some(array), Some(&Value::Int(_index))) =
            (mode_args.first(), mode_args.get(1))
        else {
            return Err(invalid("VarHandle array coordinates"));
        };
        if matches!(array, Value::Null) {
            return Err(null_pointer(loader));
        }
        let target = match array {
            Value::PrimitiveArray(handle) => VarTarget::PrimitiveArray(*handle),
            Value::ObjectArray(handle) => VarTarget::ObjectArray(*handle),
            _ => return Err(class_cast(loader, "VarHandle array coordinate")),
        };
        return Ok((target, 2));
    }
    // Field-shaped VarHandles carry `fieldOffset`.
    let offset_value = object_field(loader, vh, b"fieldOffset", b"J").map_err(|error| {
        io::Error::other(format!(
            "{} (VarHandle class {})",
            error,
            String::from_utf8_lossy(&klass.class.name)
        ))
    })?;
    let offset = offset_value.long()?;
    let base = object_field(loader, vh, b"base", b"Ljava/lang/Object;").ok();
    match base {
        Some(Value::ClassMirror(class)) => {
            // Static field: base is the declaring class mirror.
            let declaring = loader.load(class).map_err(|_| class_not_found(loader, class))?;
            for (index, info) in declaring.static_field_infos.iter().enumerate() {
                if i64::from(info.offset) == offset {
                    let kind = info
                        .descriptor
                        .first()
                        .copied()
                        .ok_or_else(|| invalid("Invalid field descriptor"))?;
                    return Ok((
                        VarTarget::StaticField {
                            klass: declaring,
                            index,
                            kind,
                        },
                        0,
                    ));
                }
            }
            Err(invalid("Invalid VarHandle static field offset"))
        }
        Some(Value::Null) | Some(Value::Empty) | None => {
            // Instance field: first coordinate is the receiver.
            let Some(&receiver) = mode_args.first() else {
                return Err(invalid("VarHandle receiver coordinate"));
            };
            match receiver {
                Value::Null => return Err(null_pointer(loader)),
                Value::Object(receiver_handle) => {
                    let objects = loader.borrow_objects();
                    let target_object = objects
                        .get(receiver_handle)
                        .ok_or_else(|| invalid("Invalid receiver reference"))?;
                    let (slot, kind) = find_instance_slot(target_object, offset)?;
                    drop(objects);
                    Ok((
                        VarTarget::InstanceField {
                            receiver: receiver_handle,
                            slot,
                            kind,
                        },
                        1,
                    ))
                }
                _ => Err(class_cast(
                    loader,
                    "VarHandle receiver is not an object",
                )),
            }
        }
        _ => Err(invalid("Unsupported VarHandle base")),
    }
}

/// Unboxes a wrapper object (`Integer`, `Long`, ...) back to its primitive
/// value. Non-wrapper values are returned unchanged.
fn unbox_value<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Value<'a>> {
    let Value::Object(handle) = value else {
        return Ok(value);
    };
    let klass = object_klass(loader, handle)?;
    let descriptor: &[u8] = match klass.class.name {
        b"java/lang/Integer" => b"I",
        b"java/lang/Long" => b"J",
        b"java/lang/Float" => b"F",
        b"java/lang/Double" => b"D",
        b"java/lang/Boolean" => b"Z",
        b"java/lang/Byte" => b"B",
        b"java/lang/Short" => b"S",
        b"java/lang/Character" => b"C",
        _ => return Ok(value),
    };
    object_field(loader, handle, b"value", descriptor)
}

/// Boxes a primitive value into its wrapper class. Reference values and
/// `Null` pass through unchanged.
fn box_value<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    tag: u8,
) -> io::Result<Value<'a>> {
    let (class, descriptor, inner) = match (tag, value) {
        (b'J', Value::Long(v)) => (b"java/lang/Long".as_slice(), b"J".as_slice(), Value::Long(v)),
        (b'D', Value::Double(v)) => {
            (b"java/lang/Double".as_slice(), b"D".as_slice(), Value::Double(v))
        }
        (b'F', Value::Float(v)) => (b"java/lang/Float".as_slice(), b"F".as_slice(), Value::Float(v)),
        (b'Z', Value::Int(v)) => (b"java/lang/Boolean".as_slice(), b"Z".as_slice(), Value::Int(v & 1)),
        (b'B', Value::Int(v)) => (b"java/lang/Byte".as_slice(), b"B".as_slice(), Value::Int(v as i8 as i32)),
        (b'C', Value::Int(v)) => {
            (b"java/lang/Character".as_slice(), b"C".as_slice(), Value::Int(v as u16 as i32))
        }
        (b'S', Value::Int(v)) => (b"java/lang/Short".as_slice(), b"S".as_slice(), Value::Int(v as i16 as i32)),
        (_, Value::Long(v)) => (b"java/lang/Long".as_slice(), b"J".as_slice(), Value::Long(v)),
        (_, Value::Double(v)) => {
            (b"java/lang/Double".as_slice(), b"D".as_slice(), Value::Double(v))
        }
        (_, Value::Float(v)) => (b"java/lang/Float".as_slice(), b"F".as_slice(), Value::Float(v)),
        (_, Value::Int(v)) => (b"java/lang/Integer".as_slice(), b"I".as_slice(), Value::Int(v)),
        _ => return Ok(value),
    };
    let klass = loader.load(class)?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0);
    let mut object = InstanceOop::new(klass)?;
    let slot = object.klass.instanceField(b"value", descriptor)?;
    object.fields[slot] = inner;
    Ok(Value::Object(loader.alloc_object(object)))
}

/// Returns the return-type tag of a method descriptor.
fn return_tag(descriptor: &[u8]) -> u8 {
    let Some(close) = descriptor.iter().position(|byte| *byte == b')') else {
        return b'V';
    };
    let mut index = close + 1;
    while descriptor.get(index) == Some(&b'[') {
        index += 1;
    }
    descriptor.get(index).copied().unwrap_or(b'V')
}

/// Adapts a VarHandle access-mode result to the call-site return type:
/// primitive results are boxed when the signature returns a reference, and
/// wrapper results are unboxed for primitive returns.
fn finish_vh_result<'a>(
    loader: &ClassLoader<'a>,
    result: Value<'a>,
    kind: u8,
    descriptor: &[u8],
) -> io::Result<Value<'a>> {
    match signature::method(descriptor)?.1 {
        Some(4) => {
            if matches!(
                result,
                Value::Int(_) | Value::Long(_) | Value::Float(_) | Value::Double(_)
            ) {
                box_value(loader, result, kind)
            } else {
                Ok(result)
            }
        }
        Some(_) => {
            if matches!(result, Value::Object(_)) {
                unbox_value(loader, result)
            } else {
                Ok(result)
            }
        }
        None => Ok(result),
    }
}

/// Coerces a Java value into the storage representation for a field/element
/// kind byte (descriptor tag).
fn coerce_value<'a>(value: Value<'a>, kind: u8) -> Value<'a> {
    match kind {
        b'J' => match value {
            Value::Int(v) => Value::Long(i64::from(v)),
            Value::Long(v) => Value::Long(v),
            Value::Float(v) => Value::Long(v as i64),
            Value::Double(v) => Value::Long(v as i64),
            other => match other {
                Value::Int(_) | Value::Long(_) | Value::Float(_) | Value::Double(_) => other,
                _ => Value::Empty,
            },
        },
        b'D' => match value {
            Value::Double(v) => Value::Double(v),
            Value::Float(v) => Value::Double(f64::from(v)),
            Value::Long(v) => Value::Double(v as f64),
            Value::Int(v) => Value::Double(f64::from(v)),
            other => other,
        },
        b'F' => match value {
            Value::Float(v) => Value::Float(v),
            Value::Double(v) => Value::Float(v as f32),
            Value::Long(v) => Value::Float(v as f32),
            Value::Int(v) => Value::Float(v as f32),
            other => other,
        },
        b'L' | b'[' => value,
        _ => match value {
            Value::Int(v) => Value::Int(match kind {
                b'B' => i32::from(v as i8),
                b'C' => i32::from(v as u16),
                b'S' => i32::from(v as i16),
                b'Z' => v & 1,
                _ => v,
            }),
            Value::Long(v) => Value::Int(match kind {
                b'B' => i32::from((v as i8) as i32),
                b'C' => i32::from((v as u16) as i32),
                b'S' => i32::from((v as i16) as i32),
                b'Z' => (v as i32) & 1,
                _ => v as i32,
            }),
            Value::Float(v) => Value::Int(v as i32),
            Value::Double(v) => Value::Int(v as i32),
            other => other,
        },
    }
}

/// Reads a stored field/element value and normalizes small integral kinds.
fn read_kind_value<'a>(value: Value<'a>, kind: u8) -> Value<'a> {
    match (kind, value) {
        (b'B', Value::Int(v)) => Value::Int(i32::from(v as i8)),
        (b'C', Value::Int(v)) => Value::Int(i32::from(v as u16)),
        (b'S', Value::Int(v)) => Value::Int(i32::from(v as i16)),
        (b'Z', Value::Int(v)) => Value::Int(v & 1),
        (_, v) => v,
    }
}

fn values_equal(a: Value<'_>, b: Value<'_>) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Double(x), Value::Double(y)) => x.to_bits() == y.to_bits(),
        _ => a == b,
    }
}

fn numeric_add(a: Value<'_>, b: Value<'_>, kind: u8) -> io::Result<Value<'static>> {
    Ok(match kind {
        b'J' => {
            let x = match a {
                Value::Long(v) => v,
                Value::Int(v) => i64::from(v),
                _ => return Err(invalid("VarHandle add operand")),
            };
            let y = match b {
                Value::Long(v) => v,
                Value::Int(v) => i64::from(v),
                _ => return Err(invalid("VarHandle add operand")),
            };
            Value::Long(x.wrapping_add(y))
        }
        b'F' => {
            let x = match a {
                Value::Float(v) => v,
                _ => return Err(invalid("VarHandle add operand")),
            };
            let y = match b {
                Value::Float(v) => v,
                _ => return Err(invalid("VarHandle add operand")),
            };
            Value::Float(x + y)
        }
        b'D' => {
            let x = match a {
                Value::Double(v) => v,
                _ => return Err(invalid("VarHandle add operand")),
            };
            let y = match b {
                Value::Double(v) => v,
                _ => return Err(invalid("VarHandle add operand")),
            };
            Value::Double(x + y)
        }
        _ => {
            let x = a.int()?;
            let y = b.int()?;
            Value::Int(x.wrapping_add(y))
        }
    })
}

fn numeric_bitwise(a: Value<'_>, b: Value<'_>, kind: u8, op: AccessOp) -> io::Result<Value<'static>> {
    if kind == b'J' {
        let x = match a {
            Value::Long(v) => v,
            Value::Int(v) => i64::from(v),
            _ => return Err(invalid("VarHandle bitwise operand")),
        };
        let y = match b {
            Value::Long(v) => v,
            Value::Int(v) => i64::from(v),
            _ => return Err(invalid("VarHandle bitwise operand")),
        };
        let result = match op {
            AccessOp::GetAndBitwiseOr => x | y,
            AccessOp::GetAndBitwiseAnd => x & y,
            AccessOp::GetAndBitwiseXor => x ^ y,
            _ => unreachable!(),
        };
        return Ok(Value::Long(result));
    }
    let x = a.int()?;
    let y = b.int()?;
    let result = match op {
        AccessOp::GetAndBitwiseOr => x | y,
        AccessOp::GetAndBitwiseAnd => x & y,
        AccessOp::GetAndBitwiseXor => x ^ y,
        _ => unreachable!(),
    };
    Ok(Value::Int(match kind {
        b'B' => i32::from(result as i8),
        b'C' => i32::from(result as u16),
        b'S' => i32::from(result as i16),
        b'Z' => result & 1,
        _ => result,
    }))
}

fn primitive_array_kind(array: &crate::share::oops::typeArrayOop::TypeArrayOop) -> u8 {
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    match array {
        TypeArrayOop::Boolean(_) => b'Z',
        TypeArrayOop::Byte(_) => b'B',
        TypeArrayOop::Char(_) => b'C',
        TypeArrayOop::Short(_) => b'S',
        TypeArrayOop::Int(_) => b'I',
        TypeArrayOop::Long(_) => b'J',
        TypeArrayOop::Float(_) => b'F',
        TypeArrayOop::Double(_) => b'D',
    }
}

fn primitive_array_get(
    array: &crate::share::oops::typeArrayOop::TypeArrayOop,
    index: usize,
) -> io::Result<Value<'static>> {
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    let out_of_bounds = || Error::other("ArrayIndexOutOfBoundsException");
    Ok(match array {
        TypeArrayOop::Boolean(values) => Value::Int(i32::from(*values.get(index).ok_or_else(out_of_bounds)?)),
        TypeArrayOop::Byte(values) => Value::Int(i32::from(*values.get(index).ok_or_else(out_of_bounds)?)),
        TypeArrayOop::Char(values) => Value::Int(i32::from(*values.get(index).ok_or_else(out_of_bounds)?)),
        TypeArrayOop::Short(values) => Value::Int(i32::from(*values.get(index).ok_or_else(out_of_bounds)?)),
        TypeArrayOop::Int(values) => Value::Int(*values.get(index).ok_or_else(out_of_bounds)?),
        TypeArrayOop::Long(values) => Value::Long(*values.get(index).ok_or_else(out_of_bounds)?),
        TypeArrayOop::Float(values) => Value::Float(*values.get(index).ok_or_else(out_of_bounds)?),
        TypeArrayOop::Double(values) => Value::Double(*values.get(index).ok_or_else(out_of_bounds)?),
    })
}

fn primitive_array_set(
    array: &mut crate::share::oops::typeArrayOop::TypeArrayOop,
    index: usize,
    value: Value<'_>,
) -> io::Result<()> {
    use crate::share::oops::typeArrayOop::TypeArrayOop;
    let out_of_bounds = || Error::other("ArrayIndexOutOfBoundsException");
    match array {
        TypeArrayOop::Boolean(values) => {
            let v = match value {
                Value::Int(v) => v,
                Value::Long(v) => v as i32,
                _ => return Err(invalid("VarHandle boolean operand")),
            };
            *values.get_mut(index).ok_or_else(out_of_bounds)? = u8::from(v != 0);
        }
        TypeArrayOop::Byte(values) => {
            *values.get_mut(index).ok_or_else(out_of_bounds)? = value.int()? as i8;
        }
        TypeArrayOop::Char(values) => {
            *values.get_mut(index).ok_or_else(out_of_bounds)? = value.int()? as u16;
        }
        TypeArrayOop::Short(values) => {
            *values.get_mut(index).ok_or_else(out_of_bounds)? = value.int()? as i16;
        }
        TypeArrayOop::Int(values) => {
            *values.get_mut(index).ok_or_else(out_of_bounds)? = value.int()?;
        }
        TypeArrayOop::Long(values) => {
            *values.get_mut(index).ok_or_else(out_of_bounds)? = match value {
                Value::Long(v) => v,
                Value::Int(v) => i64::from(v),
                _ => return Err(invalid("VarHandle long operand")),
            };
        }
        TypeArrayOop::Float(values) => {
            *values.get_mut(index).ok_or_else(out_of_bounds)? = match value {
                Value::Float(v) => v,
                _ => return Err(invalid("VarHandle float operand")),
            };
        }
        TypeArrayOop::Double(values) => {
            *values.get_mut(index).ok_or_else(out_of_bounds)? = match value {
                Value::Double(v) => v,
                _ => return Err(invalid("VarHandle double operand")),
            };
        }
    }
    Ok(())
}

fn var_handle_access<'a>(
    loader: &ClassLoader<'a>,
    vh: usize,
    name: &[u8],
    descriptor: &[u8],
    mode_args: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let Some(op) = access_op(name) else {
        return Ok(None);
    };
    {
        let klass = object_klass(loader, vh)?;
        if klass.class.name.starts_with(b"java/lang/invoke/VarHandleByteArrayAs") {
            return byte_array_view_access(loader, vh, &klass.class.name, op, mode_args);
        }
    }
    let (target, ncoords) = resolve_var_target(loader, vh, mode_args)?;
    let value_args = &mode_args[ncoords.min(mode_args.len())..];
    let expected_values = match op {
        AccessOp::Get => 0,
        AccessOp::Set => 1,
        AccessOp::CompareAndSet
        | AccessOp::CompareAndExchange
        | AccessOp::WeakCompareAndSet
        | AccessOp::GetAndSet
        | AccessOp::GetAndAdd
        | AccessOp::GetAndBitwiseOr
        | AccessOp::GetAndBitwiseAnd
        | AccessOp::GetAndBitwiseXor => {
            if matches!(
                op,
                AccessOp::CompareAndSet
                    | AccessOp::CompareAndExchange
                    | AccessOp::WeakCompareAndSet
            ) {
                2
            } else {
                1
            }
        }
    };
    if value_args.len() != expected_values {
        return Err(wrong_method_type(loader));
    }
    // Primitive-typed targets take unboxed values; unwrap any boxed
    // primitives the caller may have passed.
    let primitive_target = match &target {
        VarTarget::InstanceField { kind, .. } | VarTarget::StaticField { kind, .. } => {
            !matches!(*kind, b'L' | b'[')
        }
        VarTarget::PrimitiveArray(_) => true,
        VarTarget::ObjectArray(_) => false,
    };
    let unboxed_args;
    let value_args: &[Value<'a>] = if primitive_target && !value_args.is_empty() {
        let mut out = Vec::with_capacity(value_args.len());
        for value in value_args {
            out.push(unbox_value(loader, *value)?);
        }
        unboxed_args = out;
        &unboxed_args
    } else {
        value_args
    };
    match target {
        VarTarget::InstanceField {
            receiver,
            slot,
            kind,
        } => {
            if matches!(op, AccessOp::Set) || value_args.len() == 2 {
                // ensure receiver non-null
            }
            let result = {
                let mut objects = loader.borrow_objects_mut();
                let object = objects
                    .get_mut(receiver)
                    .ok_or_else(|| invalid("Invalid receiver reference"))?;
                let current = object
                    .fields
                    .get(slot)
                    .copied()
                    .unwrap_or(Value::Null);
                match op {
                    AccessOp::Get => read_kind_value(current, kind),
                    AccessOp::Set => {
                        let value = coerce_value(value_args[0], kind);
                        if let Some(cell) = object.fields.get_mut(slot) {
                            *cell = value;
                        }
                        return Ok(None);
                    }
                    AccessOp::CompareAndSet | AccessOp::WeakCompareAndSet => {
                        let expected = coerce_value(value_args[0], kind);
                        let new_value = coerce_value(value_args[1], kind);
                        if values_equal(current, expected) {
                            if let Some(cell) = object.fields.get_mut(slot) {
                                *cell = new_value;
                            }
                            Value::Int(1)
                        } else {
                            Value::Int(0)
                        }
                    }
                    AccessOp::CompareAndExchange => {
                        let expected = coerce_value(value_args[0], kind);
                        let new_value = coerce_value(value_args[1], kind);
                        let old = read_kind_value(current, kind);
                        if values_equal(current, expected) {
                            if let Some(cell) = object.fields.get_mut(slot) {
                                *cell = new_value;
                            }
                        }
                        old
                    }
                    AccessOp::GetAndSet => {
                        let new_value = coerce_value(value_args[0], kind);
                        let old = read_kind_value(current, kind);
                        if let Some(cell) = object.fields.get_mut(slot) {
                            *cell = new_value;
                        }
                        old
                    }
                    AccessOp::GetAndAdd => {
                        let delta = coerce_value(value_args[0], kind);
                        let old = read_kind_value(current, kind);
                        let updated = numeric_add(old, delta, kind)?;
                        if let Some(cell) = object.fields.get_mut(slot) {
                            *cell = coerce_value(updated, kind);
                        }
                        old
                    }
                    AccessOp::GetAndBitwiseOr
                    | AccessOp::GetAndBitwiseAnd
                    | AccessOp::GetAndBitwiseXor => {
                        let delta = coerce_value(value_args[0], kind);
                        let old = read_kind_value(current, kind);
                        let updated = numeric_bitwise(old, delta, kind, op)?;
                        if let Some(cell) = object.fields.get_mut(slot) {
                            *cell = coerce_value(updated, kind);
                        }
                        old
                    }
                }
            };
            Ok(Some(finish_vh_result(loader, result, kind, descriptor)?))
        }
        VarTarget::StaticField { klass, index, kind } => {
            let cell = klass
                .static_fields
                .get(index)
                .ok_or_else(|| invalid("Invalid static field slot"))?;
            let current = cell.get();
            let result = match op {
                AccessOp::Get => read_kind_value(current, kind),
                AccessOp::Set => {
                    cell.set(coerce_value(value_args[0], kind));
                    return Ok(None);
                }
                AccessOp::CompareAndSet | AccessOp::WeakCompareAndSet => {
                    let expected = coerce_value(value_args[0], kind);
                    let new_value = coerce_value(value_args[1], kind);
                    if values_equal(current, expected) {
                        cell.set(new_value);
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                }
                AccessOp::CompareAndExchange => {
                    let expected = coerce_value(value_args[0], kind);
                    let new_value = coerce_value(value_args[1], kind);
                    let old = read_kind_value(current, kind);
                    if values_equal(current, expected) {
                        cell.set(new_value);
                    }
                    old
                }
                AccessOp::GetAndSet => {
                    let new_value = coerce_value(value_args[0], kind);
                    let old = read_kind_value(current, kind);
                    cell.set(new_value);
                    old
                }
                AccessOp::GetAndAdd => {
                    let delta = coerce_value(value_args[0], kind);
                    let old = read_kind_value(current, kind);
                    let updated = numeric_add(old, delta, kind)?;
                    cell.set(coerce_value(updated, kind));
                    old
                }
                AccessOp::GetAndBitwiseOr
                | AccessOp::GetAndBitwiseAnd
                | AccessOp::GetAndBitwiseXor => {
                    let delta = coerce_value(value_args[0], kind);
                    let old = read_kind_value(current, kind);
                    let updated = numeric_bitwise(old, delta, kind, op)?;
                    cell.set(coerce_value(updated, kind));
                    old
                }
            };
            Ok(Some(finish_vh_result(loader, result, kind, descriptor)?))
        }
        VarTarget::PrimitiveArray(handle) => {
            let Some(&Value::Int(index)) = mode_args.get(ncoords.saturating_sub(1)) else {
                return Err(invalid("VarHandle array index"));
            };
            let index = usize::try_from(index).map_err(|_| array_index_oob(loader))?;
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            let kind = primitive_array_kind(array);
            let result = match op {
                AccessOp::Get => primitive_array_get(array, index)?,
                AccessOp::Set => {
                    let value = coerce_value(value_args[0], kind);
                    primitive_array_set(array, index, value)?;
                    return Ok(None);
                }
                AccessOp::CompareAndSet | AccessOp::WeakCompareAndSet => {
                    let expected = coerce_value(value_args[0], kind);
                    let new_value = coerce_value(value_args[1], kind);
                    let current = primitive_array_get(array, index)?;
                    if values_equal(current, expected) {
                        primitive_array_set(array, index, new_value)?;
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                }
                AccessOp::CompareAndExchange => {
                    let expected = coerce_value(value_args[0], kind);
                    let new_value = coerce_value(value_args[1], kind);
                    let current = primitive_array_get(array, index)?;
                    let old = read_kind_value(current, kind);
                    if values_equal(current, expected) {
                        primitive_array_set(array, index, new_value)?;
                    }
                    old
                }
                AccessOp::GetAndSet => {
                    let new_value = coerce_value(value_args[0], kind);
                    let current = primitive_array_get(array, index)?;
                    let old = read_kind_value(current, kind);
                    primitive_array_set(array, index, new_value)?;
                    old
                }
                AccessOp::GetAndAdd => {
                    let delta = coerce_value(value_args[0], kind);
                    let current = primitive_array_get(array, index)?;
                    let old = read_kind_value(current, kind);
                    let updated = numeric_add(old, delta, kind)?;
                    primitive_array_set(array, index, coerce_value(updated, kind))?;
                    old
                }
                AccessOp::GetAndBitwiseOr
                | AccessOp::GetAndBitwiseAnd
                | AccessOp::GetAndBitwiseXor => {
                    let delta = coerce_value(value_args[0], kind);
                    let current = primitive_array_get(array, index)?;
                    let old = read_kind_value(current, kind);
                    let updated = numeric_bitwise(old, delta, kind, op)?;
                    primitive_array_set(array, index, coerce_value(updated, kind))?;
                    old
                }
            };
            Ok(Some(finish_vh_result(loader, result, kind, descriptor)?))
        }
        VarTarget::ObjectArray(handle) => {
            let Some(&Value::Int(index)) = mode_args.get(ncoords.saturating_sub(1)) else {
                return Err(invalid("VarHandle array index"));
            };
            let index = usize::try_from(index).map_err(|_| array_index_oob(loader))?;
            let mut arrays = loader.obj_arrays.borrow_mut();
            let array = arrays
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            let current = array
                .elements
                .get(index)
                .copied()
                .ok_or_else(|| Error::other("ArrayIndexOutOfBoundsException"))?;
            let result = match op {
                AccessOp::Get => current,
                AccessOp::Set => {
                    array
                        .elements
                        .get_mut(index)
                        .map(|cell| *cell = value_args[0])
                        .ok_or_else(|| Error::other("ArrayIndexOutOfBoundsException"))?;
                    return Ok(None);
                }
                AccessOp::CompareAndSet | AccessOp::WeakCompareAndSet => {
                    if values_equal(current, value_args[0]) {
                        if let Some(cell) = array.elements.get_mut(index) {
                            *cell = value_args[1];
                        }
                        Value::Int(1)
                    } else {
                        Value::Int(0)
                    }
                }
                AccessOp::CompareAndExchange => {
                    if values_equal(current, value_args[0]) {
                        if let Some(cell) = array.elements.get_mut(index) {
                            *cell = value_args[1];
                        }
                    }
                    current
                }
                AccessOp::GetAndSet => {
                    if let Some(cell) = array.elements.get_mut(index) {
                        *cell = value_args[0];
                    }
                    current
                }
                _ => {
                    return Err(throw_java(
                        loader,
                        b"java/lang/UnsupportedOperationException",
                        Some("VarHandle operation not supported for reference arrays"),
                    ))
                }
            };
            Ok(Some(result))
        }
    }
}

/// Coerces `value` to the expected parameter `kind` for MethodHandle `invoke`
/// (asType-style conversions including boxing/unboxing of wrappers).
fn coerce_to_kind<'a>(loader: &ClassLoader<'a>, value: Value<'a>, kind: u8) -> io::Result<Value<'a>> {
    let mut value = value;
    let mut natural = match value {
        Value::Int(_) => 0,
        Value::Long(_) => 1,
        Value::Float(_) => 2,
        Value::Double(_) => 3,
        _ => 4,
    };
    if natural == 4 && kind != 4 {
        value = unbox_value(loader, value)?;
        natural = match value {
            Value::Int(_) => 0,
            Value::Long(_) => 1,
            Value::Float(_) => 2,
            Value::Double(_) => 3,
            _ => 4,
        };
        if natural == 4 {
            return Err(wrong_method_type(loader));
        }
    }
    if kind == 4 {
        if natural == 4 {
            return Ok(value);
        }
        let tag = match value {
            Value::Long(_) => b'J',
            Value::Float(_) => b'F',
            Value::Double(_) => b'D',
            _ => b'I',
        };
        return box_value(loader, value, tag);
    }
    if natural == kind {
        return Ok(value);
    }
    if natural == 4 {
        return Err(wrong_method_type(loader));
    }
    Ok(match kind {
        0 => match value {
            Value::Long(v) => Value::Int(v as i32),
            Value::Float(v) => Value::Int(v as i32),
            Value::Double(v) => Value::Int(v as i32),
            _ => value,
        },
        1 => match value {
            Value::Int(v) => Value::Long(i64::from(v)),
            Value::Float(v) => Value::Long(v as i64),
            Value::Double(v) => Value::Long(v as i64),
            _ => value,
        },
        2 => match value {
            Value::Int(v) => Value::Float(v as f32),
            Value::Long(v) => Value::Float(v as f32),
            Value::Double(v) => Value::Float(v as f32),
            _ => value,
        },
        3 => match value {
            Value::Int(v) => Value::Double(f64::from(v)),
            Value::Long(v) => Value::Double(v as f64),
            Value::Float(v) => Value::Double(f64::from(v)),
            _ => value,
        },
        _ => value,
    })
}

/// Returns the receiver's runtime klass for MethodHandle dispatch.
fn receiver_klass<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
) -> io::Result<Arc<InstanceKlass<'a>>> {
    match value {
        Value::Object(handle) => object_klass(loader, handle),
        Value::ClassMirror(_) => loader.load(b"java/lang/Class"),
        Value::String(_) | Value::HostString(_) => loader.load(b"java/lang/String"),
        Value::PrimitiveArray(handle) => {
            let arrays = loader.arrays.borrow();
            let name: &'static [u8] = match arrays.get(handle) {
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Boolean(_)) => b"[Z",
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Byte(_)) => b"[B",
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Char(_)) => b"[C",
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Short(_)) => b"[S",
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Int(_)) => b"[I",
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Long(_)) => b"[J",
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Float(_)) => b"[F",
                Some(crate::share::oops::typeArrayOop::TypeArrayOop::Double(_)) => b"[D",
                None => return Err(invalid("Invalid array reference")),
            };
            loader.load(name)
        }
        Value::ObjectArray(handle) => {
            let arrays = loader.obj_arrays.borrow();
            let array = arrays
                .get(handle)
                .ok_or_else(|| invalid("Invalid array reference"))?;
            let component = array.component_name();
            let mut name = Vec::with_capacity(component.len() + 1);
            name.push(b'[');
            if component.starts_with(b"[") || component.starts_with(b":") {
                name.extend_from_slice(component);
            } else {
                name.push(b'L');
                name.extend_from_slice(component);
                name.push(b';');
            }
            let text = std::str::from_utf8(&name)
                .map_err(|_| invalid("Invalid array class name"))?;
            let name: &'a str = loader.storage.alloc_str(text);
            loader.load(name.as_bytes())
        }
        _ => Err(invalid("MethodHandle receiver is not an object")),
    }
}

/// Executes a resolved method/constructor, dispatching to nativeLookup for
/// native targets.
fn execute_member<'a>(
    loader: &ClassLoader<'a>,
    target_klass: &Arc<InstanceKlass<'a>>,
    member_index: usize,
    args: &[Value<'a>],
    is_static_native: bool,
    owner_name: &[u8],
) -> io::Result<Option<Value<'a>>> {
    let member = target_klass.class.methods[member_index].clone();
    if member.accessFlags & ACC_NATIVE != 0 {
        let handled = if is_static_native {
            nativeLookup::invokeStatic(
                loader,
                owner_name,
                member.name,
                member.descriptor,
                args,
                &mut io::sink(),
                0,
            )?
        } else {
            nativeLookup::invokeInstance(
                loader,
                owner_name,
                member.name,
                member.descriptor,
                args,
                &mut io::sink(),
                0,
            )?
        };
        if let Some(result) = handled {
            return Ok(result);
        }
    }
    let code = Code::parse(&member)?;
    let _ = bytecodeInterpreter::initialize(loader, target_klass, &mut io::sink(), 0);
    bytecodeInterpreter::execute(
        loader,
        target_klass,
        code,
        args,
        &mut io::sink(),
        0,
        member.accessFlags,
    )
}

/// Reads a field by name+descriptor on an object or declaring class.
fn read_named_field<'a>(
    loader: &ClassLoader<'a>,
    receiver: Value<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<Value<'a>> {
    match receiver {
        Value::Object(handle) => {
            let klass = object_klass(loader, handle)?;
            let slot = klass.instanceField(name, descriptor)?;
            let objects = loader.borrow_objects();
            let object = objects
                .get(handle)
                .ok_or_else(|| invalid("Invalid object reference"))?;
            Ok(object.fields.get(slot).copied().unwrap_or(Value::Null))
        }
        Value::ClassMirror(class) => {
            let klass = loader.load(class).map_err(|_| class_not_found(loader, class))?;
            let cell = klass.staticField(name, descriptor)?;
            Ok(cell.get())
        }
        _ => Err(class_cast(loader, "Field receiver is not an object")),
    }
}

fn write_named_field<'a>(
    loader: &ClassLoader<'a>,
    receiver: Value<'a>,
    name: &[u8],
    descriptor: &[u8],
    value: Value<'a>,
) -> io::Result<()> {
    match receiver {
        Value::Object(handle) => {
            let mut objects = loader.borrow_objects_mut();
            let object = objects
                .get_mut(handle)
                .ok_or_else(|| invalid("Invalid object reference"))?;
            let slot = object.klass.instanceField(name, descriptor)?;
            if let Some(cell) = object.fields.get_mut(slot) {
                *cell = value;
            }
            Ok(())
        }
        Value::ClassMirror(class) => {
            let klass = loader.load(class).map_err(|_| class_not_found(loader, class))?;
            let cell = klass.staticField(name, descriptor)?;
            cell.set(value);
            Ok(())
        }
        _ => Err(class_cast(loader, "Field receiver is not an object")),
    }
}

fn field_kind_byte(descriptor: &[u8]) -> u8 {
    descriptor.first().copied().unwrap_or(b'I')
}

/// Field accessor MethodHandle path (reference kinds 1-4).
fn field_method_handle<'a>(
    loader: &ClassLoader<'a>,
    state: &MemberState<'a>,
    ref_kind: u32,
    call_descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let type_name = class_name(loader, state.r#type)?;
    let descriptor = mirror_descriptor(&type_name);
    let kind = field_kind_byte(&descriptor);
    let args = &arguments[1..];
    match ref_kind {
        REF_GET_FIELD | REF_PUT_FIELD => {
            let Some(&receiver) = args.first() else {
                return Err(invalid("Field MethodHandle receiver"));
            };
            match ref_kind {
                REF_GET_FIELD => {
                    let value = read_named_field(loader, receiver, &state.name, &descriptor)?;
                    Ok(Some(finish_vh_result(
                        loader,
                        read_kind_value(value, kind),
                        kind,
                        call_descriptor,
                    )?))
                }
                _ => {
                    let Some(&value) = args.get(1) else {
                        return Err(invalid("Field MethodHandle value"));
                    };
                    write_named_field(
                        loader,
                        receiver,
                        &state.name,
                        &descriptor,
                        coerce_value(value, kind),
                    )?;
                    Ok(None)
                }
            }
        }
        REF_GET_STATIC | REF_PUT_STATIC => {
            let Some(clazz_name) = state.clazz.clone() else {
                return Err(no_such_field(loader, &state.name, &descriptor));
            };
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let cell = klass.staticField(&state.name, &descriptor)?;
            match ref_kind {
                REF_GET_STATIC => Ok(Some(finish_vh_result(
                    loader,
                    read_kind_value(cell.get(), kind),
                    kind,
                    call_descriptor,
                )?)),
                _ => {
                    let Some(&value) = args.first() else {
                        return Err(invalid("Field MethodHandle value"));
                    };
                    cell.set(coerce_value(value, kind));
                    Ok(None)
                }
            }
        }
        _ => Err(invalid("Unsupported field reference kind")),
    }
}

pub(crate) fn method_handle_invoke<'a>(
    loader: &ClassLoader<'a>,
    mh: usize,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    // Signature-polymorphic exactness check.
    if name != b"invoke" {
        let mh_type = object_field(loader, mh, b"type", b"Ljava/lang/invoke/MethodType;")?;
        let expected = method_type_descriptor(loader, mh_type)?;
        if std::env::var_os("JVM25_TRACE").is_some() {
            eprintln!(
                "[mh-invoke] name={} desc={} expected={}",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor),
                String::from_utf8_lossy(&expected)
            );
        }
        if expected != descriptor
            && (name != b"invokeBasic" || descriptor_kinds(&expected)? != descriptor_kinds(descriptor)?)
        {
            return Err(wrong_method_type(loader));
        }
    }
    let member_value = match object_field(loader, mh, b"member", b"Ljava/lang/invoke/MemberName;") {
        Ok(value) => value,
        Err(error) => {
            if std::env::var_os("JVM25_TRACE").is_some() {
                let mut chain = Vec::new();
                if let Ok(k) = object_klass(loader, mh) {
                    let mut cur = Some(k);
                    while let Some(klass) = cur {
                        chain.push(String::from_utf8_lossy(&klass.class.name).replace('/', "."));
                        cur = klass.super_klass();
                    }
                }
                eprintln!(
                    "[mh-invoke] member lookup failed chain={chain:?} name={} desc={}",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor)
                );
            }
            // BoundMethodHandle species carry no `member` field; their
            // behaviour lives in the LambdaForm.
            if let Ok(klass) = object_klass(loader, mh) {
                if klass.class.name.starts_with(b"java/lang/invoke/BoundMethodHandle") {
                    return lambda_form_invoke(loader, mh, name, descriptor, arguments);
                }
            }
            return Err(error);
        }
    };
    let Value::Object(member_handle) = member_value else {
        return Err(invalid("MethodHandle without MemberName"));
    };
    let state = read_member_state(loader, member_handle)?;
    let ref_kind = member_ref_kind(state.flags);
    if ref_kind <= REF_PUT_STATIC && ref_kind >= REF_GET_FIELD {
        return field_method_handle(loader, &state, ref_kind, descriptor, arguments);
    }
    let Some(clazz_name) = state.clazz.clone() else {
        return Err(no_such_method(loader, &state.name, b""));
    };
    let member_descriptor = member_descriptor(loader, &state)?;
    let args = &arguments[1..];
    // For `invoke` (non-exact), adapt call-site arguments to the MethodHandle
    // type's parameter kinds.
    let converted;
    let call_args: &[Value<'a>] = if name == b"invoke" {
        let mh_type = object_field(loader, mh, b"type", b"Ljava/lang/invoke/MethodType;")?;
        let mh_desc = method_type_descriptor(loader, mh_type)?;
        let (param_kinds, _) = descriptor_kinds(&mh_desc)?;
        if param_kinds.len() != args.len() {
            return Err(wrong_method_type(loader));
        }
        let mut out = Vec::with_capacity(args.len());
        for (kind, value) in param_kinds.iter().zip(args.iter()) {
            out.push(coerce_to_kind(loader, *value, *kind)?);
        }
        converted = out;
        &converted
    } else {
        args
    };
    let result = match ref_kind {
        REF_INVOKE_VIRTUAL | REF_INVOKE_INTERFACE => {
            let Some(&receiver) = call_args.first() else {
                return Err(invalid("MethodHandle receiver"));
            };
            if matches!(receiver, Value::Null) {
                return Err(null_pointer(loader));
            }
            let receiver_klass = receiver_klass(loader, receiver)?;
            let found = find_method_member(&receiver_klass, &state.name, &member_descriptor, false)
                .or_else(|| find_interface_method(&receiver_klass, &state.name, &member_descriptor));
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            execute_member(
                loader,
                &target_klass,
                index,
                call_args,
                false,
                target_klass.class.name,
            )?
        }
        REF_INVOKE_STATIC => {
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let found = find_method_member(&klass, &state.name, &member_descriptor, true)
                .or_else(|| {
                    find_method_member(&klass, &state.name, &member_descriptor, false)
                });
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            execute_member(
                loader,
                &target_klass,
                index,
                call_args,
                true,
                target_klass.class.name,
            )?
        }
        REF_INVOKE_SPECIAL => {
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let found = find_method_member(&klass, &state.name, &member_descriptor, false);
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            execute_member(
                loader,
                &target_klass,
                index,
                call_args,
                false,
                target_klass.class.name,
            )?
        }
        REF_NEW_INVOKE_SPECIAL => {
            if state.name != b"<init>" {
                return Err(invalid("NewInvokeSpecial requires <init>"));
            }
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0);
            let object = InstanceOop::new(klass.clone())?;
            let handle = loader.alloc_object(object);
            let found = klass
                .class
                .methods
                .iter()
                .position(|method| {
                    method.name == b"<init>"
                        && method.descriptor == member_descriptor
                        && method.accessFlags & ACC_STATIC == 0
                })
                .map(|index| (Arc::clone(&klass), index));
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, b"<init>", &member_descriptor));
            };
            let mut ctor_args = Vec::with_capacity(call_args.len() + 1);
            ctor_args.push(Value::Object(handle));
            ctor_args.extend_from_slice(call_args);
            execute_member(
                loader,
                &target_klass,
                index,
                &ctor_args,
                false,
                target_klass.class.name,
            )?;
            Some(Value::Object(handle))
        }
        _ => return Err(invalid("Unsupported MethodHandle reference kind")),
    };
    // Coerce the result to the call-site return kind for `invoke`.
    match result {
        Some(value) => {
            if name == b"invoke" {
                let (_, ret_kind) = descriptor_kinds(descriptor)?;
                if let Some(kind) = ret_kind {
                    let value = if kind == 4
                        && matches!(
                            value,
                            Value::Int(_) | Value::Long(_) | Value::Float(_) | Value::Double(_)
                        )
                    {
                        box_value(loader, value, return_tag(&member_descriptor))?
                    } else {
                        value
                    };
                    return Ok(Some(coerce_to_kind(loader, value, kind)?));
                }
            }
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

/// Evaluates one expression `LambdaForm.Name`: its `NamedFunction.member`
/// is invoked with the already-evaluated values referenced by the name's
/// `arguments` array (each element is a `Name` whose `index` selects a
/// slot in the form's value vector).
fn lambda_name_eval<'a>(
    loader: &ClassLoader<'a>,
    name_handle: usize,
    values: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let function_value = object_field(
        loader,
        name_handle,
        b"function",
        b"Ljava/lang/invoke/LambdaForm$NamedFunction;",
    )?;
    let Value::Object(function) = function_value else {
        return Err(invalid("Unsupported LambdaForm name"));
    };
    let args_value = object_field(loader, name_handle, b"arguments", b"[Ljava/lang/Object;")?;
    let mut call_args: Vec<Value<'a>> = Vec::new();
    if let Value::ObjectArray(arg_array) = args_value {
        let arg_handles: Vec<usize> = {
            let arrays = loader.obj_arrays.borrow();
            let array = arrays
                .get(arg_array)
                .ok_or_else(|| invalid("Invalid LambdaForm arguments"))?;
            array
                .elements
                .iter()
                .map(|v| match v {
                    Value::Object(h) => Ok(*h),
                    _ => Err(invalid("LambdaForm argument is not a name")),
                })
                .collect::<io::Result<Vec<_>>>()?
        };
        for arg_name in arg_handles {
            let index_value = object_field(loader, arg_name, b"index", b"S")?;
            let index = index_value.int()? as usize;
            call_args.push(values.get(index).copied().unwrap_or(Value::Null));
        }
    }
    let member_value = object_field(
        loader,
        function,
        b"member",
        b"Ljava/lang/invoke/MemberName;",
    )?;
    let Value::Object(member_handle) = member_value else {
        return Err(invalid("LambdaForm function without MemberName"));
    };
    let state = read_member_state(loader, member_handle)?;
    lambda_function_invoke(loader, state, &call_args)
}

/// Dispatches a `LambdaForm.NamedFunction` member: field accesses read or
/// write the first evaluated argument, `MethodHandle.invoke*` expressions
/// recurse into `method_handle_invoke`, and ordinary method references go
/// through the same resolution path as `DirectMethodHandle`.
fn lambda_function_invoke<'a>(
    loader: &ClassLoader<'a>,
    state: MemberState<'a>,
    call_args: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let ref_kind = member_ref_kind(state.flags);
    let member_descriptor = member_descriptor(loader, &state)?;
    if member_is_field(&state) {
        let type_name = class_name(loader, state.r#type)?;
        let field_descriptor = mirror_descriptor(&type_name);
        match ref_kind {
            REF_GET_FIELD => {
                let receiver = call_args.first().copied().unwrap_or(Value::Null);
                return read_named_field(loader, receiver, &state.name, &field_descriptor);
            }
            REF_GET_STATIC => {
                let clazz_name = state
                    .clazz
                    .clone()
                    .ok_or_else(|| no_such_field(loader, &state.name, &field_descriptor))?;
                let klass = loader
                    .load(&clazz_name)
                    .map_err(|_| class_not_found(loader, &clazz_name))?;
                let cell = klass.staticField(&state.name, &field_descriptor)?;
                return Ok(cell.get());
            }
            REF_PUT_FIELD => {
                let receiver = call_args.first().copied().unwrap_or(Value::Null);
                let value = call_args.get(1).copied().unwrap_or(Value::Null);
                write_named_field(loader, receiver, &state.name, &field_descriptor, value)?;
                return Ok(Value::Null);
            }
            REF_PUT_STATIC => {
                let clazz_name = state
                    .clazz
                    .clone()
                    .ok_or_else(|| no_such_field(loader, &state.name, &field_descriptor))?;
                let klass = loader
                    .load(&clazz_name)
                    .map_err(|_| class_not_found(loader, &clazz_name))?;
                let cell = klass.staticField(&state.name, &field_descriptor)?;
                cell.set(call_args.first().copied().unwrap_or(Value::Null));
                return Ok(Value::Null);
            }
            _ => return Err(invalid("Unsupported LambdaForm field kind")),
        }
    }
    // `MethodHandle.invoke{,Exact,Basic}` expressions take the target
    // handle as their first argument.
    if state.clazz.as_deref() == Some(b"java/lang/invoke/MethodHandle".as_slice())
        && matches!(state.name.as_slice(), b"invoke" | b"invokeExact" | b"invokeBasic")
    {
        let Some(&Value::Object(target)) = call_args.first() else {
            return Err(invalid("LambdaForm invoke without target"));
        };
        // invokeBasic is signature-polymorphic: present the target's own
        // type so the exactness check passes; invoke/invokeExact carry the
        // call-site type in the MemberName.
        let descriptor = if state.name == b"invokeBasic" {
            let mh_type = object_field(loader, target, b"type", b"Ljava/lang/invoke/MethodType;")?;
            method_type_descriptor(loader, mh_type)?
        } else {
            member_descriptor.clone()
        };
        let mut nested = Vec::with_capacity(call_args.len());
        nested.push(Value::Object(target));
        nested.extend_from_slice(&call_args[1..]);
        return method_handle_invoke(loader, target, &state.name, &descriptor, &nested)
            .map(|v| v.unwrap_or(Value::Null));
    }
    let Some(clazz_name) = state.clazz.clone() else {
        return Err(no_such_method(loader, &state.name, b""));
    };
    match ref_kind {
        REF_INVOKE_VIRTUAL | REF_INVOKE_INTERFACE => {
            let Some(&receiver) = call_args.first() else {
                return Err(invalid("MethodHandle receiver"));
            };
            if matches!(receiver, Value::Null) {
                return Err(null_pointer(loader));
            }
            let receiver_klass = receiver_klass(loader, receiver)?;
            let found = find_method_member(&receiver_klass, &state.name, &member_descriptor, false)
                .or_else(|| find_interface_method(&receiver_klass, &state.name, &member_descriptor));
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            execute_member(loader, &target_klass, index, call_args, false, target_klass.class.name)
                .map(|v| v.unwrap_or(Value::Null))
        }
        REF_INVOKE_STATIC | REF_INVOKE_SPECIAL => {
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let want_static = ref_kind == REF_INVOKE_STATIC;
            let found = find_method_member(&klass, &state.name, &member_descriptor, want_static)
                .or_else(|| {
                    find_method_member(&klass, &state.name, &member_descriptor, !want_static)
                });
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            execute_member(
                loader,
                &target_klass,
                index,
                call_args,
                want_static,
                target_klass.class.name,
            )
            .map(|v| v.unwrap_or(Value::Null))
        }
        REF_NEW_INVOKE_SPECIAL => {
            if state.name != b"<init>" {
                return Err(invalid("NewInvokeSpecial requires <init>"));
            }
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0);
            let object = InstanceOop::new(klass.clone())?;
            let handle = loader.alloc_object(object);
            let found = klass
                .class
                .methods
                .iter()
                .position(|method| {
                    method.name == b"<init>"
                        && method.descriptor == member_descriptor
                        && method.accessFlags & ACC_STATIC == 0
                })
                .map(|index| (Arc::clone(&klass), index));
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, b"<init>", &member_descriptor));
            };
            let mut ctor_args = Vec::with_capacity(call_args.len() + 1);
            ctor_args.push(Value::Object(handle));
            ctor_args.extend_from_slice(call_args);
            execute_member(
                loader,
                &target_klass,
                index,
                &ctor_args,
                false,
                target_klass.class.name,
            )?;
            Ok(Value::Object(handle))
        }
        _ => Err(invalid("Unsupported LambdaForm reference kind")),
    }
}

/// Executes a `BoundMethodHandle` (or any MethodHandle lacking a `member`
/// field) by interpreting its `LambdaForm`: the first `arity` names bind
/// the call arguments and the remaining names are evaluated in order,
/// each invoking its `NamedFunction` member.
fn lambda_form_invoke<'a>(
    loader: &ClassLoader<'a>,
    mh: usize,
    _name: &[u8],
    _descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let form_value = object_field(loader, mh, b"form", b"Ljava/lang/invoke/LambdaForm;")?;
    let Value::Object(form) = form_value else {
        return Err(invalid("MethodHandle without LambdaForm"));
    };
    let names_value = object_field(loader, form, b"names", b"[Ljava/lang/invoke/LambdaForm$Name;")?;
    let Value::ObjectArray(names_array) = names_value else {
        return Err(invalid("LambdaForm without names"));
    };
    let arity = object_field(loader, form, b"arity", b"I")?.int()? as usize;
    let result_index = object_field(loader, form, b"result", b"I")?.int()? as isize;
    let name_handles: Vec<usize> = {
        let arrays = loader.obj_arrays.borrow();
        let array = arrays
            .get(names_array)
            .ok_or_else(|| invalid("Invalid LambdaForm names"))?;
        array
            .elements
            .iter()
            .map(|v| match v {
                Value::Object(h) => Ok(*h),
                _ => Err(invalid("LambdaForm name is not an object")),
            })
            .collect::<io::Result<Vec<_>>>()?
    };
    let count = name_handles.len();
    let mut values: Vec<Value<'a>> = vec![Value::Empty; count];
    for i in 0..arity.min(count) {
        // arguments[0] is the MethodHandle receiver, matching names[0]
        // ("this"); the remaining slots are the call arguments.
        values[i] = arguments.get(i).copied().unwrap_or(Value::Null);
    }
    for i in arity..count {
        values[i] = lambda_name_eval(loader, name_handles[i], &values)?;
    }
    let index = if result_index < 0 {
        count.saturating_sub(1)
    } else {
        result_index as usize
    };
    Ok(Some(values.get(index).copied().unwrap_or(Value::Null)))
}

/// `MethodHandle.linkTo{Static,Virtual,Special,Interface,Native}`: the
/// signature-polymorphic linkers used by `java.lang.invoke` internals
/// (DirectMethodHandle, VarHandleGuards, LambdaForm stubs). The first
/// argument is the target `MemberName`; for receiver kinds the receiver
/// follows it, then the call arguments.
fn link_to<'a>(
    loader: &ClassLoader<'a>,
    link: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let Some(&Value::Object(member_handle)) = arguments.first() else {
        return Err(invalid("linkTo requires a MemberName"));
    };
    let state = read_member_state(loader, member_handle)?;
    if member_is_field(&state) {
        return Err(invalid("linkTo requires a method MemberName"));
    }
    let Some(clazz_name) = state.clazz.clone() else {
        return Err(no_such_method(loader, &state.name, b""));
    };
    let member_descriptor = member_descriptor(loader, &state)?;
    let has_receiver = link != b"linkToStatic";
    let (receiver, call_args) = if has_receiver {
        let Some(&receiver) = arguments.get(1) else {
            return Err(invalid("linkTo receiver"));
        };
        if matches!(receiver, Value::Null) {
            return Err(null_pointer(loader));
        }
        (Some(receiver), &arguments[2..])
    } else {
        (None, &arguments[1..])
    };

    if link == b"linkToNative" {
        // Route to the native dispatch tables; the MemberName's declaring
        // class is the native owner.
        let klass = loader
            .load(&clazz_name)
            .map_err(|_| class_not_found(loader, &clazz_name))?;
        let is_static = (member_ref_kind(state.flags) == REF_INVOKE_STATIC)
            || (klass
                .class
                .methods
                .iter()
                .find(|m| m.name == state.name && m.descriptor == member_descriptor)
                .is_some_and(|m| m.accessFlags & ACC_STATIC != 0));
        let owner = target_owner_name(&klass, &state.name, &member_descriptor);
        let mut native_args = Vec::with_capacity(call_args.len() + 1);
        if !is_static {
            native_args.push(receiver.unwrap_or(Value::Null));
        }
        native_args.extend_from_slice(call_args);
        let result = if is_static {
            crate::share::prims::nativeLookup::invokeStatic(
                loader,
                &owner,
                &state.name,
                &member_descriptor,
                &native_args,
                &mut io::sink(),
                0,
            )?
        } else {
            crate::share::prims::nativeLookup::invokeInstance(
                loader,
                &owner,
                &state.name,
                &member_descriptor,
                &native_args,
                &mut io::sink(),
                0,
            )?
        };
        return match result {
            Some(Some(value)) => Ok(Some(value)),
            Some(None) => Ok(None),
            None => Err(invalid("linkToNative target is not a native method")),
        };
    }

    let result = match link {
        b"linkToStatic" => {
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let found = find_method_member(&klass, &state.name, &member_descriptor, true)
                .or_else(|| find_method_member(&klass, &state.name, &member_descriptor, false));
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            execute_member(
                loader,
                &target_klass,
                index,
                call_args,
                true,
                target_klass.class.name,
            )?
        }
        b"linkToVirtual" | b"linkToInterface" => {
            let receiver = receiver.unwrap_or(Value::Null);
            let receiver_klass = receiver_klass(loader, receiver)?;
            let found = find_method_member(&receiver_klass, &state.name, &member_descriptor, false)
                .or_else(|| find_interface_method(&receiver_klass, &state.name, &member_descriptor));
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            let mut full_args = Vec::with_capacity(call_args.len() + 1);
            full_args.push(receiver);
            full_args.extend_from_slice(call_args);
            execute_member(
                loader,
                &target_klass,
                index,
                &full_args,
                false,
                target_klass.class.name,
            )?
        }
        b"linkToSpecial" => {
            let klass = loader
                .load(&clazz_name)
                .map_err(|_| class_not_found(loader, &clazz_name))?;
            let found = find_method_member(&klass, &state.name, &member_descriptor, false);
            let Some((target_klass, index)) = found else {
                return Err(no_such_method(loader, &state.name, &member_descriptor));
            };
            let mut full_args = Vec::with_capacity(call_args.len() + 1);
            full_args.push(receiver.unwrap_or(Value::Null));
            full_args.extend_from_slice(call_args);
            execute_member(
                loader,
                &target_klass,
                index,
                &full_args,
                false,
                target_klass.class.name,
            )?
        }
        _ => return Err(invalid("Unsupported linkTo variant")),
    };
    Ok(result)
}

/// Declaring class name for a member resolved on `klass` (walks
/// superclasses so inherited natives dispatch under the right owner).
fn target_owner_name<'a>(
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> Vec<u8> {
    let mut current = Some(Arc::clone(klass));
    while let Some(k) = current {
        if k
            .class
            .methods
            .iter()
            .any(|m| m.name == name && m.descriptor == descriptor)
        {
            return k.class.name.to_vec();
        }
        current = k.super_klass();
    }
    klass.class.name.to_vec()
}

pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    if owner == b"java/lang/invoke/MethodHandle"
        && matches!(
            name,
            b"linkToStatic" | b"linkToVirtual" | b"linkToSpecial" | b"linkToInterface" | b"linkToNative"
        )
    {
        let _ = descriptor;
        return link_to(loader, name, arguments).map(|result| Some(result));
    }
    if owner != b"java/lang/invoke/MethodHandleNatives" {
        return Ok(None);
    }
    let result = match (name, descriptor) {
        (b"init", b"(Ljava/lang/invoke/MemberName;Ljava/lang/Object;)V") => {
            natives_init(loader, arguments)?;
            None
        }
        (b"expand", b"(Ljava/lang/invoke/MemberName;)V") => {
            natives_expand(loader, arguments)?;
            None
        }
        (
            b"resolve",
            b"(Ljava/lang/invoke/MemberName;Ljava/lang/Class;IZ)Ljava/lang/invoke/MemberName;",
        ) => Some(natives_resolve(loader, arguments)?),
        (b"objectFieldOffset", b"(Ljava/lang/invoke/MemberName;)J") => {
            Some(Value::Long(member_field_offset(loader, arguments)?))
        }
        (b"staticFieldOffset", b"(Ljava/lang/invoke/MemberName;)J") => {
            Some(Value::Long(member_field_offset(loader, arguments)?))
        }
        (b"staticFieldBase", b"(Ljava/lang/invoke/MemberName;)Ljava/lang/Object;") => {
            Some(natives_static_field_base(loader, arguments)?)
        }
        (b"getMemberVMInfo", b"(Ljava/lang/invoke/MemberName;)Ljava/lang/Object;") => {
            Some(natives_member_vm_info(loader, arguments)?)
        }
        (
            b"setCallSiteTargetNormal",
            b"(Ljava/lang/invoke/CallSite;Ljava/lang/invoke/MethodHandle;)V",
        )
        | (
            b"setCallSiteTargetVolatile",
            b"(Ljava/lang/invoke/CallSite;Ljava/lang/invoke/MethodHandle;)V",
        ) => {
            natives_set_call_site_target(loader, arguments)?;
            None
        }
        (b"getNamedCon", b"(I[Ljava/lang/Object;)I") => Some(Value::Int(0)),
        (
            b"copyOutBootstrapArguments",
            b"(Ljava/lang/Class;[III[Ljava/lang/Object;IZLjava/lang/Object;)V",
        ) => None,
        // VM-entry points used by the HotSpot invokedynamic/condy linkage
        // path; this interpreter resolves call sites through its own
        // lambda pipeline and never reaches these from Java bytecode.
        (b"linkCallSite", _) | (b"linkCallSiteImpl", _) | (b"linkDynamicConstant", _)
        | (b"linkDynamicConstantImpl", _) | (b"linkMethod", _) | (b"linkMethodImpl", _) => {
            Some(Value::Null)
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

pub(crate) fn invoke_instance<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    if owner == b"java/lang/invoke/VarHandle" && access_op(name).is_some() {
        if let Some(vh) = var_handle_receiver(loader, arguments)? {
            if let Some(result) = var_handle_access(loader, vh, name, descriptor, &arguments[1..])? {
                return Ok(Some(Some(result)));
            }
            return Ok(Some(None));
        }
        return Ok(None);
    }
    if owner == b"java/lang/invoke/MethodHandle"
        && matches!(name, b"invokeExact" | b"invoke" | b"invokeBasic")
    {
        if let Some(mh) = method_handle_receiver(loader, arguments)? {
            if let Some(result) =
                method_handle_invoke(loader, mh, name, descriptor, arguments)?
            {
                return Ok(Some(Some(result)));
            }
            return Ok(Some(None));
        }
        return Ok(None);
    }
    if owner.starts_with(b"java/lang/invoke/") {
        // Subclass-typed call sites (e.g. DirectMethodHandle receivers).
        if access_op(name).is_some() {
            if let Some(vh) = var_handle_receiver(loader, arguments)? {
                if let Some(result) =
                    var_handle_access(loader, vh, name, descriptor, &arguments[1..])?
                {
                    return Ok(Some(Some(result)));
                }
                return Ok(Some(None));
            }
        }
        if matches!(name, b"invokeExact" | b"invoke" | b"invokeBasic") {
            if let Some(mh) = method_handle_receiver(loader, arguments)? {
                if let Some(result) =
                    method_handle_invoke(loader, mh, name, descriptor, arguments)?
                {
                    return Ok(Some(Some(result)));
                }
                return Ok(Some(None));
            }
        }
    }
    Ok(None)
}

/// Element descriptor for `VarHandleByteArrayAs*` view classes:
/// (size in bytes, is_float, is_double, is_unsigned_16).
fn byte_view_kind(class_name: &[u8]) -> Option<(usize, bool, bool, bool)> {
    if class_name.windows(6).any(|w| w == b"AsInts") {
        Some((4, false, false, false))
    } else if class_name.windows(7).any(|w| w == b"AsLongs") {
        Some((8, false, false, false))
    } else if class_name.windows(8).any(|w| w == b"AsShorts") {
        Some((2, false, false, false))
    } else if class_name.windows(7).any(|w| w == b"AsChars") {
        Some((2, false, false, true))
    } else if class_name.windows(8).any(|w| w == b"AsFloats") {
        Some((4, true, false, false))
    } else if class_name.windows(9).any(|w| w == b"AsDoubles") {
        Some((8, false, true, false))
    } else {
        None
    }
}

fn byte_view_oob() -> io::Error {
    io::Error::other("java.lang.IndexOutOfBoundsException: Index out of bounds")
}

fn byte_view_read(
    bytes: &[i8],
    offset: usize,
    size: usize,
    be: bool,
) -> io::Result<u64> {
    if offset + size > bytes.len() {
        return Err(byte_view_oob());
    }
    let mut raw = [0u8; 8];
    for (i, &byte) in bytes[offset..offset + size].iter().enumerate() {
        raw[if be { size - 1 - i } else { i }] = byte as u8;
    }
    Ok(u64::from_le_bytes(raw))
}

fn byte_view_write(
    bytes: &mut [i8],
    offset: usize,
    size: usize,
    be: bool,
    raw: u64,
) -> io::Result<()> {
    if offset + size > bytes.len() {
        return Err(byte_view_oob());
    }
    let mut raw_bytes = raw.to_le_bytes();
    if be {
        raw_bytes[..size].reverse();
    }
    for (i, &byte) in raw_bytes[..size].iter().enumerate() {
        bytes[offset + i] = byte as i8;
    }
    Ok(())
}

/// Raw element value -> Java `Value` for the view's element type.
fn byte_view_value(
    raw: u64,
    size: usize,
    is_float: bool,
    is_double: bool,
    unsigned16: bool,
) -> Value<'static> {
    match (size, is_float, is_double) {
        (4, true, _) => Value::Float(f32::from_bits(raw as u32)),
        (8, _, true) => Value::Double(f64::from_bits(raw)),
        (8, _, _) => Value::Long(raw as i64),
        (4, _, _) => Value::Int(raw as u32 as i32),
        (2, _, _) if unsigned16 => Value::Int(raw as u16 as i32),
        (2, _, _) => Value::Int((raw as u16) as i16 as i32),
        _ => Value::Int(raw as u8 as i8 as i32),
    }
}

/// Java `Value` -> raw element bits for the view's element type.
fn byte_view_raw(
    value: Value<'_>,
    size: usize,
    is_float: bool,
    is_double: bool,
) -> io::Result<u64> {
    match (size, is_float, is_double, value) {
        (8, _, true, Value::Double(v)) => Ok(v.to_bits()),
        (8, _, true, Value::Float(v)) => Ok(f64::from(v).to_bits()),
        (8, _, false, Value::Long(v)) => Ok(v as u64),
        (8, _, false, Value::Int(v)) => Ok((v as i64) as u64),
        (4, true, _, Value::Float(v)) => Ok(u64::from(v.to_bits())),
        (4, true, _, Value::Int(v)) => Ok(u64::from((v as f32).to_bits())),
        (4, _, _, Value::Int(v)) => Ok((v as u32) as u64),
        (4, _, _, Value::Long(v)) => Ok((v as u32) as u64),
        (2, _, _, Value::Int(v)) => Ok((v as u16) as u64),
        (2, _, _, Value::Long(v)) => Ok((v as u16) as u64),
        (1, _, _, Value::Int(v)) => Ok((v as u8) as u64),
        _ => Err(invalid("VarHandle byte-view value type mismatch")),
    }
}

/// VarHandle access on `VarHandleByteArrayAs*` receivers: coordinates are
/// `(byte[] array, int index)` and the index counts view elements.
fn byte_array_view_access<'a>(
    loader: &ClassLoader<'a>,
    vh: usize,
    class_name: &[u8],
    op: AccessOp,
    mode_args: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let Some((size, is_float, is_double, unsigned16)) = byte_view_kind(class_name) else {
        return Ok(None);
    };
    let Value::PrimitiveArray(array) = mode_args.first().copied().unwrap_or(Value::Null) else {
        return Err(null_pointer(loader));
    };
    let Value::Int(index) = mode_args.get(1).copied().unwrap_or(Value::Null) else {
        return Err(invalid("VarHandle byte-view index"));
    };
    if index < 0 {
        return Err(byte_view_oob());
    }
    // The index coordinate is a byte offset into the backing array.
    let offset = index as usize;
    let be = matches!(
        object_field(loader, vh, b"be", b"Z").unwrap_or(Value::Int(0)),
        Value::Int(flag) if flag != 0
    );
    let value_args = &mode_args[2.min(mode_args.len())..];

    let mut arrays = loader.arrays.borrow_mut();
    let array_ref = arrays
        .get_mut(array)
        .ok_or_else(|| invalid("Invalid array reference"))?;
    let crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) = array_ref else {
        return Err(invalid("Expected byte array"));
    };
    if offset + size > bytes.len() {
        if std::env::var_os("JVM25_TRACE").is_some() {
            eprintln!(
                "[vh-oob] {} index={} size={} offset={} len={}",
                String::from_utf8_lossy(class_name),
                index,
                size,
                offset,
                bytes.len()
            );
        }
        return Err(byte_view_oob());
    }

    let result = match op {
        AccessOp::Get => {
            let raw = byte_view_read(bytes, offset, size, be)?;
            return Ok(Some(byte_view_value(raw, size, is_float, is_double, unsigned16)));
        }
        AccessOp::Set => {
            let raw = byte_view_raw(
                value_args.first().copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            byte_view_write(bytes, offset, size, be, raw)?;
            return Ok(None);
        }
        AccessOp::CompareAndSet | AccessOp::WeakCompareAndSet => {
            let expected = byte_view_raw(
                value_args.first().copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            let update = byte_view_raw(
                value_args.get(1).copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            let current = byte_view_read(bytes, offset, size, be)?;
            if current == expected {
                byte_view_write(bytes, offset, size, be, update)?;
            }
            Value::Int(i32::from(current == expected))
        }
        AccessOp::CompareAndExchange => {
            let expected = byte_view_raw(
                value_args.first().copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            let update = byte_view_raw(
                value_args.get(1).copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            let current = byte_view_read(bytes, offset, size, be)?;
            if current == expected {
                byte_view_write(bytes, offset, size, be, update)?;
            }
            byte_view_value(current, size, is_float, is_double, unsigned16)
        }
        AccessOp::GetAndSet => {
            let update = byte_view_raw(
                value_args.first().copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            let current = byte_view_read(bytes, offset, size, be)?;
            byte_view_write(bytes, offset, size, be, update)?;
            byte_view_value(current, size, is_float, is_double, unsigned16)
        }
        AccessOp::GetAndAdd => {
            let delta = byte_view_raw(
                value_args.first().copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            let current = byte_view_read(bytes, offset, size, be)?;
            let updated = if is_float {
                (f32::from_bits(current as u32) + f32::from_bits(delta as u32)).to_bits() as u64
            } else if is_double {
                (f64::from_bits(current) + f64::from_bits(delta)).to_bits()
            } else {
                let width = size * 8;
                let mask = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
                current.wrapping_add(delta) & mask
            };
            byte_view_write(bytes, offset, size, be, updated)?;
            byte_view_value(current, size, is_float, is_double, unsigned16)
        }
        AccessOp::GetAndBitwiseOr
        | AccessOp::GetAndBitwiseAnd
        | AccessOp::GetAndBitwiseXor => {
            let delta = byte_view_raw(
                value_args.first().copied().unwrap_or(Value::Null),
                size,
                is_float,
                is_double,
            )?;
            let current = byte_view_read(bytes, offset, size, be)?;
            let width = size * 8;
            let mask = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
            let updated = match op {
                AccessOp::GetAndBitwiseOr => current | delta,
                AccessOp::GetAndBitwiseAnd => current & delta,
                _ => current ^ delta,
            } & mask;
            byte_view_write(bytes, offset, size, be, updated)?;
            byte_view_value(current, size, is_float, is_double, unsigned16)
        }
    };
    Ok(Some(result))
}
