/*
 * Port of OpenJDK 25 HotSpot VtableStubs
 * (share/code/vtableStubs.hpp / vtableStubs.cpp)
 *
 * Implements code stub generator and cache for polymorphic vtable and itable dispatchers.
 */

use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VtableStub {
    is_vtable_stub: bool,
    index: i32,
    code_size: usize,
    entry_point: u64,
}

impl VtableStub {
    pub fn new(is_vtable_stub: bool, index: i32, code_size: usize, entry_point: u64) -> Self {
        Self {
            is_vtable_stub,
            index,
            code_size,
            entry_point,
        }
    }

    #[inline]
    pub fn is_vtable_stub(&self) -> bool {
        self.is_vtable_stub
    }

    #[inline]
    pub fn is_itable_stub(&self) -> bool {
        !self.is_vtable_stub
    }

    #[inline]
    pub fn index(&self) -> i32 {
        self.index
    }

    #[inline]
    pub fn code_size(&self) -> usize {
        self.code_size
    }

    #[inline]
    pub fn entry_point(&self) -> u64 {
        self.entry_point
    }
}

pub struct VtableStubs {
    stubs: RwLock<HashMap<(bool, i32), VtableStub>>,
    next_stub_entry: RwLock<u64>,
}

impl VtableStubs {
    pub const BASE_STUB_ADDR: u64 = 0x0000_1000_0000;

    pub fn new() -> Self {
        Self {
            stubs: RwLock::new(HashMap::new()),
            next_stub_entry: RwLock::new(Self::BASE_STUB_ADDR),
        }
    }

    pub fn lookup(&self, is_vtable_stub: bool, index: i32) -> Option<VtableStub> {
        let guard = self.stubs.read().unwrap();
        guard.get(&(is_vtable_stub, index)).cloned()
    }

    pub fn create_stub(&self, is_vtable_stub: bool, index: i32) -> VtableStub {
        // Fast path check
        if let Some(stub) = self.lookup(is_vtable_stub, index) {
            return stub;
        }

        let mut stubs_guard = self.stubs.write().unwrap();
        if let Some(stub) = stubs_guard.get(&(is_vtable_stub, index)) {
            return stub.clone();
        }

        let mut entry_guard = self.next_stub_entry.write().unwrap();
        let entry_point = *entry_guard;
        let code_size = if is_vtable_stub { 24 } else { 48 };
        *entry_guard += code_size as u64 + 16;

        let stub = VtableStub::new(is_vtable_stub, index, code_size, entry_point);
        stubs_guard.insert((is_vtable_stub, index), stub.clone());
        stub
    }

    pub fn create_vtable_stub(&self, index: i32) -> VtableStub {
        self.create_stub(true, index)
    }

    pub fn create_itable_stub(&self, index: i32) -> VtableStub {
        self.create_stub(false, index)
    }

    pub fn stub_count(&self) -> usize {
        self.stubs.read().unwrap().len()
    }
}

impl Default for VtableStubs {
    fn default() -> Self {
        Self::new()
    }
}
