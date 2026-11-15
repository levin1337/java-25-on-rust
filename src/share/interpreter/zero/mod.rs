/*
 * Port of OpenJDK 25 HotSpot Zero interpreter subtree
 * (share/interpreter/zero/).
 *
 * The Zero interpreter is the template-table interpreter without the
 * tos-cache optimisations.  This port re-exports the tree-walking
 * `bytecodeInterpreter` (which already implements the Zero dispatch
 * loop semantics) and adds the Zero-specific state machine and
 * generator scaffolding.
 */

pub mod zeroInterpreter;
pub mod zeroInterpreterGenerator;

pub use super::bytecodeInterpreter;
pub use super::bytecodeInterpreter::{execute, execute_labeled};
pub use zeroInterpreter::ZeroInterpreter;
pub use zeroInterpreterGenerator::ZeroInterpreterGenerator;
