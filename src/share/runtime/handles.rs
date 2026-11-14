use crate::share::oops::oop::Oop;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    oop: Oop,
}

impl Handle {
    pub fn new(oop: Oop) -> Self {
        Self { oop }
    }

    pub fn resolve(&self) -> Oop {
        self.oop
    }

    pub fn is_null(&self) -> bool {
        self.oop.is_null()
    }
}
