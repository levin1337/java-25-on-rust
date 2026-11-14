/*
 * Port of OpenJDK 25 HotSpot TemplateTable
 * (share/interpreter/templateTable.hpp / templateTable.cpp)
 *
 * Implements Top-of-Stack (TosState) caching models, template flags,
 * and bytecode template descriptor definitions for the HotSpot interpreter.
 */

use super::bytecodes::Code;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TosState {
    Btos, // byte / boolean
    Ztos, // boolean
    Ctos, // char
    Stos, // short
    Itos, // int
    Ltos, // long
    Ftos, // float
    Dtos, // double
    Atos, // object / array reference
    Vtos, // void (empty top-of-stack)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    flags: u32,
    tos_in: TosState,
    tos_out: TosState,
}

impl Template {
    pub const USES_BCP: u32 = 1 << 0;
    pub const DOES_DISPATCH: u32 = 1 << 1;
    pub const CALLS_VM: u32 = 1 << 2;
    pub const WIDE: u32 = 1 << 3;

    pub const fn new(flags: u32, tos_in: TosState, tos_out: TosState) -> Self {
        Self {
            flags,
            tos_in,
            tos_out,
        }
    }

    #[inline]
    pub fn uses_bcp(&self) -> bool {
        (self.flags & Self::USES_BCP) != 0
    }

    #[inline]
    pub fn does_dispatch(&self) -> bool {
        (self.flags & Self::DOES_DISPATCH) != 0
    }

    #[inline]
    pub fn calls_vm(&self) -> bool {
        (self.flags & Self::CALLS_VM) != 0
    }

    #[inline]
    pub fn is_wide(&self) -> bool {
        (self.flags & Self::WIDE) != 0
    }

    #[inline]
    pub fn tos_in(&self) -> TosState {
        self.tos_in
    }

    #[inline]
    pub fn tos_out(&self) -> TosState {
        self.tos_out
    }
}

pub struct TemplateTable;

impl TemplateTable {
    pub fn template_for(opcode: u8) -> Template {
        match opcode {
            // Nop
            0x00 => Template::new(0, TosState::Vtos, TosState::Vtos),
            // aconst_null
            0x01 => Template::new(0, TosState::Vtos, TosState::Atos),
            // iconst_m1 .. iconst_5
            0x02..=0x08 => Template::new(0, TosState::Vtos, TosState::Itos),
            // lconst_0, lconst_1
            0x09..=0x0a => Template::new(0, TosState::Vtos, TosState::Ltos),
            // fconst_0 .. fconst_2
            0x0b..=0x0d => Template::new(0, TosState::Vtos, TosState::Ftos),
            // dconst_0, dconst_1
            0x0e..=0x0f => Template::new(0, TosState::Vtos, TosState::Dtos),
            // bipush, sipush
            0x10..=0x11 => Template::new(Template::USES_BCP, TosState::Vtos, TosState::Itos),
            // ldc, ldc_w, ldc2_w
            0x12..=0x14 => Template::new(
                Template::USES_BCP | Template::CALLS_VM,
                TosState::Vtos,
                TosState::Vtos,
            ),
            // iload, iload_0 .. iload_3
            0x15 | 0x1a..=0x1d => Template::new(0, TosState::Vtos, TosState::Itos),
            // lload, lload_0 .. lload_3
            0x16 | 0x1e..=0x21 => Template::new(0, TosState::Vtos, TosState::Ltos),
            // fload, fload_0 .. fload_3
            0x17 | 0x22..=0x25 => Template::new(0, TosState::Vtos, TosState::Ftos),
            // dload, dload_0 .. dload_3
            0x18 | 0x26..=0x29 => Template::new(0, TosState::Vtos, TosState::Dtos),
            // aload, aload_0 .. aload_3
            0x19 | 0x2a..=0x2d => Template::new(0, TosState::Vtos, TosState::Atos),
            // iaload .. saload
            0x2e => Template::new(0, TosState::Itos, TosState::Itos), // iaload
            0x2f => Template::new(0, TosState::Itos, TosState::Ltos), // laload
            0x30 => Template::new(0, TosState::Itos, TosState::Ftos), // faload
            0x31 => Template::new(0, TosState::Itos, TosState::Dtos), // daload
            0x32 => Template::new(0, TosState::Itos, TosState::Atos), // aaload
            0x33 => Template::new(0, TosState::Itos, TosState::Btos), // baload
            0x34 => Template::new(0, TosState::Itos, TosState::Ctos), // caload
            0x35 => Template::new(0, TosState::Itos, TosState::Stos), // saload
            // istore, lstore, fstore, dstore, astore
            0x36 | 0x3b..=0x3e => Template::new(0, TosState::Itos, TosState::Vtos),
            0x37 | 0x3f..=0x42 => Template::new(0, TosState::Ltos, TosState::Vtos),
            0x38 | 0x43..=0x46 => Template::new(0, TosState::Ftos, TosState::Vtos),
            0x39 | 0x47..=0x4a => Template::new(0, TosState::Dtos, TosState::Vtos),
            0x3a | 0x4b..=0x4e => Template::new(0, TosState::Atos, TosState::Vtos),
            // iadd, isub, imul, idiv, irem
            0x60 | 0x64 | 0x68 | 0x6c | 0x70 => Template::new(0, TosState::Itos, TosState::Itos),
            // ladd, lsub, lmul, ldiv, lrem
            0x61 | 0x65 | 0x69 | 0x6d | 0x71 => Template::new(0, TosState::Ltos, TosState::Ltos),
            // fadd, fsub, fmul, fdiv, frem
            0x62 | 0x66 | 0x6a | 0x6e | 0x72 => Template::new(0, TosState::Ftos, TosState::Ftos),
            // dadd, dsub, dmul, ddiv, drem
            0x63 | 0x67 | 0x6b | 0x6f | 0x73 => Template::new(0, TosState::Dtos, TosState::Dtos),
            // invokevirtual, invokespecial, invokestatic, invokeinterface, invokedynamic
            0xb6..=0xba => Template::new(
                Template::USES_BCP | Template::CALLS_VM | Template::DOES_DISPATCH,
                TosState::Vtos,
                TosState::Vtos,
            ),
            // ireturn, lreturn, freturn, dreturn, areturn, return
            0xac => Template::new(Template::DOES_DISPATCH, TosState::Itos, TosState::Itos),
            0xad => Template::new(Template::DOES_DISPATCH, TosState::Ltos, TosState::Ltos),
            0xae => Template::new(Template::DOES_DISPATCH, TosState::Ftos, TosState::Ftos),
            0xaf => Template::new(Template::DOES_DISPATCH, TosState::Dtos, TosState::Dtos),
            0xb0 => Template::new(Template::DOES_DISPATCH, TosState::Atos, TosState::Atos),
            0xb1 => Template::new(Template::DOES_DISPATCH, TosState::Vtos, TosState::Vtos),
            // default fallback
            _ => Template::new(0, TosState::Vtos, TosState::Vtos),
        }
    }

    #[inline]
    pub fn template_for_code(code: Code) -> Template {
        TemplateTable::template_for(code as u8)
    }
}
