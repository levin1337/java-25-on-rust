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

/// Kind of a resolved constant-pool member reference (JVMS §4.4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRefKind {
    Field,
    Method,
    InterfaceMethod,
}

/// Resolved view of a Fieldref/Methodref/InterfaceMethodref constant:
/// owner class internal name plus member name and descriptor.  This is
/// the exact triple JIT stub sites and resolved cache entries need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberRef<'a> {
    pub kind: MemberRefKind,
    pub owner: &'a [u8],
    pub name: &'a [u8],
    pub descriptor: &'a [u8],
}

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

    /// Resolves a Fieldref/Methodref/InterfaceMethodref index to its
    /// owner/name/descriptor triple, mirroring HotSpot
    /// `ConstantPool::klass_ref_at` + `name_and_type_at` composition.
    pub fn member_ref_at(&self, index: u16) -> io::Result<MemberRef<'a>> {
        let (kind, owner_idx, nat_idx) = match self.get(index)? {
            Constant::FieldRef(owner, nat) => (MemberRefKind::Field, *owner, *nat),
            Constant::MethodRef(owner, nat) => (MemberRefKind::Method, *owner, *nat),
            Constant::InterfaceMethodRef(owner, nat) => {
                (MemberRefKind::InterfaceMethod, *owner, *nat)
            }
            other => {
                return Err(Error::other(format!(
                    "Expected member reference constant, got {:?}",
                    other
                )));
            }
        };
        let owner = self.class_name_at(owner_idx)?;
        let (name, descriptor) = self.name_and_type_at(nat_idx)?;
        Ok(MemberRef {
            kind,
            owner,
            name,
            descriptor,
        })
    }

    /// Kind of the constant at `index` when it is a member reference.
    pub fn member_ref_kind_at(&self, index: u16) -> io::Result<Option<MemberRefKind>> {
        Ok(match self.get(index)? {
            Constant::FieldRef(_, _) => Some(MemberRefKind::Field),
            Constant::MethodRef(_, _) => Some(MemberRefKind::Method),
            Constant::InterfaceMethodRef(_, _) => Some(MemberRefKind::InterfaceMethod),
            _ => None,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn utf8(s: &'static [u8]) -> Constant<'static> {
        Constant::Utf8(s)
    }

    fn pool_fixture() -> Vec<Constant<'static>> {
        vec![
            Constant::Unusable,                    // 0
            utf8(b"java/lang/Object"),             // 1
            Constant::Class(1),                    // 2
            utf8(b"count"),                        // 3
            utf8(b"I"),                            // 4
            Constant::NameAndType(3, 4),           // 5
            Constant::FieldRef(2, 5),              // 6
            utf8(b"java/lang/String"),             // 7
            Constant::Class(7),                    // 8
            utf8(b"length"),                       // 9
            utf8(b"()I"),                          // 10
            Constant::NameAndType(9, 10),          // 11
            Constant::MethodRef(8, 11),            // 12
            Constant::InterfaceMethodRef(2, 11),  // 13
            Constant::Integer(42),                 // 14
        ]
    }

    #[test]
    fn member_ref_at_resolves_fieldref() {
        let constants = pool_fixture();
        let pool = ConstantPool::new(&constants);
        let reference = pool.member_ref_at(6).unwrap();
        assert_eq!(reference.kind, MemberRefKind::Field);
        assert_eq!(reference.owner, b"java/lang/Object".as_slice());
        assert_eq!(reference.name, b"count".as_slice());
        assert_eq!(reference.descriptor, b"I".as_slice());
    }

    #[test]
    fn member_ref_at_resolves_method_and_interface_method() {
        let constants = pool_fixture();
        let pool = ConstantPool::new(&constants);
        let method = pool.member_ref_at(12).unwrap();
        assert_eq!(method.kind, MemberRefKind::Method);
        assert_eq!(method.owner, b"java/lang/String".as_slice());
        assert_eq!(method.name, b"length".as_slice());
        assert_eq!(method.descriptor, b"()I".as_slice());
        let iface = pool.member_ref_at(13).unwrap();
        assert_eq!(iface.kind, MemberRefKind::InterfaceMethod);
        assert_eq!(iface.owner, b"java/lang/Object".as_slice());
    }

    #[test]
    fn member_ref_at_rejects_non_member_constants() {
        let constants = pool_fixture();
        let pool = ConstantPool::new(&constants);
        assert!(pool.member_ref_at(14).is_err());
        assert!(pool.member_ref_at(1).is_err());
        assert!(pool.member_ref_at(99).is_err());
    }

    #[test]
    fn member_ref_kind_at_reports_kind() {
        let constants = pool_fixture();
        let pool = ConstantPool::new(&constants);
        assert_eq!(
            pool.member_ref_kind_at(6).unwrap(),
            Some(MemberRefKind::Field)
        );
        assert_eq!(
            pool.member_ref_kind_at(13).unwrap(),
            Some(MemberRefKind::InterfaceMethod)
        );
        assert_eq!(pool.member_ref_kind_at(14).unwrap(), None);
    }

    #[test]
    fn scalar_accessors_roundtrip() {
        let constants = pool_fixture();
        let pool = ConstantPool::new(&constants);
        assert_eq!(pool.int_at(14).unwrap(), 42);
        assert_eq!(pool.utf8_at(3).unwrap(), b"count".as_slice());
        assert_eq!(pool.class_name_at(8).unwrap(), b"java/lang/String".as_slice());
        assert_eq!(pool.name_and_type_at(11).unwrap(), (b"length".as_slice(), b"()I".as_slice()));
        assert_eq!(pool.tag_at(12).unwrap(), ConstantTag::Methodref);
        assert_eq!(pool.tag_at(6).unwrap(), ConstantTag::Fieldref);
    }
}
