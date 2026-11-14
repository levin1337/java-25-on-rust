/*
 * Port of OpenJDK 25 HotSpot Reflection (share/runtime/reflection.hpp / reflection.cpp)
 *
 * Reflection implements core JVM support for java.lang.reflect:
 * - Boxing and unboxing of primitive values
 * - Widening of primitive types according to the Java Language Specification (JLS §5.1.2)
 * - Member and class access checking (public, protected, package-private, private)
 */

use crate::share::interpreter::bytecodeInterpreter::Value;

pub struct Reflection;

impl Reflection {
    pub const ACCESS_OK: i32 = 0;
    pub const ACCESS_NOT_ALLOWED: i32 = 1;

    pub const ACC_PUBLIC: u16 = 0x0001;
    pub const ACC_PRIVATE: u16 = 0x0002;
    pub const ACC_PROTECTED: u16 = 0x0004;

    /// Checks if a member with given access_flags can be accessed by an accessor class.
    pub fn check_member_access(
        accessor_klass: &str,
        member_holder: &str,
        member_flags: u16,
    ) -> bool {
        Self::check_member_access_with_subclass(accessor_klass, member_holder, member_flags, false)
    }

    /// Checks member access considering subclass relationship for protected members.
    pub fn check_member_access_with_subclass(
        accessor_klass: &str,
        member_holder: &str,
        member_flags: u16,
        is_subclass: bool,
    ) -> bool {
        let is_public = (member_flags & Self::ACC_PUBLIC) != 0;
        let is_private = (member_flags & Self::ACC_PRIVATE) != 0;
        let is_protected = (member_flags & Self::ACC_PROTECTED) != 0;

        if is_public {
            return true;
        }

        if accessor_klass == member_holder {
            return true;
        }

        if is_private {
            return false;
        }

        let same_package = Self::is_same_class_package(accessor_klass, member_holder);
        if same_package {
            return true;
        }

        if is_protected && is_subclass {
            return true;
        }

        false
    }

    /// Determines if two internal class names belong to the same package.
    pub fn is_same_class_package(klass1: &str, klass2: &str) -> bool {
        Self::package_name(klass1) == Self::package_name(klass2)
    }

    pub fn package_name(klass: &str) -> &str {
        if let Some(pos) = klass.rfind('/') {
            &klass[..pos]
        } else {
            ""
        }
    }

    /// Widens a primitive Value from source type to target type according to JLS §5.1.2.
    /// Supports:
    /// - byte ('B') -> short ('S'), int ('I'), long ('J'), float ('F'), double ('D')
    /// - short ('S') -> int ('I'), long ('J'), float ('F'), double ('D')
    /// - char ('C') -> int ('I'), long ('J'), float ('F'), double ('D')
    /// - int ('I') -> long ('J'), float ('F'), double ('D')
    /// - long ('J') -> float ('F'), double ('D')
    /// - float ('F') -> double ('D')
    pub fn widen_primitive<'a>(
        from_type: &str,
        to_type: &str,
        val: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if from_type == to_type {
            return Ok(val);
        }

        match (from_type, to_type, val) {
            // byte -> short, int, long, float, double
            ("B", "S", Value::Int(i)) => Ok(Value::Int(i)),
            ("B", "I", Value::Int(i)) => Ok(Value::Int(i)),
            ("B", "J", Value::Int(i)) => Ok(Value::Long(i as i64)),
            ("B", "F", Value::Int(i)) => Ok(Value::Float(i as f32)),
            ("B", "D", Value::Int(i)) => Ok(Value::Double(i as f64)),

            // short -> int, long, float, double
            ("S", "I", Value::Int(i)) => Ok(Value::Int(i)),
            ("S", "J", Value::Int(i)) => Ok(Value::Long(i as i64)),
            ("S", "F", Value::Int(i)) => Ok(Value::Float(i as f32)),
            ("S", "D", Value::Int(i)) => Ok(Value::Double(i as f64)),

            // char -> int, long, float, double
            ("C", "I", Value::Int(i)) => Ok(Value::Int(i)),
            ("C", "J", Value::Int(i)) => Ok(Value::Long(i as i64)),
            ("C", "F", Value::Int(i)) => Ok(Value::Float(i as f32)),
            ("C", "D", Value::Int(i)) => Ok(Value::Double(i as f64)),

            // int -> long, float, double
            ("I", "J", Value::Int(i)) => Ok(Value::Long(i as i64)),
            ("I", "F", Value::Int(i)) => Ok(Value::Float(i as f32)),
            ("I", "D", Value::Int(i)) => Ok(Value::Double(i as f64)),

            // long -> float, double
            ("J", "F", Value::Long(l)) => Ok(Value::Float(l as f32)),
            ("J", "D", Value::Long(l)) => Ok(Value::Double(l as f64)),

            // float -> double
            ("F", "D", Value::Float(f)) => Ok(Value::Double(f as f64)),

            _ => Err(format!(
                "Cannot widen from type '{}' to type '{}'",
                from_type, to_type
            )),
        }
    }

    /// Determines the primitive type descriptor ('Z', 'B', 'C', 'S', 'I', 'J', 'F', 'D')
    /// for a given boxing wrapper class name.
    pub fn unbox_target_descriptor(box_class: &str) -> Option<&'static str> {
        match box_class {
            "java/lang/Boolean" => Some("Z"),
            "java/lang/Byte" => Some("B"),
            "java/lang/Character" => Some("C"),
            "java/lang/Short" => Some("S"),
            "java/lang/Integer" => Some("I"),
            "java/lang/Long" => Some("J"),
            "java/lang/Float" => Some("F"),
            "java/lang/Double" => Some("D"),
            _ => None,
        }
    }

    /// Returns the boxing class for a given primitive descriptor.
    pub fn box_class_for_primitive(descriptor: &str) -> Option<&'static str> {
        match descriptor {
            "Z" => Some("java/lang/Boolean"),
            "B" => Some("java/lang/Byte"),
            "C" => Some("java/lang/Character"),
            "S" => Some("java/lang/Short"),
            "I" => Some("java/lang/Integer"),
            "J" => Some("java/lang/Long"),
            "F" => Some("java/lang/Float"),
            "D" => Some("java/lang/Double"),
            _ => None,
        }
    }

    #[inline]
    pub fn is_primitive_descriptor(desc: &str) -> bool {
        matches!(desc, "Z" | "B" | "C" | "S" | "I" | "J" | "F" | "D")
    }
}
