/*
 * Generic invokedynamic bootstrap support.
 *
 * Services call sites whose bootstrap method is neither LambdaMetafactory
 * nor the inline StringConcatFactory/ObjectMethods fast paths, by performing
 * the real JVMS linkage:
 *
 *   1. resolve the bootstrap MethodHandle constant to a static method,
 *   2. build a `MethodHandles.Lookup` for the caller class,
 *   3. materialize the call-site name, `MethodType`, and static bootstrap
 *      arguments as real Java objects,
 *   4. invoke the bootstrap method, which returns a `CallSite`,
 *   5. extract `CallSite.getTarget()` and cache the resulting
 *      `MethodHandle` per call site,
 *   6. invoke the target with the call-site arguments.
 */

use crate::classfile::Constant;
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bootstrapInfo::{BootstrapArg, BootstrapInfo};
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use std::io::{self, Error, Write};
use std::sync::Arc;

/// Materializes a `java.lang.String` object from UTF-8 text.
fn java_string<'a>(
    loader: &ClassLoader<'a>,
    text: &str,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Value<'a>> {
    let value = Value::HostString(loader.storage.alloc_str(text));
    Ok(
        crate::share::prims::stringNatives::materialize(loader, value, output, depth)?
            .unwrap_or(value),
    )
}

/// Builds a `java.lang.invoke.MethodType` object from a method descriptor
/// via `MethodType.fromMethodDescriptorString(String, ClassLoader)`.
fn method_type_object<'a>(
    loader: &ClassLoader<'a>,
    descriptor: &str,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Value<'a>> {
    let mt_klass = loader.load(b"java/lang/invoke/MethodType")?;
    let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
        loader,
        &mt_klass,
        output,
        depth + 1,
    )?;
    let mut call = JavaCallArguments::new();
    call.push_oop(java_string(loader, descriptor, output, depth + 1)?);
    let class_loader = loader
        .systemClassLoaderObject()
        .map(Value::Object)
        .unwrap_or(Value::Null);
    call.push_oop(class_loader);
    let result = JavaCalls::call_static(
        loader,
        &mt_klass,
        b"fromMethodDescriptorString",
        b"(Ljava/lang/String;Ljava/lang/ClassLoader;)Ljava/lang/invoke/MethodType;",
        &call,
        output,
    )?;
    result.ok_or_else(|| Error::other("MethodType.fromMethodDescriptorString returned null"))
}

/// Normalizes a class-file `Class` constant name to the internal form used
/// by `ClassMirror`/`loader.load`: object descriptors lose their `L...;`
/// wrapper, primitives map to the `:name` form, arrays pass through.
fn normalize_class_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix('L').and_then(|r| r.strip_suffix(';')) {
        return rest.to_owned();
    }
    match name {
        "Z" => return ":boolean".into(),
        "B" => return ":byte".into(),
        "C" => return ":char".into(),
        "S" => return ":short".into(),
        "I" => return ":int".into(),
        "J" => return ":long".into(),
        "F" => return ":float".into(),
        "D" => return ":double".into(),
        "V" => return ":void".into(),
        _ => {}
    }
    name.to_owned()
}

/// Materializes a `java.lang.Class` object for an internal class name.
fn class_object<'a>(loader: &ClassLoader<'a>, internal_name: &str) -> io::Result<Value<'a>> {
    let mirror = Value::ClassMirror(loader.storage.alloc_str(internal_name).as_bytes());
    match crate::share::prims::classNatives::materialize_class(loader, mirror)? {
        Some(value) => Ok(value),
        None => Err(Error::other("Failed to materialize java.lang.Class")),
    }
}

fn lookup_object<'a>(
    loader: &ClassLoader<'a>,
    caller: &InstanceKlass<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Value<'a>> {
    let lookup_klass = loader.load(b"java/lang/invoke/MethodHandles$Lookup")?;
    let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
        loader,
        &lookup_klass,
        output,
        depth + 1,
    )?;
    let object = crate::share::oops::instanceOop::InstanceOop::new(Arc::clone(&lookup_klass))?;
    let handle = loader.alloc_object(object);
    let mut call = JavaCallArguments::with_receiver(Value::Object(handle));
    call.push_oop(class_object(
        loader,
        &String::from_utf8_lossy(caller.class.name),
    )?);
    JavaCalls::call_special(
        loader,
        &lookup_klass,
        b"<init>",
        b"(Ljava/lang/Class;)V",
        &call,
        output,
    )?;
    Ok(Value::Object(handle))
}

/// Resolves a bootstrap `MethodHandle` constant argument to a real
/// `java.lang.invoke.MethodHandle` object via `Lookup.findX`.
fn method_handle_arg<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    kind: u8,
    index: u16,
    lookup: Value<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Value<'a>> {
    let [owner, name, descriptor] = klass.memberRef(index, false)?;
    let owner_name = String::from_utf8_lossy(owner).into_owned();
    let member_name = String::from_utf8_lossy(name).into_owned();
    let member_desc = String::from_utf8_lossy(descriptor).into_owned();

    let class_value = class_object(loader, &owner_name)?;
    let mt_value = method_type_object(loader, &member_desc, output, depth + 1)?;
    let name_value = java_string(loader, &member_name, output, depth + 1)?;

    let find_name: &[u8] = match kind {
        // REF_invokeVirtual / REF_invokeInterface
        5 | 9 => b"findVirtual",
        // REF_invokeStatic
        6 => b"findStatic",
        // REF_invokeSpecial
        7 => b"findSpecial",
        // REF_newInvokeSpecial -> findConstructor
        8 => b"findConstructor",
        _ => {
            return Err(Error::other(format!(
                "Unsupported bootstrap MethodHandle reference kind {kind}"
            )));
        }
    };
    let find_desc: &[u8] = if kind == 8 {
        b"(Ljava/lang/Class;Ljava/lang/invoke/MethodType;)Ljava/lang/invoke/MethodHandle;"
    } else {
        b"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/invoke/MethodType;)Ljava/lang/invoke/MethodHandle;"
    };

    let lookup_klass = loader.load(b"java/lang/invoke/MethodHandles$Lookup")?;
    let mut call = JavaCallArguments::with_receiver(lookup);
    call.push_oop(class_value);
    if kind != 8 {
        call.push_oop(name_value);
    }
    call.push_oop(mt_value);
    let result =
        JavaCalls::call_virtual(loader, &lookup_klass, find_name, find_desc, &call, output)?;
    result.ok_or_else(|| Error::other("Lookup.find* returned null MethodHandle"))
}

/// Converts a static bootstrap argument into the Java object the bootstrap
/// method expects.
fn bootstrap_arg_value<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    arg: &BootstrapArg,
    lookup: Value<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Value<'a>> {
    match arg {
        BootstrapArg::Int(v) => Ok(Value::Int(*v)),
        BootstrapArg::Long(v) => Ok(Value::Long(*v)),
        BootstrapArg::Float(bits) => Ok(Value::Float(f32::from_bits(*bits))),
        BootstrapArg::Double(bits) => Ok(Value::Double(f64::from_bits(*bits))),
        BootstrapArg::String(s) => java_string(loader, s, output, depth + 1),
        BootstrapArg::Klass(name) => {
            // Class constants store field descriptors ("Ljava/lang/String;",
            // "[I", "I"); normalize to the internal mirror form.
            class_object(loader, &normalize_class_name(name))
        }
        BootstrapArg::MethodType(desc) => method_type_object(loader, desc, output, depth + 1),
        BootstrapArg::MethodHandle { kind, index } => {
            method_handle_arg(loader, klass, *kind, *index, lookup, output, depth + 1)
        }
    }
}

/// Materializes the static bootstrap arguments for a call to `bsm_descriptor`.
/// Per JVMS §5.4.3.6 a variable-arity bootstrap collects its trailing static
/// arguments into an array of the last parameter type; `Object...` bootstraps
/// (e.g. `SwitchBootstraps.typeSwitch`) therefore receive a single
/// `Object[]` instead of one slot per argument.
fn bootstrap_tail_args<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    bs_info: &BootstrapInfo,
    lookup: Value<'a>,
    bsm_descriptor: &[u8],
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Vec<Value<'a>>> {
    let mut values = Vec::with_capacity(bs_info.arguments().len());
    for arg in bs_info.arguments() {
        values.push(bootstrap_arg_value(
            loader,
            klass,
            arg,
            lookup,
            output,
            depth + 1,
        )?);
    }
    let params_end = bsm_descriptor
        .iter()
        .position(|byte| *byte == b')')
        .unwrap_or(bsm_descriptor.len());
    if bsm_descriptor.len() > params_end + 1
        && bsm_descriptor[1..params_end].ends_with(b"[Ljava/lang/Object;")
    {
        let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
            values.len() as i32,
            b"java/lang/Object",
        )?;
        array.elements.copy_from_slice(&values);
        return Ok(vec![Value::ObjectArray(loader.alloc_obj_array(array))]);
    }
    Ok(values)
}

/// Runs the real bootstrap method for an invokedynamic call site and
/// returns the linked target `MethodHandle` object.
fn link_call_site<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    bs_info: &BootstrapInfo,
    indy_name: &[u8],
    indy_descriptor: &[u8],
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Value<'a>> {
    // Resolve the bootstrap method handle to (owner, name, descriptor).
    let Constant::MethodHandle(_, ref_index) = klass.constant(bs_info.bsm_ref())? else {
        return Err(Error::other(
            "Bootstrap method is not a MethodHandle constant",
        ));
    };
    let [owner, name, descriptor] = klass.memberRef(*ref_index, false)?;
    let bsm_klass = loader.load(owner)?;

    let lookup = lookup_object(loader, klass, output, depth + 1)?;
    let name_value = java_string(
        loader,
        &String::from_utf8_lossy(indy_name),
        output,
        depth + 1,
    )?;
    let type_value = method_type_object(
        loader,
        &String::from_utf8_lossy(indy_descriptor),
        output,
        depth + 1,
    )?;

    let mut call = JavaCallArguments::new();
    call.push_oop(lookup);
    call.push_oop(name_value);
    call.push_oop(type_value);
    for value in bootstrap_tail_args(loader, klass, bs_info, lookup, descriptor, output, depth + 1)? {
        call.push_oop(value);
    }

    let result = JavaCalls::call_static(loader, &bsm_klass, name, descriptor, &call, output)?;
    let Value::Object(call_site) = result.unwrap_or(Value::Null) else {
        return Err(Error::other("Bootstrap method did not return a CallSite"));
    };

    // callSite.getTarget() -> MethodHandle
    let call_site_klass = {
        let objects = loader.borrow_objects();
        objects
            .get(call_site)
            .map(|o| Arc::clone(&o.klass))
            .ok_or_else(|| Error::other("Invalid CallSite object"))?
    };
    let get_call = JavaCallArguments::with_receiver(Value::Object(call_site));
    let target = JavaCalls::call_virtual(
        loader,
        &call_site_klass,
        b"getTarget",
        b"()Ljava/lang/invoke/MethodHandle;",
        &get_call,
        output,
    )?;
    match target {
        Some(Value::Object(handle)) => Ok(Value::Object(handle)),
        _ => Err(Error::other("CallSite.getTarget returned null")),
    }
}

/// Services an invokedynamic call site through its real bootstrap method.
/// Returns the value produced by the linked call-site target invoked with
/// the call-site arguments (`captured` is the full dynamic argument slice —
/// for invokedynamic the call-site type covers captures and call arguments
/// alike). `Ok(None)` means the call site has a void result.
pub fn invoke<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    bs_info: &BootstrapInfo,
    indy_name: &[u8],
    indy_descriptor: &[u8],
    call_args: &[Value<'a>],
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Option<Value<'a>>> {
    let caller = klass.class.name;
    let bsm_index = bs_info.bss_index() as u16;
    let target = if let Some(value) = loader.indy_target(caller, bsm_index) {
        value
    } else {
        let value = link_call_site(
            loader,
            klass,
            bs_info,
            indy_name,
            indy_descriptor,
            output,
            depth + 1,
        )?;
        loader.cache_indy_target(caller, bsm_index, value);
        value
    };
    let Value::Object(mh_handle) = target else {
        return Err(Error::other(
            "Cached invokedynamic target is not a MethodHandle",
        ));
    };
    // MethodHandle.invoke: the receiver slot is the MethodHandle object
    // itself, followed by the call-site arguments.
    let mut invoke_args = Vec::with_capacity(call_args.len() + 1);
    invoke_args.push(Value::Object(mh_handle));
    invoke_args.extend_from_slice(call_args);
    crate::share::prims::methodHandleNatives::method_handle_invoke(
        loader,
        mh_handle,
        b"invoke",
        indy_descriptor,
        &invoke_args,
    )
}

/// Resolves a `condy` (ConstantDynamic) constant through its real bootstrap
/// method. Unlike invokedynamic the bootstrap's return value IS the constant
/// — there is no CallSite/target indirection. Results are cached per
/// constant-pool index.
pub fn resolve_condy<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    cp_index: u32,
    bsm_attr_idx: u16,
    name: &[u8],
    field_descriptor: &[u8],
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Value<'a>> {
    let caller = klass.class.name;
    if let Some(value) = loader.condy_value(caller, cp_index) {
        return Ok(value);
    }

    // For condy the bootstrap MethodType has no parameters and the
    // constant's type as its return type: "()" + field descriptor.
    let mut mt_desc = String::from("()");
    mt_desc.push_str(&String::from_utf8_lossy(field_descriptor));

    let bs_info = crate::share::interpreter::linkResolver::LinkResolver::resolve_invokedynamic(
        loader,
        klass,
        bsm_attr_idx,
        name,
        field_descriptor,
    )?;

    let Constant::MethodHandle(_, ref_index) = klass.constant(bs_info.bsm_ref())? else {
        return Err(Error::other(
            "Bootstrap method is not a MethodHandle constant",
        ));
    };
    let [owner, bsm_name, bsm_desc] = klass.memberRef(*ref_index, false)?;
    let bsm_klass = loader.load(owner)?;

    let lookup = lookup_object(loader, klass, output, depth + 1)?;
    let name_value = java_string(loader, &String::from_utf8_lossy(name), output, depth + 1)?;
    let type_value = method_type_object(loader, &mt_desc, output, depth + 1)?;

    let mut call = JavaCallArguments::new();
    call.push_oop(lookup);
    call.push_oop(name_value);
    call.push_oop(type_value);
    for value in bootstrap_tail_args(loader, klass, &bs_info, lookup, bsm_desc, output, depth + 1)? {
        call.push_oop(value);
    }

    let value = JavaCalls::call_static(loader, &bsm_klass, bsm_name, bsm_desc, &call, output)?
        .ok_or_else(|| Error::other("condy bootstrap returned no value"))?;
    loader.cache_condy_value(caller, cp_index, value);
    Ok(value)
}
