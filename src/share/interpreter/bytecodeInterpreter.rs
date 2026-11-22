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
use crate::share::classfile::classLoader::{CallSiteKey, ClassLoader};
use crate::share::classfile::systemDictionary::SystemDictionary;
use crate::share::interpreter::bootstrapInfo::BootstrapArg;
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::{arrayKlass::ArrayKlass, typeArrayOop::Element};
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use crate::share::runtime::signature;
use crate::{
    classfile::Constant,
    share::oops::{
        instanceKlass::{InitState, InstanceKlass},
        method::Code,
    },
};
use std::cell::{Cell, RefCell};
use std::io::{self, Error, Write};
use std::sync::Arc;

static BYTECODE_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static FRAME_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn bytecode_count() -> u64 {
    BYTECODE_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

pub static NATIVE_NANOS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn frame_count() -> u64 {
    FRAME_COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

fn jit_skip(owner: &[u8], code: &crate::share::oops::method::Code<'_>) -> bool {
    if holdsScopedValueBindings(owner, code.name) {
        return true;
    }
    static SKIP: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let skip = SKIP.get_or_init(|| {
        std::env::var("JVM25_JIT_SKIP")
            .unwrap_or_default()
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect()
    });
    if skip.is_empty() {
        return false;
    }
    let key = format!(
        "{}.{}{}",
        String::from_utf8_lossy(owner),
        String::from_utf8_lossy(code.name),
        String::from_utf8_lossy(code.descriptor)
    );
    skip.iter().any(|part| key.contains(part.as_str()))
}

struct InterpreterFlags {
    dump: bool,
    memstat: bool,
    trace: bool,
    traceExceptions: bool,
    traceBlocking: bool,
    traceCalls: bool,
    traceCompile: bool,
    noJit: bool,
    debugField: bool,
    traceStackOverflow: bool,
    traceServiceConfiguration: bool,
    traceLabeled: bool,
    traceParser: bool,
}

fn interpreterFlags() -> &'static InterpreterFlags {
    static flags: std::sync::OnceLock<InterpreterFlags> = std::sync::OnceLock::new();
    flags.get_or_init(|| {
        let enabled = |name| std::env::var_os(name).is_some();
        InterpreterFlags {
            dump: enabled("JVM25_DUMP"),
            memstat: enabled("JVM25_MEMSTAT"),
            trace: enabled("JVM25_TRACE"),
            traceExceptions: enabled("JVM25_TRACE_EXC"),
            traceBlocking: enabled("JVM25_BLOCKTRACE"),
            traceCalls: enabled("JVM25_TRACE_CALLS"),
            traceCompile: enabled("JVM25_TRACE_COMPILE"),
            noJit: enabled("JVM25_NO_JIT"),
            debugField: enabled("JVM25_DEBUG_FIELD"),
            traceStackOverflow: enabled("JVM25_SOE"),
            traceServiceConfiguration: enabled("JVM25_TRACE_SCE"),
            traceLabeled: enabled("JVM25_TRACE_LABELED"),
            traceParser: enabled("JVM25_TRACE_PARSER"),
        }
    })
}

fn sampleThreadFrames(bytecodeCount: u64) {
    thread_local! {
        static lastSample: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
    }
    let now = std::time::Instant::now();
    if !lastSample.with(|sample| {
        if sample.get().is_some_and(|last| now.duration_since(last).as_secs() == 0) {
            return false;
        }
        sample.set(Some(now));
        true
    }) {
        return;
    }
    let mut frames = Vec::new();
    for_each_active_frame(&mut |frame| frames.push(frame.site()));
    let (att, ok, deopt, thrown, unsup) = crate::share::jitruntime::jit_stats();
    let cc = crate::share::code::codeCache::CodeCache::global();
    eprintln!(
        "[dump @{bytecodeCount} tid={:?} native={}] jit(att={} ok={} deopt={} thr={} unsup={}) nmethods={} osr={} {frames:?}",
        crate::share::prims::jniReferences::current_thread_id(),
        crate::share::prims::nativeLookup::current_native(),
        att, ok, deopt, thrown, unsup,
        cc.nmethods_count(),
        cc.osr_nmethods_count(),
    );
    crate::share::runtime::threading::debug_frames(frames);
}

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

/// Payload carried inside `io::Error` for exceptions that must be
/// delivered to Java `catch` handlers. Holds the object-arena handle
/// of the thrown `java.lang.Throwable` instance.
#[derive(Debug)]
pub struct JavaThrowable(pub usize);

impl std::fmt::Display for JavaThrowable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JavaThrowable({})", self.0)
    }
}

impl std::error::Error for JavaThrowable {}

/// Uncaught Java exception that keeps both the human-readable description
/// (so `error.to_string()` still yields `name: message`) and the live
/// throwable object handle for stack-trace/cause inspection.
#[derive(Debug)]
pub struct JavaFailure {
    pub description: String,
    pub throwable: usize,
}

impl std::fmt::Display for JavaFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.description)
    }
}

impl std::error::Error for JavaFailure {}

pub(crate) fn throwable_of(error: &Error) -> Option<usize> {
    let inner = error.get_ref()?;
    if let Some(throwable) = inner.downcast_ref::<JavaThrowable>() {
        return Some(throwable.0);
    }
    inner
        .downcast_ref::<JavaFailure>()
        .map(|failure| failure.throwable)
}

thread_local! {
    static ACTIVE_FRAMES: RefCell<Vec<*const Frame<'static>>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn for_each_active_frame(f: &mut impl FnMut(&Frame<'static>)) {
    ACTIVE_FRAMES.with(|frames| {
        for &frame in frames.borrow().iter() {
            f(unsafe { &*frame });
        }
    });
}

fn holdsScopedValueBindings(owner: &[u8], method: &[u8]) -> bool {
    method == b"runWith" && matches!(owner, b"java/lang/Thread" | b"java/lang/ScopedValue$Carrier")
}

pub(crate) fn findScopedValueBindings() -> Value<'static> {
    ACTIVE_FRAMES.with(|frames| {
        frames
            .borrow()
            .iter()
            .rev()
            .find_map(|&ptr| {
                let frame = unsafe { &*ptr };
                if !holdsScopedValueBindings(frame.owner, frame.code.name) {
                    return None;
                }
                frame.locals.get(1).copied().filter(|value| *value != Value::Null)
            })
            .unwrap_or(Value::Null)
    })
}

pub fn current_frame_site() -> String {
    ACTIVE_FRAMES.with(|frames| {
        let frames = frames.borrow();
        frames
            .iter()
            .rev()
            .take(6)
            .map(|ptr| unsafe {
                let frame = &**ptr;
                format!("{}@{}", String::from_utf8_lossy(frame.owner), frame.pc)
            })
            .collect::<Vec<_>>()
            .join(" <- ")
    })
}

struct FrameRegistration;

impl FrameRegistration {
    fn new<'a>(frame: &Frame<'a>) -> Self {
        let erased: &'static Frame<'static> =
            unsafe { std::mem::transmute::<&Frame<'a>, &'static Frame<'static>>(frame) };
        ACTIVE_FRAMES.with(|frames| frames.borrow_mut().push(erased as *const _));
        Self
    }
}

impl Drop for FrameRegistration {
    fn drop(&mut self) {
        ACTIVE_FRAMES.with(|frames| {
            frames.borrow_mut().pop();
        });
    }
}

pub(crate) struct Frame<'a> {
    code: Code<'a>,
    pc: usize,
    instructionPc: usize,
    threadId: u64,
    pub(crate) locals: Vec<Value<'a>>,
    pub(crate) stack: Vec<Value<'a>>,
    stackSlots: usize,
    owner: &'a [u8],
    monitors: Vec<u64>,
    /// Raw pointer/length to the declaring class's constant pool, used by
    /// `stackWalkInfo` to resolve LineNumberTable entries. The `InstanceKlass`
    /// `Arc` owning the `ClassFile` (and thus this `Vec`'s buffer) is kept
    /// alive by the `execute*` call chain for the entire lifetime of every
    /// active interpreter frame; the buffer never reallocates after parse.
    constants_ptr: *const Constant<'a>,
    constants_len: usize,
}

impl<'a> Frame<'a> {
    pub(crate) fn locals(&self) -> &[Value<'a>] {
        &self.locals
    }

    pub(crate) fn stack(&self) -> &[Value<'a>] {
        &self.stack
    }

    pub(crate) fn stackWalkInfo(&self) -> crate::share::prims::stackwalk::StackFrameInfo {
        use crate::share::oops::symbol::Symbol;
        // SAFETY: see `constants_ptr` field docs — the owning InstanceKlass
        // Arc outlives every active interpreter frame.
        let constants = unsafe {
            std::slice::from_raw_parts(self.constants_ptr, self.constants_len)
        };
        let line_number = u16::try_from(self.instructionPc)
            .ok()
            .and_then(|bci| self.code.lineNumber(constants, bci))
            .map_or(-1, i32::from);
        crate::share::prims::stackwalk::StackFrameInfo::new(
            Symbol::new(self.owner),
            Symbol::new(self.code.name),
            Symbol::new(self.code.descriptor),
            self.instructionPc as i32,
            line_number,
            false,
        )
    }

    pub(crate) fn threadId(&self) -> u64 {
        self.threadId
    }

    pub(crate) fn site(&self) -> String {
        format!(
            "{} {}.{}@{}",
            String::from_utf8_lossy(self.owner),
            String::from_utf8_lossy(self.code.name),
            String::from_utf8_lossy(self.code.descriptor),
            self.pc
        )
    }

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
        let value = self.stack.pop().ok_or_else(|| {
            Error::other(format!(
                "Operand stack underflow at pc {} in {}.{}{}",
                self.pc,
                String::from_utf8_lossy(self.owner),
                String::from_utf8_lossy(self.code.name),
                String::from_utf8_lossy(self.code.descriptor)
            ))
        })?;
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
                String::from_utf8_lossy(self.owner)
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
        let target = pc
            .checked_add_signed(offset as isize)
            .filter(|target| *target < self.code.bytes.len())
            .ok_or_else(|| Error::other("Invalid branch target"))?;
        // Backward branch = loop backedge; feed the tiered-compilation
        // counters exactly like HotSpot's backward-branch dispatch does.
        if target < self.pc && !interpreterFlags().noJit && !jit_skip(self.owner, &self.code)
        {
            let key = self.code.bytes.as_ptr() as usize;
            if crate::share::oops::methodCounters::backedge_tick(key) {
                if interpreterFlags().traceCompile {
                    eprintln!(
                        "[compile-request:backedge] {}.{}{}",
                        String::from_utf8_lossy(self.owner),
                        String::from_utf8_lossy(self.code.name),
                        String::from_utf8_lossy(self.code.descriptor),
                    );
                }
                // Hot loop backedge: request an OSR compile (and possibly a
                // regular recompile) through the broker.  `enclosing` is
                // None — we do not yet track the innermost compiled frame.
                let flags = crate::share::compiler::compileBroker::MethodCompileFlags {
                    is_static_initializer: self.code.name == b"<clinit>",
                    ..Default::default()
                };
                crate::share::compiler::compileBroker::CompileBroker::global()
                    .backedge_event(
                        crate::share::oops::symbol::Symbol::new(self.owner),
                        crate::share::oops::symbol::Symbol::new(self.code.name),
                        crate::share::oops::symbol::Symbol::new(self.code.descriptor),
                        target as i32,
                        Some(key),
                        flags,
                        false,
                        None,
                        crate::share::compiler::compilerDefinitions::CompLevel::Simple,
                    );
            }
        }
        self.pc = target;
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
            return Err(Error::other(format!(
                "Invalid local type at pc {} in {}.{}{} (local {} kind {})",
                self.pc,
                String::from_utf8_lossy(self.owner),
                String::from_utf8_lossy(self.code.name),
                String::from_utf8_lossy(self.code.descriptor),
                index,
                kind
            )));
        }
        Ok(value)
    }

    fn storeOperand(&mut self, index: u16, kind: u8) -> io::Result<()> {
        let value = self.pop()?;
        if !value.matchesLocal(kind) {
            return Err(Error::other(format!(
                "Invalid local store type at pc {} in {}.{}{} (local {} kind {}) value={value:?}",
                self.pc,
                String::from_utf8_lossy(self.owner),
                String::from_utf8_lossy(self.code.name),
                String::from_utf8_lossy(self.code.descriptor),
                index,
                kind
            )));
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
                    } else if let Value::Object(handle) = exc {
                        let objects = loader.borrow_objects();
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
                    if interpreterFlags().traceExceptions
                        && let Value::Object(handle) = exc
                    {
                        eprintln!(
                            "[exc-caught] {}.{}{} pc={pc} handler={} exception={}",
                            String::from_utf8_lossy(klass.class.name),
                            String::from_utf8_lossy(self.code.name),
                            String::from_utf8_lossy(self.code.descriptor),
                            entry.handler_pc,
                            throwableDescription(loader, handle),
                        );
                    }
                    self.stack.clear();
                    self.stackSlots = exc.slots();
                    self.stack.push(exc);
                    self.pc = entry.handler_pc;
                    return true;
                }
                if interpreterFlags().traceExceptions {
                    let exc_name = match exc {
                        Value::Object(h) => loader
                            .borrow_objects()
                            .get(h)
                            .map(|o| dotted(&o.klass.class.name))
                            .unwrap_or_else(|| "?".into()),
                        _ => format!("{exc:?}"),
                    };
                    eprintln!(
                        "[exc-nomatch] {}.{}{} pc={} range={}..{} catch_type={} exc={}",
                        dotted(&klass.class.name),
                        String::from_utf8_lossy(self.code.name),
                        String::from_utf8_lossy(self.code.descriptor),
                        pc,
                        entry.start_pc,
                        entry.end_pc,
                        entry.catch_type,
                        exc_name,
                    );
                }
            }
        }
        if interpreterFlags().traceExceptions
            && !self.code.exception_table.is_empty()
        {
            eprintln!(
                "[exc-noframe] {} pc={} table={:?}",
                dotted(&klass.class.name),
                pc,
                self.code
                    .exception_table
                    .iter()
                    .map(|e| (e.start_pc, e.end_pc, e.handler_pc, e.catch_type))
                    .collect::<Vec<_>>()
            );
        }
        false
    }
}

fn javaFloat(value: f32) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f32::INFINITY {
        "Infinity".to_owned()
    } else if value == f32::NEG_INFINITY {
        "-Infinity".to_owned()
    } else {
        let text = value.to_string();
        if text.contains(['.', 'e', 'E']) {
            text
        } else {
            format!("{text}.0")
        }
    }
}

fn javaDouble(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "Infinity".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else {
        let text = value.to_string();
        if text.contains(['.', 'e', 'E']) {
            text
        } else {
            format!("{text}.0")
        }
    }
}

/// Renders a stack value the way `String.valueOf` would during string
/// concatenation: strings as text, objects via their `toString()`.
pub(crate) fn describeValue<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<String> {
    describeValueKind(loader, value, false, output, depth)
}

fn describeValueKind<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    as_boolean: bool,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<String> {
    match value {
        Value::Int(value) if as_boolean => Ok(if value != 0 { "true" } else { "false" }.to_owned()),
        Value::Int(value) => Ok(value.to_string()),
        Value::Long(value) => Ok(value.to_string()),
        Value::Float(value) => Ok(javaFloat(value)),
        Value::Double(value) => Ok(javaDouble(value)),
        Value::String(bytes) => decodeString(bytes),
        Value::HostString(text) => Ok(text.to_owned()),
        Value::Null => Ok("null".to_owned()),
        Value::Object(handle) => {
            if let Some(text) = crate::share::prims::stringNatives::string(loader, value)? {
                return Ok(text.into_owned());
            }
            let klass = {
                let objects = loader.borrow_objects();
                objects.get(handle).map(|object| Arc::clone(&object.klass))
            };
            let Some(klass) = klass else {
                return Ok(format!("Object@{handle:x}"));
            };
            match LinkResolver::resolve_virtual_call(&klass, b"toString", b"()Ljava/lang/String;") {
                Ok((target, code)) => {
                    let flags =
                        methodAccessFlags(&target, b"toString", b"()Ljava/lang/String;", false);
                    match execute(loader, &target, code, &[value], output, depth + 1, flags) {
                        Ok(Some(result)) => describeValue(loader, result, output, depth + 1),
                        Ok(None) => Ok("null".to_owned()),
                        Err(error) => {
                            if interpreterFlags().trace {
                                eprintln!(
                                    "[describeValue] toString failed for {}: {error}",
                                    dotted(klass.class.name)
                                );
                            }
                            Ok(format!("Object@{handle:x}"))
                        }
                    }
                }
                Err(error) => {
                    if interpreterFlags().trace {
                        eprintln!(
                            "[describeValue] resolve toString failed for {}: {error}",
                            dotted(klass.class.name)
                        );
                    }
                    Ok(format!("Object@{handle:x}"))
                }
            }
        }
        _ => Ok(String::new()),
    }
}

pub(crate) fn initialize<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<()> {
    let _rg = crate::share::recprobe::Guard::enter("initialize");
    let tid = crate::share::runtime::threading::current_tid(loader);
    match klass.state.get() {
        InitState::FullyInitialized => return Ok(()),
        InitState::InitializationError => {
            return Err(jex(
                loader,
                output,
                depth,
                b"java/lang/NoClassDefFoundError",
                Some(&format!(
                    "Could not initialize class {}",
                    dotted(klass.class.name)
                )),
            ));
        }
        _ => {}
    }
    // JVMS §12.4.2: exactly one thread runs <clinit>; concurrent requests
    // block on the class's init gate until the state leaves
    // BeingInitialized; a recursive request by the initializing thread
    // returns immediately.
    let (init_lock, init_cond) = &*klass.init_gate;
    {
        let mut owner = init_lock.lock().unwrap();
        loop {
            match klass.state.get() {
                InitState::FullyInitialized => return Ok(()),
                InitState::InitializationError => {
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/NoClassDefFoundError",
                        Some(&format!(
                            "Could not initialize class {}",
                            dotted(klass.class.name)
                        )),
                    ));
                }
                _ => {}
            }
            if *owner == Some(tid) {
                return Ok(());
            }
            if owner.is_none() {
                *owner = Some(tid);
                break;
            }
            if interpreterFlags().traceBlocking {
                eprintln!("[block] init-wait tid={tid} class={}", dotted(klass.class.name));
            }
            owner = init_cond.wait(owner).unwrap();
        }
    }
    klass.state.set(InitState::BeingInitialized);
    let result = (|| {
        if (klass.class.accessFlags & 0x0200) == 0 {
            if let Some(superKlass) = &klass.super_klass {
                initialize(loader, superKlass, output, depth + 1)?;
            }
            // JVMS §5.5 step 7: initializing a class also initializes every
            // direct superinterface that declares a non-static concrete
            // method (default methods).
            for iface in klass.default_method_superinterfaces() {
                initialize(loader, &iface, output, depth + 1)?;
            }
        }
        let code = klass.initializer().map_err(|error| {
            Error::other(format!(
                "{}: {error}",
                String::from_utf8_lossy(klass.class.name)
            ))
        })?;
        if let Some(code) = code
            && execute(loader, klass, code, &[], output, depth, 0)?.is_some()
        {
            return Err(Error::other("Class initializer must return void"));
        }
        Ok(())
    })();
    if let Err(error) = &result
        && interpreterFlags().trace
    {
        let detail = throwable_of(error)
            .map(|handle| throwableDescription(loader, handle))
            .unwrap_or_else(|| format!("{error}"));
        eprintln!(
            "[clinit-fail] {}: {}",
            String::from_utf8_lossy(klass.class.name),
            detail
        );
    }
    klass.state.set(if result.is_ok() {
        InitState::FullyInitialized
    } else {
        InitState::InitializationError
    });
    *init_lock.lock().unwrap() = None;
    init_cond.notify_all();
    result
}

pub fn execute<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    code: Code<'a>,
    arguments: &[Value<'a>],
    output: &mut dyn Write,
    depth: usize,
    access_flags: u16,
) -> io::Result<Option<Value<'a>>> {
    execute_labeled(
        loader,
        klass,
        code,
        arguments,
        output,
        depth,
        access_flags,
        None,
    )
}

pub fn execute_labeled<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    code: Code<'a>,
    arguments: &[Value<'a>],
    output: &mut dyn Write,
    depth: usize,
    access_flags: u16,
    method: Option<(&'a [u8], &'a [u8])>,
) -> io::Result<Option<Value<'a>>> {
    // `owner` is the declaring class's internal name; method name and
    // descriptor are already available through `code`, and `stackWalkInfo`
    // relies on `owner` being a real class name.
    let owner = klass.class.name;
    let _ = method;
    // JDK intrinsics must be honored on every entry path, not only when
    // the caller is interpreted: JIT-compiled callers reach this function
    // through their call stubs and would otherwise run intrinsified
    // methods (SHA block compression, UTF16 string ops, ...) in the
    // bytecode interpreter.
    if access_flags & 0x0008 != 0 {
        if let Some(value) = crate::share::prims::jdkIntrinsics::invoke(
            loader,
            owner,
            code.name,
            code.descriptor,
            arguments,
        )? {
            return Ok(Some(value));
        }
    } else {
        if crate::share::prims::jdkIntrinsics::invokeVoidInstance(
            loader,
            owner,
            code.name,
            code.descriptor,
            arguments,
        ) {
            return Ok(None);
        }
        if let Some(value) = crate::share::prims::jdkIntrinsics::invokeInstance(
            loader,
            klass,
            code.name,
            code.descriptor,
            arguments,
        ) {
            return Ok(Some(value));
        }
    }
    // Tiered compilation: every interpreted entry bumps the method's
    // invocation counter; crossing the threshold flags it for the JIT.
    let code_key = code.bytes.as_ptr() as usize;
    // Executable tier: once the broker has installed an nmethod for
    // this method, subsequent invocations run the compiled code
    // directly.  The entry point is published two ways: the
    // method-counters dispatch key (set by the broker at install) and
    // the CodeCache method index itself.  `try_invoke` returns None
    // for anything outside the executable subset (unsupported
    // signature, arity mismatch), in which case we fall through to
    // the interpreter unchanged; `Deoptimize` retires the nmethod and
    // resumes interpretation, and `Thrown` propagates a Java
    // throwable raised inside compiled code.
    let no_jit = interpreterFlags().noJit;
    let mut compiled_addr = if no_jit {
        0
    } else {
        crate::share::oops::methodCounters::compiled_addr(code_key)
    };
    if !no_jit && compiled_addr == 0 && crate::share::oops::methodCounters::is_hot(code_key) {
        // Hot method without a published dispatch entry: consult the
        // CodeCache nmethod index by method key (HotSpot's
        // `nmethod::entry_point` lookup on the Method).
        let holder = String::from_utf8_lossy(owner);
        let name = String::from_utf8_lossy(code.name);
        let sig = String::from_utf8_lossy(code.descriptor);
        if let Some(nm) = crate::share::code::codeCache::CodeCache::global()
            .find_nmethod(holder.as_ref(), name.as_ref(), sig.as_ref())
            && nm.is_in_use()
            && !nm.is_osr_method()
            && nm.entry_point() != 0
        {
            compiled_addr = nm.entry_point();
        }
    }
    if compiled_addr != 0 {
        // The published dispatch entry can outlive its nmethod
        // (deoptimization, sweeping): validate before calling into it.
        let live = crate::share::code::codeCache::CodeCache::global()
            .isLiveEntry(compiled_addr);
        if !live {
            crate::share::oops::methodCounters::clear_compiled(code_key);
            compiled_addr = 0;
        }
    }
    if compiled_addr != 0
        && access_flags & (0x0100 | 0x0400 | 0x0020) == 0
    {
        match crate::share::jitruntime::try_invoke(
            compiled_addr,
            code.descriptor,
            access_flags & 0x0008 != 0,
            arguments,
            depth,
        ) {
            Some(crate::share::jitruntime::CompiledResult::Value(value)) => {
                return Ok(value);
            }
            Some(crate::share::jitruntime::CompiledResult::Thrown(handle)) => {
                if interpreterFlags().traceExceptions {
                    eprintln!(
                        "[jit-throw] {}.{}{} handle={handle}",
                        String::from_utf8_lossy(owner),
                        String::from_utf8_lossy(code.name),
                        String::from_utf8_lossy(code.descriptor)
                    );
                }
                if code.exception_table.is_empty() {
                    return Err(Error::other(JavaThrowable(handle)));
                }
            }
            Some(crate::share::jitruntime::CompiledResult::Deoptimize) => {
                // Uncommon trap / unsupported stub: retire the
                // nmethod (HotSpot `make_not_entrant`) and resume in
                // the interpreter with intact state.
                if let Some(nm) = crate::share::code::codeCache::CodeCache::global()
                    .find_nmethod_at_address(compiled_addr)
                {
                    nm.make_not_entrant();
                }
                crate::share::oops::methodCounters::clear_compiled(code_key);
            }
            None => {}
        }
    }
    if !no_jit
        && !jit_skip(owner, &code)
        && crate::share::oops::methodCounters::invocation_tick(code_key)
    {
        if interpreterFlags().traceCompile {
            eprintln!(
                "[compile-request:invoke] {}.{}{}",
                String::from_utf8_lossy(owner),
                String::from_utf8_lossy(code.name),
                String::from_utf8_lossy(code.descriptor),
            );
        }
        // Tiered compilation: feed the invocation event to the broker so a
        // hot method is queued for C1/C2.  Compilation itself runs on a
        // compiler thread; the interpreter keeps executing until an
        // nmethod is installed.
        let flags = crate::share::compiler::compileBroker::MethodCompileFlags {
            is_native: access_flags & 0x0100 != 0,
            is_abstract: access_flags & 0x0400 != 0,
            is_static_initializer: code.name == b"<clinit>",
            ..Default::default()
        };
        crate::share::compiler::compileBroker::CompileBroker::global()
            .compile_if_required(
                crate::share::oops::symbol::Symbol::new(owner),
                crate::share::oops::symbol::Symbol::new(code.name),
                crate::share::oops::symbol::Symbol::new(code.descriptor),
                Some(code_key),
                flags,
                false,
            );
    }
    loader.call_stack.push(klass.class.name);
    let monitor = if access_flags & 0x0020 != 0 {
        let target = if access_flags & 0x0008 != 0 {
            Some(Value::ClassMirror(klass.class.name))
        } else {
            arguments.first().copied()
        };
        match target {
            Some(value) => match monitorEnter(loader, output, depth, value) {
                Ok(_) => Some(value),
                Err(error) => {
                    loader.call_stack.pop();
                    return Err(error);
                }
            },
            None => None,
        }
    } else {
        None
    };
    let result = executeFrame(
        loader,
        klass,
        code,
        arguments,
        output,
        depth,
        owner,
        access_flags,
    );
    if let Some(value) = monitor {
        let _ = monitorExit(loader, output, depth, value);
    }
    loader.call_stack.pop();
    if depth == 0
        && let Err(error) = &result
        && let Some(handle) = throwable_of(error)
    {
        if interpreterFlags().trace {
            let stack: Vec<String> = loader
                .call_stack
                .snapshot()
                .iter()
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect();
            eprintln!(
                "[uncaught] {} stack={stack:?}",
                throwableDescription(loader, handle)
            );
        }
        return Err(Error::other(JavaFailure {
            description: throwableDescription(loader, handle),
            throwable: handle,
        }));
    }
    result
}

/// Renders an uncaught throwable as `fully.qualified.Name: message`.
pub(crate) fn throwableDescription(loader: &ClassLoader<'_>, handle: usize) -> String {
    let objects = loader.borrow_objects();
    let Some(object) = objects.get(handle) else {
        return "Uncaught exception".to_owned();
    };
    let name = dotted(object.klass.class.name);
    let message = object
        .klass
        .instanceField(b"detailMessage", b"Ljava/lang/String;")
        .ok()
        .and_then(|index| object.fields.get(index).copied())
        .and_then(|value| match value {
            Value::String(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            Value::HostString(text) => Some(text.to_owned()),
            _ => None,
        })
        .filter(|text| !text.is_empty());
    match message {
        Some(message) => format!("{name}: {message}"),
        None => name,
    }
}

fn executeFrame<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    code: Code<'a>,
    arguments: &[Value<'a>],
    output: &mut dyn Write,
    depth: usize,
    owner: &'a [u8],
    access_flags: u16,
) -> io::Result<Option<Value<'a>>> {
    FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Real JVMs throw StackOverflowError from a stack-pointer guard, not a
    // fixed frame count: the interpreter recurses in Rust, so threads that
    // declared their OS stack bounds are checked against the actual limit.
    // Threads without bounds (JNI-attached natives) keep a conservative
    // depth fallback.
    if crate::share::runtime::threading::stack_near_overflow()
        || (!crate::share::runtime::threading::stack_bounds_known() && depth >= 2048)
    {
        if interpreterFlags().traceStackOverflow {
            let stack = loader.call_stack.snapshot();
            let start = stack.len().saturating_sub(200);
            eprintln!(
                "[soe] depth={depth} frames={} tail={:?}",
                stack.len(),
                stack[start..]
                    .iter()
                    .map(|name| String::from_utf8_lossy(name).into_owned())
                    .collect::<Vec<_>>()
            );
        }
        let errorClass = loader.load(b"java/lang/StackOverflowError")?;
        let error = loader.alloc_object(InstanceOop::new(errorClass)?);
        return Err(Error::other(JavaThrowable(error)));
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
        instructionPc: 0,
        threadId: crate::share::runtime::threading::current_tid(loader),
        locals,
        owner,
        monitors: Vec::new(),
        constants_ptr: klass.class.constants.as_ptr(),
        constants_len: klass.class.constants.len(),
    };
    let _registration = FrameRegistration::new(&frame);
    let mut safepoint_tick = 0u32;
    let dumpEnabled = interpreterFlags().dump;
    loop {
        safepoint_tick = safepoint_tick.wrapping_add(1);
        let bytecodeCount = BYTECODE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if safepoint_tick & 0x3ff == 0 {
            crate::share::runtime::threading::schedule(loader, output, depth)?;
            crate::share::memory::heap::safepoint();
        }
        if dumpEnabled && bytecodeCount & 0x3ff == 0 {
            sampleThreadFrames(bytecodeCount);
        }
        if loader.gc_due() {
            loader.reset_allocations();
            let _ = crate::share::memory::heap::collect(loader);
            if interpreterFlags().memstat {
                static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                if TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 200 == 0 {
                    eprintln!(
                        "[memstat] objects={} arrays={} obj_arrays={} bump={}MB classes={}",
                        loader.borrow_objects().len(),
                        loader.arrays.borrow().len(),
                        loader.obj_arrays.borrow().len(),
                        loader.storage.allocated_bytes() / (1024 * 1024),
                        loader.classes.borrow().len(),
                    );
                }
            }
        }
        let insn_pc = frame.pc;
        frame.instructionPc = insn_pc;
        match step(loader, klass, &mut frame, output, depth) {
            Ok(Step::Push(value)) => {
                if let Err(error) = frame.push(value) {
                    release_frame_monitors(loader, &frame);
                    return Err(error);
                }
            }
            Ok(Step::Continue) => {}
            Ok(Step::Return(value)) => {
                return Ok(value);
            }
            Err(error) => match throwable_of(&error) {
                Some(handle) => {
                    let exc = Value::Object(handle);
                    if !frame.handle_exception(insn_pc, exc, loader, klass) {
                        if interpreterFlags().trace {
                            let mut frames = Vec::new();
                            for_each_active_frame(&mut |f| frames.push(f.site()));
                            eprintln!("[uncaught-in-frame] stack={frames:?}");
                        }
                        release_frame_monitors(loader, &frame);
                        return Err(error);
                    }
                }
                None => {
                    let error = java_error_throwable(loader, output, depth, error);
                    if throwable_of(&error).is_some() {
                        let handle = throwable_of(&error).unwrap();
                        let exc = Value::Object(handle);
                        if frame.handle_exception(insn_pc, exc, loader, klass) {
                            continue;
                        }
                    }
                    if interpreterFlags().trace {
                        let mut frames = Vec::new();
                        for_each_active_frame(&mut |f| frames.push(f.site()));
                        eprintln!("[fatal] {error} stack={frames:?}");
                    }
                    release_frame_monitors(loader, &frame);
                    return Err(error);
                }
            },
        }
    }
}

enum Step<'a> {
    Push(Value<'a>),
    Continue,
    Return(Option<Value<'a>>),
}

fn dotted(name: &[u8]) -> String {
    String::from_utf8_lossy(name).replace('/', ".")
}

/// Allocates a real `java.lang.Throwable` subclass instance, runs its
/// `<init>(String)` constructor when available, and returns the
/// object-arena handle. The object is kept alive through
/// `gc_extra_roots` while the constructor executes.
pub(crate) fn throwableObject<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    class_name: &[u8],
    message: Option<&str>,
) -> io::Result<usize> {
    if interpreterFlags().trace {
        eprintln!(
            "[npe] creating {} msg={:?} in {}",
            dotted(class_name),
            message,
            loader
                .call_stack
                .snapshot()
                .iter()
                .rev()
                .take(8)
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect::<Vec<_>>()
                .join(" <- ")
        );
        eprintln!("{}", std::backtrace::Backtrace::force_capture());
    }
    let klass = loader.load(class_name)?;
    let _ = initialize(loader, &klass, &mut io::sink(), 0);
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(handle));

    let message_bytes: Option<&[u8]> =
        message.map(|text| &*loader.storage.alloc_slice_copy(text.as_bytes()));
    let init_result = (|| -> io::Result<()> {
        let member = klass.class.methods.iter().find(|method| {
            method.name == b"<init>"
                && method.descriptor == b"(Ljava/lang/String;)V"
                && method.accessFlags & 0x0008 == 0
        });
        if let Some(member) = member {
            let code = Code::parse(member)?;
            let argument = match message_bytes {
                Some(bytes) => Value::String(bytes),
                None => Value::Null,
            };
            execute(
                loader,
                &klass,
                code,
                &[Value::Object(handle), argument],
                output,
                depth + 1,
                member.accessFlags,
            )?;
        }
        Ok(())
    })();
    let mut roots = loader.gc_extra_roots.borrow_mut();
    if let Some(position) = roots
        .iter()
        .rposition(|value| matches!(value, Value::Object(root) if *root == handle))
    {
        roots.remove(position);
    }
    drop(roots);
    let _ = init_result;

    if let (Some(bytes), Ok(index)) = (
        message_bytes,
        klass.instanceField(b"detailMessage", b"Ljava/lang/String;"),
    ) {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle)
            && matches!(object.fields[index], Value::Null)
        {
            object.fields[index] = Value::String(bytes);
        }
    }
    Ok(handle)
}

/// Some natives report Java exceptions as plain `io::Error` strings in
/// the form `java.io.IOException: message` (or `java/io/IOException: ...`).
/// Convert those into real throwable objects so `catch` clauses can
/// match them; anything else is returned unchanged.
fn java_error_throwable<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    error: Error,
) -> Error {
    if throwable_of(&error).is_some() {
        return error;
    }
    let text = error.to_string();
    let Some((class, message)) = text.split_once(": ") else {
        return error;
    };
    let internal = if class.contains('/') {
        class.to_owned()
    } else {
        class.replace('.', "/")
    };
    let looks_like_class = ["java/", "javax/", "jdk/", "sun/"]
        .iter()
        .any(|prefix| internal.starts_with(prefix))
        && internal
            .rsplit('/')
            .next()
            .is_some_and(|simple| {
                simple.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            });
    if !looks_like_class {
        return error;
    }
    jex(loader, output, depth, internal.as_bytes(), Some(message))
}

/// Builds an `io::Error` carrying a real throwable object for `class_name`.
/// Falls back to a plain linkage-style error when the class cannot be
/// loaded or instantiated (e.g. minimal bootstrap environments).
pub(crate) fn jex<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    class_name: &[u8],
    message: Option<&str>,
) -> Error {
    if interpreterFlags().trace {
        let mut frames = Vec::new();
        for_each_active_frame(&mut |frame| frames.push(frame.site()));
        eprintln!(
            "[jex] {} msg={:?} stack={frames:?}",
            dotted(class_name),
            message
        );
    }
    match throwableObject(loader, output, depth, class_name, message) {
        Ok(handle) => Error::other(JavaThrowable(handle)),
        Err(_) => {
            let text = dotted(class_name);
            match message {
                Some(message) => Error::other(format!("{text}: {message}")),
                None => Error::other(text),
            }
        }
    }
}

/// Converts resolution/verification failures into the corresponding
/// `java.lang.Error` throwable when the failure is a Java-level linkage
/// error, preserving fatal VM errors otherwise.
fn linkageExc<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    error: Error,
) -> Error {
    if throwable_of(&error).is_some() {
        return error;
    }
    use crate::share::classfile::linkageError::LinkageErrorKind;
    let class_name: &[u8] = if let Some(kind) =
        crate::share::classfile::linkageError::linkage_kind(&error)
    {
        match kind {
            LinkageErrorKind::ClassNotFound => b"java/lang/ClassNotFoundException",
            LinkageErrorKind::NoClassDefFound => b"java/lang/NoClassDefFoundError",
            LinkageErrorKind::ClassFormat => b"java/lang/ClassFormatError",
            LinkageErrorKind::UnsupportedClassVersion => b"java/lang/UnsupportedClassVersionError",
            LinkageErrorKind::Verify => b"java/lang/VerifyError",
            LinkageErrorKind::ClassCircularity => b"java/lang/ClassCircularityError",
            LinkageErrorKind::IncompatibleClassChange => b"java/lang/IncompatibleClassChangeError",
            LinkageErrorKind::NoSuchField => b"java/lang/NoSuchFieldError",
            LinkageErrorKind::NoSuchMethod => b"java/lang/NoSuchMethodError",
            LinkageErrorKind::IllegalAccess => b"java/lang/IllegalAccessError",
            LinkageErrorKind::Instantiation => b"java/lang/InstantiationError",
            LinkageErrorKind::ExceptionInInitializer => b"java/lang/ExceptionInInitializerError",
            LinkageErrorKind::UnsatisfiedLink => b"java/lang/UnsatisfiedLinkError",
        }
    } else {
        let message = error.to_string();
        let prefix = message.split(':').next().unwrap_or("").trim();
        match prefix {
            "NoSuchMethodError" => b"java/lang/NoSuchMethodError",
            "NoSuchFieldError" => b"java/lang/NoSuchFieldError",
            "IncompatibleClassChangeError" => b"java/lang/IncompatibleClassChangeError",
            "InstantiationError" => b"java/lang/InstantiationError",
            "IllegalAccessError" => b"java/lang/IllegalAccessError",
            "UnsatisfiedLinkError" => b"java/lang/UnsatisfiedLinkError",
            "BootstrapMethodError" => b"java/lang/BootstrapMethodError",
            "AbstractMethodError" => b"java/lang/AbstractMethodError",
            "ClassCircularityError" => b"java/lang/ClassCircularityError",
            "ExceptionInInitializerError" => b"java/lang/ExceptionInInitializerError",
            _ if message.contains("native method is not linked") => b"java/lang/UnsatisfiedLinkError",
            _ if message.contains("NoSuchMethod") || message.contains("Missing method") => {
                b"java/lang/NoSuchMethodError"
            }
            _ if message.contains("AbstractMethodError") => b"java/lang/AbstractMethodError",
            _ if message.contains("Missing static method") => b"java/lang/NoSuchMethodError",
            _ if message.contains("Missing constructor") => b"java/lang/NoSuchMethodError",
            _ if message.contains("Missing field") => b"java/lang/NoSuchFieldError",
            _ if message.contains("does not implement interface")
                || message.contains("Conflicting interface") =>
            {
                b"java/lang/IncompatibleClassChangeError"
            }
            _ => return error,
        }
    };
    jex(loader, output, depth, class_name, Some(&error.to_string()))
}

fn is_print_stream(loader: &ClassLoader<'_>, value: Value<'_>) -> bool {
    match value {
        Value::Output => true,
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            objects.get(handle).is_some_and(|object| {
                let mut current = Some(object.klass.as_ref());
                while let Some(klass) = current {
                    if klass.class.name == b"java/io/PrintStream" {
                        return true;
                    }
                    current = klass.super_klass.as_deref();
                }
                false
            })
        }
        _ => false,
    }
}

pub(crate) fn monitorKey(value: Value<'_>) -> Option<u64> {
    const OBJ_ARRAY: u64 = 1 << 62;
    const PRIM_ARRAY: u64 = 2 << 62;
    const OTHER: u64 = 3 << 62;
    match value {
        Value::Object(handle) => Some(handle as u64),
        Value::ObjectArray(handle) => Some(OBJ_ARRAY | handle as u64),
        Value::PrimitiveArray(handle) => Some(PRIM_ARRAY | handle as u64),
        Value::Output => Some(u64::MAX),
        Value::String(bytes) | Value::ClassMirror(bytes) => {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            Some(OTHER | (hasher.finish() & !(3 << 62)))
        }
        Value::HostString(text) => {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            Some(OTHER | (hasher.finish() & !(3 << 62)))
        }
        Value::Arguments(arguments) => {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (arguments.as_ptr() as usize, arguments.len()).hash(&mut hasher);
            Some(OTHER | (hasher.finish() & !(3 << 62)))
        }
        _ => None,
    }
}

pub(crate) fn monitorEnter<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    value: Value<'a>,
) -> io::Result<u64> {
    if matches!(value, Value::Null) {
        return Err(jex(
            loader,
            output,
            depth,
            b"java/lang/NullPointerException",
            None,
        ));
    }
    let Some(key) = monitorKey(value) else {
        return Err(Error::other("Expected reference for monitor"));
    };
    // Fast path first; on contention the monitor's condition variable
    // blocks this OS thread until the owner releases it.
    let acquired = {
        loader.synchronizer.borrow_mut().try_enter(
            key as usize,
            crate::share::runtime::threading::current_tid(loader),
        )
    };
    if !acquired {
        let monitor = loader.synchronizer.borrow_mut().monitor(key as usize);
        crate::share::memory::heap::quiesce();
        monitor.enter_blocking(crate::share::runtime::threading::current_tid(loader));
        crate::share::memory::heap::unquiesce();
    }
    Ok(key)
}

pub(crate) fn monitorExit<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    value: Value<'a>,
) -> io::Result<()> {
    if matches!(value, Value::Null) {
        return Err(jex(
            loader,
            output,
            depth,
            b"java/lang/NullPointerException",
            None,
        ));
    }
    let Some(key) = monitorKey(value) else {
        return Err(Error::other("Expected reference for monitor"));
    };
    let exit_result = {
        loader
            .synchronizer
            .borrow_mut()
            .exit(
                key as usize,
                crate::share::runtime::threading::current_tid(loader),
            )
    };
    exit_result
        .map_err(|message| {
            jex(
                loader,
                output,
                depth,
                b"java/lang/IllegalMonitorStateException",
                Some(message),
            )
        })?;
    Ok(())
}

fn release_frame_monitors(loader: &ClassLoader<'_>, frame: &Frame<'_>) {
    if frame.monitors.is_empty() {
        return;
    }
    let mut synchronizer = loader.synchronizer.borrow_mut();
    for &key in &frame.monitors {
        let _ = synchronizer.exit(
            key as usize,
            crate::share::runtime::threading::current_tid(loader),
        );
    }
}

fn methodAccessFlags<'a>(
    klass: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
    is_static: bool,
) -> u16 {
    let mut current = Some(klass);
    while let Some(owner) = current {
        if let Some(method) = owner.class.methods.iter().find(|method| {
            method.name == name
                && method.descriptor == descriptor
                && (method.accessFlags & 0x0008 != 0) == is_static
        }) {
            return method.accessFlags;
        }
        current = owner.super_klass.as_deref();
    }
    0
}

/// Returns the cached resolution for an invoke* call site or resolves,
/// caches, and returns it. Only successful resolutions are cached;
/// failures re-resolve every time (matching uncached semantics). The
/// resolved method's access flags are cached alongside so callers skip
/// the `methodAccessFlags` hierarchy scan too.
pub(crate) fn cachedCall<'a>(
    loader: &ClassLoader<'a>,
    key: CallSiteKey,
    name: &[u8],
    descriptor: &[u8],
    resolve: impl FnOnce() -> io::Result<(Arc<InstanceKlass<'a>>, Code<'a>)>,
) -> io::Result<(Arc<InstanceKlass<'a>>, Code<'a>, u16)> {
    if let Some((klass, code, flags)) = loader.invoke_cache.borrow().get(&key) {
        return Ok((Arc::clone(klass), code.clone(), *flags));
    }
    let (klass, code) = resolve()?;
    let flags = methodAccessFlags(&klass, name, descriptor, key.is_static());
    loader
        .invoke_cache
        .borrow_mut()
        .insert(key, (Arc::clone(&klass), code.clone(), flags));
    Ok((klass, code, flags))
}

fn step<'a>(
    loader: &ClassLoader<'a>,
    klass: &InstanceKlass<'a>,
    frame: &mut Frame<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<Step<'a>> {
    let pc = frame.pc;
    let opcode = frame.byte()?;
    let value = match opcode {
        0x00 => return Ok(Step::Continue),                      // nop
        0x01 => Value::Null,                                    // aconst_null
        0x02..=0x08 => Value::Int(i32::from(opcode) - 3),       // iconst_m1..5
        0x09..=0x0a => Value::Long(i64::from(opcode - 9)),      // lconst_0..1
        0x0b..=0x0d => Value::Float(f32::from(opcode - 0x0b)),  // fconst_0..2
        0x0e..=0x0f => Value::Double(f64::from(opcode - 0x0e)), // dconst_0..1
        0x10 => Value::Int(i32::from(frame.byte()? as i8)),     // bipush
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
                (_, Constant::Dynamic(bsm_attr_idx, nat_idx)) => {
                    // condy: resolve through the real bootstrap method.
                    let Constant::NameAndType(name_idx, desc_idx) = klass.constant(*nat_idx)?
                    else {
                        return Err(Error::other("Expected NameAndType constant"));
                    };
                    let name = klass.utf8(*name_idx)?;
                    let desc = klass.utf8(*desc_idx)?;
                    super::genericBootstrap::resolve_condy(
                        loader,
                        klass,
                        u32::from(index),
                        *bsm_attr_idx,
                        name,
                        desc,
                        output,
                        depth + 1,
                    )?
                }
                _ => {
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/LinkageError",
                        Some("Unsupported ldc constant"),
                    ));
                }
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
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/NullPointerException",
                    None,
                ));
            };
            let load_result = {
                let arrays = loader.arrays.borrow();
                arrays
                    .get(handle)
                    .ok_or_else(|| Error::other("Invalid array reference"))
                    .and_then(|array| array.load(opcode - 0x2e, index))
            };
            let array_len = {
                let arrays = loader.arrays.borrow();
                arrays.get(handle).map(|array| array.len())
            };
            match load_result.map_err(|error| {
                jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/ArrayIndexOutOfBoundsException",
                    Some(&format!(
                        "{} (index {} len {:?} pc {} in {}.{})",
                        error,
                        index,
                        array_len,
                        pc,
                        String::from_utf8_lossy(klass.class.name),
                        String::from_utf8_lossy(frame.code.name)
                    )),
                )
            })? {
                Element::Int(value) => Value::Int(value),
                Element::Long(value) => Value::Long(value),
                Element::Float(value) => Value::Float(value),
                Element::Double(value) => Value::Double(value),
            }
        }
        0x32 => {
            let index = frame.pop()?.int()? as usize;
            match frame.pop()? {
                Value::Arguments(arguments) => {
                    Value::HostString(arguments.get(index).ok_or_else(|| {
                        jex(
                            loader,
                            output,
                            depth,
                            b"java/lang/ArrayIndexOutOfBoundsException",
                            Some(&format!("Index {index} out of bounds")),
                        )
                    })?)
                }
                Value::ObjectArray(handle) => {
                    let load_result = {
                        let arrays = loader.obj_arrays.borrow();
                        arrays
                            .get(handle)
                            .ok_or_else(|| Error::other("Invalid object array reference"))
                            .and_then(|array| array.get(index))
                    };
                    let array_len = {
                        let arrays = loader.obj_arrays.borrow();
                        arrays.get(handle).map(|array| array.len())
                    };
                    load_result.map_err(|error| {
                        jex(
                            loader,
                            output,
                            depth,
                            b"java/lang/ArrayIndexOutOfBoundsException",
                            Some(&format!(
                                "Index {index} out of bounds for length {array_len:?} in {}.{}",
                                String::from_utf8_lossy(klass.class.name),
                                String::from_utf8_lossy(frame.code.name)
                            )),
                        )
                    })?
                }
                _ => {
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/NullPointerException",
                        None,
                    ));
                }
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
            return Ok(Step::Continue);
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
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/NullPointerException",
                    None,
                ));
            };
            let store_result = {
                let mut arrays = loader.arrays.borrow_mut();
                arrays
                    .get_mut(handle)
                    .ok_or_else(|| Error::other("Invalid array reference"))
                    .and_then(|array| array.store(opcode - 0x4f, index, value))
            };
            store_result.map_err(|error| {
                jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/ArrayIndexOutOfBoundsException",
                    Some(&error.to_string()),
                )
            })?;
            return Ok(Step::Continue);
        }
        0x53 => {
            let value = frame.pop()?;
            let index = frame.pop()?.int()? as usize;
            let Value::ObjectArray(handle) = frame.pop()? else {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/NullPointerException",
                    None,
                ));
            };
            let component = {
                let arrays = loader.obj_arrays.borrow();
                let array = arrays
                    .get(handle)
                    .ok_or_else(|| Error::other("Invalid object array reference"))?;
                match array.get(index) {
                    Ok(_) => array.component,
                    Err(error) => {
                        drop(arrays);
                        return Err(jex(
                            loader,
                            output,
                            depth,
                            b"java/lang/ArrayIndexOutOfBoundsException",
                            Some(&error.to_string()),
                        ));
                    }
                }
            };
            if !loader.referenceAssignable(value, component)? {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/ArrayStoreException",
                    Some("array element type mismatch"),
                ));
            }
            loader
                .obj_arrays
                .borrow_mut()
                .get_mut(handle)
                .ok_or_else(|| Error::other("Invalid object array reference"))?
                .set(index, value)?;
            return Ok(Step::Continue);
        }
        0x57 => {
            if frame.pop()?.slots() != 1 {
                return Err(Error::other("Invalid pop operand"));
            }
            return Ok(Step::Continue);
        }
        0x58 => {
            if frame.pop()?.slots() == 1 && frame.pop()?.slots() != 1 {
                return Err(Error::other("Invalid pop2 operands"));
            }
            return Ok(Step::Continue);
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
            return Ok(Step::Continue);
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
            return Ok(Step::Continue);
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
            return Ok(Step::Continue);
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
            return Ok(Step::Continue);
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
            return Ok(Step::Continue);
        }
        0x5f => {
            let v1 = frame.pop()?;
            let v2 = frame.pop()?;
            if v1.slots() != 1 || v2.slots() != 1 {
                return Err(Error::other("Invalid swap operand size"));
            }
            frame.push(v1)?;
            frame.push(v2)?;
            return Ok(Step::Continue);
        }
        0x60 | 0x64 | 0x68 | 0x6c | 0x70 | 0x78 | 0x7a | 0x7c | 0x7e | 0x80 | 0x82 => {
            let right = frame.pop()?.int()?;
            let left = frame.pop()?.int()?;
            if matches!(opcode, 0x6c | 0x70) && right == 0 {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/ArithmeticException",
                    Some("/ by zero"),
                ));
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
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/ArithmeticException",
                        Some("/ by zero"),
                    ));
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
            return Ok(Step::Continue);
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
            return Ok(Step::Continue);
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
                (literal @ Value::String(_), object @ Value::Object(_))
                | (object @ Value::Object(_), literal @ Value::String(_)) => {
                    crate::share::prims::stringNatives::materialize(loader, literal, output, depth)?
                        == Some(object)
                }
                (Value::String(a), Value::String(b)) => a == b,
                (Value::ClassMirror(name), Value::Object(handle))
                | (Value::Object(handle), Value::ClassMirror(name)) => {
                    loader.class_object_names.borrow().get(&handle).copied() == Some(name)
                }
                (Value::ClassMirror(a), Value::ClassMirror(b)) => a == b,
                (Value::HostString(a), Value::HostString(b)) => a == b,
                (Value::Arguments(a), Value::Arguments(b)) => {
                    std::ptr::eq(a.as_ptr(), b.as_ptr()) && a.len() == b.len()
                }
                (Value::String(a), Value::HostString(b))
                | (Value::HostString(b), Value::String(a)) => a == b.as_bytes(),
                _ => false,
            };
            if eq == (opcode == 0xa5) {
                frame.jump(pc, i32::from(offset))?;
            }
            return Ok(Step::Continue);
        }
        0xa8 => {
            // jsr
            let offset = i16::from_be_bytes(frame.read()?);
            let return_address = frame.pc;
            frame.push(Value::ReturnAddress(return_address))?;
            frame.jump(pc, i32::from(offset))?;
            return Ok(Step::Continue);
        }
        0xa9 => {
            // ret
            let index = frame.byte()?;
            let val = *frame.local(u16::from(index))?;
            let Value::ReturnAddress(target_pc) = val else {
                return Err(Error::other("Expected return address in ret"));
            };
            frame.pc = target_pc;
            return Ok(Step::Continue);
        }
        0xaa | 0xab => {
            frame.switch(pc, opcode == 0xaa)?;
            return Ok(Step::Continue);
        }
        0xac..=0xb0 => {
            let value = frame.pop()?;
            if !value.matchesLocal(opcode - 0xac) {
                let variant = match value {
                    Value::Int(_) => "Int",
                    Value::Long(_) => "Long",
                    Value::Float(_) => "Float",
                    Value::Double(_) => "Double",
                    Value::String(_) => "String",
                    Value::ClassMirror(_) => "ClassMirror",
                    Value::HostString(_) => "HostString",
                    Value::Arguments(_) => "Arguments",
                    Value::Output => "Output",
                    Value::PrimitiveArray(_) => "PrimitiveArray",
                    Value::Object(_) => "Object",
                    Value::ObjectArray(_) => "ObjectArray",
                    Value::ReturnAddress(_) => "ReturnAddress",
                    Value::Null => "Null",
                    Value::Empty => "Empty",
                };
                let tail: String = frame
                    .code
                    .bytes
                    .iter()
                    .rev()
                    .take(24)
                    .rev()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                eprintln!(
                    "[ret-operand] owner={} method={} desc={} opcode={:#x} pc={} value={} tail={}",
                    String::from_utf8_lossy(frame.owner),
                    String::from_utf8_lossy(frame.code.name),
                    String::from_utf8_lossy(frame.code.descriptor),
                    opcode,
                    frame.pc,
                    variant,
                    tail
                );
                return Err(Error::other("Invalid return operand"));
            }
            return Ok(Step::Return(Some(value)));
        }
        0xb1 => return Ok(Step::Return(None)),
        0xb2 | 0xb3 => {
            let [owner, name, descriptor] =
                klass.memberRef(u16::from_be_bytes(frame.read()?), true)?;
            {
                if opcode == 0xb2
                    && owner == b"jdk/internal/misc/UnsafeConstants"
                    && let Some(value) =
                        crate::share::prims::r#unsafe::Unsafe::platformConstant(name, descriptor)
                {
                    frame.push(Value::Int(value))?;
                    return Ok(Step::Continue);
                }
                let target = loader
                    .load(owner)
                    .and_then(|klass| klass.staticFieldOwner(name, descriptor))
                    .map_err(|error| linkageExc(loader, output, depth, error))?;
                let field = target
                    .staticField(name, descriptor)
                    .map_err(|error| linkageExc(loader, output, depth, error))?;
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
                    return Ok(Step::Continue);
                }
                field.get()
            }
        }
        0xb4 | 0xb5 => {
            let [owner, name, descriptor] =
                klass.memberRef(u16::from_be_bytes(frame.read()?), true)?;
            let value = (opcode == 0xb5).then(|| frame.pop()).transpose()?;
            let mut receiver = frame.pop()?;
            // Lightweight string values have no field storage; materialize them
            // into real java.lang.String objects on field access.
            if matches!(receiver, Value::String(_) | Value::HostString(_))
                && let Some(materialized) = crate::share::prims::stringNatives::materialize(
                    loader,
                    receiver,
                    output,
                    depth + 1,
                )?
            {
                receiver = materialized;
            }
            // Class literals have no field storage; materialize them into
            // cached java.lang.Class objects so getfield/putfield work.
            if matches!(receiver, Value::ClassMirror(_))
                && let Some(materialized) =
                    crate::share::prims::classNatives::materialize_class(loader, receiver)?
            {
                receiver = materialized;
            }
            if matches!(receiver, Value::Null) {
                if interpreterFlags().trace {
                    eprintln!(
                        "[npe-site] field {}.{}{} pc={} in {} [{}]",
                        String::from_utf8_lossy(owner),
                        String::from_utf8_lossy(name),
                        String::from_utf8_lossy(descriptor),
                        pc,
                        String::from_utf8_lossy(klass.class.name),
                        String::from_utf8_lossy(frame.owner),
                    );
                }
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/NullPointerException",
                    None,
                ));
            }
            let Value::Object(handle) = receiver else {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/IncompatibleClassChangeError",
                    Some("Expected object field receiver"),
                ));
            };
            let field_result: io::Result<Option<Value>> = (|| {
                let mut objects = loader.borrow_objects_mut();
                let object = objects
                    .get_mut(handle)
                    .ok_or_else(|| Error::other("Invalid object reference"))?;
                if !SystemDictionary::is_subclass_of(&object.klass, owner) {
                    return Err(Error::other("Unsupported field receiver class"));
                }
                let ownerKlass = loader.load(owner)?;
                let index = ownerKlass.instanceField(name, descriptor)?;
                if index >= object.fields.len() {
                    return Err(Error::other("Invalid instance field slot"));
                }
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
                    return Ok(None);
                }
                if interpreterFlags().debugField
                    && name == b"value"
                    && matches!(object.fields.get(index), Some(Value::Null) | None)
                {
                    eprintln!(
                        "[field-null] {}.{}{} idx={} len={} handle={} klass={} pc={} in {}.{}",
                        String::from_utf8_lossy(owner),
                        String::from_utf8_lossy(name),
                        String::from_utf8_lossy(descriptor),
                        index,
                        object.fields.len(),
                        handle,
                        String::from_utf8_lossy(object.klass.class.name),
                        pc,
                        String::from_utf8_lossy(frame.owner),
                        String::from_utf8_lossy(frame.code.name),
                    );
                }
                Ok(Some(object.fields[index]))
            })();
            let value = field_result.map_err(|error| {
                if error.to_string() == "Unsupported field receiver class" {
                    return jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/IncompatibleClassChangeError",
                        Some("Field receiver class mismatch"),
                    );
                }
                linkageExc(loader, output, depth, error)
            })?;
            if let Some(value) = value {
                frame.push(value)?;
            }
            return Ok(Step::Continue);
        }
        0xb6..=0xb9 => {
            let index = u16::from_be_bytes(frame.read()?);
            let interfaceCount = if opcode == 0xb9 {
                let [count, zero] = frame.read()?;
                if zero != 0 || !matches!(klass.constant(index)?, Constant::InterfaceMethodRef(..))
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
                return Ok(Step::Continue);
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
                .ok_or_else(|| {
                    Error::other(format!(
                        "Missing method arguments at pc {} in {}.{}{} ({}.{}) stack={} need={}",
                        pc,
                        String::from_utf8_lossy(frame.owner),
                        String::from_utf8_lossy(frame.code.name),
                        String::from_utf8_lossy(frame.code.descriptor),
                        String::from_utf8_lossy(owner),
                        String::from_utf8_lossy(name),
                        frame.stack.len(),
                        count + receiverSlots,
                    ))
                })?;
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
                && let Some(value) = crate::share::prims::stringNatives::materialize(
                    loader,
                    frame.stack[start],
                    output,
                    depth + 1,
                )?
            {
                frame.stack[start] = value;
            }
            if matches!(opcode, 0xb6 | 0xb9)
                && owner.first() == Some(&b'[')
                && name == b"clone"
                && descriptor == b"()Ljava/lang/Object;"
            {
                let cloned = match frame.stack[start] {
                    Value::Null => {
                        return Err(jex(
                            loader,
                            output,
                            depth,
                            b"java/lang/NullPointerException",
                            None,
                        ));
                    }
                    Value::PrimitiveArray(handle) => {
                        let array = {
                            let arrays = loader.arrays.borrow();
                            arrays
                                .get(handle)
                                .ok_or_else(|| Error::other("Invalid array reference"))?
                                .clone()
                        };
                        Value::PrimitiveArray(loader.alloc_type_array(array))
                    }
                    Value::ObjectArray(handle) => {
                        let (component, elements) = {
                            let arrays = loader.obj_arrays.borrow();
                            let array = arrays
                                .get(handle)
                                .ok_or_else(|| Error::other("Invalid array reference"))?;
                            (array.component, array.elements.clone())
                        };
                        let mut array =
                            crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                                elements.len() as i32,
                                component,
                            )?;
                        array.elements = elements;
                        Value::ObjectArray(loader.alloc_obj_array(array))
                    }
                    Value::Arguments(arguments) => {
                        let mut array =
                            crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                                arguments.len() as i32,
                                b"java/lang/String",
                            )?;
                        array.elements = arguments
                            .iter()
                            .map(|arg| Value::HostString(arg.as_str()))
                            .collect();
                        Value::ObjectArray(loader.alloc_obj_array(array))
                    }
                    _ => return Err(Error::other("Expected array receiver for clone")),
                };
                frame.stack.truncate(start);
                frame.stackSlots -= slots;
                frame.push(cloned)?;
                return Ok(Step::Continue);
            }
            if matches!(opcode, 0xb6 | 0xb9)
                && let Some(value) = crate::share::prims::stringNatives::invokeVirtual(
                    loader,
                    owner,
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
                return Ok(Step::Continue);
            }
            if matches!(opcode, 0xb6 | 0xb7)
                && owner == b"java/lang/Object"
                && name == b"hashCode"
                && descriptor == b"()I"
            {
                let receiver = frame.stack[start];
                if matches!(receiver, Value::Null) {
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/NullPointerException",
                        None,
                    ));
                }
                // invokespecial must bypass overrides: identity hash always.
                let hash = if opcode == 0xb7 {
                    Some(crate::share::prims::objectNatives::identity(loader, receiver))
                } else {
                    crate::share::prims::objectNatives::hashCode(loader, receiver)?
                };
                if let Some(hash) = hash {
                    frame.pop()?;
                    frame.push(Value::Int(hash))?;
                    return Ok(Step::Continue);
                }
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
                return Ok(Step::Continue);
            }
            if opcode != 0xb8 {
                let __t = std::time::Instant::now();
                let result = crate::share::prims::nativeLookup::invokeInstance(
                    loader,
                    owner,
                    name,
                    descriptor,
                    &frame.stack[start..],
                    output,
                    depth,
                )?;
                NATIVE_NANOS.fetch_add(
                    __t.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                if let Some(result) = result {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    if let Some(value) = result {
                        frame.push(value)?;
                    }
                    return Ok(Step::Continue);
                }
            }

            if opcode == 0xb8 {
                if owner == b"java/lang/System"
                    && descriptor == b"(Ljava/lang/String;)V"
                    && matches!(name, b"load" | b"loadLibrary")
                {
                    let value = frame.pop()?;
                    let path = crate::share::prims::stringNatives::string(loader, value)?
                        .ok_or_else(|| Error::other("Expected library name"))?;
                    let libraries = crate::share::prims::nativeLibraries::NativeLibraries::global();
                    let (java_home, java_library_path) = {
                        let properties = loader.systemProperties.borrow();
                        (
                            properties.get("java.home").copied().map(str::to_owned),
                            properties
                                .get("java.library.path")
                                .copied()
                                .map(str::to_owned),
                        )
                    };
                    let mapped_name = if cfg!(windows) {
                        if path.ends_with(".dll") {
                            path.to_string()
                        } else {
                            format!("{path}.dll")
                        }
                    } else if cfg!(target_os = "macos") {
                        if path.starts_with("lib") && path.ends_with(".dylib") {
                            path.to_string()
                        } else {
                            format!("lib{path}.dylib")
                        }
                    } else if path.starts_with("lib") && path.ends_with(".so") {
                        path.to_string()
                    } else {
                        format!("lib{path}.so")
                    };
                    if name == b"load" {
                        let library_path = std::path::Path::new(path.as_ref());
                        let canonical = library_path
                            .canonicalize()
                            .unwrap_or_else(|_| library_path.to_path_buf());
                        let builtin = java_home.as_deref().is_some_and(|home| {
                            let home = std::path::Path::new(home);
                            ["lib", "bin"]
                                .iter()
                                .any(|sub| canonical.starts_with(home.join(sub)))
                        });
                        if !builtin {
                            if let Err(error) = libraries.load(library_path) {
                                return Err(jex(
                                    loader,
                                    output,
                                    depth,
                                    b"java/lang/UnsatisfiedLinkError",
                                    Some(&error.to_string()),
                                ));
                            }
                        }
                        return Ok(Step::Continue);
                    }
                    let builtin = java_home.as_deref().is_some_and(|home| {
                        ["lib", "bin"].iter().any(|sub| {
                            std::path::Path::new(home)
                                .join(sub)
                                .join(&mapped_name)
                                .is_file()
                        })
                    });
                    if builtin {
                        return Ok(Step::Continue);
                    }
                    let mut found = None;
                    if let Some(library_path) = java_library_path.as_deref() {
                        for dir in std::env::split_paths(library_path) {
                            let candidate = dir.join(&mapped_name);
                            if candidate.is_file() {
                                found = Some(candidate);
                                break;
                            }
                        }
                    }
                    match found {
                        Some(candidate) => {
                            if let Err(error) = libraries.load(&candidate) {
                                return Err(jex(
                                    loader,
                                    output,
                                    depth,
                                    b"java/lang/UnsatisfiedLinkError",
                                    Some(&error.to_string()),
                                ));
                            }
                        }
                        None => {
                            return Err(jex(
                                loader,
                                output,
                                depth,
                                b"java/lang/UnsatisfiedLinkError",
                                Some(&format!(
                                    "no {path} in java.library.path"
                                )),
                            ));
                        }
                    }
                    return Ok(Step::Continue);
                }
                if owner == b"java/lang/ClassLoader"
                    && name == b"registerNatives"
                    && descriptor == b"()V"
                {
                    return Ok(Step::Continue);
                }
                if klass.class.name == b"jdk/internal/misc/Unsafe"
                    && [owner, name, descriptor]
                        == [
                            b"jdk/internal/misc/Unsafe".as_slice(),
                            b"registerNatives",
                            b"()V",
                        ]
                {
                    return Ok(Step::Continue);
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
                            crate::share::cds::cdsConfig::CDSConfig::initialize_from_archive()?
                        }
                        Value::Null => {
                            return Err(jex(
                                loader,
                                output,
                                depth,
                                b"java/lang/NullPointerException",
                                None,
                            ));
                        }
                        _ => return Err(Error::other("Expected class mirror")),
                    }
                    return Ok(Step::Continue);
                }
                if [owner, name, descriptor]
                    == [
                        b"java/lang/Class".as_slice(),
                        b"getPrimitiveClass",
                        b"(Ljava/lang/String;)Ljava/lang/Class;",
                    ]
                {
                    let value = crate::share::prims::classNatives::primitiveClass(frame.pop()?)?;
                    frame.push(value)?;
                    return Ok(Step::Continue);
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
                    return Ok(Step::Continue);
                }
                if let Some(intrinsic) = crate::share::prims::jdkIntrinsics::invoke(
                    loader,
                    owner,
                    name,
                    descriptor,
                    &frame.stack[start..],
                )? {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    frame.push(intrinsic)?;
                    return Ok(Step::Continue);
                }
                let __t = std::time::Instant::now();
                let native_result = crate::share::prims::nativeLookup::invokeStatic(
                    loader,
                    owner,
                    name,
                    descriptor,
                    &frame.stack[start..],
                    output,
                    depth,
                )?;
                NATIVE_NANOS.fetch_add(
                    __t.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                if let Some(result) = native_result {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    if let Some(value) = result {
                        frame.push(value)?;
                    }
                    return Ok(Step::Continue);
                }
            }
            if opcode == 0xb6
                && owner == b"jdk/internal/misc/Unsafe"
                && descriptor == b"(Ljava/lang/Class;)I"
                && matches!(name, b"arrayBaseOffset0" | b"arrayIndexScale0")
                && let [Value::Object(handle), Value::ClassMirror(class)] = &frame.stack[start..]
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
                return Ok(Step::Continue);
            }
            let methodError = |error: Error| {
                if throwable_of(&error).is_some()
                    || crate::share::classfile::linkageError::linkage_kind(&error).is_some()
                {
                    error
                } else {
                    Error::other(format!(
                        "{}.{}{}: {}",
                        String::from_utf8_lossy(owner),
                        String::from_utf8_lossy(name),
                        String::from_utf8_lossy(descriptor),
                        error
                    ))
                }
            };
            let (target, code, flags) = if opcode == 0xb8 {
                let target = loader
                    .load(owner)
                    .map_err(|error| linkageExc(loader, output, depth, error))?;
                let key = CallSiteKey::new(2, &target, owner, name, descriptor);
                let (target, code, flags) = cachedCall(loader, key, name, descriptor, || {
                    LinkResolver::resolve_static_call(&target, name, descriptor)
                })
                .map_err(methodError)
                .map_err(|error| linkageExc(loader, output, depth, error))?;
                initialize(loader, &target, output, depth + 1)?;
                if interpreterFlags().traceLabeled
                    && owner
                        .windows(b"VoxelShape".len())
                        .any(|w| w == b"VoxelShape")
                {
                    eprintln!(
                        "[invokestatic-resolve] {}.{}{} -> {}.{}{} pc={}",
                        String::from_utf8_lossy(owner),
                        String::from_utf8_lossy(name),
                        String::from_utf8_lossy(descriptor),
                        String::from_utf8_lossy(target.class.name),
                        String::from_utf8_lossy(code.name),
                        String::from_utf8_lossy(code.descriptor),
                        pc
                    );
                }
                (target, code, flags)
            } else {
                if let Value::ClassMirror(class_name) = frame.stack[start] {
                    match (name, descriptor) {
                        (b"equals", b"(Ljava/lang/Object;)Z") => {
                            let equal = crate::share::prims::classNatives::class_value_name(
                                loader,
                                frame.stack[start + 1],
                            )? == Some(class_name);
                            frame.stack.truncate(start);
                            frame.stackSlots -= slots;
                            frame.push(Value::Int(i32::from(equal)))?;
                            return Ok(Step::Continue);
                        }
                        (b"hashCode", b"()I") => {
                            frame.stack.truncate(start);
                            frame.stackSlots -= slots;
                            frame.push(Value::Int(
                                crate::share::prims::objectNatives::identity(loader, Value::ClassMirror(
                                    class_name,
                                )),
                            ))?;
                            return Ok(Step::Continue);
                        }
                        (b"toString", b"()Ljava/lang/String;") => {
                            frame.stack.truncate(start);
                            frame.stackSlots -= slots;
                            let dotted = decodeString(class_name)?.replace('/', ".");
                            let kind =
                                if class_name.starts_with(b"[") || class_name.starts_with(b":") {
                                    "class"
                                } else {
                                    match loader.load(class_name) {
                                        Ok(class_klass)
                                            if class_klass.class.accessFlags & 0x0200 != 0 =>
                                        {
                                            "interface"
                                        }
                                        _ => "class",
                                    }
                                };
                            let text = loader.storage.alloc_str(&format!("{kind} {dotted}"));
                            frame.push(Value::HostString(text))?;
                            return Ok(Step::Continue);
                        }
                        (b"isAssignableFrom", b"(Ljava/lang/Class;)Z") => {
                            let result = match frame.stack.get(start + 1) {
                                Some(Value::ClassMirror(other)) => {
                                    if class_name == *other {
                                        true
                                    } else if other.starts_with(b"[") {
                                        loader.classAssignable(other, class_name).unwrap_or(false)
                                    } else if class_name.starts_with(b"[") {
                                        false
                                    } else {
                                        match (loader.load(class_name), loader.load(other)) {
                                            (Ok(_target), Ok(source)) => {
                                                SystemDictionary::is_subclass_of(
                                                    &source, class_name,
                                                ) || SystemDictionary::implements_interface(
                                                    &source, class_name,
                                                )
                                            }
                                            _ => false,
                                        }
                                    }
                                }
                                _ => false,
                            };
                            frame.stack.truncate(start);
                            frame.stackSlots -= slots;
                            frame.push(Value::Int(i32::from(result)))?;
                            return Ok(Step::Continue);
                        }
                        (b"wait", _) | (b"notify", b"()V") | (b"notifyAll", b"()V") => {
                            frame.stack.truncate(start);
                            frame.stackSlots -= slots;
                            return Ok(Step::Continue);
                        }
                        _ => {
                            let mut result = crate::share::prims::classNatives::invoke(
                                loader,
                                name,
                                descriptor,
                                &frame.stack[start..],
                            )
                            .map_err(|error| linkageExc(loader, output, depth, error))?;
                            if result.is_none() {
                                result = crate::share::prims::reflectionNatives::class_invoke(
                                    loader,
                                    name,
                                    descriptor,
                                    &frame.stack[start..],
                                )
                                .map_err(|error| linkageExc(loader, output, depth, error))?;
                            }
                            match result {
                                Some(value) => {
                                    frame.stack.truncate(start);
                                    frame.stackSlots -= slots;
                                    frame.push(value)?;
                                    return Ok(Step::Continue);
                                }
                                None => {
                                    // Materialize the class literal into a real
                                    // `java.lang.Class` object and continue with
                                    // ordinary virtual dispatch on it.
                                    match crate::share::prims::classNatives::materialize_class(
                                        loader,
                                        Value::ClassMirror(class_name),
                                    )
                                    .map_err(|error| linkageExc(loader, output, depth, error))?
                                    {
                                        Some(object) => {
                                            frame.stack[start] = object;
                                        }
                                        None => {
                                            return Err(methodError(Error::other(format!(
                                                "Expected instance method receiver at {pc} in {}: ClassMirror",
                                                String::from_utf8_lossy(klass.class.name)
                                            ))));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                let receiver = frame.stack[start];
                if matches!(receiver, Value::Null) {
                    if interpreterFlags().trace {
                        eprintln!(
                            "[npe-site] {}.{}{} pc={} in {}",
                            String::from_utf8_lossy(owner),
                            String::from_utf8_lossy(name),
                            String::from_utf8_lossy(descriptor),
                            pc,
                            String::from_utf8_lossy(klass.class.name),
                        );
                    }
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/NullPointerException",
                        None,
                    ));
                }
                let object_klass = match receiver {
                    Value::Object(handle) => {
                        let objects = loader.borrow_objects();
                        let object = objects
                            .get(handle)
                            .ok_or_else(|| Error::other("Invalid object reference"))?;
                        std::sync::Arc::clone(&object.klass)
                    }
                    // Arrays are objects too: they only expose java.lang.Object
                    // methods (wait/notify/hashCode/toString/getClass; clone is
                    // handled by a dedicated fast path above).
                    Value::PrimitiveArray(_) | Value::ObjectArray(_) => {
                        loader.load(b"java/lang/Object")?
                    }
                    _ => {
                        eprintln!(
                            "[icce-site] {}.{}{} pc={} in {} recv={:?}",
                            String::from_utf8_lossy(owner),
                            String::from_utf8_lossy(name),
                            String::from_utf8_lossy(descriptor),
                            pc,
                            String::from_utf8_lossy(klass.class.name),
                            receiver,
                        );
                        return Err(jex(
                            loader,
                            output,
                            depth,
                            b"java/lang/IncompatibleClassChangeError",
                            Some("Expected object receiver"),
                        ));
                    }
                };
                if opcode == 0xb6
                    && let Some(value) = crate::share::prims::varHandleNatives::polymorphic(
                        loader,
                        &object_klass.class.name,
                        name,
                        descriptor,
                        &frame.stack[start..],
                    )
                    .map_err(|error| linkageExc(loader, output, depth, error))?
                {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    // Signature-polymorphic void access modes (VarHandle set*)
                    // yield no operand-stack value.
                    if descriptor
                        .iter()
                        .rposition(|&b| b == b')')
                        .and_then(|i| descriptor.get(i + 1))
                        != Some(&b'V')
                    {
                        frame.push(value)?;
                    }
                    return Ok(Step::Continue);
                }
                if opcode == 0xb9 && !SystemDictionary::implements_interface(&object_klass, owner) {
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/IncompatibleClassChangeError",
                        Some(&format!(
                            "{} does not implement {}",
                            dotted(object_klass.class.name),
                            dotted(owner),
                        )),
                    ));
                }

                if opcode == 0xb7
                    && owner == b"java/lang/Object"
                    && name == b"<init>"
                    && descriptor == b"()V"
                {
                    frame.stack.truncate(start);
                    frame.stackSlots -= slots;
                    return Ok(Step::Continue);
                }

                if opcode == 0xb7 {
                    let target = loader
                        .load(owner)
                        .map_err(|error| linkageExc(loader, output, depth, error))?;
                    let key = CallSiteKey::new(1, &target, owner, name, descriptor);
                    cachedCall(loader, key, name, descriptor, || {
                        LinkResolver::resolve_special_call(&target, name, descriptor)
                    })
                    .map_err(methodError)
                    .map_err(|error| linkageExc(loader, output, depth, error))?
                } else if opcode == 0xb9 {
                    let key = CallSiteKey::new(3, &object_klass, owner, name, descriptor);
                    cachedCall(loader, key, name, descriptor, || {
                        LinkResolver::resolve_interface_call(&object_klass, owner, name, descriptor)
                    })
                    .map_err(methodError)
                    .map_err(|error| linkageExc(loader, output, depth, error))?
                } else {
                    let key = CallSiteKey::new(0, &object_klass, owner, name, descriptor);
                    cachedCall(loader, key, name, descriptor, || {
                        // JVMS 5.4.5: private methods resolved in the symbolic
                        // class are selected without override lookup.
                        match loader.load(owner) {
                            Ok(owner_klass) => LinkResolver::resolve_virtual_call_scoped(
                                &object_klass,
                                &owner_klass,
                                name,
                                descriptor,
                            ),
                            Err(_) => {
                                LinkResolver::resolve_virtual_call(&object_klass, name, descriptor)
                            }
                        }
                    })
                    .map_err(methodError)
                    .map_err(|error| linkageExc(loader, output, depth, error))?
                }
            };


            if interpreterFlags().traceCalls
                && (owner == b"net/fabricmc/loader/impl/launch/FabricLauncherBase"
                    || owner == b"net/fabricmc/loader/impl/launch/knot/FabricGlobalPropertyService"
                    || owner == b"org/spongepowered/asm/mixin/MixinEnvironment"
                    || owner == b"org/spongepowered/asm/mixin/MixinEnvironment$Phase"
                    || owner == b"org/spongepowered/asm/launch/GlobalProperties"
                    || (interpreterFlags().traceParser
                        && owner == b"sun/nio/fs/WindowsPathParser"))
            {
                let parser_trace = interpreterFlags().traceParser
                    && owner == b"sun/nio/fs/WindowsPathParser";
                eprintln!(
                    "[call] {}.{}{} op={:#x} depth={}{}",
                    String::from_utf8_lossy(owner),
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor),
                    opcode,
                    depth,
                    if parser_trace {
                        format!(" args={:?}", &frame.stack[start..])
                    } else {
                        String::new()
                    }
                );
            }
            if opcode == 0xb7
                && owner == b"java/nio/file/InvalidPathException"
                && name == b"<init>"
            {
                match frame.stack.get(start + 1) {
                    Some(Value::HostString(s)) => {
                        let bytes = s.as_bytes();
                        let head: Vec<u8> = bytes.iter().take(16).copied().collect();
                        eprintln!(
                            "[ipe] host len={} head={:02x?} text={:?}",
                            bytes.len(),
                            head,
                            s
                        );
                    }
                    Some(Value::String(b)) => {
                        let head: Vec<u8> = b.iter().take(16).copied().collect();
                        eprintln!("[ipe] str len={} head={:02x?}", b.len(), head);
                    }
                    Some(other) => {
                        if let Some(text) =
                            crate::share::prims::stringNatives::string(loader, *other)?
                        {
                            let bytes = text.as_bytes();
                            let head: Vec<u8> = bytes.iter().take(16).copied().collect();
                            eprintln!(
                                "[ipe] obj len={} head={:02x?} text={:?}",
                                bytes.len(),
                                head,
                                text
                            );
                        } else {
                            let klass_name = if let Value::Object(handle) = *other {
                                loader
                                    .borrow_objects()
                                    .get(handle)
                                    .map(|object| {
                                        String::from_utf8_lossy(&object.klass.class.name)
                                            .into_owned()
                                    })
                                    .unwrap_or_else(|| "?".to_string())
                            } else {
                                "?".to_string()
                            };
                            eprintln!("[ipe] input other: {:?} klass={}", other, klass_name);
                        }
                    }
                    None => eprintln!("[ipe] no arg"),
                }
            }
            let ret_val = if opcode != 0xb8
                && result.is_none()
                && crate::share::prims::jdkIntrinsics::invokeVoidInstance(
                    loader,
                    target.class.name,
                    name,
                    descriptor,
                    &frame.stack[start..],
                )
            {
                None
            } else if opcode != 0xb8
                && let Some(value) = crate::share::prims::jdkIntrinsics::invokeInstance(
                    loader,
                    &target,
                    name,
                    descriptor,
                    &frame.stack[start..],
                )
            {
                Some(value)
            } else {
                execute_labeled(
                    loader,
                    &target,
                    code,
                    &frame.stack[start..],
                    output,
                    depth + 1,
                    flags,
                    Some((name, descriptor)),
                )?
            };

            frame.stack.truncate(start);
            frame.stackSlots -= slots;
            match (result, ret_val) {
                (Some(kind), Some(val)) if val.matchesLocal(kind) => val,
                (None, None) => return Ok(Step::Continue),
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

            let bs_info = LinkResolver::resolve_invokedynamic(
                loader,
                klass,
                *bsm_attr_idx,
                name,
                descriptor,
            )?;

            let (parameters, _result) = signature::method(descriptor)?;
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

            let mut is_string_concat = name == b"makeConcat" || name == b"makeConcatWithConstants";
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

                let mut boolean_params = Vec::new();
                {
                    let mut rest = descriptor.strip_prefix(b"(").unwrap_or(descriptor);
                    while !rest.is_empty() && rest[0] != b')' {
                        let dims = rest.iter().take_while(|byte| **byte == b'[').count();
                        let tag = rest.get(dims).copied().unwrap_or(0);
                        boolean_params.push(tag == b'Z');
                        let len = if tag == b'L' {
                            dims + 1
                                + rest[dims + 1..]
                                    .iter()
                                    .position(|byte| *byte == b';')
                                    .map(|pos| pos + 1)
                                    .unwrap_or(rest.len().saturating_sub(dims + 1))
                        } else {
                            dims + 1
                        };
                        rest = rest.get(len..).unwrap_or(&[]);
                    }
                }
                let mut dynamic_args = Vec::with_capacity(frame.stack.len() - start);
                for (arg_index, value) in frame.stack[start..].iter().enumerate() {
                    dynamic_args.push(describeValueKind(
                        loader,
                        *value,
                        boolean_params.get(arg_index).copied().unwrap_or(false),
                        output,
                        depth + 1,
                    )?);
                }

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
                let captured = frame.stack[start..].to_vec();
                frame.stack.truncate(start);
                frame.stackSlots -= slots;
                if let Some(value) =
                    super::objectMethods::invoke(loader, klass, &bs_info, &captured, output, depth)?
                {
                    frame.push(value)?;
                    return Ok(Step::Continue);
                }
                match super::lambdaMetafactory::invoke(
                    loader, klass, &bs_info, name, descriptor, &captured,
                )? {
                    Some(value) => {
                        frame.push(value)?;
                        return Ok(Step::Continue);
                    }
                    None => {
                        // Generic JVMS linkage: run the real bootstrap
                        // method, link the CallSite, and invoke its target.
                        if let Some(value) = super::genericBootstrap::invoke(
                            loader, klass, &bs_info, name, descriptor, &captured,
                            output, depth,
                        )? {
                            frame.push(value)?;
                        }
                        return Ok(Step::Continue);
                    }
                }
            }
        }
        0xbb => {
            let name = klass.className(u16::from_be_bytes(frame.read()?))?;
            let target = loader.load(name)?;
            if (target.class.accessFlags & 0x0600) != 0 {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/InstantiationError",
                    Some("Cannot instantiate abstract class or interface"),
                ));
            }
            initialize(loader, &target, output, depth + 1)?;
            // HotSpot registers finalizable instances with
            // java.lang.ref.Finalizer at allocation time.
            let needs_finalizer = target.has_finalizer();
            let object = InstanceOop::new(target)?;
            let handle = loader.alloc_object(object);
            if needs_finalizer {
                let finalizer = loader.load(b"java/lang/ref/Finalizer")?;
                let mut args = JavaCallArguments::new();
                args.push_oop(Value::Object(handle));
                JavaCalls::call_static(
                    loader,
                    &finalizer,
                    b"register",
                    b"(Ljava/lang/Object;)V",
                    &args,
                    output,
                )?;
            }
            Value::Object(handle)
        }
        0xbc => {
            let kind = frame.byte()?;
            ArrayKlass::primitive(loader, kind, frame.pop()?.int()?).map_err(|error| {
                if error.to_string().contains("Negative") {
                    jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/NegativeArraySizeException",
                        Some(&error.to_string()),
                    )
                } else {
                    linkageExc(loader, output, depth, error)
                }
            })?
        }
        0xbd => {
            let component = klass.className(u16::from_be_bytes(frame.read()?))?;
            ArrayKlass::reference(loader, component, frame.pop()?.int()?).map_err(|error| {
                if error.to_string().contains("Negative") {
                    jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/NegativeArraySizeException",
                        Some(&error.to_string()),
                    )
                } else {
                    linkageExc(loader, output, depth, error)
                }
            })?
        }
        0xbe => {
            let arrayref = frame.pop()?;
            if matches!(arrayref, Value::Null) {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/NullPointerException",
                    None,
                ));
            }
            let length = match arrayref {
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
                _ => {
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/IncompatibleClassChangeError",
                        Some("arraylength on non-array"),
                    ));
                }
            };
            Value::Int(length as i32)
        }
        0xbf => {
            // athrow
            let exc = frame.pop()?;
            if matches!(exc, Value::Null) {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/NullPointerException",
                    None,
                ));
            }
            let throwable = match exc {
                Value::Object(handle) => {
                    let objects = loader.borrow_objects();
                    objects.get(handle).map(|object| Arc::clone(&object.klass))
                }
                _ => None,
            };
            let is_throwable = throwable.as_ref().is_some_and(|klass| {
                klass.class.name == b"java/lang/Throwable"
                    || SystemDictionary::is_subclass_of(&klass, b"java/lang/Throwable")
            });
            if !is_throwable {
                return Err(jex(
                    loader,
                    output,
                    depth,
                    b"java/lang/VerifyError",
                    Some("athrow operand is not a Throwable"),
                ));
            }
            let trace_throw = interpreterFlags().trace;
            let is_oom = throwable.as_ref().is_some_and(|klass| {
                klass
                    .class
                    .name
                    .windows(b"OutOfMemoryError".len())
                    .any(|window| window == b"OutOfMemoryError")
            });
            if trace_throw || is_oom {
                let thrown = match exc {
                    Value::Object(handle) => throwableDescription(loader, handle),
                    _ => throwable
                        .as_ref()
                        .map(|klass| dotted(klass.class.name))
                        .unwrap_or_default(),
                };
                let mut frames = Vec::new();
                for_each_active_frame(&mut |frame| frames.push(frame.site()));
                eprintln!("[athrow] {thrown} stack={frames:?}");
            }
            if interpreterFlags().traceServiceConfiguration
                && let Value::Object(handle) = exc
                && let Some(klass) = throwable.as_ref()
                && klass.class.name == b"java/util/ServiceConfigurationError"
            {
                let mut cause = Some(handle);
                let mut depth_guard = 0;
                while let Some(h) = cause
                    && depth_guard < 8
                {
                    depth_guard += 1;
                    eprintln!("[sce] {}", throwableDescription(loader, h));
                    let objects = loader.borrow_objects();
                    let Some(object) = objects.get(h) else { break };
                    cause = object
                        .klass
                        .instanceField(b"cause", b"Ljava/lang/Throwable;")
                        .ok()
                        .and_then(|slot| object.fields.get(slot).copied())
                        .and_then(|v| match v {
                            Value::Object(next) if next != h => Some(next),
                            _ => None,
                        });
                }
            }
            if frame.handle_exception(pc, exc, loader, klass) {
                return Ok(Step::Continue);
            }
            return Err(match exc {
                Value::Object(handle) => Error::other(JavaThrowable(handle)),
                _ => Error::other(format!(
                    "Uncaught exception at pc {pc} in {}: {exc:?}",
                    String::from_utf8_lossy(klass.class.name),
                )),
            });
        }
        0xc0 | 0xc1 => {
            let target = klass.className(u16::from_be_bytes(frame.read()?))?;
            let value = frame.pop()?;
            if opcode == 0xc1 {
                Value::Int(i32::from(
                    !matches!(value, Value::Null) && loader.referenceAssignable(value, target)?,
                ))
            } else {
                if !loader.referenceAssignable(value, target)? {
                    if interpreterFlags().trace {
                        eprintln!(
                            "[trace] checkcast fail pc={pc} in {} target={} value={value:?}",
                            String::from_utf8_lossy(klass.class.name),
                            String::from_utf8_lossy(target),
                        );
                    }
                    return Err(jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/ClassCastException",
                        Some(&format!(
                            "cannot cast to {}",
                            String::from_utf8_lossy(target).replace('/', ".")
                        )),
                    ));
                }
                value
            }
        }
        0xc2 | 0xc3 => {
            let val = frame.pop()?;
            if opcode == 0xc2 {
                let key = monitorEnter(loader, output, depth, val)?;
                frame.monitors.push(key);
            } else {
                monitorExit(loader, output, depth, val)?;
                if let Some(key) = monitorKey(val)
                    && let Some(position) = frame.monitors.iter().rposition(|k| *k == key)
                {
                    frame.monitors.remove(position);
                }
            }
            return Ok(Step::Continue);
        }
        0xc4 => {
            // wide
            let instruction = frame.byte()?;
            let index = u16::from_be_bytes(frame.read()?);
            match instruction {
                0x15..=0x19 => frame.load(index, instruction - 0x15)?,
                0x36..=0x3a => {
                    frame.storeOperand(index, instruction - 0x36)?;
                    return Ok(Step::Continue);
                }
                0x84 => {
                    let increment = i16::from_be_bytes(frame.read()?);
                    frame.increment(index, i32::from(increment))?;
                    return Ok(Step::Continue);
                }
                0xa9 => {
                    let val = *frame.local(index)?;
                    let Value::ReturnAddress(target_pc) = val else {
                        return Err(Error::other("Expected return address in wide ret"));
                    };
                    frame.pc = target_pc;
                    return Ok(Step::Continue);
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
            ArrayKlass::multi(loader, descriptor, &lengths[..dimensions]).map_err(|error| {
                if error.to_string().contains("Negative") {
                    jex(
                        loader,
                        output,
                        depth,
                        b"java/lang/NegativeArraySizeException",
                        Some(&error.to_string()),
                    )
                } else {
                    linkageExc(loader, output, depth, error)
                }
            })?
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
            return Ok(Step::Continue);
        }
        0xc8 => {
            // goto_w
            let offset = frame.readInt()?;
            frame.jump(pc, offset)?;
            return Ok(Step::Continue);
        }
        0xc9 => {
            // jsr_w
            let offset = frame.readInt()?;
            let return_address = frame.pc;
            frame.push(Value::ReturnAddress(return_address))?;
            frame.jump(pc, offset)?;
            return Ok(Step::Continue);
        }
        _ => {
            return Err(Error::other(format!(
                "Unsupported opcode {} (0x{opcode:02x}) at {pc}",
                bytecodes::name(opcode).unwrap_or("unknown")
            )));
        }
    };
    Ok(Step::Push(value))
}
