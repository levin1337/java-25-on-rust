/*
 * Port of OpenJDK 25 HotSpot CodeBuffer
 * (share/code/codeBuffer.hpp / codeBuffer.cpp)
 *
 * Manages executable memory for JIT-compiled code with proper alignment,
 * protection, and lifetime management.
 */

use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(windows)]
mod win32 {
    use std::os::raw::c_void;
    pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
    pub const PAGE_EXECUTE_READ: u32 = 0x20;
    pub const PAGE_READWRITE: u32 = 0x04;
    pub const MEM_COMMIT: u32 = 0x1000;
    pub const MEM_RESERVE: u32 = 0x2000;
    pub const MEM_RELEASE: u32 = 0x8000;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn VirtualAlloc(lp_address: *mut c_void, dw_size: usize, fl_allocation_type: u32, fl_protect: u32) -> *mut c_void;
        pub fn VirtualFree(lp_address: *mut c_void, dw_size: usize, dw_free_type: u32) -> i32;
        pub fn VirtualProtect(
            lp_address: *mut c_void,
            dw_size: usize,
            fl_new_protect: u32,
            lpfl_old_protect: *mut u32,
        ) -> i32;
    }
}

/// Default code buffer size (256 KB)
pub const DEFAULT_BUFFER_SIZE: usize = 256 * 1024;

/// Minimum buffer size (4 KB - one page)
pub const MIN_BUFFER_SIZE: usize = 4096;

/// Maximum buffer size (64 MB)
pub const MAX_BUFFER_SIZE: usize = 64 * 1024 * 1024;

/// Code buffer alignment (64 bytes for cache line alignment)
pub const CODE_BUFFER_ALIGNMENT: usize = 64;

/// Instruction alignment (16 bytes for loop alignment)
pub const INSTRUCTION_ALIGNMENT: usize = 16;

/// Represents a region of executable memory for JIT code
pub struct CodeBuffer {
    /// Base address of the allocated memory region
    base: NonNull<u8>,
    /// Total size of the allocated region
    size: usize,
    /// Current write position (offset from base)
    position: AtomicUsize,
    /// High water mark (maximum position ever reached)
    high_water_mark: AtomicUsize,
    /// Whether the buffer is executable (RWX) or just writable (RW)
    executable: bool,
    /// Optional name for debugging
    name: Option<String>,
}

impl CodeBuffer {
    /// Creates a new code buffer with the given size
    pub fn new(size: usize) -> Result<Self, CodeBufferError> {
        Self::new_named(size, None)
    }

    /// Creates a new named code buffer
    pub fn new_named(size: usize, name: Option<String>) -> Result<Self, CodeBufferError> {
        if size < MIN_BUFFER_SIZE || size > MAX_BUFFER_SIZE {
            return Err(CodeBufferError::InvalidSize(size));
        }

        // Align size to page boundary
        let page_size = page_size();
        let aligned_size = (size + page_size - 1) & !(page_size - 1);

        let ptr = allocate_executable(aligned_size)?;
        let base = NonNull::new(ptr).ok_or(CodeBufferError::AllocationFailed(aligned_size))?;

        Ok(Self {
            base,
            size: aligned_size,
            position: AtomicUsize::new(0),
            high_water_mark: AtomicUsize::new(0),
            executable: true,
            name,
        })
    }

    /// Returns the base address of the buffer
    #[inline]
    pub fn base(&self) -> *const u8 {
        self.base.as_ptr()
    }

    /// Returns the base address as a mutable pointer
    #[inline]
    pub fn base_mut(&self) -> *mut u8 {
        self.base.as_ptr()
    }

    /// Returns the total size of the buffer
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns the current position (bytes written)
    #[inline]
    pub fn position(&self) -> usize {
        self.position.load(Ordering::Relaxed)
    }

    /// Returns the remaining space in the buffer
    #[inline]
    pub fn remaining(&self) -> usize {
        self.size - self.position()
    }

    /// Returns the high water mark
    #[inline]
    pub fn high_water_mark(&self) -> usize {
        self.high_water_mark.load(Ordering::Relaxed)
    }

    /// Returns the buffer name if set
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Checks if the buffer is executable
    #[inline]
    pub fn is_executable(&self) -> bool {
        self.executable
    }

    /// Gets a pointer to the current write position
    #[inline]
    pub fn current_ptr(&self) -> *mut u8 {
        unsafe { self.base.as_ptr().add(self.position()) }
    }

    /// Gets a const pointer to the current write position
    #[inline]
    pub fn current_ptr_const(&self) -> *const u8 {
        unsafe { self.base.as_ptr().add(self.position()) }
    }

    /// Advances the position by the given number of bytes
    pub fn advance(&self, bytes: usize) -> Result<*mut u8, CodeBufferError> {
        let old_pos = self.position.fetch_add(bytes, Ordering::AcqRel);
        let new_pos = old_pos + bytes;

        if new_pos > self.size {
            // Rollback
            self.position.fetch_sub(bytes, Ordering::AcqRel);
            return Err(CodeBufferError::Overflow(new_pos, self.size));
        }

        // Update high water mark
        let mut hwm = self.high_water_mark.load(Ordering::Relaxed);
        while new_pos > hwm {
            match self.high_water_mark.compare_exchange_weak(
                hwm, new_pos, Ordering::Relaxed, Ordering::Relaxed
            ) {
                Ok(_) => break,
                Err(current) => hwm = current,
            }
        }

        Ok(unsafe { self.base.as_ptr().add(old_pos) })
    }

    /// Writes bytes at the current position and advances
    pub fn write(&self, data: &[u8]) -> Result<*mut u8, CodeBufferError> {
        let ptr = self.advance(data.len())?;
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()) };
        Ok(ptr)
    }

    /// Writes a single byte
    pub fn write_byte(&self, byte: u8) -> Result<*mut u8, CodeBufferError> {
        self.write(&[byte])
    }

    /// Writes a u16 (little-endian)
    pub fn write_u16(&self, value: u16) -> Result<*mut u8, CodeBufferError> {
        self.write(&value.to_le_bytes())
    }

    /// Writes a u32 (little-endian)
    pub fn write_u32(&self, value: u32) -> Result<*mut u8, CodeBufferError> {
        self.write(&value.to_le_bytes())
    }

    /// Writes a u64 (little-endian)
    pub fn write_u64(&self, value: u64) -> Result<*mut u8, CodeBufferError> {
        self.write(&value.to_le_bytes())
    }

    /// Writes an i32 (little-endian)
    pub fn write_i32(&self, value: i32) -> Result<*mut u8, CodeBufferError> {
        self.write(&value.to_le_bytes())
    }

    /// Writes an i64 (little-endian)
    pub fn write_i64(&self, value: i64) -> Result<*mut u8, CodeBufferError> {
        self.write(&value.to_le_bytes())
    }

    /// Aligns the current position to the given alignment
    pub fn align(&self, alignment: usize) -> Result<*mut u8, CodeBufferError> {
        let pos = self.position();
        let aligned = (pos + alignment - 1) & !(alignment - 1);
        let padding = aligned - pos;

        if padding > 0 {
            // Fill with NOPs (0x90)
            let ptr = self.advance(padding)?;
            unsafe { std::ptr::write_bytes(ptr, 0x90, padding) };
        }

        Ok(self.current_ptr())
    }

    /// Aligns to instruction boundary (16 bytes)
    pub fn align_instruction(&self) -> Result<*mut u8, CodeBufferError> {
        self.align(INSTRUCTION_ALIGNMENT)
    }

    /// Aligns to cache line boundary (64 bytes)
    pub fn align_cache_line(&self) -> Result<*mut u8, CodeBufferError> {
        self.align(CODE_BUFFER_ALIGNMENT)
    }

    /// Reserves space without writing (for patching later)
    pub fn reserve(&self, bytes: usize) -> Result<*mut u8, CodeBufferError> {
        self.advance(bytes)
    }

    /// Patches bytes at a specific offset
    pub fn patch(&self, offset: usize, data: &[u8]) -> Result<(), CodeBufferError> {
        if offset + data.len() > self.size {
            return Err(CodeBufferError::PatchOutOfBounds(offset, data.len(), self.size));
        }
        if offset + data.len() > self.position() {
            return Err(CodeBufferError::PatchBeyondWritten(offset, data.len(), self.position()));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.base.as_ptr().add(offset), data.len());
        }
        Ok(())
    }

    /// Patches a u32 at a specific offset
    pub fn patch_u32(&self, offset: usize, value: u32) -> Result<(), CodeBufferError> {
        self.patch(offset, &value.to_le_bytes())
    }

    /// Patches a u64 at a specific offset
    pub fn patch_u64(&self, offset: usize, value: u64) -> Result<(), CodeBufferError> {
        self.patch(offset, &value.to_le_bytes())
    }

    /// Patches an i32 at a specific offset
    pub fn patch_i32(&self, offset: usize, value: i32) -> Result<(), CodeBufferError> {
        self.patch(offset, &value.to_le_bytes())
    }

    /// Patches a relative jump/call offset (32-bit signed displacement)
    pub fn patch_rel32(&self, offset: usize, target: *const u8) -> Result<(), CodeBufferError> {
        let base = self.base() as usize;
        let target_addr = target as usize;
        let instruction_addr = base + offset;
        let displacement = (target_addr as isize - (instruction_addr + 4) as isize) as i32;
        self.patch_i32(offset, displacement)
    }

    /// Makes the buffer read-only executable (RX) - for production use
    pub fn make_readonly_executable(&mut self) -> Result<(), CodeBufferError> {
        #[cfg(unix)]
        {
            use libc::{mprotect, PROT_READ, PROT_EXEC};
            let result = unsafe { mprotect(self.base.as_ptr() as *mut _, self.size, PROT_READ | PROT_EXEC) };
            if result != 0 {
                return Err(CodeBufferError::ProtectionFailed);
            }
        }

        #[cfg(windows)]
        {
            let mut old_protect: u32 = 0;
            let result = unsafe {
                win32::VirtualProtect(self.base.as_ptr() as *mut _, self.size, win32::PAGE_EXECUTE_READ, &mut old_protect)
            };
            if result == 0 {
                return Err(CodeBufferError::ProtectionFailed);
            }
        }

        self.executable = true;
        Ok(())
    }

    /// Makes the buffer writable again (RW)
    pub fn make_writable(&mut self) -> Result<(), CodeBufferError> {
        #[cfg(unix)]
        {
            use libc::{mprotect, PROT_READ, PROT_WRITE};
            let result = unsafe { mprotect(self.base.as_ptr() as *mut _, self.size, PROT_READ | PROT_WRITE) };
            if result != 0 {
                return Err(CodeBufferError::ProtectionFailed);
            }
        }

        #[cfg(windows)]
        {
            let mut old_protect: u32 = 0;
            let result = unsafe {
                win32::VirtualProtect(self.base.as_ptr() as *mut _, self.size, win32::PAGE_READWRITE, &mut old_protect)
            };
            if result == 0 {
                return Err(CodeBufferError::ProtectionFailed);
            }
        }

        self.executable = false;
        Ok(())
    }

    /// Flushes instruction cache for the written region
    pub fn flush_icache(&self) {
        let written = self.position();
        if written == 0 {
            return;
        }

        #[cfg(target_arch = "x86_64")]
        {
            // On x86_64, instruction cache is coherent with data cache
            // but we still need a memory barrier
            std::sync::atomic::fence(Ordering::SeqCst);
        }

        #[cfg(target_arch = "aarch64")]
        {
            // On ARM64, we need explicit cache flush
            unsafe {
                // __builtin___clear_cache equivalent
                std::arch::asm!("dsb ish; isb ish", options(nomem, nostack, preserves_flags));
            }
        }
    }

    /// Returns a slice of the written code
    pub fn code_slice(&self) -> &[u8] {
        let written = self.position();
        unsafe { std::slice::from_raw_parts(self.base.as_ptr(), written) }
    }

    /// Returns a mutable slice of the written code (if writable)
    pub fn code_slice_mut(&mut self) -> &mut [u8] {
        let written = self.position();
        unsafe { std::slice::from_raw_parts_mut(self.base.as_ptr(), written) }
    }

    /// Resets the buffer position to 0 (does not deallocate)
    pub fn reset(&self) {
        self.position.store(0, Ordering::Relaxed);
    }

    /// Checks if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.position() == 0
    }

    /// Checks if the buffer is full
    pub fn is_full(&self) -> bool {
        self.position() >= self.size
    }

    /// Returns utilization ratio (0.0 to 1.0)
    pub fn utilization(&self) -> f64 {
        self.position() as f64 / self.size as f64
    }
}

impl Drop for CodeBuffer {
    fn drop(&mut self) {
        free_executable(self.base.as_ptr(), self.size);
    }
}

#[cfg(windows)]
fn allocate_executable(size: usize) -> Result<*mut u8, CodeBufferError> {
    let ptr = unsafe {
        win32::VirtualAlloc(std::ptr::null_mut(), size, win32::MEM_COMMIT | win32::MEM_RESERVE, win32::PAGE_EXECUTE_READWRITE)
    };
    if ptr.is_null() { Err(CodeBufferError::AllocationFailed(size)) } else { Ok(ptr as *mut u8) }
}

#[cfg(windows)]
fn free_executable(ptr: *mut u8, _size: usize) {
    unsafe { win32::VirtualFree(ptr as *mut _, 0, win32::MEM_RELEASE); }
}

#[cfg(unix)]
fn allocate_executable(size: usize) -> Result<*mut u8, CodeBufferError> {
    let ptr = unsafe {
        libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0)
    };
    if ptr == libc::MAP_FAILED { Err(CodeBufferError::AllocationFailed(size)) } else { Ok(ptr as *mut u8) }
}

#[cfg(unix)]
fn free_executable(ptr: *mut u8, size: usize) {
    unsafe { libc::munmap(ptr as *mut _, size); }
}

// SAFETY: CodeBuffer manages its own memory and provides safe access
unsafe impl Send for CodeBuffer {}
unsafe impl Sync for CodeBuffer {}

/// Errors that can occur with CodeBuffer operations
#[derive(Debug)]
pub enum CodeBufferError {
    InvalidSize(usize),
    LayoutError,
    AllocationFailed(usize),
    ProtectionFailed,
    Overflow(usize, usize),
    PatchOutOfBounds(usize, usize, usize),
    PatchBeyondWritten(usize, usize, usize),
}

impl std::fmt::Display for CodeBufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSize(s) => write!(f, "invalid buffer size {} (must be between {} and {})", s, MIN_BUFFER_SIZE, MAX_BUFFER_SIZE),
            Self::LayoutError => write!(f, "memory layout error"),
            Self::AllocationFailed(s) => write!(f, "memory allocation failed for size {}", s),
            Self::ProtectionFailed => write!(f, "memory protection change failed"),
            Self::Overflow(pos, size) => write!(f, "buffer overflow: tried to write to position {}, buffer size is {}", pos, size),
            Self::PatchOutOfBounds(o, l, s) => write!(f, "patch out of bounds: offset {}, length {}, buffer size {}", o, l, s),
            Self::PatchBeyondWritten(o, l, w) => write!(f, "patch beyond written data: offset {}, length {}, written {}", o, l, w),
        }
    }
}

impl std::error::Error for CodeBufferError {}

/// Returns the system page size
fn page_size() -> usize {
    #[cfg(unix)]
    {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }
    #[cfg(windows)]
    {
        4096 // Windows page size is always 4KB
    }
}

/// A handle to a region within a CodeBuffer for relocation/patching
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeBufferHandle {
    /// Offset from buffer base
    pub offset: usize,
    /// Size of the region
    pub size: usize,
}

impl CodeBufferHandle {
    pub fn new(offset: usize, size: usize) -> Self {
        Self { offset, size }
    }

    pub fn end(&self) -> usize {
        self.offset + self.size
    }
}

/// A growing code buffer that can expand when full
pub struct GrowingCodeBuffer {
    buffers: Vec<CodeBuffer>,
    current: usize,
    initial_size: usize,
    growth_factor: f64,
}

impl GrowingCodeBuffer {
    pub fn new(initial_size: usize) -> Result<Self, CodeBufferError> {
        let buffer = CodeBuffer::new(initial_size)?;
        Ok(Self {
            buffers: vec![buffer],
            current: 0,
            initial_size,
            growth_factor: 2.0,
        })
    }

    pub fn current(&self) -> &CodeBuffer {
        &self.buffers[self.current]
    }

    pub fn current_mut(&mut self) -> &mut CodeBuffer {
        &mut self.buffers[self.current]
    }

    pub fn position(&self) -> usize {
        self.buffers[self.current].position()
    }

    pub fn advance(&mut self, bytes: usize) -> Result<*mut u8, CodeBufferError> {
        let current = self.current();
        if current.remaining() >= bytes {
            return current.advance(bytes);
        }

        // Need to grow
        let _ = current;
        self.grow()?;
        self.buffers[self.current].advance(bytes)
    }

    fn grow(&mut self) -> Result<(), CodeBufferError> {
        let current_size = self.buffers[self.current].size();
        let new_size = (current_size as f64 * self.growth_factor) as usize;
        let new_size = new_size.min(MAX_BUFFER_SIZE).max(MIN_BUFFER_SIZE);

        let new_buffer = CodeBuffer::new(new_size)?;
        self.buffers.push(new_buffer);
        self.current += 1;
        Ok(())
    }

    pub fn write(&mut self, data: &[u8]) -> Result<*mut u8, CodeBufferError> {
        let ptr = self.advance(data.len())?;
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()) };
        Ok(ptr)
    }

    pub fn flush_icache(&self) {
        for buffer in &self.buffers {
            buffer.flush_icache();
        }
    }

    pub fn total_written(&self) -> usize {
        self.buffers.iter().map(|b| b.position()).sum()
    }

    pub fn total_capacity(&self) -> usize {
        self.buffers.iter().map(|b| b.size()).sum()
    }
}