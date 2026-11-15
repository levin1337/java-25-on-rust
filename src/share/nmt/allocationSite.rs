/*
 * Port of OpenJDK 25 HotSpot AllocationSite (share/nmt/allocationSite.hpp)
 *
 * An allocation site represents a code path that makes a memory
 * allocation: a NativeCallStack plus the MemTag it allocates under.
 */

use super::memTag::MemTag;
use super::nativeCallStack::NativeCallStack;

#[derive(Clone, Debug)]
pub struct AllocationSite {
    call_stack: NativeCallStack,
    mem_tag: MemTag,
}

impl AllocationSite {
    pub fn new(stack: NativeCallStack, mem_tag: MemTag) -> Self {
        Self { call_stack: stack, mem_tag }
    }

    pub fn equals_stack(&self, stack: &NativeCallStack) -> bool {
        self.call_stack.equals(stack)
    }

    pub fn equals(&self, other: &Self) -> bool {
        other.equals_stack(&self.call_stack)
    }

    pub fn call_stack(&self) -> &NativeCallStack {
        &self.call_stack
    }

    pub fn mem_tag(&self) -> MemTag {
        self.mem_tag
    }
}
