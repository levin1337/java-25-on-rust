use super::{
    klass::Klass, objArrayOop::ObjArrayOop, objLayout::ObjLayoutMode, symbol::Symbol,
    typeArrayOop::TypeArrayOop,
};
use crate::share::{
    classfile::classLoader::ClassLoader, interpreter::bytecodeInterpreter::Value,
    utilities::accessFlags::AccessFlags,
};
use std::io::{self, Error};

#[derive(Debug, Clone)]
pub struct ArrayKlassDesc {
    pub klass: Klass,
    pub dimension: i32,
    pub higher_dimension: Option<String>,
    pub lower_dimension: Option<String>,
    pub element_type_name: String,
    pub element_size: usize,
}

impl ArrayKlassDesc {
    pub fn new(name: Symbol, dimension: i32, element_type_name: &str, element_size: usize) -> Self {
        let mut flags = AccessFlags::empty();
        flags.set_is_final(true);
        flags.set_is_abstract(false);
        Self {
            klass: Klass::new(name, flags),
            dimension,
            higher_dimension: None,
            lower_dimension: None,
            element_type_name: element_type_name.to_string(),
            element_size,
        }
    }

    pub fn dimension(&self) -> i32 {
        self.dimension
    }

    pub fn set_dimension(&mut self, dimension: i32) {
        self.dimension = dimension;
    }

    pub fn higher_dimension(&self) -> Option<&str> {
        self.higher_dimension.as_deref()
    }

    pub fn set_higher_dimension(&mut self, higher: Option<String>) {
        self.higher_dimension = higher;
    }

    pub fn lower_dimension(&self) -> Option<&str> {
        self.lower_dimension.as_deref()
    }

    pub fn set_lower_dimension(&mut self, lower: Option<String>) {
        self.lower_dimension = lower;
    }

    pub fn element_type_name(&self) -> &str {
        &self.element_type_name
    }

    pub fn element_size(&self) -> usize {
        self.element_size
    }

    pub fn log2_element_size(&self) -> usize {
        match self.element_size {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            _ => 0,
        }
    }

    pub fn is_array_klass(&self) -> bool {
        true
    }

    pub fn array_header_in_bytes(&self, mode: ObjLayoutMode) -> usize {
        mode.array_base_offset(self.element_size, self.element_size)
    }
}

pub struct ArrayKlass;

impl ArrayKlass {
    pub fn primitive<'a>(loader: &ClassLoader<'a>, kind: u8, length: i32) -> io::Result<Value<'a>> {
        let array = TypeArrayOop::new(kind, length)?;
        Ok(Value::PrimitiveArray(loader.alloc_type_array(array)))
    }

    pub fn reference<'a>(
        loader: &ClassLoader<'a>,
        component: &'a [u8],
        length: i32,
    ) -> io::Result<Value<'a>> {
        let array = ObjArrayOop::withComponent(length, component)?;
        Ok(Self::publish(loader, array))
    }

    pub fn multi<'a>(
        loader: &ClassLoader<'a>,
        descriptor: &'a [u8],
        lengths: &[i32],
    ) -> io::Result<Value<'a>> {
        let rank = descriptor.iter().take_while(|byte| **byte == b'[').count();
        if lengths.is_empty() || lengths.len() > rank || rank > 255 {
            return Err(Error::other("Invalid array dimensions"));
        }
        let element = &descriptor[rank..];
        if !matches!(
            element,
            [b'Z' | b'C' | b'F' | b'D' | b'B' | b'S' | b'I' | b'J']
        ) && !matches!(element, [b'L', name @ .., b';'] if !name.is_empty() && !name.contains(&b';') && !name.contains(&b'['))
        {
            return Err(Error::other("Invalid array descriptor"));
        }
        if lengths.iter().any(|&length| length < 0) {
            return Err(Error::other("NegativeArraySizeException"));
        }
        let primitives = loader.arrays.borrow().len();
        let references = loader.obj_arrays.borrow().len();
        let result = Self::allocateDimensions(loader, descriptor, lengths);
        if result.is_err() {
            loader.arrays.borrow_mut().truncate(primitives);
            loader.obj_arrays.borrow_mut().truncate(references);
        }
        result
    }

    fn allocateDimensions<'a>(
        loader: &ClassLoader<'a>,
        descriptor: &'a [u8],
        lengths: &[i32],
    ) -> io::Result<Value<'a>> {
        let component = &descriptor[1..];
        let length = lengths[0];
        if let [tag] = component {
            let kind = match tag {
                b'Z' => 4,
                b'C' => 5,
                b'F' => 6,
                b'D' => 7,
                b'B' => 8,
                b'S' => 9,
                b'I' => 10,
                b'J' => 11,
                _ => return Err(Error::other("Invalid primitive array descriptor")),
            };
            return Self::primitive(loader, kind, length);
        }
        let componentName = match component {
            [b'L', name @ .., b';'] => name,
            _ => component,
        };
        let mut array = ObjArrayOop::withComponent(length, componentName)?;
        if lengths.len() > 1 {
            for element in &mut array.elements {
                *element = Self::allocateDimensions(loader, component, &lengths[1..])?;
            }
        }
        Ok(Self::publish(loader, array))
    }

    fn publish<'a>(loader: &ClassLoader<'a>, array: ObjArrayOop<'a>) -> Value<'a> {
        Value::ObjectArray(loader.alloc_obj_array(array))
    }
}
