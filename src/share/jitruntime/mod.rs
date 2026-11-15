/*
 * JIT runtime bridge: the executable tier that connects interpreter
 * frames to C1-compiled nmethods on Windows x64.
 *
 * Layout (mirrors HotSpot's `runtime/` stubs + `JavaCalls`):
 *
 *  - `stubs`: real `extern "C"` runtime-stub handlers plus tiny
 *    machine-code trampolines emitted into an RWX arena.  Generated
 *    code calls the trampoline address recorded in
 *    `BackendOptions.stubs`; the trampoline forwards the Win64
 *    argument registers to the Rust handler and then either returns
 *    to the JIT call site (stub produced a value / was a poll) or
 *    unwinds the JIT frame back into `runtime::try_invoke`
 *    (exceptional exit carrying a pending throwable / deopt request).
 *
 *  - `runtime`: signature parsing restricted to the executable subset,
 *    argument marshalling, the raw `call` into nmethod entry points,
 *    and the pending-outcome channel shared with the stubs.
 */

pub mod runtime;
pub mod stubs;

pub use runtime::{signature_supported, try_invoke, CompiledResult};
pub use stubs::{publish_executable, stub_map};
