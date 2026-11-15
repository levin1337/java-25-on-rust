/*
 * Port of OpenJDK 25 HotSpot x86_64 Register definitions
 * (cpu/x86/vm/assembler_x86.hpp / registers_x86.hpp)
 *
 * Complete register model for x86_64: GPRs, XMM/YMM/ZMM, flags, and encodings.
 */

use std::fmt;

/// General-purpose register encoding (0-15 for RAX-R15)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Register {
    RAX = 0,
    RCX = 1,
    RDX = 2,
    RBX = 3,
    RSP = 4,
    RBP = 5,
    RSI = 6,
    RDI = 7,
    R8  = 8,
    R9  = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

impl Register {
    /// Number of general-purpose registers
    pub const COUNT: usize = 16;

    /// Returns the register encoding (0-15)
    #[inline]
    pub fn encoding(self) -> u8 {
        self as u8
    }

    /// Creates a register from its encoding (0-15)
    pub fn from_encoding(enc: u8) -> Option<Self> {
        match enc {
            0 => Some(Register::RAX),
            1 => Some(Register::RCX),
            2 => Some(Register::RDX),
            3 => Some(Register::RBX),
            4 => Some(Register::RSP),
            5 => Some(Register::RBP),
            6 => Some(Register::RSI),
            7 => Some(Register::RDI),
            8 => Some(Register::R8),
            9 => Some(Register::R9),
            10 => Some(Register::R10),
            11 => Some(Register::R11),
            12 => Some(Register::R12),
            13 => Some(Register::R13),
            14 => Some(Register::R14),
            15 => Some(Register::R15),
            _ => None,
        }
    }

    /// Returns true if this register requires a REX prefix (R8-R15)
    #[inline]
    pub fn needs_rex(self) -> bool {
        self as u8 >= 8
    }

    /// Returns the register name as used in assembly
    pub fn name(self) -> &'static str {
        match self {
            Register::RAX => "rax",
            Register::RCX => "rcx",
            Register::RDX => "rdx",
            Register::RBX => "rbx",
            Register::RSP => "rsp",
            Register::RBP => "rbp",
            Register::RSI => "rsi",
            Register::RDI => "rdi",
            Register::R8  => "r8",
            Register::R9  => "r9",
            Register::R10 => "r10",
            Register::R11 => "r11",
            Register::R12 => "r12",
            Register::R13 => "r13",
            Register::R14 => "r14",
            Register::R15 => "r15",
        }
    }

    /// Returns the 32-bit sub-register name (eax, ecx, ...)
    pub fn name32(self) -> &'static str {
        match self {
            Register::RAX => "eax",
            Register::RCX => "ecx",
            Register::RDX => "edx",
            Register::RBX => "ebx",
            Register::RSP => "esp",
            Register::RBP => "ebp",
            Register::RSI => "esi",
            Register::RDI => "edi",
            Register::R8  => "r8d",
            Register::R9  => "r9d",
            Register::R10 => "r10d",
            Register::R11 => "r11d",
            Register::R12 => "r12d",
            Register::R13 => "r13d",
            Register::R14 => "r14d",
            Register::R15 => "r15d",
        }
    }

    /// Returns the 16-bit sub-register name (ax, cx, ...)
    pub fn name16(self) -> &'static str {
        match self {
            Register::RAX => "ax",
            Register::RCX => "cx",
            Register::RDX => "dx",
            Register::RBX => "bx",
            Register::RSP => "sp",
            Register::RBP => "bp",
            Register::RSI => "si",
            Register::RDI => "di",
            Register::R8  => "r8w",
            Register::R9  => "r9w",
            Register::R10 => "r10w",
            Register::R11 => "r11w",
            Register::R12 => "r12w",
            Register::R13 => "r13w",
            Register::R14 => "r14w",
            Register::R15 => "r15w",
        }
    }

    /// Returns the 8-bit low sub-register name (al, cl, dl, bl, sil, dil, r8b-r15b)
    pub fn name8(self) -> &'static str {
        match self {
            Register::RAX => "al",
            Register::RCX => "cl",
            Register::RDX => "dl",
            Register::RBX => "bl",
            Register::RSP => "spl",
            Register::RBP => "bpl",
            Register::RSI => "sil",
            Register::RDI => "dil",
            Register::R8  => "r8b",
            Register::R9  => "r9b",
            Register::R10 => "r10b",
            Register::R11 => "r11b",
            Register::R12 => "r12b",
            Register::R13 => "r13b",
            Register::R14 => "r14b",
            Register::R15 => "r15b",
        }
    }

    /// Returns the 8-bit high sub-register name (ah, ch, dh, bh) - only for RAX-RDI
    pub fn name8h(self) -> Option<&'static str> {
        match self {
            Register::RAX => Some("ah"),
            Register::RCX => Some("ch"),
            Register::RDX => Some("dh"),
            Register::RBX => Some("bh"),
            _ => None,
        }
    }

    /// Caller-saved (volatile) registers per System V AMD64 ABI
    pub fn is_caller_saved(self) -> bool {
        matches!(
            self,
            Register::RAX | Register::RCX | Register::RDX | Register::RSI |
            Register::RDI | Register::R8 | Register::R9 | Register::R10 | Register::R11
        )
    }

    /// Callee-saved (non-volatile) registers per System V AMD64 ABI
    pub fn is_callee_saved(self) -> bool {
        matches!(
            self,
            Register::RBX | Register::RBP | Register::R12 | Register::R13 | Register::R14 | Register::R15
        )
    }

    /// Argument registers in order (System V AMD64 ABI)
    pub const ARG_REGS: [Register; 6] = [
        Register::RDI, Register::RSI, Register::RDX,
        Register::RCX, Register::R8, Register::R9,
    ];

    /// Return value register
    pub const RET_REG: Register = Register::RAX;

    /// Second return value register (for 128-bit returns)
    pub const RET_REG2: Register = Register::RDX;

    /// Stack pointer
    pub const SP: Register = Register::RSP;

    /// Frame pointer
    pub const FP: Register = Register::RBP;

    /// All registers as a slice
    pub const ALL: [Register; 16] = [
        Register::RAX, Register::RCX, Register::RDX, Register::RBX,
        Register::RSP, Register::RBP, Register::RSI, Register::RDI,
        Register::R8,  Register::R9,  Register::R10, Register::R11,
        Register::R12, Register::R13, Register::R14, Register::R15,
    ];

    /// Caller-saved registers as a slice
    pub const CALLER_SAVED: [Register; 9] = [
        Register::RAX, Register::RCX, Register::RDX, Register::RSI,
        Register::RDI, Register::R8, Register::R9, Register::R10, Register::R11,
    ];

    /// Callee-saved registers as a slice
    pub const CALLEE_SAVED: [Register; 6] = [
        Register::RBX, Register::RBP, Register::R12, Register::R13, Register::R14, Register::R15,
    ];
}

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// XMM/YMM/ZMM vector registers (16 registers, 128/256/512-bit)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum XMMRegister {
    XMM0  = 0,
    XMM1  = 1,
    XMM2  = 2,
    XMM3  = 3,
    XMM4  = 4,
    XMM5  = 5,
    XMM6  = 6,
    XMM7  = 7,
    XMM8  = 8,
    XMM9  = 9,
    XMM10 = 10,
    XMM11 = 11,
    XMM12 = 12,
    XMM13 = 13,
    XMM14 = 14,
    XMM15 = 15,
}

impl XMMRegister {
    pub const COUNT: usize = 16;

    #[inline]
    pub fn encoding(self) -> u8 {
        self as u8
    }

    pub fn from_encoding(enc: u8) -> Option<Self> {
        if enc < 16 { Some(unsafe { std::mem::transmute(enc) }) } else { None }
    }

    pub fn name(self) -> &'static str {
        match self {
            XMMRegister::XMM0  => "xmm0",
            XMMRegister::XMM1  => "xmm1",
            XMMRegister::XMM2  => "xmm2",
            XMMRegister::XMM3  => "xmm3",
            XMMRegister::XMM4  => "xmm4",
            XMMRegister::XMM5  => "xmm5",
            XMMRegister::XMM6  => "xmm6",
            XMMRegister::XMM7  => "xmm7",
            XMMRegister::XMM8  => "xmm8",
            XMMRegister::XMM9  => "xmm9",
            XMMRegister::XMM10 => "xmm10",
            XMMRegister::XMM11 => "xmm11",
            XMMRegister::XMM12 => "xmm12",
            XMMRegister::XMM13 => "xmm13",
            XMMRegister::XMM14 => "xmm14",
            XMMRegister::XMM15 => "xmm15",
        }
    }

    pub fn needs_rex(self) -> bool {
        self as u8 >= 8
    }

    /// Argument vector registers per System V AMD64 ABI
    pub const ARG_REGS: [XMMRegister; 8] = [
        XMMRegister::XMM0, XMMRegister::XMM1, XMMRegister::XMM2, XMMRegister::XMM3,
        XMMRegister::XMM4, XMMRegister::XMM5, XMMRegister::XMM6, XMMRegister::XMM7,
    ];

    pub const ALL: [XMMRegister; 16] = [
        XMMRegister::XMM0, XMMRegister::XMM1, XMMRegister::XMM2, XMMRegister::XMM3,
        XMMRegister::XMM4, XMMRegister::XMM5, XMMRegister::XMM6, XMMRegister::XMM7,
        XMMRegister::XMM8, XMMRegister::XMM9, XMMRegister::XMM10, XMMRegister::XMM11,
        XMMRegister::XMM12, XMMRegister::XMM13, XMMRegister::XMM14, XMMRegister::XMM15,
    ];
}

impl fmt::Display for XMMRegister {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// RFLAGS register bits
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum RFlags {
    CF = 1 << 0,   // Carry Flag
    PF = 1 << 2,   // Parity Flag
    AF = 1 << 4,   // Adjust Flag
    ZF = 1 << 6,   // Zero Flag
    SF = 1 << 7,   // Sign Flag
    TF = 1 << 8,   // Trap Flag
    IF = 1 << 9,   // Interrupt Flag
    DF = 1 << 10,  // Direction Flag
    OF = 1 << 11,  // Overflow Flag
    IOPL = 3 << 12, // I/O Privilege Level
    NT = 1 << 14,  // Nested Task
    RF = 1 << 16,  // Resume Flag
    VM = 1 << 17,  // Virtual-8086 Mode
    AC = 1 << 18,  // Alignment Check
    VIF = 1 << 19, // Virtual Interrupt Flag
    VIP = 1 << 20, // Virtual Interrupt Pending
    ID = 1 << 21,  // ID Flag
}

/// Condition codes for conditional jumps and moves
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Condition {
    O   = 0x0,  // Overflow
    NO  = 0x1,  // Not Overflow
    B   = 0x2,  // Below (CF=1) / NAE / C
    AE  = 0x3,  // Above or Equal (CF=0) / NB / NC
    E   = 0x4,  // Equal (ZF=1) / Z
    NE  = 0x5,  // Not Equal (ZF=0) / NZ
    BE  = 0x6,  // Below or Equal (CF=1 or ZF=1) / NA
    A   = 0x7,  // Above (CF=0 and ZF=0) / NBE
    S   = 0x8,  // Sign (SF=1)
    NS  = 0x9,  // Not Sign (SF=0)
    P   = 0xA,  // Parity (PF=1) / PE
    NP  = 0xB,  // Not Parity (PF=0) / PO
    L   = 0xC,  // Less (SF!=OF) / NGE
    GE  = 0xD,  // Greater or Equal (SF=OF) / NL
    LE  = 0xE,  // Less or Equal (ZF=1 or SF!=OF) / NG
    G   = 0xF,  // Greater (ZF=0 and SF=OF) / NLE
}

impl Condition {
    /// Inverts the condition
    pub fn invert(self) -> Self {
        // Toggle the low bit for most conditions
        match self {
            Condition::O  => Condition::NO,
            Condition::NO => Condition::O,
            Condition::B  => Condition::AE,
            Condition::AE => Condition::B,
            Condition::E  => Condition::NE,
            Condition::NE => Condition::E,
            Condition::BE => Condition::A,
            Condition::A  => Condition::BE,
            Condition::S  => Condition::NS,
            Condition::NS => Condition::S,
            Condition::P  => Condition::NP,
            Condition::NP => Condition::P,
            Condition::L  => Condition::GE,
            Condition::GE => Condition::L,
            Condition::LE => Condition::G,
            Condition::G  => Condition::LE,
        }
    }

    /// Returns the condition mnemonic
    pub fn mnemonic(self) -> &'static str {
        match self {
            Condition::O  => "o",
            Condition::NO => "no",
            Condition::B  => "b",
            Condition::AE => "ae",
            Condition::E  => "e",
            Condition::NE => "ne",
            Condition::BE => "be",
            Condition::A  => "a",
            Condition::S  => "s",
            Condition::NS => "ns",
            Condition::P  => "p",
            Condition::NP => "np",
            Condition::L  => "l",
            Condition::GE => "ge",
            Condition::LE => "le",
            Condition::G  => "g",
        }
    }

    /// Returns true if this is a signed comparison condition
    pub fn is_signed(self) -> bool {
        matches!(self, Condition::L | Condition::GE | Condition::LE | Condition::G)
    }

    /// Returns true if this is an unsigned comparison condition
    pub fn is_unsigned(self) -> bool {
        matches!(self, Condition::B | Condition::AE | Condition::BE | Condition::A)
    }
}

/// Operand size for instructions
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum OperandSize {
    Size8  = 0,
    Size16 = 1,
    Size32 = 2,
    Size64 = 3,
}

impl OperandSize {
    pub fn bytes(self) -> usize {
        match self {
            OperandSize::Size8  => 1,
            OperandSize::Size16 => 2,
            OperandSize::Size32 => 4,
            OperandSize::Size64 => 8,
        }
    }

    pub fn from_bytes(bytes: usize) -> Option<Self> {
        match bytes {
            1 => Some(OperandSize::Size8),
            2 => Some(OperandSize::Size16),
            4 => Some(OperandSize::Size32),
            8 => Some(OperandSize::Size64),
            _ => None,
        }
    }
}

/// Scale factor for SIB addressing (1, 2, 4, 8)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Scale {
    S1 = 0,
    S2 = 1,
    S4 = 2,
    S8 = 3,
}

impl Scale {
    pub fn value(self) -> u8 {
        match self {
            Scale::S1 => 1,
            Scale::S2 => 2,
            Scale::S4 => 4,
            Scale::S8 => 8,
        }
    }

    pub fn encoding(self) -> u8 {
        self as u8
    }
}