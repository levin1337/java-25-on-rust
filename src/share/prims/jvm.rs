/*
 * Port of OpenJDK 25 HotSpot JVM Native Methods (share/prims/jvm.hpp / jvm.cpp)
 *
 * Implements fundamental runtime native implementations for java.lang.System,
 * java.lang.Class, java.lang.Float, and java.lang.Double.
 */

use std::{
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct Jvm;

impl Jvm {
    /// Implements System.currentTimeMillis
    pub fn current_time_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Implements System.nanoTime
    pub fn nano_time() -> i64 {
        #[allow(non_upper_case_globals)]
        static origin: OnceLock<Instant> = OnceLock::new();
        origin.get_or_init(Instant::now).elapsed().as_nanos() as i64
    }

    pub fn nanoTimeAdjustment(offsetSeconds: i64) -> i64 {
        let epochNanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos() as i128,
            Err(error) => -(error.duration().as_nanos() as i128),
        };
        let nanosPerSecond = 1_000_000_000;
        let offsetSeconds = i128::from(offsetSeconds);
        let difference = epochNanos.div_euclid(nanosPerSecond) - offsetSeconds;
        if difference.abs() >= 1_i128 << 32 {
            return -1;
        }
        (epochNanos - offsetSeconds * nanosPerSecond) as i64
    }

    /// Implements Runtime.totalMemory
    pub fn total_memory() -> usize {
        128 * 1024 * 1024
    }

    /// Implements Runtime.freeMemory
    pub fn free_memory(used: usize) -> usize {
        Self::total_memory().saturating_sub(used)
    }

    /// Implements Runtime.maxMemory
    pub fn max_memory() -> usize {
        512 * 1024 * 1024
    }

    /// Implements System.arraycopy with bounds and index checks
    pub fn arraycopy<T: Copy>(
        src: &[T],
        src_pos: usize,
        dest: &mut [T],
        dest_pos: usize,
        length: usize,
    ) -> Result<(), &'static str> {
        if length == 0 {
            return Ok(());
        }
        if src_pos
            .checked_add(length)
            .map_or(true, |end| end > src.len())
            || dest_pos
                .checked_add(length)
                .map_or(true, |end| end > dest.len())
        {
            return Err("IndexOutOfBoundsException");
        }

        dest[dest_pos..dest_pos + length].copy_from_slice(&src[src_pos..src_pos + length]);
        Ok(())
    }

    /// Implements System.identityHashCode matching HotSpot ObjectSynchronizer::FastHashCode
    pub fn identity_hash_code(object_handle: usize) -> i32 {
        if object_handle == 0 {
            return 0;
        }
        let mut x = object_handle as u64;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
        x ^= x >> 33;
        let h = (x as i32) & i32::MAX;
        if h == 0 { 1 } else { h }
    }

    /// Implements Float.floatToRawIntBits
    #[inline]
    pub fn float_to_raw_int_bits(value: f32) -> i32 {
        value.to_bits() as i32
    }

    /// Implements Float.intBitsToFloat
    #[inline]
    pub fn int_bits_to_float(bits: i32) -> f32 {
        f32::from_bits(bits as u32)
    }

    /// Implements Double.doubleToRawLongBits
    #[inline]
    pub fn double_to_raw_long_bits(value: f64) -> i64 {
        value.to_bits() as i64
    }

    /// Implements Double.longBitsToDouble
    #[inline]
    pub fn long_bits_to_double(bits: i64) -> f64 {
        f64::from_bits(bits as u64)
    }

    /// Implements Class.isPrimitive
    pub fn is_primitive_class(descriptor: &str) -> bool {
        matches!(
            descriptor,
            "Z" | "B"
                | "C"
                | "S"
                | "I"
                | "J"
                | "F"
                | "D"
                | "V"
                | "boolean"
                | "byte"
                | "char"
                | "short"
                | "int"
                | "long"
                | "float"
                | "double"
                | "void"
        )
    }

    /// Implements Class.isArray
    #[inline]
    pub fn is_array_class(descriptor: &str) -> bool {
        descriptor.starts_with('[')
    }
}
