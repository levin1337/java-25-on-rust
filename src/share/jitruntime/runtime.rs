/*
 * Interpreter <-> compiled-code runtime for the C1 x64 backend.
 *
 * This is the execution bridge: `try_invoke` is called by
 * `bytecodeInterpreter::execute_labeled` once an nmethod has been
 * installed for the method.  It marshals interpreter `Value`
 * arguments into the Windows x64 register/stack convention used by
 * the C1 backend (`emit_params`), calls the nmethod entry point and
 * translates the result back into interpreter values.
 *
 * Compiled leaf methods carry no object references they can
 * dereference (the interpreter object model is an arena of handles,
 * not raw oops), so the executable subset stays primitive:
 * integer/long arithmetic, conversions, comparisons, branches,
 * switches, returns — plus runtime stubs for polls, exceptions,
 * monitors and deoptimization.
 *
 * Exceptional control flow: stubs that must not return to JIT code
 * (NPE/AIOOBE/ArithmeticException, `athrow`, monitor failures,
 * deoptimization) record a `Pending` outcome in thread-local storage
 * and unwind the JIT frame through a generated trampoline that lands
 * on the post-call site of `invoke_raw`.  `try_invoke` then reports
 * `Thrown(handle)` or `Deoptimize` instead of a value.
 */

use super::stubs;
use crate::share::interpreter::bytecodeInterpreter::Value;

/// Max stack-passed arguments supported by the trampoline.  With four
/// Win64 argument registers this bounds total parameters at 20.
const MAX_STACK_ARGS: usize = 16;
/// Total parameter slots (registers + stack) the trampoline can marshal.
const MAX_PARAMS: usize = 4 + MAX_STACK_ARGS;

thread_local! {
    /// Outcome recorded by a runtime stub that unwound out of JIT code.
    static PENDING: std::cell::Cell<Pending> = const { std::cell::Cell::new(Pending::None) };
    /// Interpreter recursion depth of the frame that entered JIT code;
    /// stubs forward it to throwable-object construction.
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Result channel between JIT-called stubs and `try_invoke`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    /// No exceptional exit was requested.
    None,
    /// A Java throwable (object-arena handle) must be propagated.
    Thrown(usize),
    /// The compiled code hit an unsupported/uncommon path; the caller
    /// must resume the method in the interpreter (HotSpot deopt).
    Deoptimize,
}

pub(crate) fn set_pending_throw(handle: usize) {
    PENDING.with(|p| p.set(Pending::Thrown(handle)));
}

pub(crate) fn set_pending_deopt() {
    PENDING.with(|p| p.set(Pending::Deoptimize));
}

pub(crate) fn take_pending() -> Pending {
    PENDING.with(|p| p.replace(Pending::None))
}

pub(crate) fn set_depth(depth: usize) {
    DEPTH.with(|d| d.set(depth));
}

pub(crate) fn current_depth() -> usize {
    DEPTH.with(|d| d.get())
}

/// Outcome of a compiled-code invocation.
#[derive(Debug)]
pub enum CompiledResult<'a> {
    /// Method returned normally; inner `Option` is the return value
    /// (`None` for `void`).
    Value(Option<Value<'a>>),
    /// A Java throwable (object-arena handle) was raised by a stub or
    /// by `athrow`; the interpreter must propagate it.
    Thrown(usize),
    /// The nmethod requested deoptimization; the interpreter must
    /// re-execute the method and the nmethod should be retired.
    Deoptimize,
}

/// Parsed view of a method descriptor restricted to the executable
/// subset: primitive types plus references.  References cross the
/// ABI as tagged 64-bit handles (see `encode_ref`/`decode_ref`);
/// compiled code treats them as opaque words and only runtime stubs
/// interpret the tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    Int32,
    Int64,
    Float32,
    Float64,
    Ref,
    Void,
}

/// Reference encoding shared by `try_invoke` marshaling and every
/// runtime stub that passes object values through the GP registers.
///
/// The interpreter keeps three disjoint handle spaces
/// (`borrow_objects`, `arrays`, `obj_arrays`) plus lightweight
/// non-handle `Value` variants (borrowed strings, mirrors).  Only
/// the three handle spaces plus null can cross the JIT boundary;
/// anything else makes the caller fall back to the interpreter
/// (parameters) or deoptimizes (stub results).
///
/// Layout: bits [63:48] = space tag, bits [47:0] = handle + 1
/// (so a zero word unambiguously means `null`).
pub const REF_TAG_OBJECT: u64 = 0;
pub const REF_TAG_OBJECT_ARRAY: u64 = 1;
pub const REF_TAG_PRIMITIVE_ARRAY: u64 = 2;
const REF_HANDLE_MASK: u64 = (1u64 << 48) - 1;

/// Encodes an interpreter `Value` as the raw word compiled code sees.
/// Returns `None` for variants that have no handle representation.
pub fn encode_ref(value: &Value<'_>) -> Option<u64> {
    let (tag, handle) = match value {
        Value::Null => return Some(0),
        Value::Object(h) => (REF_TAG_OBJECT, *h),
        Value::ObjectArray(h) => (REF_TAG_OBJECT_ARRAY, *h),
        Value::PrimitiveArray(h) => (REF_TAG_PRIMITIVE_ARRAY, *h),
        _ => return None,
    };
    let slot = (handle as u64).checked_add(1)?;
    if slot > REF_HANDLE_MASK {
        return None;
    }
    Some((tag << 48) | slot)
}

/// Decodes a raw word produced by compiled code / a stub back into
/// an interpreter `Value`.  Only encodings `encode_ref` can produce
/// occur in practice; unknown tags map to `Null` defensively (a
/// misbehaving nmethod is retired by the deopt path anyway).
pub fn decode_ref(raw: u64) -> Value<'static> {
    if raw == 0 {
        return Value::Null;
    }
    let tag = raw >> 48;
    let handle = ((raw & REF_HANDLE_MASK).wrapping_sub(1)) as usize;
    match tag {
        REF_TAG_OBJECT => Value::Object(handle),
        REF_TAG_OBJECT_ARRAY => Value::ObjectArray(handle),
        REF_TAG_PRIMITIVE_ARRAY => Value::PrimitiveArray(handle),
        _ => Value::Null,
    }
}

/// Whether `raw` encodes a non-null reference word.
pub fn ref_is_null(raw: u64) -> bool {
    raw == 0
}

pub struct Signature {
    /// Parameter kinds in declaration order (`this` excluded).
    pub params: Vec<SlotKind>,
    /// Whether the method is an instance method (`this` occupies
    /// argument register 0 as an opaque handle).
    pub is_instance: bool,
    pub ret: SlotKind,
}

/// Parses `(params)return` and rejects anything outside the
/// executable subset.  Returns `None` for unsupported signatures.
pub fn parse_signature(descriptor: &[u8], is_static: bool) -> Option<Signature> {
    if descriptor.first() != Some(&b'(') {
        return None;
    }
    let mut params = Vec::new();
    let mut i = 1usize;
    while i < descriptor.len() && descriptor[i] != b')' {
        let kind = match descriptor[i] {
            b'B' | b'C' | b'I' | b'S' | b'Z' => SlotKind::Int32,
            b'J' => SlotKind::Int64,
            b'F' => SlotKind::Float32,
            b'D' => SlotKind::Float64,
            b'L' => {
                // Reference type: skip to the terminating ';'.
                let mut j = i + 1;
                while j < descriptor.len() && descriptor[j] != b';' {
                    j += 1;
                }
                if j >= descriptor.len() {
                    return None;
                }
                i = j;
                SlotKind::Ref
            }
            b'[' => {
                // Array type: skip the bracket run then the element
                // (either a single primitive letter or L...;).
                let mut j = i;
                while j < descriptor.len() && descriptor[j] == b'[' {
                    j += 1;
                }
                if j >= descriptor.len() {
                    return None;
                }
                if descriptor[j] == b'L' {
                    while j < descriptor.len() && descriptor[j] != b';' {
                        j += 1;
                    }
                    if j >= descriptor.len() {
                        return None;
                    }
                } else if !matches!(descriptor[j], b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z') {
                    return None;
                }
                i = j;
                SlotKind::Ref
            }
            _ => return None,
        };
        params.push(kind);
        i += 1;
        if params.len() > MAX_PARAMS {
            return None;
        }
    }
    if i >= descriptor.len() || descriptor[i] != b')' {
        return None;
    }
    i += 1;
    if i >= descriptor.len() {
        return None;
    }
    let ret = match descriptor[i] {
        b'V' => SlotKind::Void,
        b'B' | b'C' | b'I' | b'S' | b'Z' => SlotKind::Int32,
        b'J' => SlotKind::Int64,
        b'F' => SlotKind::Float32,
        b'D' => SlotKind::Float64,
        b'L' | b'[' => SlotKind::Ref,
        _ => return None,
    };
    Some(Signature {
        params,
        is_instance: !is_static,
        ret,
    })
}

/// Whether `descriptor`/`is_static` describe a method the executable
/// tier can marshal.  Used by the compiler to refuse methods whose
/// signature cannot round-trip through `try_invoke`.
pub fn signature_supported(descriptor: &[u8], is_static: bool) -> bool {
    parse_signature(descriptor, is_static).is_some()
}

/// Converts one interpreter `Value` argument to the raw 64-bit slot the
/// compiled code expects.  Primitive values pass through directly;
/// references use the tagged `encode_ref` scheme.  Returns `None` on a
/// representation mismatch (the caller then falls back to the
/// interpreter).
fn arg_bits(value: &Value<'_>) -> Option<u64> {
    match value {
        Value::Int(v) => Some(*v as u32 as u64),
        Value::Long(v) => Some(*v as u64),
        Value::Float(v) => Some(v.to_bits() as u64),
        Value::Double(v) => Some(v.to_bits()),
        _ => encode_ref(value),
    }
}

/// Argument block consumed by `invoke_raw`; keeps the asm! register
/// pressure low (a single pointer operand).
#[repr(C)]
struct CallArgs {
    /// Win64 integer argument registers: rcx, rdx, r8, r9.
    regs: [u64; 4],
    /// Win64 float argument registers: xmm0-xmm3 (bit patterns).
    xmm: [u64; 4],
    /// Stack-passed arguments (placed at [rsp+32] by the callee side).
    stack: *const u64,
    /// Number of stack arguments.
    nstack: usize,
    /// nmethod entry point.
    addr: usize,
}

/// Calls compiled code at `args.addr` (nmethod entry point) with the
/// given raw arguments following the Windows x64 convention emitted by
/// `X86Emitter`: arguments 0..3 in rcx/rdx/r8/r9, the rest on the
/// stack above a 32-byte shadow space.  Returns the raw rax result.
///
/// Frame protocol shared with the stub-escape trampolines: the JIT
/// prologue/epilogue leaves `rsp` equal to the pre-`call` value on
/// both normal return and trampoline unwind, and `r12` holds the
/// caller's `rsp - 8`, so the post-call code just restores `rsp`/`r12`.
#[cfg(windows)]
unsafe fn invoke_raw(args: &CallArgs) -> (u64, u64) {
    let mut result: u64;
    let mut xmm_result: u64;
    unsafe {
        core::arch::asm!(
            "push r12",
            "mov r12, rsp",
            "and rsp, -16",
            "mov r10, {args}",
            // frame = align16(32 + nstack*8): shadow space + stack args
            "mov rcx, [r10+72]",
            "lea rax, [rcx*8+47]",
            "and rax, -16",
            "sub rsp, rax",
            // rep movsq: copy stack args to [rsp+32 ..] (above shadow space)
            "mov rsi, [r10+64]",
            "lea rdi, [rsp+32]",
            "mov rcx, [r10+72]",
            "cld",
            "rep movsq",
            // Win64 argument registers
            "mov rcx, [r10]",
            "mov rdx, [r10+8]",
            "mov r8, [r10+16]",
            "mov r9, [r10+24]",
            // xmm0-xmm3 from the xmm slot array
            "movq xmm0, [r10+32]",
            "movq xmm1, [r10+40]",
            "movq xmm2, [r10+48]",
            "movq xmm3, [r10+56]",
            "mov rax, [r10+80]",
            "call rax",
            // Landing pad: reached both by the normal JIT `ret` and by
            // the stub unwind trampoline; rsp is the pre-call value and
            // the 32-byte shadow space at [rsp] is still ours.
            // Stash xmm0's return bits through memory: a GP<->XMM movq
            // here proved unreliable (xmm clobber declarations let LLVM
            // keep xmm0 live as zero across the boundary).
            "movsd [rsp], xmm0",
            "mov r11, [rsp]",
            "mov rsp, r12",
            "pop r12",
            args = in(reg) args,
            out("rax") result,
            out("r11") xmm_result,
            out("rcx") _,
            out("rdx") _,
            out("rsi") _,
            out("rdi") _,
            out("r8") _,
            out("r9") _,
            out("r10") _,
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
            out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
            out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
        );
    }
    (result, xmm_result)
}

#[cfg(not(windows))]
unsafe fn invoke_raw(_args: &CallArgs) -> (u64, u64) {
    (0, 0)
}

/// Attempts to execute `addr` (a compiled nmethod entry point) for the
/// method described by `descriptor`.  Returns:
///  - `None` — the method is outside the executable subset or the
///    argument list does not match; caller must interpret instead.
///  - `Some(CompiledResult::Value(v))` — executed normally.
///  - `Some(CompiledResult::Thrown(h))` — a Java throwable handle.
///  - `Some(CompiledResult::Deoptimize)` — resume in the interpreter.
pub fn try_invoke<'a>(
    addr: usize,
    descriptor: &[u8],
    is_static: bool,
    arguments: &[Value<'a>],
    depth: usize,
) -> Option<CompiledResult<'a>> {
    if addr == 0 {
        return None;
    }
    let sig = parse_signature(descriptor, is_static)?;
    let expected = sig.params.len() + usize::from(sig.is_instance);
    if expected != arguments.len() {
        return None;
    }
    let mut regs = [0u64; 4];
    let mut xmm = [0u64; 4];
    let mut stack = [0u64; MAX_STACK_ARGS];
    let mut nstack = 0usize;
    let mut arg_iter = arguments.iter();
    // Instance methods receive `this` as argument 0: an opaque handle
    // that compiled code may hold but never dereference (any
    // dereference would have failed the HIR safety scan).
    let mut arg_index = 0usize;
    if sig.is_instance {
        let bits = arg_bits(arg_iter.next()?)?;
        if arg_index < 4 {
            regs[arg_index] = bits;
        } else {
            return None; // unreachable: params capped, but stay safe
        }
        arg_index += 1;
    }
    for (kind, value) in sig.params.iter().zip(arg_iter) {
        // Win64 unified slot: floats occupy xmm[i], integers gp[i];
        // slots >= 4 spill to the shared stack argument area.
        let is_float = matches!(kind, SlotKind::Float32 | SlotKind::Float64);
        let bits = match (kind, value) {
            (SlotKind::Int32, Value::Int(v)) => *v as u32 as u64,
            (SlotKind::Int64, Value::Long(v)) => *v as u64,
            (SlotKind::Float32, Value::Float(v)) => v.to_bits() as u64,
            (SlotKind::Float64, Value::Double(v)) => v.to_bits(),
            (SlotKind::Ref, _) => match encode_ref(value) {
                Some(bits) => bits,
                None => return None,
            },
            _ => return None,
        };
        if arg_index < 4 {
            if is_float {
                xmm[arg_index] = bits;
            } else {
                regs[arg_index] = bits;
            }
        } else {
            let s = arg_index - 4;
            if s >= MAX_STACK_ARGS {
                return None;
            }
            stack[s] = bits;
            nstack = s + 1;
        }
        arg_index += 1;
    }

    // Arm the outcome channel: stubs record pending results here and
    // unwind through the trampoline back to the post-call site.
    take_pending();
    set_depth(depth);
    let call_args = CallArgs {
        regs,
        xmm,
        stack: stack.as_ptr(),
        nstack,
        addr,
    };
    let (raw, xmm_raw) = unsafe { invoke_raw(&call_args) };
    if std::env::var_os("JVM25_JIT_DEBUG").is_some() {
        eprintln!("[jit] ret={:?} raw={:016x} xmm_raw={:016x}", sig.ret, raw, xmm_raw);
    }
    match take_pending() {
        Pending::Thrown(handle) => Some(CompiledResult::Thrown(handle)),
        Pending::Deoptimize => Some(CompiledResult::Deoptimize),
        Pending::None => {
            let value = match sig.ret {
                SlotKind::Void => None,
                SlotKind::Int32 => Some(Value::Int(raw as u32 as i32)),
                SlotKind::Int64 => Some(Value::Long(raw as i64)),
                SlotKind::Float32 => Some(Value::Float(f32::from_bits(xmm_raw as u32))),
                SlotKind::Float64 => Some(Value::Double(f64::from_bits(xmm_raw))),
                SlotKind::Ref => Some(decode_ref(raw)),
            };
            Some(CompiledResult::Value(value))
        }
    }
}

/// Re-export so the compiler backend can pull the stub table without
/// depending on the stubs module directly.
pub use stubs::stub_map;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::c1::asm::x86::ParamInfo;
    use crate::share::c1::c1_LIR::LirKind;

    /// Compiles `bytecode` with C1, publishes the code into the exec
    /// arena and returns the entry address — the same path
    /// `CodeCache::install_nmethod` uses.
    fn compile_and_publish(bytecode: &[u8], max_locals: usize, params: Vec<ParamInfo>) -> usize {
        let mut compiler = crate::share::c1::c1_Compiler::C1Compiler::new();
        let (code, _frame, _pcs) = compiler.compile_to_code(bytecode, max_locals, params).unwrap();
        crate::share::jitruntime::publish_executable(&code, code.len()).unwrap()
    }

    #[test]
    fn iconst_ireturn_runs_native() {
        // iconst_1; ireturn  ->  ()I
        let addr = compile_and_publish(&[0x04, 0xac], 1, Vec::new());
        let result = try_invoke(addr, b"()I", true, &[], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 1),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn iadd_runs_native() {
        // iload_0; iload_1; iadd; ireturn -> (II)I
        let addr = compile_and_publish(
            &[0x1a, 0x1b, 0x60, 0xac],
            2,
            vec![
                ParamInfo { local_slot: 0, kind: LirKind::Int32 },
                ParamInfo { local_slot: 1, kind: LirKind::Int32 },
            ],
        );
        let args = [Value::Int(20), Value::Int(22)];
        let result = try_invoke(addr, b"(II)I", true, &args, 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 42),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn lconst_lload_ladd_runs_native() {
        // lconst_0 is 0x09; use lload params instead:
        // lload_0; lload_2; ladd; lreturn -> (JJ)J
        // second long param occupies JVM slots 2-3 -> lload_2 (0x20)
        let addr = compile_and_publish(
            &[0x1e, 0x20, 0x61, 0xad],
            4,
            vec![
                ParamInfo { local_slot: 0, kind: LirKind::Int64 },
                ParamInfo { local_slot: 2, kind: LirKind::Int64 },
            ],
        );
        let args = [Value::Long(1_000_000_007), Value::Long(2_000_000_003)];
        let result = try_invoke(addr, b"(JJ)J", true, &args, 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Long(v))) => {
                assert_eq!(v, 3_000_000_010)
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn div_by_zero_unwinds_to_pending() {
        // iconst_1; iconst_0; idiv; ireturn -> the emitted div-by-zero
        // check calls the ArithmeticException stub; with no JVM loader
        // installed the handler records a deopt and the trampoline
        // unwinds the JIT frame back into invoke_raw.
        let addr = compile_and_publish(&[0x04, 0x03, 0x6c, 0xac], 1, Vec::new());
        let result = try_invoke(addr, b"()I", true, &[], 0).unwrap();
        assert!(matches!(result, CompiledResult::Deoptimize | CompiledResult::Thrown(_)));
    }

    #[test]
    fn void_return_runs_native() {
        // return -> ()V
        let addr = compile_and_publish(&[0xb1], 0, Vec::new());
        let result = try_invoke(addr, b"()V", true, &[], 0).unwrap();
        assert!(matches!(result, CompiledResult::Value(None)));
    }

    #[test]
    fn fconst_freturn_runs_native() {
        // fconst_0 (0x0b); freturn (0xae) -> ()F
        let addr = compile_and_publish(&[0x0b, 0xae], 1, Vec::new());
        let result = try_invoke(addr, b"()F", true, &[], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Float(v))) => assert_eq!(v, 0.0),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn dconst_dreturn_runs_native() {
        // dconst_1 (0x0f); dreturn (0xaf) -> ()D
        let addr = compile_and_publish(&[0x0f, 0xaf], 2, Vec::new());
        let result = try_invoke(addr, b"()D", true, &[], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Double(v))) => assert_eq!(v, 1.0),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn fadd_runs_native() {
        // fload_0 (0x22); fload_1 (0x23); fadd (0x62); freturn (0xae)
        // -> (FF)F ; float params occupy one local slot each
        let addr = compile_and_publish(
            &[0x22, 0x23, 0x62, 0xae],
            2,
            vec![
                ParamInfo { local_slot: 0, kind: LirKind::Float32 },
                ParamInfo { local_slot: 1, kind: LirKind::Float32 },
            ],
        );
        let args = [Value::Float(1.5), Value::Float(2.25)];
        let result = try_invoke(addr, b"(FF)F", true, &args, 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Float(v))) => assert_eq!(v, 3.75),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn dadd_runs_native() {
        // dload_0 (0x26); dload_2 (0x28); dadd (0x63); dreturn (0xaf)
        // -> (DD)D ; each double occupies two local slots, so the second
        // param is at JVM slot 2 -> dload_2
        let addr = compile_and_publish(
            &[0x26, 0x28, 0x63, 0xaf],
            4,
            vec![
                ParamInfo { local_slot: 0, kind: LirKind::Float64 },
                ParamInfo { local_slot: 2, kind: LirKind::Float64 },
            ],
        );
        let args = [Value::Double(1.5), Value::Double(2.25)];
        let result = try_invoke(addr, b"(DD)D", true, &args, 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Double(v))) => assert_eq!(v, 3.75),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn if_icmpeq_param_param() {
        // iload_0; iload_1; if_icmpeq L1; iconst_0; ireturn; L1: iconst_1; ireturn
        // (II)Z
        let addr = compile_and_publish(
            &[
                0x1a, 0x1b, 0x9f, 0x00, 0x05, 0x03, 0xac, 0x04, 0xac,
            ],
            2,
            vec![
                ParamInfo { local_slot: 0, kind: LirKind::Int32 },
                ParamInfo { local_slot: 1, kind: LirKind::Int32 },
            ],
        );
        for (a, b, want) in [(5i32, 5i32, 1i32), (5, 6, 0), (0, 0, 1)] {
            let args = [Value::Int(a), Value::Int(b)];
            let result = try_invoke(addr, b"(II)Z", true, &args, 0).unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, want, "({a},{b})"),
                other => panic!("({a},{b}) unexpected: {:?}", other),
            }
        }
    }

    #[test]
    fn if_icmpeq_param_bipush() {
        // iload_0; bipush 92; if_icmpeq L1; iconst_0; ireturn; L1: iconst_1; ireturn
        // (I)Z
        let addr = compile_and_publish(
            &[0x1a, 0x10, 0x5c, 0x9f, 0x00, 0x05, 0x03, 0xac, 0x04, 0xac],
            1,
            vec![ParamInfo { local_slot: 0, kind: LirKind::Int32 }],
        );
        for (a, want) in [(92i32, 1i32), (67, 0), (0, 0)] {
            let args = [Value::Int(a)];
            let result = try_invoke(addr, b"(I)Z", true, &args, 0).unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, want, "({a})"),
                other => panic!("({a}) unexpected: {:?}", other),
            }
        }
    }

    #[test]
    fn ifeq_zero_param() {
        // iload_0; ifeq L1; iconst_1; ireturn; L1: iconst_0; ireturn
        // (I)Z
        let addr = compile_and_publish(
            &[0x1a, 0x99, 0x00, 0x05, 0x04, 0xac, 0x03, 0xac],
            1,
            vec![ParamInfo { local_slot: 0, kind: LirKind::Int32 }],
        );
        for (a, want) in [(0i32, 0i32), (7, 1)] {
            let args = [Value::Int(a)];
            let result = try_invoke(addr, b"(I)Z", true, &args, 0).unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, want, "({a})"),
                other => panic!("({a}) unexpected: {:?}", other),
            }
        }
    }

    #[test]
    fn dump_isslash_code() {
        let mut compiler = crate::share::c1::c1_Compiler::C1Compiler::new();
        let (code, frame, pcs) = compiler
            .compile_to_code(
                &[
                    0x1a, 0x10, 0x5c, 0x9f, 0x00, 0x09, 0x1a, 0x10, 0x2f, 0xa0, 0x00, 0x07,
                    0x04, 0xa7, 0x00, 0x04, 0x03, 0xac,
                ],
                1,
                vec![ParamInfo { local_slot: 0, kind: LirKind::Int32 }],
            )
            .unwrap();
        eprintln!("frame={} pcs={:?}", frame, pcs);
        for (i, b) in code.iter().enumerate() {
            eprint!("{:02x} ", b);
            if i % 16 == 15 {
                eprintln!();
            }
        }
        eprintln!();
    }

    #[test]
    fn isslash_branch_runs_native() {
        // Mirrors sun.nio.fs.WindowsPathParser.isSlash(C)Z:
        //   0: iload_0; 1: bipush '\\'(92); 3: if_icmpeq +9 -> 12
        //   6: iload_0; 7: bipush '/'(47); 9: if_icmpne +7 -> 16
        //  12: iconst_1; 13: goto +4 -> 17; 16: iconst_0; 17: ireturn
        let addr = compile_and_publish(
            &[
                0x1a, 0x10, 0x5c, 0x9f, 0x00, 0x09, 0x1a, 0x10, 0x2f, 0xa0, 0x00, 0x07, 0x04,
                0xa7, 0x00, 0x04, 0x03, 0xac,
            ],
            1,
            vec![ParamInfo { local_slot: 0, kind: LirKind::Int32 }],
        );
        for (arg, want) in [(92i32, 1i32), (47, 1), (67, 0), (58, 0), (0, 0), (255, 0)] {
            let args = [Value::Int(arg)];
            let result = try_invoke(addr, b"(C)Z", true, &args, 0).unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => {
                    assert_eq!(v, want, "isSlash({arg})")
                }
                other => panic!("isSlash({arg}) unexpected: {:?}", other),
            }
        }
    }

    #[test]
    fn dump_div_code() {
        let mut compiler = crate::share::c1::c1_Compiler::C1Compiler::new();
        let (code, frame, pcs) = compiler
            .compile_to_code(&[0x04, 0x03, 0x6c, 0xac], 1, Vec::new())
            .unwrap();
        eprintln!("frame={} pcs={:?}", frame, pcs);
        for (i, b) in code.iter().enumerate() {
            eprint!("{:02x} ", b);
            if i % 16 == 15 {
                eprintln!();
            }
        }
        eprintln!();
    }

    #[test]
    fn mixed_int_float_params_use_unified_slots() {
        // iload_0 (0x1a); fload_1 (0x23); f2i (0x8b); iadd (0x60);
        // ireturn (0xac) -> (IF)I
        // iload_0 -> local 0; fload_1 -> local 1 (floats take one slot)
        let addr = compile_and_publish(
            &[0x1a, 0x23, 0x8b, 0x60, 0xac],
            2,
            vec![
                ParamInfo { local_slot: 0, kind: LirKind::Int32 },
                ParamInfo { local_slot: 1, kind: LirKind::Float32 },
            ],
        );
        let args = [Value::Int(40), Value::Float(2.9)];
        let result = try_invoke(addr, b"(IF)I", true, &args, 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 42),
            other => panic!("unexpected result: {:?}", other),
        }
    }
}
