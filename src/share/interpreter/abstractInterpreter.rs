/*
 * Port of OpenJDK 25 HotSpot AbstractInterpreter (share/interpreter/abstractInterpreter.hpp)
 *
 * Implements method kind classification and fast-path dispatch for interpreter execution:
 * empty methods, getters, setters, native methods, and synchronized methods.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    ZeroLocals,
    ZeroLocalsSynchronized,
    Native,
    NativeSynchronized,
    Empty,
    Getter,
    Setter,
    Abstract,
}

pub struct AbstractInterpreter;

impl AbstractInterpreter {
    pub const ACC_SYNCHRONIZED: u16 = 0x0020;
    pub const ACC_NATIVE: u16 = 0x0100;
    pub const ACC_ABSTRACT: u16 = 0x0400;

    /// Classifies a method based on its access flags and bytecode sequence
    pub fn method_kind(access_flags: u16, code: &[u8]) -> MethodKind {
        if (access_flags & Self::ACC_ABSTRACT) != 0 {
            return MethodKind::Abstract;
        }

        let is_sync = (access_flags & Self::ACC_SYNCHRONIZED) != 0;

        if (access_flags & Self::ACC_NATIVE) != 0 {
            return if is_sync {
                MethodKind::NativeSynchronized
            } else {
                MethodKind::Native
            };
        }

        // Fast path: Empty method (only a return bytecode: 0xb1)
        if code == [0xb1] {
            return MethodKind::Empty;
        }

        // Fast path: Simple getter
        // aload_0 (0x2a), getfield (0xb4), return (*return: 0xac..0xb0)
        if code.len() == 4 && code[0] == 0x2a && code[1] == 0xb4 && matches!(code[3], 0xac..=0xb0) {
            return MethodKind::Getter;
        }

        // Fast path: Simple setter
        // aload_0 (0x2a), load_1 (0x1b..=0x1f or 0x2b), putfield (0xb5), return (0xb1)
        if code.len() == 5
            && code[0] == 0x2a
            && matches!(code[1], 0x1b..=0x1f | 0x2b)
            && code[2] == 0xb5
            && code[4] == 0xb1
        {
            return MethodKind::Setter;
        }

        if is_sync {
            MethodKind::ZeroLocalsSynchronized
        } else {
            MethodKind::ZeroLocals
        }
    }
}
