

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    jfr::jni::java_call::JfrJavaArguments,
    oops::instanceKlass::InstanceKlass,
    runtime::javaCalls::JavaValue,
};
use std::{io, sync::Arc};

const JVM_UPCALLS_CLASS: &[u8] = b"jdk/jfr/internal/JVMUpcalls";

fn upcalls_klass<'a>(loader: &ClassLoader<'a>) -> io::Result<Arc<InstanceKlass<'a>>> {
    loader.load(JVM_UPCALLS_CLASS)
}

fn byte_array<'a>(loader: &ClassLoader<'a>, bytes: &[u8]) -> Value<'a> {
    let array = crate::share::oops::typeArrayOop::TypeArrayOop::Byte(
        bytes.iter().map(|&b| b as i8).collect(),
    );
    Value::PrimitiveArray(loader.alloc_type_array(array))
}

fn long_array<'a>(loader: &ClassLoader<'a>, values: &[i64]) -> Value<'a> {
    let array = crate::share::oops::typeArrayOop::TypeArrayOop::Long(values.to_vec());
    Value::PrimitiveArray(loader.alloc_type_array(array))
}

fn int_array<'a>(loader: &ClassLoader<'a>, values: &[i32]) -> Value<'a> {
    let array = crate::share::oops::typeArrayOop::TypeArrayOop::Int(values.to_vec());
    Value::PrimitiveArray(loader.alloc_type_array(array))
}

fn string_value<'a>(loader: &ClassLoader<'a>, text: &str) -> Value<'a> {
    Value::String(loader.storage.alloc_str(text).as_bytes())
}

fn string_array_bytes<'a>(loader: &ClassLoader<'a>, values: &[&[u8]]) -> io::Result<Value<'a>> {
    let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
        values.len() as i32,
        b"java/lang/String",
    )?;
    for (index, bytes) in values.iter().enumerate() {
        let text = String::from_utf8_lossy(bytes).into_owned();
        array.obj_at_put(index, string_value(loader, &text))?;
    }
    Ok(Value::ObjectArray(loader.alloc_obj_array(array)))
}

fn result_bytes<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> Option<Vec<u8>> {
    let Value::PrimitiveArray(handle) = value else {
        return None;
    };
    let arrays = loader.arrays.borrow();
    match arrays.get(handle)? {
        crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) => {
            Some(bytes.iter().map(|&b| b as u8).collect())
        }
        _ => None,
    }
}

fn invoke<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn io::Write,
    trace_id: i64,
    force_instrumentation: bool,
    boot_class_loader: bool,
    class_being_redefined: Value<'a>,
    class_data: &[u8],
    name: &[u8],
    signature: &[u8],
) -> io::Result<Option<Vec<u8>>> {
    let klass = upcalls_klass(loader)?;
    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        name,
        signature,
    );
    args.push_long(trace_id);
    args.push_int(i32::from(force_instrumentation));
    args.push_int(i32::from(boot_class_loader));
    args.push_oop(class_being_redefined);
    args.push_oop(byte_array(loader, class_data));
    args.call_static(loader, output)?;
    let JavaValue::Value(value) = args.result().clone() else {
        return Ok(None);
    };
    Ok(result_bytes(loader, value))
}

pub fn on_retransform<'a>(
    loader: &ClassLoader<'a>,
    trace_id: i64,
    class_being_redefined: Value<'a>,
    class_data: &[u8],
    output: &mut dyn io::Write,
) -> io::Result<Option<Vec<u8>>> {
    invoke(
        loader,
        output,
        trace_id,
        false,
        false,
        class_being_redefined,
        class_data,
        b"onRetransform",
        b"(JZZLjava/lang/Class;[B)[B",
    )
}

pub fn new_bytes_eager_instrumentation<'a>(
    loader: &ClassLoader<'a>,
    trace_id: i64,
    force_instrumentation: bool,
    boot_class_loader: bool,
    super_class: Value<'a>,
    class_data: &[u8],
    output: &mut dyn io::Write,
) -> io::Result<Option<Vec<u8>>> {
    invoke(
        loader,
        output,
        trace_id,
        force_instrumentation,
        boot_class_loader,
        super_class,
        class_data,
        b"bytesForEagerInstrumentation",
        b"(JZZLjava/lang/Class;[B)[B",
    )
}

pub fn unhide_internal_types<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn io::Write,
) -> io::Result<bool> {
    let klass = upcalls_klass(loader)?;
    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        b"unhideInternalTypes",
        b"()V",
    );
    match args.call_static(loader, output) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

#[derive(Clone, Debug)]
pub struct TracedMethod {
    pub id: i64,
    pub name: Vec<u8>,
    pub signature: Vec<u8>,
    pub modification: i32,
}

#[allow(clippy::too_many_arguments)]
pub fn on_method_trace<'a>(
    loader: &ClassLoader<'a>,
    module: Value<'a>,
    class_loader: Value<'a>,
    class_name: &str,
    class_bytes: &[u8],
    methods: &[TracedMethod],
    output: &mut dyn io::Write,
) -> io::Result<Option<Vec<u8>>> {
    let klass = upcalls_klass(loader)?;
    let ids: Vec<i64> = methods.iter().map(|m| m.id).collect();
    let name_array = string_array_bytes(
        loader,
        &methods.iter().map(|m| m.name.as_slice()).collect::<Vec<_>>(),
    )?;
    let signature_array = string_array_bytes(
        loader,
        &methods
            .iter()
            .map(|m| m.signature.as_slice())
            .collect::<Vec<_>>(),
    )?;
    let modifications: Vec<i32> = methods.iter().map(|m| m.modification).collect();

    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        b"onMethodTrace",
        b"(Ljava/lang/Module;Ljava/lang/ClassLoader;Ljava/lang/String;[B[J[Ljava/lang/String;[Ljava/lang/String;[I)[B",
    );
    args.push_oop(module);
    args.push_oop(class_loader);
    args.push_oop(string_value(loader, class_name));
    args.push_oop(byte_array(loader, class_bytes));
    args.push_oop(long_array(loader, &ids));
    args.push_oop(name_array);
    args.push_oop(signature_array);
    args.push_oop(int_array(loader, &modifications));
    args.call_static(loader, output)?;
    let JavaValue::Value(value) = args.result().clone() else {
        return Ok(None);
    };
    Ok(result_bytes(loader, value))
}

pub fn publish_method_timers_for_klass<'a>(
    loader: &ClassLoader<'a>,
    klass_id: i64,
    output: &mut dyn io::Write,
) -> io::Result<()> {
    let klass = upcalls_klass(loader)?;
    let mut args = JfrJavaArguments::with_klass(
        JavaValue::Value(Value::Null),
        klass,
        b"publishMethodTimersForClass",
        b"(J)V",
    );
    args.push_long(klass_id);
    args.call_static(loader, output)
}
