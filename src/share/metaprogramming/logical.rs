pub trait BoolConstant {
    const VALUE: bool;
}

pub struct True;
pub struct False;

impl BoolConstant for True {
    const VALUE: bool = true;
}

impl BoolConstant for False {
    const VALUE: bool = false;
}

pub const fn negation<const VALUE: bool>() -> bool {
    !VALUE
}

pub fn conjunction(values: &[bool]) -> bool {
    values.iter().all(|v| *v)
}

pub fn disjunction(values: &[bool]) -> bool {
    values.iter().any(|v| *v)
}

pub const fn conjunction_const<const N: usize>(values: [bool; N]) -> bool {
    let mut i = 0;
    while i < N {
        if !values[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub const fn disjunction_const<const N: usize>(values: [bool; N]) -> bool {
    let mut i = 0;
    while i < N {
        if values[i] {
            return true;
        }
        i += 1;
    }
    false
}

pub fn negation_of<T: BoolConstant>() -> bool {
    !T::VALUE
}
