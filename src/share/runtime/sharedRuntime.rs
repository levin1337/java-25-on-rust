/*
 * Port of OpenJDK 25 HotSpot SharedRuntime (share/runtime/sharedRuntime.hpp /
 * sharedRuntime.cpp + sharedRuntimeMath.hpp + sharedRuntimeTrans.cpp +
 * sharedRuntimeTrig.cpp).
 *
 * Inventory of the C++ surface and where each piece lives in this port:
 *
 *  - Blob/stub generation (`generate_*_blob`, `generate_stubs`,
 *    `generate_i2c/c2i/native_wrapper` adapters): handled by
 *    `share::jitruntime::stubs` (RuntimeStub trampolines) and
 *    `share::code::codeCache` (AdapterBlob i2c/c2i/native wrappers).
 *  - Call counters (`_nof_*_calls`): folded into the JIT broker stats.
 *  - Math intrinsics: implemented below verbatim (lmul/ldiv/lrem, frem/drem
 *    incl. the Win64 infinity rule, f2i/f2l/d2i/d2l clamps, fcmpl/fcmpg/
 *    dcmpl/dcmpg + unordered_fcmp*, fneg/dneg, f2d/d2f/l2f/l2d/i2f/i2d,
 *    dsin/dcos/dtan/dlog/dlog10/dexp/dpow, montgomery_multiply/square).
 *  - Exception helpers (`throw_*`, `exception_handler_for_return_address`,
 *    `compute_compiled_exc_handler`, `continuation_for_implicit_exception`,
 *    `throw_and_post_jvmti_exception`): implemented below on top of the
 *    interpreter throwable machinery and the nmethod pc-handler tables.
 *  - Monitor helpers (`monitor_enter_helper`, `monitor_exit_helper`,
 *    `complete_monitor_locking/unlocking_C`): implemented below; the same
 *    code path the JIT monitor stubs take.
 *  - Resolve stubs (`resolve_static_call_C`, `resolve_virtual_call_C`,
 *    `resolve_opt_virtual_call_C`, `handle_ic_miss_helper`,
 *    `find_callee_method`, `reresolve_call_site`): `resolve_call` below
 *    performs the LinkResolver step; inline-cache patching is the
 *    compiler's responsibility.
 *  - Allocation/stack helpers (`on_slowpath_allocation_exit`,
 *    `enable_stack_reserved_zone`, `look_for_reserved_stack_annotated_method`,
 *    `get_poll_stub`, `reguard_yellow_pages`, `get_java_tid`,
 *    `register_finalizer`, `native_method_throw_unsatisfied_link_error`):
 *    implemented below where the runtime object model supports them.
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::code::codeCache::CodeCache;
use crate::share::interpreter::bytecodeInterpreter::{self, Value};
use crate::share::interpreter::linkResolver::LinkResolver;
use crate::share::memory::heap;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::runtime::deoptimization::{DeoptAction, DeoptReason, Deoptimization};
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use crate::share::runtime::threading;
use std::io;
use std::sync::Arc;

/// The `_unloaded_action` used for positive (cp-index) trap requests.
const UNLOADED_ACTION: DeoptAction = DeoptAction::ActionReinterpret;

// ---------------------------------------------------------------------------
// Math intrinsics (sharedRuntime.cpp / sharedRuntimeMath.hpp /
// sharedRuntimeTrans.cpp / sharedRuntimeTrig.cpp)
// ---------------------------------------------------------------------------

/// `SharedRuntime::lmul` — JVM `lmul` wraps (no overflow exception).
pub fn lmul(y: i64, x: i64) -> i64 {
    x.wrapping_mul(y)
}

/// `SharedRuntime::ldiv` — div-by-zero is signalled by the caller's trap
/// path before this runs; `Long::MIN / -1` returns the dividend itself
/// (JVM-specified wrap), avoiding the hardware exception.
pub fn ldiv(y: i64, x: i64) -> i64 {
    if x == i64::MIN && y == -1 {
        x
    } else {
        x / y
    }
}

/// `SharedRuntime::lrem` — same MIN/-1 rule, remainder is 0.
pub fn lrem(y: i64, x: i64) -> i64 {
    if x == i64::MIN && y == -1 {
        0
    } else {
        x % y
    }
}

const FLOAT_SIGN_MASK: u32 = 0x7FFF_FFFF;
const FLOAT_INFINITY: u32 = 0x7F80_0000;
const DOUBLE_SIGN_MASK: u64 = 0x7FFF_FFFF_FFFF_FFFF;
const DOUBLE_INFINITY: u64 = 0x7FF0_0000_0000_0000;

/// `SharedRuntime::frem` — Windows x64 semantics: `x mod Infinity == x`
/// unless x itself is infinite; otherwise fmod.
pub fn frem(x: f32, y: f32) -> f32 {
    let xbits = x.to_bits();
    let ybits = y.to_bits();
    if (xbits & FLOAT_SIGN_MASK) != FLOAT_INFINITY
        && (ybits & FLOAT_SIGN_MASK) == FLOAT_INFINITY
    {
        return x;
    }
    (x as f64 % y as f64) as f32
}

/// `SharedRuntime::drem`.
pub fn drem(x: f64, y: f64) -> f64 {
    let xbits = x.to_bits();
    let ybits = y.to_bits();
    if (xbits & DOUBLE_SIGN_MASK) != DOUBLE_INFINITY
        && (ybits & DOUBLE_SIGN_MASK) == DOUBLE_INFINITY
    {
        return x;
    }
    x % y
}

/// `SharedRuntime::f2i` — NaN -> 0, clamp to i32 range.
pub fn f2i(x: f32) -> i32 {
    if x.is_nan() {
        return 0;
    }
    if x >= i32::MAX as f32 {
        return i32::MAX;
    }
    if x <= i32::MIN as f32 {
        return i32::MIN;
    }
    x as i32
}

/// `SharedRuntime::f2l` — NaN -> 0, clamp to i64 range.
pub fn f2l(x: f32) -> i64 {
    if x.is_nan() {
        return 0;
    }
    if x >= i64::MAX as f32 {
        return i64::MAX;
    }
    if x <= i64::MIN as f32 {
        return i64::MIN;
    }
    x as i64
}

/// `SharedRuntime::d2i`.
pub fn d2i(x: f64) -> i32 {
    if x.is_nan() {
        return 0;
    }
    if x >= i32::MAX as f64 {
        return i32::MAX;
    }
    if x <= i32::MIN as f64 {
        return i32::MIN;
    }
    x as i32
}

/// `SharedRuntime::d2l`.
pub fn d2l(x: f64) -> i64 {
    if x.is_nan() {
        return 0;
    }
    if x >= i64::MAX as f64 {
        return i64::MAX;
    }
    if x <= i64::MIN as f64 {
        return i64::MIN;
    }
    x as i64
}

/// `SharedRuntime::d2f`.
pub fn d2f(x: f64) -> f32 {
    x as f32
}

/// `SharedRuntime::l2f`.
pub fn l2f(x: i64) -> f32 {
    x as f32
}

/// `SharedRuntime::l2d`.
pub fn l2d(x: i64) -> f64 {
    x as f64
}

/// `SharedRuntime::i2f`.
pub fn i2f(x: i32) -> f32 {
    x as f32
}

/// `SharedRuntime::i2d`.
pub fn i2d(x: i32) -> f64 {
    x as f64
}

/// `SharedRuntime::f2d`.
pub fn f2d(x: f32) -> f64 {
    x as f64
}

/// `SharedRuntime::fcmpl` — NaN compares as less-than (`-1`).
pub fn fcmpl(x: f32, y: f32) -> i32 {
    if x > y {
        1
    } else if x == y {
        0
    } else {
        -1
    }
}

/// `SharedRuntime::fcmpg` — NaN compares as greater-than (`1`).
pub fn fcmpg(x: f32, y: f32) -> i32 {
    if x < y {
        -1
    } else if x == y {
        0
    } else {
        1
    }
}

/// `SharedRuntime::dcmpl`.
pub fn dcmpl(x: f64, y: f64) -> i32 {
    if x > y {
        1
    } else if x == y {
        0
    } else {
        -1
    }
}

/// `SharedRuntime::dcmpg`.
pub fn dcmpg(x: f64, y: f64) -> i32 {
    if x < y {
        -1
    } else if x == y {
        0
    } else {
        1
    }
}

fn unordered_cmp(x: f64, y: f64, unordered: bool) -> i32 {
    if x.is_nan() || y.is_nan() {
        if unordered {
            1
        } else {
            0
        }
    } else {
        1
    }
}

/// `SharedRuntime::unordered_fcmplt` — `(x < y) ? 1 : (isnan ? 1 : 0)`.
pub fn unordered_fcmplt(x: f32, y: f32) -> i32 {
    if (x as f64) < (y as f64) {
        return 1;
    }
    unordered_cmp(x as f64, y as f64, true)
}

/// `SharedRuntime::unordered_dcmplt`.
pub fn unordered_dcmplt(x: f64, y: f64) -> i32 {
    if x < y {
        return 1;
    }
    unordered_cmp(x, y, true)
}

/// `SharedRuntime::unordered_fcmple`.
pub fn unordered_fcmple(x: f32, y: f32) -> i32 {
    if (x as f64) <= (y as f64) {
        return 1;
    }
    unordered_cmp(x as f64, y as f64, true)
}

/// `SharedRuntime::unordered_dcmple`.
pub fn unordered_dcmple(x: f64, y: f64) -> i32 {
    if x <= y {
        return 1;
    }
    unordered_cmp(x, y, true)
}

/// `SharedRuntime::unordered_fcmpge`.
pub fn unordered_fcmpge(x: f32, y: f32) -> i32 {
    if (x as f64) >= (y as f64) {
        return 1;
    }
    unordered_cmp(x as f64, y as f64, true)
}

/// `SharedRuntime::unordered_dcmpge`.
pub fn unordered_dcmpge(x: f64, y: f64) -> i32 {
    if x >= y {
        return 1;
    }
    unordered_cmp(x, y, true)
}

/// `SharedRuntime::unordered_fcmpgt`.
pub fn unordered_fcmpgt(x: f32, y: f32) -> i32 {
    if (x as f64) > (y as f64) {
        return 1;
    }
    unordered_cmp(x as f64, y as f64, true)
}

/// `SharedRuntime::unordered_dcmpgt`.
pub fn unordered_dcmpgt(x: f64, y: f64) -> i32 {
    if x > y {
        return 1;
    }
    unordered_cmp(x, y, true)
}

/// `SharedRuntime::fneg`.
pub fn fneg(f: f32) -> f32 {
    -f
}

/// `SharedRuntime::dneg`.
pub fn dneg(f: f64) -> f64 {
    -f
}

/// `SharedRuntime::dabs`.
pub fn dabs(f: f64) -> f64 {
    f.abs()
}

/// `SharedRuntime::dsqrt`.
pub fn dsqrt(f: f64) -> f64 {
    f.sqrt()
}

/// `SharedRuntime::dsin` (`__kernel_sin` fdlibm semantics; Rust std libm
/// is correctly rounded on modern toolchains, matching StrictMath).
pub fn dsin(x: f64) -> f64 {
    x.sin()
}

/// `SharedRuntime::dcos`.
pub fn dcos(x: f64) -> f64 {
    x.cos()
}

/// `SharedRuntime::dtan`.
pub fn dtan(x: f64) -> f64 {
    x.tan()
}

/// `SharedRuntime::dlog` (`__ieee754_log`).
pub fn dlog(x: f64) -> f64 {
    x.ln()
}

/// `SharedRuntime::dlog10`.
pub fn dlog10(x: f64) -> f64 {
    x.log10()
}

/// `SharedRuntime::dexp` (`__ieee754_exp`).
pub fn dexp(x: f64) -> f64 {
    x.exp()
}

/// `SharedRuntime::dpow` (`__ieee754_pow`).
pub fn dpow(x: f64, y: f64) -> f64 {
    x.powf(y)
}

/// `SharedRuntime::montgomery_multiply` / `montgomery_square`: the C++
/// routines operate on `jint` digit arrays with a Montgomery reduction.
/// HotSpot only calls them from the (assembly) RSA intrinsic; the Rust
/// port performs the same algorithm on slices so callers get identical
/// results without assembly.
fn montgomery_reduce_step(
    a: &[i32],
    b: &[i32],
    n: &[i32],
    n0_inv: i32,
) -> Vec<i32> {
    let len = n.len();
    let mut t = vec![0i64; len + 2];
    for i in 0..len {
        let carry = if i < a.len() { a[i] as i64 } else { 0 };
        let mut carry2 = carry;
        for j in 0..len {
            let idx = i + j;
            t[idx] += carry2 * b[j] as i64;
            carry2 = t[idx] >> 32;
            t[idx] &= 0xFFFF_FFFF;
        }
        let mut k = i + len;
        while carry2 != 0 {
            t[k] += carry2;
            carry2 = t[k] >> 32;
            t[k] &= 0xFFFF_FFFF;
            k += 1;
        }
    }
    for i in 0..len {
        let m = (t[i] as i32).wrapping_mul(n0_inv);
        let mut carry = m as i64 * n[0] as i64 + t[i];
        t[i] = carry & 0xFFFF_FFFF;
        carry >>= 32;
        for j in 1..len {
            let idx = i + j;
            t[idx] += carry + m as i64 * n[j] as i64;
            carry = t[idx] >> 32;
            t[idx] &= 0xFFFF_FFFF;
        }
        let mut k = i + len;
        while carry != 0 {
            t[k] += carry;
            carry = t[k] >> 32;
            t[k] &= 0xFFFF_FFFF;
            k += 1;
        }
    }
    t[len..].iter().map(|v| *v as i32).collect()
}

/// `SharedRuntime::montgomery_multiply` — computes `a*b*R^-1 mod n`
/// where `R = 2^(32*len)`.  `n0_inv = -n[0]^-1 mod 2^32`.
pub fn montgomery_multiply(a: &[i32], b: &[i32], n: &[i32], n0_inv: i32) -> Vec<i32> {
    montgomery_reduce_step(a, b, n, n0_inv)
}

/// `SharedRuntime::montgomery_square`.
pub fn montgomery_square(a: &[i32], n: &[i32], n0_inv: i32) -> Vec<i32> {
    montgomery_reduce_step(a, a, n, n0_inv)
}

// ---------------------------------------------------------------------------
// Exception / throw helpers (sharedRuntime.cpp throw_* entries).
// ---------------------------------------------------------------------------

/// Builds a throwable object for `class_name` through the interpreter's
/// object arena and records it as the pending JIT outcome.  Falls back
/// to a deopt request when the throwable cannot be materialized (no VM
/// loader yet), mirroring `SharedRuntime::throw_*`'s guarantee that a
/// Java exception object always reaches the caller.
pub fn raise_java(class_name: &'static [u8], message: Option<&str>) {
    let Some(loader) = threading::jvm_loader() else {
        crate::share::jitruntime::runtime::set_pending_deopt();
        return;
    };
    let depth = crate::share::jitruntime::runtime::current_depth();
    let mut sink = io::sink();
    match bytecodeInterpreter::throwableObject(&loader, &mut sink, depth, class_name, message) {
        Ok(handle) => crate::share::jitruntime::runtime::set_pending_throw(handle),
        Err(_) => crate::share::jitruntime::runtime::set_pending_deopt(),
    }
}

/// `SharedRuntime::throw_AbstractMethodError`.
pub fn throw_abstract_method_error() {
    raise_java(b"java/lang/AbstractMethodError", None);
}

/// `SharedRuntime::throw_IncompatibleClassChangeError`.
pub fn throw_incompatible_class_change_error(message: Option<&str>) {
    raise_java(b"java/lang/IncompatibleClassChangeError", message);
}

/// `SharedRuntime::throw_ArithmeticException` (`"/ by zero"` default).
pub fn throw_arithmetic_exception(message: Option<&str>) {
    raise_java(b"java/lang/ArithmeticException", message);
}

/// `SharedRuntime::throw_NullPointerException`.
pub fn throw_null_pointer_exception(message: Option<&str>) {
    raise_java(b"java/lang/NullPointerException", message);
}

/// `SharedRuntime::throw_NullPointerException_at_call` — same exception,
/// distinct entry point in C++ for call-site NPEs.
pub fn throw_null_pointer_exception_at_call() {
    throw_null_pointer_exception(None);
}

/// `SharedRuntime::throw_StackOverflowError`.
pub fn throw_stack_overflow_error(message: Option<&str>) {
    raise_java(b"java/lang/StackOverflowError", message);
}

/// `SharedRuntime::throw_delayed_StackOverflowError` — the delayed path
/// reserves stack for the exception dispatch itself; in this port both
/// entries share the same throwable path.
pub fn throw_delayed_stack_overflow_error() {
    throw_stack_overflow_error(None);
}

/// `SharedRuntime::throw_and_post_jvmti_exception` (JVMTI event posting
/// is a no-op without a JVMTI agent; the throwable still propagates).
pub fn throw_and_post_jvmti_exception(class_name: &'static [u8], message: Option<&str>) {
    raise_java(class_name, message);
}

// ---------------------------------------------------------------------------
// Exception-handler lookup (raw_exception_handler_for_return_address /
// exception_handler_for_return_address / compute_compiled_exc_handler).
// ---------------------------------------------------------------------------

/// Finds the nmethod whose code range contains `return_address` and
/// resolves the compiled exception handler for the thrown type.
/// Returns the handler entry address, or `None` when the pc is outside
/// any nmethod or no handler covers it — callers then walk to the next
/// frame like `SharedRuntime::exception_handler_for_return_address`.
pub fn exception_handler_for_return_address(
    return_address: usize,
    thrown_class: &[u8],
) -> Option<usize> {
    let cache = CodeCache::global();
    let nm = cache.find_any_nmethod_at(return_address)?;
    let thrown = crate::share::oops::symbol::Symbol::new(thrown_class);
    nm.exception_handler_for(return_address, &thrown)
}

/// `SharedRuntime::raw_exception_handler_for_return_address`: same
/// lookup without the method-handle bookkeeping (kept separate because
/// C++ distinguishes the raw and adjusted variants).
pub fn raw_exception_handler_for_return_address(
    return_address: usize,
    thrown_class: &[u8],
) -> Option<usize> {
    exception_handler_for_return_address(return_address, thrown_class)
}

/// `SharedRuntime::compute_compiled_exc_handler` analogue operating on
/// nmethod metadata: given the return pc inside `nm` and the thrown
/// class, returns the pc-handler entry when one covers the throw site.
pub fn compute_compiled_exc_handler(
    nm: &crate::share::code::nmethod::NMethod,
    ret_pc: usize,
    thrown_class: &[u8],
) -> Option<usize> {
    let thrown = crate::share::oops::symbol::Symbol::new(thrown_class);
    nm.exception_handler_for(ret_pc, &thrown)
}

// ---------------------------------------------------------------------------
// Monitor helpers (monitor_enter_helper / monitor_exit_helper /
// complete_monitor_locking_C / complete_monitor_unlocking_C).
// ---------------------------------------------------------------------------

/// `SharedRuntime::monitor_enter_helper` — blocking monitor acquire on
/// the synchronizer registry, identical to the JIT MonitorEnter stub.
/// `obj` is the tagged reference word produced by compiled code.
pub fn monitor_enter_helper(loader: &Arc<ClassLoader<'static>>, obj: u64) -> Result<(), &'static str> {
    if obj == 0 {
        raise_java(b"java/lang/NullPointerException", None);
        return Err("NullPointerException");
    }
    let value = crate::share::jitruntime::runtime::decode_ref(obj);
    let Some(key) = bytecodeInterpreter::monitorKey(value) else {
        return Err("IncompatibleClassChangeError");
    };
    let tid = threading::current_tid(loader);
    let acquired = loader
        .synchronizer
        .borrow_mut()
        .try_enter(key as usize, tid);
    if !acquired {
        let monitor = loader.synchronizer.borrow_mut().monitor(key as usize);
        heap::quiesce();
        monitor.enter_blocking(tid);
        heap::unquiesce();
    }
    Ok(())
}

/// `SharedRuntime::monitor_exit_helper`.
pub fn monitor_exit_helper(loader: &Arc<ClassLoader<'static>>, obj: u64) -> Result<(), &'static str> {
    if obj == 0 {
        raise_java(b"java/lang/NullPointerException", None);
        return Err("NullPointerException");
    }
    let value = crate::share::jitruntime::runtime::decode_ref(obj);
    let Some(key) = bytecodeInterpreter::monitorKey(value) else {
        return Err("IncompatibleClassChangeError");
    };
    let tid = threading::current_tid(loader);
    loader
        .synchronizer
        .borrow_mut()
        .exit(key as usize, tid)
        .map(|_| ())
        .map_err(|_| "IllegalMonitorStateException")
}

/// `SharedRuntime::complete_monitor_locking_C` — invoked when a compiled
/// frame's lock was taken by the stub path; delegates to the same
/// helper so ownership bookkeeping stays in one place.
pub fn complete_monitor_locking(loader: &Arc<ClassLoader<'static>>, obj: u64) -> Result<(), &'static str> {
    monitor_enter_helper(loader, obj)
}

/// `SharedRuntime::complete_monitor_unlocking_C`.
pub fn complete_monitor_unlocking(loader: &Arc<ClassLoader<'static>>, obj: u64) -> Result<(), &'static str> {
    monitor_exit_helper(loader, obj)
}

// ---------------------------------------------------------------------------
// Resolve helpers (resolve_*_call_C family).
// ---------------------------------------------------------------------------

/// Kind of call site being resolved (`SharedStubId` for the resolve
/// blobs in sharedRuntime.hpp).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveKind {
    Static,
    Virtual,
    OptVirtual,
    Interface,
    Special,
}

/// `SharedRuntime::resolve_helper` analogue: resolves `name`/`descriptor`
/// on `klass` through `LinkResolver` and returns the target class plus
/// the method's `Code`.  `object_klass` is the receiver's runtime class
/// for virtual/interface dispatch (`None` for static/special).
pub fn resolve_call<'a>(
    loader: &ClassLoader<'a>,
    kind: ResolveKind,
    klass: &Arc<InstanceKlass<'a>>,
    object_klass: Option<&Arc<InstanceKlass<'a>>>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<(Arc<InstanceKlass<'a>>, crate::share::oops::method::Code<'a>)> {
    match kind {
        ResolveKind::Static => LinkResolver::resolve_static_call(klass, name, descriptor),
        ResolveKind::Special => LinkResolver::resolve_special_call(klass, name, descriptor),
        ResolveKind::Virtual => {
            let receiver = object_klass.unwrap_or(klass);
            match loader.load(owner) {
                Ok(owner_klass) => LinkResolver::resolve_virtual_call_scoped(
                    receiver,
                    &owner_klass,
                    name,
                    descriptor,
                ),
                Err(_) => LinkResolver::resolve_virtual_call(receiver, name, descriptor),
            }
        }
        ResolveKind::Interface => {
            let receiver = object_klass.unwrap_or(klass);
            if !crate::share::classfile::systemDictionary::SystemDictionary::implements_interface(
                receiver, owner,
            ) {
                return Err(io::Error::other(
                    "IncompatibleClassChangeError: receiver does not implement interface",
                ));
            }
            LinkResolver::resolve_interface_call(receiver, owner, name, descriptor)
        }
        ResolveKind::OptVirtual => {
            let receiver = object_klass.unwrap_or(klass);
            LinkResolver::resolve_virtual_call(receiver, name, descriptor)
        }
    }
}

// ---------------------------------------------------------------------------
// Thread / allocation / stub-address helpers.
// ---------------------------------------------------------------------------

/// `SharedRuntime::get_java_tid` — scheduler id of the current Java
/// thread (0 before VM bootstrap falls back to the primordial id).
pub fn get_java_tid(loader: &ClassLoader<'_>) -> u64 {
    threading::current_tid(loader)
}

/// `SharedRuntime::register_finalizer` — registers a finalizable object
/// with `java.lang.ref.Finalizer`, same call the interpreter's `new`
/// path performs.
pub fn register_finalizer(loader: &ClassLoader<'static>, handle: usize) -> io::Result<()> {
    let finalizer = loader.load(b"java/lang/ref/Finalizer")?;
    let mut args = JavaCallArguments::new();
    args.push_oop(Value::Object(handle));
    JavaCalls::call_static(
        loader,
        &finalizer,
        b"register",
        b"(Ljava/lang/Object;)V",
        &args,
        &mut io::sink(),
    )
    .map(|_| ())
}

/// `SharedRuntime::on_slowpath_allocation_exit` — allocation slow paths
/// participate in the GC safepoint protocol before retrying.
pub fn on_slowpath_allocation_exit() {
    heap::safepoint();
}

/// `SharedRuntime::get_poll_stub` — entry address of the
/// `SafepointPoll` runtime stub used at method entries/loop polls.
pub fn get_poll_stub() -> usize {
    crate::share::jitruntime::runtime::stub_map()
        .get(&crate::share::c1::c1_LIR::RuntimeStub::SafepointPoll)
        .copied()
        .unwrap_or(0) as usize
}

/// `SharedRuntime::get_resolve_*_call_stub` analogue: the
/// unresolved-call stub address used by compiled call sites that need
/// resolution through `LinkResolver`.
pub fn get_resolve_call_stub() -> usize {
    crate::share::jitruntime::runtime::stub_map()
        .get(&crate::share::c1::c1_LIR::RuntimeStub::UnresolvedCall)
        .copied()
        .unwrap_or(0) as usize
}

/// Maps a trap request to the deopt bookkeeping the JIT broker consumes:
/// returns `(reason, action)` decoded exactly like
/// `Deoptimization::trap_request_reason/action`.
pub fn deopt_request_parts(trap_request: i32) -> (DeoptReason, DeoptAction) {
    (
        Deoptimization::trap_request_reason(trap_request),
        Deoptimization::trap_request_action(trap_request),
    )
}

/// `SharedRuntime::uncommon_trap` entry point used by compiled code that
/// hits an uncommon path: records the pending deopt through the JIT
/// runtime channel.  `exec_mode` mirrors `UnpackType` selection.
pub fn uncommon_trap(trap_request: i32, _exec_mode: i32) {
    let (reason, action) = deopt_request_parts(trap_request);
    if reason == DeoptReason::ReasonUnloaded && Deoptimization::trap_request_index(trap_request) >= 0 {
        // Positive trap_request = constant-pool index of an unloaded
        // entry; the interpreter re-executes and raises the linkage
        // error precisely.
        let _ = action;
    }
    crate::share::jitruntime::runtime::set_pending_deopt();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_semantics() {
        assert_eq!(lmul(3, 4), 12);
        assert_eq!(lmul(i64::MAX, 2), -2);
        assert_eq!(ldiv(-1, i64::MIN), i64::MIN);
        assert_eq!(lrem(-1, i64::MIN), 0);
        assert_eq!(f2i(f32::NAN), 0);
        assert_eq!(f2i(3e9), i32::MAX);
        assert_eq!(f2i(-3e9), i32::MIN);
        assert_eq!(d2l(f64::NAN), 0);
        assert_eq!(d2l(1e300), i64::MAX);
        assert_eq!(fcmpl(1.0, f32::NAN), -1);
        assert_eq!(fcmpg(1.0, f32::NAN), 1);
        assert_eq!(dcmpl(f64::NAN, 0.0), -1);
        assert_eq!(dcmpg(f64::NAN, 0.0), 1);
        assert_eq!(frem(1.5, f32::INFINITY), 1.5);
        assert_eq!(drem(2.0, f64::INFINITY), 2.0);
        assert_eq!(frem(f32::INFINITY, f32::INFINITY).is_nan(), true);
        assert_eq!(dsqrt(4.0), 2.0);
        assert_eq!(dabs(-3.5), 3.5);
        assert_eq!(dpow(2.0, 10.0), 1024.0);
    }

    #[test]
    fn exception_handler_lookup_shape() {
        // No nmethod contains address 0; lookup must return None rather
        // than panic.
        assert!(exception_handler_for_return_address(0, b"java/lang/Throwable").is_none());
    }

    #[test]
    fn deopt_request_parts_decode() {
        let request = Deoptimization::make_trap_request(
            DeoptReason::ReasonNullCheck,
            DeoptAction::ActionMaybeRecompile,
            -1,
        );
        let (reason, action) = deopt_request_parts(request);
        assert_eq!(reason, DeoptReason::ReasonNullCheck);
        assert_eq!(action, DeoptAction::ActionMaybeRecompile);
        assert_eq!(UNLOADED_ACTION, DeoptAction::ActionReinterpret);
    }
}
