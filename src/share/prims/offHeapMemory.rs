/*
 * Port of OpenJDK 25 HotSpot Direct Memory / Off-Heap Subsystem
 * (share/prims/unsafe.cpp, share/memory/allocation.hpp)
 *
 * Implements off-heap native memory allocation, address generation,
 * and bounds-checked primitive memory reads/writes.
 * Essential for LWJGL vertex buffers, OpenGL direct pointers, and Netty byte buffers.
 */

use std::collections::BTreeMap;
use std::io::{self, Error};
use std::sync::{Mutex, OnceLock};

#[derive(Debug)]
struct MemoryBlock {
    storage: Vec<u128>,
    len: usize,
}

impl MemoryBlock {
    fn new(len: usize) -> Self {
        Self {
            storage: vec![0; len.div_ceil(size_of::<u128>()).max(1)],
            len,
        }
    }

    fn address(&mut self) -> u64 {
        self.storage.as_mut_ptr() as u64
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.storage.as_ptr().cast(), self.len) }
    }

    fn bytesMut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.storage.as_mut_ptr().cast(), self.len) }
    }
}

#[derive(Debug, Default)]
pub struct OffHeapMemory {
    blocks: Mutex<BTreeMap<u64, MemoryBlock>>,
}

static GLOBAL_OFF_HEAP: OnceLock<OffHeapMemory> = OnceLock::new();

impl OffHeapMemory {
    pub const PAGE_SIZE: usize = 4096;
    pub const ADDRESS_SIZE: usize = 8;

    pub fn new() -> Self {
        Self {
            blocks: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn global() -> &'static Self {
        GLOBAL_OFF_HEAP.get_or_init(Self::new)
    }

    /// Allocates unmanaged off-heap memory block aligned to 8 bytes
    pub fn allocate_memory(&self, bytes: usize) -> u64 {
        let mut block = MemoryBlock::new(bytes);
        let address = block.address();
        self.blocks.lock().unwrap().insert(address, block);
        address
    }

    /// Reallocates an existing memory block to a new size
    pub fn reallocate_memory(&self, addr: u64, new_size: usize) -> io::Result<u64> {
        if addr == 0 {
            return Ok(self.allocate_memory(new_size));
        }

        let mut blocks = self.blocks.lock().unwrap();
        let previous = blocks.remove(&addr).ok_or_else(|| Error::other("Invalid memory address for reallocate"))?;
        let mut block = MemoryBlock::new(new_size);
        let length = previous.len.min(new_size);
        block.bytesMut()[..length].copy_from_slice(&previous.bytes()[..length]);
        let address = block.address();
        blocks.insert(address, block);
        Ok(address)
    }

    /// Frees an allocated off-heap block
    pub fn free_memory(&self, addr: u64) {
        if addr != 0 {
            let mut blocks = self.blocks.lock().unwrap();
            blocks.remove(&addr);
        }
    }

    fn find_block_and_offset<'a>(
        blocks: &'a BTreeMap<u64, MemoryBlock>,
        addr: u64,
        size: usize,
    ) -> io::Result<(&'a [u8], usize)> {
        if let Some((&base, block)) = blocks.range(..=addr).next_back() {
            if addr - base <= block.len as u64 && size <= block.len - (addr - base) as usize {
                let offset = (addr - base) as usize;
                return Ok((block.bytes(), offset));
            }
        }
        Err(Error::other(format!(
            "Off-heap memory access violation at 0x{:016X} (size {})",
            addr, size
        )))
    }

    fn find_block_and_offset_mut<'a>(
        blocks: &'a mut BTreeMap<u64, MemoryBlock>,
        addr: u64,
        size: usize,
    ) -> io::Result<(&'a mut [u8], usize)> {
        let base = blocks.range(..=addr).next_back().map(|(&b, _)| b);
        if let Some(base) = base {
            let block = blocks.get_mut(&base).unwrap();
            if addr - base <= block.len as u64 && size <= block.len - (addr - base) as usize {
                let offset = (addr - base) as usize;
                return Ok((block.bytesMut(), offset));
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

    pub fn read_bytes(&self, addr: u64, bytes: usize) -> io::Result<Vec<u8>> {
        let blocks = self.blocks.lock().unwrap();
        if let Ok((data, offset)) = Self::find_block_and_offset(&blocks, addr, bytes) {
            return Ok(data[offset..offset + bytes].to_vec());
        }
        if Self::ownsAddress(&blocks, addr) {
            return Err(Error::other("Off-heap read exceeds allocation"));
        }
        drop(blocks);
        let mut data = vec![0; bytes];
        nativeMemory(addr, &mut data, None)?;
        Ok(data)
    }

    pub fn write_bytes(&self, addr: u64, bytes: &[u8]) -> io::Result<()> {
        let mut blocks = self.blocks.lock().unwrap();
        if let Ok((data, offset)) = Self::find_block_and_offset_mut(&mut blocks, addr, bytes.len()) {
            data[offset..offset + bytes.len()].copy_from_slice(bytes);
            return Ok(());
        }
        if Self::ownsAddress(&blocks, addr) {
            return Err(Error::other("Off-heap write exceeds allocation"));
        }
        drop(blocks);
        nativeMemory(addr, &mut [], Some(bytes))
    }

    fn ownsAddress(blocks: &BTreeMap<u64, MemoryBlock>, addr: u64) -> bool {
        blocks.range(..=addr).next_back().is_some_and(|(&base, block)| addr - base <= block.len as u64)
    }

    pub fn set_memory(&self, addr: u64, bytes: usize, value: u8) -> io::Result<()> {
        let mut blocks = self.blocks.lock().unwrap();
        let (data, offset) = Self::find_block_and_offset_mut(&mut blocks, addr, bytes)?;
        data[offset..offset + bytes].fill(value);
        Ok(())
    }
}

#[cfg(windows)]
fn nativeMemory(address: u64, destination: &mut [u8], source: Option<&[u8]>) -> io::Result<()> {
    use core::ffi::c_void;
    unsafe extern "system" {
        fn ReadProcessMemory(process: isize, base: *const c_void, buffer: *mut c_void, size: usize, count: *mut usize) -> i32;
        fn WriteProcessMemory(process: isize, base: *mut c_void, buffer: *const c_void, size: usize, count: *mut usize) -> i32;
    }
    let mut count = 0;
    let (result, expected) = if let Some(source) = source {
        let size = source.len();
        (unsafe { WriteProcessMemory(-1, address as *mut c_void, source.as_ptr().cast(), size, &mut count) }, size)
    } else {
        let size = destination.len();
        (unsafe { ReadProcessMemory(-1, address as *const c_void, destination.as_mut_ptr().cast(), size, &mut count) }, size)
    };
    if result == 0 {
        return Err(Error::last_os_error());
    }
    if count != expected {
        return Err(Error::other("Incomplete native memory operation"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn nativeMemory(_: u64, _: &mut [u8], _: Option<&[u8]>) -> io::Result<()> {
    Err(Error::other("Address is outside the JVM allocations"))
}
