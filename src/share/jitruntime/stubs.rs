/*
 * Runtime stubs for JIT-compiled code (HotSpot `runtime/` stubs
 * analogue), Windows x64.
 *
 * Every `RuntimeStub` variant gets a real address in `stub_map()`.
 * The address points at a small machine-code *trampoline* emitted
 * into an RWX arena (`publish_executable`); the trampoline
 *
 *   1. reserves the 32-byte Win64 shadow space and calls the matching
 *      `extern "C"` Rust handler.  rcx/rdx/r8/r9 carry the lowered
 *      operands and rbp carries the JIT frame base (5th argument),
 *   2. saves the handler's rax result, then calls `stub_flow_check`
 *      which reports whether a pending outcome (thrown throwable or
 *      deopt request) was recorded in TLS,
 *   3. if no outcome is pending the trampoline restores rax and
 *      returns to the JIT call site,
 *   4. otherwise it abandons the JIT frame that called the stub:
 *      `jit_unwind` performs that frame's epilogue (`rsp = rbp + 16`,
 *      `rbp = [rbp]`) and jumps to its return address — always the
 *      post-call landing pad inside `runtime::invoke_raw`, because
 *      every JIT activation is entered through `call rax` there.
 *      `try_invoke` consumes the pending outcome and reports it to
 *      its invoker; when that invoker is itself a `CallMethod`
 *      handler running under another JIT frame, the handler re-arms
 *      the pending channel and the next trampoline unwinds the next
 *      frame — so nested JIT→JIT and JIT→stub→interpreter→JIT
 *      chains cascade correctly to the outermost `try_invoke`.
 *
 * Heap handlers resolve constant-pool sites through the leaked
 * `StubSite` pointer carried as a `ConstI64` operand, operate on the
 * same handle-based object arenas as the interpreter
 * (`ClassLoader::borrow_objects`/`arrays`/`obj_arrays`) and publish
 * every statically-allocated frame slot range as a conservative GC
 * root set for the duration of the call.
 */

use super::runtime::{
    self, REF_HANDLE_MASK, REF_TAG_OBJECT_ARRAY, REF_TAG_PRIMITIVE_ARRAY,
    decode_ref, encode_ref,
};
use crate::share::c1::asm::codeBuffer::CodeBuffer;
use crate::share::c1::c1_LIR::{RuntimeStub, StubSite};
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::systemDictionary::SystemDictionary;
use crate::share::code::codeCache::CodeCache;
use crate::share::interpreter::bytecodeInterpreter::{self, Value};
use crate::share::interpreter::linkResolver::LinkResolver;
use crate::share::memory::heap;
use crate::share::oops::arrayKlass::ArrayKlass;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::typeArrayOop::Element;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use crate::share::runtime::threading;
use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

/// Same poll throttle as the interpreter safepoint cadence.
const SAFEPOINT_POLL_STRIDE: u32 = 256;

/// Process-wide arena keeping emitted stub trampolines and installed
/// nmethod code alive.  Like HotSpot's code cache, pages are never
/// returned to the OS once published (sweeping marks nmethods
/// non-entrant/zombie but does not unmap in this port).
static EXEC_ARENA: OnceLock<Mutex<Vec<CodeBuffer>>> = OnceLock::new();

fn arena() -> &'static Mutex<Vec<CodeBuffer>> {
    EXEC_ARENA.get_or_init(|| Mutex::new(Vec::new()))
}

/// Metadata `try_invoke` needs to root a compiled frame: how many
/// 8-byte slots below `rbp` may contain references.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExecutableMeta {
    pub frame_qwords: usize,
}

static EXEC_META: OnceLock<Mutex<BTreeMap<usize, ExecutableMeta>>> = OnceLock::new();

fn exec_meta() -> &'static Mutex<BTreeMap<usize, ExecutableMeta>> {
    EXEC_META.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Publishes `code` with explicit frame metadata and registers `meta`
/// for the returned entry address.  Returns `None` when the arena
/// allocation failed.
pub fn publish_executable_meta(code: &[u8], reserve: usize, meta: ExecutableMeta) -> Option<usize> {
    let want = reserve.max(code.len()).max(4096);
    let buf = CodeBuffer::new(want).ok()?;
    buf.write(code).ok()?;
    buf.flush_icache();
    let addr = buf.base() as usize;
    arena().lock().unwrap().push(buf);
    exec_meta().lock().unwrap().insert(addr, meta);
    Some(addr)
}

/// Number of frame qwords registered for the nmethod at `addr`
/// (0 for unknown addresses — conservative rooting then scans
/// nothing, matching the previous primitive-only behaviour).
pub fn frame_qwords_for(addr: usize) -> usize {
    exec_meta()
        .lock()
        .unwrap()
        .get(&addr)
        .map(|m| m.frame_qwords)
        .unwrap_or(0)
}

/// Parses the standard prologue (`push rbp; mov rbp,rsp`, then any
/// callee-saved `push`es, then `sub rsp,imm`) emitted by
/// `X86Emitter::emit_prologue` to recover the total frame size in
/// qwords.  The C2 emitter pushes saved registers between `mov rbp,rsp`
/// and `sub rsp`, so the scan must skip push encodings (`[REX] 50+r`)
/// instead of expecting `sub` immediately at offset 4.
fn prologue_frame_qwords(code: &[u8]) -> usize {
    if code.len() < 4 || code[0] != 0x55 || code[1] != 0x48 || code[2] != 0x89 || code[3] != 0xe5 {
        return 0;
    }
    let mut i = 4;
    let mut pushes = 0usize;
    loop {
        // Skip an optional REX prefix (0x40-0x4f) — emit_push only emits
        // one for r8-r15, but accept any here.
        let mut j = i;
        if j < code.len() && (0x40..=0x4f).contains(&code[j]) {
            j += 1;
        }
        if j < code.len() && (0x50..=0x57).contains(&code[j]) {
            pushes += 1;
            i = j + 1;
            continue;
        }
        break;
    }
    if i + 7 <= code.len() && code[i] == 0x48 && code[i + 1] == 0x81 && code[i + 2] == 0xec {
        let size = i32::from_le_bytes(code[i + 3..i + 7].try_into().unwrap()) as usize;
        return pushes + size / 8;
    }
    if i + 4 <= code.len() && code[i] == 0x48 && code[i + 1] == 0x83 && code[i + 2] == 0xec {
        return pushes + (code[i + 3] as usize) / 8;
    }
    pushes
}

/// Publishes `code` at offset 0 of a fresh RWX region reserved to at
/// least `reserve` bytes and returns the entry address.  The backing
/// `CodeBuffer` is kept in the arena so the pages stay mapped and
/// executable for the lifetime of the VM.  Frame metadata is derived
/// from the prologue so `try_invoke` can root compiled reference
/// slots during runtime-stub calls.
pub fn publish_executable(code: &[u8], reserve: usize) -> Option<usize> {
    let meta = ExecutableMeta {
        frame_qwords: prologue_frame_qwords(code),
    };
    publish_executable_meta(code, reserve, meta)
}

// ---------------------------------------------------------------------------
// Conservative GC rooting for compiled frames.
// ---------------------------------------------------------------------------

/// RAII guard that publishes the JIT frame's reference-capable slots
/// to `ClassLoader::gc_extra_roots` for the duration of a runtime
/// stub call.  The compiled frame has no live-PC map, so every
/// 8-byte slot below `rbp` (locals, operand stack, temps and frame
/// padding) is scanned conservatively: any word that decodes as a
/// valid tagged reference inside the current arena bounds is treated
/// as a root.  False positives only retain objects longer; they can
/// never free a live one.
struct FrameRoots {
    loader: Arc<ClassLoader<'static>>,
    start: usize,
    values: Vec<Value<'static>>,
}

impl FrameRoots {
    fn push(&mut self, value: Value<'static>) {
        self.loader.gc_extra_roots.borrow_mut().push(value);
        self.values.push(value);
    }
}

impl Drop for FrameRoots {
    fn drop(&mut self) {
        let mut roots = self.loader.gc_extra_roots.borrow_mut();
        let expected = self.start + self.values.len();
        if roots.len() == expected
            && roots[self.start..] == self.values[..]
        {
            roots.truncate(self.start);
            return;
        }
        for value in self.values.iter().rev() {
            if let Some(pos) = roots.iter().rposition(|root| root == value) {
                roots.remove(pos);
            }
        }
    }
}

/// Scans `frame_qwords` slots below the JIT `rbp` (`frame`) and pushes
/// every well-formed tagged reference onto `gc_extra_roots`.  Returns
/// `None` when there is nothing to protect (no loader, empty frame or
/// no candidate roots) so callers can use `let _roots = ...` cheaply.
fn frame_roots(loader: &Arc<ClassLoader<'static>>, frame: u64) -> Option<FrameRoots> {
    let qwords = runtime::current_frame_qwords();
    if frame == 0 || qwords == 0 {
        return None;
    }
    let objects = loader.borrow_objects().len();
    let arrays = loader.arrays.borrow().len();
    let obj_arrays = loader.obj_arrays.borrow().len();
    let mut roots = FrameRoots {
        loader: Arc::clone(loader),
        start: loader.gc_extra_roots.borrow().len(),
        values: Vec::new(),
    };
    for i in 0..qwords {
        let addr = frame.wrapping_sub(8 + (i as u64) * 8);
        let raw = unsafe { (addr as *const u64).read_unaligned() };
        if raw == 0 {
            continue;
        }
        let tag = raw >> 48;
        let slot = raw & REF_HANDLE_MASK;
        if slot == 0 {
            continue;
        }
        let handle = (slot - 1) as usize;
        let in_bounds = match tag {
            0 => handle < objects,
            REF_TAG_OBJECT_ARRAY => handle < obj_arrays,
            REF_TAG_PRIMITIVE_ARRAY => handle < arrays,
            _ => false,
        };
        if in_bounds {
            roots.push(decode_ref(raw));
        }
    }
    if roots.values.is_empty() {
        return None;
    }
    Some(roots)
}

// ---------------------------------------------------------------------------
// Shared stub helpers.
// ---------------------------------------------------------------------------

/// Handler signature: rcx/rdx/r8/r9 carry the lowered operands and
/// rbp (the JIT frame base) is passed as the fifth argument so heap
/// handlers can root the frame.  The return value is the raw rax
/// result; exceptional exits are signalled exclusively through the
/// pending-outcome channel checked by the trampoline.
type StubHandler = unsafe extern "C" fn(u64, u64, u64, u64, u64) -> u64;

/// Trampoline-side flow check: returns 1 when a pending outcome was
/// recorded (the trampoline then unwinds to `invoke_raw`), 0 to
/// continue in JIT code with rax as the stub result.
extern "C" fn stub_flow_check() -> u64 {
    u64::from(runtime::pending_active())
}

fn deopt() -> u64 {
    runtime::set_pending_deopt();
    0
}

fn loader_or_deopt() -> Option<Arc<ClassLoader<'static>>> {
    match threading::jvm_loader() {
        Some(loader) => Some(loader),
        None => {
            runtime::set_pending_deopt();
            None
        }
    }
}

/// Borrows the site triple (owner class, member name, descriptor)
/// from a leaked `StubSite` pointer operand.  `None` means the
/// constant-pool index was never resolved — always a deopt.
fn site_slices(ptr: u64) -> Option<(&'static [u8], &'static [u8], &'static [u8])> {
    if ptr == 0 {
        return None;
    }
    let site = unsafe { &*(ptr as *const StubSite) };
    Some(unsafe { site.as_slices() })
}

/// Converts an interpreter `Value` read from the heap into the raw
/// word a compiled dst slot expects.  Reference values go through the
/// tagged encoding; lightweight strings and class mirrors are
/// materialized into real heap objects first so they can cross the
/// boundary (mirrors `bytecodeInterpreter`'s materialization on
/// field/call boundaries).
fn heap_value_to_raw(loader: &ClassLoader<'static>, value: &Value<'static>) -> Option<u64> {
    match value {
        Value::Int(v) => Some(*v as u32 as u64),
        Value::Long(v) => Some(*v as u64),
        Value::Float(f) => Some(f.to_bits() as u64),
        Value::Double(d) => Some(d.to_bits()),
        _ => {
            if let Some(raw) = encode_ref(value) {
                return Some(raw);
            }
            let depth = runtime::current_depth();
            match value {
                Value::String(_) | Value::HostString(_) => {
                    let materialized = crate::share::prims::stringNatives::materialize(
                        loader,
                        *value,
                        &mut io::sink(),
                        depth + 1,
                    )
                    .ok()??;
                    encode_ref(&materialized)
                }
                Value::ClassMirror(_) => {
                    let materialized =
                        crate::share::prims::classNatives::materialize_class(loader, *value)
                            .ok()??;
                    encode_ref(&materialized)
                }
                _ => None,
            }
        }
    }
}

/// Converts a raw operand word into the `Value` stored into an
/// instance/static field or primitive array, applying the JVM
/// narrowing rules for `Z`/`B`/`C`/`S` fields exactly like the
/// interpreter's `putfield`/`putstatic` paths.
fn raw_to_field_value(raw: u64, descriptor: &[u8]) -> Value<'static> {
    match descriptor.first().copied() {
        Some(b'J') => Value::Long(raw as i64),
        Some(b'F') => Value::Float(f32::from_bits(raw as u32)),
        Some(b'D') => Value::Double(f64::from_bits(raw)),
        Some(b'L') | Some(b'[') => decode_ref(raw),
        Some(b'Z') => Value::Int((raw as u32 as i32) & 1),
        Some(b'B') => Value::Int(i32::from((raw as u32) as u8 as i8)),
        Some(b'C') => Value::Int(i32::from((raw as u32) as u16)),
        Some(b'S') => Value::Int(i32::from((raw as u32 as i32) as i16)),
        _ => Value::Int(raw as u32 as i32),
    }
}

/// Maps an allocation failure to the JVM exception the interpreter
/// would raise: `NegativeArraySizeException` for negative lengths,
/// otherwise a linkage/deopt fallback (the interpreter reproduces
/// the precise linkage error on re-execution).
fn alloc_error(error: &io::Error) -> u64 {
    if error.to_string().contains("Negative") {
        return throw_java(
            b"java/lang/NegativeArraySizeException",
            Some(&error.to_string()),
        );
    }
    deopt()
}

/// Runs `<clinit>` for `target` and translates failures into the
/// pending channel.  Throwable-carrying errors become pending throws;
/// plain errors deopt so the interpreter raises them precisely.
fn initialize_or_fail(
    loader: &Arc<ClassLoader<'static>>,
    target: &Arc<crate::share::oops::instanceKlass::InstanceKlass<'static>>,
) -> u64 {
    let depth = runtime::current_depth();
    match bytecodeInterpreter::initialize(loader, target, &mut io::sink(), depth + 1) {
        Ok(()) => 0,
        Err(error) => {
            if let Some(handle) = bytecodeInterpreter::throwable_of(&error) {
                runtime::set_pending_throw(handle);
            } else {
                runtime::set_pending_deopt();
            }
            1
        }
    }
}

/// Builds a real throwable object for `class_name` through the
/// interpreter's object arena and records it as the pending outcome.
/// Falls back to deoptimization when the throwable cannot be
/// materialized (e.g. bootstrap without a loader) so the interpreter
/// raises the failure itself.
fn throw_java(class_name: &'static [u8], message: Option<&str>) -> u64 {
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return 0;
    };
    let depth = runtime::current_depth();
    let mut sink = io::sink();
    match bytecodeInterpreter::throwableObject(
        &loader,
        &mut sink,
        depth,
        class_name,
        message,
    ) {
        Ok(handle) => {
            runtime::set_pending_throw(handle);
        }
        Err(_) => {
            runtime::set_pending_deopt();
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Exception / control-flow stubs.
// ---------------------------------------------------------------------------

thread_local! {
    static SAFEPOINT_COUNTDOWN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// `SafepointPoll`: lets a running compiled loop participate in the
/// stop-the-world GC protocol.  Throttled like the interpreter's
/// safepoint cadence.  The frame roots guard keeps every reference
/// slot visible to the GC while the world is stopped.
unsafe extern "C" fn h_safepoint_poll(_a: u64, _b: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let fire = SAFEPOINT_COUNTDOWN.with(|c| {
        let v = c.get();
        if v <= 1 {
            c.set(SAFEPOINT_POLL_STRIDE);
            true
        } else {
            c.set(v - 1);
            false
        }
    });
    if fire {
        let _roots = threading::jvm_loader().and_then(|loader| frame_roots(&loader, frame));
        heap::safepoint();
    }
    0
}

unsafe extern "C" fn h_arithmetic_exception(_a: u64, _b: u64, _c: u64, _d: u64, _f: u64) -> u64 {
    throw_java(b"java/lang/ArithmeticException", Some("/ by zero"))
}

unsafe extern "C" fn h_null_pointer_exception(_a: u64, _b: u64, _c: u64, _d: u64, _f: u64) -> u64 {
    throw_java(b"java/lang/NullPointerException", None)
}

unsafe extern "C" fn h_array_index_out_of_bounds(_a: u64, _b: u64, _c: u64, _d: u64, _f: u64) -> u64 {
    throw_java(b"java/lang/ArrayIndexOutOfBoundsException", None)
}

unsafe extern "C" fn h_array_store_exception(_a: u64, _b: u64, _c: u64, _d: u64, _f: u64) -> u64 {
    throw_java(b"java/lang/ArrayStoreException", None)
}

/// `athrow`: arg0 is the tagged throwable reference (0 == null,
/// which the JVM specifies must raise NullPointerException instead).
/// The throwable must resolve to a `java.lang.Throwable` subclass,
/// matching the interpreter's VerifyError check.
unsafe extern "C" fn h_throw(obj: u64, _b: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let _roots = threading::jvm_loader().and_then(|loader| frame_roots(&loader, frame));
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let Some(loader) = loader_or_deopt() else { return 0 };
    let handle = match decode_ref(obj) {
        Value::Object(handle) => handle,
        _ => return deopt(),
    };
    let is_throwable = {
        let objects = loader.borrow_objects();
        objects.get(handle).is_some_and(|object| {
            object.klass.class.name == b"java/lang/Throwable"
                || SystemDictionary::is_subclass_of(&object.klass, b"java/lang/Throwable")
        })
    };
    if is_throwable {
        runtime::set_pending_throw(handle);
    } else {
        throw_java(b"java/lang/VerifyError", Some("athrow operand is not a Throwable"));
    }
    0
}

/// `Deoptimize`: explicit uncommon-trap request (used by stubs that
/// cannot service the operation natively).
unsafe extern "C" fn h_deoptimize(_a: u64, _b: u64, _c: u64, _d: u64, _f: u64) -> u64 {
    runtime::set_pending_deopt();
    0
}

/// Monitor enter/exit use `monitorKey` on the decoded reference so
/// the synchronizer keys agree with the interpreter's regardless of
/// whether the value is an object or an array.
unsafe extern "C" fn h_monitor_enter(obj: u64, _b: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return 0;
    };
    let _roots = frame_roots(&loader, frame);
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let value = decode_ref(obj);
    let Some(key) = bytecodeInterpreter::monitorKey(value) else {
        return deopt();
    };
    let tid = threading::current_tid(&loader);
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
    0
}

unsafe extern "C" fn h_monitor_exit(obj: u64, _b: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return 0;
    };
    let _roots = frame_roots(&loader, frame);
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let value = decode_ref(obj);
    let Some(key) = bytecodeInterpreter::monitorKey(value) else {
        return deopt();
    };
    let tid = threading::current_tid(&loader);
    let result = loader
        .synchronizer
        .borrow_mut()
        .exit(key as usize, tid);
    match result {
        Ok(_) => 0,
        Err(message) => throw_java(
            b"java/lang/IllegalMonitorStateException",
            Some(message),
        ),
    }
}

// ---------------------------------------------------------------------------
// Field / static access.
// ---------------------------------------------------------------------------

/// Shared resolution+access for `getfield`/`putfield`: arg0 is the
/// tagged receiver, arg1 the `StubSite` pointer and (for putfield)
/// arg2 the raw value word.  Mirrors the interpreter's ordering:
/// NPE on null, ICCE on non-object receivers, subclass check against
/// the field owner, then slot resolution and narrowed store.
unsafe extern "C" fn h_get_field(obj: u64, site_ptr: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return 0;
    };
    let _roots = frame_roots(&loader, frame);
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let handle = match decode_ref(obj) {
        Value::Object(handle) => handle,
        _ => {
            return throw_java(
                b"java/lang/IncompatibleClassChangeError",
                Some("Expected object field receiver"),
            );
        }
    };
    let Some((owner, name, descriptor)) = site_slices(site_ptr) else {
        return deopt();
    };
    let index = {
        let mut objects = loader.borrow_objects_mut();
        let Some(object) = objects.get_mut(handle) else {
            return deopt();
        };
        if !SystemDictionary::is_subclass_of(&object.klass, owner) {
            drop(objects);
            return throw_java(
                b"java/lang/IncompatibleClassChangeError",
                Some("Field receiver class mismatch"),
            );
        }
        let Ok(owner_klass) = loader.load(owner) else {
            return deopt();
        };
        let Ok(index) = owner_klass.instanceField(name, descriptor) else {
            return deopt();
        };
        if index >= object.fields.len() {
            return deopt();
        }
        index
    };
    let value = loader.borrow_objects()[handle].fields[index];
    match heap_value_to_raw(&loader, &value) {
        Some(raw) => raw,
        None => deopt(),
    }
}

unsafe extern "C" fn h_put_field(obj: u64, site_ptr: u64, value: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return 0;
    };
    let _roots = frame_roots(&loader, frame);
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let handle = match decode_ref(obj) {
        Value::Object(handle) => handle,
        _ => {
            return throw_java(
                b"java/lang/IncompatibleClassChangeError",
                Some("Expected object field receiver"),
            );
        }
    };
    let Some((owner, name, descriptor)) = site_slices(site_ptr) else {
        return deopt();
    };
    let index = {
        let mut objects = loader.borrow_objects_mut();
        let Some(object) = objects.get_mut(handle) else {
            return deopt();
        };
        if !SystemDictionary::is_subclass_of(&object.klass, owner) {
            drop(objects);
            return throw_java(
                b"java/lang/IncompatibleClassChangeError",
                Some("Field receiver class mismatch"),
            );
        }
        let Ok(owner_klass) = loader.load(owner) else {
            return deopt();
        };
        let Ok(index) = owner_klass.instanceField(name, descriptor) else {
            return deopt();
        };
        if index >= object.fields.len() {
            return deopt();
        }
        index
    };
    let stored = raw_to_field_value(value, descriptor);
    let mut objects = loader.borrow_objects_mut();
    let Some(object) = objects.get_mut(handle) else {
        return deopt();
    };
    if index >= object.fields.len() {
        return deopt();
    }
    object.fields[index] = stored;
    0
}

/// Shared resolution for `getstatic`/`putstatic`: arg0 is the
/// `StubSite` pointer, arg1 (putstatic only) the raw value word.
/// Resolves the actual static-field owner, runs `<clinit>` and then
/// reads/writes the `Share<Value>` cell — the same path the
/// interpreter takes for 0xb2/0xb3.
unsafe fn static_field_access(
    site_ptr: u64,
    value: Option<u64>,
    frame: u64,
    loader: &Arc<ClassLoader<'static>>,
) -> u64 {
    let _roots = frame_roots(loader, frame);
    let Some((owner, name, descriptor)) = site_slices(site_ptr) else {
        return deopt();
    };
    if value.is_none()
        && owner == b"jdk/internal/misc/UnsafeConstants"
        && let Some(v) = crate::share::prims::r#unsafe::Unsafe::platformConstant(name, descriptor)
    {
        return v as u32 as u64;
    }
    let target = match loader
        .load(owner)
        .and_then(|klass| klass.staticFieldOwner(name, descriptor))
    {
        Ok(target) => target,
        Err(_) => return deopt(),
    };
    let field_cell = match target.staticField(name, descriptor) {
        Ok(cell) => cell,
        Err(_) => return deopt(),
    };
    let failed = initialize_or_fail(loader, &target);
    if failed != 0 {
        return 0;
    }
    match value {
        Some(raw) => {
            field_cell.set(raw_to_field_value(raw, descriptor));
            0
        }
        None => {
            let field_value = field_cell.get();
            match heap_value_to_raw(loader, &field_value) {
                Some(raw) => raw,
                None => deopt(),
            }
        }
    }
}

unsafe extern "C" fn h_get_static(site_ptr: u64, _b: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    unsafe { static_field_access(site_ptr, None, frame, &loader) }
}

unsafe extern "C" fn h_put_static(site_ptr: u64, value: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    unsafe { static_field_access(site_ptr, Some(value), frame, &loader) }
}

// ---------------------------------------------------------------------------
// Allocation.
// ---------------------------------------------------------------------------

/// `new`: arg0 is the `StubSite` carrying the internal class name.
/// Mirrors the interpreter: abstract/interface check, `<clinit>`,
/// `java.lang.ref.Finalizer` registration for finalizable classes.
unsafe extern "C" fn h_new_instance(site_ptr: u64, _b: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    let Some((class, _name, _desc)) = site_slices(site_ptr) else {
        return deopt();
    };
    let target = match loader.load(class) {
        Ok(target) => target,
        Err(_) => return deopt(),
    };
    if (target.class.accessFlags & 0x0600) != 0 {
        return throw_java(
            b"java/lang/InstantiationError",
            Some("Cannot instantiate abstract class or interface"),
        );
    }
    let failed = initialize_or_fail(&loader, &target);
    if failed != 0 {
        return 0;
    }
    let needs_finalizer = target.has_finalizer();
    let object = match InstanceOop::new(target.clone()) {
        Ok(object) => object,
        Err(_) => return deopt(),
    };
    let handle = loader.alloc_object(object);
    if needs_finalizer {
        let register = (|| -> io::Result<()> {
            let finalizer = loader.load(b"java/lang/ref/Finalizer")?;
            let mut args = JavaCallArguments::new();
            args.push_oop(Value::Object(handle));
            JavaCalls::call_static(
                &loader,
                &finalizer,
                b"register",
                b"(Ljava/lang/Object;)V",
                &args,
                &mut io::sink(),
            )
            .map(|_| ())
        })();
        if let Err(error) = register {
            if let Some(th) = bytecodeInterpreter::throwable_of(&error) {
                runtime::set_pending_throw(th);
            } else {
                runtime::set_pending_deopt();
            }
            return 0;
        }
    }
    match encode_ref(&Value::Object(handle)) {
        Some(raw) => raw,
        None => deopt(),
    }
}

/// `newarray`: arg0 is the length, arg1 the JVM element-type code
/// (4=boolean .. 11=long, matching `TypeArrayOop::new`).
unsafe extern "C" fn h_new_array(len: u64, kind: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    let length = len as u32 as i32;
    match ArrayKlass::primitive(&loader, kind as u8, length) {
        Ok(value) => match encode_ref(&value) {
            Some(raw) => raw,
            None => deopt(),
        },
        Err(error) => alloc_error(&error),
    }
}

/// `anewarray`: arg0 is the `StubSite` carrying the component class
/// name, arg1 the length.
unsafe extern "C" fn h_anew_array(site_ptr: u64, len: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    let Some((component, _name, _desc)) = site_slices(site_ptr) else {
        return deopt();
    };
    if component.is_empty() {
        return deopt();
    }
    let length = len as u32 as i32;
    match ArrayKlass::reference(&loader, component, length) {
        Ok(value) => match encode_ref(&value) {
            Some(raw) => raw,
            None => deopt(),
        },
        Err(error) => alloc_error(&error),
    }
}

/// `multianewarray` needs a variable-length dimension list the
/// four-register convention cannot carry; the graph builder already
/// rejects the opcode, so this stays an uncommon trap.
unsafe extern "C" fn h_multianew_array(_a: u64, _b: u64, _c: u64, _d: u64, _f: u64) -> u64 {
    runtime::set_pending_deopt();
    0
}

// ---------------------------------------------------------------------------
// Casts / type checks.
// ---------------------------------------------------------------------------

/// `checkcast`: arg0 tagged value, arg1 `StubSite` with the target
/// class.  Null passes through unchanged; `referenceAssignable`
/// decides compatibility exactly like the interpreter.
unsafe extern "C" fn h_check_cast(value: u64, site_ptr: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    if value == 0 {
        return 0;
    }
    let Some((target, _name, _desc)) = site_slices(site_ptr) else {
        return deopt();
    };
    let v = decode_ref(value);
    match loader.referenceAssignable(v, target) {
        Ok(true) => value,
        Ok(false) => throw_java(
            b"java/lang/ClassCastException",
            Some(&format!(
                "cannot cast to {}",
                String::from_utf8_lossy(target).replace('/', ".")
            )),
        ),
        Err(_) => deopt(),
    }
}

/// `instanceof`: arg0 tagged value, arg1 `StubSite`; returns 0/1.
unsafe extern "C" fn h_instance_of(value: u64, site_ptr: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    if value == 0 {
        return 0;
    }
    let Some((target, _name, _desc)) = site_slices(site_ptr) else {
        return deopt();
    };
    let v = decode_ref(value);
    match loader.referenceAssignable(v, target) {
        Ok(assignable) => u64::from(assignable),
        Err(_) => deopt(),
    }
}

unsafe extern "C" fn h_unresolved_field(_a: u64, _b: u64, _c: u64, _d: u64, _f: u64) -> u64 {
    runtime::set_pending_deopt();
    0
}

unsafe extern "C" fn h_unresolved_call(a: u64, b: u64, c: u64, d: u64, f: u64) -> u64 {
    unsafe { h_call_method(a, b, c, d, f) }
}

// ---------------------------------------------------------------------------
// Array operations.
// ---------------------------------------------------------------------------

/// `arraylength`: arg0 tagged array reference.
unsafe extern "C" fn h_array_length(array: u64, _b: u64, _c: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    if array == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    match decode_ref(array) {
        Value::PrimitiveArray(handle) => {
            let arrays = loader.arrays.borrow();
            match arrays.get(handle) {
                Some(a) => a.len() as u64,
                None => deopt(),
            }
        }
        Value::ObjectArray(handle) => {
            let arrays = loader.obj_arrays.borrow();
            match arrays.get(handle) {
                Some(a) => a.len() as u64,
                None => deopt(),
            }
        }
        _ => throw_java(
            b"java/lang/IncompatibleClassChangeError",
            Some("arraylength on non-array"),
        ),
    }
}

fn index_out_of_bounds() -> u64 {
    throw_java(b"java/lang/ArrayIndexOutOfBoundsException", None)
}

/// `*aload`: args are (array, index, kind).  kind 12 selects the
/// object-array path (`aaload`); every other code is a
/// `TypeArrayOop::load` element kind.
unsafe extern "C" fn h_array_load(array: u64, index: u64, kind: u64, _d: u64, frame: u64) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    if array == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let idx = index as u32 as i32 as usize;
    if kind == 12 {
        let handle = match decode_ref(array) {
            Value::ObjectArray(handle) => handle,
            _ => return throw_java(b"java/lang/NullPointerException", None),
        };
        let value = {
            let arrays = loader.obj_arrays.borrow();
            let Some(a) = arrays.get(handle) else {
                return deopt();
            };
            match a.get(idx) {
                Ok(value) => value,
                Err(_) => return index_out_of_bounds(),
            }
        };
        return match heap_value_to_raw(&loader, &value) {
            Some(raw) => raw,
            None => deopt(),
        };
    }
    let handle = match decode_ref(array) {
        Value::PrimitiveArray(handle) => handle,
        _ => return throw_java(b"java/lang/NullPointerException", None),
    };
    let arrays = loader.arrays.borrow();
    let Some(a) = arrays.get(handle) else {
        return deopt();
    };
    if std::env::var_os("JVM25_JIT_OOB").is_some() && idx >= a.len() {
        eprintln!("[arrayload-oob] idx={idx} len={} kind={kind} frame={frame:#x}", a.len());
    }
    match a.load(kind as u8, idx) {
        Ok(element) => match element {
            Element::Int(v) => v as u32 as u64,
            Element::Long(v) => v as u64,
            Element::Float(f) => f.to_bits() as u64,
            Element::Double(d) => d.to_bits(),
        },
        Err(error) => {
            if error.to_string().contains("bounds") {
                index_out_of_bounds()
            } else {
                deopt()
            }
        }
    }
}

/// `*astore`: args are (array, index, value, kind).  kind 12 is the
/// object-array path with the `referenceAssignable` store check;
/// other codes store through `TypeArrayOop::store`.
unsafe extern "C" fn h_array_store(
    array: u64,
    index: u64,
    value: u64,
    kind: u64,
    frame: u64,
) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    if array == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let idx = index as u32 as i32 as usize;
    if kind == 12 {
        let handle = match decode_ref(array) {
            Value::ObjectArray(handle) => handle,
            _ => return throw_java(b"java/lang/NullPointerException", None),
        };
        let stored = decode_ref(value);
        let component = {
            let arrays = loader.obj_arrays.borrow();
            let Some(a) = arrays.get(handle) else {
                return deopt();
            };
            match a.get(idx) {
                Ok(_) => a.component,
                Err(_) => return index_out_of_bounds(),
            }
        };
        match loader.referenceAssignable(stored, component) {
            Ok(true) => {}
            Ok(false) => {
                return throw_java(b"java/lang/ArrayStoreException", None);
            }
            Err(_) => return deopt(),
        }
        let mut arrays = loader.obj_arrays.borrow_mut();
        let Some(a) = arrays.get_mut(handle) else {
            return deopt();
        };
        if a.set(idx, stored).is_err() {
            return deopt();
        }
        return 0;
    }
    let handle = match decode_ref(array) {
        Value::PrimitiveArray(handle) => handle,
        _ => return throw_java(b"java/lang/NullPointerException", None),
    };
    let element = match kind {
        1 => Element::Long(value as i64),
        2 => Element::Float(f32::from_bits(value as u32)),
        3 => Element::Double(f64::from_bits(value)),
        _ => Element::Int(value as u32 as i32),
    };
    let mut arrays = loader.arrays.borrow_mut();
    let Some(a) = arrays.get_mut(handle) else {
        return deopt();
    };
    match a.store(kind as u8, idx, element) {
        Ok(()) => 0,
        Err(error) => {
            if error.to_string().contains("bounds") {
                index_out_of_bounds()
            } else {
                deopt()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Floating-point remainder / conversions (SharedRuntime math intrinsics).
// ---------------------------------------------------------------------------

/// `frem`/`drem`: arg0/arg1 carry the raw operand bit patterns (the f32
/// payload sits in the low 32 bits when kind == 0), arg2 selects single
/// (`0`) or double (`1`) precision.  Java remainder is fmod plus the
/// Win64 infinity rule — `sharedRuntime::{frem,drem}` implement it
/// exactly; the result leaves through the xmm channel like every other
/// floating-point stub result.
unsafe extern "C" fn h_float_rem(a: u64, b: u64, kind: u64, _d: u64, _frame: u64) -> u64 {
    use crate::share::runtime::sharedRuntime;
    let bits = if kind == 0 {
        sharedRuntime::frem(f32::from_bits(a as u32), f32::from_bits(b as u32)).to_bits() as u64
    } else {
        sharedRuntime::drem(f64::from_bits(a), f64::from_bits(b)).to_bits()
    };
    runtime::set_stub_xmm_result(bits);
    bits
}

/// `f2i`/`f2l`/`d2i`/`d2l`: arg0 carries the source bit pattern, arg2
/// selects the conversion (0 = F2I, 1 = F2L, 2 = D2I, 3 = D2L).  The
/// sharedRuntime helpers implement the Java rules (NaN -> 0, saturate
/// on overflow, round toward zero) that a raw `cvttss2si` does not, so
/// the result goes out in rax as a plain GP word.
unsafe extern "C" fn h_float_convert(value: u64, _b: u64, kind: u64, _d: u64, _frame: u64) -> u64 {
    use crate::share::runtime::sharedRuntime;
    match kind {
        0 => u64::from(sharedRuntime::f2i(f32::from_bits(value as u32)) as u32),
        1 => sharedRuntime::f2l(f32::from_bits(value as u32)) as u64,
        2 => u64::from(sharedRuntime::d2i(f64::from_bits(value)) as u32),
        _ => sharedRuntime::d2l(f64::from_bits(value)) as u64,
    }
}

// ---------------------------------------------------------------------------
// Method calls (HotSpot `call`/`stub` analogue).
// ---------------------------------------------------------------------------

/// Upper bound on flattened call arguments the CallMethod stub
/// marshals through its stack argument block.
const MAX_CALL_ARGS: usize = 64;

/// Access flags of the resolved method, found by scanning the holder
/// hierarchy exactly like the interpreter's `methodAccessFlags`.
fn method_access_flags<'a>(
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

/// Converts an interpreter `Value` produced by a callee back into the
/// raw word the JIT dst slot expects; float/double results additionally
/// publish their bit pattern through the xmm channel.
fn call_result_raw(loader: &ClassLoader<'static>, value: &Value<'static>) -> u64 {
    match value {
        Value::Float(f) => {
            runtime::set_stub_xmm_result(*f as u64);
            f.to_bits() as u64
        }
        Value::Double(d) => {
            runtime::set_stub_xmm_result(d.to_bits());
            d.to_bits()
        }
        _ => match heap_value_to_raw(loader, value) {
            Some(raw) => raw,
            None => {
                runtime::set_pending_deopt();
                0
            }
        },
    }
}

/// Receiver-class lookup for virtual/interface dispatch, mirroring the
/// interpreter: objects expose their klass, arrays expose
/// `java.lang.Object`, null receivers raise NPE and anything else is
/// an ICCE.  Returns the raw handler result (0 on success paths that
/// already recorded a pending outcome).
fn receiver_klass(
    loader: &Arc<ClassLoader<'static>>,
    receiver: Value<'static>,
    frame: u64,
) -> Result<Arc<InstanceKlass<'static>>, u64> {
    let _roots = frame_roots(loader, frame);
    match receiver {
        Value::Null => Err(throw_java(b"java/lang/NullPointerException", None)),
        Value::Object(handle) => {
            let objects = loader.borrow_objects();
            match objects.get(handle) {
                Some(object) => Ok(Arc::clone(&object.klass)),
                None => Err(deopt()),
            }
        }
        Value::PrimitiveArray(_) | Value::ObjectArray(_) => match loader.load(b"java/lang/Object") {
            Ok(klass) => Ok(klass),
            Err(_) => Err(deopt()),
        },
        _ => Err(throw_java(
            b"java/lang/IncompatibleClassChangeError",
            Some("Expected object receiver"),
        )),
    }
}

/// `CallMethod`: the generic invoke stub.  rcx is the leaked `StubSite`
/// (owner class / method name / descriptor), rdx points at the
/// caller-built array of raw argument words (receiver first for
/// instance methods), r8 is the word count and r9 the invoke opcode.
/// The handler resolves the target through `LinkResolver`, runs the
/// installed nmethod via `try_invoke` when one exists and otherwise
/// executes the method in `bytecodeInterpreter` — the same dispatch
/// the interpreter performs for `invoke*` bytecodes.
unsafe extern "C" fn h_call_method(
    site_ptr: u64,
    args_ptr: u64,
    n_args: u64,
    opcode: u64,
    frame: u64,
) -> u64 {
    let Some(loader) = loader_or_deopt() else { return 0 };
    let _roots = frame_roots(&loader, frame);
    let Some((owner, name, descriptor)) = site_slices(site_ptr) else {
        return deopt();
    };
    {
        static JIT_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *JIT_TRACE.get_or_init(|| std::env::var_os("JVM25_JIT_TRACE").is_some()) {
            static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u64>>> =
                std::sync::OnceLock::new();
            let seen = SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
            if seen.lock().map(|mut s| s.insert(site_ptr ^ opcode)).unwrap_or(false) {
                eprintln!(
                    "[callmethod-site] site={site_ptr:#x} op={:#x} {}.{}{}",
                    opcode as u8,
                    String::from_utf8_lossy(owner),
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor)
                );
            }
        }
    }
    let op = opcode as u8;
    let is_static = op == 0xb8;
    let n = n_args as usize;
    if n > MAX_CALL_ARGS {
        return deopt();
    }
    let Some(sig) = runtime::parse_signature(descriptor, is_static) else {
        return deopt();
    };
    let expected = sig.params.len() + usize::from(sig.is_instance);
    if expected != n || (n > 0 && args_ptr == 0) {
        if std::env::var_os("JVM25_JIT_DEBUG").is_some() {
            eprintln!("[callmethod] arity mismatch n={n} expected={expected} args_ptr={args_ptr:#x}");
        }
        return deopt();
    }
    let raw_args = if n == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(args_ptr as *const u64, n) }
    };
    // Flatten raw words into interpreter Values; reference arguments
    // are also pinned as GC roots for the duration of the call.
    let mut values: Vec<Value<'static>> = Vec::with_capacity(n);
    let mut arg_roots = FrameRoots {
        loader: Arc::clone(&loader),
        start: loader.gc_extra_roots.borrow().len(),
        values: Vec::new(),
    };
    let mut index = 0usize;
    if sig.is_instance {
        let receiver = decode_ref(raw_args[0]);
        arg_roots.push(receiver);
        values.push(receiver);
        index = 1;
    }
    for kind in &sig.params {
        let raw = raw_args[index];
        index += 1;
        let value = match kind {
            runtime::SlotKind::Int32 => Value::Int(raw as u32 as i32),
            runtime::SlotKind::Int64 => Value::Long(raw as i64),
            runtime::SlotKind::Float32 => Value::Float(f32::from_bits(raw as u32)),
            runtime::SlotKind::Float64 => Value::Double(f64::from_bits(raw)),
            runtime::SlotKind::Ref => decode_ref(raw),
            runtime::SlotKind::Void => return deopt(),
        };
        if matches!(kind, runtime::SlotKind::Ref) {
            arg_roots.push(value);
        }
        values.push(value);
    }
    let callerDepth = runtime::current_depth();
    let dispatched = (|| {
        use crate::share::prims::{jdkIntrinsics, nativeLookup, stringNatives};

        if is_static {
            if let Some(value) = jdkIntrinsics::invoke(&loader, owner, name, descriptor, &values)? {
                return Ok(Some(Some(value)));
            }
            nativeLookup::invokeStatic(&loader, owner, name, descriptor, &values, &mut io::sink(), callerDepth)
        } else {
            if matches!(op, 0xb6 | 0xb9)
                && let Some(value) = stringNatives::invokeVirtual(&loader, owner, name, descriptor, &values)?
            {
                return Ok(Some(value));
            }
            nativeLookup::invokeInstance(&loader, owner, name, descriptor, &values, &mut io::sink(), callerDepth)
        }
    })();
    match dispatched {
        Ok(Some(value)) => return value.map_or(0, |value| call_result_raw(&loader, &value)),
        Ok(None) => {}
        Err(error) => {
            if let Some(handle) = bytecodeInterpreter::throwable_of(&error) {
                runtime::set_pending_throw(handle);
                return 0;
            }
            return deopt();
        }
    }
    let depth = callerDepth + 1;
    // Virtual/interface dispatch needs the receiver's runtime class;
    // resolve it up front so its NPE/ICCE outcomes propagate directly.
    let object_klass = if matches!(op, 0xb6 | 0xb9) {
        let receiver = values.first().copied().unwrap_or(Value::Null);
        match receiver_klass(&loader, receiver, frame) {
            Ok(klass) => Some(klass),
            Err(raw) => return raw,
        }
    } else {
        None
    };
    let resolved = (|| -> io::Result<(Arc<InstanceKlass<'static>>, crate::share::oops::method::Code<'static>)> {
        match op {
            0xb8 => {
                let target = loader.load(owner)?;
                LinkResolver::resolve_static_call(&target, name, descriptor)
            }
            0xb7 => {
                let target = loader.load(owner)?;
                LinkResolver::resolve_special_call(&target, name, descriptor)
            }
            0xb9 => {
                let object_klass = object_klass.as_ref().unwrap();
                if !SystemDictionary::implements_interface(object_klass, owner) {
                    return Err(io::Error::other(
                        "IncompatibleClassChangeError: receiver does not implement interface",
                    ));
                }
                LinkResolver::resolve_interface_call(object_klass, owner, name, descriptor)
            }
            0xb6 => {
                let object_klass = object_klass.as_ref().unwrap();
                match loader.load(owner) {
                    Ok(owner_klass) => LinkResolver::resolve_virtual_call_scoped(
                        object_klass,
                        &owner_klass,
                        name,
                        descriptor,
                    ),
                    Err(_) => LinkResolver::resolve_virtual_call(object_klass, name, descriptor),
                }
            }
            _ => Err(io::Error::other("unsupported invoke opcode")),
        }
    })();
    let (target, code) = match resolved {
        Ok(resolved) => resolved,
        Err(error) => {
            if std::env::var_os("JVM25_JIT_DEBUG").is_some() {
                eprintln!("[callmethod] resolve failed: {error}");
            }
            return if let Some(handle) = bytecodeInterpreter::throwable_of(&error) {
                runtime::set_pending_throw(handle);
                0
            } else {
                deopt()
            };
        }
    };
    if let Err(error) = bytecodeInterpreter::initialize(&loader, target.as_ref(), &mut io::sink(), depth) {
        if std::env::var_os("JVM25_JIT_DEBUG").is_some() {
            eprintln!("[callmethod] initialize failed: {error}");
        }
        return if let Some(handle) = bytecodeInterpreter::throwable_of(&error) {
            runtime::set_pending_throw(handle);
            0
        } else {
            deopt()
        };
    }
    let flags = method_access_flags(&target, name, descriptor, is_static);
    let addr = compiled_entry(&target.class.name, name, descriptor);
    if addr != 0 && flags & (0x0100 | 0x0400 | 0x0020) == 0 {
        match runtime::try_invoke(addr, descriptor, is_static, &values, depth) {
            Some(runtime::CompiledResult::Value(value)) => {
                return match value {
                    Some(v) => call_result_raw(&loader, &v),
                    None => 0,
                };
            }
            Some(runtime::CompiledResult::Thrown(handle)) => {
                runtime::set_pending_throw(handle);
                return 0;
            }
            Some(runtime::CompiledResult::Deoptimize) | None => {}
        }
    }
    match bytecodeInterpreter::execute_labeled(
        &loader,
        &target,
        code,
        &values,
        &mut io::sink(),
        depth,
        flags,
        Some((name, descriptor)),
    ) {
        Ok(value) => match value {
            Some(v) => call_result_raw(&loader, &v),
            None => 0,
        },
        Err(error) => {
            if std::env::var_os("JVM25_JIT_DEBUG").is_some() {
                eprintln!("[callmethod] execute failed: {error}");
            }
            if let Some(handle) = bytecodeInterpreter::throwable_of(&error) {
                runtime::set_pending_throw(handle);
                0
            } else {
                deopt()
            }
        }
    }
}

/// Entry point of the installed, in-use nmethod for
/// `holder.name+descriptor`, or 0.
fn compiled_entry(holder: &[u8], name: &[u8], descriptor: &[u8]) -> usize {
    let holder = String::from_utf8_lossy(holder);
    let name = String::from_utf8_lossy(name);
    let descriptor = String::from_utf8_lossy(descriptor);
    let cache = CodeCache::global();
    let Some(nm) = cache.find_nmethod(holder.as_ref(), name.as_ref(), descriptor.as_ref()) else {
        return 0;
    };
    if nm.is_in_use() && !nm.is_osr_method() && nm.entry_point() != 0 {
        nm.entry_point()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Stub escape / unwind.
// ---------------------------------------------------------------------------

/// Reads and clears the xmm result channel for the trampoline.
extern "C" fn stub_xmm_result() -> u64 {
    runtime::take_stub_xmm_result()
}

/// Unwind entry called by every trampoline when a pending outcome was
/// recorded.  `frame` is the rbp of the JIT frame that called the
/// stub.  The sequence performs exactly that frame's epilogue plus
/// `ret`: `rsp = frame + 16`, `rbp = [frame]`, `jmp [frame + 8]`.
///
/// Nested activations need no chain walk: every JIT activation is
/// entered through `runtime::invoke_raw` (`call rax`), so the caller
/// frame's return address is always *this* activation's post-call
/// landing pad inside `invoke_raw`.  `try_invoke` consumes the pending
/// outcome and reports it to whoever invoked the compiled code — for
/// stub-routed calls that is `h_call_method`, which re-arms the
/// pending channel so the next trampoline up the stack unwinds the
/// next JIT frame.  The outcome therefore cascades activation by
/// activation until it reaches the outermost `try_invoke`, covering
/// JIT→JIT, JIT→stub→interpreter→JIT and interpreter-nested chains.
/// Never returns.
pub unsafe extern "C" fn jit_unwind(frame: u64) -> ! {
    let saved_rbp = unsafe { *(frame as *const u64) };
    let rsp = frame + 16;
    let ret = unsafe { *((frame + 8) as *const u64) };
    unsafe {
        core::arch::asm!(
            "mov rsp, {rsp}",
            "mov rbp, {saved}",
            "jmp {target}",
            rsp = in(reg) rsp,
            saved = in(reg) saved_rbp,
            target = in(reg) ret,
            options(noreturn),
        );
    }
}

// ---------------------------------------------------------------------------
// Stub table: RuntimeStub -> trampoline address.
// ---------------------------------------------------------------------------

struct StubEntry {
    stub: RuntimeStub,
    handler: StubHandler,
}

fn stub_entries() -> &'static [StubEntry] {
    static ENTRIES: OnceLock<Vec<StubEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        vec![
            StubEntry { stub: RuntimeStub::SafepointPoll, handler: h_safepoint_poll },
            StubEntry { stub: RuntimeStub::Deoptimize, handler: h_deoptimize },
            StubEntry { stub: RuntimeStub::ArithmeticException, handler: h_arithmetic_exception },
            StubEntry { stub: RuntimeStub::NullPointerException, handler: h_null_pointer_exception },
            StubEntry { stub: RuntimeStub::ArrayIndexOutOfBounds, handler: h_array_index_out_of_bounds },
            StubEntry { stub: RuntimeStub::ArrayStoreException, handler: h_array_store_exception },
            StubEntry { stub: RuntimeStub::NewInstance, handler: h_new_instance },
            StubEntry { stub: RuntimeStub::NewArray, handler: h_new_array },
            StubEntry { stub: RuntimeStub::AnewArray, handler: h_anew_array },
            StubEntry { stub: RuntimeStub::MultianewArray, handler: h_multianew_array },
            StubEntry { stub: RuntimeStub::CheckCast, handler: h_check_cast },
            StubEntry { stub: RuntimeStub::InstanceOf, handler: h_instance_of },
            StubEntry { stub: RuntimeStub::MonitorEnter, handler: h_monitor_enter },
            StubEntry { stub: RuntimeStub::MonitorExit, handler: h_monitor_exit },
            StubEntry { stub: RuntimeStub::Throw, handler: h_throw },
            StubEntry { stub: RuntimeStub::GetField, handler: h_get_field },
            StubEntry { stub: RuntimeStub::PutField, handler: h_put_field },
            StubEntry { stub: RuntimeStub::GetStatic, handler: h_get_static },
            StubEntry { stub: RuntimeStub::PutStatic, handler: h_put_static },
            StubEntry { stub: RuntimeStub::ArrayLength, handler: h_array_length },
            StubEntry { stub: RuntimeStub::ArrayLoad, handler: h_array_load },
            StubEntry { stub: RuntimeStub::ArrayStore, handler: h_array_store },
            StubEntry { stub: RuntimeStub::FloatRem, handler: h_float_rem },
            StubEntry { stub: RuntimeStub::FloatConvert, handler: h_float_convert },
            StubEntry { stub: RuntimeStub::UnresolvedField, handler: h_unresolved_field },
            StubEntry { stub: RuntimeStub::UnresolvedCall, handler: h_unresolved_call },
        ]
    })
}

/// Emits the per-stub trampoline machine code:
///
/// ```text
///   sub rsp, 0x28            ; shadow space + one stack arg slot
///   mov [rsp+0x20], rbp      ; 5th argument: JIT frame base
///   mov rax, <handler>
///   call rax                 ; -> extern "C" handler
///                            ;    (rcx..r9 = operands, stack arg = rbp)
///   mov r11, rax             ; save stub result
///   mov rax, <xmm_result>
///   call rax                 ; -> xmm bits for float/double (0 else)
///   movq xmm0, rax           ; JIT reads float results from xmm0
///   mov rax, <flow_check>
///   call rax                 ; -> pending outcome? 1 : 0
///   test eax, eax
///   jz  ret_path             ; RET_CONTINUE: value in rax/xmm0
///   ; ---- unwind to invoke_raw post-call landing pad ----
///   mov rcx, rbp             ; innermost JIT frame base
///   mov rax, <jit_unwind>
///   jmp rax                  ; never returns
/// ret_path:
///   mov rax, r11
///   add rsp, 0x28
///   ret
/// ```
///
/// On the unwind path `jit_unwind` walks the rbp chain to the
/// outermost JIT frame of the current `try_invoke` activation and
/// jumps to the post-`call {f}` landing pad inside `invoke_raw`, so
/// `rsp` becomes exactly the value the JIT epilogue would have
/// produced and `rip` lands where the pending outcome is read back
/// into TLS by Rust.
fn trampoline_bytes(handler_addr: usize) -> Vec<u8> {
    let check_addr = stub_flow_check as *const () as usize;
    let xmm_addr = stub_xmm_result as *const () as usize;
    let unwind_addr = jit_unwind as *const () as usize;
    let mut b = Vec::with_capacity(112);
    b.extend_from_slice(&[0x48, 0x83, 0xEC, 0x28]); // sub rsp,0x28
    // Win64 stack argument 5 ([rsp+0x20] after the sub, i.e. the qword
    // above the 32-byte shadow space): the JIT frame base in rbp.
    b.extend_from_slice(&[0x48, 0x89, 0x6C, 0x24, 0x20]); // mov [rsp+0x20],rbp
    b.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    b.extend_from_slice(&(handler_addr as u64).to_le_bytes());
    b.extend_from_slice(&[0xFF, 0xD0]); // call rax
    b.extend_from_slice(&[0x49, 0x89, 0xC3]); // mov r11,rax
    b.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    b.extend_from_slice(&(xmm_addr as u64).to_le_bytes());
    b.extend_from_slice(&[0xFF, 0xD0]); // call rax
    b.extend_from_slice(&[0x66, 0x48, 0x0F, 0x6E, 0xC0]); // movq xmm0,rax
    b.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    b.extend_from_slice(&(check_addr as u64).to_le_bytes());
    b.extend_from_slice(&[0xFF, 0xD0]); // call rax
    b.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    let jz_pos = b.len();
    b.extend_from_slice(&[0x0F, 0x84, 0, 0, 0, 0]); // jz rel32 -> ret_path
    // unwind: jit_unwind(frame=rbp) performs the calling JIT frame's
    // epilogue and jumps to its return address; never returns.
    b.extend_from_slice(&[0x48, 0x8B, 0xCD]); // mov rcx,rbp
    b.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    b.extend_from_slice(&(unwind_addr as u64).to_le_bytes());
    b.extend_from_slice(&[0xFF, 0xE0]); // jmp rax
    // ret_path:
    let ret_pos = b.len();
    let rel = (ret_pos as i32) - ((jz_pos + 6) as i32);
    b[jz_pos + 2..jz_pos + 6].copy_from_slice(&rel.to_le_bytes());
    b.extend_from_slice(&[0x4C, 0x89, 0xD8]); // mov rax,r11
    b.extend_from_slice(&[0x48, 0x83, 0xC4, 0x28]); // add rsp,0x28
    b.push(0xC3); // ret
    b
}

static STUB_MAP: OnceLock<BTreeMap<RuntimeStub, u64>> = OnceLock::new();

/// Builds the stub table once: emits every trampoline into a single
/// RWX page and maps each `RuntimeStub` to its trampoline address.
/// These are the addresses `BackendOptions.stubs` receives.
pub fn stub_map() -> BTreeMap<RuntimeStub, u64> {
    STUB_MAP
        .get_or_init(|| {
            let entries = stub_entries();
            let mut code = Vec::with_capacity(entries.len() * 80);
            let mut offsets = Vec::with_capacity(entries.len());
            for entry in entries {
                offsets.push(code.len());
                code.extend_from_slice(&trampoline_bytes(entry.handler as usize));
            }
            let base = publish_executable(&code, code.len().max(4096)).unwrap_or(0);
            let mut map = BTreeMap::new();
            if base != 0 {
                for (entry, off) in entries.iter().zip(offsets) {
                    map.insert(entry.stub, (base + off) as u64);
                }
            }
            map
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::prologue_frame_qwords;

    #[test]
    fn prologue_frame_qwords_c1_style() {
        // push rbp; mov rbp,rsp; sub rsp,0x80  ->  16 qwords
        let code = [0x55, 0x48, 0x89, 0xe5, 0x48, 0x81, 0xec, 0x80, 0, 0, 0];
        assert_eq!(prologue_frame_qwords(&code), 16);
    }

    #[test]
    fn prologue_frame_qwords_c2_saved_regs() {
        // push rbp; mov rbp,rsp; push rbx; push r12; sub rsp,0x40
        // -> 2 saved + 8 frame qwords = 10
        let code = [
            0x55, 0x48, 0x89, 0xe5, 0x53, 0x41, 0x54, 0x48, 0x81, 0xec, 0x40, 0, 0, 0,
        ];
        assert_eq!(prologue_frame_qwords(&code), 10);
    }

    #[test]
    fn prologue_frame_qwords_imm8_and_no_frame() {
        // push rbp; mov rbp,rsp; push rbp? no — push rax; sub rsp,0x18
        // -> 1 saved + 3 frame qwords = 4
        let code = [0x55, 0x48, 0x89, 0xe5, 0x50, 0x48, 0x83, 0xec, 0x18];
        assert_eq!(prologue_frame_qwords(&code), 4);
        // prologue only, no frame body -> just the pushes
        let code = [0x55, 0x48, 0x89, 0xe5, 0x53, 0x5d, 0xc3];
        assert_eq!(prologue_frame_qwords(&code), 1);
        // not a prologue at all
        assert_eq!(prologue_frame_qwords(&[0x90, 0x90]), 0);
    }
}
