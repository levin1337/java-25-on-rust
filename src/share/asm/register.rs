/*
 * Port of OpenJDK 25 HotSpot share/asm/register.hpp
 *
 * HotSpot register model: registers are value objects wrapping an integer
 * code.  `Register` covers general-purpose registers (x86_64: rax..r15,
 * code 0..15, NOREG = invalid).  `FloatRegister` covers XMM/YMM/ZMM vector
 * registers with a length tag (like FloatRegisterImpl::_len).
 */

use std::fmt;

/// Invalid register marker, mirrors HotSpot `noreg`.
pub const NOREG: Register = Register { code: u8::MAX };

/// General-purpose register.  `code` is the hardware encoding (0..15 on
/// x86_64); `u8::MAX` denotes noreg.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Register {
    code: u8,
}

impl Register {
    pub const RAX: Register = Register { code: 0 };
    pub const RCX: Register = Register { code: 1 };
    pub const RDX: Register = Register { code: 2 };
    pub const RBX: Register = Register { code: 3 };
    pub const RSP: Register = Register { code: 4 };
    pub const RBP: Register = Register { code: 5 };
    pub const RSI: Register = Register { code: 6 };
    pub const RDI: Register = Register { code: 7 };
    pub const R8: Register = Register { code: 8 };
    pub const R9: Register = Register { code: 9 };
    pub const R10: Register = Register { code: 10 };
    pub const R11: Register = Register { code: 11 };
    pub const R12: Register = Register { code: 12 };
    pub const R13: Register = Register { code: 13 };
    pub const R14: Register = Register { code: 14 };
    pub const R15: Register = Register { code: 15 };

    /// Number of valid GPR encodings.
    pub const NUMBER_OF_REGISTERS: usize = 16;

    pub const ALL: [Register; 16] = [
        Register::RAX, Register::RCX, Register::RDX, Register::RBX,
        Register::RSP, Register::RBP, Register::RSI, Register::RDI,
        Register::R8, Register::R9, Register::R10, Register::R11,
        Register::R12, Register::R13, Register::R14, Register::R15,
    ];

    /// Registers used by the System V AMD64 C ABI for arguments.
    pub const C_ARG_REGS: [Register; 6] = [
        Register::RDI, Register::RSI, Register::RDX,
        Register::RCX, Register::R8, Register::R9,
    ];

    /// Registers used by the Microsoft x64 C ABI for arguments.
    pub const WIN64_ARG_REGS: [Register; 4] = [
        Register::RCX, Register::RDX, Register::R8, Register::R9,
    ];

    /// x86_64 stack alignment requirement.
    pub const STACK_ALIGNMENT: usize = 16;

    #[inline]
    pub fn as_u8(self) -> u8 {
        self.code
    }

    /// Hardware encoding 0..15, panics on noreg like HotSpot asserts.
    #[inline]
    pub fn encoding(self) -> u8 {
        assert!(self.is_valid(), "invalid register");
        self.code
    }

    #[inline]
    pub fn is_valid(self) -> bool {
        self.code < Self::NUMBER_OF_REGISTERS as u8
    }

    #[inline]
    pub fn is_caller_save(self) -> bool {
        matches!(
            self,
            Register::RAX | Register::RCX | Register::RDX | Register::RSI |
            Register::RDI | Register::R8 | Register::R9 | Register::R10 | Register::R11
        )
    }

    #[inline]
    pub fn is_callee_save(self) -> bool {
        matches!(
            self,
            Register::RBX | Register::RSP | Register::RBP |
            Register::R12 | Register::R13 | Register::R14 | Register::R15
        )
    }

    #[inline]
    pub fn needs_rex(self) -> bool {
        self.code >= 8
    }

    /// Low 32-bit view of the register (zero-extends to 64 on x86_64).
    #[inline]
    pub fn as_w(self) -> Register {
        self
    }

    pub fn name(self) -> &'static str {
        match self {
            Register::RAX => "rax", Register::RCX => "rcx", Register::RDX => "rdx",
            Register::RBX => "rbx", Register::RSP => "rsp", Register::RBP => "rbp",
            Register::RSI => "rsi", Register::RDI => "rdi", Register::R8 => "r8",
            Register::R9 => "r9", Register::R10 => "r10", Register::R11 => "r11",
            Register::R12 => "r12", Register::R13 => "r13", Register::R14 => "r14",
            Register::R15 => "r15", _ => "noreg",
        }
    }

    pub fn name32(self) -> &'static str {
        match self {
            Register::RAX => "eax", Register::RCX => "ecx", Register::RDX => "edx",
            Register::RBX => "ebx", Register::RSP => "esp", Register::RBP => "ebp",
            Register::RSI => "esi", Register::RDI => "edi", Register::R8 => "r8d",
            Register::R9 => "r9d", Register::R10 => "r10d", Register::R11 => "r11d",
            Register::R12 => "r12d", Register::R13 => "r13d", Register::R14 => "r14d",
            Register::R15 => "r15d", _ => "noreg",
        }
    }

    pub fn name16(self) -> &'static str {
        match self {
            Register::RAX => "ax", Register::RCX => "cx", Register::RDX => "dx",
            Register::RBX => "bx", Register::RSP => "sp", Register::RBP => "bp",
            Register::RSI => "si", Register::RDI => "di", Register::R8 => "r8w",
            Register::R9 => "r9w", Register::R10 => "r10w", Register::R11 => "r11w",
            Register::R12 => "r12w", Register::R13 => "r13w", Register::R14 => "r14w",
            Register::R15 => "r15w", _ => "noreg",
        }
    }

    pub fn name8(self) -> &'static str {
        match self {
            Register::RAX => "al", Register::RCX => "cl", Register::RDX => "dl",
            Register::RBX => "bl", Register::RSP => "spl", Register::RBP => "bpl",
            Register::RSI => "sil", Register::RDI => "dil", Register::R8 => "r8b",
            Register::R9 => "r9b", Register::R10 => "r10b", Register::R11 => "r11b",
            Register::R12 => "r12b", Register::R13 => "r13b", Register::R14 => "r14b",
            Register::R15 => "r15b", _ => "noreg",
        }
    }

    pub fn name8h(self) -> Option<&'static str> {
        match self {
            Register::RAX => Some("ah"), Register::RCX => Some("ch"),
            Register::RDX => Some("dh"), Register::RBX => Some("bh"),
            _ => None,
        }
    }

    /// HotSpot-style RegSet: bitmask over all registers plus noreg slot.
    pub fn bit(self) -> u64 {
        if self.is_valid() {
            1u64 << self.code
        } else {
            0
        }
    }
}

impl fmt::Debug for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}
impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Bitset over GPRs, mirrors HotSpot `RegSet` (AbstractRegSet).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RegSet {
    bits: u64,
}

impl RegSet {
    pub const EMPTY: RegSet = RegSet { bits: 0 };
    pub const ALL_REGS: RegSet = RegSet { bits: 0xffff };

    pub const fn of(r: Register) -> RegSet {
        RegSet { bits: r.bit() }
    }

    pub const fn of2(a: Register, b: Register) -> RegSet {
        RegSet { bits: a.bit() | b.bit() }
    }

    pub const fn of3(a: Register, b: Register, c: Register) -> RegSet {
        RegSet { bits: a.bit() | b.bit() | c.bit() }
    }

    pub fn contains(self, r: Register) -> bool {
        (self.bits & r.bit()) != 0
    }

    pub fn plus(self, r: Register) -> RegSet {
        RegSet { bits: self.bits | r.bit() }
    }

    pub fn minus(self, r: Register) -> RegSet {
        RegSet { bits: self.bits & !r.bit() }
    }

    /// Iterate set registers in encoding order.
    pub fn regs(self) -> impl Iterator<Item = Register> {
        let bits = self.bits;
        (0..16u8).filter_map(|c| {
            if (bits >> c) & 1 != 0 {
                Some(Register { code: c })
            } else {
                None
            }
        })
    }

    pub fn bits(self) -> u64 {
        self.bits
    }

    pub fn size(self) -> u32 {
        self.bits.count_ones()
    }

    pub fn is_empty(self) -> bool {
        self.bits == 0
    }
}

/// Vector register length tag, mirrors FloatRegisterImpl length kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatRegisterLength {
    /// 128-bit XMM
    XMM,
    /// 256-bit YMM
    YMM,
    /// 512-bit ZMM
    ZMM,
    /// Scalar single (S)
    Single,
    /// Scalar double (D)
    Double,
    /// Scalar quad (Q)
    Quad,
}

impl FloatRegisterLength {
    pub fn bytes(self) -> usize {
        match self {
            FloatRegisterLength::XMM | FloatRegisterLength::Single => 16,
            FloatRegisterLength::YMM | FloatRegisterLength::Double => 32,
            FloatRegisterLength::ZMM | FloatRegisterLength::Quad => 64,
        }
    }

    pub fn is_vector(self) -> bool {
        matches!(self, FloatRegisterLength::XMM | FloatRegisterLength::YMM | FloatRegisterLength::ZMM)
    }
}

/// Floating-point / vector register (xmm0..15 with a length tag).
/// `code` is the encoding 0..15; `u8::MAX` denotes noreg.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FloatRegister {
    code: u8,
    len: FloatRegisterLength,
}

impl FloatRegister {
    pub const COUNT: usize = 16;

    pub const XMM0: FloatRegister = FloatRegister { code: 0, len: FloatRegisterLength::XMM };
    pub const XMM1: FloatRegister = FloatRegister { code: 1, len: FloatRegisterLength::XMM };
    pub const XMM2: FloatRegister = FloatRegister { code: 2, len: FloatRegisterLength::XMM };
    pub const XMM3: FloatRegister = FloatRegister { code: 3, len: FloatRegisterLength::XMM };
    pub const XMM4: FloatRegister = FloatRegister { code: 4, len: FloatRegisterLength::XMM };
    pub const XMM5: FloatRegister = FloatRegister { code: 5, len: FloatRegisterLength::XMM };
    pub const XMM6: FloatRegister = FloatRegister { code: 6, len: FloatRegisterLength::XMM };
    pub const XMM7: FloatRegister = FloatRegister { code: 7, len: FloatRegisterLength::XMM };
    pub const XMM8: FloatRegister = FloatRegister { code: 8, len: FloatRegisterLength::XMM };
    pub const XMM9: FloatRegister = FloatRegister { code: 9, len: FloatRegisterLength::XMM };
    pub const XMM10: FloatRegister = FloatRegister { code: 10, len: FloatRegisterLength::XMM };
    pub const XMM11: FloatRegister = FloatRegister { code: 11, len: FloatRegisterLength::XMM };
    pub const XMM12: FloatRegister = FloatRegister { code: 12, len: FloatRegisterLength::XMM };
    pub const XMM13: FloatRegister = FloatRegister { code: 13, len: FloatRegisterLength::XMM };
    pub const XMM14: FloatRegister = FloatRegister { code: 14, len: FloatRegisterLength::XMM };
    pub const XMM15: FloatRegister = FloatRegister { code: 15, len: FloatRegisterLength::XMM };

    pub const NOREG: FloatRegister = FloatRegister { code: u8::MAX, len: FloatRegisterLength::XMM };

    pub const ALL: [FloatRegister; 16] = [
        FloatRegister::XMM0, FloatRegister::XMM1, FloatRegister::XMM2, FloatRegister::XMM3,
        FloatRegister::XMM4, FloatRegister::XMM5, FloatRegister::XMM6, FloatRegister::XMM7,
        FloatRegister::XMM8, FloatRegister::XMM9, FloatRegister::XMM10, FloatRegister::XMM11,
        FloatRegister::XMM12, FloatRegister::XMM13, FloatRegister::XMM14, FloatRegister::XMM15,
    ];

    #[inline]
    pub fn new(code: u8, len: FloatRegisterLength) -> Self {
        Self { code, len }
    }

    #[inline]
    pub fn code(self) -> u8 {
        self.code
    }

    #[inline]
    pub fn len(self) -> FloatRegisterLength {
        self.len
    }

    #[inline]
    pub fn encoding(self) -> u8 {
        assert!(self.is_valid(), "invalid float register");
        self.code
    }

    #[inline]
    pub fn is_valid(self) -> bool {
        self.code < Self::COUNT as u8
    }

    /// View as 128-bit XMM register.
    #[inline]
    pub fn as_XMMRegister(self) -> FloatRegister {
        FloatRegister { code: self.code, len: FloatRegisterLength::XMM }
    }

    /// View with a different vector length (ymm/zmm of same index).
    #[inline]
    pub fn with_len(self, len: FloatRegisterLength) -> FloatRegister {
        FloatRegister { code: self.code, len }
    }

    #[inline]
    pub fn needs_rex(self) -> bool {
        self.code >= 8
    }

    pub fn name(self) -> &'static str {
        let prefix = match self.len {
            FloatRegisterLength::YMM => "ymm",
            FloatRegisterLength::ZMM => "zmm",
            _ => "xmm",
        };
        match (prefix, self.code) {
            ("xmm", 0) => "xmm0", ("xmm", 1) => "xmm1", ("xmm", 2) => "xmm2",
            ("xmm", 3) => "xmm3", ("xmm", 4) => "xmm4", ("xmm", 5) => "xmm5",
            ("xmm", 6) => "xmm6", ("xmm", 7) => "xmm7", ("xmm", 8) => "xmm8",
            ("xmm", 9) => "xmm9", ("xmm", 10) => "xmm10", ("xmm", 11) => "xmm11",
            ("xmm", 12) => "xmm12", ("xmm", 13) => "xmm13", ("xmm", 14) => "xmm14",
            ("xmm", 15) => "xmm15",
            ("ymm", 0) => "ymm0", ("ymm", 1) => "ymm1", ("ymm", 2) => "ymm2",
            ("ymm", 3) => "ymm3", ("ymm", 4) => "ymm4", ("ymm", 5) => "ymm5",
            ("ymm", 6) => "ymm6", ("ymm", 7) => "ymm7", ("ymm", 8) => "ymm8",
            ("ymm", 9) => "ymm9", ("ymm", 10) => "ymm10", ("ymm", 11) => "ymm11",
            ("ymm", 12) => "ymm12", ("ymm", 13) => "ymm13", ("ymm", 14) => "ymm14",
            ("ymm", 15) => "ymm15",
            ("zmm", 0) => "zmm0", ("zmm", 1) => "zmm1", ("zmm", 2) => "zmm2",
            ("zmm", 3) => "zmm3", ("zmm", 4) => "zmm4", ("zmm", 5) => "zmm5",
            ("zmm", 6) => "zmm6", ("zmm", 7) => "zmm7", ("zmm", 8) => "zmm8",
            ("zmm", 9) => "zmm9", ("zmm", 10) => "zmm10", ("zmm", 11) => "zmm11",
            ("zmm", 12) => "zmm12", ("zmm", 13) => "zmm13", ("zmm", 14) => "zmm14",
            ("zmm", 15) => "zmm15",
            _ => "noreg",
        }
    }

    /// System V AMD64 vector argument registers.
    pub const C_ARG_REGS: [FloatRegister; 8] = [
        FloatRegister::XMM0, FloatRegister::XMM1, FloatRegister::XMM2, FloatRegister::XMM3,
        FloatRegister::XMM4, FloatRegister::XMM5, FloatRegister::XMM6, FloatRegister::XMM7,
    ];
}

impl fmt::Debug for FloatRegister {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}
impl fmt::Display for FloatRegister {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// x86 RFLAGS bit mask (like the `cc` field of emitted bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ConditionCode {
    O = 0x0,
    NO = 0x1,
    B = 0x2,
    AE = 0x3,
    E = 0x4,
    NE = 0x5,
    BE = 0x6,
    A = 0x7,
    S = 0x8,
    NS = 0x9,
    P = 0xA,
    NP = 0xB,
    L = 0xC,
    GE = 0xD,
    LE = 0xE,
    G = 0xF,
}

impl ConditionCode {
    pub fn invert(self) -> ConditionCode {
        match self {
            ConditionCode::O => ConditionCode::NO,
            ConditionCode::NO => ConditionCode::O,
            ConditionCode::B => ConditionCode::AE,
            ConditionCode::AE => ConditionCode::B,
            ConditionCode::E => ConditionCode::NE,
            ConditionCode::NE => ConditionCode::E,
            ConditionCode::BE => ConditionCode::A,
            ConditionCode::A => ConditionCode::BE,
            ConditionCode::S => ConditionCode::NS,
            ConditionCode::NS => ConditionCode::S,
            ConditionCode::P => ConditionCode::NP,
            ConditionCode::NP => ConditionCode::P,
            ConditionCode::L => ConditionCode::GE,
            ConditionCode::GE => ConditionCode::L,
            ConditionCode::LE => ConditionCode::G,
            ConditionCode::G => ConditionCode::LE,
        }
    }

    pub fn mnemonic(self) -> &'static str {
        match self {
            ConditionCode::O => "o", ConditionCode::NO => "no",
            ConditionCode::B => "b", ConditionCode::AE => "ae",
            ConditionCode::E => "e", ConditionCode::NE => "ne",
            ConditionCode::BE => "be", ConditionCode::A => "a",
            ConditionCode::S => "s", ConditionCode::NS => "ns",
            ConditionCode::P => "p", ConditionCode::NP => "np",
            ConditionCode::L => "l", ConditionCode::GE => "ge",
            ConditionCode::LE => "le", ConditionCode::G => "g",
        }
    }

    pub fn encoding(self) -> u8 {
        self as u8 & 0x0f
    }
}

/// Scale factor for SIB addressing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Scale {
    S1 = 0,
    S2 = 1,
    S4 = 2,
    S8 = 3,
}

impl Scale {
    pub fn bytes(self) -> u32 {
        match self {
            Scale::S1 => 1, Scale::S2 => 2, Scale::S4 => 4, Scale::S8 => 8,
        }
    }
    pub fn encoding(self) -> u8 {
        self as u8
    }
}

/// Operand size code used by the REX.W / operand-size prefix logic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperandSize {
    Size1 = 0,
    Size2 = 1,
    Size4 = 2,
    Size8 = 3,
}

impl OperandSize {
    pub fn bytes(self) -> usize {
        match self {
            OperandSize::Size1 => 1,
            OperandSize::Size2 => 2,
            OperandSize::Size4 => 4,
            OperandSize::Size8 => 8,
        }
    }
    pub fn from_bytes(n: usize) -> Option<Self> {
        match n {
            1 => Some(OperandSize::Size1),
            2 => Some(OperandSize::Size2),
            4 => Some(OperandSize::Size4),
            8 => Some(OperandSize::Size8),
            _ => None,
        }
    }
}
