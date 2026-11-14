#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackOverflow {
    limit: usize,
    base: usize,
}

impl StackOverflow {
    pub fn new(base: usize, size: usize, reserved: usize) -> Option<Self> {
        let usable = size.checked_sub(reserved)?;
        Some(Self {
            limit: base.checked_sub(usable)?,
            base,
        })
    }

    pub fn fits(self, pointer: usize, frameBytes: usize) -> bool {
        pointer <= self.base
            && pointer
                .checked_sub(frameBytes)
                .is_some_and(|start| start >= self.limit)
    }
}
