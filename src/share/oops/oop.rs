use super::markWord::MarkWord;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oop {
    Null,
    Instance(usize),
    TypeArray(usize),
    ObjArray(usize),
}

impl Oop {
    pub fn is_null(self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn is_instance(self) -> bool {
        matches!(self, Self::Instance(_))
    }

    pub fn is_array(self) -> bool {
        matches!(self, Self::TypeArray(_) | Self::ObjArray(_))
    }
}

pub struct OopDesc {
    pub mark: MarkWord,
}

impl OopDesc {
    pub fn new() -> Self {
        Self {
            mark: MarkWord::prototype(),
        }
    }
}

impl Default for OopDesc {
    fn default() -> Self {
        Self::new()
    }
}
