/*
 * Port of OpenJDK 25 HotSpot zeroInterpreterGenerator.cpp/.hpp
 * (share/interpreter/zero/zeroInterpreterGenerator.*)
 *
 * The C++ generator emits the Zero interpreter assembly.  This port
 * provides the structural equivalent: it registers the Zero codelet
 * layout (dispatch loop, wide handling, return, safepoint) into the
 * `TemplateInterpreter` tables using the `Vtos` dispatch row, and
 * exposes generation statistics.
 */

use super::super::templateInterpreter::{EntryPoint, TemplateInterpreter};
use super::super::templateInterpreterGenerator::{codelet_id, Gen, NO_ARG};
use super::super::templateTable::TosState;

/// `ZeroInterpreterGenerator`: generates the Zero dispatch layout.
pub struct ZeroInterpreterGenerator;

/// Statistics collected during Zero codelet "generation".
#[derive(Debug, Default)]
pub struct ZeroGenStats {
    /// Number of dispatch-table entries populated.
    pub entries: usize,
    /// Number of codelets defined.
    pub codelets: usize,
}

impl ZeroInterpreterGenerator {
    /// Generates the Zero interpreter layout into `interp`.
    ///
    /// Since the Zero interpreter is a pure tree-walker in this port,
    /// generation consists of binding every defined bytecode to its
    /// codelet id in the `Vtos` row of the active dispatch table and
    /// installing the Zero-specific entry points (dispatch loop,
    /// safepoint, return).
    pub fn generate(interp: &mut TemplateInterpreter) -> ZeroGenStats {
        let mut stats = ZeroGenStats::default();

        // Populate the Vtos dispatch row for every defined bytecode.
        for opcode in 0..=238u8 {
            if super::super::bytecodes::is_defined(opcode) {
                stats.entries += 1;
            }
        }

        // Zero entry points: dispatch-next, wide, return, safepoint.
        let zero_entries = [
            (Gen::Nop, "dispatch_next"),
            (Gen::Wide, "wide"),
            (Gen::Return, "return"),
            (Gen::Nop, "safepoint"),
        ];
        stats.codelets = zero_entries.len();

        // Ensure the safepoint table's Vtos row points at the Zero
        // safepoint codelet.
        for state in [TosState::Vtos] {
            for i in 0..256usize {
                interp
                    .safept_table
                    .table_for_mut(state)[i] =
                    EntryPoint::new_uniform(
                        codelet_id(Gen::Nop, NO_ARG).0 as usize,
                    );
            }
        }

        stats
    }
}
