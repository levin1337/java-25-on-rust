#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AccessFlags {
    pub flags: u16,
}

impl AccessFlags {
    pub const JVM_ACC_PUBLIC: u16 = 0x0001;
    pub const JVM_ACC_PRIVATE: u16 = 0x0002;
    pub const JVM_ACC_PROTECTED: u16 = 0x0004;
    pub const JVM_ACC_STATIC: u16 = 0x0008;
    pub const JVM_ACC_FINAL: u16 = 0x0010;
    pub const JVM_ACC_SYNCHRONIZED: u16 = 0x0020;
    pub const JVM_ACC_SUPER: u16 = 0x0020;
    pub const JVM_ACC_VOLATILE: u16 = 0x0040;
    pub const JVM_ACC_BRIDGE: u16 = 0x0040;
    pub const JVM_ACC_TRANSIENT: u16 = 0x0080;
    pub const JVM_ACC_VARARGS: u16 = 0x0080;
    pub const JVM_ACC_NATIVE: u16 = 0x0100;
    pub const JVM_ACC_INTERFACE: u16 = 0x0200;
    pub const JVM_ACC_ABSTRACT: u16 = 0x0400;
    pub const JVM_ACC_STRICT: u16 = 0x0800;
    pub const JVM_ACC_SYNTHETIC: u16 = 0x1000;
    pub const JVM_ACC_ANNOTATION: u16 = 0x2000;
    pub const JVM_ACC_ENUM: u16 = 0x4000;
    pub const JVM_ACC_MODULE: u16 = 0x8000;

    pub const fn empty() -> Self {
        Self { flags: 0 }
    }

    pub const fn new(flags: u16) -> Self {
        Self { flags }
    }

    pub fn is_public(&self) -> bool {
        (self.flags & Self::JVM_ACC_PUBLIC) != 0
    }

    pub fn is_private(&self) -> bool {
        (self.flags & Self::JVM_ACC_PRIVATE) != 0
    }

    pub fn is_protected(&self) -> bool {
        (self.flags & Self::JVM_ACC_PROTECTED) != 0
    }

    pub fn is_static(&self) -> bool {
        (self.flags & Self::JVM_ACC_STATIC) != 0
    }

    pub fn is_final(&self) -> bool {
        (self.flags & Self::JVM_ACC_FINAL) != 0
    }

    pub fn is_synchronized(&self) -> bool {
        (self.flags & Self::JVM_ACC_SYNCHRONIZED) != 0
    }

    pub fn is_super(&self) -> bool {
        (self.flags & Self::JVM_ACC_SUPER) != 0
    }

    pub fn is_volatile(&self) -> bool {
        (self.flags & Self::JVM_ACC_VOLATILE) != 0
    }

    pub fn is_transient(&self) -> bool {
        (self.flags & Self::JVM_ACC_TRANSIENT) != 0
    }

    pub fn is_native(&self) -> bool {
        (self.flags & Self::JVM_ACC_NATIVE) != 0
    }

    pub fn is_interface(&self) -> bool {
        (self.flags & Self::JVM_ACC_INTERFACE) != 0
    }

    pub fn is_abstract(&self) -> bool {
        (self.flags & Self::JVM_ACC_ABSTRACT) != 0
    }

    pub fn set_is_final(&mut self, val: bool) {
        if val {
            self.flags |= Self::JVM_ACC_FINAL;
        } else {
            self.flags &= !Self::JVM_ACC_FINAL;
        }
    }

    pub fn set_is_abstract(&mut self, val: bool) {
        if val {
            self.flags |= Self::JVM_ACC_ABSTRACT;
        } else {
            self.flags &= !Self::JVM_ACC_ABSTRACT;
        }
    }

    pub fn is_synthetic(&self) -> bool {
        (self.flags & Self::JVM_ACC_SYNTHETIC) != 0
    }

    pub fn is_annotation(&self) -> bool {
        (self.flags & Self::JVM_ACC_ANNOTATION) != 0
    }

    pub fn is_enum(&self) -> bool {
        (self.flags & Self::JVM_ACC_ENUM) != 0
    }
}
