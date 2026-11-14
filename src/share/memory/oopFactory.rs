/*
 * Port of OpenJDK 25 HotSpot OopFactory (share/memory/oopFactory.hpp / oopFactory.cpp)
 *
 * Provides factory methods for creating OOP instances, basic type leaf arrays,
 * and object arrays in the Java heap.
 */

use crate::share::oops::{
    instanceKlass::InstanceKlass, instanceOop::InstanceOop, objArrayOop::ObjArrayOop,
    typeArrayOop::TypeArrayOop,
};
use std::io;
use std::rc::Rc;

pub struct OopFactory;

impl OopFactory {
    pub fn new_instance<'a>(klass: Rc<InstanceKlass<'a>>) -> io::Result<InstanceOop<'a>> {
        InstanceOop::new(klass)
    }

    pub fn new_type_array(kind: u8, length: i32) -> io::Result<TypeArrayOop> {
        TypeArrayOop::new(kind, length)
    }

    pub fn new_bool_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(4, length)
    }

    pub fn new_char_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(5, length)
    }

    pub fn new_float_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(6, length)
    }

    pub fn new_double_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(7, length)
    }

    pub fn new_byte_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(8, length)
    }

    pub fn new_short_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(9, length)
    }

    pub fn new_int_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(10, length)
    }

    pub fn new_long_array(length: i32) -> io::Result<TypeArrayOop> {
        Self::new_type_array(11, length)
    }

    pub fn new_char_array_from_utf8(s: &str) -> io::Result<TypeArrayOop> {
        let u16_chars: Vec<u16> = s.encode_utf16().collect();
        let len = u16_chars.len() as i32;
        let mut arr = Self::new_char_array(len)?;
        if let TypeArrayOop::Char(ref mut vec) = arr {
            vec.clone_from(&u16_chars);
        }
        Ok(arr)
    }

    pub fn new_byte_array_from_bytes(bytes: &[u8]) -> io::Result<TypeArrayOop> {
        let i8_bytes: Vec<i8> = bytes.iter().map(|&b| b as i8).collect();
        let len = i8_bytes.len() as i32;
        let mut arr = Self::new_byte_array(len)?;
        if let TypeArrayOop::Byte(ref mut vec) = arr {
            vec.clone_from(&i8_bytes);
        }
        Ok(arr)
    }

    pub fn new_obj_array<'a>(length: i32) -> io::Result<ObjArrayOop<'a>> {
        ObjArrayOop::new(length)
    }
}
