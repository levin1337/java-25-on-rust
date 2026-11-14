/*
 * Port of OpenJDK 25 HotSpot PcDesc (share/code/pcDesc.hpp / pcDesc.cpp)
 *
 * PcDescs map a native PC offset in an nmethod to the corresponding
 * bytecode index (BCI), scope information, and deoptimization state.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PcDesc {
    pc_offset: i32,
    scope_decode_offset: i32,
    obj_decode_offset: i32,
    flags: i32,
}

impl PcDesc {
    pub const PCDESC_REEXECUTE: i32 = 1 << 0;
    pub const PCDESC_IS_METHOD_HANDLE_INVOKE: i32 = 1 << 1;
    pub const PCDESC_RETURN_OOP: i32 = 1 << 2;
    pub const PCDESC_RETHROW_EXCEPTION: i32 = 1 << 3;
    pub const PCDESC_HAS_EA_LOCAL_IN_SCOPE: i32 = 1 << 4;
    pub const PCDESC_ARG_ESCAPE: i32 = 1 << 5;

    pub const LOWER_OFFSET_LIMIT: i32 = -1;
    pub const UPPER_OFFSET_LIMIT: i32 = (u32::MAX >> 1) as i32;

    pub fn new(pc_offset: i32, scope_decode_offset: i32, flags: i32) -> Self {
        Self {
            pc_offset,
            scope_decode_offset,
            obj_decode_offset: 0,
            flags,
        }
    }

    pub fn with_obj(
        pc_offset: i32,
        scope_decode_offset: i32,
        obj_decode_offset: i32,
        flags: i32,
    ) -> Self {
        Self {
            pc_offset,
            scope_decode_offset,
            obj_decode_offset,
            flags,
        }
    }

    #[inline]
    pub fn pc_offset(&self) -> i32 {
        self.pc_offset
    }

    #[inline]
    pub fn set_pc_offset(&mut self, offset: i32) {
        self.pc_offset = offset;
    }

    #[inline]
    pub fn scope_decode_offset(&self) -> i32 {
        self.scope_decode_offset
    }

    #[inline]
    pub fn set_scope_decode_offset(&mut self, offset: i32) {
        self.scope_decode_offset = offset;
    }

    #[inline]
    pub fn obj_decode_offset(&self) -> i32 {
        self.obj_decode_offset
    }

    #[inline]
    pub fn set_obj_decode_offset(&mut self, offset: i32) {
        self.obj_decode_offset = offset;
    }

    #[inline]
    pub fn flags(&self) -> i32 {
        self.flags
    }

    #[inline]
    pub fn set_flags(&mut self, flags: i32) {
        self.flags = flags;
    }

    #[inline]
    fn set_flag(&mut self, mask: i32, val: bool) {
        if val {
            self.flags |= mask;
        } else {
            self.flags &= !mask;
        }
    }

    #[inline]
    pub fn should_reexecute(&self) -> bool {
        (self.flags & Self::PCDESC_REEXECUTE) != 0
    }

    #[inline]
    pub fn set_should_reexecute(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_REEXECUTE, val);
    }

    #[inline]
    pub fn is_method_handle_invoke(&self) -> bool {
        (self.flags & Self::PCDESC_IS_METHOD_HANDLE_INVOKE) != 0
    }

    #[inline]
    pub fn set_is_method_handle_invoke(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_IS_METHOD_HANDLE_INVOKE, val);
    }

    #[inline]
    pub fn is_return_oop(&self) -> bool {
        (self.flags & Self::PCDESC_RETURN_OOP) != 0
    }

    #[inline]
    pub fn set_return_oop(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_RETURN_OOP, val);
    }

    #[inline]
    pub fn should_rethrow_exception(&self) -> bool {
        (self.flags & Self::PCDESC_RETHROW_EXCEPTION) != 0
    }

    #[inline]
    pub fn rethrow_exception(&self) -> bool {
        self.should_rethrow_exception()
    }

    #[inline]
    pub fn set_rethrow_exception(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_RETHROW_EXCEPTION, val);
    }

    #[inline]
    pub fn has_ea_local_in_scope(&self) -> bool {
        (self.flags & Self::PCDESC_HAS_EA_LOCAL_IN_SCOPE) != 0
    }

    #[inline]
    pub fn set_has_ea_local_in_scope(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_HAS_EA_LOCAL_IN_SCOPE, val);
    }

    #[inline]
    pub fn arg_escape(&self) -> bool {
        (self.flags & Self::PCDESC_ARG_ESCAPE) != 0
    }

    #[inline]
    pub fn set_arg_escape(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_ARG_ESCAPE, val);
    }

    #[inline]
    pub fn real_pc(&self, code_begin: usize) -> usize {
        code_begin.saturating_add_signed(self.pc_offset as isize)
    }

    #[inline]
    pub fn is_same_info(&self, other: &PcDesc) -> bool {
        self.scope_decode_offset == other.scope_decode_offset
            && self.obj_decode_offset == other.obj_decode_offset
            && self.flags == other.flags
    }
}
