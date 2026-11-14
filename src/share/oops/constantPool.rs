/*
 * Port of OpenJDK 25 HotSpot ConstantPool (share/oops/constantPool.hpp, constantPool.cpp)
 *
 * ConstantPool represents the array of symbolic and literal constants of a class:
 * - Integer, Long, Float, Double primitives
 * - Utf8 strings and String literals
 * - Class, Fieldref, Methodref, InterfaceMethodref symbolic links
 * - NameAndType, MethodHandle, MethodType, Dynamic, InvokeDynamic metadata
 * - Linked ConstantPoolCache for bytecode quickening and execution
 */

use super::cpCache::ConstantPoolCache;
use crate::classfile::Constant;
use crate::share::utilities::constantTag::ConstantTag;
use std::io::{self, Error};

pub struct ConstantPool<'a> {
    constants: &'a [Constant<'a>],
    cache: Option<ConstantPoolCache>,
    pool_holder: Option<String>,
}

impl<'a> ConstantPool<'a> {
    pub fn new(constants: &'a [Constant<'a>]) -> Self {
        Self {
            constants,
            cache: None,
            pool_holder: None,
        }
    }

    pub fn with_cache(constants: &'a [Constant<'a>], cache: ConstantPoolCache) -> Self {
        Self {
            constants,
            cache: Some(cache),
            pool_holder: None,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.constants.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.constants.is_empty()
    }

    #[inline]
    pub fn pool_holder(&self) -> Option<&str> {
        self.pool_holder.as_deref()
    }

    #[inline]
    pub fn set_pool_holder(&mut self, holder: String) {
        self.pool_holder = Some(holder);
    }

    #[inline]
    pub fn cache(&self) -> Option<&ConstantPoolCache> {
        self.cache.as_ref()
    }

    #[inline]
    pub fn cache_mut(&mut self) -> Option<&mut ConstantPoolCache> {
        self.cache.as_mut()
    }

    #[inline]
    pub fn set_cache(&mut self, cache: ConstantPoolCache) {
        self.cache = Some(cache);
    }

    pub fn get(&self, index: u16) -> io::Result<&Constant<'a>> {
        self.constants
            .get(usize::from(index))
            .ok_or_else(|| Error::other("Invalid constant pool index"))
    }

    pub fn tag_at(&self, index: u16) -> io::Result<ConstantTag> {
        let c = self.get(index)?;
        Ok(match c {
            Constant::Unusable => ConstantTag::Invalid,
            Constant::Utf8(_) => ConstantTag::Utf8,
            Constant::Integer(_) => ConstantTag::Integer,
            Constant::Float(_) => ConstantTag::Float,
            Constant::Long(_) => ConstantTag::Long,
            Constant::Double(_) => ConstantTag::Double,
            Constant::Class(_) => ConstantTag::Class,
            Constant::String(_) => ConstantTag::String,
            Constant::FieldRef(_, _) => ConstantTag::Fieldref,
            Constant::MethodRef(_, _) => ConstantTag::Methodref,
            Constant::InterfaceMethodRef(_, _) => ConstantTag::InterfaceMethodref,
            Constant::NameAndType(_, _) => ConstantTag::NameAndType,
            Constant::MethodHandle(_, _) => ConstantTag::MethodHandle,
            Constant::MethodType(_) => ConstantTag::MethodType,
            Constant::Dynamic(_, _) => ConstantTag::Dynamic,
            Constant::InvokeDynamic(_, _) => ConstantTag::InvokeDynamic,
            Constant::Module(_) => ConstantTag::Module,
            Constant::Package(_) => ConstantTag::Package,
        })
    }

    pub fn utf8_at(&self, index: u16) -> io::Result<&'a [u8]> {
        match self.get(index)? {
            Constant::Utf8(bytes) => Ok(bytes),
            _ => Err(Error::other("Expected Utf8 constant")),
        }
    }

    pub fn class_name_at(&self, index: u16) -> io::Result<&'a [u8]> {
        match self.get(index)? {
            Constant::Class(name_idx) => self.utf8_at(*name_idx),
            _ => Err(Error::other("Expected Class constant")),
        }
    }

    pub fn string_at(&self, index: u16) -> io::Result<&'a [u8]> {
        match self.get(index)? {
            Constant::String(utf8_idx) => self.utf8_at(*utf8_idx),
            _ => Err(Error::other("Expected String constant")),
        }
    }

    pub fn name_and_type_at(&self, index: u16) -> io::Result<(&'a [u8], &'a [u8])> {
        match self.get(index)? {
            Constant::NameAndType(name_idx, sig_idx) => {
                let name = self.utf8_at(*name_idx)?;
                let sig = self.utf8_at(*sig_idx)?;
                Ok((name, sig))
            }
            _ => Err(Error::other("Expected NameAndType constant")),
        }
    }

    pub fn int_at(&self, index: u16) -> io::Result<i32> {
        match self.get(index)? {
            Constant::Integer(v) => Ok(*v),
            _ => Err(Error::other("Expected Integer constant")),
        }
    }

    pub fn long_at(&self, index: u16) -> io::Result<i64> {
        match self.get(index)? {
            Constant::Long(v) => Ok(*v),
            _ => Err(Error::other("Expected Long constant")),
        }
    }

    pub fn float_at(&self, index: u16) -> io::Result<f32> {
        match self.get(index)? {
            Constant::Float(v) => Ok(*v),
            _ => Err(Error::other("Expected Float constant")),
        }
    }

    pub fn double_at(&self, index: u16) -> io::Result<f64> {
        match self.get(index)? {
            Constant::Double(v) => Ok(*v),
            _ => Err(Error::other("Expected Double constant")),
        }
    }
}
