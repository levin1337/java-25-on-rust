/*
 * Port of OpenJDK 25 HotSpot JavaCalls
 * (share/runtime/javaCalls.hpp / javaCalls.cpp)
 *
 * Implements VM invocation of Java methods:
 * static calls, virtual calls with vtable dispatch, special (invokespecial) calls,
 * JavaCallArguments packing, and JavaCalls execution helper.
 */

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::{
        bytecodeInterpreter::{self, Value},
        linkResolver::LinkResolver,
    },
    oops::{instanceKlass::InstanceKlass, instanceOop::InstanceOop},
};
use std::io::{self, Write};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum JavaValue<'a> {
    Void,
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Value(Value<'a>),
}

#[derive(Clone, Debug, Default)]
pub struct JavaCallArguments<'a> {
    parameters: Vec<Value<'a>>,
}

impl<'a> JavaCallArguments<'a> {
    pub fn new() -> Self {
        Self {
            parameters: Vec::new(),
        }
    }

    pub fn with_receiver(receiver: Value<'a>) -> Self {
        let mut args = Self::new();
        args.parameters.push(receiver);
        args
    }

    #[inline]
    pub fn push_int(&mut self, val: i32) {
        self.parameters.push(Value::Int(val));
    }

    #[inline]
    pub fn push_long(&mut self, val: i64) {
        self.parameters.push(Value::Long(val));
    }

    #[inline]
    pub fn push_float(&mut self, val: f32) {
        self.parameters.push(Value::Float(val));
    }

    #[inline]
    pub fn push_double(&mut self, val: f64) {
        self.parameters.push(Value::Double(val));
    }

    #[inline]
    pub fn push_oop(&mut self, val: Value<'a>) {
        self.parameters.push(val);
    }

    #[inline]
    pub fn parameters(&self) -> &[Value<'a>] {
        &self.parameters
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.parameters.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
    }
}

pub struct JavaCalls;

impl JavaCalls {
    /// Dispatches a static method call (invokestatic)
    pub fn call_static<'a>(
        loader: &ClassLoader<'a>,
        klass: &Arc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
        args: &JavaCallArguments<'a>,
        output: &mut dyn Write,
    ) -> io::Result<Option<Value<'a>>> {
        // Resolve through the hierarchy so `execute` runs with the
        // *declaring* class's constant pool — running a superclass method
        // body under the receiver's klass would decode cp indexes against
        // the wrong constant pool.
        let (target, code) = LinkResolver::resolve_static_call(klass, name, descriptor)?;
        bytecodeInterpreter::initialize(loader, klass, output, 0)?;
        bytecodeInterpreter::execute(loader, &target, code, args.parameters(), output, 0, 0)
    }

    /// Dispatches a special method call (invokespecial: constructors, private methods, super calls)
    pub fn call_special<'a>(
        loader: &ClassLoader<'a>,
        klass: &Arc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
        args: &JavaCallArguments<'a>,
        output: &mut dyn Write,
    ) -> io::Result<Option<Value<'a>>> {
        let (target, code) = LinkResolver::resolve_special_call(klass, name, descriptor)?;
        bytecodeInterpreter::initialize(loader, klass, output, 0)?;
        bytecodeInterpreter::execute(loader, &target, code, args.parameters(), output, 0, 0)
    }

    /// Dispatches a virtual method call with vtable dynamic dispatch (invokevirtual)
    pub fn call_virtual<'a>(
        loader: &ClassLoader<'a>,
        receiver_klass: &Arc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
        args: &JavaCallArguments<'a>,
        output: &mut dyn Write,
    ) -> io::Result<Option<Value<'a>>> {
        let (target, code) =
            LinkResolver::resolve_virtual_call(receiver_klass, name, descriptor)?;
        bytecodeInterpreter::initialize(loader, &target, output, 0)?;
        bytecodeInterpreter::execute(
            loader,
            &target,
            code,
            args.parameters(),
            output,
            0,
            0,
        )
    }
}

/// Bootstraps the minimal JDK runtime state that HotSpot normally establishes
/// in `System.initPhase1/2/3`: `VM.initLevel`, `VM.savedProps`, `System.props`
/// and `System.lineSeparator`. Skipped silently when no JDK image is present.
fn bootstrap<'a>(loader: &ClassLoader<'a>, output: &mut dyn Write) -> io::Result<()> {
    // Merge platform properties (java.home, os.name, ...) underneath any
    // user-supplied -D entries so System.getProperty/props see the full set.
    {
        let platform = crate::share::prims::systemProperties::platform_properties(loader);
        let mut props = loader.systemProperties.borrow_mut();
        for (key, value) in platform {
            props
                .entry(key)
                .or_insert_with(|| loader.storage.alloc_str(&value));
        }
    }
    let vm = match loader.load(b"jdk/internal/misc/VM") {
        Ok(klass) => klass,
        Err(_) => return Ok(()),
    };
    bytecodeInterpreter::initialize(loader, &vm, output, 0)?;
    vm.staticField(b"initLevel", b"I")?.set(Value::Int(4));

    let saved_props = vm.staticField(b"savedProps", b"Ljava/util/Map;")?;
    if matches!(saved_props.get(), Value::Null) {
        let map_klass = loader.load(b"java/util/HashMap")?;
        bytecodeInterpreter::initialize(loader, &map_klass, output, 0)?;
        let object = InstanceOop::new(map_klass.clone())?;
        let handle = loader.alloc_object(object);
        let mut ctor_args = JavaCallArguments::new();
        ctor_args.push_oop(Value::Object(handle));
        JavaCalls::call_special(
            loader,
            &map_klass,
            b"<init>",
            b"()V",
            &ctor_args,
            output,
        )?;
        let properties: Vec<(String, &'a str)> = loader
            .systemProperties
            .borrow()
            .iter()
            .map(|(key, value)| (key.clone(), *value))
            .collect();
        for (key, value) in properties {
            let key = loader.storage.alloc_str(&key);
            let mut args = JavaCallArguments::new();
            args.push_oop(Value::Object(handle));
            args.push_oop(Value::HostString(key));
            args.push_oop(Value::HostString(value));
            JavaCalls::call_virtual(
                loader,
                &map_klass,
                b"put",
                b"(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;",
                &args,
                output,
            )?;
        }
        saved_props.set(Value::Object(handle));
    }

    let system = loader.load(b"java/lang/System")?;
    bytecodeInterpreter::initialize(loader, &system, output, 0)?;
    if let Ok(separator) = system.staticField(b"lineSeparator", b"Ljava/lang/String;") {
        if matches!(separator.get(), Value::Null) {
            separator.set(Value::HostString("\n"));
        }
    }
    let props_cell = system.staticField(b"props", b"Ljava/util/Properties;")?;
    if matches!(props_cell.get(), Value::Null) {
        let props_klass = loader.load(b"java/util/Properties")?;
        bytecodeInterpreter::initialize(loader, &props_klass, output, 0)?;
        let object = InstanceOop::new(props_klass.clone())?;
        let handle = loader.alloc_object(object);
        let mut ctor_args = JavaCallArguments::new();
        ctor_args.push_oop(Value::Object(handle));
        JavaCalls::call_special(
            loader,
            &props_klass,
            b"<init>",
            b"()V",
            &ctor_args,
            output,
        )?;
        let properties: Vec<(String, &'a str)> = loader
            .systemProperties
            .borrow()
            .iter()
            .map(|(key, value)| (key.clone(), *value))
            .collect();
        for (key, value) in properties {
            let key = loader.storage.alloc_str(&key);
            let mut args = JavaCallArguments::new();
            args.push_oop(Value::Object(handle));
            args.push_oop(Value::HostString(key));
            args.push_oop(Value::HostString(value));
            JavaCalls::call_virtual(
                loader,
                &props_klass,
                b"setProperty",
                b"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/Object;",
                &args,
                output,
            )?;
        }
        props_cell.set(Value::Object(handle));
    }
    Ok(())
}

pub fn mainClass<'a>(
    loader: &ClassLoader<'a>,
    name: &str,
    arguments: &'a [String],
    output: &mut dyn Write,
) -> io::Result<()> {
    crate::share::runtime::threading::mark_primordial();
    let t0 = std::time::Instant::now();
    if let Ok(system) = loader.load(b"java/lang/System") {
        // HotSpot eagerly initializes java.lang.ref.Reference during VM
        // startup; its <clinit> registers JavaLangRefAccess in SharedSecrets,
        // which System.initPhase1 needs for startThreads().
        if let Ok(reference) = loader.load(b"java/lang/ref/Reference") {
            let _ = bytecodeInterpreter::initialize(loader, &reference, output, 0);
        }
        let args = JavaCallArguments::new();
        eprintln!("[phase] 1");
        let _ = JavaCalls::call_static(loader, &system, b"initPhase1", b"()V", &args, output);
        eprintln!("[phase] 2 {:?}", t0.elapsed());
        let mut args = JavaCallArguments::new();
        args.push_int(0);
        args.push_int(0);
        let _ = JavaCalls::call_static(loader, &system, b"initPhase2", b"(ZZ)I", &args, output);
        eprintln!("[phase] 3 {:?}", t0.elapsed());
        let args = JavaCallArguments::new();
        let _ = JavaCalls::call_static(loader, &system, b"initPhase3", b"()V", &args, output);
        eprintln!(
            "[phase] done {:?} bytecodes={} classes={}",
            t0.elapsed(),
            crate::share::interpreter::bytecodeInterpreter::bytecode_count(),
            loader.classes.borrow().len()
        );
        eprintln!(
            "[phase]   frames={} natives={}",
            crate::share::interpreter::bytecodeInterpreter::frame_count(),
            crate::share::prims::nativeLookup::NATIVE_COUNT.load(
                std::sync::atomic::Ordering::Relaxed
            )
        );
        eprintln!(
            "[phase]   native_nanos={}",
            crate::share::interpreter::bytecodeInterpreter::NATIVE_NANOS.load(
                std::sync::atomic::Ordering::Relaxed
            )
        );
    }
    let mut encoded = Vec::with_capacity(name.len());
    for unit in name.encode_utf16() {
        match unit {
            1..=0x7f => encoded.push(unit as u8),
            0..=0x7ff => {
                encoded.extend([0xc0 | (unit >> 6) as u8, 0x80 | (unit & 0x3f) as u8]);
            }
            _ => encoded.extend([
                0xe0 | (unit >> 12) as u8,
                0x80 | ((unit >> 6) & 0x3f) as u8,
                0x80 | (unit & 0x3f) as u8,
            ]),
        }
    }
    eprintln!("[phase] load main {:?}", t0.elapsed());
    let klass = loader.load(&encoded)?;
    eprintln!("[phase] main loaded {:?}", t0.elapsed());
    let mut args = JavaCallArguments::new();
    args.push_oop(Value::Arguments(arguments));

    eprintln!("[dbg] bootstrap... {:?}", t0.elapsed());
    bootstrap(loader, output)?;
    eprintln!("[dbg] bootstrap done {:?}", t0.elapsed());
    let res = match JavaCalls::call_static(
        loader,
        &klass,
        b"main",
        b"([Ljava/lang/String;)V",
        &args,
        output,
    ) {
        Ok(res) => res,
        Err(error) => {
            // Surface the throwable's class/message/cause chain on stderr —
            // System.err is not wired to the process stderr, so printStackTrace
            // output would be lost.
            if let Some(handle) = bytecodeInterpreter::throwable_of(&error) {
                dump_throwable_chain(loader, handle);
            }
            return Err(error);
        }
    };

    if res.is_some() {
        return Err(io::Error::other("main must return void"));
    }
    // JVM exit semantics: stay alive until all non-daemon Java threads
    // finish, like a real VM does after main() returns.
    crate::share::runtime::threading::wait_non_daemon(loader);
    crate::share::prims::nativeLookup::dump_native_profile();
    Ok(())
}

/// Prints `className: detailMessage` for a throwable and its `cause` chain
/// to the process stderr. Used when `main` exits with a Java exception.
fn dump_throwable_chain<'a>(loader: &ClassLoader<'a>, mut handle: usize) {
    for _ in 0..16 {
        let (class_name, message, cause) = {
            let objects = loader.borrow_objects();
            let Some(object) = objects.get(handle) else {
                return;
            };
            let class_name = String::from_utf8_lossy(&object.klass.class.name).into_owned();
            let message_slot = object
                .klass
                .instanceField(b"detailMessage", b"Ljava/lang/String;")
                .ok();
            let cause_slot = object
                .klass
                .instanceField(b"cause", b"Ljava/lang/Throwable;")
                .ok();
            let message = message_slot
                .and_then(|slot| object.fields.get(slot).copied())
                .and_then(|value| {
                    crate::share::prims::stringNatives::string(loader, value)
                        .ok()
                        .flatten()
                        .map(|cow| cow.into_owned())
                });
            let cause = cause_slot
                .and_then(|slot| object.fields.get(slot).copied())
                .and_then(|value| match value {
                    Value::Object(h) => Some(h),
                    _ => None,
                });
            (class_name, message, cause)
        };
        match &message {
            Some(message) => eprintln!("[throw] {class_name}: {message}"),
            None => eprintln!("[throw] {class_name}"),
        }
        // Print the captured backtrace (`StackTraceElement[]` stored by
        // `Throwable.fillInStackTrace`) so fatal errors show where they
        // were raised even though System.err routing may be limited.
        {
            let objects = loader.borrow_objects();
            if let Some(object) = objects.get(handle)
                && let Ok(slot) = object
                    .klass
                    .instanceField(b"backtrace", b"Ljava/lang/Object;")
                && let Some(Value::ObjectArray(array_handle)) = object.fields.get(slot).copied()
            {
                let elements: Vec<usize> = {
                    let obj_arrays = loader.obj_arrays.borrow();
                    match obj_arrays.get(array_handle) {
                        Some(array) => array
                            .elements
                            .iter()
                            .filter_map(|v| match v {
                                Value::Object(h) => Some(*h),
                                _ => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    }
                };
                for element in elements.into_iter().take(24) {
                    let line = {
                        let objects = loader.borrow_objects();
                        let Some(obj) = objects.get(element) else { continue };
                        let get_str = |name: &[u8], desc: &[u8]| -> Option<String> {
                            let slot = obj.klass.instanceField(name, desc).ok()?;
                            match obj.fields.get(slot).copied()? {
                                Value::HostString(s) => Some(s.to_string()),
                                Value::String(b) => {
                                    crate::share::prims::stringNatives::string(loader, Value::String(b))
                                        .ok()
                                        .flatten()
                                        .map(|c| c.into_owned())
                                }
                                _ => None,
                            }
                        };
                        let get_int = |name: &[u8]| -> i32 {
                            obj.klass
                                .instanceField(name, b"I")
                                .ok()
                                .and_then(|s| match obj.fields.get(s).copied()? {
                                    Value::Int(v) => Some(v),
                                    _ => None,
                                })
                                .unwrap_or(-1)
                        };
                        format!(
                            "\tat {}.{}({}:{})",
                            get_str(b"declaringClass", b"Ljava/lang/String;").unwrap_or_default(),
                            get_str(b"methodName", b"Ljava/lang/String;").unwrap_or_default(),
                            get_str(b"fileName", b"Ljava/lang/String;").unwrap_or_default(),
                            get_int(b"lineNumber")
                        )
                    };
                    eprintln!("{line}");
                }
            }
        }
        match cause {
            // Throwable.cause == this is the JDK "unset" sentinel.
            Some(next) if next != handle => handle = next,
            _ => return,
        }
    }
}
