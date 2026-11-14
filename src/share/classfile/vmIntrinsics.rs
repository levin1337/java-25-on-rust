/*
 * Port of OpenJDK 25 HotSpot VM Intrinsics
 * (share/classfile/vmIntrinsics.hpp / vmIntrinsics.cpp)
 *
 * Implements VM intrinsic identifications, resolution, and fast execution
 * for Math, Bitwise, String, Arrays, System, and Unsafe operations.
 */

use crate::share::interpreter::bytecodeInterpreter::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum IntrinsicId {
    None = 0,

    // java.lang.Math & StrictMath
    Dsin,
    Dcos,
    Dtan,
    Dsqrt,
    Dlog,
    Dlog10,
    Dpow,
    Dexp,
    MinI,
    MinL,
    MaxI,
    MaxL,
    AbsI,
    AbsL,
    AbsF,
    AbsD,

    // Integer / Long bitwise operations
    NumberOfLeadingZerosI,
    NumberOfLeadingZerosL,
    NumberOfTrailingZerosI,
    NumberOfTrailingZerosL,
    BitCountI,
    BitCountL,
    ReverseBytesI,
    ReverseBytesL,
    ReverseBytesS,
    ReverseI,
    ReverseL,

    // java.lang.String
    StringEquals,
    StringCompareTo,
    StringIndexOf,
    StringIndexOfChar,
    StringIsLatin1,

    // java.lang.System
    Arraycopy,
    CurrentTimeMillis,
    NanoTime,
    IdentityHashCode,

    // java.lang.Object
    ObjectGetClass,
    ObjectHashCode,
    ObjectClone,

    // java.lang.Float / Double raw bits
    FloatToRawIntBits,
    IntBitsToFloat,
    DoubleToRawLongBits,
    LongBitsToDouble,

    // sun.misc.Unsafe / jdk.internal.misc.Unsafe
    UnsafeCompareAndSetInt,
    UnsafeCompareAndSetLong,
    UnsafeCompareAndSetReference,
    UnsafeGetAndAddInt,
    UnsafeGetAndAddLong,
    UnsafeGetAndSetInt,
    UnsafeGetAndSetLong,
    UnsafeGetAndSetReference,
    UnsafeAllocateMemory,
    UnsafeFreeMemory,

    // java.lang.Thread
    ThreadCurrentThread,
    ThreadIsInterrupted,
}

pub struct VmIntrinsics;

impl VmIntrinsics {
    pub fn find_intrinsic(klass: &str, name: &str, sig: &str) -> IntrinsicId {
        match (klass, name, sig) {
            // Math
            ("java/lang/Math", "sin", "(D)D") | ("java/lang/StrictMath", "sin", "(D)D") => {
                IntrinsicId::Dsin
            }
            ("java/lang/Math", "cos", "(D)D") | ("java/lang/StrictMath", "cos", "(D)D") => {
                IntrinsicId::Dcos
            }
            ("java/lang/Math", "tan", "(D)D") | ("java/lang/StrictMath", "tan", "(D)D") => {
                IntrinsicId::Dtan
            }
            ("java/lang/Math", "sqrt", "(D)D") | ("java/lang/StrictMath", "sqrt", "(D)D") => {
                IntrinsicId::Dsqrt
            }
            ("java/lang/Math", "log", "(D)D") | ("java/lang/StrictMath", "log", "(D)D") => {
                IntrinsicId::Dlog
            }
            ("java/lang/Math", "log10", "(D)D") | ("java/lang/StrictMath", "log10", "(D)D") => {
                IntrinsicId::Dlog10
            }
            ("java/lang/Math", "pow", "(DD)D") | ("java/lang/StrictMath", "pow", "(DD)D") => {
                IntrinsicId::Dpow
            }
            ("java/lang/Math", "exp", "(D)D") | ("java/lang/StrictMath", "exp", "(D)D") => {
                IntrinsicId::Dexp
            }
            ("java/lang/Math", "min", "(II)I") => IntrinsicId::MinI,
            ("java/lang/Math", "min", "(JJ)J") => IntrinsicId::MinL,
            ("java/lang/Math", "max", "(II)I") => IntrinsicId::MaxI,
            ("java/lang/Math", "max", "(JJ)J") => IntrinsicId::MaxL,
            ("java/lang/Math", "abs", "(I)I") => IntrinsicId::AbsI,
            ("java/lang/Math", "abs", "(J)J") => IntrinsicId::AbsL,
            ("java/lang/Math", "abs", "(F)F") => IntrinsicId::AbsF,
            ("java/lang/Math", "abs", "(D)D") => IntrinsicId::AbsD,

            // Integer bits
            ("java/lang/Integer", "numberOfLeadingZeros", "(I)I") => {
                IntrinsicId::NumberOfLeadingZerosI
            }
            ("java/lang/Integer", "numberOfTrailingZeros", "(I)I") => {
                IntrinsicId::NumberOfTrailingZerosI
            }
            ("java/lang/Integer", "bitCount", "(I)I") => IntrinsicId::BitCountI,
            ("java/lang/Integer", "reverseBytes", "(I)I") => IntrinsicId::ReverseBytesI,
            ("java/lang/Integer", "reverse", "(I)I") => IntrinsicId::ReverseI,

            // Long bits
            ("java/lang/Long", "numberOfLeadingZeros", "(J)I") => {
                IntrinsicId::NumberOfLeadingZerosL
            }
            ("java/lang/Long", "numberOfTrailingZeros", "(J)I") => {
                IntrinsicId::NumberOfTrailingZerosL
            }
            ("java/lang/Long", "bitCount", "(J)I") => IntrinsicId::BitCountL,
            ("java/lang/Long", "reverseBytes", "(J)J") => IntrinsicId::ReverseBytesL,
            ("java/lang/Long", "reverse", "(J)J") => IntrinsicId::ReverseL,

            // Short bits
            ("java/lang/Short", "reverseBytes", "(S)S") => IntrinsicId::ReverseBytesS,

            // String
            ("java/lang/String", "equals", "(Ljava/lang/Object;)Z") => IntrinsicId::StringEquals,
            ("java/lang/String", "compareTo", "(Ljava/lang/String;)I") => {
                IntrinsicId::StringCompareTo
            }
            ("java/lang/String", "indexOf", "(II)I") => IntrinsicId::StringIndexOfChar,

            // System
            ("java/lang/System", "arraycopy", "(Ljava/lang/Object;ILjava/lang/Object;II)V") => {
                IntrinsicId::Arraycopy
            }
            ("java/lang/System", "currentTimeMillis", "()J") => IntrinsicId::CurrentTimeMillis,
            ("java/lang/System", "nanoTime", "()J") => IntrinsicId::NanoTime,
            ("java/lang/System", "identityHashCode", "(Ljava/lang/Object;)I") => {
                IntrinsicId::IdentityHashCode
            }

            // Object
            ("java/lang/Object", "getClass", "()Ljava/lang/Class;") => IntrinsicId::ObjectGetClass,
            ("java/lang/Object", "hashCode", "()I") => IntrinsicId::ObjectHashCode,
            ("java/lang/Object", "clone", "()Ljava/lang/Object;") => IntrinsicId::ObjectClone,

            // Float / Double raw bits
            ("java/lang/Float", "floatToRawIntBits", "(F)I") => IntrinsicId::FloatToRawIntBits,
            ("java/lang/Float", "intBitsToFloat", "(I)F") => IntrinsicId::IntBitsToFloat,
            ("java/lang/Double", "doubleToRawLongBits", "(D)J") => IntrinsicId::DoubleToRawLongBits,
            ("java/lang/Double", "longBitsToDouble", "(J)D") => IntrinsicId::LongBitsToDouble,

            // Unsafe
            ("sun/misc/Unsafe" | "jdk/internal/misc/Unsafe", "allocateMemory", "(J)J") => {
                IntrinsicId::UnsafeAllocateMemory
            }
            ("sun/misc/Unsafe" | "jdk/internal/misc/Unsafe", "freeMemory", "(J)V") => {
                IntrinsicId::UnsafeFreeMemory
            }
            (
                "sun/misc/Unsafe" | "jdk/internal/misc/Unsafe",
                "compareAndSetInt",
                "(Ljava/lang/Object;JII)Z",
            ) => IntrinsicId::UnsafeCompareAndSetInt,
            (
                "sun/misc/Unsafe" | "jdk/internal/misc/Unsafe",
                "compareAndSetLong",
                "(Ljava/lang/Object;JJJ)Z",
            ) => IntrinsicId::UnsafeCompareAndSetLong,

            _ => IntrinsicId::None,
        }
    }

    pub fn is_intrinsic(klass: &str, name: &str, sig: &str) -> bool {
        Self::find_intrinsic(klass, name, sig) != IntrinsicId::None
    }

    /// Fast execution of pure mathematical and bitwise intrinsics
    pub fn execute_fast(id: IntrinsicId, args: &[Value<'_>]) -> Option<Value<'static>> {
        match id {
            IntrinsicId::Dsin => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(d.sin()))
            }
            IntrinsicId::Dcos => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(d.cos()))
            }
            IntrinsicId::Dtan => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(d.tan()))
            }
            IntrinsicId::Dsqrt => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(d.sqrt()))
            }
            IntrinsicId::Dlog => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(d.ln()))
            }
            IntrinsicId::Dlog10 => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(d.log10()))
            }
            IntrinsicId::Dpow => {
                let base = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                let exp = match args.get(1)? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(base.powf(exp)))
            }
            IntrinsicId::Dexp => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(d.exp()))
            }
            IntrinsicId::MinI => {
                let a = match args.first()? {
                    Value::Int(v) => *v,
                    _ => return None,
                };
                let b = match args.get(1)? {
                    Value::Int(v) => *v,
                    _ => return None,
                };
                Some(Value::Int(a.min(b)))
            }
            IntrinsicId::MinL => {
                let a = match args.first()? {
                    Value::Long(v) => *v,
                    _ => return None,
                };
                let b = match args.get(1)? {
                    Value::Long(v) => *v,
                    _ => return None,
                };
                Some(Value::Long(a.min(b)))
            }
            IntrinsicId::MaxI => {
                let a = match args.first()? {
                    Value::Int(v) => *v,
                    _ => return None,
                };
                let b = match args.get(1)? {
                    Value::Int(v) => *v,
                    _ => return None,
                };
                Some(Value::Int(a.max(b)))
            }
            IntrinsicId::MaxL => {
                let a = match args.first()? {
                    Value::Long(v) => *v,
                    _ => return None,
                };
                let b = match args.get(1)? {
                    Value::Long(v) => *v,
                    _ => return None,
                };
                Some(Value::Long(a.max(b)))
            }
            IntrinsicId::AbsI => {
                let a = match args.first()? {
                    Value::Int(v) => *v,
                    _ => return None,
                };
                Some(Value::Int(a.abs()))
            }
            IntrinsicId::AbsL => {
                let a = match args.first()? {
                    Value::Long(v) => *v,
                    _ => return None,
                };
                Some(Value::Long(a.abs()))
            }
            IntrinsicId::AbsF => {
                let a = match args.first()? {
                    Value::Float(v) => *v,
                    _ => return None,
                };
                Some(Value::Float(a.abs()))
            }
            IntrinsicId::AbsD => {
                let a = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(a.abs()))
            }

            // Bitwise intrinsics
            IntrinsicId::NumberOfLeadingZerosI => {
                let a = match args.first()? {
                    Value::Int(v) => *v as u32,
                    _ => return None,
                };
                Some(Value::Int(a.leading_zeros() as i32))
            }
            IntrinsicId::NumberOfLeadingZerosL => {
                let a = match args.first()? {
                    Value::Long(v) => *v as u64,
                    _ => return None,
                };
                Some(Value::Int(a.leading_zeros() as i32))
            }
            IntrinsicId::NumberOfTrailingZerosI => {
                let a = match args.first()? {
                    Value::Int(v) => *v as u32,
                    _ => return None,
                };
                Some(Value::Int(a.trailing_zeros() as i32))
            }
            IntrinsicId::NumberOfTrailingZerosL => {
                let a = match args.first()? {
                    Value::Long(v) => *v as u64,
                    _ => return None,
                };
                Some(Value::Int(a.trailing_zeros() as i32))
            }
            IntrinsicId::BitCountI => {
                let a = match args.first()? {
                    Value::Int(v) => *v as u32,
                    _ => return None,
                };
                Some(Value::Int(a.count_ones() as i32))
            }
            IntrinsicId::BitCountL => {
                let a = match args.first()? {
                    Value::Long(v) => *v as u64,
                    _ => return None,
                };
                Some(Value::Int(a.count_ones() as i32))
            }
            IntrinsicId::ReverseBytesI => {
                let a = match args.first()? {
                    Value::Int(v) => *v,
                    _ => return None,
                };
                Some(Value::Int(a.swap_bytes()))
            }
            IntrinsicId::ReverseBytesL => {
                let a = match args.first()? {
                    Value::Long(v) => *v,
                    _ => return None,
                };
                Some(Value::Long(a.swap_bytes()))
            }
            IntrinsicId::ReverseI => {
                let a = match args.first()? {
                    Value::Int(v) => *v as u32,
                    _ => return None,
                };
                Some(Value::Int(a.reverse_bits() as i32))
            }
            IntrinsicId::ReverseL => {
                let a = match args.first()? {
                    Value::Long(v) => *v as u64,
                    _ => return None,
                };
                Some(Value::Long(a.reverse_bits() as i64))
            }

            // Raw bits conversion
            IntrinsicId::FloatToRawIntBits => {
                let f = match args.first()? {
                    Value::Float(v) => *v,
                    _ => return None,
                };
                Some(Value::Int(f.to_bits() as i32))
            }
            IntrinsicId::IntBitsToFloat => {
                let i = match args.first()? {
                    Value::Int(v) => *v,
                    _ => return None,
                };
                Some(Value::Float(f32::from_bits(i as u32)))
            }
            IntrinsicId::DoubleToRawLongBits => {
                let d = match args.first()? {
                    Value::Double(v) => *v,
                    _ => return None,
                };
                Some(Value::Long(d.to_bits() as i64))
            }
            IntrinsicId::LongBitsToDouble => {
                let l = match args.first()? {
                    Value::Long(v) => *v,
                    _ => return None,
                };
                Some(Value::Double(f64::from_bits(l as u64)))
            }

            _ => None,
        }
    }
}
