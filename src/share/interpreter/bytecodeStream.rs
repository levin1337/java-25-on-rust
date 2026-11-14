/*
 * Port of OpenJDK 25 HotSpot BytecodeStream (share/interpreter/bytecodeStream.hpp, bytecodeStream.cpp)
 *
 * A BytecodeStream is used for fast iteration and decoding over the bytecodes of a method.
 */

use super::bytecodes;

pub struct BytecodeStream<'a> {
    bytes: &'a [u8],
    bci: usize,
    next_bci: usize,
    end_bci: usize,
    is_wide: bool,
}

impl<'a> BytecodeStream<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        let mut stream = Self {
            bytes,
            bci: 0,
            next_bci: 0,
            end_bci: bytes.len(),
            is_wide: false,
        };
        stream.step();
        stream
    }

    #[inline]
    pub fn bci(&self) -> usize {
        self.bci
    }

    #[inline]
    pub fn next_bci(&self) -> usize {
        self.next_bci
    }

    #[inline]
    pub fn end_bci(&self) -> usize {
        self.end_bci
    }

    pub fn set_interval(&mut self, beg_bci: usize, end_bci: usize) {
        let end = end_bci.min(self.bytes.len());
        let beg = beg_bci.min(end);
        self.bci = beg;
        self.next_bci = beg;
        self.end_bci = end;
        self.is_wide = false;
        self.step();
    }

    pub fn set_start(&mut self, beg_bci: usize) {
        self.set_interval(beg_bci, self.bytes.len());
    }

    #[inline]
    pub fn is_at_end(&self) -> bool {
        self.bci >= self.end_bci
    }

    #[inline]
    pub fn is_last_bytecode(&self) -> bool {
        self.next_bci >= self.end_bci
    }

    #[inline]
    pub fn is_wide(&self) -> bool {
        self.is_wide
    }

    #[inline]
    pub fn instruction_size(&self) -> usize {
        self.next_bci.saturating_sub(self.bci)
    }

    pub fn current_opcode(&self) -> Option<u8> {
        if self.bci < self.end_bci {
            self.bytes.get(self.bci).copied()
        } else {
            None
        }
    }

    #[inline]
    pub fn raw_code(&self) -> Option<u8> {
        self.current_opcode()
    }

    pub fn opcode_name(&self) -> Option<&'static str> {
        self.current_opcode().and_then(bytecodes::name)
    }

    pub fn next(&mut self) -> Option<u8> {
        if self.is_at_end() {
            return None;
        }
        let op = self.current_opcode();
        self.bci = self.next_bci;
        self.step();
        op
    }

    pub fn get_offset_s2(&self) -> i16 {
        if self.bci + 2 < self.bytes.len() {
            i16::from_be_bytes([self.bytes[self.bci + 1], self.bytes[self.bci + 2]])
        } else {
            0
        }
    }

    pub fn get_offset_s4(&self) -> i32 {
        if self.bci + 4 < self.bytes.len() {
            i32::from_be_bytes([
                self.bytes[self.bci + 1],
                self.bytes[self.bci + 2],
                self.bytes[self.bci + 3],
                self.bytes[self.bci + 4],
            ])
        } else {
            0
        }
    }

    pub fn dest(&self) -> usize {
        let offset = self.get_offset_s2() as isize;
        self.bci.wrapping_add_signed(offset)
    }

    pub fn dest_w(&self) -> usize {
        let offset = self.get_offset_s4() as isize;
        self.bci.wrapping_add_signed(offset)
    }

    pub fn get_index_u1(&self) -> u8 {
        self.bytes.get(self.bci + 1).copied().unwrap_or(0)
    }

    pub fn get_index_u2(&self) -> u16 {
        if self.bci + 2 < self.bytes.len() {
            u16::from_be_bytes([self.bytes[self.bci + 1], self.bytes[self.bci + 2]])
        } else {
            0
        }
    }

    fn step(&mut self) {
        if self.bci >= self.end_bci {
            self.next_bci = self.end_bci;
            self.is_wide = false;
            return;
        }

        let op = self.bytes[self.bci];
        if op == 0xc4 {
            // wide opcode
            self.is_wide = true;
            if self.bci + 1 < self.end_bci {
                let sub_op = self.bytes[self.bci + 1];
                let len = if sub_op == 0x84 { 6 } else { 4 };
                self.next_bci = (self.bci + len).min(self.end_bci);
            } else {
                self.next_bci = self.end_bci;
            }
            return;
        }

        self.is_wide = false;
        if op == 0xaa {
            // tableswitch: padded to 4-byte alignment
            let pad = (4 - ((self.bci + 1) % 4)) % 4;
            let base = self.bci + 1 + pad;
            if base + 12 <= self.bytes.len() {
                let low = i32::from_be_bytes([
                    self.bytes[base + 4],
                    self.bytes[base + 5],
                    self.bytes[base + 6],
                    self.bytes[base + 7],
                ]);
                let high = i32::from_be_bytes([
                    self.bytes[base + 8],
                    self.bytes[base + 9],
                    self.bytes[base + 10],
                    self.bytes[base + 11],
                ]);
                let count = (high as i64 - low as i64 + 1).max(0) as usize;
                self.next_bci = (base + 12 + count * 4).min(self.end_bci);
                return;
            }
        } else if op == 0xab {
            // lookupswitch: padded to 4-byte alignment
            let pad = (4 - ((self.bci + 1) % 4)) % 4;
            let base = self.bci + 1 + pad;
            if base + 8 <= self.bytes.len() {
                let npairs = i32::from_be_bytes([
                    self.bytes[base + 4],
                    self.bytes[base + 5],
                    self.bytes[base + 6],
                    self.bytes[base + 7],
                ])
                .max(0) as usize;
                self.next_bci = (base + 8 + npairs * 8).min(self.end_bci);
                return;
            }
        }

        let l = bytecodes::length_for(op);
        let len = if l > 0 { l as usize } else { 1 };
        self.next_bci = (self.bci + len).min(self.end_bci);
    }
}
