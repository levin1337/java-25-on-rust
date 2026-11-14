#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ConstantTag {
    Invalid = 0,
    Utf8 = 1,
    Unicode = 2,
    Integer = 3,
    Float = 4,
    Long = 5,
    Double = 6,
    Class = 7,
    String = 8,
    Fieldref = 9,
    Methodref = 10,
    InterfaceMethodref = 11,
    NameAndType = 12,
    MethodHandle = 15,
    MethodType = 16,
    Dynamic = 17,
    InvokeDynamic = 18,
    Module = 19,
    Package = 20,
}

impl ConstantTag {
    pub fn from_u8(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Invalid),
            1 => Some(Self::Utf8),
            2 => Some(Self::Unicode),
            3 => Some(Self::Integer),
            4 => Some(Self::Float),
            5 => Some(Self::Long),
            6 => Some(Self::Double),
            7 => Some(Self::Class),
            8 => Some(Self::String),
            9 => Some(Self::Fieldref),
            10 => Some(Self::Methodref),
            11 => Some(Self::InterfaceMethodref),
            12 => Some(Self::NameAndType),
            15 => Some(Self::MethodHandle),
            16 => Some(Self::MethodType),
            17 => Some(Self::Dynamic),
            18 => Some(Self::InvokeDynamic),
            19 => Some(Self::Module),
            20 => Some(Self::Package),
            _ => None,
        }
    }

    pub fn is_klass(&self) -> bool {
        matches!(self, Self::Class)
    }

    pub fn is_field(&self) -> bool {
        matches!(self, Self::Fieldref)
    }

    pub fn is_method(&self) -> bool {
        matches!(self, Self::Methodref | Self::InterfaceMethodref)
    }

    pub fn is_utf8(&self) -> bool {
        matches!(self, Self::Utf8)
    }

    pub fn is_loadable(&self) -> bool {
        matches!(
            self,
            Self::Integer
                | Self::Float
                | Self::Long
                | Self::Double
                | Self::Class
                | Self::String
                | Self::MethodHandle
                | Self::MethodType
                | Self::Dynamic
        )
    }
}
