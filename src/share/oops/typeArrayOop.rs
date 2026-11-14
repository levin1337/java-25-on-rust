use super::objLayout::ObjLayoutMode;
use std::io::{self, Error};

#[derive(Debug, Clone)]
pub enum TypeArrayOop {
    Boolean(Vec<u8>),
    Char(Vec<u16>),
    Float(Vec<f32>),
    Double(Vec<f64>),
    Byte(Vec<i8>),
    Short(Vec<i16>),
    Int(Vec<i32>),
    Long(Vec<i64>),
}

#[derive(Clone, Copy, Debug)]
pub enum Element {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
}

fn zeros<T: Default + Clone>(length: usize) -> io::Result<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(length).map_err(Error::other)?;
    values.resize(length, T::default());
    Ok(values)
}

macro_rules! access {
    ($array:expr, $kind:expr, $index:expr, $get:ident $(, $value:expr)?) => {
        match ($array, $kind) {
            (TypeArrayOop::Int(values), 0) => $get!(values, $index, Int, i32 $(, $value)?),
            (TypeArrayOop::Long(values), 1) => $get!(values, $index, Long, i64 $(, $value)?),
            (TypeArrayOop::Float(values), 2) => $get!(values, $index, Float, f32 $(, $value)?),
            (TypeArrayOop::Double(values), 3) => $get!(values, $index, Double, f64 $(, $value)?),
            (TypeArrayOop::Byte(values), 5) => $get!(values, $index, Int, i8 $(, $value)?),
            (TypeArrayOop::Boolean(values), 5) => $get!(values, $index, Int, u8 $(, ($value).boolean())?),
            (TypeArrayOop::Char(values), 6) => $get!(values, $index, Int, u16 $(, $value)?),
            (TypeArrayOop::Short(values), 7) => $get!(values, $index, Int, i16 $(, $value)?),
            _ => Err(Error::other("Invalid primitive array type")),
        }
    };
}

macro_rules! load {
    ($values:ident, $index:expr, $kind:ident, $type:ty) => {
        $values
            .get($index)
            .map(|&value| Element::$kind(value.into()))
            .ok_or_else(|| Error::other("Array index out of bounds"))
    };
}

macro_rules! store {
    ($values:ident, $index:expr, $kind:ident, $type:ty, $element:expr) => {{
        let Element::$kind(value) = $element else {
            return Err(Error::other("Invalid array element type"));
        };
        *$values
            .get_mut($index)
            .ok_or_else(|| Error::other("Array index out of bounds"))? = value as $type;
        Ok(())
    }};
}

impl Element {
    fn boolean(self) -> Self {
        match self {
            Self::Int(value) => Self::Int(value & 1),
            _ => self,
        }
    }
}

impl TypeArrayOop {
    pub(crate) fn descriptor(&self) -> &'static [u8] {
        match self {
            Self::Boolean(_) => b"[Z",
            Self::Char(_) => b"[C",
            Self::Float(_) => b"[F",
            Self::Double(_) => b"[D",
            Self::Byte(_) => b"[B",
            Self::Short(_) => b"[S",
            Self::Int(_) => b"[I",
            Self::Long(_) => b"[J",
        }
    }

    pub fn new(kind: u8, length: i32) -> io::Result<Self> {
        let length = usize::try_from(length).map_err(|_| Error::other("Negative array size"))?;
        Ok(match kind {
            4 => Self::Boolean(zeros(length)?),
            5 => Self::Char(zeros(length)?),
            6 => Self::Float(zeros(length)?),
            7 => Self::Double(zeros(length)?),
            8 => Self::Byte(zeros(length)?),
            9 => Self::Short(zeros(length)?),
            10 => Self::Int(zeros(length)?),
            11 => Self::Long(zeros(length)?),
            _ => return Err(Error::other("Invalid newarray type")),
        })
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Boolean(values) => values.len(),
            Self::Char(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Double(values) => values.len(),
            Self::Byte(values) => values.len(),
            Self::Short(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Long(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn load(&self, kind: u8, index: usize) -> io::Result<Element> {
        access!(self, kind, index, load)
    }

    pub fn store(&mut self, kind: u8, index: usize, value: Element) -> io::Result<()> {
        access!(self, kind, index, store, value)
    }

    pub fn element_size(&self) -> usize {
        match self {
            Self::Boolean(_) | Self::Byte(_) => 1,
            Self::Char(_) | Self::Short(_) => 2,
            Self::Float(_) | Self::Int(_) => 4,
            Self::Double(_) | Self::Long(_) => 8,
        }
    }

    pub fn element_align(&self) -> usize {
        self.element_size()
    }

    pub fn element_offset(&self, mode: ObjLayoutMode, index: usize) -> usize {
        let align = self.element_align();
        let base = mode.array_base_offset(self.element_size(), align);
        base + index * self.element_size()
    }

    pub fn object_size_in_bytes(&self, mode: ObjLayoutMode) -> usize {
        mode.array_allocation_size_in_bytes(self.len(), self.element_size(), self.element_align())
    }

    pub fn byte_at(&self, which: usize) -> io::Result<i8> {
        match self {
            Self::Byte(v) => v
                .get(which)
                .copied()
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected byte array")),
        }
    }

    pub fn byte_at_put(&mut self, which: usize, contents: i8) -> io::Result<()> {
        match self {
            Self::Byte(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = contents;
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected byte array")),
        }
    }

    pub fn bool_at(&self, which: usize) -> io::Result<bool> {
        match self {
            Self::Boolean(v) => v
                .get(which)
                .map(|b| *b != 0)
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected boolean array")),
        }
    }

    pub fn bool_at_put(&mut self, which: usize, contents: bool) -> io::Result<()> {
        match self {
            Self::Boolean(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = if contents { 1 } else { 0 };
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected boolean array")),
        }
    }

    pub fn char_at(&self, which: usize) -> io::Result<u16> {
        match self {
            Self::Char(v) => v
                .get(which)
                .copied()
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected char array")),
        }
    }

    pub fn char_at_put(&mut self, which: usize, contents: u16) -> io::Result<()> {
        match self {
            Self::Char(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = contents;
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected char array")),
        }
    }

    pub fn short_at(&self, which: usize) -> io::Result<i16> {
        match self {
            Self::Short(v) => v
                .get(which)
                .copied()
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected short array")),
        }
    }

    pub fn short_at_put(&mut self, which: usize, contents: i16) -> io::Result<()> {
        match self {
            Self::Short(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = contents;
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected short array")),
        }
    }

    pub fn int_at(&self, which: usize) -> io::Result<i32> {
        match self {
            Self::Int(v) => v
                .get(which)
                .copied()
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected int array")),
        }
    }

    pub fn int_at_put(&mut self, which: usize, contents: i32) -> io::Result<()> {
        match self {
            Self::Int(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = contents;
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected int array")),
        }
    }

    pub fn long_at(&self, which: usize) -> io::Result<i64> {
        match self {
            Self::Long(v) => v
                .get(which)
                .copied()
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected long array")),
        }
    }

    pub fn long_at_put(&mut self, which: usize, contents: i64) -> io::Result<()> {
        match self {
            Self::Long(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = contents;
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected long array")),
        }
    }

    pub fn float_at(&self, which: usize) -> io::Result<f32> {
        match self {
            Self::Float(v) => v
                .get(which)
                .copied()
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected float array")),
        }
    }

    pub fn float_at_put(&mut self, which: usize, contents: f32) -> io::Result<()> {
        match self {
            Self::Float(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = contents;
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected float array")),
        }
    }

    pub fn double_at(&self, which: usize) -> io::Result<f64> {
        match self {
            Self::Double(v) => v
                .get(which)
                .copied()
                .ok_or_else(|| Error::other("Array index out of bounds")),
            _ => Err(Error::other("Type mismatch: expected double array")),
        }
    }

    pub fn double_at_put(&mut self, which: usize, contents: f64) -> io::Result<()> {
        match self {
            Self::Double(v) => {
                let elem = v
                    .get_mut(which)
                    .ok_or_else(|| Error::other("Array index out of bounds"))?;
                *elem = contents;
                Ok(())
            }
            _ => Err(Error::other("Type mismatch: expected double array")),
        }
    }
}
