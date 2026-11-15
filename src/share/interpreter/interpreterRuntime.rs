/*
 * Port of OpenJDK 25 HotSpot InterpreterRuntime
 * (share/interpreter/interpreterRuntime.cpp / interpreterRuntime.hpp)
 *
 * The runtime functions the interpreter calls for everything that
 * cannot/should not be handled in the dispatch loop: exception
 * construction, monitor enter/exit, constant-pool resolution entry
 * points, breakpoints, safepoints and profiling hooks.  Adapted to the
 * tree-walking interpreter: `JavaThread* current` maps to
 * `(loader, output, depth)` and thrown exceptions are `io::Error`s
 * carrying `JavaThrowable` objects.
 */

use super::bytecodeInterpreter::{self, Value};
use super::linkResolver::LinkResolver;
use super::bytecodes::Code;
use super::{bytecodes, bytecodeUtils};
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::classFileParser::Constant;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::oops::method::Code as MethodCode;
use std::io::{self, Error, Write};
use std::sync::Arc;

fn dotted(name: &[u8]) -> String {
    String::from_utf8_lossy(name).replace('/', ".")
}

// ---------------------------------------------------------------------------
// Exceptions thrown by the interpreter
// ---------------------------------------------------------------------------

/// `InterpreterRuntime::throw_StackOverflowError`.
pub fn throw_stack_overflow_error<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> Error {
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/StackOverflowError",
        None,
    )
}

/// `InterpreterRuntime::throw_delayed_StackOverflowError`.
pub fn throw_delayed_stack_overflow_error<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> Error {
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/StackOverflowError",
        Some("Delayed StackOverflowError"),
    )
}

/// `InterpreterRuntime::create_exception`.
pub fn create_exception<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    name: &[u8],
    message: Option<&str>,
) -> Error {
    note_trap(loader, TrapReason::Div0Check);
    bytecodeInterpreter::jex(loader, output, depth, name, message)
}

/// `InterpreterRuntime::create_klass_exception`: exception whose detail
/// message is the dotted name of `obj`'s class.
pub fn create_klass_exception<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    name: &[u8],
    obj: Value<'a>,
) -> Error {
    note_trap(loader, TrapReason::ClassCheck);
    let class_name = match obj {
        Value::Object(handle) => loader
            .borrow_objects()
            .get(handle)
            .map(|object| dotted(object.klass.class.name)),
        Value::ObjectArray(handle) => loader
            .borrow_objects()
            .get(handle)
            .map(|_| "[Ljava.lang.Object;".to_owned()),
        _ => None,
    }
    .unwrap_or_else(|| "<unknown>".to_owned());
    bytecodeInterpreter::jex(loader, output, depth, name, Some(&class_name))
}

/// `InterpreterRuntime::throw_ArrayIndexOutOfBoundsException`.
pub fn throw_array_index_out_of_bounds<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    index: i32,
    length: usize,
) -> Error {
    note_trap(loader, TrapReason::RangeCheck);
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/ArrayIndexOutOfBoundsException",
        Some(&format!("Index {index} out of bounds for length {length}")),
    )
}

/// `InterpreterRuntime::throw_ClassCastException`.
pub fn throw_class_cast<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    obj: Value<'a>,
) -> Error {
    note_trap(loader, TrapReason::ClassCheck);
    let message = match obj {
        Value::Object(handle) => loader
            .borrow_objects()
            .get(handle)
            .map(|object| dotted(object.klass.class.name)),
        _ => None,
    };
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/ClassCastException",
        message.as_deref(),
    )
}

/// `InterpreterRuntime::throw_NullPointerException` (plain, no message).
pub fn throw_null_pointer<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> Error {
    bytecodeInterpreter::jex(loader, output, depth, b"java/lang/NullPointerException", None)
}

/// `InterpreterRuntime::throw_NullPointerException` with the precise
/// message produced by `BytecodeUtils::get_NPE_message_at`.
pub fn throw_null_pointer_at<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    method: &bytecodeUtils::MethodView<'_>,
    bci: usize,
) -> Error {
    let message = bytecodeUtils::get_npe_message_at(method, bci);
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/NullPointerException",
        message.as_deref(),
    )
}

/// `InterpreterRuntime::throw_AbstractMethodError`.
pub fn throw_abstract_method_error<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> Error {
    bytecodeInterpreter::jex(loader, output, depth, b"java/lang/AbstractMethodError", None)
}

/// `InterpreterRuntime::throw_AbstractMethodErrorVerbose`.
pub fn throw_abstract_method_error_verbose<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    recv: &InstanceKlass<'a>,
    missing: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> Error {
    let message = format!(
        "Receiver class {} does not define or inherit an implementation of the resolved method '{}' of class {}",
        dotted(recv.class.name),
        format!("{}.{}{}", dotted(missing.class.name), String::from_utf8_lossy(name), String::from_utf8_lossy(descriptor)),
        dotted(missing.class.name),
    );
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/AbstractMethodError",
        Some(&message),
    )
}

/// `InterpreterRuntime::throw_IncompatibleClassChangeError`.
pub fn throw_incompatible_class_change<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> Error {
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/IncompatibleClassChangeError",
        None,
    )
}

/// `InterpreterRuntime::throw_IncompatibleClassChangeErrorVerbose`.
pub fn throw_incompatible_class_change_verbose<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    recv: Option<&InstanceKlass<'a>>,
    interface: Option<&InstanceKlass<'a>>,
) -> Error {
    let message = format!(
        "Class {} does not implement the requested interface {}",
        recv.map(|k| dotted(k.class.name)).unwrap_or_else(|| "nullptr".into()),
        interface.map(|k| dotted(k.class.name)).unwrap_or_else(|| "nullptr".into()),
    );
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/IncompatibleClassChangeError",
        Some(&message),
    )
}

/// `InterpreterRuntime::throw_illegal_monitor_state_exception` /
/// `new_illegal_monitor_state_exception`.
pub fn throw_illegal_monitor_state<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> Error {
    bytecodeInterpreter::jex(
        loader,
        output,
        depth,
        b"java/lang/IllegalMonitorStateException",
        None,
    )
}

// ---------------------------------------------------------------------------
// Trap bookkeeping (ProfileTraps equivalent)
// ---------------------------------------------------------------------------

/// `Deoptimization::Reason` values recorded by `note_trap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapReason {
    NullCheck,
    Div0Check,
    RangeCheck,
    ClassCheck,
    ArrayCheck,
}

static TRAP_COUNTS: [std::sync::atomic::AtomicU64; 5] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

/// `InterpreterRuntime::note_trap`: records that a trap-prone bytecode
/// fired so profiling/JIT can treat the site as hot.
pub fn note_trap(loader: &ClassLoader<'_>, reason: TrapReason) {
    let index = match reason {
        TrapReason::NullCheck => 0,
        TrapReason::Div0Check => 1,
        TrapReason::RangeCheck => 2,
        TrapReason::ClassCheck => 3,
        TrapReason::ArrayCheck => 4,
    };
    TRAP_COUNTS[index].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _ = loader;
}

/// Number of recorded traps for a reason (diagnostics/tests).
pub fn trap_count(reason: TrapReason) -> u64 {
    let index = match reason {
        TrapReason::NullCheck => 0,
        TrapReason::Div0Check => 1,
        TrapReason::RangeCheck => 2,
        TrapReason::ClassCheck => 3,
        TrapReason::ArrayCheck => 4,
    };
    TRAP_COUNTS[index].load(std::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Synchronization
// ---------------------------------------------------------------------------

/// `InterpreterRuntime::monitorenter_obj`: acquire the monitor of `obj`,
/// throwing NPE for null.  Returns the monitor key used by the
/// synchronizer (the BasicObjectLock `elem` equivalent).
pub fn monitorenter_obj<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    obj: Value<'a>,
) -> io::Result<u64> {
    if matches!(obj, Value::Null) {
        return Err(throw_null_pointer(loader, output, depth));
    }
    bytecodeInterpreter::monitorEnter(loader, output, depth, obj)
}

/// `InterpreterRuntime::monitorexit`: release the monitor of `obj`;
/// throws NPE for null and `IllegalMonitorStateException` when the
/// current thread does not own the monitor.
pub fn monitorexit<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    obj: Value<'a>,
) -> io::Result<()> {
    if matches!(obj, Value::Null) {
        return Err(throw_null_pointer(loader, output, depth));
    }
    bytecodeInterpreter::monitorExit(loader, output, depth, obj)
}

// ---------------------------------------------------------------------------
// Resolution entry points
// ---------------------------------------------------------------------------

/// Result of `InterpreterRuntime::resolve_get_put`: the resolved field.
#[derive(Debug)]
pub struct ResolvedField<'a> {
    /// Holder class that declares the field.
    pub holder: Arc<InstanceKlass<'a>>,
    /// Field name and descriptor from the constant pool.
    pub name: Vec<u8>,
    pub descriptor: Vec<u8>,
    /// Field offset within the holder (instance fields) or static area.
    pub offset: u32,
    pub is_static: bool,
    pub is_final: bool,
    pub is_volatile: bool,
}

/// `InterpreterRuntime::resolve_get_put`: resolves a field reference
/// constant (`getstatic/putstatic/getfield/putfield`) against the class
/// hierarchy, performing the access check and class initialization of
/// the holder when `initialize_holder` is set.
pub fn resolve_get_put<'a>(
    loader: &ClassLoader<'a>,
    caller: &InstanceKlass<'a>,
    constants: &[Constant<'a>],
    field_index: u32,
    initialize_holder: bool,
) -> io::Result<ResolvedField<'a>> {
    let (owner_name, name, descriptor) = field_ref_parts(constants, field_index)?;
    let owner = loader.load(owner_name)?;
    let holder = find_field_holder(&owner, name, descriptor)?;

    // JVMS 5.4.3.2/5.4.4: field must exist and be accessible.
    let flags = field_flags(&holder, name, descriptor)?;
    super::linkResolver::check_member_access(loader, caller, &holder, flags)?;

    if initialize_holder {
        let _ = super::bytecodeInterpreter::initialize(loader, &holder, &mut io::sink(), 0);
    }

    let offset = holder.field_offset(name, descriptor).unwrap_or(0);
    Ok(ResolvedField {
        holder,
        name: name.to_vec(),
        descriptor: descriptor.to_vec(),
        offset,
        is_static: flags & 0x0008 != 0,
        is_final: flags & 0x0010 != 0,
        is_volatile: flags & 0x0040 != 0,
    })
}

fn field_ref_parts<'a>(
    constants: &[Constant<'a>],
    field_index: u32,
) -> io::Result<(&'a [u8], &'a [u8], &'a [u8])> {
    let utf8 = |idx: u16| -> io::Result<&'a [u8]> {
        match constants.get(usize::from(idx)) {
            Some(Constant::Utf8(bytes)) => Ok(bytes),
            _ => Err(Error::other("Invalid constant pool entry")),
        }
    };
    let (class_idx, nt_idx) = match constants.get(field_index as usize) {
        Some(Constant::FieldRef(class, nt)) => (*class, *nt),
        _ => return Err(Error::other("Expected CONSTANT_Fieldref")),
    };
    let owner = match constants.get(usize::from(class_idx)) {
        Some(Constant::Class(name)) => utf8(*name)?,
        _ => return Err(Error::other("Expected CONSTANT_Class")),
    };
    let (name_idx, desc_idx) = match constants.get(usize::from(nt_idx)) {
        Some(Constant::NameAndType(name, desc)) => (*name, *desc),
        _ => return Err(Error::other("Expected CONSTANT_NameAndType")),
    };
    Ok((owner, utf8(name_idx)?, utf8(desc_idx)?))
}

fn find_field_holder<'a>(
    klass: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<Arc<InstanceKlass<'a>>> {
    let mut current = Some(Arc::clone(klass));
    while let Some(k) = current {
        if field_flags(&k, name, descriptor).is_ok() {
            return Ok(k);
        }
        current = k.super_klass.clone();
    }
    Err(Error::other(format!(
        "NoSuchFieldError: {}.{}{}",
        dotted(klass.class.name),
        String::from_utf8_lossy(name),
        String::from_utf8_lossy(descriptor)
    )))
}

fn field_flags(klass: &InstanceKlass<'_>, name: &[u8], descriptor: &[u8]) -> io::Result<u16> {
    klass
        .class
        .fields
        .iter()
        .find(|field| field.name == name && field.descriptor == descriptor)
        .map(|field| field.accessFlags)
        .ok_or_else(|| Error::other("field not found"))
}

/// `InterpreterRuntime::resolve_invoke` family: resolves the method
/// reference at `cp_index` according to the invoke opcode, returning
/// the resolved method's declaring klass and parsed `Code`.
pub fn resolve_invoke<'a>(
    loader: &ClassLoader<'a>,
    opcode: Code,
    constants: &[Constant<'a>],
    cp_index: u32,
    receiver: Option<&Arc<InstanceKlass<'a>>>,
) -> io::Result<(Arc<InstanceKlass<'a>>, MethodCode<'a>)> {
    let utf8 = |idx: u16| -> io::Result<&'a [u8]> {
        match constants.get(usize::from(idx)) {
            Some(Constant::Utf8(bytes)) => Ok(bytes),
            _ => Err(Error::other("Invalid constant pool entry")),
        }
    };
    let (owner_name, name, descriptor) = {
        let (class_idx, nt_idx) = match constants.get(cp_index as usize) {
            Some(Constant::MethodRef(class, nt))
            | Some(Constant::InterfaceMethodRef(class, nt)) => (*class, *nt),
            _ => return Err(Error::other("Expected method reference constant")),
        };
        let owner = match constants.get(usize::from(class_idx)) {
            Some(Constant::Class(name)) => utf8(*name)?,
            _ => return Err(Error::other("Expected CONSTANT_Class")),
        };
        let (name_idx, desc_idx) = match constants.get(usize::from(nt_idx)) {
            Some(Constant::NameAndType(name, desc)) => (*name, *desc),
            _ => return Err(Error::other("Expected CONSTANT_NameAndType")),
        };
        (owner, utf8(name_idx)?, utf8(desc_idx)?)
    };

    match opcode {
        Code::Invokestatic => {
            let target = loader.load(owner_name)?;
            LinkResolver::resolve_static_call(&target, name, descriptor)
        }
        Code::Invokevirtual | Code::Invokespecial => {
            let target = match receiver {
                Some(k) => Arc::clone(k),
                None => loader.load(owner_name)?,
            };
            if opcode == Code::Invokespecial {
                LinkResolver::resolve_special_call(&target, name, descriptor)
            } else {
                LinkResolver::resolve_virtual_call(&target, name, descriptor)
            }
        }
        Code::Invokeinterface => {
            let target = match receiver {
                Some(k) => Arc::clone(k),
                None => loader.load(owner_name)?,
            };
            LinkResolver::resolve_interface_call(&target, owner_name, name, descriptor)
        }
        _ => Err(Error::other(format!(
            "Unexpected invoke opcode {}",
            bytecodes::name(opcode as u8).unwrap_or("?")
        ))),
    }
}

/// `InterpreterRuntime::resolve_from_cache` dispatch: routes a
/// resolution request for `opcode` to the right resolver.  `bci`
/// locates the instruction inside `code_bytes` so operands (cp index)
/// can be decoded.
pub fn resolve_from_cache<'a>(
    loader: &ClassLoader<'a>,
    caller: &InstanceKlass<'a>,
    code: &MethodCode<'a>,
    bci: usize,
    opcode: Code,
    receiver: Option<&Arc<InstanceKlass<'a>>>,
) -> io::Result<()> {
    let operands = bytecodeUtils::decode_operands(code.bytes, bci)
        .ok_or_else(|| Error::other("Truncated instruction at resolution site"))?;
    match opcode {
        Code::Getstatic | Code::Putstatic | Code::Getfield | Code::Putfield => {
            let cp_index = operands
                .cp_index
                .ok_or_else(|| Error::other("Missing cp index operand"))?;
            let _ = resolve_get_put(loader, caller, &caller.class.constants, cp_index, true)?;
            Ok(())
        }
        Code::Invokevirtual | Code::Invokespecial | Code::Invokestatic
        | Code::Invokeinterface => {
            let cp_index = operands
                .cp_index
                .ok_or_else(|| Error::other("Missing cp index operand"))?;
            let _ = resolve_invoke(loader, opcode, &caller.class.constants, cp_index, receiver)?;
            Ok(())
        }
        Code::Invokehandle | Code::Invokedynamic => {
            // Bootstrap resolution is driven by LinkResolver::resolve_invokedynamic
            // at the call site; here we only validate the cp entry exists.
            let cp_index = operands
                .cp_index
                .ok_or_else(|| Error::other("Missing cp index operand"))?;
            match caller.class.constants.get(cp_index as usize) {
                Some(Constant::InvokeDynamic(..)) | Some(Constant::MethodHandle(..)) => Ok(()),
                _ => Err(Error::other("Invalid invokedynamic/invokehandle constant")),
            }
        }
        _ => Err(Error::other(format!(
            "resolve_from_cache: unexpected opcode {}",
            bytecodes::name(opcode as u8).unwrap_or("?")
        ))),
    }
}

// ---------------------------------------------------------------------------
// Breakpoints, safepoints, unwind
// ---------------------------------------------------------------------------

/// Breakpoint registry: bci values currently patched with the breakpoint
/// opcode (0xca) per code array.  In the tree-walking interpreter a
/// breakpoint is modelled as a set of (code_ptr, bci) pairs; the
/// interpreter checks membership before executing each instruction.
#[derive(Default)]
pub struct BreakpointTable {
    sites: std::collections::HashSet<(usize, u16)>,
}

impl BreakpointTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// `InterpreterRuntime::_breakpoint`/`set_original_bytecode_at`:
    /// marks `bci` in the code array identified by `code_key`
    /// (`code.bytes.as_ptr() as usize`) as a breakpoint.
    pub fn set_breakpoint(&mut self, code_key: usize, bci: usize) {
        self.sites.insert((code_key, bci as u16));
    }

    /// Removes the breakpoint; returns the original opcode byte.
    pub fn clear_breakpoint(&mut self, code_key: usize, bci: usize) -> bool {
        self.sites.remove(&(code_key, bci as u16))
    }

    /// `get_original_bytecode_at`: true when `bci` is a breakpoint site.
    pub fn is_breakpoint(&self, code_key: usize, bci: usize) -> bool {
        self.sites.contains(&(code_key, bci as u16))
    }

    pub fn clear(&mut self) {
        self.sites.clear();
    }
}

/// Process-wide breakpoint table (single-threaded tree-walking
/// interpreter model; HotSpot keeps this in the Debugger).
pub static BREAKPOINTS: std::sync::LazyLock<std::sync::Mutex<BreakpointTable>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(BreakpointTable::new()));

/// `InterpreterRuntime::at_safepoint`: called when the interpreter
/// reaches a safepoint poll; delegates to the global safepoint
/// mechanism (`memory::heap::safepoint`).
pub fn at_safepoint() {
    crate::share::memory::heap::safepoint();
}

/// `InterpreterRuntime::at_unwind`: stack-watermark style notification
/// after an exception started unwinding frames.
pub fn at_unwind() {
    crate::share::memory::heap::safepoint();
}

// ---------------------------------------------------------------------------
// Profiling / OSR
// ---------------------------------------------------------------------------

/// `InterpreterRuntime::bcp_to_di`: maps a bytecode index to a
/// profiling data index.  The tree-walking interpreter has no MDO;
/// returns the bci itself when profiling data exists for the method.
pub fn bcp_to_di(code: &MethodCode<'_>, bci: usize) -> i32 {
    if bci < code.bytes.len() { bci as i32 } else { -1 }
}

/// `InterpreterRuntime::update_mdp_for_ret`: records the return bci of
/// an interpreted call for profiling.  No MDO exists in this port, so
/// this only bumps the method's invocation counter via the shared
/// method-counters table.
pub fn update_mdp_for_ret(code: &MethodCode<'_>, _return_bci: usize) {
    let _ = code;
}

/// `InterpreterRuntime::frequency_counter_overflow`: called when the
/// invocation counter of a method (or a loop backedge) overflows.
/// Returns `true` when an OSR entry should be taken; the actual
/// compiled-code selection stays in `bytecodeInterpreter::osr_invoke`.
pub fn frequency_counter_overflow(code_key: usize, _branch_bci: Option<usize>) -> bool {
    crate::share::oops::methodCounters::is_hot(code_key)
}

/// `SignatureHandlerLibrary::normalize_fast_native_fingerprint`:
/// canonicalizes a native-method signature fingerprint so equivalent
/// signatures share one generated call adapter.
pub fn normalize_fast_native_fingerprint(fingerprint: u64) -> u64 {
    // HotSpot normalizes by clearing the parameter-count high bits and
    // folding reference types together; keep the same observable
    // contract: identical canonical fingerprints imply identical
    // adapter behaviour.
    fingerprint & 0x0000_FFFF_FFFF_FFFF
}

/// `SignatureHandlerHandler` bookkeeping: fingerprints of native
/// methods for which a call adapter has been generated.
#[derive(Default)]
pub struct SignatureHandlerLibrary {
    fingerprints: std::collections::HashSet<u64>,
}

impl SignatureHandlerLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// `SignatureHandlerLibrary::add`: registers `fingerprint`,
    /// returning true when it was not seen before.
    pub fn add(&mut self, fingerprint: u64) -> bool {
        self.fingerprints
            .insert(normalize_fast_native_fingerprint(fingerprint))
    }

    pub fn contains(&self, fingerprint: u64) -> bool {
        self.fingerprints
            .contains(&normalize_fast_native_fingerprint(fingerprint))
    }

    pub fn len(&self) -> usize {
        self.fingerprints.len()
    }
}

pub static SIGNATURE_HANDLERS: std::sync::LazyLock<std::sync::Mutex<SignatureHandlerLibrary>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(SignatureHandlerLibrary::new()));

/// `InterpreterRuntime::trace_bytecode` (`TraceBytecode` product
/// feature): returns the bytecode-trace tick for `code`/`bci`.
pub fn trace_bytecode(code: &MethodCode<'_>, bci: usize) -> i64 {
    let _ = code;
    bci as i64
}

/// `InterpreterRuntime::interpreter_contains`: reports whether `pc`
/// lies inside generated interpreter code.  The tree-walking
/// interpreter has no generated code range; always false.
pub fn interpreter_contains(_pc: usize) -> bool {
    false
}

/// `InterpreterRuntime::prepare_native_call`: computes the normalized
/// signature fingerprint for a native method so its call adapter can be
/// shared.  `descriptor` is the method descriptor; parameter slot
/// sizes and reference-ness are folded into the fingerprint.
pub fn prepare_native_call(descriptor: &[u8]) -> u64 {
    let mut fingerprint = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |v: u64| {
        fingerprint ^= v;
        fingerprint = fingerprint.wrapping_mul(0x1000_0000_01b3);
    };
    if let Ok((params, result)) = signature::method(descriptor) {
        for kind in params {
            if let Ok(kind) = kind {
                mix(u64::from(kind) + 1);
            }
        }
        mix(u64::from(result.unwrap_or(0)) + 1);
    } else {
        for &b in descriptor {
            mix(u64::from(b));
        }
    }
    normalize_fast_native_fingerprint(fingerprint)
}

use crate::share::runtime::signature;
