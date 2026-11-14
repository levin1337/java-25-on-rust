#[derive(Debug, Clone)]
pub struct TenuredGeneration {
    pub size: usize,
    pub used: usize,
}

impl TenuredGeneration {
    pub fn new(size: usize) -> Self {
        Self { size, used: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.size
    }

    pub fn used(&self) -> usize {
        self.used
    }
}
