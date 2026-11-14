/*
 * Port of OpenJDK 25 HotSpot JVM Native Methods (share/prims/jvm.cpp)
 *
 * Implements fundamental runtime native implementations for java.lang.System,
 * java.lang.Class, java.lang.Float, and java.lang.Double.
 */

use super::jvm::Jvm;

pub struct JvmNatives;

impl JvmNatives {
    /// Implements System.arraycopy with bounds and index checks
    #[inline]
    pub fn arraycopy<T: Copy>(
        src: &[T],
        src_pos: usize,
        dest: &mut [T],
        dest_pos: usize,
        length: usize,
    ) -> Result<(), &'static str> {
        Jvm::arraycopy(src, src_pos, dest, dest_pos, length)
    }

    /// Implements System.identityHashCode
    #[inline]
    pub fn identity_hash_code(object_handle: usize) -> i32 {
        Jvm::identity_hash_code(object_handle)
    }

    /// Implements Float.floatToRawIntBits
    #[inline]
    pub fn float_to_raw_int_bits(value: f32) -> i32 {
        Jvm::float_to_raw_int_bits(value)
    }

    /// Implements Float.intBitsToFloat
    #[inline]
    pub fn int_bits_to_float(bits: i32) -> f32 {
        Jvm::int_bits_to_float(bits)
    }

    /// Implements Double.doubleToRawLongBits
    #[inline]
    pub fn double_to_raw_long_bits(value: f64) -> i64 {
        Jvm::double_to_raw_long_bits(value)
    }

    /// Implements Double.longBitsToDouble
    #[inline]
    pub fn long_bits_to_double(bits: i64) -> f64 {
        Jvm::long_bits_to_double(bits)
    }

    /// Implements Class.isPrimitive
    #[inline]
    pub fn is_primitive_class(descriptor: &str) -> bool {
        Jvm::is_primitive_class(descriptor)
    }

    /// Implements Class.isArray
    #[inline]
    pub fn is_array_class(descriptor: &str) -> bool {
        Jvm::is_array_class(descriptor)
    }
}
