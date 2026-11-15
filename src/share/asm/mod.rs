/*
 * OpenJDK 25 HotSpot asm/ port
 *
 * share-level assembler framework: Assembler base + x86_64 encodings,
 * register model, MacroAssembler high-level ops, CodeBuffer/CodeBlob,
 * and the relocation table (relocInfo).
 */

pub mod assembler;
pub mod code_buffer;
pub mod macro_assembler;
pub mod register;
pub mod reloc;
pub mod x86_64;

pub use assembler::{Assembler, AssemblerError, LabelId, LabelTable, PatchKind, PatchSite, RelocSpec};
pub use code_buffer::{
    CodeBlob, CodeBlobType, CodeBuffer, CodeBufferError, CodeSection, ExceptionHandlerEntry,
    ExceptionHandlerTable, LocBase, MonitorValue, ObjectValue, OopMap, OopMapSet, ScopeConst,
    ScopeDesc, ScopeValue, ScopeValueKind, SectionId, CODE_ENTRY_ALIGNMENT, END_SLOP,
};
pub use macro_assembler::{CallTarget, CardBarrierParams, MacroAssembler};
pub use register::{
    ConditionCode, FloatRegister, FloatRegisterLength, NOREG, OperandSize, RegSet, Register,
    Scale,
};
pub use reloc::{RelocEntry, RelocError, RelocTable, RelocType, RelocValue, RelocWidth};
pub use x86_64::{win64, Address, RmOperand, ShiftCount};
