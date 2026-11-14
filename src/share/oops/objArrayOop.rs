use super::arrayOop::ArrayOopDesc;
use super::objLayout::ObjLayoutMode;
use crate::share::interpreter::bytecodeInterpreter::Value;
use std::io::{self, Error};

pub struct ObjArrayOop<'a> {
    pub base: ArrayOopDesc,
    pub elements: Vec<Value<'a>>,
    pub(crate) component: &'a [u8],
}

impl<'a> ObjArrayOop<'a> {
    pub fn new(length: i32) -> io::Result<Self> {
        Self::withComponent(length, b"java/lang/Object")
    }

    pub(crate) fn withComponent(length: i32, component: &'a [u8]) -> io::Result<Self> {
        let len =
            usize::try_from(length).map_err(|_| Error::other("NegativeArraySizeException"))?;
        let mut elements = Vec::new();
        elements.try_reserve_exact(len).map_err(Error::other)?;
        elements.resize(len, Value::Null);
        Ok(Self {
            base: ArrayOopDesc::new(length),
            elements,
            component,
        })
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn get(&self, index: usize) -> io::Result<Value<'a>> {
        self.elements
            .get(index)
            .copied()
            .ok_or_else(|| Error::other("ArrayIndexOutOfBoundsException"))
    }

    pub fn set(&mut self, index: usize, value: Value<'a>) -> io::Result<()> {
        let elem = self
            .elements
            .get_mut(index)
            .ok_or_else(|| Error::other("ArrayIndexOutOfBoundsException"))?;
        *elem = value;
        Ok(())
    }

    pub fn obj_at(&self, index: usize) -> io::Result<Value<'a>> {
        self.get(index)
    }

    pub fn obj_at_put(&mut self, index: usize, value: Value<'a>) -> io::Result<()> {
        self.set(index, value)
    }

    pub fn replace_if_null(
        &mut self,
        index: usize,
        exchange_value: Value<'a>,
    ) -> io::Result<Value<'a>> {
        let elem = self
            .elements
            .get_mut(index)
            .ok_or_else(|| Error::other("ArrayIndexOutOfBoundsException"))?;
        let old = *elem;
        if matches!(old, Value::Null) {
            *elem = exchange_value;
        }
        Ok(old)
    }

    pub fn base_offset_in_bytes(mode: ObjLayoutMode) -> usize {
        let oop_size = mode.narrow_oop_size();
        mode.array_base_offset(oop_size, oop_size)
    }

    pub fn obj_at_offset(mode: ObjLayoutMode, index: usize) -> usize {
        let oop_size = mode.narrow_oop_size();
        Self::base_offset_in_bytes(mode) + index * oop_size
    }

    pub fn object_size_in_bytes(mode: ObjLayoutMode, length: usize) -> usize {
        let oop_size = mode.narrow_oop_size();
        mode.array_allocation_size_in_bytes(length, oop_size, oop_size)
    }

    pub fn object_size_in_words(mode: ObjLayoutMode, length: usize) -> usize {
        Self::object_size_in_bytes(mode, length) / 8
    }

    pub fn component_name(&self) -> &'a [u8] {
        self.component
    }
}
