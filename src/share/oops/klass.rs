use crate::share::oops::symbol::Symbol;
use crate::share::utilities::accessFlags::AccessFlags;

#[derive(Debug, Clone)]
pub struct Klass {
    pub name: Symbol,
    pub access_flags: AccessFlags,
}

impl Klass {
    pub fn new(name: Symbol, access_flags: AccessFlags) -> Self {
        Self { name, access_flags }
    }

    pub fn name(&self) -> &Symbol {
        &self.name
    }

    pub fn is_interface(&self) -> bool {
        self.access_flags.is_interface()
    }

    pub fn is_abstract(&self) -> bool {
        self.access_flags.is_abstract()
    }

    pub fn is_final(&self) -> bool {
        self.access_flags.is_final()
    }

    pub fn is_public(&self) -> bool {
        self.access_flags.is_public()
    }
}
