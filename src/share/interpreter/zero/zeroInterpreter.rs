/*
 * Port of OpenJDK 25 HotSpot zeroInterpreter.hpp
 * (share/interpreter/zero/zeroInterpreter.hpp)
 *
 * `ZeroInterpreter` is the state-machine abstraction over the Zero
 * dispatch loop: it tracks the current bytecode pointer, the frame
 * and the dispatch state, and delegates actual instruction execution
 * to the tree-walking `bytecodeInterpreter`.  The C++ class generates
 * a `ZeroInterpreter` codelet via `zeroInterpreterGenerator`; here
 * the same states are enumerated explicitly.
 */

use super::super::bytecodeInterpreter::{self, Value};
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::classFileParser::Constant;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::oops::method::Code;
use std::io::{self, Write};
use std::sync::Arc;

/// Dispatch states of the Zero interpreter loop, mirroring the
/// `_masm->dispatch()` state machine in `zero/bytecodeInterpreter.cpp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroState {
    /// Fetching the next bytecode (`_dispatch_next`).
    DispatchNext,
    /// Executing a normal bytecode template.
    Execute,
    /// Inside a `wide` prefix sequence.
    Wide,
    /// Returning from an interpreted call.
    Return,
    /// Unwinding the stack for an exception.
    Unwind,
    /// Safepoint poll before dispatch.
    Safepoint,
    /// Done: method returned.
    Done,
}

/// `ZeroInterpreter`: the interpreter object state for one frame.
pub struct ZeroInterpreter<'a> {
    /// Current bytecode index within `code.bytes`.
    pub bci: usize,
    /// Method code being interpreted.
    pub code: Code<'a>,
    /// Owning class.
    pub klass: Arc<InstanceKlass<'a>>,
    /// Operand stack.
    pub stack: Vec<Value<'a>>,
    /// Local variables.
    pub locals: Vec<Value<'a>>,
    /// Dispatch state machine position.
    pub state: ZeroState,
    /// Pending exception (Java throwable) being propagated.
    pub pending_exception: Option<io::Error>,
    /// `has_jsrs`/`has_monitor_bytecodes` metadata from the rewriter.
    pub has_monitor_bytecodes: bool,
}

impl<'a> ZeroInterpreter<'a> {
    /// Creates the interpreter state for `code` invoked with `args`.
    pub fn new(
        klass: Arc<InstanceKlass<'a>>,
        code: Code<'a>,
        args: &[Value<'a>],
    ) -> Self {
        let mut locals = vec![Value::Empty; usize::from(code.maxLocals).max(args.len() + 1)];
        // `this` for instance methods is pushed by the caller into args
        // already; place args at locals[0..].
        for (i, arg) in args.iter().enumerate() {
            if i < locals.len() {
                locals[i] = *arg;
            }
        }
        let stack_capacity = usize::from(code.maxStack);
        Self {
            bci: 0,
            code,
            klass,
            stack: Vec::with_capacity(stack_capacity),
            locals,
            state: ZeroState::DispatchNext,
            pending_exception: None,
            has_monitor_bytecodes: false,
        }
    }

    /// `_dispatch_next`: advance to the next bytecode, handling `wide`.
    pub fn dispatch_next(&mut self) -> Option<u8> {
        if self.bci >= self.code.bytes.len() {
            self.state = ZeroState::Done;
            return None;
        }
        let opcode = self.code.bytes[self.bci];
        if opcode == super::super::bytecodes::Code::Wide as u8 {
            self.state = ZeroState::Wide;
            self.bci += 1;
            if self.bci >= self.code.bytes.len() {
                return None;
            }
            return Some(self.code.bytes[self.bci]);
        }
        Some(opcode)
    }

    /// Executes one bytecode at the current `bci` by delegating the
    /// whole method to the tree-walking interpreter (the Zero
    /// dispatch loop is step-wise; the tree-walker performs the same
    /// semantics in one pass).
    pub fn run<'b>(
        &mut self,
        loader: &ClassLoader<'a>,
        output: &mut dyn Write,
        depth: usize,
        constants: &[Constant<'a>],
        access_flags: u16,
    ) -> io::Result<Option<Value<'a>>> {
        let args: Vec<Value<'a>> = self.locals.iter().copied().collect();
        bytecodeInterpreter::execute_labeled(
            loader,
            &self.klass,
            self.code.clone(),
            &args,
            output,
            depth,
            access_flags,
            Some((self.code.name, self.code.descriptor)),
        )
    }
}

/// Zero-specific dispatch table access: the Zero interpreter uses a
/// single dispatch table (no tos caching), equivalent to the
/// `TosState::Vtos` row of the template dispatch table.
pub fn zero_dispatch_entry(
    table: &super::super::templateInterpreter::DispatchTable,
    opcode: u8,
) -> usize {
    table.entry_for(super::super::templateTable::TosState::Vtos, opcode as usize)
}
