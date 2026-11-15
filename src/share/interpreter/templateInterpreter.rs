/*
 * Port of OpenJDK 25 HotSpot TemplateInterpreter
 * (share/interpreter/templateInterpreter.hpp / templateInterpreter.cpp)
 *
 * The template interpreter dispatches bytecodes through a table of
 * per-TosState entry points (`EntryPoint` inside `DispatchTable`).
 * This port keeps the same data structures — dispatch tables, return
 * entries, deopt entries, invoke return addresses and the safepoint
 * table switch — but entries are symbolic codelet identifiers instead
 * of generated machine-code addresses, and execution delegates to the
 * tree-walking interpreter (`bytecodeInterpreter::execute`).
 */

use super::templateTable::{Template, TemplateTable, TosState};
use super::interpreterRuntime;
use crate::share::classfile::classFileParser::Constant;
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::oops::method::Code;
use std::io::{self, Write};
use std::sync::Arc;

/// Number of Top-of-Stack cache states (`number_of_states` = 10:
/// btos, ztos, ctos, stos, itos, ltos, ftos, dtos, atos, vtos).
pub const NUMBER_OF_STATES: usize = 10;

/// `EntryPoint`: a group of tosca-specific entry points for one
/// bytecode.  Entries are codelet identifiers (symbolic "addresses").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryPoint {
    entry: [usize; NUMBER_OF_STATES],
}

impl EntryPoint {
    pub const ZERO: EntryPoint = EntryPoint {
        entry: [0; NUMBER_OF_STATES],
    };

    /// `EntryPoint()`: all states point at the same default entry.
    pub fn new_uniform(entry: usize) -> Self {
        Self {
            entry: [entry; NUMBER_OF_STATES],
        }
    }

    /// `EntryPoint(b,z,c,s,a,i,l,f,d,v)`: per-state entries where
    /// boolean/byte/char/short share the int entry.
    pub fn new(
        b: usize,
        z: usize,
        c: usize,
        s: usize,
        a: usize,
        i: usize,
        l: usize,
        f: usize,
        d: usize,
        v: usize,
    ) -> Self {
        Self {
            entry: [b, z, c, s, a, i, l, f, d, v],
        }
    }

    /// `EntryPoint(a,i,l,f,d,v)`: b/z/c/s share the int entry.
    pub fn new_int_family(a: usize, i: usize, l: usize, f: usize, d: usize, v: usize) -> Self {
        Self::new(i, i, i, i, a, i, l, f, d, v)
    }

    /// `entry(TosState)`.
    pub fn entry(&self, state: TosState) -> usize {
        self.entry[tos_state_index(state)]
    }

    /// `set_entry(TosState, entry)`.
    pub fn set_entry(&mut self, state: TosState, entry: usize) {
        self.entry[tos_state_index(state)] = entry;
    }

    pub fn entries(&self) -> &[usize; NUMBER_OF_STATES] {
        &self.entry
    }
}

pub fn tos_state_index(state: TosState) -> usize {
    match state {
        TosState::Btos => 0,
        TosState::Ztos => 1,
        TosState::Ctos => 2,
        TosState::Stos => 3,
        TosState::Itos => 4,
        TosState::Ltos => 5,
        TosState::Ftos => 6,
        TosState::Dtos => 7,
        TosState::Atos => 8,
        TosState::Vtos => 9,
    }
}

/// `DispatchTable`: `number_of_states` dispatch tables of 256 entries.
#[derive(Clone)]
pub struct DispatchTable {
    table: [[EntryPoint; 256]; NUMBER_OF_STATES],
}

impl DispatchTable {
    pub fn new() -> Self {
        Self {
            table: [[EntryPoint::ZERO; 256]; NUMBER_OF_STATES],
        }
    }

    /// `entry(i)`.
    pub fn entry(&self, i: usize) -> EntryPoint {
        // Entries are stored per tosca state; the "entry point" for a
        // bytecode is the set across all states, represented by state 0
        // plus per-state access through `entry_for`.
        self.table[0][i & 0xff]
    }

    /// `entry_for(state, i)`: the codelet for `i` in tosca `state`.
    pub fn entry_for(&self, state: TosState, i: usize) -> usize {
        self.table[tos_state_index(state)][i & 0xff].entry(state)
    }

    /// `set_entry(i, EntryPoint)`.
    pub fn set_entry(&mut self, i: usize, entry: EntryPoint) {
        for state in 0..NUMBER_OF_STATES {
            self.table[state][i & 0xff] = entry;
        }
    }

    /// `table_for(state)` equivalent: raw row access.
    pub fn table_for(&self, state: TosState) -> &[EntryPoint; 256] {
        &self.table[tos_state_index(state)]
    }

    pub fn table_for_mut(&mut self, state: TosState) -> &mut [EntryPoint; 256] {
        &mut self.table[tos_state_index(state)]
    }
}

impl Default for DispatchTable {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Codelet identifiers
// ---------------------------------------------------------------------------

/// Symbolic codelet ids used as "entry addresses".  The tree-walking
/// interpreter does not emit machine code, so codelets are identified
/// by an enum-like u16 namespace; `TemplateInterpreterGenerator`
/// assigns them deterministically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CodeletId(pub u16);

/// Well-known codelet ids (mirroring the named entries in
/// templateInterpreter.hpp).
pub mod codelets {
    use super::CodeletId;
    pub const NORMAL: CodeletId = CodeletId(1);
    pub const WIDE: CodeletId = CodeletId(2);
    pub const RETURN: CodeletId = CodeletId(3);
    pub const EARLYRET: CodeletId = CodeletId(4);
    pub const SAFEPOINT: CodeletId = CodeletId(5);
    pub const INVOKE_RETURN: CodeletId = CodeletId(6);
    pub const INVOKEINTERFACE_RETURN: CodeletId = CodeletId(7);
    pub const INVOKEDYNAMIC_RETURN: CodeletId = CodeletId(8);
    pub const DEOPT_REEXECUTE: CodeletId = CodeletId(9);
    pub const THROW_EXCEPTION: CodeletId = CodeletId(10);
    pub const REMOVE_ACTIVATION: CodeletId = CodeletId(11);
}

// ---------------------------------------------------------------------------
// TemplateInterpreter statics
// ---------------------------------------------------------------------------

/// `TemplateInterpreter::MoreConstants`.
pub const MAX_INVOKE_LENGTH: usize = 5;
pub const MAX_BYTECODE_LENGTH: usize = 6;
pub const NUMBER_OF_RETURN_ENTRIES: usize = MAX_INVOKE_LENGTH + 1;
pub const NUMBER_OF_DEOPT_ENTRIES: usize = MAX_BYTECODE_LENGTH + 1;
pub const NUMBER_OF_RETURN_ADDRS: usize = NUMBER_OF_STATES;

/// `Throw_*` runtime entry points: symbolic ids of the interpreter
/// runtime helpers used by trap bytecodes.
#[derive(Clone, Copy, Debug)]
pub enum ThrowEntry {
    ArrayIndexOutOfBounds,
    ArrayStore,
    Arithmetic,
    ClassCast,
    NullPointer,
    Exception,
    StackOverflow,
}

/// `TemplateInterpreter` global state: the three dispatch tables and
/// the return/deopt/invoke-return entry arrays.
pub struct TemplateInterpreter {
    /// `_normal_table`: dispatch table for normal execution.
    pub normal_table: DispatchTable,
    /// `_safept_table`: dispatch table used at safepoints.
    pub safept_table: DispatchTable,
    /// `_active_table`: currently active dispatch table.
    pub active_table: DispatchTable,
    /// `_return_entry[number_of_return_entries]`.
    pub return_entry: Vec<EntryPoint>,
    /// `_earlyret_entry`.
    pub earlyret_entry: EntryPoint,
    /// `_deopt_entry[number_of_deopt_entries]`.
    pub deopt_entry: Vec<EntryPoint>,
    /// `_deopt_reexecute_return_entry`.
    pub deopt_reexecute_return_entry: usize,
    /// `_safept_entry`.
    pub safept_entry: EntryPoint,
    /// `_invoke_return_entry[number_of_return_addrs]`.
    pub invoke_return_entry: Vec<EntryPoint>,
    /// `_invokeinterface_return_entry`.
    pub invokeinterface_return_entry: Vec<EntryPoint>,
    /// `_invokedynamic_return_entry`.
    pub invokedynamic_return_entry: Vec<EntryPoint>,
    /// `_wentry_point[DispatchTable::length]`: wide-instruction entries.
    pub wentry_point: Vec<EntryPoint>,
    /// Throw entry codelet ids.
    pub throw_entries: [usize; 7],
    pub remove_activation_entry: usize,
    pub remove_activation_preserving_args_entry: usize,
    pub cont_resume_interpreter_adapter: usize,
}

impl TemplateInterpreter {
    /// `TemplateInterpreter::initialize_code` skeleton: builds the
    /// normal and safepoint tables via the generator, then activates
    /// the normal table.
    pub fn new() -> Self {
        let mut interp = Self {
            normal_table: DispatchTable::new(),
            safept_table: DispatchTable::new(),
            active_table: DispatchTable::new(),
            return_entry: vec![EntryPoint::ZERO; NUMBER_OF_RETURN_ENTRIES],
            earlyret_entry: EntryPoint::ZERO,
            deopt_entry: vec![EntryPoint::ZERO; NUMBER_OF_DEOPT_ENTRIES],
            deopt_reexecute_return_entry: 0,
            safept_entry: EntryPoint::ZERO,
            invoke_return_entry: vec![EntryPoint::ZERO; NUMBER_OF_RETURN_ADDRS],
            invokeinterface_return_entry: vec![EntryPoint::ZERO; NUMBER_OF_RETURN_ADDRS],
            invokedynamic_return_entry: vec![EntryPoint::ZERO; NUMBER_OF_RETURN_ADDRS],
            wentry_point: vec![EntryPoint::ZERO; 256],
            throw_entries: [0; 7],
            remove_activation_entry: codelets::REMOVE_ACTIVATION.0 as usize,
            remove_activation_preserving_args_entry: codelets::REMOVE_ACTIVATION.0 as usize,
            cont_resume_interpreter_adapter: 0,
        };
        super::templateInterpreterGenerator::TemplateInterpreterGenerator::generate(&mut interp);
        interp.active_table = interp.normal_table.clone();
        interp
    }

    /// `set_active_table` for safepoint entry/exit.
    pub fn enter_safepoint(&mut self) {
        self.active_table = self.safept_table.clone();
    }

    /// Restore the normal dispatch table after the safepoint.
    pub fn leave_safepoint(&mut self) {
        self.active_table = self.normal_table.clone();
    }

    /// `active_table` accessor used by the dispatch loop.
    pub fn active(&self) -> &DispatchTable {
        &self.active_table
    }

    /// Symbolic dispatch: returns the codelet id for `opcode` under
    /// the current tosca state, mirroring `table_for(state)[bcp]`.
    pub fn dispatch_entry(&self, state: TosState, opcode: u8) -> usize {
        self.active_table.entry_for(state, opcode as usize)
    }

    /// The codelet id for `opcode` in the normal table.
    pub fn normal_entry(&self, state: TosState, opcode: u8) -> usize {
        self.normal_table.entry_for(state, opcode as usize)
    }

    /// `Interpreter::return_from_interpreted_method` support: picks the
    /// return-entry slot for an invoke instruction length (`n` bytes).
    pub fn return_entry_for(&self, invoke_length: usize) -> EntryPoint {
        self.return_entry[invoke_length.min(NUMBER_OF_RETURN_ENTRIES - 1)]
    }

    /// Picks the invoke-kind-specific return table (invokestatic/
    /// invokespecial/invokevirtual share one, invokeinterface and
    /// invokedynamic have their own — matching the C++ statics).
    pub fn invoke_return_entry_for(&self, is_interface: bool, is_dynamic: bool, state: TosState) -> usize {
        let table = if is_dynamic {
            &self.invokedynamic_return_entry
        } else if is_interface {
            &self.invokeinterface_return_entry
        } else {
            &self.invoke_return_entry
        };
        table[tos_state_index(state).min(NUMBER_OF_STATES - 1)]
            .entry(state)
    }

    /// `Interpreter::remove_activation`: continuation entries used when
    /// an exception is not handled by the current frame.
    pub fn remove_activation(&self) -> usize {
        self.remove_activation_entry
    }

    pub fn remove_activation_preserving_args(&self) -> usize {
        self.remove_activation_preserving_args_entry
    }

    /// `Interpreter::continue` equivalent for the tree-walking
    /// interpreter: delegates to `bytecodeInterpreter::execute`, which
    /// returns the single return value (or `None` for void methods).
    pub fn execute<'a>(
        &self,
        loader: &ClassLoader<'a>,
        klass: &InstanceKlass<'a>,
        code: Code<'a>,
        arguments: &[super::bytecodeInterpreter::Value<'a>],
        output: &mut dyn Write,
        depth: usize,
        access_flags: u16,
    ) -> io::Result<Option<super::bytecodeInterpreter::Value<'a>>> {
        super::bytecodeInterpreter::execute(
            loader, klass, code, arguments, output, depth, access_flags,
        )
    }
}

impl Default for TemplateInterpreter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Throw helpers bound to the symbolic throw entries
// ---------------------------------------------------------------------------

/// Executes the runtime helper for `entry`, mirroring the
/// `throw_*_entry` codelets.  Returns the `io::Error` carrying the
/// throwable object (same contract as the interpreter's `jex`).
pub fn throw_entry<'a>(
    entry: ThrowEntry,
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    index: Option<i32>,
    length: Option<usize>,
) -> io::Error {
    match entry {
        ThrowEntry::ArrayIndexOutOfBounds => interpreterRuntime::throw_array_index_out_of_bounds(
            loader, output, depth, index.unwrap_or(0), length.unwrap_or(0),
        ),
        ThrowEntry::ArrayStore => super::bytecodeInterpreter::jex(
            loader, output, depth, b"java/lang/ArrayStoreException", None,
        ),
        ThrowEntry::Arithmetic => super::bytecodeInterpreter::jex(
            loader, output, depth, b"java/lang/ArithmeticException", Some("/ by zero"),
        ),
        ThrowEntry::ClassCast => super::bytecodeInterpreter::jex(
            loader, output, depth, b"java/lang/ClassCastException", None,
        ),
        ThrowEntry::NullPointer => interpreterRuntime::throw_null_pointer(loader, output, depth),
        ThrowEntry::Exception => super::bytecodeInterpreter::jex(
            loader, output, depth, b"java/lang/Throwable", None,
        ),
        ThrowEntry::StackOverflow => {
            interpreterRuntime::throw_stack_overflow_error(loader, output, depth)
        }
    }
}

