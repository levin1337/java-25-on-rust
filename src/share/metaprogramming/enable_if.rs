pub trait EnableIf<const COND: bool> {}

impl EnableIf<true> for () {}

pub trait Conditional<T> {
    type Output;
}

pub struct EnableIfValue<T, const COND: bool> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> EnableIfValue<T, true> {
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> Default for EnableIfValue<T, true> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn is_enabled<const COND: bool>() -> bool {
    COND
}
