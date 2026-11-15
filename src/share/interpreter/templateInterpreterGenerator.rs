/*
 * Port of OpenJDK 25 HotSpot TemplateInterpreterGenerator +
 * TemplateTable::initialize (share/interpreter/templateTable.cpp).
 *
 * The C++ generator emits machine code per bytecode template; this
 * port emits the same *template table*: for every bytecode a
 * `Template` (flags + tos_in/tos_out + generator id + argument) that
 * `TemplateInterpreterGenerator::generate` installs into the
 * `DispatchTable` as symbolic codelet ids.  Execution still delegates
 * to the tree-walking interpreter — the table is the authoritative
 * description of each bytecode's operand use, tos transition and
 * dispatch behaviour.
 */

use super::bytecodes::Code;
use super::templateInterpreter::{CodeletId, DispatchTable, EntryPoint, TemplateInterpreter};
use super::templateTable::{Template, TemplateTable, TosState};

/// TemplateTable operand-use flag bits (`ubcp|disp|clvm|iswd`).
pub const UBCP: u32 = 1 << 0;
pub const DISP: u32 = 1 << 1;
pub const CLVM: u32 = 1 << 2;
pub const ISWD: u32 = 1 << 3;

/// TemplateTable generator functions, one per codelet family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gen {
    Nop,
    AconstNull,
    Iconst,
    Lconst,
    Fconst,
    Dconst,
    Bipush,
    Sipush,
    Ldc,
    Ldc2W,
    Iload,
    Lload,
    Fload,
    Dload,
    Aload,
    Aload0,
    Iaload,
    Laload,
    Faload,
    Daload,
    Aaload,
    Baload,
    Caload,
    Saload,
    Istore,
    Lstore,
    Fstore,
    Dstore,
    Astore,
    Iastore,
    Lastore,
    Fastore,
    Dastore,
    Aastore,
    Bastore,
    Castore,
    Sastore,
    Pop,
    Pop2,
    Dup,
    DupX1,
    DupX2,
    Dup2,
    Dup2X1,
    Dup2X2,
    Swap,
    Iop2,
    Lop2,
    Fop2,
    Dop2,
    Idiv,
    Ldiv,
    Irem,
    Lrem,
    Ineg,
    Lneg,
    Fneg,
    Dneg,
    Ishl,
    Lshl,
    Ishr,
    Lshr,
    Iushr,
    Lushr,
    Iinc,
    Convert,
    Lcmp,
    FloatCmp,
    DoubleCmp,
    If0Cmp,
    IfIcmp,
    IfAcmp,
    IfNullCmp,
    Goto,
    GotoW,
    Jsr,
    JsrW,
    Ret,
    Tableswitch,
    Lookupswitch,
    FastLinearswitch,
    FastBinaryswitch,
    Return,
    ReturnRegisterFinalizer,
    Getstatic,
    Putstatic,
    Getfield,
    Putfield,
    Invokevirtual,
    Invokespecial,
    Invokestatic,
    Invokeinterface,
    Invokedynamic,
    Invokehandle,
    FastInvokevfinal,
    New,
    Newarray,
    Anewarray,
    Multianewarray,
    Arraylength,
    Athrow,
    Checkcast,
    Instanceof,
    Monitorenter,
    Monitorexit,
    Wide,
    WideIload,
    WideLload,
    WideFload,
    WideDload,
    WideAload,
    WideIstore,
    WideLstore,
    WideFstore,
    WideDstore,
    WideAstore,
    WideIinc,
    WideRet,
    Breakpoint,
    FastAccessField,
    FastStoreField,
    FastXaccess,
    FastIload,
    FastIload2,
    FastIcaload,
    FastAldc,
    NofastGetfield,
    NofastPutfield,
    NofastAload0,
    NofastIload,
    ShouldNotReachHere,
    Unimplemented,
}

/// Binary integer/long/float/double operation selector (`iop2`, `lop2`,
/// `fop2`, `dop2` args).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Ushr,
}

impl Op {
    pub const fn name(self) -> &'static str {
        match self {
            Op::Add => "add",
            Op::Sub => "sub",
            Op::Mul => "mul",
            Op::Div => "div",
            Op::Rem => "rem",
            Op::And => "_and",
            Op::Or => "_or",
            Op::Xor => "_xor",
            Op::Shl => "shl",
            Op::Shr => "shr",
            Op::Ushr => "ushr",
        }
    }
}

/// Comparison condition for `if_0cmp`/`if_icmp`/`if_acmp`/`if_nullcmp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    Equal,
    NotEqual,
    Less,
    GreaterEqual,
    Greater,
    LessEqual,
}

impl Condition {
    pub const fn name(self) -> &'static str {
        match self {
            Condition::Equal => "equal",
            Condition::NotEqual => "not_equal",
            Condition::Less => "less",
            Condition::GreaterEqual => "greater_equal",
            Condition::Greater => "greater",
            Condition::LessEqual => "less_equal",
        }
    }
}

/// Field/arg type selector for fast field templates (`f1_byte`, `f2_byte`,
/// `atos`, `itos`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    F1Byte,
    F2Byte,
    Atos,
    Itos,
    Ltos,
    Ftos,
    Dtos,
    Vtos,
}

/// One template-table row: `def(code, flags, tos_in, tos_out, generator, arg)`.
#[derive(Clone, Copy)]
pub struct Def {
    pub code: Code,
    pub flags: u32,
    pub tos_in: TosState,
    pub tos_out: TosState,
    pub generator: Gen,
    pub arg: i32,
}

pub const NO_ARG: i32 = i32::MIN;

fn def(code: Code, flags: u32, tos_in: TosState, tos_out: TosState, generator: Gen) -> Def {
    Def { code, flags, tos_in, tos_out, generator, arg: NO_ARG }
}

fn def_arg(code: Code, flags: u32, tos_in: TosState, tos_out: TosState, generator: Gen, arg: i32) -> Def {
    Def { code, flags, tos_in, tos_out, generator, arg }
}

fn iconst(code: Code, value: i32) -> Def {
    def_arg(code, 0, TosState::Vtos, TosState::Itos, Gen::Iconst, value)
}

fn load(code: Code, flags: u32, generator: Gen, arg: i32) -> Def {
    let tos = match generator {
        Gen::Iload | Gen::FastIload | Gen::FastIload2 | Gen::FastIcaload | Gen::NofastIload => TosState::Itos,
        Gen::Lload => TosState::Ltos,
        Gen::Fload => TosState::Ftos,
        Gen::Dload => TosState::Dtos,
        _ => TosState::Atos,
    };
    def_arg(code, flags, TosState::Vtos, tos, generator, arg)
}

fn store(code: Code, flags: u32, tos_in: TosState, generator: Gen, arg: i32) -> Def {
    def_arg(code, flags, tos_in, TosState::Vtos, generator, arg)
}

fn fast_field(code: Code, generator: Gen, ft: FieldType) -> Def {
    let (tos_in, tos_out) = match ft {
        FieldType::Atos => (TosState::Atos, TosState::Atos),
        FieldType::Itos => (TosState::Itos, TosState::Itos),
        FieldType::Ltos => (TosState::Ltos, TosState::Ltos),
        FieldType::Ftos => (TosState::Ftos, TosState::Ftos),
        FieldType::Dtos => (TosState::Dtos, TosState::Dtos),
        _ => (TosState::Vtos, TosState::Vtos),
    };
    def_arg(code, 0, tos_in, tos_out, generator, ft as i32)
}

const V: TosState = TosState::Vtos;
const I: TosState = TosState::Itos;
const L: TosState = TosState::Ltos;
const F: TosState = TosState::Ftos;
const D: TosState = TosState::Dtos;
const A: TosState = TosState::Atos;

const UB: u32 = UBCP;
const DI: u32 = DISP;
const CV: u32 = CLVM;
const SW: u32 = ISWD;

/// `TemplateTable::initialize`: the complete `def` table.
pub fn template_table() -> Vec<Def> {
    use Condition::*;
    let mut t: Vec<Def> = Vec::with_capacity(300);
    macro_rules! d {
        ($code:expr, $flags:expr, $tin:expr, $tout:expr, $generator:expr) => {
            t.push(def($code, $flags, $tin, $tout, $generator))
        };
    }
    macro_rules! da {
        ($code:expr, $flags:expr, $tin:expr, $tout:expr, $generator:expr, $arg:expr) => {
            t.push(def_arg($code, $flags, $tin, $tout, $generator, $arg))
        };
    }

    // ---- constants -------------------------------------------------------
    d!(Code::Nop, 0, V, V, Gen::Nop);
    d!(Code::AconstNull, 0, V, A, Gen::AconstNull);
    t.push(iconst(Code::IconstM1, -1));
    t.push(iconst(Code::Iconst0, 0));
    t.push(iconst(Code::Iconst1, 1));
    t.push(iconst(Code::Iconst2, 2));
    t.push(iconst(Code::Iconst3, 3));
    t.push(iconst(Code::Iconst4, 4));
    t.push(iconst(Code::Iconst5, 5));
    da!(Code::Lconst0, 0, V, L, Gen::Lconst, 0);
    da!(Code::Lconst1, 0, V, L, Gen::Lconst, 1);
    da!(Code::Fconst0, 0, V, F, Gen::Fconst, 0);
    da!(Code::Fconst1, 0, V, F, Gen::Fconst, 1);
    da!(Code::Fconst2, 0, V, F, Gen::Fconst, 2);
    da!(Code::Dconst0, 0, V, D, Gen::Dconst, 0);
    da!(Code::Dconst1, 0, V, D, Gen::Dconst, 1);
    d!(Code::Bipush, UB, V, I, Gen::Bipush);
    d!(Code::Sipush, UB, V, I, Gen::Sipush);
    d!(Code::Ldc, UB | CV, V, V, Gen::Ldc);
    d!(Code::LdcW, UB | CV, V, V, Gen::Ldc);
    d!(Code::Ldc2W, UB | CV, V, V, Gen::Ldc2W);

    // ---- loads -----------------------------------------------------------
    d!(Code::Iload, UB | CV, V, I, Gen::Iload);
    d!(Code::Lload, UB, V, L, Gen::Lload);
    d!(Code::Fload, UB, V, F, Gen::Fload);
    d!(Code::Dload, UB, V, D, Gen::Dload);
    d!(Code::Aload, UB | CV, V, A, Gen::Aload);
    for i in 0..4i32 {
        t.push(load(Code::Iload0 + i as u8, 0, Gen::Iload, i));
        t.push(load(Code::Lload0 + i as u8, 0, Gen::Lload, i));
        t.push(load(Code::Fload0 + i as u8, 0, Gen::Fload, i));
        t.push(load(Code::Dload0 + i as u8, 0, Gen::Dload, i));
        if i == 0 {
            d!(Code::Aload0, UB | CV, V, A, Gen::Aload0);
        } else {
            t.push(load(Code::Aload0 + i as u8, 0, Gen::Aload, i));
        }
    }

    // ---- array loads -----------------------------------------------------
    for (code, generator, tout) in [
        (Code::Iaload, Gen::Iaload, I), (Code::Laload, Gen::Laload, L),
        (Code::Faload, Gen::Faload, F), (Code::Daload, Gen::Daload, D),
        (Code::Aaload, Gen::Aaload, A), (Code::Baload, Gen::Baload, I),
        (Code::Caload, Gen::Caload, I), (Code::Saload, Gen::Saload, I),
    ] {
        d!(code, 0, I, tout, generator);
    }

    // ---- stores ----------------------------------------------------------
    d!(Code::Istore, UB | CV, I, V, Gen::Istore);
    d!(Code::Lstore, UB, L, V, Gen::Lstore);
    d!(Code::Fstore, UB, F, V, Gen::Fstore);
    d!(Code::Dstore, UB, D, V, Gen::Dstore);
    d!(Code::Astore, UB | CV, V, V, Gen::Astore);
    for i in 0..4i32 {
        t.push(store(Code::Istore0 + i as u8, 0, I, Gen::Istore, i));
        t.push(store(Code::Lstore0 + i as u8, 0, L, Gen::Lstore, i));
        t.push(store(Code::Fstore0 + i as u8, 0, F, Gen::Fstore, i));
        t.push(store(Code::Dstore0 + i as u8, 0, D, Gen::Dstore, i));
        t.push(store(Code::Astore0 + i as u8, 0, V, Gen::Astore, i));
    }

    // ---- array stores ----------------------------------------------------
    for (code, generator, tin, flags) in [
        (Code::Iastore, Gen::Iastore, I, 0), (Code::Lastore, Gen::Lastore, L, 0),
        (Code::Fastore, Gen::Fastore, F, 0), (Code::Dastore, Gen::Dastore, D, 0),
        (Code::Aastore, Gen::Aastore, V, CV), (Code::Bastore, Gen::Bastore, I, 0),
        (Code::Castore, Gen::Castore, I, 0), (Code::Sastore, Gen::Sastore, I, 0),
    ] {
        d!(code, flags, tin, V, generator);
    }

    // ---- stack -----------------------------------------------------------
    for (code, generator) in [
        (Code::Pop, Gen::Pop), (Code::Pop2, Gen::Pop2), (Code::Dup, Gen::Dup),
        (Code::DupX1, Gen::DupX1), (Code::DupX2, Gen::DupX2), (Code::Dup2, Gen::Dup2),
        (Code::Dup2X1, Gen::Dup2X1), (Code::Dup2X2, Gen::Dup2X2), (Code::Swap, Gen::Swap),
    ] {
        d!(code, 0, V, V, generator);
    }

    // ---- arithmetic ------------------------------------------------------
    for (code, generator, tin, tout, op) in [
        (Code::Iadd, Gen::Iop2, I, I, Op::Add), (Code::Ladd, Gen::Lop2, L, L, Op::Add),
        (Code::Fadd, Gen::Fop2, F, F, Op::Add), (Code::Dadd, Gen::Dop2, D, D, Op::Add),
        (Code::Isub, Gen::Iop2, I, I, Op::Sub), (Code::Lsub, Gen::Lop2, L, L, Op::Sub),
        (Code::Fsub, Gen::Fop2, F, F, Op::Sub), (Code::Dsub, Gen::Dop2, D, D, Op::Sub),
        (Code::Imul, Gen::Iop2, I, I, Op::Mul), (Code::Lmul, Gen::Lop2, L, L, Op::Mul),
        (Code::Fmul, Gen::Fop2, F, F, Op::Mul), (Code::Dmul, Gen::Dop2, D, D, Op::Mul),
        (Code::Iand, Gen::Iop2, I, I, Op::And), (Code::Land, Gen::Lop2, L, L, Op::And),
        (Code::Ior, Gen::Iop2, I, I, Op::Or), (Code::Lor, Gen::Lop2, L, L, Op::Or),
        (Code::Ixor, Gen::Iop2, I, I, Op::Xor), (Code::Lxor, Gen::Lop2, L, L, Op::Xor),
        (Code::Fdiv, Gen::Fop2, F, F, Op::Div), (Code::Ddiv, Gen::Dop2, D, D, Op::Div),
        (Code::Frem, Gen::Fop2, F, F, Op::Rem), (Code::Drem, Gen::Dop2, D, D, Op::Rem),
        (Code::Ishl, Gen::Iop2, I, I, Op::Shl), (Code::Lshl, Gen::Lop2, L, L, Op::Shl),
        (Code::Ishr, Gen::Iop2, I, I, Op::Shr), (Code::Lshr, Gen::Lop2, L, L, Op::Shr),
        (Code::Iushr, Gen::Iop2, I, I, Op::Ushr), (Code::Lushr, Gen::Lop2, L, L, Op::Ushr),
    ] {
        da!(code, 0, tin, tout, generator, op as i32);
    }
    for (code, generator, tin, tout) in [
        (Code::Idiv, Gen::Idiv, I, I), (Code::Ldiv, Gen::Ldiv, L, L),
        (Code::Irem, Gen::Irem, I, I), (Code::Lrem, Gen::Lrem, L, L),
        (Code::Ineg, Gen::Ineg, I, I), (Code::Lneg, Gen::Lneg, L, L),
        (Code::Fneg, Gen::Fneg, F, F), (Code::Dneg, Gen::Dneg, D, D),
    ] {
        d!(code, 0, tin, tout, generator);
    }
    d!(Code::Iinc, UB | CV, V, V, Gen::Iinc);

    // ---- conversions -----------------------------------------------------
    for (code, tin, tout) in [
        (Code::I2l, I, L), (Code::I2f, I, F), (Code::I2d, I, D),
        (Code::L2i, L, I), (Code::L2f, L, F), (Code::L2d, L, D),
        (Code::F2i, F, I), (Code::F2l, F, L), (Code::F2d, F, D),
        (Code::D2i, D, I), (Code::D2l, D, L), (Code::D2f, D, F),
        (Code::I2b, I, I), (Code::I2c, I, I), (Code::I2s, I, I),
    ] {
        d!(code, 0, tin, tout, Gen::Convert);
    }
    d!(Code::Lcmp, 0, L, I, Gen::Lcmp);
    d!(Code::Fcmpl, 0, F, I, Gen::FloatCmp);
    d!(Code::Fcmpg, 0, F, I, Gen::FloatCmp);
    d!(Code::Dcmpl, 0, D, I, Gen::DoubleCmp);
    d!(Code::Dcmpg, 0, D, I, Gen::DoubleCmp);

    // ---- branches --------------------------------------------------------
    for (code, cond) in [
        (Code::Ifeq, Equal), (Code::Ifne, NotEqual), (Code::Iflt, Less),
        (Code::Ifge, GreaterEqual), (Code::Ifgt, Greater), (Code::Ifle, LessEqual),
    ] {
        da!(code, UB | CV, I, V, Gen::If0Cmp, cond as i32);
    }
    for (code, cond) in [
        (Code::IfIcmpeq, Equal), (Code::IfIcmpne, NotEqual), (Code::IfIcmplt, Less),
        (Code::IfIcmpge, GreaterEqual), (Code::IfIcmpgt, Greater), (Code::IfIcmple, LessEqual),
    ] {
        da!(code, UB | CV, I, V, Gen::IfIcmp, cond as i32);
    }
    da!(Code::IfAcmpeq, UB | CV, A, V, Gen::IfAcmp, Equal as i32);
    da!(Code::IfAcmpne, UB | CV, A, V, Gen::IfAcmp, NotEqual as i32);
    d!(Code::Goto, UB | DI | CV, V, V, Gen::Goto);
    d!(Code::Jsr, UB | DI, V, V, Gen::Jsr);
    d!(Code::Ret, UB | DI, V, V, Gen::Ret);
    d!(Code::Tableswitch, UB | DI, I, V, Gen::Tableswitch);
    d!(Code::Lookupswitch, UB | DI, I, I, Gen::Lookupswitch);

    // ---- returns ---------------------------------------------------------
    d!(Code::Ireturn, DI | CV, I, I, Gen::Return);
    d!(Code::Lreturn, DI | CV, L, L, Gen::Return);
    d!(Code::Freturn, DI | CV, F, F, Gen::Return);
    d!(Code::Dreturn, DI | CV, D, D, Gen::Return);
    d!(Code::Areturn, DI | CV, A, A, Gen::Return);
    d!(Code::Return, DI | CV, V, V, Gen::Return);

    // ---- fields ----------------------------------------------------------
    d!(Code::Getstatic, UB | CV, V, V, Gen::Getstatic);
    d!(Code::Putstatic, UB | CV, V, V, Gen::Putstatic);
    d!(Code::Getfield, UB | CV, V, V, Gen::Getfield);
    d!(Code::Putfield, UB | CV, V, V, Gen::Putfield);

    // ---- invokes ---------------------------------------------------------
    d!(Code::Invokevirtual, UB | DI | CV, V, V, Gen::Invokevirtual);
    d!(Code::Invokespecial, UB | DI | CV, V, V, Gen::Invokespecial);
    d!(Code::Invokestatic, UB | DI | CV, V, V, Gen::Invokestatic);
    d!(Code::Invokeinterface, UB | DI | CV, V, V, Gen::Invokeinterface);
    d!(Code::Invokedynamic, UB | DI | CV, V, V, Gen::Invokedynamic);

    // ---- misc ------------------------------------------------------------
    d!(Code::New, UB | CV, V, A, Gen::New);
    d!(Code::Newarray, UB | CV, I, A, Gen::Newarray);
    d!(Code::Anewarray, UB | CV, I, A, Gen::Anewarray);
    d!(Code::Arraylength, 0, A, I, Gen::Arraylength);
    d!(Code::Athrow, DI, A, V, Gen::Athrow);
    d!(Code::Checkcast, UB | CV, A, A, Gen::Checkcast);
    d!(Code::Instanceof, UB | CV, A, I, Gen::Instanceof);
    d!(Code::Monitorenter, DI | CV, A, V, Gen::Monitorenter);
    d!(Code::Monitorexit, CV, A, V, Gen::Monitorexit);
    d!(Code::Wide, UB | DI, V, V, Gen::Wide);
    d!(Code::Multianewarray, UB | CV, V, A, Gen::Multianewarray);
    da!(Code::Ifnull, UB | CV, A, V, Gen::IfNullCmp, Equal as i32);
    da!(Code::Ifnonnull, UB | CV, A, V, Gen::IfNullCmp, NotEqual as i32);
    d!(Code::GotoW, UB | CV, V, V, Gen::GotoW);
    d!(Code::JsrW, UB, V, V, Gen::JsrW);

    // ---- wide Java spec bytecodes ---------------------------------------
    for (code, generator, tout) in [
        (Code::Iload, Gen::WideIload, I), (Code::Lload, Gen::WideLload, L),
        (Code::Fload, Gen::WideFload, F), (Code::Dload, Gen::WideDload, D),
        (Code::Aload, Gen::WideAload, A),
    ] {
        d!(code, UB | SW, V, tout, generator);
    }
    for (code, generator, tin) in [
        (Code::Istore, Gen::WideIstore, I), (Code::Lstore, Gen::WideLstore, L),
        (Code::Fstore, Gen::WideFstore, F), (Code::Dstore, Gen::WideDstore, D),
        (Code::Astore, Gen::WideAstore, V),
    ] {
        d!(code, UB | SW, tin, V, generator);
    }
    d!(Code::Iinc, UB | SW, V, V, Gen::WideIinc);
    d!(Code::Ret, UB | DI | SW, V, V, Gen::WideRet);
    d!(Code::Breakpoint, UB | DI | CV, V, V, Gen::Breakpoint);

    // ---- JVM bytecodes ---------------------------------------------------
    for (code, ft) in [
        (Code::FastAgetfield, FieldType::Atos), (Code::FastBgetfield, FieldType::Itos),
        (Code::FastCgetfield, FieldType::Itos), (Code::FastDgetfield, FieldType::Dtos),
        (Code::FastFgetfield, FieldType::Ftos), (Code::FastIgetfield, FieldType::Itos),
        (Code::FastLgetfield, FieldType::Ltos), (Code::FastSgetfield, FieldType::Itos),
    ] {
        t.push(fast_field(code, Gen::FastAccessField, ft));
    }
    for (code, ft) in [
        (Code::FastAputfield, FieldType::Atos), (Code::FastBputfield, FieldType::Itos),
        (Code::FastZputfield, FieldType::Itos), (Code::FastCputfield, FieldType::Itos),
        (Code::FastDputfield, FieldType::Dtos), (Code::FastFputfield, FieldType::Ftos),
        (Code::FastIputfield, FieldType::Itos), (Code::FastLputfield, FieldType::Ltos),
        (Code::FastSputfield, FieldType::Itos),
    ] {
        t.push(fast_field(code, Gen::FastStoreField, ft));
    }
    d!(Code::FastAload0, 0, V, A, Gen::Aload);
    da!(Code::FastIaccess0, UB, V, I, Gen::FastXaccess, I as i32);
    da!(Code::FastAaccess0, UB, V, A, Gen::FastXaccess, A as i32);
    da!(Code::FastFaccess0, UB, V, F, Gen::FastXaccess, F as i32);
    d!(Code::FastIload, UB, V, I, Gen::FastIload);
    d!(Code::FastIload2, UB, V, I, Gen::FastIload2);
    d!(Code::FastIcaload, UB, V, I, Gen::FastIcaload);
    d!(Code::FastInvokevfinal, UB | DI | CV, V, V, Gen::FastInvokevfinal);
    d!(Code::FastLinearswitch, UB | DI, I, V, Gen::FastLinearswitch);
    d!(Code::FastBinaryswitch, UB | DI, I, V, Gen::FastBinaryswitch);
    d!(Code::FastAldc, UB | CV, V, A, Gen::FastAldc);
    d!(Code::FastAldcW, UB | CV, V, A, Gen::FastAldc);
    d!(Code::ReturnRegisterFinalizer, DI | CV, V, V, Gen::ReturnRegisterFinalizer);
    d!(Code::Invokehandle, UB | DI | CV, V, V, Gen::Invokehandle);
    d!(Code::NofastGetfield, UB | CV, V, V, Gen::NofastGetfield);
    d!(Code::NofastPutfield, UB | CV, V, V, Gen::NofastPutfield);
    d!(Code::NofastAload0, CV, V, A, Gen::NofastAload0);
    d!(Code::NofastIload, UB | CV, V, I, Gen::NofastIload);
    d!(Code::ShouldNotReachHere, 0, V, V, Gen::ShouldNotReachHere);

    t
}

// ---------------------------------------------------------------------------
// TemplateInterpreterGenerator
// ---------------------------------------------------------------------------

/// `TemplateInterpreterGenerator`: installs the template table into the
/// interpreter's dispatch tables.  In C++ this class emits machine code;
/// here it emits `Template`s bound to symbolic codelet ids, one per
/// `(generator, arg)` pair, so `dispatch_entry` resolves a real handler id for
/// every defined bytecode.
pub struct TemplateInterpreterGenerator;

impl TemplateInterpreterGenerator {
    /// `generate`: builds `_normal_table`/`_safept_table`, the wide
    /// entries, return/deopt/invoke-return entries and throw entries.
    pub fn generate(interp: &mut TemplateInterpreter) {
        let defs = template_table();
        let mut codelet_ids: std::collections::HashMap<(Gen, i32), u16> =
            std::collections::HashMap::new();
        let mut next_id: u16 = 0x100;

        for def in defs {
            let template = Template::new(
                map_flags(def.flags),
                def.tos_in,
                def.tos_out,
            );
            debug_assert!(
                template.uses_bcp() == (def.flags & UBCP != 0)
                    && template.does_dispatch() == (def.flags & DISP != 0)
                    && template.calls_vm() == (def.flags & CV != 0)
                    && template.is_wide() == (def.flags & SW != 0),
                "template flags mismatch for {:?}",
                def.code
            );
            let key = (def.generator, def.arg);
            let id = *codelet_ids.entry(key).or_insert_with(|| {
                next_id += 1;
                next_id
            });
            let entry = EntryPoint::new_uniform(id as usize);
            interp.normal_table.set_entry(def.code as usize, entry);
            interp.safept_table.set_entry(def.code as usize, entry);
        }

        for i in 0..interp.wentry_point.len() {
            let code = i as u8;
            let generator = if code == Code::Iinc as u8 {
                Gen::WideIinc
            } else {
                Gen::Wide
            };
            interp.wentry_point[i] =
                EntryPoint::new_uniform(codelet_id(generator, NO_ARG).0 as usize);
        }

        for i in 0..interp.return_entry.len() {
            interp.return_entry[i] =
                EntryPoint::new_uniform(codelet_id(Gen::Return, NO_ARG).0 as usize);
        }
        interp.earlyret_entry =
            EntryPoint::new_uniform(codelet_id(Gen::Return, NO_ARG).0 as usize);
        for i in 0..interp.deopt_entry.len() {
            interp.deopt_entry[i] =
                EntryPoint::new_uniform(codelet_id(Gen::Unimplemented, NO_ARG).0 as usize);
        }
        interp.deopt_reexecute_return_entry =
            codelet_id(Gen::Unimplemented, NO_ARG).0 as usize;
        interp.safept_entry =
            EntryPoint::new_uniform(codelet_id(Gen::Nop, NO_ARG).0 as usize);

        for i in 0..interp.invoke_return_entry.len() {
            interp.invoke_return_entry[i] =
                EntryPoint::new_uniform(codelet_id(Gen::Return, NO_ARG).0 as usize);
            interp.invokeinterface_return_entry[i] =
                EntryPoint::new_uniform(codelet_id(Gen::Return, NO_ARG).0 as usize);
            interp.invokedynamic_return_entry[i] =
                EntryPoint::new_uniform(codelet_id(Gen::Return, NO_ARG).0 as usize);
        }

        for i in 0..interp.throw_entries.len() {
            interp.throw_entries[i] = 0x200 + i;
        }
    }
}

/// Deterministic codelet id for a `(generator, arg)` pair.
pub fn codelet_id(generator: Gen, arg: i32) -> CodeletId {
    let mut h: u32 = 0x9e37_79b9;
    h = h.wrapping_mul(31).wrapping_add(generator as u32 + 1);
    h = h.wrapping_mul(31).wrapping_add(arg as u32);
    CodeletId((h & 0x7fff) as u16)
}

fn map_flags(flags: u32) -> u32 {
    let mut out = 0u32;
    if flags & UBCP != 0 {
        out |= Template::USES_BCP;
    }
    if flags & DISP != 0 {
        out |= Template::DOES_DISPATCH;
    }
    if flags & CV != 0 {
        out |= Template::CALLS_VM;
    }
    if flags & SW != 0 {
        out |= Template::WIDE;
    }
    out
}

#[allow(dead_code)]
fn _assert_template_table_used() {
    let _ = TemplateTable::template_for(0);
}
