#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GCWhen {
    BeforeGC,
    AfterGC,
}
