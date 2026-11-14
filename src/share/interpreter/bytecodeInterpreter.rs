/*
 * Port of OpenJDK 25 HotSpot BytecodeInterpreter (share/interpreter/zero/bytecodeInterpreter.hpp, bytecodeInterpreter.cpp)
 *
 * BytecodeInterpreter implements the core bytecode interpretation execution loop,
 * operand stack and frame management, exception table searching and dispatch,
 * and invocation of virtual (vtable), interface (itable), special, static, and dynamic calls.
 */

use super::bytecodes;
use super::linkResolver::LinkResolver;
use super::numeric;
use crate::classfile::decodeString;
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::systemDictionary::SystemDictionary;
use crate::share::interpreter::bootstrapInfo::BootstrapArg;
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::{arrayKlass::ArrayKlass, typeArrayOop::Element};
use crate::share::runtime::signature;
use crate::{
    classfile::Constant,
    share::oops::{
        instanceKlass::{InitState, InstanceKlass},
        method::Code,
    },
};
use std::io::{self, Error, Write};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(&'a [u8]),
    ClassMirror(&'a [u8]),
    HostString(&'a str),
    Arguments(&'a [String]),
    PrimitiveArray(usize),
    Object(usize),
    ObjectArray(usize),
    ReturnAddress(usize),
    Null,
    Output,
    Empty,
}

impl Value<'_> {
    pub fn matchesLocal(self, kind: u8) -> bool {
        matches!(
            (kind, self),
            (0, Self::Int(_))
                | (1, Self::Long(_))
                | (2, Self::Float(_))
                | (3, Self::Double(_))
                | (
                    4,
                    Self::String(_)
                        | Self::ClassMirror(_)
                        | Self::HostString(_)
                        | Self::Arguments(_)
                        | Self::Output
                        | Self::PrimitiveArray(_)
                        | Self::Object(_)
                        | Self::ObjectArray(_)
                        | Self::ReturnAddress(_)
                        | Self::Null
                )
        )
    }

    pub fn slots(self) -> usize {
        if matches!(self, Self::Long(_) | Self::Double(_)) {
            2
        } else {
            1
        }
    }

    pub fn int(self) -> io::Result<i32> {
        match self {
            Self::Int(value) => Ok(value),
            _ => Err(Error::other("Expected int")),
        }
    }

    pub fn long(self) -> io::Result<i64> {
        match self {
            Self::Long(value) => Ok(value),
            _ => Err(Error::other("Expected long")),
        }
    }

    pub fn float(self) -> io::Result<f32> {
        match self {
            Self::Float(value) => Ok(value),
            _ => Err(Error::other("Expected float")),
        }
    }

    pub fn double(self) -> io::Result<f64> {
        match self {
            Self::Double(value) => Ok(value),
            _ => Err(Error::other("Expected double")),
        }
    }
}

struct Frame<'a> {
    code: Code<'a>,
    pc: usize,
    locals: Vec<Value<'a>>,
    stack: Vec<Value<'a>>,
    stackSlots: usize,
    owner: &'a [u8],
}

impl<'a> Frame<'a> {
    fn read<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let bytes = self
            .code
            .bytes
            .get(self.pc..)
            .and_then(|bytes| bytes.first_chunk::<N>())
            .ok_or_else(|| Error::other("Truncated instruction"))?;
        self.pc += N;
        Ok(*bytes)
    }

    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.read::<1>()?[0])
    }

    fn pop(&mut self) -> io::Result<Value<'a>> {
        let value = self
            .stack
            .pop()
            .ok_or_else(|| Error::other("Operand stack underflow"))?;
        self.stackSlots -= value.slots();
        Ok(value)
    }

    fn push(&mut self, value: Value<'a>) -> io::Result<()> {
        let slots = self.stackSlots + value.slots();
        if slots > self.code.maxStack {
            return Err(Error::other(format!(
                "Operand stack overflow: {} slots needed, {} max at pc {} in {}",
                slots,
                self.code.maxStack,
                self.pc,
                String::from_utf8_lossy(self.locals.first().map(|_| b"".as_slice()).unwrap_or(b""))
            )));
        }
        self.stack.push(value);
        self.stackSlots = slots;
        Ok(())
    }

    fn local(&mut self, index: u16) -> io::Result<&mut Value<'a>> {
        self.locals
            .get_mut(usize::from(index))
            .ok_or_else(|| Error::other("Invalid local index"))
    }

    fn store(&mut self, index: u16, value: Value<'a>) -> io::Result<()> {
        let index = usize::from(index);
        if index + value.slots() > self.locals.len() {
            return Err(Error::other("Invalid local index"));
        }
        if index > 0 && self.locals[index - 1].slots() == 2 {
            self.locals[index - 1] = Value::Empty;
        }
        self.locals[index] = value;
        if value.slots() == 2 {
            self.locals[index + 1] = Value::Empty;
        }
        Ok(())
    }

    fn jump(&mut self, pc: usize, offset: i32) -> io::Result<()> {
        self.pc = pc
            .checked_add_signed(offset as isize)
            .filter(|target| *target < self.code.bytes.len())
            .ok_or_else(|| Error::other("Invalid branch target"))?;
        Ok(())
    }

    fn increment(&mut self, index: u16, increment: i32) -> io::Result<()> {
        let local = self.local(index)?;
        *local = Value::Int(local.int()?.wrapping_add(increment));
        Ok(())
    }

    fn load(&mut self, index: u16, kind: u8) -> io::Result<Value<'a>> {
        let value = *self.local(index)?;
        if !value.matchesLocal(kind) {
            return Err(Error::other("Invalid local type"));
        }
        Ok(value)
    }

    fn storeOperand(&mut self, index: u16, kind: u8) -> io::Result<()> {
        let value = self.pop()?;
        if !value.matchesLocal(kind) {
            return Err(Error::other("Invalid local type"));
        }
        self.store(index, value)
    }

    fn readInt(&mut self) -> io::Result<i32> {
        Ok(i32::from_be_bytes(self.read()?))
    }

    fn switch(&mut self, pc: usize, dense: bool) -> io::Result<()> {
        let key = self.pop()?.int()?;
        self.pc = self.pc.next_multiple_of(4);
        let default = self.readInt()?;
        let offset = if dense {
            let low = self.readInt()?;
            let high = self.readInt()?;
            let count = i64::from(high) - i64::from(low) + 1;
            if count <= 0 || count as u64 > ((self.code.bytes.len() - self.pc) / 4) as u64 {
                return Err(Error::other("Invalid tableswitch bounds"));
            }
            if (low..=high).contains(&key) {
                self.pc += ((i64::from(key) - i64::from(low)) as usize) * 4;
                self.readInt()?
            } else {
                default
            }
        } else {
            let count = self.readInt()?;
            if count < 0 || count as usize > (self.code.bytes.len() - self.pc) / 8 {
                return Err(Error::other("Invalid lookupswitch size"));
            }
            let mut offset = default;
            let mut previous = None;
            for _ in 0..count {
                let candidate = self.readInt()?;
                let target = self.readInt()?;
                if previous.is_some_and(|prev| prev >= candidate) {
                    return Err(Error::other("Invalid lookupswitch ordering"));
                }
                if candidate == key {
                    offset = target;
                }
                previous = Some(candidate);
            }
            offset
        };
        self.jump(pc, offset)
    }

    fn handle_exception(
        &mut self,
        pc: usize,
        exc: Value<'a>,
        loader: &ClassLoader<'a>,
        klass: &InstanceKlass<'a>,
    ) -> bool {
        for entry in &self.code.exception_table {
            if entry.start_pc <= pc && pc < entry.end_pc {
                let is_match = if entry.catch_type == 0 {
                    true
                } else if let Ok(catch_name) = klass.className(entry.catch_type) {
                    if loader.referenceAssignable(exc, catch_name).unwrap_or(false) {
                        true
                    } else if matches!(exc, Value::Null) {
                        true
                    } else if let Value::Object(handle) = exc {
                        let objects = loader.objects.borrow();
                        if let Some(obj) = objects.get(handle) {
                            SystemDictionary::is_subclass_of(&obj.klass, catch_name)
                                || SystemDictionary::implements_interface(&obj.klass, catch_name)
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if is_match {
                    self.stack.clear();
                    self.stackSlots = exc.slots();
                    self.stack.push(exc);
                    self.pc = entry.handler_pc;
                    return true;
                }
            }
        }
        false
    }
}

pub(crate) fn initialize<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    output: &mut impl Write,
    depth: usize,
) -> io::Result<()> {
    match klass.state.get() {
        InitState::FullyInitialized | InitState::BeingInitialized => return Ok(()),
        InitState::InitializationError => {
            return Err(Error::other("Class initialization previously failed"));
        }
        _ => {}
    }
    klass.state.set(InitState::BeingInitialized);
    let result = (|| {
        if (klass.class.accessFlags & 0x0200) == 0
            && let Some(superKlass) = &klass.super_klass
        {
            initialize(loader, superKlass, output, depth + 1)?;
        }
        let code = klass.initializer().map_err(|error| {
            Error::other(format!(
                "{}: {error}",
                String::from_utf8_lossy(klass.class.name)
            ))
        })?;
        if let Some(code) = code
            && execute(loader, klass, code, &[], output, depth)?.is_some()
        {
            return Err(Error::other("Class initializer must return void"));
        }
        Ok(())
    })();
    klass.state.set(if result.is_ok() {
        InitState::FullyInitialized
    } else {
        InitState::InitializationError
    });
    result
}

pub fn execute<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    code: Code<'a>,
    arguments: &[Value<'a>],
    output: &mut impl Write,
    depth: usize,
) -> io::Result<Option<Value<'a>>> {
    if depth >= 128 {
        return Err(Error::other("Invalid frame or call depth exceeded"));
    }
    let mut locals = vec![Value::Empty; code.maxLocals];
    let mut index = 0;
    for &value in arguments {
        if index + value.slots() > locals.len() {
            return Err(Error::other("Arguments exceed local slots"));
        }
        locals[index] = value;
        index += value.slots();
    }
    let mut frame = Frame {
        stack: Vec::with_capacity(code.maxStack),
        stackSlots: 0,
        code,
        pc: 0,
        locals,
    };
    loop {
        let pc = frame.pc;
        let opcode = frame.byte()?;
        let value = match opcode {
            0x00 => continue,                                                 // nop
            0x01 => Value::Null,                                              // aconst_null
            0x02..=0x08 => Value::Int(i32::from(opcode) - 3),                 // iconst_m1..5
            0x09..=0x0a => Value::Long(i64::from(opcode - 9)),                // lconst_0..1
            0x0b..=0x0d => Value::Float(f32::from(opcode - 0x0b)),            // fconst_0..2
            0x0e..=0x0f => Value::Double(f64::from(opcode - 0x0e)),           // dconst_0..1
            0x10 => Value::Int(i32::from(frame.byte()? as i8)),               // bipush
            0x11 => Value::Int(i32::from(i16::from_be_bytes(frame.read()?))), // sipush
            0x12..=0x14 => {
                let index = if opcode == 0x12 {
                    u16::from(frame.byte()?)
                } else {
                    u16::from_be_bytes(frame.read()?)
                };
                match (opcode, klass.constant(index)?) {
                    (0x14, Constant::Long(value)) => Value::Long(*value),
                    (0x14, Constant::Double(value)) => Value::Double(*value),
                    (0x14, _) => return Err(Error::other("Invalid ldc2_w constant")),
                    (_, Constant::Integer(value)) => Value::Int(*value),
                    (_, Constant::Float(value)) => Value::Float(*value),
                    (_, Constant::String(index)) => match klass.constant(*index)? {
                        Constant::Utf8(bytes) => Value::String(bytes),
                        _ => return Err(Error::other("Invalid string constant")),
                    },
                    (_, Constant::Class(c_idx)) => {
                        let name = klass.utf8(*c_idx)?;
                        Value::ClassMirror(name)
                    }
                    _ => return Err(Error::other("Unsupported ldc constant")),
                }
            }
            0x15..=0x2d => {
                let index = if opcode < 0x1a {
                    frame.byte()?
                } else {
                    (opcode - 0x1a) % 4
                };
                let kind = if opcode < 0x1a {
                    opcode - 0x15
                } else {
                    (opcode - 0x1a) / 4
                };
                frame.load(u16::from(index), kind)?
            }
            0x2e..=0x31 | 0x33..=0x35 => {
                let index = frame.pop()?.int()? as usize;
                let Value::PrimitiveArray(handle) = frame.pop()? else {
                    return Err(Error::other("Expected primitive array"));
                };
                let arrays = loader.arrays.borrow();
                let array = arrays
                    .get(handle)
                    .ok_or_else(|| Error::other("Invalid array reference"))?;
                match array.load(opcode - 0x2e, index)? {
                    Element::Int(value) => Value::Int(value),
                    Element::Long(value) => Value::Long(value),
                    Element::Float(value) => Value::Float(value),
                    Element::Double(value) => Value::Double(value),
                }
            }
            0x32 => {
                let index = frame.pop()?.int()? as usize;
                match frame.pop()? {
                    Value::Arguments(arguments) => Value::HostString(
                        arguments
                            .get(index)
                            .ok_or_else(|| Error::other("Array index out of bounds"))?,
                    ),
                    Value::ObjectArray(handle) => {
                        let arrays = loader.obj_arrays.borrow();
                        let array = arrays
                            .get(handle)
                            .ok_or_else(|| Error::other("Invalid object array reference"))?;
                        array.get(index)?
                    }
                    _ => return Err(Error::other("Expected object array or arguments")),
                }
            }
            0x36..=0x4e => {
                let index = if opcode < 0x3b {
                    frame.byte()?
                } else {
                    (opcode - 0x3b) % 4
                };
                let kind = if opcode < 0x3b {
                    opcode - 0x36
                } else {
                    (opcode - 0x3b) / 4
                };
                frame.storeOperand(u16::from(index), kind)?;
                continue;
            }
            0x4f..=0x52 | 0x54..=0x56 => {
                let value = match frame.pop()? {
                    Value::Int(value) => Element::Int(value),
                    Value::Long(value) => Element::Long(value),
                    Value::Float(value) => Element::Float(value),
                    Value::Double(value) => Element::Double(value),
                    _ => return Err(Error::other("Expected numeric array element")),
                };
                let index = frame.pop()?.int()? as usize;
                let Value::PrimitiveArray(handle) = frame.pop()? else {
                    return Err(Error::other("Expected primitive array"));
                };
                loader
                    .arrays
                    .borrow_mut()
                    .get_mut(handle)
                    .ok_or_else(|| Error::other("Invalid array reference"))?
                    .store(opcode - 0x4f, index, value)?;
                continue;
            }
            0x53 => {
                let value = frame.pop()?;
                let index = frame.pop()?.int()? as usize;
                let Value::ObjectArray(handle) = frame.pop()? else {
                    return Err(Error::other("Expected object array"));
                };
                let component = {
                    let arrays = loader.obj_arrays.borrow();
                    let array = arrays
                        .get(handle)
                        .ok_or_else(|| Error::other("Invalid object array reference"))?;
                    array.get(index)?;
                    array.component
                };
                if !loader.referenceAssignable(value, component)? {
                    return Err(Error::other("ArrayStoreException"));
                }
                loader.obj_arrays.borrow_mut()[handle].set(index, value)?;
                continue;
            }
            0x57 => {
                if frame.pop()?.slots() != 1 {
                    return Err(Error::other("Invalid pop operand"));
                }
                continue;
            }
            0x58 => {
                if frame.pop()?.slots() == 1 && frame.pop()?.slots() != 1 {
                    return Err(Error::other("Invalid pop2 operands"));
                }
                continue;
            }
            0x59 => *frame
                .stack
                .last()
                .filter(|value| value.slots() == 1)
                .ok_or_else(|| Error::other("Invalid dup operand"))?,
            0x5a => {
                let v1 = frame.pop()?;
                let v2 = frame.pop()?;
                if v1.slots() != 1 || v2.slots() != 1 {
                    return Err(Error::other("Invalid dup_x1 operand size"));
                }
                frame.push(v1)?;
                frame.push(v2)?;
                frame.push(v1)?;
                continue;
            }
            0x5b => {
                let v1 = frame.pop()?;
                if v1.slots() != 1 {
                    return Err(Error::other("Invalid dup_x2 operand size"));
                }
                let v2 = frame.pop()?;
                if v2.slots() == 2 {
                    frame.push(v1)?;
                    frame.push(v2)?;
                    frame.push(v1)?;
                } else {
                    let v3 = frame.pop()?;
                    if v3.slots() != 1 {
                        return Err(Error::other("Invalid dup_x2 operand size"));
                    }
                    frame.push(v1)?;
                    frame.push(v3)?;
                    frame.push(v2)?;
                    frame.push(v1)?;
                }
                continue;
            }
            0x5c => {
                let v1 = frame.pop()?;
                if v1.slots() == 2 {
                    frame.push(v1)?;
                    frame.push(v1)?;
                } else {
                    let v2 = frame.pop()?;
                    if v2.slots() != 1 {
                        return Err(Error::other("Invalid dup2 operand size"));
                    }
                    frame.push(v2)?;
                    frame.push(v1)?;
                    frame.push(v2)?;
                    frame.push(v1)?;
                }
                continue;
            }
            0x5d => {
                let v1 = frame.pop()?;
                let v2 = frame.pop()?;
                if v1.slots() == 2 && v2.slots() == 1 {
                    frame.push(v1)?;
                    frame.push(v2)?;
                    frame.push(v1)?;
                } else if v1.slots() == 1 && v2.slots() == 1 {
                    let v3 = frame.pop()?;
                    if v3.slots() != 1 {
                        return Err(Error::other("Invalid dup2_x1"));
                    }
                    frame.push(v2)?;
                    frame.push(v1)?;
                    frame.push(v3)?;
                    frame.push(v2)?;
                    frame.push(v1)?;
                } else {
                    return Err(Error::other("Invalid dup2_x1 operand size"));
                }
                continue;
            }
            0x5e => {
                let v1 = frame.pop()?;
                let v2 = frame.pop()?;
                if v1.slots() == 2 && v2.slots() == 2 {
                    frame.push(v1)?;
                    frame.push(v2)?;
                    frame.push(v1)?;
                } else if v1.slots() == 2 && v2.slots() == 1 {
                    let v3 = frame.pop()?;
                    if v3.slots() != 1 {
                        return Err(Error::other("Invalid dup2_x2"));
                    }
                    frame.push(v1)?;
                    frame.push(v3)?;
                    frame.push(v2)?;
                    frame.push(v1)?;
                } else if v1.slots() == 1 && v2.slots() == 1 {
                    let v3 = frame.pop()?;
                    if v3.slots() == 2 {
                        frame.push(v2)?;
                        frame.push(v1)?;
                        frame.push(v3)?;
                        frame.push(v2)?;
                        frame.push(v1)?;
                    } else {
                        let v4 = frame.pop()?;
                        if v4.slots() != 1 {
                            return Err(Error::other("Invalid dup2_x2"));
                        }
                        frame.push(v2)?;
                        frame.push(v1)?;
                        frame.push(v4)?;
                        frame.push(v3)?;
                        frame.push(v2)?;
                        frame.push(v1)?;
                    }
                } else {
                    return Err(Error::other("Invalid dup2_x2 operand size"));
                }
                continue;
            }
            0x5f => {
                let v1 = frame.pop()?;
                let v2 = frame.pop()?;
                if v1.slots() != 1 || v2.slots() != 1 {
                    return Err(Error::other("Invalid swap operand size"));
                }
                frame.push(v1)?;
                frame.push(v2)?;
                continue;
            }
            0x60 | 0x64 | 0x68 | 0x6c | 0x70 | 0x78 | 0x7a | 0x7c | 0x7e | 0x80 | 0x82 => {
                let right = frame.pop()?.int()?;
                let left = frame.pop()?.int()?;
                if matches!(opcode, 0x6c | 0x70) && right == 0 {
                    if frame.handle_exception(pc, Value::Null, loader, klass) {
                        continue;
                    }
                    return Err(Error::other("java.lang.ArithmeticException: / by zero"));
                }
                Value::Int(match opcode {
                    0x60 => left.wrapping_add(right),
                    0x64 => left.wrapping_sub(right),
                    0x68 => left.wrapping_mul(right),
                    0x6c => left.wrapping_div(right),
                    0x70 => left.wrapping_rem(right),
                    0x78 => left.wrapping_shl((right as u32) & 0x1f),
                    0x7a => left.wrapping_shr((right as u32) & 0x1f),
                    0x7c => (left as u32).wrapping_shr((right as u32) & 0x1f) as i32,
                    0x7e => left & right,
                    0x80 => left | right,
                    _ => left ^ right,
                })
            }
            0x79 => {
                // lshl
                let right = frame.pop()?.int()?;
                let left = frame.pop()?.long()?;
                Value::Long(left.wrapping_shl((right as u32) & 0x3f))
            }
            0x7b => {
                // lshr
                let right = frame.pop()?.int()?;
                let left = frame.pop()?.long()?;
                Value::Long(left.wrapping_shr((right as u32) & 0x3f))
            }
            0x7d => {
                // lushr
                let right = frame.pop()?.int()?;
                let left = frame.pop()?.long()?;
                Value::Long((left as u64).wrapping_shr((right as u32) & 0x3f) as i64)
            }
            0x7f => {
                // land
                let right = frame.pop()?.long()?;
                let left = frame.pop()?.long()?;
                Value::Long(left & right)
            }
            0x81 => {
                // lor
                let right = frame.pop()?.long()?;
                let left = frame.pop()?.long()?;
                Value::Long(left | right)
            }
            0x83 => {
                // lxor
                let right = frame.pop()?.long()?;
                let left = frame.pop()?.long()?;
                Value::Long(left ^ right)
            }
            0x61..=0x63 | 0x65..=0x67 | 0x69..=0x6b | 0x6d..=0x6f | 0x71..=0x73 => {
                let right = frame.pop()?;
                let left = frame.pop()?;
                if matches!(opcode, 0x6d | 0x71) {
                    if let Value::Long(r) = right
                        && r == 0
                    {
                        if frame.handle_exception(pc, Value::Null, loader, klass) {
                            continue;
                        }
                        return Err(Error::other("java.lang.ArithmeticException: / by zero"));
                    }
                }
                numeric::binary(opcode, left, right)?
            }
            0x74 => Value::Int(frame.pop()?.int()?.wrapping_neg()),
            0x75..=0x77 => match (opcode, frame.pop()?) {
                (0x75, Value::Long(value)) => Value::Long(value.wrapping_neg()),
                (0x76, Value::Float(value)) => Value::Float(-value),
                (0x77, Value::Double(value)) => Value::Double(-value),
                _ => return Err(Error::other("Invalid negation operand")),
            },
            0x84 => {
                let [index, increment] = frame.read()?;
                frame.increment(u16::from(index), i32::from(increment as i8))?;
                continue;
            }
            0x85..=0x93 => numeric::convert(opcode, frame.pop()?)?,
            0x94..=0x98 => {
                let right = frame.pop()?;
                Value::Int(numeric::compare(opcode, frame.pop()?, right)?)
            }
            0x99..=0xa4 | 0xa7 => {
                let offset = i16::from_be_bytes(frame.read()?);
                let jump = if opcode == 0xa7 {
                    true
                } else {
                    let right = frame.pop()?.int()?;
                    let (left, right) = if opcode < 0x9f {
                        (right, 0)
                    } else {
                        (frame.pop()?.int()?, right)
                    };
                    match (opcode - 0x99) % 6 {
                        0 => left == right,
                        1 => left != right,
                        2 => left < right,
                        3 => left >= right,
                        4 => left > right,
                        _ => left <= right,
                    }
                };
                if jump {
                    frame.jump(pc, i32::from(offset))?;
                }
                continue;
            }
            0xa5 | 0xa6 => {
                let offset = i16::from_be_bytes(frame.read()?);
                let v2 = frame.pop()?;
                let v1 = frame.pop()?;
                let eq = match (v1, v2) {
                    (Value::Null, Value::Null) => true,
                    (Value::Object(a), Value::Object(b)) => a == b,
                    (Value::PrimitiveArray(a), Value::PrimitiveArray(b)) => a == b,
                    (Value::ObjectArray(a), Value::ObjectArray(b)) => a == b,
                    (Value::String(a), Value::String(b)) => a == b,
                    (Value::ClassMirror(a), Value::ClassMirror(b)) => a == b,
                    (Value::HostString(a), Value::HostString(b)) => a == b,
                    _ => false,
                };
                if eq == (opcode == 0xa5) {
                    frame.jump(pc, i32::from(offset))?;
                }
                continue;
            }
            0xa8 => {
                // jsr
                let offset = i16::from_be_bytes(frame.read()?);
                let return_address = frame.pc;
                frame.push(Value::ReturnAddress(return_address))?;
                frame.jump(pc, i32::from(offset))?;
                continue;
            }
            0xa9 => {
                // ret
                let index = frame.byte()?;
                let val = *frame.local(u16::from(index))?;
                let Value::ReturnAddress(target_pc) = val else {
                    return Err(Error::other("Expected return address in ret"));
                };
                frame.pc = target_pc;
                continue;
            }
            0xaa | 0xab => {
                frame.switch(pc, opcode == 0xaa)?;
                continue;
            }
            0xac..=0xb0 => {
                let value = frame.pop()?;
                if !value.matchesLocal(opcode - 0xac) {
                    return Err(Error::other("Invalid return operand"));
                }
                return Ok(Some(value));
            }
            0xb1 => return Ok(None),
            0xb2 | 0xb3 => {
                let [owner, name, descriptor] =
                    klass.memberRef(u16::from_be_bytes(frame.read()?), true)?;
                if opcode == 0xb2
                    && [owner, name, descriptor]
                        == [
                            b"java/lang/System".as_slice(),
                            b"out",
                            b"Ljava/io/PrintStream;",
                        ]
                {
                    Value::Output
                } else {
                    if opcode == 0xb2
                        && owner == b"jdk/internal/misc/UnsafeConstants"
                        && let Some(value) = crate::share::prims::r#unsafe::Unsafe::platformConstant(
                            name, descriptor,
                        )
                    {
                        frame.push(Value::Int(value))?;
                        continue;
                    }
                    let target = loader.load(owner)?.staticFieldOwner(name, descriptor)?;
                    let field = target.staticField(name, descriptor)?;
                    initialize(loader, &target, output, depth + 1)?;
                    if opcode == 0xb3 {
                        let mut value = frame.pop()?;
                        if !value.matchesLocal(signature::fieldKind(descriptor)?) {
                            return Err(Error::other("Invalid static field value"));
                        }
                        value = match (descriptor, value) {
                            (b"Z", Value::Int(v)) => Value::Int(v & 1),
                            (b"B", Value::Int(v)) => Value::Int(i32::from(v as i8)),
                            (b"C", Value::Int(v)) => Value::Int(i32::from(v as u16)),
                            (b"S", Value::Int(v)) => Value::Int(i32::from(v as i16)),
                            _ => value,
                        };
                        field.set(value);
                        continue;
                    }
                    field.get()
                }
            }
            0xb4 | 0xb5 => {
                let [owner, name, descriptor] =
                    klass.memberRef(u16::from_be_bytes(frame.read()?), true)?;
                let value = (opcode == 0xb5).then(|| frame.pop()).transpose()?;
                let Value::Object(handle) = frame.pop()? else {
                    return Err(Error::other("Expected instance field receiver"));
                };
                let mut objects = loader.objects.borrow_mut();
                let object = objects
                    .get_mut(handle)
                    .ok_or_else(|| Error::other("Invalid object reference"))?;
                if !SystemDictionary::is_subclass_of(&object.klass, owner) {
                    return Err(Error::other("Unsupported field receiver class"));
                }
                let ownerKlass = loader.load(owner)?;
                let index = ownerKlass.instanceField(name, descriptor)?;
                if let Some(mut val) = value {
                    if !val.matchesLocal(signature::fieldKind(descriptor)?) {
                        return Err(Error::other("Invalid instance field value"));
                    }
                    val = match (descriptor, val) {
                        (b"Z", Value::Int(v)) => Value::Int(v & 1),
                        (b"B", Value::Int(v)) => Value::Int(i32::from(v as i8)),
                        (b"C", Value::Int(v)) => Value::Int(i32::from(v as u16)),
                        (b"S", Value::Int(v)) => Value::Int(i32::from(v as i16)),
                        _ => val,
                    };
                    object.fields[index] = val;
                    continue;
                }
                object.fields[index]
            }
            0xb6..=0xb9 => {
                let index = u16::from_be_bytes(frame.read()?);
                let interfaceCount = if opcode == 0xb9 {
                    let [count, zero] = frame.read()?;
                    if zero != 0
                        || !matches!(klass.constant(index)?, Constant::InterfaceMethodRef(..))
                    {
                        return Err(Error::other("Invalid invokeinterface operands"));
                    }
                    Some(usize::from(count))
                } else {
                    None
                };
                let [owner, name, descriptor] = klass.memberRef(index, false)?;

                if matches!(opcode, 0xb6 | 0xb9)
                    && name == b"getClass"
                    && descriptor == b"()Ljava/lang/Class;"
                {
                    let value = crate::share::prims::classNatives::classOf(loader, frame.pop()?)?;
                    frame.push(value)?;
                    continue;
                }
                if opcode == 0xb6
                    && owner == b"java/io/PrintStream"
                    && (name == b"println" || name == b"print")
                {
                    let is_println = name == b"println";
                    let value = if descriptor == b"()V" {
                        Value::Empty
                    } else {
                        frame.pop()?
                    };
                    if !matches!(frame.pop()?, Value::Output) {
                        return Err(Error::other("Invalid print receiver"));
                    }
                    match (descriptor, value) {
                        (b"()V", _) => {
                            if is_println {
                                writeln!(output)?;
                            }
                        }
                        (b"(I)V", Value::Int(v))
                        | (b"(B)V", Value::Int(v))
                        | (b"(S)V", Value::Int(v)) => {
                            if is_println {
                                writeln!(output, "{v}")?;
                            } else {
                                write!(output, "{v}")?;
                            }
                        }
                        (b"(Z)V", Value::Int(v)) => {
                            let text = if v != 0 { "true" } else { "false" };
                            if is_println {
                                writeln!(output, "{text}")?;
                            } else {
                                write!(output, "{text}")?;
                            }
                        }
                        (b"(C)V", Value::Int(v)) => {
                            let ch = char::from_u32(v as u32).unwrap_or('?');
                            if is_println {
                                writeln!(output, "{ch}")?;
                            } else {
                                write!(output, "{ch}")?;
                            }
                        }
                        (b"(J)V", Value::Long(v)) => {
                            if is_println {
                                writeln!(output, "{v}")?;
                            } else {
                                write!(output, "{v}")?;
                            }
                        }
                        (b"(F)V", Value::Float(v)) => {
                            if is_println {
                                writeln!(output, "{v}")?;
                            } else {
                                write!(output, "{v}")?;
                            }
                        }
                        (b"(D)V", Value::Double(v)) => {
                            if is_println {
                                writeln!(output, "{v}")?;
                            } else {
                                write!(output, "{v}")?;
                            }
                        }
                        (b"(Ljava/lang/String;)V", Value::HostString(v)) => {
                            if is_println {
                                writeln!(output, "{v}")?;
                            } else {
                                write!(output, "{v}")?;
                            }
                        }
                        (b"(Ljava/lang/String;)V", Value::String(v)) => {
                            let text = decodeString(v)?;
                            if is_println {
                                writeln!(output, "{text}")?;
                            } else {
                                write!(output, "{text}")?;
                            }
                        }
                        (b"(Ljava/lang/Object;)V", Value::Null)
                        | (b"(Ljava/lang/String;)V", Value::Null) => {
                            if is_println {
                                writeln!(output, "null")?;
                            } else {
                                write!(output, "null")?;
                            }
                        }
                        (b"(Ljava/lang/Object;)V", Value::Object(handle)) => {
                            if is_println {
                                writeln!(output, "Object@{handle:x}")?;
                            } else {
                                write!(output, "Object@{handle:x}")?;
                            }
                        }
                        (b"(Ljava/lang/Object;)V", Value::HostString(v)) => {
                            if is_println {
                                writeln!(output, "{v}")?;
                            } else {
                                write!(output, "{v}")?;
                            }
                        }
                        (b"(Ljava/lang/Object;)V", Value::String(v)) => {
                            let text = decodeString(v)?;
                            if is_println {
                                writeln!(output, "{text}")?;
                            } else {
                                write!(output, "{text}")?;
                            }
                        }
                        _ => return Err(Error::other("Unsupported println/print signature")),
                    }
                    continue;
                }

                let (parameters, result) = signature::method(descriptor)?;
                let count = parameters.count();
                let receiverSlots = usize::from(opcode != 0xb8);
                if count + receiverSlots > 255 {
                    return Err(Error::other("Too many method parameters"));
                }
                let start = frame
                    .stack
                    .len()
                    .checked_sub(count + receiverSlots)
                    .ok_or_else(|| Error::other("Missing method arguments"))?;
                let mut slots = receiverSlots;
                for (kind, &val) in signature::method(descriptor)?
                    .0
                    .zip(&frame.stack[start + receiverSlots..])
                {
                    if !val.matchesLocal(kind?) {
                        return Err(Error::other("Invalid method argument"));
                    }
                    slots += val.slots();
                }
                if slots > 255 {
                    return Err(Error::other("Too many method argument slots"));
                }
                if interfaceCount.is_some_and(|count| count != slots) {
                    return Err(Error::other("Invalid invokeinterface argument count"));
                }
                if matches!(opcode, 0xb6 | 0xb9)
                    && matches!(
                        owner,
                        b"java/lang/String" | b"java/lang/CharSequence" | b"java/lang/Object"
                    )
                    && let Some(value) = crate::share::prims::stringNatives::invoke(
                        loader,
                        name,
                        descriptor,
                        &frame.stack[start..],
                    )?
                {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    if let Some(value) = value {
                        frame.push(value)?;
                    }
                    continue;
                }
                if opcode == 0xb6
                    && owner == b"java/lang/Object"
                    && name == b"hashCode"
                    && descriptor == b"()I"
                    && let Some(hash) =
                        crate::share::prims::objectNatives::hashCode(loader, frame.stack[start])?
                {
                    frame.pop()?;
                    frame.push(Value::Int(hash))?;
                    continue;
                }
                if opcode == 0xb6
                    && owner == b"java/lang/Class"
                    && let Some(value) = crate::share::prims::classNatives::invoke(
                        loader,
                        name,
                        descriptor,
                        &frame.stack[start..],
                    )?
                {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    frame.push(value)?;
                    continue;
                }
                if opcode != 0xb8
                    && let Some(result) = crate::share::prims::nativeLookup::invokeInstance(
                        loader,
                        owner,
                        name,
                        descriptor,
                        &frame.stack[start..],
                    )?
                {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    if let Some(value) = result {
                        frame.push(value)?;
                    }
                    continue;
                }

                if opcode == 0xb8 {
                    if owner == b"java/lang/System"
                        && descriptor == b"(Ljava/lang/String;)V"
                        && matches!(name, b"load" | b"loadLibrary")
                    {
                        let value = frame.pop()?;
                        let path = crate::share::prims::stringNatives::string(loader, value)?
                            .ok_or_else(|| Error::other("Expected library name"))?;
                        let libraries =
                            crate::share::prims::nativeLibraries::NativeLibraries::global();
                        if name == b"load" {
                            libraries.load(std::path::Path::new(path.as_ref()))?;
                        } else {
                            let properties = loader.systemProperties.borrow();
                            libraries.loadLibraryWithPaths(
                                path.as_ref(),
                                properties.get("java.library.path").copied(),
                            )?;
                        }
                        continue;
                    }
                    if klass.class.name == b"jdk/internal/misc/Unsafe"
                        && [owner, name, descriptor]
                            == [
                                b"jdk/internal/misc/Unsafe".as_slice(),
                                b"registerNatives",
                                b"()V",
                            ]
                    {
                        continue;
                    }
                    if [owner, name, descriptor]
                        == [
                            b"jdk/internal/misc/CDS".as_slice(),
                            b"initializeFromArchive",
                            b"(Ljava/lang/Class;)V",
                        ]
                    {
                        match frame.pop()? {
                            Value::ClassMirror(_) => {
                                crate::share::cds::cdsConfig::CDSConfig::initializeFromArchive()?
                            }
                            Value::Null => return Err(Error::other("NullPointerException")),
                            _ => return Err(Error::other("Expected class mirror")),
                        }
                        continue;
                    }
                    if [owner, name, descriptor]
                        == [
                            b"java/lang/Class".as_slice(),
                            b"getPrimitiveClass",
                            b"(Ljava/lang/String;)Ljava/lang/Class;",
                        ]
                    {
                        let value =
                            crate::share::prims::classNatives::primitiveClass(frame.pop()?)?;
                        frame.push(value)?;
                        continue;
                    }
                    if owner == b"java/lang/System"
                        && let Some(value) = crate::share::prims::systemProperties::invoke(
                            loader,
                            name,
                            descriptor,
                            &frame.stack[start..],
                        )?
                    {
                        frame.stack.truncate(start);
                        frame.stackSlots -= slots;
                        frame.push(value)?;
                        continue;
                    }
                    if (owner, name, descriptor)
                        == (
                            b"java/lang/System",
                            b"arraycopy",
                            b"(Ljava/lang/Object;ILjava/lang/Object;II)V",
                        )
                    {
                        let args = frame.stack[start..]
                            .try_into()
                            .map_err(|_| Error::other("Invalid arraycopy argument count"))?;
                        crate::share::prims::arrayCopy::copy(loader, args)?;
                        frame.stack.truncate(start);
                        frame.stackSlots -= slots;
                        continue;
                    }
                    if let Some(value) = crate::share::prims::nativeLookup::invokeStatic(
                        loader,
                        owner,
                        name,
                        descriptor,
                        &frame.stack[start..],
                    ) {
                        frame.stack.truncate(start);
                        frame.stackSlots -= slots;
                        frame.push(value)?;
                        continue;
                    }
                }
                if opcode == 0xb6
                    && owner == b"jdk/internal/misc/Unsafe"
                    && descriptor == b"(Ljava/lang/Class;)I"
                    && matches!(name, b"arrayBaseOffset0" | b"arrayIndexScale0")
                    && let [Value::Object(handle), Value::ClassMirror(class)] =
                        &frame.stack[start..]
                    && loader
                        .objects
                        .borrow()
                        .get(*handle)
                        .is_some_and(|object| object.klass.class.name == owner)
                {
                    let scale = crate::share::prims::r#unsafe::Unsafe::arrayLayout(class)?;
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    frame.push(Value::Int(if name == b"arrayIndexScale0" {
                        scale
                    } else {
                        0
                    }))?;
                    continue;
                }
                let methodError = |error| {
                    Error::other(format!(
                        "{}.{}{}: {error}",
                        String::from_utf8_lossy(owner),
                        String::from_utf8_lossy(name),
                        String::from_utf8_lossy(descriptor)
                    ))
                };
                let (target, code) = if opcode == 0xb8 {
                    let target = loader.load(owner)?;
                    let (target, code) =
                        LinkResolver::resolve_static_call(&target, name, descriptor)
                            .map_err(methodError)?;
                    initialize(loader, &target, output, depth + 1)?;
                    (target, code)
                } else {
                    let Value::Object(handle) = frame.stack[start] else {
                        return Err(methodError(Error::other(format!(
                            "Expected instance method receiver at {pc} in {}",
                            String::from_utf8_lossy(klass.class.name)
                        ))));
                    };
                    let objects = loader.objects.borrow();
                    let object = objects
                        .get(handle)
                        .ok_or_else(|| Error::other("Invalid object reference"))?;
                    if opcode == 0xb9
                        && !SystemDictionary::implements_interface(&object.klass, owner)
                    {
                        return Err(Error::other("Receiver does not implement interface"));
                    }

                    if opcode == 0xb7
                        && owner == b"java/lang/Object"
                        && name == b"<init>"
                        && descriptor == b"()V"
                    {
                        frame.stack.truncate(start);
                        frame.stackSlots -= slots;
                        continue;
                    }

                    if opcode == 0xb7 {
                        let target = loader.load(owner)?;
                        LinkResolver::resolve_special_call(&target, name, descriptor)
                            .map_err(methodError)?
                    } else if opcode == 0xb9 {
                        LinkResolver::resolve_interface_call(&object.klass, owner, name, descriptor)
                            .map_err(methodError)?
                    } else {
                        LinkResolver::resolve_virtual_call(&object.klass, name, descriptor)
                            .map_err(methodError)?
                    }
                };

                let ret_result = execute(
                    loader,
                    &target,
                    code,
                    &frame.stack[start..],
                    output,
                    depth + 1,
                );

                let ret_val = match ret_result {
                    Ok(val) => val,
                    Err(e) => {
                        if frame.handle_exception(pc, Value::Null, loader, klass) {
                            continue;
                        }
                        return Err(e);
                    }
                };

                frame.stack.truncate(start);
                frame.stackSlots -= slots;
                match (result, ret_val) {
                    (Some(kind), Some(val)) if val.matchesLocal(kind) => val,
                    (None, None) => continue,
                    _ => return Err(Error::other("Invalid method return")),
                }
            }
            0xba => {
                // invokedynamic
                let index = u16::from_be_bytes(frame.read()?);
                let [zero1, zero2] = frame.read()?;
                if zero1 != 0 || zero2 != 0 {
                    return Err(Error::other("Invalid invokedynamic operands"));
                }
                let constant = klass.constant(index)?;
                let Constant::InvokeDynamic(bsm_attr_idx, nat_idx) = constant else {
                    return Err(Error::other("Expected InvokeDynamic constant"));
                };
                let Constant::NameAndType(name_idx, desc_idx) = klass.constant(*nat_idx)? else {
                    return Err(Error::other("Expected NameAndType constant"));
                };
                let name = klass.utf8(*name_idx)?;
                let descriptor = klass.utf8(*desc_idx)?;

                let caller_rc = Rc::new(InstanceKlass::new(
                    klass.class.clone(),
                    klass.super_klass(),
                    klass.interfaces.clone(),
                )?);
                let bs_info = LinkResolver::resolve_invokedynamic(
                    loader,
                    &caller_rc,
                    *bsm_attr_idx,
                    name,
                    descriptor,
                )?;

                let (parameters, result) = signature::method(descriptor)?;
                let count = parameters.count();
                let start = frame
                    .stack
                    .len()
                    .checked_sub(count)
                    .ok_or_else(|| Error::other("Missing invokedynamic arguments"))?;
                let mut slots = 0;
                for (kind, &val) in signature::method(descriptor)?.0.zip(&frame.stack[start..]) {
                    if !val.matchesLocal(kind?) {
                        return Err(Error::other("Invalid invokedynamic argument"));
                    }
                    slots += val.slots();
                }

                let mut is_string_concat =
                    name == b"makeConcat" || name == b"makeConcatWithConstants";
                if !is_string_concat {
                    if let Some(target) = bs_info.resolved_target() {
                        if target.contains("StringConcatFactory") {
                            is_string_concat = true;
                        }
                    }
                }

                if is_string_concat {
                    let mut formatted = String::new();
                    let recipe = bs_info.arguments().first().and_then(|arg| {
                        if let BootstrapArg::String(s) = arg {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    });

                    let dynamic_args: Vec<String> = frame.stack[start..]
                        .iter()
                        .map(|v| match v {
                            Value::Int(i) => i.to_string(),
                            Value::Long(l) => l.to_string(),
                            Value::Float(f) => f.to_string(),
                            Value::Double(d) => d.to_string(),
                            Value::String(bytes) => decodeString(bytes)
                                .unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string()),
                            Value::HostString(s) => s.to_string(),
                            Value::Null => "null".to_string(),
                            Value::Object(h) => format!("Object@{h:x}"),
                            _ => String::new(),
                        })
                        .collect();

                    if let Some(recipe_str) = recipe {
                        let mut arg_iter = dynamic_args.into_iter();
                        let mut const_iter = bs_info.arguments().iter().skip(1);
                        for ch in recipe_str.chars() {
                            if ch == '\u{0001}' {
                                if let Some(arg_val) = arg_iter.next() {
                                    formatted.push_str(&arg_val);
                                }
                            } else if ch == '\u{0002}' {
                                if let Some(c) = const_iter.next() {
                                    match c {
                                        BootstrapArg::String(s) => formatted.push_str(s),
                                        BootstrapArg::Int(i) => formatted.push_str(&i.to_string()),
                                        BootstrapArg::Long(l) => formatted.push_str(&l.to_string()),
                                        _ => {}
                                    }
                                }
                            } else {
                                formatted.push(ch);
                            }
                        }
                    } else {
                        for arg_val in dynamic_args {
                            formatted.push_str(&arg_val);
                        }
                    }

                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    let allocated_str = loader.storage.alloc_str(&formatted);
                    Value::HostString(allocated_str)
                } else {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    match result {
                        Some(_) => Value::Null,
                        None => continue,
                    }
                }
            }
            0xbb => {
                let name = klass.className(u16::from_be_bytes(frame.read()?))?;
                let target = loader.load(name)?;
                if (target.class.accessFlags & 0x0600) != 0 {
                    return Err(Error::other(
                        "Cannot instantiate abstract class or interface",
                    ));
                }
                initialize(loader, &target, output, depth + 1)?;
                let object = InstanceOop::new(target)?;
                let mut objects = loader.objects.borrow_mut();
                let handle = objects.len();
                objects.push(object);
                Value::Object(handle)
            }
            0xbc => {
                let kind = frame.byte()?;
                ArrayKlass::primitive(loader, kind, frame.pop()?.int()?)?
            }
            0xbd => {
                let component = klass.className(u16::from_be_bytes(frame.read()?))?;
                ArrayKlass::reference(loader, component, frame.pop()?.int()?)?
            }
            0xbe => {
                let length = match frame.pop()? {
                    Value::Arguments(arguments) => arguments.len(),
                    Value::PrimitiveArray(handle) => loader
                        .arrays
                        .borrow()
                        .get(handle)
                        .ok_or_else(|| Error::other("Invalid array reference"))?
                        .len(),
                    Value::ObjectArray(handle) => loader
                        .obj_arrays
                        .borrow()
                        .get(handle)
                        .ok_or_else(|| Error::other("Invalid object array reference"))?
                        .len(),
                    _ => return Err(Error::other("Expected array")),
                };
                Value::Int(length as i32)
            }
            0xbf => {
                // athrow
                let exc = frame.pop()?;
                if matches!(exc, Value::Null) {
                    return Err(Error::other("NullPointerException"));
                }
                if frame.handle_exception(pc, exc, loader, klass) {
                    continue;
                }
                return Err(Error::other(format!(
                    "Uncaught exception at pc {pc} in {}: {exc:?} class={}",
                    String::from_utf8_lossy(klass.class.name),
                    loader
                        .objects
                        .borrow()
                        .get(match exc {
                            Value::Object(handle) => handle,
                            _ => usize::MAX,
                        })
                        .map(|object| String::from_utf8_lossy(object.klass.class.name).into_owned())
                        .unwrap_or_else(|| "<invalid>".to_owned())
                )));
            }
            0xc0 | 0xc1 => {
                let target = klass.className(u16::from_be_bytes(frame.read()?))?;
                let value = frame.pop()?;
                if opcode == 0xc1 {
                    Value::Int(i32::from(
                        !matches!(value, Value::Null)
                            && loader.referenceAssignable(value, target)?,
                    ))
                } else {
                    if !loader.referenceAssignable(value, target)? {
                        if frame.handle_exception(pc, Value::Null, loader, klass) {
                            continue;
                        }
                        return Err(Error::other(format!(
                            "ClassCastException: cannot cast to {}",
                            String::from_utf8_lossy(target)
                        )));
                    }
                    value
                }
            }
            0xc2 | 0xc3 => {
                let val = frame.pop()?;
                if matches!(val, Value::Null) {
                    return Err(Error::other("NullPointerException"));
                }
                continue;
            }
            0xc4 => {
                // wide
                let instruction = frame.byte()?;
                let index = u16::from_be_bytes(frame.read()?);
                match instruction {
                    0x15..=0x19 => frame.load(index, instruction - 0x15)?,
                    0x36..=0x3a => {
                        frame.storeOperand(index, instruction - 0x36)?;
                        continue;
                    }
                    0x84 => {
                        let increment = i16::from_be_bytes(frame.read()?);
                        frame.increment(index, i32::from(increment))?;
                        continue;
                    }
                    0xa9 => {
                        let val = *frame.local(index)?;
                        let Value::ReturnAddress(target_pc) = val else {
                            return Err(Error::other("Expected return address in wide ret"));
                        };
                        frame.pc = target_pc;
                        continue;
                    }
                    _ => return Err(Error::other("Unsupported wide instruction")),
                }
            }
            0xc5 => {
                let descriptor = klass.className(u16::from_be_bytes(frame.read()?))?;
                let dimensions = usize::from(frame.byte()?);
                let mut lengths = [0; 255];
                for length in lengths[..dimensions].iter_mut().rev() {
                    *length = frame.pop()?.int()?;
                }
                ArrayKlass::multi(loader, descriptor, &lengths[..dimensions])?
            }
            0xc6 | 0xc7 => {
                let offset = i16::from_be_bytes(frame.read()?);
                let value = frame.pop()?;
                if !value.matchesLocal(4) {
                    return Err(Error::other("Expected reference for null comparison"));
                }
                if matches!(value, Value::Null) == (opcode == 0xc6) {
                    frame.jump(pc, i32::from(offset))?;
                }
                continue;
            }
            0xc8 => {
                // goto_w
                let offset = frame.readInt()?;
                frame.jump(pc, offset)?;
                continue;
            }
            0xc9 => {
                // jsr_w
                let offset = frame.readInt()?;
                let return_address = frame.pc;
                frame.push(Value::ReturnAddress(return_address))?;
                frame.jump(pc, offset)?;
                continue;
            }
            _ => {
                return Err(Error::other(format!(
                    "Unsupported opcode {} (0x{opcode:02x}) at {pc}",
                    bytecodes::name(opcode).unwrap_or("unknown")
                )));
            }
        };
        frame.push(value)?;
    }
}
