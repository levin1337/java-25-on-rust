/*
 * Port of OpenJDK 25 HotSpot shenandoahGenerationType.hpp
 * (share/gc/shenandoah/shenandoahGenerationType.hpp)
 */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShenandoahGenerationType {
    NonGen,
    Global,
    Young,
    Old,
}

impl ShenandoahGenerationType {
    pub fn name(self) -> &'static str {
        match self {
            Self::NonGen => "Non-Generational",
            Self::Global => "Global",
            Self::Old => "Old",
            Self::Young => "Young",
        }
    }

    pub fn is_young(self) -> bool {
        self == Self::Young
    }

    pub fn is_old(self) -> bool {
        self == Self::Old
    }

    pub fn is_global(self) -> bool {
        self == Self::Global || self == Self::NonGen
    }
}
