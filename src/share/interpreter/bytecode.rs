/*
 * Port of OpenJDK 25 HotSpot Bytecode abstractions (share/interpreter/bytecode.hpp, bytecode.cpp)
 *
 * Lightweight typed wrappers around bytecode pointers/slices for structured inspection.
 */

use super::bytecodes::{self, Code};

pub struct Bytecode<'a> {
    code: &'a [u8],
    bci: usize,
}

impl<'a> Bytecode<'a> {
    pub fn new(code: &'a [u8], bci: usize) -> Option<Self> {
        if bci < code.len() {
            Some(Self { code, bci })
        } else {
            None
        }
    }

    pub fn bci(&self) -> usize {
        self.bci
    }

    pub fn opcode(&self) -> u8 {
        self.code[self.bci]
    }

    pub fn code(&self) -> Option<Code> {
        bytecodes::code_from_u8(self.opcode())
    }

    pub fn instruction_size(&self) -> usize {
        bytecodes::length_at(self.code, self.bci)
    }

    pub fn get_u1(&self, offset: usize) -> Option<u8> {
        self.code.get(self.bci + offset).copied()
    }

    pub fn get_u2(&self, offset: usize) -> Option<u16> {
        let start = self.bci + offset;
        let bytes: [u8; 2] = self.code.get(start..start + 2)?.try_into().ok()?;
        Some(u16::from_be_bytes(bytes))
    }

    pub fn get_u4(&self, offset: usize) -> Option<u32> {
        let start = self.bci + offset;
        let bytes: [u8; 4] = self.code.get(start..start + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(bytes))
    }

    pub fn get_s2(&self, offset: usize) -> Option<i16> {
        self.get_u2(offset).map(|v| v as i16)
    }

    pub fn get_s4(&self, offset: usize) -> Option<i32> {
        self.get_u4(offset).map(|v| v as i32)
    }
}

pub struct BytecodeTableswitch<'a> {
    bytecode: Bytecode<'a>,
    aligned_offset: usize,
}

impl<'a> BytecodeTableswitch<'a> {
    pub fn new(code: &'a [u8], bci: usize) -> Option<Self> {
        let bc = Bytecode::new(code, bci)?;
        if bc.opcode() != 0xaa {
            return None;
        }
        let pad = (4 - ((bci + 1) % 4)) % 4;
        let aligned_offset = 1 + pad;
        Some(Self {
            bytecode: bc,
            aligned_offset,
        })
    }

    pub fn default_offset(&self) -> Option<i32> {
        self.bytecode.get_s4(self.aligned_offset)
    }

    pub fn low_key(&self) -> Option<i32> {
        self.bytecode.get_s4(self.aligned_offset + 4)
    }

    pub fn high_key(&self) -> Option<i32> {
        self.bytecode.get_s4(self.aligned_offset + 8)
    }

    pub fn length(&self) -> Option<usize> {
        let low = self.low_key()?;
        let high = self.high_key()?;
        if high >= low {
            Some((high - low + 1) as usize)
        } else {
            None
        }
    }

    pub fn dest_offset_at(&self, index: usize) -> Option<i32> {
        let len = self.length()?;
        if index < len {
            self.bytecode.get_s4(self.aligned_offset + 12 + index * 4)
        } else {
            None
        }
    }
}

pub struct BytecodeLookupswitch<'a> {
    bytecode: Bytecode<'a>,
    aligned_offset: usize,
}

impl<'a> BytecodeLookupswitch<'a> {
    pub fn new(code: &'a [u8], bci: usize) -> Option<Self> {
        let bc = Bytecode::new(code, bci)?;
        if bc.opcode() != 0xab {
            return None;
        }
        let pad = (4 - ((bci + 1) % 4)) % 4;
        let aligned_offset = 1 + pad;
        Some(Self {
            bytecode: bc,
            aligned_offset,
        })
    }

    pub fn default_offset(&self) -> Option<i32> {
        self.bytecode.get_s4(self.aligned_offset)
    }

    pub fn number_of_pairs(&self) -> Option<usize> {
        self.bytecode
            .get_s4(self.aligned_offset + 4)
            .map(|n| n.max(0) as usize)
    }

    pub fn match_at(&self, index: usize) -> Option<i32> {
        let count = self.number_of_pairs()?;
        if index < count {
            self.bytecode.get_s4(self.aligned_offset + 8 + index * 8)
        } else {
            None
        }
    }

    pub fn offset_at(&self, index: usize) -> Option<i32> {
        let count = self.number_of_pairs()?;
        if index < count {
            self.bytecode
                .get_s4(self.aligned_offset + 8 + index * 8 + 4)
        } else {
            None
        }
    }
}

pub struct BytecodeInvoke<'a> {
    bytecode: Bytecode<'a>,
}

impl<'a> BytecodeInvoke<'a> {
    pub fn new(code: &'a [u8], bci: usize) -> Option<Self> {
        let bc = Bytecode::new(code, bci)?;
        let op = bc.opcode();
        if matches!(op, 0xb6..=0xba) {
            Some(Self { bytecode: bc })
        } else {
            None
        }
    }

    pub fn is_invokevirtual(&self) -> bool {
        self.bytecode.opcode() == 0xb6
    }

    pub fn is_invokespecial(&self) -> bool {
        self.bytecode.opcode() == 0xb7
    }

    pub fn is_invokestatic(&self) -> bool {
        self.bytecode.opcode() == 0xb8
    }

    pub fn is_invokeinterface(&self) -> bool {
        self.bytecode.opcode() == 0xb9
    }

    pub fn is_invokedynamic(&self) -> bool {
        self.bytecode.opcode() == 0xba
    }

    pub fn constant_pool_index(&self) -> Option<u16> {
        self.bytecode.get_u2(1)
    }
}
