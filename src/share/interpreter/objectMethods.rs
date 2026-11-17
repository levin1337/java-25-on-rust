/*
 * ObjectMethods.bootstrap support for records.
 *
 * JDK records generate toString/hashCode/equals as invokedynamic call sites
 * bootstrapped by java.lang.runtime.ObjectMethods. Instead of the full
 * MethodHandle machinery, we evaluate the three Object methods directly:
 * the bootstrap arguments carry the record Class, the ";"-separated field
 * names and getField MethodHandles (the names alone suffice for us).
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::classFileParser::Constant;
use crate::share::interpreter::bootstrapInfo::{BootstrapArg, BootstrapInfo};
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceKlass::InstanceKlass;
use std::io::{self, Error, Write};
use std::sync::Arc;

fn is_object_methods_bootstrap(
    klass: &InstanceKlass<'_>,
    bs_info: &BootstrapInfo,
) -> io::Result<bool> {
    let Constant::MethodHandle(_, ref_index) = klass.constant(bs_info.bsm_ref())? else {
        return Ok(false);
    };
    let [owner, name, _desc] = klass.memberRef(*ref_index, false)?;
    Ok(owner == b"java/lang/runtime/ObjectMethods" && name == b"bootstrap")
}

/// Field (name -> slot, descriptor) lookup on the record class.
fn record_field<'a>(
    record: &InstanceKlass<'a>,
    name: &[u8],
) -> io::Result<(usize, Vec<u8>)> {
    let descriptor = record
        .class
        .fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.descriptor.to_vec())
        .ok_or_else(|| Error::other("Record component field not found"))?;
    let slot = record.instanceField(name, &descriptor)?;
    Ok((slot, descriptor))
}

fn field_value<'a>(
    _loader: &ClassLoader<'a>,
    object: &crate::share::oops::instanceOop::InstanceOop<'a>,
    record: &InstanceKlass<'a>,
    name: &[u8],
) -> io::Result<Value<'a>> {
    let (slot, _) = record_field(record, name)?;
    Ok(object
        .fields
        .get(slot)
        .copied()
        .unwrap_or(Value::Null))
}

fn string_text<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
) -> Option<std::borrow::Cow<'a, str>> {
    crate::share::prims::stringNatives::string(loader, value)
        .ok()
        .flatten()
}

fn string_hash(text: &str) -> i32 {
    text.encode_utf16()
        .fold(0i32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(i32::from(unit))
        })
}

fn field_hash<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> i32 {
    match value {
        Value::Int(v) => v,
        Value::Long(v) => ((((v as u64) ^ ((v as u64) >> 32)) as u32) as i32),
        Value::Float(v) => v.to_bits() as i32,
        Value::Double(v) => {
            let bits = v.to_bits();
            ((bits ^ (bits >> 32)) as u32) as i32
        }
        Value::Null => 0,
        _ => match string_text(loader, value) {
            Some(text) => string_hash(&text),
            None => crate::share::prims::objectNatives::identity(value),
        },
    }
}

fn values_equal<'a>(loader: &ClassLoader<'a>, a: Value<'a>, b: Value<'a>) -> bool {
    use crate::share::interpreter::bytecodeInterpreter::Value as V;
    match (a, b) {
        (V::Null, V::Null) => true,
        (V::Int(x), V::Int(y)) => x == y,
        (V::Long(x), V::Long(y)) => x == y,
        (V::Float(x), V::Float(y)) => x.to_bits() == y.to_bits(),
        (V::Double(x), V::Double(y)) => x.to_bits() == y.to_bits(),
        _ => {
            if let (Some(sa), Some(sb)) = (string_text(loader, a), string_text(loader, b)) {
                return sa == sb;
            }
            match (a, b) {
                (V::Object(x), V::Object(y))
                | (V::ObjectArray(x), V::ObjectArray(y))
                | (V::PrimitiveArray(x), V::PrimitiveArray(y)) => x == y,
                (V::String(x), V::String(y)) => x == y,
                _ => false,
            }
        }
    }
}

/// Attempts to service an invokedynamic call site through
/// ObjectMethods.bootstrap (record toString/hashCode/equals).
/// Returns `Ok(None)` for other bootstraps.
pub fn invoke<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    bs_info: &BootstrapInfo,
    captured: &[Value<'a>],
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Option<Value<'a>>> {
    if !is_object_methods_bootstrap(klass, bs_info)? {
        return Ok(None);
    }
    let args = bs_info.arguments();
    let Some(BootstrapArg::Klass(record_name)) = args.first() else {
        return Err(Error::other("ObjectMethods: missing record class"));
    };
    let field_names: Vec<String> = match args.get(1) {
        Some(BootstrapArg::String(names)) => names
            .split(';')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };

    let record = loader.load(record_name.as_bytes())?;
    let receiver = captured.first().copied().unwrap_or(Value::Null);
    let method = bs_info.name().as_bytes();

    let object = match receiver {
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            objects
                .get(handle)
                .map(|o| crate::share::oops::instanceOop::InstanceOop {
                    klass: Arc::clone(&o.klass),
                    fields: o.fields.clone(),
                })
        }
        _ => None,
    };

    match method {
        b"toString" => {
            let Some(object) = object else {
                return Ok(Some(Value::HostString("null".into())));
            };
            // Simple name: last '/' segment, then last '$' segment.
            let dotted = record_name.replace('/', ".");
            let simple = dotted
                .rsplit('.')
                .next()
                .unwrap_or(&dotted)
                .rsplit('$')
                .next()
                .unwrap_or(dotted.as_str());
            let mut text = format!("{simple}[");
            for (i, name) in field_names.iter().enumerate() {
                if i > 0 {
                    text.push_str(", ");
                }
                let value = field_value(loader, &object, &record, name.as_bytes())?;
                text.push_str(&format!("{name}="));
                text.push_str(&crate::share::interpreter::bytecodeInterpreter::describeValue(
                    loader,
                    value,
                    output,
                    depth + 1,
                )?);
            }
            text.push(']');
            Ok(Some(Value::HostString(loader.storage.alloc_str(&text))))
        }
        b"hashCode" => {
            let Some(object) = object else {
                return Ok(Some(Value::Int(0)));
            };
            let mut hash = 1i32;
            for name in &field_names {
                let value = field_value(loader, &object, &record, name.as_bytes())?;
                hash = hash.wrapping_mul(31).wrapping_add(field_hash(loader, value));
            }
            Ok(Some(Value::Int(hash)))
        }
        b"equals" => {
            let other = captured.get(1).copied().unwrap_or(Value::Null);
            let result = match (object, other) {
                (Some(object), Value::Object(handle)) => {
                    let (other_fields, same_klass) = {
                        let objects = loader.borrow_objects();
                        match objects.get(handle) {
                            Some(other_obj) => (
                                other_obj.fields.clone(),
                                Arc::ptr_eq(&other_obj.klass, &record),
                            ),
                            None => return Ok(Some(Value::Int(0))),
                        }
                    };
                    if !same_klass {
                        return Ok(Some(Value::Int(0)));
                    }
                    let other_object = crate::share::oops::instanceOop::InstanceOop {
                        klass: Arc::clone(&record),
                        fields: other_fields,
                    };
                    let mut equal = true;
                    for name in &field_names {
                        let a = field_value(loader, &object, &record, name.as_bytes())?;
                        let b = field_value(loader, &other_object, &record, name.as_bytes())?;
                        if !values_equal(loader, a, b) {
                            equal = false;
                            break;
                        }
                    }
                    equal
                }
                _ => false,
            };
            Ok(Some(Value::Int(i32::from(result))))
        }
        _ => Ok(None),
    }
}
