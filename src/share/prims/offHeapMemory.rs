/*
 * Port of OpenJDK 25 HotSpot Direct Memory / Off-Heap Subsystem
 * (share/prims/unsafe.cpp, share/memory/allocation.hpp)
 *
 * Implements off-heap native memory allocation, address generation,
 * and primitive memory reads/writes in 100% safe Rust.
 * Essential for LWJGL vertex buffers, OpenGL direct pointers, and Netty byte buffers.
 */

use std::collections::HashMap;
use std::io::{self, Error};
use std::sync::{Mutex, OnceLock};

#[derive(Debug)]
struct MemoryBlock {
    data: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct OffHeapMemory {
    blocks: Mutex<HashMap<u64, MemoryBlock>>,
    next_address: Mutex<u64>,
}

static GLOBAL_OFF_HEAP: OnceLock<OffHeapMemory> = OnceLock::new();

impl OffHeapMemory {
    pub const BASE_ADDRESS: u64 = 0x0000_7FFF_0000_0000; // Realistic 64-bit off-heap virtual base
    pub const PAGE_SIZE: usize = 4096;
    pub const ADDRESS_SIZE: usize = 8;

    pub fn new() -> Self {
        Self {
            blocks: Mutex::new(HashMap::new()),
            next_address: Mutex::new(Self::BASE_ADDRESS),
        }
    }

    pub fn global() -> &'static Self {
        GLOBAL_OFF_HEAP.get_or_init(Self::new)
    }

    /// Allocates unmanaged off-heap memory block aligned to 8 bytes
    pub fn allocate_memory(&self, bytes: usize) -> u64 {
        let aligned_bytes = (bytes + 7) & !7;
        let mut next_guard = self.next_address.lock().unwrap();
        let addr = *next_guard;
        *next_guard += aligned_bytes as u64 + 64; // pad to avoid overlaps

        let block = MemoryBlock {
            data: vec![0u8; bytes],
        };
        let mut blocks = self.blocks.lock().unwrap();
        blocks.insert(addr, block);
        addr
    }

    /// Reallocates an existing memory block to a new size
    pub fn reallocate_memory(&self, addr: u64, new_size: usize) -> io::Result<u64> {
        if addr == 0 {
            return Ok(self.allocate_memory(new_size));
        }

        let mut blocks = self.blocks.lock().unwrap();
        if let Some(mut block) = blocks.remove(&addr) {
            block.data.resize(new_size, 0);
            blocks.insert(addr, block);
            Ok(addr)
        } else {
            Err(Error::other("Invalid memory address for reallocate"))
        }
    }

    /// Frees an allocated off-heap block
    pub fn free_memory(&self, addr: u64) {
        if addr != 0 {
            let mut blocks = self.blocks.lock().unwrap();
            blocks.remove(&addr);
        }
    }

    fn find_block_and_offset<'a>(
        blocks: &'a HashMap<u64, MemoryBlock>,
        addr: u64,
        size: usize,
    ) -> io::Result<(&'a [u8], usize)> {
        for (&base, block) in blocks.iter() {
            if addr >= base && addr + (size as u64) <= base + (block.data.len() as u64) {
                let offset = (addr - base) as usize;
                return Ok((&block.data, offset));
            }
        }
        Err(Error::other(format!(
            "Off-heap memory access violation at 0x{:016X} (size {})",
            addr, size
        )))
    }

    fn find_block_and_offset_mut<'a>(
        blocks: &'a mut HashMap<u64, MemoryBlock>,
        addr: u64,
        size: usize,
    ) -> io::Result<(&'a mut [u8], usize)> {
        for (&base, block) in blocks.iter_mut() {
            if addr >= base && addr + (size as u64) <= base + (block.data.len() as u64) {
                let offset = (addr - base) as usize;
                return Ok((&mut block.data, offset));
            }
        }
        Err(Error::other(format!(
            "Off-heap memory access violation at 0x{:016X} (size {})",
            addr, size
        )))
    }

    // ------------------------------------------------------------------------
    // Direct primitive reads
    // ------------------------------------------------------------------------

    pub fn get_byte(&self, addr: u64) -> io::Result<i8> {
        let blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset(&blocks, addr, 1)?;
        Ok(data[offset] as i8)
    }

    pub fn get_short(&self, addr: u64) -> io::Result<i16> {
        let blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset(&blocks, addr, 2)?;
        let bytes: [u8; 2] = data[offset..offset + 2].try_into().unwrap();
        Ok(i16::from_ne_bytes(bytes))
    }

    pub fn get_int(&self, addr: u64) -> io::Result<i32> {
        let blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset(&blocks, addr, 4)?;
        let bytes: [u8; 4] = data[offset..offset + 4].try_into().unwrap();
        Ok(i32::from_ne_bytes(bytes))
    }

    pub fn get_long(&self, addr: u64) -> io::Result<i64> {
        let blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset(&blocks, addr, 8)?;
        let bytes: [u8; 8] = data[offset..offset + 8].try_into().unwrap();
        Ok(i64::from_ne_bytes(bytes))
    }

    pub fn get_float(&self, addr: u64) -> io::Result<f32> {
        let bits = self.get_int(addr)?;
        Ok(f32::from_bits(bits as u32))
    }

    pub fn get_double(&self, addr: u64) -> io::Result<f64> {
        let bits = self.get_long(addr)?;
        Ok(f64::from_bits(bits as u64))
    }

    // ------------------------------------------------------------------------
    // Direct primitive writes
    // ------------------------------------------------------------------------

    pub fn put_byte(&self, addr: u64, val: i8) -> io::Result<()> {
        let mut blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset_mut(&mut blocks, addr, 1)?;
        data[offset] = val as u8;
        Ok(())
    }

    pub fn put_short(&self, addr: u64, val: i16) -> io::Result<()> {
        let mut blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset_mut(&mut blocks, addr, 2)?;
        data[offset..offset + 2].copy_from_slice(&val.to_ne_bytes());
        Ok(())
    }

    pub fn put_int(&self, addr: u64, val: i32) -> io::Result<()> {
        let mut blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset_mut(&mut blocks, addr, 4)?;
        data[offset..offset + 4].copy_from_slice(&val.to_ne_bytes());
        Ok(())
    }

    pub fn put_long(&self, addr: u64, val: i64) -> io::Result<()> {
        let mut blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset_mut(&mut blocks, addr, 8)?;
        data[offset..offset + 8].copy_from_slice(&val.to_ne_bytes());
        Ok(())
    }

    pub fn put_float(&self, addr: u64, val: f32) -> io::Result<()> {
        self.put_int(addr, val.to_bits() as i32)
    }

    pub fn put_double(&self, addr: u64, val: f64) -> io::Result<()> {
        self.put_long(addr, val.to_bits() as i64)
    }

    // ------------------------------------------------------------------------
    // Block operations
    // ------------------------------------------------------------------------

    pub fn copy_memory(&self, src: u64, dst: u64, bytes: usize) -> io::Result<()> {
        if bytes == 0 || src == dst {
            return Ok(());
        }
        let mut temp = vec![0u8; bytes];
        {
            let blocks = self.blocks.lock().unwrap();
            let (src_data, src_off) = Self::find_block_and_offset(&blocks, src, bytes)?;
            temp.copy_from_slice(&src_data[src_off..src_off + bytes]);
        }
        {
            let mut blocks = self.blocks.lock().unwrap();
            let (dst_data, dst_off) = Self::find_block_and_offset_mut(&mut blocks, dst, bytes)?;
            dst_data[dst_off..dst_off + bytes].copy_from_slice(&temp);
        }
        Ok(())
    }

    pub fn set_memory(&self, addr: u64, bytes: usize, value: u8) -> io::Result<()> {
        let mut blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset_mut(&mut blocks, addr, bytes)?;
        data[offset..offset + bytes].fill(value);
        Ok(())
    }
}
