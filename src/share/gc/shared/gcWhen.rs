#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GCWhen {
    BeforeGC,
    AfterGC,
    Concurrent,
}

impl GCWhen {
    pub fn is_concurrent(&self) -> bool {
        matches!(self, Self::Concurrent)
    }
}
