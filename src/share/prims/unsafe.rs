/*
 * Port of OpenJDK 25 HotSpot Unsafe (share/prims/unsafe.hpp / unsafe.cpp)
 *
 * Implements JDK internal Unsafe operations in 100% safe Rust:
 * - Direct field access by offset (get/put for primitive types and references)
 * - Atomic compare-and-set (CAS) operations
 * - Atomic compare-and-exchange operations
 */

use crate::share::interpreter::bytecodeInterpreter::Value;
use std::io::{self, Error};

pub struct Unsafe;

impl Unsafe {
    pub(crate) fn arrayLayout(class: &[u8]) -> io::Result<i32> {
        Ok(match class {
            b"[Z" | b"[B" => 1,
            b"[S" | b"[C" => 2,
            b"[I" | b"[F" => 4,
            b"[J" | b"[D" => 8,
            [b'[', b'[' | b'L', ..] => size_of::<usize>() as i32,
            _ => {
                return Err(Error::other(
                    "IllegalArgumentException: expected array class",
                ));
            }
        })
    }

    pub(crate) fn platformConstant(name: &[u8], descriptor: &[u8]) -> Option<i32> {
        match (name, descriptor) {
            (b"ADDRESS_SIZE0", b"I") => Some(size_of::<usize>() as i32),
            (b"BIG_ENDIAN", b"Z") => Some(i32::from(cfg!(target_endian = "big"))),
            _ => None,
        }
    }
    // ------------------------------------------------------------------------
    // Int operations
    // ------------------------------------------------------------------------

    pub fn get_int(fields: &[Value<'_>], offset: usize) -> io::Result<i32> {
        match fields.get(offset) {
            Some(Value::Int(v)) => Ok(*v),
            Some(_) => Err(Error::other("Type mismatch in Unsafe.getInt")),
            None => Err(Error::other("Offset out of bounds in Unsafe.getInt")),
        }
    }

    pub fn put_int(fields: &mut [Value<'_>], offset: usize, val: i32) -> io::Result<()> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.putInt"))?;
        *field = Value::Int(val);
        Ok(())
    }

    pub fn compare_and_set_int(
        fields: &mut [Value<'_>],
        offset: usize,
        expected: i32,
        val: i32,
    ) -> io::Result<bool> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.compareAndSetInt"))?;
        match field {
            Value::Int(current) if *current == expected => {
                *current = val;
                Ok(true)
            }
            Value::Int(_) => Ok(false),
            _ => Err(Error::other("Type mismatch in Unsafe.compareAndSetInt")),
        }
    }

    pub fn compare_and_exchange_int(
        fields: &mut [Value<'_>],
        offset: usize,
        expected: i32,
        val: i32,
    ) -> io::Result<i32> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.compareAndExchangeInt"))?;
        match field {
            Value::Int(current) => {
                let old = *current;
                if old == expected {
                    *current = val;
                }
                Ok(old)
            }
            _ => Err(Error::other(
                "Type mismatch in Unsafe.compareAndExchangeInt",
            )),
        }
    }

    // ------------------------------------------------------------------------
    // Long operations
    // ------------------------------------------------------------------------

    pub fn get_long(fields: &[Value<'_>], offset: usize) -> io::Result<i64> {
        match fields.get(offset) {
            Some(Value::Long(v)) => Ok(*v),
            Some(_) => Err(Error::other("Type mismatch in Unsafe.getLong")),
            None => Err(Error::other("Offset out of bounds in Unsafe.getLong")),
        }
    }

    pub fn put_long(fields: &mut [Value<'_>], offset: usize, val: i64) -> io::Result<()> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.putLong"))?;
        *field = Value::Long(val);
        Ok(())
    }

    pub fn compare_and_set_long(
        fields: &mut [Value<'_>],
        offset: usize,
        expected: i64,
        val: i64,
    ) -> io::Result<bool> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.compareAndSetLong"))?;
        match field {
            Value::Long(current) if *current == expected => {
                *current = val;
                Ok(true)
            }
            Value::Long(_) => Ok(false),
            _ => Err(Error::other("Type mismatch in Unsafe.compareAndSetLong")),
        }
    }

    pub fn compare_and_exchange_long(
        fields: &mut [Value<'_>],
        offset: usize,
        expected: i64,
        val: i64,
    ) -> io::Result<i64> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.compareAndExchangeLong"))?;
        match field {
            Value::Long(current) => {
                let old = *current;
                if old == expected {
                    *current = val;
                }
                Ok(old)
            }
            _ => Err(Error::other(
                "Type mismatch in Unsafe.compareAndExchangeLong",
            )),
        }
    }

    // ------------------------------------------------------------------------
    // Float and Double operations
    // ------------------------------------------------------------------------

    pub fn get_float(fields: &[Value<'_>], offset: usize) -> io::Result<f32> {
        match fields.get(offset) {
            Some(Value::Float(v)) => Ok(*v),
            Some(_) => Err(Error::other("Type mismatch in Unsafe.getFloat")),
            None => Err(Error::other("Offset out of bounds in Unsafe.getFloat")),
        }
    }

    pub fn put_float(fields: &mut [Value<'_>], offset: usize, val: f32) -> io::Result<()> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.putFloat"))?;
        *field = Value::Float(val);
        Ok(())
    }

    pub fn get_double(fields: &[Value<'_>], offset: usize) -> io::Result<f64> {
        match fields.get(offset) {
            Some(Value::Double(v)) => Ok(*v),
            Some(_) => Err(Error::other("Type mismatch in Unsafe.getDouble")),
            None => Err(Error::other("Offset out of bounds in Unsafe.getDouble")),
        }
    }

    pub fn put_double(fields: &mut [Value<'_>], offset: usize, val: f64) -> io::Result<()> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.putDouble"))?;
        *field = Value::Double(val);
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Boolean / Byte / Char / Short operations
    // ------------------------------------------------------------------------

    pub fn get_boolean(fields: &[Value<'_>], offset: usize) -> io::Result<bool> {
        Self::get_int(fields, offset).map(|v| v != 0)
    }

    pub fn put_boolean(fields: &mut [Value<'_>], offset: usize, val: bool) -> io::Result<()> {
        Self::put_int(fields, offset, if val { 1 } else { 0 })
    }

    pub fn get_byte(fields: &[Value<'_>], offset: usize) -> io::Result<i8> {
        Self::get_int(fields, offset).map(|v| v as i8)
    }

    pub fn put_byte(fields: &mut [Value<'_>], offset: usize, val: i8) -> io::Result<()> {
        Self::put_int(fields, offset, val as i32)
    }

    pub fn get_char(fields: &[Value<'_>], offset: usize) -> io::Result<u16> {
        Self::get_int(fields, offset).map(|v| v as u16)
    }

    pub fn put_char(fields: &mut [Value<'_>], offset: usize, val: u16) -> io::Result<()> {
        Self::put_int(fields, offset, val as i32)
    }

    pub fn get_short(fields: &[Value<'_>], offset: usize) -> io::Result<i16> {
        Self::get_int(fields, offset).map(|v| v as i16)
    }

    pub fn put_short(fields: &mut [Value<'_>], offset: usize, val: i16) -> io::Result<()> {
        Self::put_int(fields, offset, val as i32)
    }

    // ------------------------------------------------------------------------
    // Reference operations
    // ------------------------------------------------------------------------

    pub fn get_reference(fields: &[Value<'_>], offset: usize) -> io::Result<Option<usize>> {
        match fields.get(offset) {
            Some(Value::Object(h)) => Ok(Some(*h)),
            Some(Value::Null) => Ok(None),
            Some(_) => Err(Error::other("Type mismatch in Unsafe.getReference")),
            None => Err(Error::other("Offset out of bounds in Unsafe.getReference")),
        }
    }

    pub fn put_reference(
        fields: &mut [Value<'_>],
        offset: usize,
        val: Option<usize>,
    ) -> io::Result<()> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.putReference"))?;
        *field = match val {
            Some(h) => Value::Object(h),
            None => Value::Null,
        };
        Ok(())
    }

    pub fn compare_and_set_reference(
        fields: &mut [Value<'_>],
        offset: usize,
        expected: Option<usize>,
        val: Option<usize>,
    ) -> io::Result<bool> {
        let field = fields
            .get_mut(offset)
            .ok_or_else(|| Error::other("Offset out of bounds in Unsafe.compareAndSetReference"))?;
        let current_ref = match field {
            Value::Object(h) => Some(*h),
            Value::Null => None,
            _ => {
                return Err(Error::other(
                    "Type mismatch in Unsafe.compareAndSetReference",
                ));
            }
        };

        if current_ref == expected {
            *field = match val {
                Some(h) => Value::Object(h),
                None => Value::Null,
            };
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ------------------------------------------------------------------------
    // Off-heap native memory operations (Unsafe allocateMemory / direct access)
    // ------------------------------------------------------------------------

    pub fn allocate_memory(bytes: usize) -> u64 {
        super::offHeapMemory::OffHeapMemory::global().allocate_memory(bytes)
    }

    pub fn reallocate_memory(addr: u64, new_size: usize) -> io::Result<u64> {
        super::offHeapMemory::OffHeapMemory::global().reallocate_memory(addr, new_size)
    }

    pub fn free_memory(addr: u64) {
        super::offHeapMemory::OffHeapMemory::global().free_memory(addr)
    }

    pub fn get_byte_direct(addr: u64) -> io::Result<i8> {
        super::offHeapMemory::OffHeapMemory::global().get_byte(addr)
    }

    pub fn put_byte_direct(addr: u64, val: i8) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().put_byte(addr, val)
    }

    pub fn get_short_direct(addr: u64) -> io::Result<i16> {
        super::offHeapMemory::OffHeapMemory::global().get_short(addr)
    }

    pub fn put_short_direct(addr: u64, val: i16) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().put_short(addr, val)
    }

    pub fn get_int_direct(addr: u64) -> io::Result<i32> {
        super::offHeapMemory::OffHeapMemory::global().get_int(addr)
    }

    pub fn put_int_direct(addr: u64, val: i32) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().put_int(addr, val)
    }

    pub fn get_long_direct(addr: u64) -> io::Result<i64> {
        super::offHeapMemory::OffHeapMemory::global().get_long(addr)
    }

    pub fn put_long_direct(addr: u64, val: i64) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().put_long(addr, val)
    }

    pub fn get_float_direct(addr: u64) -> io::Result<f32> {
        super::offHeapMemory::OffHeapMemory::global().get_float(addr)
    }

    pub fn put_float_direct(addr: u64, val: f32) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().put_float(addr, val)
    }

    pub fn get_double_direct(addr: u64) -> io::Result<f64> {
        super::offHeapMemory::OffHeapMemory::global().get_double(addr)
    }

    pub fn put_double_direct(addr: u64, val: f64) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().put_double(addr, val)
    }

    pub fn copy_memory(src: u64, dst: u64, bytes: usize) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().copy_memory(src, dst, bytes)
    }

    pub fn set_memory(addr: u64, bytes: usize, value: u8) -> io::Result<()> {
        super::offHeapMemory::OffHeapMemory::global().set_memory(addr, bytes, value)
    }

    // ------------------------------------------------------------------------
    // Miscellaneous Unsafe utilities
    // ------------------------------------------------------------------------

    #[inline]
    pub fn object_field_offset(offset: usize) -> usize {
        offset
    }

    #[inline]
    pub fn address_size() -> usize {
        std::mem::size_of::<usize>()
    }

    #[inline]
    pub fn page_size() -> usize {
        4096
    }
}
