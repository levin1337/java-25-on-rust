use crate::share::{
    classfile::{
        classFileParser::decodeString,
        classLoader::{ClassLoader, ClassOrigin},
    },
    interpreter::bytecodeInterpreter::{self, Value},
    oops::instanceOop::InstanceOop,
    runtime::javaCalls::{JavaCallArguments, JavaCalls},
};
use std::io;

/// Returns the single-letter primitive tag for a class name given either in
/// internal mirror form (`:Z`), dotted Java form (`boolean`), or bare tag (`Z`).
pub(crate) fn primitive_tag(name: &[u8]) -> Option<&'static [u8]> {
    if let Some(tag) = name.strip_prefix(b":") {
        return match tag {
            b"Z" => Some(b"Z"),
            b"B" => Some(b"B"),
            b"C" => Some(b"C"),
            b"S" => Some(b"S"),
            b"I" => Some(b"I"),
            b"J" => Some(b"J"),
            b"F" => Some(b"F"),
            b"D" => Some(b"D"),
            b"V" => Some(b"V"),
            _ => None,
        };
    }
    match name {
        b"boolean" => Some(b"Z"),
        b"byte" => Some(b"B"),
        b"char" => Some(b"C"),
        b"short" => Some(b"S"),
        b"int" => Some(b"I"),
        b"long" => Some(b"J"),
        b"float" => Some(b"F"),
        b"double" => Some(b"D"),
        b"void" => Some(b"V"),
        _ => None,
    }
}

fn primitiveName(tag: &[u8]) -> Option<&'static str> {
    match tag {
        b"Z" => Some("boolean"),
        b"B" => Some("byte"),
        b"C" => Some("char"),
        b"S" => Some("short"),
        b"I" => Some("int"),
        b"J" => Some("long"),
        b"F" => Some("float"),
        b"D" => Some("double"),
        b"V" => Some("void"),
        _ => None,
    }
}

pub(crate) fn primitiveClass(value: Value<'_>) -> io::Result<Value<'_>> {
    let name = match value {
        Value::HostString(name) => std::borrow::Cow::Borrowed(name),
        Value::String(bytes) => std::borrow::Cow::Owned(decodeString(bytes)?),
        _ => return Err(io::Error::other("Expected primitive name")),
    };
    let tag: &'static [u8] = match name.as_ref() {
        "boolean" => b"Z",
        "byte" => b"B",
        "char" => b"C",
        "short" => b"S",
        "int" => b"I",
        "long" => b"J",
        "float" => b"F",
        "double" => b"D",
        "void" => b"V",
        _ => return Err(io::Error::other(format!("Unknown primitive type {name}"))),
    };
    Ok(Value::ClassMirror(match tag {
        b"Z" => b":Z",
        b"B" => b":B",
        b"C" => b":C",
        b"S" => b":S",
        b"I" => b":I",
        b"J" => b":J",
        b"F" => b":F",
        b"D" => b":D",
        _ => b":V",
    }))
}

pub(crate) fn classOf<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Value<'a>> {
    let invalid = || io::Error::other("Invalid getClass receiver");
    let name: &'a [u8] = match value {
        Value::Object(handle) => {
            loader
                .objects
                .borrow()
                .get(handle)
                .ok_or_else(invalid)?
                .klass
                .class
                .name
        }
        Value::String(_) | Value::HostString(_) => b"java/lang/String",
        Value::Arguments(_) => b"[Ljava/lang/String;",
        Value::ClassMirror(_) => b"java/lang/Class",
        Value::Output => b"java/io/PrintStream",
        Value::PrimitiveArray(handle) => loader
            .arrays
            .borrow()
            .get(handle)
            .ok_or_else(invalid)?
            .descriptor(),
        Value::ObjectArray(handle) => {
            let component = loader
                .obj_arrays
                .borrow()
                .get(handle)
                .ok_or_else(invalid)?
                .component;
            let mut descriptor = Vec::with_capacity(component.len() + 3);
            descriptor.push(b'[');
            if component.starts_with(b"[") {
                descriptor.extend_from_slice(component);
            } else {
                descriptor.push(b'L');
                descriptor.extend_from_slice(component);
                descriptor.push(b';');
            }
            loader.storage.alloc_slice_copy(&descriptor)
        }
        Value::Null => return Err(io::Error::other("NullPointerException")),
        _ => return Err(invalid()),
    };
    Ok(Value::ClassMirror(name))
}

/// Resolves a `java.lang.Class` value — either a `ClassMirror` literal or a
/// materialized `java.lang.Class` object — to its internal class name.
pub(crate) fn class_value_name<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
) -> io::Result<Option<&'a [u8]>> {
    match value {
        Value::ClassMirror(name) => Ok(Some(name)),
        Value::Object(handle) => {
            let is_class_object = {
                let objects = loader.borrow_objects();
                objects
                    .get(handle)
                    .is_some_and(|object| object.klass.class.name == b"java/lang/Class")
            };
            if !is_class_object {
                return Ok(None);
            }
            Ok(loader.class_object_names.borrow().get(&handle).copied())
        }
        _ => Ok(None),
    }
}

pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let Some((receiver, arguments)) = arguments.split_first() else {
        return Ok(None);
    };
    // Accept both `ClassMirror` literals and materialized `java.lang.Class`
    // objects (produced by `materialize_class` for field/method access).
    let class = match class_value_name(loader, *receiver)? {
        Some(class) => class,
        None => return Ok(None),
    };
    if name == b"getResource" && descriptor == b"(Ljava/lang/String;)Ljava/net/URL;" {
        let resource = arguments.first().copied().ok_or_else(|| io::Error::other("Missing resource name"))?;
        return crate::share::prims::urlNatives::classResource(loader, class, resource).map(Some);
    }
    if name == b"isAssignableFrom" && descriptor == b"(Ljava/lang/Class;)Z" {
        let target = class;
        let source = match arguments.first().copied().unwrap_or(Value::Null) {
            Value::Null => return Err(io::Error::other("NullPointerException")),
            value => match class_value_name(loader, value)? {
                Some(source) => source,
                None => return Ok(Some(Value::Int(0))),
            },
        };
        let assignable = if target == source {
            true
        } else if target.starts_with(b":") || source.starts_with(b":") {
            false
        } else if source.starts_with(b"[") {
            loader.classAssignable(source, target)?
        } else {
            let source = loader.load(source)?;
            crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(
                &source, target,
            ) || crate::share::classfile::systemDictionary::SystemDictionary::implements_interface(
                &source, target,
            )
        };
        return Ok(Some(Value::Int(i32::from(assignable))));
    }
    let value = match (name, descriptor) {
        (b"desiredAssertionStatus", b"()Z") => Value::Int(0),
        (b"isArray", b"()Z") => Value::Int(i32::from(class.starts_with(b"["))),
        (b"isPrimitive", b"()Z") => {
            let prim = primitive_tag(class).is_some();
            if std::env::var_os("JVM25_TRACE_ISPRIM").is_some() && prim {
                eprintln!(
                    "[isprim] class={} -> {} site={}",
                    String::from_utf8_lossy(class),
                    prim,
                    crate::share::interpreter::bytecodeInterpreter::current_frame_site()
                );
            }
            Value::Int(i32::from(prim))
        }
        (b"getName", b"()Ljava/lang/String;") => {
            if let Some(name) = class.strip_prefix(b":").and_then(primitiveName) {
                return Ok(Some(Value::HostString(name)));
            }
            let name = decodeString(class)?.replace('/', ".");
            Value::HostString(loader.storage.alloc_str(&name))
        }
        (b"isInterface", b"()Z") => Value::Int(i32::from(
            !class.starts_with(b"[")
                && !class.starts_with(b":")
                && loader.load(class)?.class.accessFlags & 0x0200 != 0,
        )),
        (b"getClassLoader", b"()Ljava/lang/ClassLoader;")
        | (b"getClassLoader0", b"()Ljava/lang/ClassLoader;") => {
            if class.starts_with(b"[") || class.starts_with(b":") {
                Value::Null
            } else if loader.bootImage.is_some() {
                if loader.is_boot_class(class)? {
                    Value::Null
                } else {
                    Value::Object(loader.systemClassLoaderObject()?)
                }
            } else {
                Value::Object(loader.systemClassLoaderObject()?)
            }
        }
        (b"getInterfaces", b"()[Ljava/lang/Class;") => {
            if class.starts_with(b"[") || class.starts_with(b":") {
                return Ok(Some(crate::share::prims::reflectionNatives::class_array(
                    loader, &[],
                )?));
            }
            let klass = loader.load(class)?;
            let names: Vec<&'a [u8]> = klass.interfaces.iter().map(|i| i.class.name).collect();
            return Ok(Some(crate::share::prims::reflectionNatives::class_array(
                loader, &names,
            )?));
        }
        (b"getModifiers", b"()I") => {
            Value::Int(crate::share::prims::reflectionNatives::class_modifiers(loader, class)?)
        }
        (b"getSimpleName", b"()Ljava/lang/String;") => {
            if class.starts_with(b"[") {
                let component = class[1..]
                    .strip_prefix(b"L")
                    .and_then(|c| c.strip_suffix(b";"))
                    .unwrap_or(&class[1..]);
                let simple = component.rsplit(|b| *b == b'/').next().unwrap_or(component);
                return Ok(Some(Value::HostString(
                    loader.storage.alloc_str(&String::from_utf8_lossy(simple)),
                )));
            }
            if let Some(name) = class.strip_prefix(b":").and_then(primitiveName) {
                return Ok(Some(Value::HostString(name)));
            }
            let dotted = decodeString(class)?.replace('/', ".");
            let simple = dotted.rsplit('.').next().unwrap_or(&dotted);
            Value::HostString(loader.storage.alloc_str(simple))
        }
        (b"getPackageName", b"()Ljava/lang/String;") => {
            if class.starts_with(b":") {
                return Ok(Some(Value::HostString("java.lang")));
            }
            let mut name = class;
            if name.starts_with(b"[") {
                name = &name[1..];
                if name.starts_with(b"L") {
                    name = &name[1..name.len() - 1];
                } else if let [tag] = name {
                    name = match *tag {
                        b'Z' => b":Z",
                        b'B' => b":B",
                        b'C' => b":C",
                        b'S' => b":S",
                        b'I' => b":I",
                        b'J' => b":J",
                        b'F' => b":F",
                        b'D' => b":D",
                        _ => b":V",
                    };
                }
            }
            if name.starts_with(b":") {
                return Ok(Some(Value::HostString("java.lang")));
            }
            let dotted = decodeString(name)?.replace('/', ".");
            let package = dotted.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
            Value::HostString(loader.storage.alloc_str(package))
        }
        (b"getComponentType", b"()Ljava/lang/Class;") => {
            if !class.starts_with(b"[") {
                return Ok(Some(Value::Null));
            }
            let component = &class[1..];
            let name = if component.starts_with(b"L") {
                &component[1..component.len() - 1]
            } else if let [tag] = component {
                match *tag {
                    b'Z' => b":Z".as_slice(),
                    b'B' => b":B".as_slice(),
                    b'C' => b":C".as_slice(),
                    b'S' => b":S".as_slice(),
                    b'I' => b":I".as_slice(),
                    b'J' => b":J".as_slice(),
                    b'F' => b":F".as_slice(),
                    b'D' => b":D".as_slice(),
                    _ => return Ok(Some(Value::Null)),
                }
            } else {
                component
            };
            Value::ClassMirror(loader.storage.alloc_slice_copy(name))
        }
        (b"getSuperclass", b"()Ljava/lang/Class;") => {
            if class.starts_with(b"[") || class.starts_with(b":") {
                return Ok(Some(Value::Null));
            }
            let klass = loader.load(class)?;
            match klass.super_klass() {
                Some(super_klass) => Value::ClassMirror(super_klass.class.name),
                None => Value::Null,
            }
        }
        (b"isInstance", b"(Ljava/lang/Object;)Z") => {
            let arg = arguments.first().copied().unwrap_or(Value::Null);
            let result = !matches!(arg, Value::Null)
                && loader.referenceAssignable(arg, class).unwrap_or(false);
            return Ok(Some(Value::Int(i32::from(result))));
        }
        (b"cast", b"(Ljava/lang/Object;)Ljava/lang/Object;") => {
            let arg = arguments.first().copied().unwrap_or(Value::Null);
            let ok = matches!(arg, Value::Null)
                || loader.referenceAssignable(arg, class).unwrap_or(false);
            if !ok {
                let stack = loader
                    .call_stack
                    .snapshot()
                    .iter()
                    .rev()
                    .take(6)
                    .map(|name| String::from_utf8_lossy(name).into_owned())
                    .collect::<Vec<_>>()
                    .join(" <- ");
                return Err(io::Error::other(format!(
                    "ClassCastException: cannot cast to {} at {stack}",
                    String::from_utf8_lossy(class)
                )));
            }
            return Ok(Some(arg));
        }
        (b"asSubclass", b"(Ljava/lang/Class;)Ljava/lang/Class;") => {
            let arg = arguments.first().copied().unwrap_or(Value::Null);
            // thisClass.asSubclass(S) succeeds iff thisClass is a subclass of (or
            // equal to) S — the same relation as `S.isAssignableFrom(thisClass)`.
            let ok = match class_value_name(loader, arg)? {
                Some(other) => {
                    if class == other {
                        true
                    } else if class.starts_with(b":") || other.starts_with(b":") {
                        false
                    } else if class.starts_with(b"[") {
                        loader.classAssignable(class, other).unwrap_or(false)
                    } else if other.starts_with(b"[") {
                        false
                    } else {
                        match loader.load(class) {
                            Ok(source) => {
                                crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(&source, other)
                                    || crate::share::classfile::systemDictionary::SystemDictionary::implements_interface(&source, other)
                            }
                            Err(e) => {
                                eprintln!("[asSubclass] load {} failed: {e}", String::from_utf8_lossy(class));
                                false
                            }
                        }
                    }
                }
                _ => false,
            };
            if !ok {
                let stack = loader
                    .call_stack
                    .snapshot()
                    .iter()
                    .rev()
                    .take(6)
                    .map(|name| String::from_utf8_lossy(name).into_owned())
                    .collect::<Vec<_>>()
                    .join(" <- ");
                return Err(io::Error::other(format!(
                    "ClassCastException: asSubclass {} from {arg:?} at {stack}",
                    String::from_utf8_lossy(class)
                )));
            }
            return Ok(Some(Value::ClassMirror(class)));
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

/// Materializes a `Value::ClassMirror` into a real `java.lang.Class`
/// instance so instance fields (`classValueMap`, `annotationData`, ...)
/// have storage. Results are cached per class name so field writes
/// persist across accesses.
pub(crate) fn materialize_class<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
) -> io::Result<Option<Value<'a>>> {
    let Value::ClassMirror(name) = value else {
        return Ok(None);
    };
    let key = name.to_vec();
    if let Some(&handle) = loader.class_objects.borrow().get(&key) {
        return Ok(Some(Value::Object(handle)));
    }
    let klass = loader.load(b"java/lang/Class")?;
    let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
        loader,
        &klass,
        &mut io::sink(),
        0,
    );
    let componentType = invoke(loader, b"getComponentType", b"()Ljava/lang/Class;", &[value])?
        .unwrap_or(Value::Null);
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle) {
            if let Ok(slot) = klass.instanceField(b"primitive", b"Z") {
                object.fields[slot] = Value::Int(i32::from(primitive_tag(name).is_some()));
            }
            if let Ok(slot) = klass.instanceField(b"componentType", b"Ljava/lang/Class;") {
                object.fields[slot] = componentType;
            }
            // Best-effort `name` field (JDK keeps the binary name there on
            // some versions); harmless when absent.
            if let Ok(slot) = klass.instanceField(b"name", b"Ljava/lang/String;")
                && slot < object.fields.len()
            {
                let dotted = if name.starts_with(b"[") {
                    // Array classes: "[Ljava/lang/String;" -> "[Ljava.lang.String;"
                    let text = String::from_utf8_lossy(name).replace('/', ".");
                    loader.storage.alloc_str(&text)
                } else {
                    let text = String::from_utf8_lossy(name).replace('/', ".");
                    loader.storage.alloc_str(&text)
                };
                object.fields[slot] = Value::HostString(dotted);
            }
        }
    }
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(handle));
    loader.class_object_names.borrow_mut().insert(handle, name);
    loader.class_objects.borrow_mut().insert(key, handle);
    let recorded = loader.class_protection_domain(name);
    let domain = recorded
        .map(Value::Object)
        .or_else(|| {
            loader
                .class_origin(name)
                .and_then(|origin| protection_domain_for_origin(loader, &origin).ok())
        });
    if recorded.is_some() && std::env::var_os("JVM25_TRACE_PD").is_some() {
        if let Some(Value::Object(handle)) = domain {
            trace_pd_url(loader, handle, name);
        }
    }
    if let Some(domain) = domain
        && let Ok(slot) = klass.instanceField(
            b"protectionDomain",
            b"Ljava/security/ProtectionDomain;",
        )
    {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle)
            && slot < object.fields.len()
        {
            object.fields[slot] = domain;
        }
    }
    Ok(Some(Value::Object(handle)))
}

/// Allocates a Java object and invokes its `<init>` constructor.
fn new_java_object<'a>(
    loader: &ClassLoader<'a>,
    internal: &[u8],
    ctor_descriptor: &[u8],
    args: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let klass = loader.load(internal)?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut io::sink(), 0)?;
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    let mut call_args = JavaCallArguments::new();
    call_args.push_oop(Value::Object(handle));
    for arg in args {
        call_args.push_oop(*arg);
    }
    JavaCalls::call_special(
        loader,
        &klass,
        b"<init>",
        ctor_descriptor,
        &call_args,
        &mut io::sink(),
    )?;
    Ok(Value::Object(handle))
}

pub(crate) fn trace_pd_url<'a>(loader: &ClassLoader<'a>, pd: usize, name: &[u8]) {
    let field_str = |object: usize, fname: &[u8], fdesc: &[u8]| -> Option<String> {
        let objects = loader.borrow_objects();
        let obj = objects.get(object)?;
        let slot = obj.klass.instanceField(fname, fdesc).ok()?;
        match obj.fields.get(slot).copied()? {
            Value::HostString(s) => Some(s.to_string()),
            Value::String(bytes) => decodeString(bytes).ok(),
            _ => None,
        }
    };
    let field_obj = |object: usize, fname: &[u8], fdesc: &[u8]| -> Option<usize> {
        let objects = loader.borrow_objects();
        let obj = objects.get(object)?;
        let slot = obj.klass.instanceField(fname, fdesc).ok()?;
        match obj.fields.get(slot).copied()? {
            Value::Object(h) => Some(h),
            _ => None,
        }
    };
    let Some(cs) = field_obj(pd, b"codesource", b"Ljava/security/CodeSource;") else {
        eprintln!("[pd] {} codesource=<none>", String::from_utf8_lossy(name));
        return;
    };
    let Some(url) = field_obj(cs, b"location", b"Ljava/net/URL;") else {
        eprintln!("[pd] {} location=<none>", String::from_utf8_lossy(name));
        return;
    };
    let proto = field_str(url, b"protocol", b"Ljava/lang/String;").unwrap_or_default();
    let file = field_str(url, b"file", b"Ljava/lang/String;").unwrap_or_default();
    let host = field_str(url, b"host", b"Ljava/lang/String;").unwrap_or_default();
    let auth = field_str(url, b"authority", b"Ljava/lang/String;").unwrap_or_default();
    eprintln!(
        "[pd] {} -> {}://{}{} file=[{}] auth=[{}]",
        String::from_utf8_lossy(name),
        proto,
        host,
        file,
        file,
        auth
    );
}

/// Builds `ProtectionDomain(CodeSource(URL(location), null), Permissions())`
/// for a classpath origin, mirroring what `URLClassLoader`/`JarLoader` store
/// in `java.lang.Class.protectionDomain` on a real JVM.
fn protection_domain_for_origin<'a>(
    loader: &ClassLoader<'a>,
    origin: &ClassOrigin,
) -> io::Result<Value<'a>> {
    let path = match origin {
        ClassOrigin::Jar(path) => path,
        ClassOrigin::Directory(path) => path,
    };
    let mut spec = String::from("file:///");
    for &byte in path.to_string_lossy().replace('\\', "/").as_bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '/' | ':' | '@') {
            spec.push(ch);
        } else {
            spec.push_str(&format!("%{byte:02X}"));
        }
    }
    if matches!(origin, ClassOrigin::Directory(_)) && !spec.ends_with('/') {
        spec.push('/');
    }
    let spec = loader.storage.alloc_str(&spec);
    let url = new_java_object(
        loader,
        b"java/net/URL",
        b"(Ljava/lang/String;)V",
        &[Value::HostString(spec)],
    )?;
    let code_source = new_java_object(
        loader,
        b"java/security/CodeSource",
        b"(Ljava/net/URL;[Ljava/security/cert/Certificate;)V",
        &[url, Value::Null],
    )?;
    let permissions = new_java_object(loader, b"java/security/Permissions", b"()V", &[])?;
    new_java_object(
        loader,
        b"java/security/ProtectionDomain",
        b"(Ljava/security/CodeSource;Ljava/security/PermissionCollection;)V",
        &[code_source, permissions],
    )
}
