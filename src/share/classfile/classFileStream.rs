use std::io::{self, Error};

pub struct ClassFileStream<'a> {
    buffer: &'a [u8],
    offset: usize,
}

impl<'a> ClassFileStream<'a> {
    pub const JAVA_CLASSFILE_MAGIC: u32 = 0xCAFEBABE;
    pub const AUTHENTICATED_CLASS_MAGIC: u32 = 0x4C564332; // 'LVC2'
    pub const ENCRYPTED_POOL_MAGIC: u32 = 0x4C435033; // 'LCP3'

    pub fn new(buffer: &'a [u8]) -> Self {
        Self { buffer, offset: 0 }
    }

    pub fn at_end(&self) -> bool {
        self.offset >= self.buffer.len()
    }

    pub fn length(&self) -> usize {
        self.buffer.len()
    }

    pub fn current(&self) -> usize {
        self.offset
    }

    pub fn remaining(&self) -> usize {
        self.buffer.len().saturating_sub(self.offset)
    }

    pub fn get_u1(&mut self) -> io::Result<u8> {
        let b = self
            .buffer
            .get(self.offset)
            .copied()
            .ok_or_else(|| Error::other("Truncated class file stream"))?;
        self.offset += 1;
        Ok(b)
    }

    pub fn get_u2(&mut self) -> io::Result<u16> {
        let bytes = self
            .buffer
            .get(self.offset..self.offset + 2)
            .ok_or_else(|| Error::other("Truncated class file stream"))?;
        self.offset += 2;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn get_u4(&mut self) -> io::Result<u32> {
        let bytes = self
            .buffer
            .get(self.offset..self.offset + 4)
            .ok_or_else(|| Error::other("Truncated class file stream"))?;
        self.offset += 4;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn get_u8(&mut self) -> io::Result<u64> {
        let bytes = self
            .buffer
            .get(self.offset..self.offset + 8)
            .ok_or_else(|| Error::other("Truncated class file stream"))?;
        self.offset += 8;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(arr))
    }

    pub fn get_bytes(&mut self, len: usize) -> io::Result<&'a [u8]> {
        let bytes = self
            .buffer
            .get(self.offset..self.offset + len)
            .ok_or_else(|| Error::other("Truncated class file stream"))?;
        self.offset += len;
        Ok(bytes)
    }

    pub fn skip(&mut self, len: usize) -> io::Result<()> {
        if self.offset + len > self.buffer.len() {
            return Err(Error::other("Truncated class file stream"));
        }
        self.offset += len;
        Ok(())
    }
}
