/*
 * Port of OpenJDK 25 HotSpot Bytecodes (share/interpreter/bytecodes.hpp, bytecodes.cpp)
 *
 * Bytecodes specifies all bytecodes used in the VM and
 * provides utility functions to get bytecode attributes.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum Code {
    #[default]
    Illegal = -1,

    // Java bytecodes
    Nop = 0,
    AconstNull = 1,
    IconstM1 = 2,
    Iconst0 = 3,
    Iconst1 = 4,
    Iconst2 = 5,
    Iconst3 = 6,
    Iconst4 = 7,
    Iconst5 = 8,
    Lconst0 = 9,
    Lconst1 = 10,
    Fconst0 = 11,
    Fconst1 = 12,
    Fconst2 = 13,
    Dconst0 = 14,
    Dconst1 = 15,
    Bipush = 16,
    Sipush = 17,
    Ldc = 18,
    LdcW = 19,
    Ldc2W = 20,
    Iload = 21,
    Lload = 22,
    Fload = 23,
    Dload = 24,
    Aload = 25,
    Iload0 = 26,
    Iload1 = 27,
    Iload2 = 28,
    Iload3 = 29,
    Lload0 = 30,
    Lload1 = 31,
    Lload2 = 32,
    Lload3 = 33,
    Fload0 = 34,
    Fload1 = 35,
    Fload2 = 36,
    Fload3 = 37,
    Dload0 = 38,
    Dload1 = 39,
    Dload2 = 40,
    Dload3 = 41,
    Aload0 = 42,
    Aload1 = 43,
    Aload2 = 44,
    Aload3 = 45,
    Iaload = 46,
    Laload = 47,
    Faload = 48,
    Daload = 49,
    Aaload = 50,
    Baload = 51,
    Caload = 52,
    Saload = 53,
    Istore = 54,
    Lstore = 55,
    Fstore = 56,
    Dstore = 57,
    Astore = 58,
    Istore0 = 59,
    Istore1 = 60,
    Istore2 = 61,
    Istore3 = 62,
    Lstore0 = 63,
    Lstore1 = 64,
    Lstore2 = 65,
    Lstore3 = 66,
    Fstore0 = 67,
    Fstore1 = 68,
    Fstore2 = 69,
    Fstore3 = 70,
    Dstore0 = 71,
    Dstore1 = 72,
    Dstore2 = 73,
    Dstore3 = 74,
    Astore0 = 75,
    Astore1 = 76,
    Astore2 = 77,
    Astore3 = 78,
    Iastore = 79,
    Lastore = 80,
    Fastore = 81,
    Dastore = 82,
    Aastore = 83,
    Bastore = 84,
    Castore = 85,
    Sastore = 86,
    Pop = 87,
    Pop2 = 88,
    Dup = 89,
    DupX1 = 90,
    DupX2 = 91,
    Dup2 = 92,
    Dup2X1 = 93,
    Dup2X2 = 94,
    Swap = 95,
    Iadd = 96,
    Ladd = 97,
    Fadd = 98,
    Dadd = 99,
    Isub = 100,
    Lsub = 101,
    Fsub = 102,
    Dsub = 103,
    Imul = 104,
    Lmul = 105,
    Fmul = 106,
    Dmul = 107,
    Idiv = 108,
    Ldiv = 109,
    Fdiv = 110,
    Ddiv = 111,
    Irem = 112,
    Lrem = 113,
    Frem = 114,
    Drem = 115,
    Ineg = 116,
    Lneg = 117,
    Fneg = 118,
    Dneg = 119,
    Ishl = 120,
    Lshl = 121,
    Ishr = 122,
    Lshr = 123,
    Iushr = 124,
    Lushr = 125,
    Iand = 126,
    Land = 127,
    Ior = 128,
    Lor = 129,
    Ixor = 130,
    Lxor = 131,
    Iinc = 132,
    I2l = 133,
    I2f = 134,
    I2d = 135,
    L2i = 136,
    L2f = 137,
    L2d = 138,
    F2i = 139,
    F2l = 140,
    F2d = 141,
    D2i = 142,
    D2l = 143,
    D2f = 144,
    I2b = 145,
    I2c = 146,
    I2s = 147,
    Lcmp = 148,
    Fcmpl = 149,
    Fcmpg = 150,
    Dcmpl = 151,
    Dcmpg = 152,
    Ifeq = 153,
    Ifne = 154,
    Iflt = 155,
    Ifge = 156,
    Ifgt = 157,
    Ifle = 158,
    IfIcmpeq = 159,
    IfIcmpne = 160,
    IfIcmplt = 161,
    IfIcmpge = 162,
    IfIcmpgt = 163,
    IfIcmple = 164,
    IfAcmpeq = 165,
    IfAcmpne = 166,
    Goto = 167,
    Jsr = 168,
    Ret = 169,
    Tableswitch = 170,
    Lookupswitch = 171,
    Ireturn = 172,
    Lreturn = 173,
    Freturn = 174,
    Dreturn = 175,
    Areturn = 176,
    Return = 177,
    Getstatic = 178,
    Putstatic = 179,
    Getfield = 180,
    Putfield = 181,
    Invokevirtual = 182,
    Invokespecial = 183,
    Invokestatic = 184,
    Invokeinterface = 185,
    Invokedynamic = 186,
    New = 187,
    Newarray = 188,
    Anewarray = 189,
    Arraylength = 190,
    Athrow = 191,
    Checkcast = 192,
    Instanceof = 193,
    Monitorenter = 194,
    Monitorexit = 195,
    Wide = 196,
    Multianewarray = 197,
    Ifnull = 198,
    Ifnonnull = 199,
    GotoW = 200,
    JsrW = 201,
    Breakpoint = 202,

    // Fast and JVM internal codes
    FastAgetfield = 203,
    FastBgetfield = 204,
    FastCgetfield = 205,
    FastDgetfield = 206,
    FastFgetfield = 207,
    FastIgetfield = 208,
    FastLgetfield = 209,
    FastSgetfield = 210,
    FastAputfield = 211,
    FastBputfield = 212,
    FastZputfield = 213,
    FastCputfield = 214,
    FastDputfield = 215,
    FastFputfield = 216,
    FastIputfield = 217,
    FastLputfield = 218,
    FastSputfield = 219,
    FastAload0 = 220,
    FastIaccess0 = 221,
    FastAaccess0 = 222,
    FastFaccess0 = 223,
    FastIload = 224,
    FastIload2 = 225,
    FastIcaload = 226,
    FastInvokevfinal = 227,
    FastLinearswitch = 228,
    FastBinaryswitch = 229,
    FastAldc = 230,
    FastAldcW = 231,
    ReturnRegisterFinalizer = 232,
    Invokehandle = 233,
    // Bytecodes rewritten at CDS dump time so they are not rewritten again
    // at run time (share/interpreter/bytecodes.hpp).
    NofastGetfield = 234,
    NofastPutfield = 235,
    NofastAload0 = 236,
    NofastIload = 237,
    ShouldNotReachHere = 238,
}

impl Code {
    pub fn from_u8(opcode: u8) -> Option<Self> {
        static CODES: [Code; 239] = [
            Code::Nop,
            Code::AconstNull,
            Code::IconstM1,
            Code::Iconst0,
            Code::Iconst1,
            Code::Iconst2,
            Code::Iconst3,
            Code::Iconst4,
            Code::Iconst5,
            Code::Lconst0,
            Code::Lconst1,
            Code::Fconst0,
            Code::Fconst1,
            Code::Fconst2,
            Code::Dconst0,
            Code::Dconst1,
            Code::Bipush,
            Code::Sipush,
            Code::Ldc,
            Code::LdcW,
            Code::Ldc2W,
            Code::Iload,
            Code::Lload,
            Code::Fload,
            Code::Dload,
            Code::Aload,
            Code::Iload0,
            Code::Iload1,
            Code::Iload2,
            Code::Iload3,
            Code::Lload0,
            Code::Lload1,
            Code::Lload2,
            Code::Lload3,
            Code::Fload0,
            Code::Fload1,
            Code::Fload2,
            Code::Fload3,
            Code::Dload0,
            Code::Dload1,
            Code::Dload2,
            Code::Dload3,
            Code::Aload0,
            Code::Aload1,
            Code::Aload2,
            Code::Aload3,
            Code::Iaload,
            Code::Laload,
            Code::Faload,
            Code::Daload,
            Code::Aaload,
            Code::Baload,
            Code::Caload,
            Code::Saload,
            Code::Istore,
            Code::Lstore,
            Code::Fstore,
            Code::Dstore,
            Code::Astore,
            Code::Istore0,
            Code::Istore1,
            Code::Istore2,
            Code::Istore3,
            Code::Lstore0,
            Code::Lstore1,
            Code::Lstore2,
            Code::Lstore3,
            Code::Fstore0,
            Code::Fstore1,
            Code::Fstore2,
            Code::Fstore3,
            Code::Dstore0,
            Code::Dstore1,
            Code::Dstore2,
            Code::Dstore3,
            Code::Astore0,
            Code::Astore1,
            Code::Astore2,
            Code::Astore3,
            Code::Iastore,
            Code::Lastore,
            Code::Fastore,
            Code::Dastore,
            Code::Aastore,
            Code::Bastore,
            Code::Castore,
            Code::Sastore,
            Code::Pop,
            Code::Pop2,
            Code::Dup,
            Code::DupX1,
            Code::DupX2,
            Code::Dup2,
            Code::Dup2X1,
            Code::Dup2X2,
            Code::Swap,
            Code::Iadd,
            Code::Ladd,
            Code::Fadd,
            Code::Dadd,
            Code::Isub,
            Code::Lsub,
            Code::Fsub,
            Code::Dsub,
            Code::Imul,
            Code::Lmul,
            Code::Fmul,
            Code::Dmul,
            Code::Idiv,
            Code::Ldiv,
            Code::Fdiv,
            Code::Ddiv,
            Code::Irem,
            Code::Lrem,
            Code::Frem,
            Code::Drem,
            Code::Ineg,
            Code::Lneg,
            Code::Fneg,
            Code::Dneg,
            Code::Ishl,
            Code::Lshl,
            Code::Ishr,
            Code::Lshr,
            Code::Iushr,
            Code::Lushr,
            Code::Iand,
            Code::Land,
            Code::Ior,
            Code::Lor,
            Code::Ixor,
            Code::Lxor,
            Code::Iinc,
            Code::I2l,
            Code::I2f,
            Code::I2d,
            Code::L2i,
            Code::L2f,
            Code::L2d,
            Code::F2i,
            Code::F2l,
            Code::F2d,
            Code::D2i,
            Code::D2l,
            Code::D2f,
            Code::I2b,
            Code::I2c,
            Code::I2s,
            Code::Lcmp,
            Code::Fcmpl,
            Code::Fcmpg,
            Code::Dcmpl,
            Code::Dcmpg,
            Code::Ifeq,
            Code::Ifne,
            Code::Iflt,
            Code::Ifge,
            Code::Ifgt,
            Code::Ifle,
            Code::IfIcmpeq,
            Code::IfIcmpne,
            Code::IfIcmplt,
            Code::IfIcmpge,
            Code::IfIcmpgt,
            Code::IfIcmple,
            Code::IfAcmpeq,
            Code::IfAcmpne,
            Code::Goto,
            Code::Jsr,
            Code::Ret,
            Code::Tableswitch,
            Code::Lookupswitch,
            Code::Ireturn,
            Code::Lreturn,
            Code::Freturn,
            Code::Dreturn,
            Code::Areturn,
            Code::Return,
            Code::Getstatic,
            Code::Putstatic,
            Code::Getfield,
            Code::Putfield,
            Code::Invokevirtual,
            Code::Invokespecial,
            Code::Invokestatic,
            Code::Invokeinterface,
            Code::Invokedynamic,
            Code::New,
            Code::Newarray,
            Code::Anewarray,
            Code::Arraylength,
            Code::Athrow,
            Code::Checkcast,
            Code::Instanceof,
            Code::Monitorenter,
            Code::Monitorexit,
            Code::Wide,
            Code::Multianewarray,
            Code::Ifnull,
            Code::Ifnonnull,
            Code::GotoW,
            Code::JsrW,
            Code::Breakpoint,
            Code::FastAgetfield,
            Code::FastBgetfield,
            Code::FastCgetfield,
            Code::FastDgetfield,
            Code::FastFgetfield,
            Code::FastIgetfield,
            Code::FastLgetfield,
            Code::FastSgetfield,
            Code::FastAputfield,
            Code::FastBputfield,
            Code::FastZputfield,
            Code::FastCputfield,
            Code::FastDputfield,
            Code::FastFputfield,
            Code::FastIputfield,
            Code::FastLputfield,
            Code::FastSputfield,
            Code::FastAload0,
            Code::FastIaccess0,
            Code::FastAaccess0,
            Code::FastFaccess0,
            Code::FastIload,
            Code::FastIload2,
            Code::FastIcaload,
            Code::FastInvokevfinal,
            Code::FastLinearswitch,
            Code::FastBinaryswitch,
            Code::FastAldc,
            Code::FastAldcW,
            Code::ReturnRegisterFinalizer,
            Code::Invokehandle,
            Code::NofastGetfield,
            Code::NofastPutfield,
            Code::NofastAload0,
            Code::NofastIload,
            Code::ShouldNotReachHere,
        ];
        CODES.get(opcode as usize).copied()
    }
}

pub fn code_from_u8(opcode: u8) -> Option<Code> {
    Code::from_u8(opcode)
}

pub const NUMBER_OF_JAVA_CODES: usize = 203;

pub fn name(opcode: u8) -> Option<&'static str> {
    #[allow(non_upper_case_globals)]
    const NAMES: [&str; NUMBER_OF_CODES] = [
        "nop",
        "aconst_null",
        "iconst_m1",
        "iconst_0",
        "iconst_1",
        "iconst_2",
        "iconst_3",
        "iconst_4",
        "iconst_5",
        "lconst_0",
        "lconst_1",
        "fconst_0",
        "fconst_1",
        "fconst_2",
        "dconst_0",
        "dconst_1",
        "bipush",
        "sipush",
        "ldc",
        "ldc_w",
        "ldc2_w",
        "iload",
        "lload",
        "fload",
        "dload",
        "aload",
        "iload_0",
        "iload_1",
        "iload_2",
        "iload_3",
        "lload_0",
        "lload_1",
        "lload_2",
        "lload_3",
        "fload_0",
        "fload_1",
        "fload_2",
        "fload_3",
        "dload_0",
        "dload_1",
        "dload_2",
        "dload_3",
        "aload_0",
        "aload_1",
        "aload_2",
        "aload_3",
        "iaload",
        "laload",
        "faload",
        "daload",
        "aaload",
        "baload",
        "caload",
        "saload",
        "istore",
        "lstore",
        "fstore",
        "dstore",
        "astore",
        "istore_0",
        "istore_1",
        "istore_2",
        "istore_3",
        "lstore_0",
        "lstore_1",
        "lstore_2",
        "lstore_3",
        "fstore_0",
        "fstore_1",
        "fstore_2",
        "fstore_3",
        "dstore_0",
        "dstore_1",
        "dstore_2",
        "dstore_3",
        "astore_0",
        "astore_1",
        "astore_2",
        "astore_3",
        "iastore",
        "lastore",
        "fastore",
        "dastore",
        "aastore",
        "bastore",
        "castore",
        "sastore",
        "pop",
        "pop2",
        "dup",
        "dup_x1",
        "dup_x2",
        "dup2",
        "dup2_x1",
        "dup2_x2",
        "swap",
        "iadd",
        "ladd",
        "fadd",
        "dadd",
        "isub",
        "lsub",
        "fsub",
        "dsub",
        "imul",
        "lmul",
        "fmul",
        "dmul",
        "idiv",
        "ldiv",
        "fdiv",
        "ddiv",
        "irem",
        "lrem",
        "frem",
        "drem",
        "ineg",
        "lneg",
        "fneg",
        "dneg",
        "ishl",
        "lshl",
        "ishr",
        "lshr",
        "iushr",
        "lushr",
        "iand",
        "land",
        "ior",
        "lor",
        "ixor",
        "lxor",
        "iinc",
        "i2l",
        "i2f",
        "i2d",
        "l2i",
        "l2f",
        "l2d",
        "f2i",
        "f2l",
        "f2d",
        "d2i",
        "d2l",
        "d2f",
        "i2b",
        "i2c",
        "i2s",
        "lcmp",
        "fcmpl",
        "fcmpg",
        "dcmpl",
        "dcmpg",
        "ifeq",
        "ifne",
        "iflt",
        "ifge",
        "ifgt",
        "ifle",
        "if_icmpeq",
        "if_icmpne",
        "if_icmplt",
        "if_icmpge",
        "if_icmpgt",
        "if_icmple",
        "if_acmpeq",
        "if_acmpne",
        "goto",
        "jsr",
        "ret",
        "tableswitch",
        "lookupswitch",
        "ireturn",
        "lreturn",
        "freturn",
        "dreturn",
        "areturn",
        "return",
        "getstatic",
        "putstatic",
        "getfield",
        "putfield",
        "invokevirtual",
        "invokespecial",
        "invokestatic",
        "invokeinterface",
        "invokedynamic",
        "new",
        "newarray",
        "anewarray",
        "arraylength",
        "athrow",
        "checkcast",
        "instanceof",
        "monitorenter",
        "monitorexit",
        "wide",
        "multianewarray",
        "ifnull",
        "ifnonnull",
        "goto_w",
        "jsr_w",
        "breakpoint",
        "fast_agetfield",
        "fast_bgetfield",
        "fast_cgetfield",
        "fast_dgetfield",
        "fast_fgetfield",
        "fast_igetfield",
        "fast_lgetfield",
        "fast_sgetfield",
        "fast_aputfield",
        "fast_bputfield",
        "fast_zputfield",
        "fast_cputfield",
        "fast_dputfield",
        "fast_fputfield",
        "fast_iputfield",
        "fast_lputfield",
        "fast_sputfield",
        "fast_aload_0",
        "fast_iaccess_0",
        "fast_aaccess_0",
        "fast_faccess_0",
        "fast_iload",
        "fast_iload2",
        "fast_icaload",
        "fast_invokevfinal",
        "fast_linearswitch",
        "fast_binaryswitch",
        "fast_aldc",
        "fast_aldc_w",
        "return_register_finalizer",
        "invokehandle",
        "nofast_getfield",
        "nofast_putfield",
        "nofast_aload_0",
        "nofast_iload",
        "_shouldnotreachhere",
    ];
    NAMES.get(usize::from(opcode)).copied()
}

pub const NUMBER_OF_CODES: usize = 239;

pub fn is_valid(opcode: i32) -> bool {
    (0..NUMBER_OF_CODES as i32).contains(&opcode)
}

pub fn is_java_code(opcode: u8) -> bool {
    usize::from(opcode) < NUMBER_OF_JAVA_CODES
}

pub fn is_defined(opcode: u8) -> bool {
    name(opcode).is_some()
}

// Fixed instruction lengths derived from the format strings in
// bytecodes.cpp (`_lengths` table).  Returns 0 for variable-length
// instructions (wide, tableswitch, lookupswitch and the JVM-internal
// fast switch variants) and -1 for undefined opcodes, matching
// `Bytecodes::length_for`.
pub fn length_for(opcode: u8) -> i32 {
    match opcode {
        0x00..=0x0f
        | 0x1a..=0x35
        | 0x3b..=0x56
        | 0x57..=0x83
        | 0x85..=0x98
        | 0xac..=0xb1
        | 0xbe..=0xbf
        | 0xc2..=0xc3
        | 0xca => 1,
        0x10 | 0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => 2,
        0x11
        | 0x13..=0x14
        | 0x84
        | 0x99..=0xa8
        | 0xb2..=0xb8
        | 0xbb
        | 0xbd
        | 0xc0..=0xc1
        | 0xc6..=0xc7 => 3,
        0xc5 => 4,
        0xb9 | 0xba | 0xc8 | 0xc9 => 5,
        // Variable-length instructions: wide, tableswitch, lookupswitch.
        0xaa | 0xab | 0xc4 => 0,
        // JVM-internal bytecodes (codes 203..=238), formats "bJJ" etc.
        0xcb..=0xd2 | 0xd3..=0xdb | 0xe3 | 0xe7 | 0xe9 | 0xea | 0xeb => 3,
        0xdc | 0xe8 | 0xec | 0xee => 1,
        0xdd..=0xdf | 0xe1 => 4,
        0xe0 | 0xe6 => 2,
        0xe2 => 3,
        0xe4 | 0xe5 => 0, // fast_linearswitch, fast_binaryswitch
        0xed => 2,        // nofast_iload "bi"
        _ => -1,
    }
}

pub fn wide_length_for(opcode: u8) -> i32 {
    match opcode {
        0x15..=0x19 | 0x36..=0x3a | 0xa9 => 4, // wide load/store/ret: wide, op, index_u2
        0x84 => 6,                             // wide iinc: wide, op, index_u2, const_s2
        _ => -1,
    }
}

pub fn is_store_into_local(opcode: u8) -> bool {
    (0x36..=0x4e).contains(&opcode)
}

pub fn is_const(opcode: u8) -> bool {
    (0x01..=0x14).contains(&opcode)
}

pub fn is_zero_const(opcode: u8) -> bool {
    matches!(opcode, 0x01 | 0x03 | 0x0b | 0x0e)
}

pub fn is_return(opcode: u8) -> bool {
    (0xac..=0xb1).contains(&opcode)
}

pub fn is_invoke(opcode: u8) -> bool {
    (0xb6..=0xba).contains(&opcode)
}

pub fn has_receiver(opcode: u8) -> bool {
    matches!(opcode, 0xb6 | 0xb7 | 0xb9)
}

pub fn has_optional_appendix(opcode: u8) -> bool {
    opcode == 0xba
}

pub fn is_field_code(opcode: u8) -> bool {
    (0xb2..=0xb5).contains(&opcode)
}

pub fn can_trap(opcode: u8) -> bool {
    match opcode {
        0x2a // aload_0 (may rewrite to a trapping access)
        | 0x2e..=0x35 // *aload
        | 0x4f..=0x56 // *astore
        | 0x6c..=0x6f // *div
        | 0x70..=0x73 // *rem
        | 0xb2..=0xba // field and invoke
        | 0xbb..=0xbd // new, newarray, anewarray
        | 0xbe // arraylength
        | 0xbf // athrow
        | 0xc0 // checkcast
        | 0xc2..=0xc3 // monitorenter, monitorexit
        | 0xc5 => true, // multianewarray
        // JVM-internal codes.
        203..=223 // fast_*getfield/putfield/aload_0/access_0
        | 227 // fast_invokevfinal
        | 230..=236 => true, // fast_aldc*, return_register_finalizer, invokehandle, nofast_getfield/putfield/aload_0
        _ => false,
    }
}

// `Bytecodes::special_length_at`: length of a variable-length
// instruction at `bci` inside the code array `code`.  Returns -1 when
// the instruction would read past the end of `code` or is malformed.
pub fn special_length_at(code: &[u8], bci: usize) -> i32 {
    let opcode = match code.get(bci) {
        Some(&opcode) => opcode,
        None => return -1,
    };
    match opcode {
        0xc4 => {
            // wide
            match code.get(bci + 1) {
                Some(&sub) => wide_length_for(sub),
                None => -1,
            }
        }
        0xaa | 0xe4 => {
            // tableswitch, fast_linearswitch
            let aligned = align4(bci + 1);
            let (Some(lo), Some(hi)) = (
                be_i32_at(code, aligned + 4),
                be_i32_at(code, aligned + 8),
            ) else {
                return -1;
            };
            if lo > hi {
                return -1;
            }
            let len = (aligned - bci) as i64 + (3 + hi as i64 - lo as i64 + 1) * 4;
            if len > 0 && len <= i32::MAX as i64 {
                len as i32
            } else {
                -1
            }
        }
        0xab | 0xe5 => {
            // lookupswitch, fast_binaryswitch
            let aligned = align4(bci + 1);
            let Some(npairs) = be_i32_at(code, aligned + 4) else {
                return -1;
            };
            if npairs < 0 {
                return -1;
            }
            let len = (aligned - bci) as i64 + (2 + 2 * npairs as i64) * 4;
            if len > 0 && len <= i32::MAX as i64 {
                len as i32
            } else {
                -1
            }
        }
        _ => 0,
    }
}

fn align4(offset: usize) -> usize {
    (offset + 3) & !3
}

fn be_i32_at(code: &[u8], offset: usize) -> Option<i32> {
    let bytes: [u8; 4] = code.get(offset..offset + 4)?.try_into().ok()?;
    Some(i32::from_be_bytes(bytes))
}

// `Bytecodes::length_for_code_at`: fixed length, or the variable
// length computed by `special_length_at` when `length_for` returns 0.
pub fn length_for_code_at(code: &Code, bci: usize, bytes: &[u8]) -> i32 {
    let len = length_for(*code as u8);
    if len > 0 {
        len
    } else {
        special_length_at(bytes, bci)
    }
}

// `Bytecodes::code_at` for a raw code array: byte value mapped through
// the `Code` enum (breakpoint decoding is the caller's concern).
pub fn code_at(bytes: &[u8], bci: usize) -> Option<Code> {
    code_from_u8(*bytes.get(bci)?)
}

// `Bytecodes::java_code_at`: the canonical Java bytecode for a
// (possibly JVM-internal rewritten) code value.
pub fn java_code_at(bytes: &[u8], bci: usize) -> Option<Code> {
    code_at(bytes, bci).map(java_code)
}

// `Bytecodes::java_length_at`.
pub fn java_length_at(bytes: &[u8], bci: usize) -> i32 {
    match java_code_at(bytes, bci) {
        Some(code) => length_for_code_at(&code, bci, bytes),
        None => -1,
    }
}

// Convenience wrapper kept for existing callers: instruction length in
// bytes, clamped to at least 1 for malformed input.
pub fn length_at(code: &[u8], bci: usize) -> usize {
    let len = instruction_length_at(code, bci);
    if len > 0 { len as usize } else { 1 }
}

// `Bytecodes::length_at` over a raw code array.
pub fn instruction_length_at(bytes: &[u8], bci: usize) -> i32 {
    match code_at(bytes, bci) {
        Some(code) => length_for_code_at(&code, bci, bytes),
        None => -1,
    }
}

// Iterate over instruction boundaries in a code array.  Yields
// `(bci, code, length)`; stops at malformed input.  Variable-length
// instructions are sized via `special_length_at`.
pub fn instructions(bytes: &[u8]) -> InstructionIter<'_> {
    InstructionIter { bytes, bci: 0 }
}

pub struct InstructionIter<'a> {
    bytes: &'a [u8],
    bci: usize,
}

impl<'a> Iterator for InstructionIter<'a> {
    type Item = (usize, Code, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.bci >= self.bytes.len() {
            return None;
        }
        let code = code_at(self.bytes, self.bci)?;
        let len = length_for_code_at(&code, self.bci, self.bytes);
        if len <= 0 || self.bci + len as usize > self.bytes.len() {
            return None;
        }
        let item = (self.bci, code, len as usize);
        self.bci += len as usize;
        Some(item)
    }
}

/// HotSpot `BasicType` (globalDefinitions.hpp), including the
/// analysis-only values used by the interpreter's operand-stack
/// simulation (`T_ADDRESS`, `T_CONFLICT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum BasicType {
    Boolean = 0,
    Char = 1,
    Float = 2,
    Double = 3,
    Byte = 4,
    Short = 5,
    Int = 6,
    Long = 7,
    Object = 8,
    Array = 9,
    Address = 10,
    Void = 11,
    Conflict = 12,
    Illegal = 13,
}

/// `type2size`: slot count of a value of the given type.
pub fn type2size(basic_type: BasicType) -> usize {
    match basic_type {
        BasicType::Long | BasicType::Double => 2,
        BasicType::Void | BasicType::Conflict | BasicType::Illegal => 0,
        _ => 1,
    }
}

/// `Bytecodes::result_type`: the type pushed by the bytecode (T_ILLEGAL
/// for instructions without a fixed result).
pub fn result_type(code: Code) -> BasicType {
    use BasicType::*;
    let i = code as i32;
    match i {
        1 => Object,                    // aconst_null
        2..=8 => Int,                   // iconst_*
        9 | 10 => Long,                 // lconst_*
        11..=13 => Float,               // fconst_*
        14 | 15 => Double,              // dconst_*
        16 | 17 => Int,                 // bipush, sipush
        21 => Int,
        22 => Long,
        23 => Float,
        24 => Double,
        25 => Object,
        26..=29 => Int,                 // iload*
        30..=33 => Long,                // lload*
        34..=37 => Float,               // fload*
        38..=41 => Double,              // dload*
        42..=45 => Object,              // aload*
        46 => Int,                      // iaload
        47 => Long,                     // laload
        48 => Float,                    // faload
        49 => Double,                   // daload
        50 => Object,                   // aaload
        51..=53 => Int,                 // baload, caload, saload
        96..=115 => match (i - 96) % 4 { // iadd..drem
            0 => Int,
            1 => Long,
            2 => Float,
            _ => Double,
        },
        116..=119 => match i - 116 {   // ineg..dneg
            0 => Int,
            1 => Long,
            2 => Float,
            _ => Double,
        },
        120 | 122 | 124 | 126 | 128 | 130 => Int, // ish*/iand/ior/ixor
        121 | 123 | 125 | 127 | 129 | 131 => Long, // lsh*/land/lor/lxor
        133 => Long,                    // i2l
        134 => Float,                   // i2f
        135 => Double,                  // i2d
        136 => Int,                     // l2i
        137 => Float,                   // l2f
        138 => Double,                  // l2d
        139 => Int,                     // f2i
        140 => Long,                    // f2l
        141 => Double,                  // f2d
        142 => Int,                     // d2i
        143 => Long,                    // d2l
        144 => Float,                   // d2f
        145 => Byte,                    // i2b
        146 => Char,                    // i2c
        147 => Short,                   // i2s
        168 | 201 => Int,               // jsr, jsr_w push return address slot
        172 => Int,                     // ireturn
        173 => Long,                    // lreturn
        174 => Float,                   // freturn
        175 => Double,                  // dreturn
        176 => Object,                  // areturn
        187 | 188 | 189 | 197 => Object, // new, newarray, anewarray, multianewarray
        190 => Int,                     // arraylength
        192 => Object,                  // checkcast
        203 => Object,                  // fast_agetfield
        204 => Int,                     // fast_bgetfield
        205 => Char,                    // fast_cgetfield
        206 => Double,                  // fast_dgetfield
        207 => Float,                   // fast_fgetfield
        208 => Int,                     // fast_igetfield
        209 => Long,                    // fast_lgetfield
        210 => Short,                   // fast_sgetfield
        220 => Object,                  // fast_aload_0
        221 => Int,                     // fast_iaccess_0
        222 => Object,                  // fast_aaccess_0
        223 => Object,                  // fast_faccess_0
        224 | 225 | 226 => Int,         // fast_iload, fast_iload2, fast_icaload
        230 | 231 => Object,            // fast_aldc, fast_aldc_w
        236 => Object,                  // nofast_aload_0
        237 => Int,                     // nofast_iload
        193 => Int,                     // instanceof
        _ => Illegal,
    }
}

/// `Bytecodes::depth`: net operand-stack effect in slots (negative for
/// pops).  `T_ILLEGAL` results for field/invoke instructions are
/// resolved dynamically by callers.
pub fn depth(code: Code) -> i32 {
    use BasicType::*;
    let _ = (Void, Illegal);
    let i = code as i32;
    match i {
        0 => 0,                         // nop
        1 => 1,                         // aconst_null
        2..=8 => 1,                     // iconst_*
        9 | 10 => 2,                    // lconst_*
        11..=13 => 1,                   // fconst_*
        14 | 15 => 2,                   // dconst_*
        16 | 17 => 1,                   // bipush, sipush
        18 | 19 => 1,                   // ldc, ldc_w
        20 => 2,                        // ldc2_w
        21 => 1,
        22 => 2,
        23 => 1,
        24 => 2,
        25 => 1,
        26..=29 => 1,
        30..=33 => 2,
        34..=37 => 1,
        38..=41 => 2,
        42..=45 => 1,
        46 | 48 | 50..=53 => -1,        // *aload (1-slot element)
        47 | 49 => 0,                   // laload, daload (2-slot element)
        54 | 56 | 58 => -1,             // istore, fstore, astore
        55 | 57 => -2,                  // lstore, dstore
        59..=62 | 67..=70 | 75..=78 => -1,
        63..=66 | 71..=74 => -2,
        79 | 81 | 83..=86 => -3,        // iastore..sastore (1-slot value)
        80 | 82 => -4,                  // lastore, dastore
        87 => -1,                       // pop
        88 => -2,                       // pop2
        89 | 90 | 91 => 1,              // dup, dup_x1, dup_x2
        92..=94 => 2,                   // dup2, dup2_x1, dup2_x2
        95 => 0,                        // swap
        96..=115 => match (i - 96) % 4 { // iadd..drem
            1 | 3 => -2,                // long/double
            _ => -1,
        },
        116..=119 => 0,                 // ineg..dneg
        120..=125 => -1,                // shifts (i- and l- variants pop 2)
        126..=131 => match i % 2 {      // iand..lxor
            0 => -1,
            _ => -2,
        },
        132 => 0,                       // iinc
        133 => 1,                       // i2l
        134 | 135 => 0,                 // i2f, i2d
        136 | 137 => -1,                // l2i, l2f
        138 => 0,                       // l2d
        139 | 142 | 144 => 0,           // f2i, d2i, d2f
        140 | 141 => 1,                 // f2l, f2d
        143 => 0,                       // d2l
        145..=147 => 0,                 // i2b, i2c, i2s
        148 | 151 | 152 => -3,          // lcmp, dcmpl, dcmpg
        149 | 150 => -1,                // fcmpl, fcmpg
        153..=158 => -1,                // if*
        159..=166 => -2,                // if_icmp*, if_acmp*
        167 | 200 => 0,                 // goto, goto_w
        168 | 201 => 0,                 // jsr, jsr_w (push handled by caller)
        169 => 0,                       // ret
        170 | 171 | 0xe4 | 0xe5 => -1,  // *switch
        172 => -1,
        173 => -2,
        174 => -1,
        175 => -2,
        176 => -1,                      // areturn
        177 => 0,                       // return
        178 => 1,                       // getstatic
        179 => -1,                      // putstatic
        180 => 0,                       // getfield
        181 => -2,                      // putfield
        182..=185 => -1,                // invokevirtual..invokeinterface
        186 | 233 => 0,                 // invokedynamic, invokehandle
        187 => 1,                       // new
        188..=189 => 0,                 // newarray, anewarray
        190 => 0,                       // arraylength
        191 => -1,                      // athrow
        192 | 193 => 0,                 // checkcast, instanceof
        194 | 195 => -1,                // monitorenter, monitorexit
        196 => 0,                       // wide
        197 => 1,                       // multianewarray
        198 | 199 => -1,                // ifnull, ifnonnull
        202 => 0,                       // breakpoint
        203..=210 => 0,                 // fast_*getfield
        211..=219 => 0,                 // fast_*putfield
        220 => 1,                       // fast_aload_0
        221..=223 => 1,                 // fast_*access_0
        224 => 1,                       // fast_iload
        225 => 2,                       // fast_iload2
        226 => 0,                       // fast_icaload
        227 => -1,                      // fast_invokevfinal
        230 | 231 => 1,                 // fast_aldc*
        232 => 0,                       // return_register_finalizer
        234 => 0,                       // nofast_getfield
        235 => -2,                      // nofast_putfield
        236 => 1,                       // nofast_aload_0
        237 => 1,                       // nofast_iload
        238 => 0,                       // shouldnotreachhere
        _ => 0,
    }
}

/// `Bytecodes::java_code`: canonical Java bytecode for a code value.
pub fn java_code(code: Code) -> Code {
    match code {
        Code::FastAgetfield
        | Code::FastBgetfield
        | Code::FastCgetfield
        | Code::FastDgetfield
        | Code::FastFgetfield
        | Code::FastIgetfield
        | Code::FastLgetfield
        | Code::FastSgetfield
        | Code::NofastGetfield => Code::Getfield,
        Code::FastAputfield
        | Code::FastBputfield
        | Code::FastZputfield
        | Code::FastCputfield
        | Code::FastDputfield
        | Code::FastFputfield
        | Code::FastIputfield
        | Code::FastLputfield
        | Code::FastSputfield
        | Code::NofastPutfield => Code::Putfield,
        Code::FastAload0 | Code::FastAaccess0 | Code::FastFaccess0 | Code::NofastAload0 => {
            Code::Aload0
        }
        Code::FastIload | Code::FastIload2 | Code::FastIcaload | Code::NofastIload => {
            Code::Iload
        }
        Code::FastIaccess0 => Code::Iload,
        Code::FastInvokevfinal | Code::Invokehandle => Code::Invokevirtual,
        Code::FastLinearswitch | Code::FastBinaryswitch => Code::Lookupswitch,
        Code::FastAldc => Code::Ldc,
        Code::FastAldcW => Code::LdcW,
        Code::ReturnRegisterFinalizer => Code::Return,
        _ => code,
    }
}

// `Bytecodes::can_rewrite`: the bytecode has a JVM-internal rewritten
// form (i.e. `java_code(code) != code`).
pub fn can_rewrite(code: Code) -> bool {
    java_code(code) != code
}

// `Bytecodes::must_rewrite` / `check_must_rewrite`: most rewritable
// bytecodes must be rewritten before execution; the exceptions are
// `aload_0` (rewrite may be delayed), `lookupswitch` (rewritten by the
// Rewriter, not the interpreter) and `new` (not always rewritten).
pub fn must_rewrite(code: Code) -> bool {
    if !can_rewrite(code) {
        return false;
    }
    !matches!(
        code,
        Code::FastAload0 | Code::NofastAload0 | Code::FastLinearswitch | Code::FastBinaryswitch | Code::New
    )
}

// `Bytecodes::uses_cp_cache` (`_fmt_has_j`): the instruction carries a
// constant-pool-cache index operand ('j'/'J' in the format string).
pub fn uses_cp_cache(code: Code) -> bool {
    matches!(code as i32,
        0xb2..=0xba // getstatic..invokedynamic ("bJJ"/"bJJJJ")
        | 203..=219   // fast_*getfield/fast_*putfield ("bJJ")
        | 221..=223   // fast_*access_0 ("b_JJ")
        | 227         // fast_invokevfinal ("bJJ")
        | 230         // fast_aldc ("bj")
        | 231         // fast_aldc_w ("bJJ")
        | 233         // invokehandle ("bJJ")
        | 234..=235   // nofast_getfield/nofast_putfield ("bJJ")
    )
}

// `Bytecodes::native_byte_order` (`_fmt_has_nbo`): the instruction
// carries a native-order ('J') operand written by the Rewriter.
pub fn native_byte_order(code: Code) -> bool {
    matches!(code as i32,
        0xb2..=0xba // getstatic..invokedynamic
        | 203..=219   // fast_*getfield/fast_*putfield
        | 221..=223   // fast_*access_0
        | 227         // fast_invokevfinal
        | 231         // fast_aldc_w
        | 233         // invokehandle
        | 234..=235   // nofast_getfield/nofast_putfield
    )
}

// `Bytecodes::is_defined` for internal codes uses the name table;
// `flags`-style predicates above cover the semantic attributes.

impl std::ops::Add<u8> for Code {
    type Output = Code;
    fn add(self, rhs: u8) -> Code {
        Code::from_u8((self as u8).wrapping_add(rhs)).unwrap_or(Code::Illegal)
    }
}
