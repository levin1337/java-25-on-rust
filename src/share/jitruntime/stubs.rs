/*
 * Runtime stubs for JIT-compiled code (HotSpot `runtime/` stubs
 * analogue), Windows x64.
 *
 * Every `RuntimeStub` variant gets a real address in `stub_map()`.
 * The address points at a small machine-code *trampoline* emitted
 * into an RWX arena (`publish_executable`); the trampoline
 *
 *   1. reserves the 32-byte Win64 shadow space and calls the matching
 *      `extern "C"` Rust handler with the argument registers the
 *      backend loaded (rcx, rdx, r8, r9),
 *   2. if the handler returns 0 (`RET_CONTINUE`) returns to the JIT
 *      call site — rax carries the stub result,
 *   3. if the handler returns 1 (`RET_UNWIND`) abandons the JIT frame:
 *      it restores `rbp`/`rsp` from the compiled frame's rbp chain and
 *      jumps to the return address of the outermost `call` into JIT
 *      code — i.e. the post-call landing pad inside
 *      `runtime::invoke_raw`.  The handler has already recorded the
 *      outcome (pending throwable or deopt request) in TLS.
 *
 * The unwind is only valid because compiled methods are leaf code:
 * `hir_executable` rejects call instructions, so a stub is always
 * called from the single JIT frame created by `invoke_raw`.
 */

use super::runtime;
use crate::share::c1::asm::codeBuffer::CodeBuffer;
use crate::share::c1::c1_LIR::RuntimeStub;
use crate::share::interpreter::bytecodeInterpreter;
use crate::share::memory::heap;
use crate::share::runtime::threading;
use std::collections::BTreeMap;
use std::io;
use std::sync::{Mutex, OnceLock};

/// Same poll throttle as the interpreter safepoint cadence.
const SAFEPOINT_POLL_STRIDE: u32 = 256;

/// Handler return codes consumed by the trampoline.
const RET_CONTINUE: u64 = 0;
const RET_UNWIND: u64 = 1;

/// Process-wide arena keeping emitted stub trampolines and installed
/// nmethod code alive.  Like HotSpot's code cache, pages are never
/// returned to the OS once published (sweeping marks nmethods
/// non-entrant/zombie but does not unmap in this port).
static EXEC_ARENA: OnceLock<Mutex<Vec<CodeBuffer>>> = OnceLock::new();

fn arena() -> &'static Mutex<Vec<CodeBuffer>> {
    EXEC_ARENA.get_or_init(|| Mutex::new(Vec::new()))
}

/// Publishes `code` at offset 0 of a fresh RWX region reserved to at
/// least `reserve` bytes and returns the entry address.  The backing
/// `CodeBuffer` is kept in the arena so the pages stay mapped and
/// executable for the lifetime of the VM.
pub fn publish_executable(code: &[u8], reserve: usize) -> Option<usize> {
    let want = reserve.max(code.len()).max(4096);
    let buf = CodeBuffer::new(want).ok()?;
    buf.write(code).ok()?;
    buf.flush_icache();
    let addr = buf.base() as usize;
    arena().lock().unwrap().push(buf);
    Some(addr)
}

// ---------------------------------------------------------------------------
// Stub handlers — `unsafe extern "C"`, Win64 args in rcx/rdx/r8/r9.
// ---------------------------------------------------------------------------

type StubHandler = unsafe extern "C" fn(u64, u64, u64, u64) -> u64;

thread_local! {
    static SAFEPOINT_COUNTDOWN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// `SafepointPoll`: lets a running compiled loop participate in the
/// stop-the-world GC protocol.  Throttled like the interpreter's
/// safepoint cadence.
unsafe extern "C" fn h_safepoint_poll(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
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
        heap::safepoint();
    }
    RET_CONTINUE
}

/// Builds a real throwable object for `class_name` through the
/// interpreter's object arena and records it as the pending outcome.
/// Falls back to deoptimization when the throwable cannot be
/// materialized (e.g. bootstrap without a loader) so the interpreter
/// raises the failure itself.
fn throw_java(class_name: &'static [u8], message: Option<&str>) -> u64 {
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return RET_UNWIND;
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
            RET_UNWIND
        }
        Err(_) => {
            runtime::set_pending_deopt();
            RET_UNWIND
        }
    }
}

unsafe extern "C" fn h_arithmetic_exception(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    throw_java(b"java/lang/ArithmeticException", Some("/ by zero"))
}

unsafe extern "C" fn h_null_pointer_exception(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    throw_java(b"java/lang/NullPointerException", None)
}

unsafe extern "C" fn h_array_index_out_of_bounds(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    throw_java(b"java/lang/ArrayIndexOutOfBoundsException", None)
}

unsafe extern "C" fn h_array_store_exception(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    throw_java(b"java/lang/ArrayStoreException", None)
}

/// `athrow`: arg0 is the throwable object-arena handle (0 == null,
/// which the JVM specifies must raise NullPointerException instead).
unsafe extern "C" fn h_throw(obj: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let valid = threading::jvm_loader().is_some_and(|loader| {
        loader.borrow_objects().get(obj as usize).is_some()
    });
    if valid {
        runtime::set_pending_throw(obj as usize);
        RET_UNWIND
    } else {
        runtime::set_pending_deopt();
        RET_UNWIND
    }
}

/// `Deoptimize`: explicit uncommon-trap request (used by stubs that
/// cannot service the operation natively).
unsafe extern "C" fn h_deoptimize(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    runtime::set_pending_deopt();
    RET_UNWIND
}

/// Monitor enter/exit operate on object-arena handles exactly like the
/// interpreter's `monitorEnter`/`monitorExit`: the key is the handle
/// itself for `Value::Object`.
unsafe extern "C" fn h_monitor_enter(obj: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return RET_UNWIND;
    };
    let key = obj;
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
    RET_CONTINUE
}

unsafe extern "C" fn h_monitor_exit(obj: u64, _b: u64, _c: u64, _d: u64) -> u64 {
    if obj == 0 {
        return throw_java(b"java/lang/NullPointerException", None);
    }
    let Some(loader) = threading::jvm_loader() else {
        runtime::set_pending_deopt();
        return RET_UNWIND;
    };
    let key = obj;
    let tid = threading::current_tid(&loader);
    let result = loader
        .synchronizer
        .borrow_mut()
        .exit(key as usize, tid);
    match result {
        Ok(_) => RET_CONTINUE,
        Err(message) => throw_java(
            b"java/lang/IllegalMonitorStateException",
            Some(message),
        ),
    }
}

/// Stubs whose operations need data the leaf-code calling convention
/// does not carry (constant-pool resolution for allocation/casts,
/// call targets for invoke, field metadata) perform a real deopt:
/// the pending outcome unwinds to the interpreter, which re-executes
/// the method and the broker retires the nmethod — HotSpot's
/// uncommon-trap -> not-entrant path.
macro_rules! deopt_stub {
    ($name:ident) => {
        unsafe extern "C" fn $name(_a: u64, _b: u64, _c: u64, _d: u64) -> u64 {
            runtime::set_pending_deopt();
            RET_UNWIND
        }
    };
}

deopt_stub!(h_new_instance);
deopt_stub!(h_new_array);
deopt_stub!(h_anew_array);
deopt_stub!(h_multianew_array);
deopt_stub!(h_check_cast);
deopt_stub!(h_instance_of);
deopt_stub!(h_unresolved_field);
deopt_stub!(h_unresolved_call);

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
            StubEntry { stub: RuntimeStub::UnresolvedField, handler: h_unresolved_field },
            StubEntry { stub: RuntimeStub::UnresolvedCall, handler: h_unresolved_call },
        ]
    })
}

/// Emits the per-stub trampoline machine code:
///
/// ```text
///   sub rsp, 0x28            ; shadow space, keeps 16-byte alignment
///   mov rax, <handler>
///   call rax                 ; -> extern "C" handler (rcx..r9 args)
///   test eax, eax
///   jz  ret_path             ; RET_CONTINUE: value in rax
///   ; ---- unwind to invoke_raw post-call landing pad ----
///   mov rax, rbp             ; rbp = innermost(only) JIT frame base
///   mov rbp, [rax]           ; restore pre-JIT rbp
///   lea rsp, [rax+16]        ; rsp = pre-call value B
///   jmp [rax+8]              ; -> return address of `call {f}` in invoke_raw
/// ret_path:
///   add rsp, 0x28
///   ret
/// ```
///
/// On the unwind path `rsp` becomes exactly the value the JIT epilogue
/// would have produced (B), and `rip` lands on the instruction after
/// `call {f}` inside `invoke_raw`, which then restores `rsp`/`r12` and
/// returns to Rust with the pending outcome in TLS.
fn trampoline_bytes(handler_addr: usize) -> Vec<u8> {
    let mut b = Vec::with_capacity(64);
    b.extend_from_slice(&[0x48, 0x83, 0xEC, 0x28]); // sub rsp,0x28
    b.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    b.extend_from_slice(&(handler_addr as u64).to_le_bytes());
    b.extend_from_slice(&[0xFF, 0xD0]); // call rax
    b.extend_from_slice(&[0x85, 0xC0]); // test eax,eax
    let jz_pos = b.len();
    b.extend_from_slice(&[0x0F, 0x84, 0, 0, 0, 0]); // jz rel32 -> ret_path
    // unwind sequence — rax still holds the JIT frame base here; the
    // return-value register is only cleared after the target is read
    // (zeroing rax first would make `jmp [rax+8]` jump to [0x8]).
    b.extend_from_slice(&[0x48, 0x89, 0xE8]); // mov rax,rbp
    b.extend_from_slice(&[0x48, 0x8B, 0x28]); // mov rbp,[rax]
    b.extend_from_slice(&[0x48, 0x8D, 0x60, 0x10]); // lea rsp,[rax+16]
    b.extend_from_slice(&[0xFF, 0x60, 0x08]); // jmp [rax+8]
    // ret_path:
    let ret_pos = b.len();
    let rel = (ret_pos as i32) - ((jz_pos + 6) as i32);
    b[jz_pos + 2..jz_pos + 6].copy_from_slice(&rel.to_le_bytes());
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
            let mut code = Vec::with_capacity(entries.len() * 64);
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

