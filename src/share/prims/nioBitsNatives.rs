//! Native methods for `java.nio.Bits` — the off-heap primitives that back
//! `ByteBuffer.allocateDirect` and direct-buffer bulk copies.
//!
//! HotSpot implements these over the same off-heap allocator that
//! `Unsafe.allocateMemory` uses; here they delegate to [`OffHeapMemory`].

use std::io;

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;

use super::offHeapMemory::OffHeapMemory;

fn long_arg(arguments: &[Value<'_>], index: usize) -> io::Result<i64> {
    match arguments.get(index) {
        Some(Value::Long(value)) => Ok(*value),
        _ => Err(io::Error::other("Expected long argument")),
    }
}

/// `Bits.reserveMemory(long)J` — allocates off-heap memory.
fn reserve_memory(arguments: &[Value<'_>]) -> io::Result<Value<'static>> {
    let size = usize::try_from(long_arg(arguments, 0)?)
        .map_err(|_| io::Error::other("NegativeArraySizeException"))?;
    Ok(Value::Long(
        OffHeapMemory::global().allocate_memory(size) as i64,
    ))
}

/// `Bits.freeMemory(long)V` — releases an off-heap block.
fn free_memory(arguments: &[Value<'_>]) -> io::Result<()> {
    let address = u64::try_from(long_arg(arguments, 0)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    OffHeapMemory::global().free_memory(address);
    Ok(())
}

/// `Bits.setMemory(long addr, long bytes, byte value)V`.
fn set_memory(arguments: &[Value<'_>]) -> io::Result<()> {
    let address = u64::try_from(long_arg(arguments, 0)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let bytes = usize::try_from(long_arg(arguments, 1)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let value = match arguments.get(2) {
        Some(Value::Int(v)) => *v as u8,
        _ => 0,
    };
    OffHeapMemory::global().set_memory(address, bytes, value)
}

/// `Bits.copyMemory(long src, long dst, long bytes)V` — off-heap to off-heap.
fn copy_memory_offheap(arguments: &[Value<'_>]) -> io::Result<()> {
    let src = u64::try_from(long_arg(arguments, 0)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let dst = u64::try_from(long_arg(arguments, 1)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let bytes = usize::try_from(long_arg(arguments, 2)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    OffHeapMemory::global().copy_memory(src, dst, bytes)
}

/// `Bits.copyMemory(long srcAddress, Object dstBase, long dstOffset, long bytes)V`
/// — copies from an off-heap address into a heap object (byte[] in practice).
fn copy_memory_to_object<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<()> {
    let src = u64::try_from(long_arg(arguments, 0)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let bytes = usize::try_from(long_arg(arguments, 3)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let offset = long_arg(arguments, 2)? as usize;
    let Some(Value::PrimitiveArray(handle)) = arguments.get(1).copied() else {
        return Err(io::Error::other("Expected array destination"));
    };
    let mut arrays = loader.arrays.borrow_mut();
    let Some(TypeArrayOop::Byte(array)) = arrays.get_mut(handle) else {
        return Err(io::Error::other("Expected byte[] destination"));
    };
    if offset + bytes > array.len() {
        return Err(io::Error::other("ArrayIndexOutOfBoundsException"));
    }
    let offheap = OffHeapMemory::global();
    for i in 0..bytes {
        array[offset + i] = offheap.get_byte(src + i as u64)? as i8;
    }
    Ok(())
}

/// `Bits.copyMemory(long srcAddress, Object srcBase, long srcOffset, long bytes)V`
/// variant used for heap->off-heap when the base object is a byte[]: copies
/// `bytes` from `srcBase[srcOffset]` to off-heap `srcAddress`.
fn copy_memory_from_object<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<()> {
    let dst = u64::try_from(long_arg(arguments, 0)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let bytes = usize::try_from(long_arg(arguments, 3)?)
        .map_err(|_| io::Error::other("IllegalArgumentException"))?;
    let offset = long_arg(arguments, 2)? as usize;
    let Some(Value::PrimitiveArray(handle)) = arguments.get(1).copied() else {
        return Err(io::Error::other("Expected array source"));
    };
    let arrays = loader.arrays.borrow();
    let Some(TypeArrayOop::Byte(array)) = arrays.get(handle) else {
        return Err(io::Error::other("Expected byte[] source"));
    };
    if offset + bytes > array.len() {
        return Err(io::Error::other("ArrayIndexOutOfBoundsException"));
    }
    let offheap = OffHeapMemory::global();
    for i in 0..bytes {
        offheap.put_byte(dst + i as u64, array[offset + i] as i8)?;
    }
    Ok(())
}

pub(crate) fn is_owner(owner: &[u8]) -> bool {
    owner == b"java/nio/Bits"
}

pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
    _output: &mut dyn std::io::Write,
) -> io::Result<Option<Option<Value<'a>>>> {
    let result: Option<Value<'a>> = match (name, descriptor) {
        (b"reserveMemory", b"(J)J") => Some(reserve_memory(arguments)?),
        (b"freeMemory", b"(J)V") => {
            free_memory(arguments)?;
            None
        }
        (b"setMemory", b"(JJB)V") => {
            set_memory(arguments)?;
            None
        }
        (b"copyMemory", b"(JJJ)V") => {
            copy_memory_offheap(arguments)?;
            None
        }
        (b"copyMemory", b"(JLjava/lang/Object;JJB)V") => {
            // The object operand can be source or destination depending on the
            // call site; Bits.copyMemory(long, Object, long, long) copies from
            // off-heap `srcAddress` into `base+offset`. DirectByteBuffer bulk
            // puts instead route through Unsafe.copyMemory, so this path is
            // exercised mainly by heap-buffer transfers.
            copy_memory_to_object(loader, arguments)?;
            None
        }
        _ => return Ok(None),
    };
    let _ = copy_memory_from_object;
    Ok(Some(result))
}
