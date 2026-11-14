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
    interpreter::bytecodeInterpreter::{self, Value},
    oops::instanceKlass::InstanceKlass,
};
use std::io::{self, Write};
use std::rc::Rc;

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
        klass: &Rc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
        args: &JavaCallArguments<'a>,
        output: &mut impl Write,
    ) -> io::Result<Option<Value<'a>>> {
        let code = klass.staticCode(name, descriptor, false)?;
        bytecodeInterpreter::initialize(loader, klass, output, 0)?;
        bytecodeInterpreter::execute(loader, klass, code, args.parameters(), output, 0)
    }

    /// Dispatches a special method call (invokespecial: constructors, private methods, super calls)
    pub fn call_special<'a>(
        loader: &ClassLoader<'a>,
        klass: &Rc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
        args: &JavaCallArguments<'a>,
        output: &mut impl Write,
    ) -> io::Result<Option<Value<'a>>> {
        let code = klass.methodCode(name, descriptor, false, false)?;
        bytecodeInterpreter::initialize(loader, klass, output, 0)?;
        bytecodeInterpreter::execute(loader, klass, code, args.parameters(), output, 0)
    }

    /// Dispatches a virtual method call with vtable dynamic dispatch (invokevirtual)
    pub fn call_virtual<'a>(
        loader: &ClassLoader<'a>,
        receiver_klass: &Rc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
        args: &JavaCallArguments<'a>,
        output: &mut impl Write,
    ) -> io::Result<Option<Value<'a>>> {
        let method_name = String::from_utf8_lossy(name);
        let method_sig = String::from_utf8_lossy(descriptor);

        // Resolve virtual method using receiver class vtable
        if let Some(_v_idx) = receiver_klass
            .vtable()
            .index_of_method(&method_name, &method_sig)
        {
            let code = receiver_klass.methodCode(name, descriptor, false, false)?;
            bytecodeInterpreter::initialize(loader, receiver_klass, output, 0)?;
            bytecodeInterpreter::execute(loader, receiver_klass, code, args.parameters(), output, 0)
        } else {
            let code = receiver_klass.methodCode(name, descriptor, false, false)?;
            bytecodeInterpreter::initialize(loader, receiver_klass, output, 0)?;
            bytecodeInterpreter::execute(loader, receiver_klass, code, args.parameters(), output, 0)
        }
    }
}

pub fn mainClass<'a>(
    loader: &ClassLoader<'a>,
    name: &str,
    arguments: &'a [String],
    output: &mut impl Write,
) -> io::Result<()> {
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
    let klass = loader.load(&encoded)?;
    let mut args = JavaCallArguments::new();
    args.push_oop(Value::Arguments(arguments));

    let res = JavaCalls::call_static(
        loader,
        &klass,
        b"main",
        b"([Ljava/lang/String;)V",
        &args,
        output,
    )?;

    if res.is_some() {
        return Err(io::Error::other("main must return void"));
    }
    Ok(())
}
