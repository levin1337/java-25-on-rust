#[derive(Debug, Clone)]
pub struct DefNewGeneration {
    pub eden_size: usize,
    pub eden_used: usize,
    pub survivor_size: usize,
    pub survivor_used: usize,
}

impl DefNewGeneration {
    pub fn new(eden_size: usize, survivor_size: usize) -> Self {
        Self {
            eden_size,
            eden_used: 0,
            survivor_size,
            survivor_used: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.eden_size + self.survivor_size * 2
    }

    pub fn used(&self) -> usize {
        self.eden_used + self.survivor_used
    }

    pub fn reset_young(&mut self) {
        self.eden_used = 0;
        self.survivor_used = 0;
    }
}
