use crate::share::{
    classfile::{classFileParser::decodeString, classLoader::ClassLoader},
    interpreter::bytecodeInterpreter::Value,
    oops::instanceOop::InstanceOop,
};
use std::io;

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
    let class = match *receiver {
        Value::ClassMirror(class) => class,
        Value::Object(handle) => {
            let is_class_object = {
                let objects = loader.borrow_objects();
                objects.get(handle).is_some_and(|object| {
                    object.klass.class.name == b"java/lang/Class"
                })
            };
            if !is_class_object {
                return Ok(None);
            }
            match loader.class_object_names.borrow().get(&handle) {
                Some(name) => *name,
                None => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    if name == b"getResource" && descriptor == b"(Ljava/lang/String;)Ljava/net/URL;" {
        let resource = arguments.first().copied().ok_or_else(|| io::Error::other("Missing resource name"))?;
        return crate::share::prims::urlNatives::classResource(loader, class, resource).map(Some);
    }
    if name == b"isAssignableFrom" && descriptor == b"(Ljava/lang/Class;)Z" {
        let target = class;
        let source = match arguments.first().copied().unwrap_or(Value::Null) {
            Value::ClassMirror(source) => source,
            Value::Null => return Err(io::Error::other("NullPointerException")),
            _ => return Ok(Some(Value::Int(0))),
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
        (b"isPrimitive", b"()Z") => Value::Int(i32::from(class.starts_with(b":"))),
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
                    .borrow()
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
            let ok = match arg {
                Value::ClassMirror(other) => {
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
                    .borrow()
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
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle) {
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
    Ok(Some(Value::Object(handle)))
}
