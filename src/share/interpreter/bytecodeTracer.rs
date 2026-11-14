/*
 * Port of OpenJDK 25 HotSpot BytecodeTracer (share/interpreter/bytecodeTracer.hpp, bytecodeTracer.cpp)
 *
 * Provides runtime bytecode tracing and execution formatting.
 */

use super::bytecodes;
use std::fmt::Write;

pub struct BytecodeTracer;

impl BytecodeTracer {
    pub fn trace(pc: usize, opcode: u8, operands: &[u8], stack_depth: usize) -> String {
        let name = bytecodes::name(opcode).unwrap_or("unknown");
        let mut s = format!("[bcp={pc:04x}] {name:<16}");
        if !operands.is_empty() {
            let _ = write!(s, " args=[");
            for (i, b) in operands.iter().enumerate() {
                if i > 0 {
                    let _ = write!(s, ", ");
                }
                let _ = write!(s, "0x{b:02x}");
            }
            s.push(']');
        }
        let _ = write!(s, " (stack_depth={stack_depth})");
        s
    }
}
