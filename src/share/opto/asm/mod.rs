/*
 * C2-owned x86-64 assembler layer (analog of share/opto/x86 in HotSpot).
 *
 * Kept separate from c1/asm: the optimizing compiler emits through its
 * own CodeBuffer/register model so the two tiers evolve independently.
 */

pub mod code_buffer;
pub mod register;
pub mod x64;
