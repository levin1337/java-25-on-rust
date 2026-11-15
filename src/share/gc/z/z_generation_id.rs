#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ZGenerationId {
    Young = 0,
    Old = 1,
}

impl ZGenerationId {
    pub fn other(self) -> Self {
        match self {
            Self::Young => Self::Old,
            Self::Old => Self::Young,
        }
    }

    pub fn is_young(self) -> bool {
        self == Self::Young
    }

    pub fn is_old(self) -> bool {
        self == Self::Old
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZGenerationIdOptional {
    Young,
    Old,
    None,
}

impl From<ZGenerationId> for ZGenerationIdOptional {
    fn from(id: ZGenerationId) -> Self {
        match id {
            ZGenerationId::Young => Self::Young,
            ZGenerationId::Old => Self::Old,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZPageType {
    Small,
    Medium,
    Large,
}

impl ZPageType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ZPageAge {
    Eden = 0,
    Survivor1 = 1,
    Survivor2 = 2,
    Survivor3 = 3,
    Survivor4 = 4,
    Survivor5 = 5,
    Survivor6 = 6,
    Survivor7 = 7,
    Survivor8 = 8,
    Survivor9 = 9,
    Survivor10 = 10,
    Survivor11 = 11,
    Survivor12 = 12,
    Survivor13 = 13,
    Survivor14 = 14,
    Old = 15,
}

impl ZPageAge {
    pub const MAX: usize = Self::Old as usize;

    pub fn next(self) -> Option<Self> {
        let value = self as u8;
        if value >= Self::Old as u8 {
            None
        } else {
            Some(unsafe { std::mem::transmute(value + 1) })
        }
    }

    pub fn is_old(self) -> bool {
        self == Self::Old
    }

    pub fn generation_id(self) -> ZGenerationId {
        if self.is_old() { ZGenerationId::Old } else { ZGenerationId::Young }
    }
}
