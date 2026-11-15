

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    oops::instanceKlass::InstanceKlass,
    runtime::javaCalls::{JavaCallArguments, JavaCalls, JavaValue},
};
use std::{
    io::{self, Write},
    sync::Arc,
};

pub const MAX_PARAMETERS: usize = 16;

pub struct JfrJavaArguments<'a> {
    klass: Option<Arc<InstanceKlass<'a>>>,
    klass_name: Vec<u8>,
    name: Vec<u8>,
    signature: Vec<u8>,
    receiver: Option<Value<'a>>,
    params: Vec<Value<'a>>,
    result: JavaValue<'a>,
}

impl<'a> JfrJavaArguments<'a> {

    pub fn new(result: JavaValue<'a>) -> Self {
        Self {
            klass: None,
            klass_name: Vec::new(),
            name: Vec::new(),
            signature: Vec::new(),
            receiver: None,
            params: Vec::with_capacity(MAX_PARAMETERS),
            result,
        }
    }

    pub fn with_coordinates(
        result: JavaValue<'a>,
        klass_name: &[u8],
        name: &[u8],
        signature: &[u8],
    ) -> Self {
        let mut args = Self::new(result);
        args.klass_name = klass_name.to_vec();
        args.name = name.to_vec();
        args.signature = signature.to_vec();
        args
    }

    pub fn with_klass(
        result: JavaValue<'a>,
        klass: Arc<InstanceKlass<'a>>,
        name: &[u8],
        signature: &[u8],
    ) -> Self {
        let mut args = Self::new(result);
        args.klass = Some(klass);
        args.name = name.to_vec();
        args.signature = signature.to_vec();
        args
    }

    pub fn set_klass_name(&mut self, klass_name: &[u8]) {
        self.klass = None;
        self.klass_name = klass_name.to_vec();
    }

    pub fn set_klass(&mut self, klass: Arc<InstanceKlass<'a>>) {
        self.klass = Some(klass);
    }

    pub fn set_name(&mut self, name: &[u8]) {
        self.name = name.to_vec();
    }

    pub fn set_signature(&mut self, signature: &[u8]) {
        self.signature = signature.to_vec();
    }

    pub fn set_receiver(&mut self, receiver: Value<'a>) {
        self.receiver = Some(receiver);
    }

    pub fn has_receiver(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn receiver(&self) -> Option<Value<'a>> {
        self.receiver
    }

    pub fn result(&self) -> &JavaValue<'a> {
        &self.result
    }

    pub fn result_mut(&mut self) -> &mut JavaValue<'a> {
        &mut self.result
    }

    pub fn name(&self) -> &[u8] {
        &self.name
    }

    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    pub fn length(&self) -> usize {
        self.params.len() + usize::from(self.receiver.is_some())
    }

    pub fn push_oop(&mut self, obj: Value<'a>) {
        self.params.push(obj);
    }

    pub fn push_jobject(&mut self, handle: Value<'a>) {
        self.params.push(handle);
    }

    pub fn push_int(&mut self, value: i32) {
        self.params.push(Value::Int(value));
    }

    pub fn push_float(&mut self, value: f32) {
        self.params.push(Value::Float(value));
    }

    pub fn push_double(&mut self, value: f64) {
        self.params.push(Value::Double(value));
    }

    pub fn push_long(&mut self, value: i64) {
        self.params.push(Value::Long(value));
    }

    pub fn param(&self, index: usize) -> Option<&Value<'a>> {
        if index == 0 && self.receiver.is_some() {
            self.receiver.as_ref()
        } else {
            let adjusted = if self.receiver.is_some() {
                index - 1
            } else {
                index
            };
            self.params.get(adjusted)
        }
    }

    pub fn copy(&self) -> JavaCallArguments<'a> {
        let mut args = JavaCallArguments::new();
        if let Some(receiver) = self.receiver {
            args.push_oop(receiver);
        }
        for value in &self.params {
            match value {
                Value::Int(v) => args.push_int(*v),
                Value::Long(v) => args.push_long(*v),
                Value::Float(v) => args.push_float(*v),
                Value::Double(v) => args.push_double(*v),
                other => args.push_oop(*other),
            }
        }
        args
    }

    fn resolve_klass(&self, loader: &ClassLoader<'a>) -> io::Result<Arc<InstanceKlass<'a>>> {
        if let Some(klass) = &self.klass {
            return Ok(Arc::clone(klass));
        }
        if self.klass_name.is_empty() {
            return Err(io::Error::other("JfrJavaArguments: no klass specified"));
        }
        loader.load(&self.klass_name)
    }

    pub fn call_static(
        &mut self,
        loader: &ClassLoader<'a>,
        output: &mut dyn Write,
    ) -> io::Result<()> {
        let klass = self.resolve_klass(loader)?;
        let call_args = self.copy();
        let value = JavaCalls::call_static(
            loader,
            &klass,
            &self.name,
            &self.signature,
            &call_args,
            output,
        )?;
        self.result = JavaValue::Value(value.unwrap_or(Value::Null));
        Ok(())
    }

    pub fn call_special(
        &mut self,
        loader: &ClassLoader<'a>,
        output: &mut dyn Write,
    ) -> io::Result<()> {
        let klass = self.resolve_klass(loader)?;
        let call_args = self.copy();
        let value = JavaCalls::call_special(
            loader,
            &klass,
            &self.name,
            &self.signature,
            &call_args,
            output,
        )?;
        self.result = JavaValue::Value(value.unwrap_or(Value::Null));
        Ok(())
    }

    pub fn call_virtual(
        &mut self,
        loader: &ClassLoader<'a>,
        output: &mut dyn Write,
    ) -> io::Result<()> {
        let klass = self.resolve_klass(loader)?;
        let call_args = self.copy();
        let value = JavaCalls::call_virtual(
            loader,
            &klass,
            &self.name,
            &self.signature,
            &call_args,
            output,
        )?;
        self.result = JavaValue::Value(value.unwrap_or(Value::Null));
        Ok(())
    }
}

pub struct JfrJavaCall;

impl JfrJavaCall {
    pub fn call_static<'a>(
        args: &mut JfrJavaArguments<'a>,
        loader: &ClassLoader<'a>,
        output: &mut dyn Write,
    ) -> io::Result<()> {
        args.call_static(loader, output)
    }

    pub fn call_special<'a>(
        args: &mut JfrJavaArguments<'a>,
        loader: &ClassLoader<'a>,
        output: &mut dyn Write,
    ) -> io::Result<()> {
        args.call_special(loader, output)
    }

    pub fn call_virtual<'a>(
        args: &mut JfrJavaArguments<'a>,
        loader: &ClassLoader<'a>,
        output: &mut dyn Write,
    ) -> io::Result<()> {
        args.call_virtual(loader, output)
    }
}
