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

    /// Element (compressed-oop) size in bytes for the given layout mode.
    pub fn element_size_in_bytes(mode: ObjLayoutMode) -> usize {
        mode.narrow_oop_size()
    }

    /// Byte offset of element 0 — identical to `base_offset_in_bytes`,
    /// named for symmetry with `TypeArrayOop`/`ArrayLayout`.
    pub fn data_offset_in_bytes(mode: ObjLayoutMode) -> usize {
        Self::base_offset_in_bytes(mode)
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

    /// JVM type descriptor of this array (`[Ljava/lang/Object;`,
    /// `[[I`, ...).  `component` stores the element name without the
    /// `L...;` wrapper for reference types.
    pub fn descriptor(&self) -> Vec<u8> {
        let mut desc = Vec::with_capacity(self.component.len() + 4);
        desc.push(b'[');
        if self.component.first() == Some(&b'[') {
            desc.extend_from_slice(self.component);
        } else {
            desc.push(b'L');
            desc.extend_from_slice(self.component);
            desc.push(b';');
        }
        desc
    }

    /// Number of array dimensions (`[[I` -> 2).
    pub fn dimension(&self) -> usize {
        self.descriptor().iter().take_while(|b| **b == b'[').count()
    }

    /// Whether this is a multi-dimensional array (elements are
    /// themselves arrays).
    pub fn is_multi_dimensional(&self) -> bool {
        self.component.first() == Some(&b'[')
    }

    /// Element descriptor: the array descriptor minus its outermost
    /// dimension (`[I` -> `I`, `[[Ljava/lang/String;` -> `[Ljava/lang/String;`).
    pub fn element_descriptor(&self) -> Vec<u8> {
        self.descriptor()[1..].to_vec()
    }

    /// Byte offset of the `length` field for the given layout mode.
    #[inline]
    pub fn length_offset_in_bytes(mode: ObjLayoutMode) -> usize {
        ArrayOopDesc::length_offset_in_bytes(mode)
    }

    /// Byte offset of element `index` for the given layout mode
    /// (compressed-oop elements).
    #[inline]
    pub fn obj_at_offset(mode: ObjLayoutMode, index: usize) -> usize {
        Self::base_offset_in_bytes(mode) + index * mode.narrow_oop_size()
    }

    /// Fills every element with `value` (HotSpot `ObjArrayOop::fill`).
    pub fn fill(&mut self, value: Value<'a>) {
        self.elements.fill(value);
    }

    /// Copies `src` elements into this array from `src_offset` for
    /// `count` elements (HotSpot `ObjArrayOop::obj_at_put` bulk form).
    pub fn copy_elements_from(
        &mut self,
        src: &[Value<'a>],
        src_offset: usize,
        count: usize,
    ) -> io::Result<()> {
        if src_offset + count > src.len() || count > self.elements.len() {
            return Err(Error::other("ArrayIndexOutOfBoundsException"));
        }
        self.elements[..count]
            .copy_from_slice(&src[src_offset..src_offset + count]);
        Ok(())
    }
}
