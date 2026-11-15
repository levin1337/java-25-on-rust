/*
 * Port of OpenJDK 25 HotSpot Interpreter
 * (share/interpreter/interpreter.hpp / interpreter.cpp)
 *
 * `Interpreter` is the abstract interpreter facade: it owns the
 * generated codelet area (`InterpreterCodelet`s) and the link to the
 * concrete interpreter implementation (TemplateInterpreter for the
 * template dispatch, ZeroInterpreter for the Zero dispatch).  In this
 * port codelets are symbolic — there is no machine-code generation —
 * so `InterpreterCodelet` records the description, bytecode and size
 * bookkeeping that the C++ class maintains, and execution delegates
 * to `bytecodeInterpreter::execute`.
 */

use super::bytecodes::Code;
use super::templateInterpreter::TemplateInterpreter;
use super::zero::ZeroInterpreterGenerator;
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::classFileParser::Constant;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::oops::method::Code as MethodCode;
use std::io::{self, Write};
use std::sync::Arc;

/// `InterpreterCodelet`: a piece of interpreter code with debugging
/// metadata.  In the tree-walking port the "code" is a symbolic
/// handler id; `code_begin`/`code_end` report the id range.
#[derive(Debug)]
pub struct InterpreterCodelet {
    description: String,
    bytecode: Option<Code>,
    size: usize,
    /// Symbolic entry point (codelet id) used by dispatch tables.
    entry: usize,
}

impl InterpreterCodelet {
    pub fn new(description: impl Into<String>, bytecode: Option<Code>, entry: usize) -> Self {
        Self {
            description: description.into(),
            bytecode,
            size: 0,
            entry,
        }
    }

    /// `initialize(size)`.
    pub fn initialize(&mut self, size: usize) {
        self.size = size;
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// `code_begin`/`code_end` as symbolic addresses.
    pub fn code_begin(&self) -> usize {
        self.entry
    }

    pub fn code_end(&self) -> usize {
        self.entry + self.size
    }

    pub fn code_size(&self) -> usize {
        self.size
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn bytecode(&self) -> Option<Code> {
        self.bytecode
    }

    pub fn entry(&self) -> usize {
        self.entry
    }

    /// `print_on`/`print` equivalent for diagnostics.
    pub fn print_on(&self, out: &mut dyn Write) {
        let _ = writeln!(
            out,
            "InterpreterCodelet: {} bytecode={:?} size={} entry={:#x}",
            self.description,
            self.bytecode.map(|c| super::bytecodes::name(c as u8).unwrap_or("?")),
            self.size,
            self.entry
        );
    }
}

/// `CodeletMark`: RAII marker that "allocates" a codelet from the
/// interpreter code area.  Here it just records the codelet being
/// built.
pub struct CodeletMark {
    codelet: InterpreterCodelet,
}

impl CodeletMark {
    pub fn new(description: impl Into<String>, bytecode: Option<Code>, entry: usize) -> Self {
        Self {
            codelet: InterpreterCodelet::new(description, bytecode, entry),
        }
    }

    pub fn codelet(&self) -> &InterpreterCodelet {
        &self.codelet
    }

    pub fn codelet_mut(&mut self) -> &mut InterpreterCodelet {
        &mut self.codelet
    }
}

/// `Interpreter`: the singleton facade over the active interpreter.
/// `current()` selects the Zero or template dispatch; both delegate
/// execution to the same tree-walking engine.
pub struct Interpreter {
    /// Generated codelet table (symbolic).
    pub codelets: Vec<InterpreterCodelet>,
    /// Template dispatch tables (normal/safepoint/active).
    pub template: TemplateInterpreter,
    /// Whether the Zero dispatch is selected (`UseZero`/`Zero` variant).
    pub use_zero: bool,
    /// Zero generation statistics.
    pub zero_stats: super::zero::zeroInterpreterGenerator::ZeroGenStats,
}

impl Interpreter {
    /// `Interpreter::initialize`: builds the codelet area and the
    /// dispatch tables.
    pub fn new() -> Self {
        let mut interp = Interpreter {
            codelets: Vec::new(),
            template: TemplateInterpreter::new(),
            use_zero: true,
            zero_stats: Default::default(),
        };
        interp.zero_stats = ZeroInterpreterGenerator::generate(&mut interp.template);
        interp
    }

    /// `Interpreter::code` accessor for the generated area.
    pub fn code(&self) -> &[InterpreterCodelet] {
        &self.codelets
    }

    /// Registers a codelet (called by generators).
    pub fn add_codelet(&mut self, codelet: InterpreterCodelet) {
        self.codelets.push(codelet);
    }

    /// Executes `code` for `klass`: the interpreter entry point shared
    /// by Zero and template dispatch.
    pub fn execute<'a>(
        &self,
        loader: &ClassLoader<'a>,
        klass: &InstanceKlass<'a>,
        code: MethodCode<'a>,
        arguments: &[super::bytecodeInterpreter::Value<'a>],
        output: &mut dyn Write,
        depth: usize,
        access_flags: u16,
    ) -> io::Result<Option<super::bytecodeInterpreter::Value<'a>>> {
        super::bytecodeInterpreter::execute(
            loader, klass, code, arguments, output, depth, access_flags,
        )
    }

    /// `Interpreter::dispatch` for a raw bytecode: returns the symbolic
    /// entry (codelet id) the active dispatch table selects.
    pub fn dispatch_entry(&self, opcode: u8) -> usize {
        if self.use_zero {
            self.template
                .active_table
                .entry_for(super::templateTable::TosState::Vtos, opcode as usize)
        } else {
            self.template.dispatch_entry(super::templateTable::TosState::Vtos, opcode)
        }
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide interpreter singleton (lazily initialised).
pub static INTERPRETER: std::sync::LazyLock<std::sync::Mutex<Interpreter>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Interpreter::new()));

/// `Interpreter::current()` equivalent.
pub fn current() -> std::sync::MutexGuard<'static, Interpreter> {
    INTERPRETER.lock().unwrap_or_else(|e| e.into_inner())
}
