

#[derive(Clone, Debug, Default)]
pub struct JfrBitSet {
    words: Vec<u64>,
    len: usize,
}

impl JfrBitSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(bits: usize) -> Self {
        let words = bits.div_ceil(64);
        Self {
            words: vec![0; words],
            len: bits,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn clear(&mut self) {
        self.words.fill(0);
    }

    fn ensure(&mut self, bit: usize) {
        if bit >= self.len {
            let words = (bit + 1).div_ceil(64);
            if words > self.words.len() {
                self.words.resize(words, 0);
            }
            self.len = bit + 1;
        }
    }

    pub fn set(&mut self, bit: usize) {
        self.ensure(bit);
        self.words[bit / 64] |= 1u64 << (bit % 64);
    }

    pub fn get(&self, bit: usize) -> bool {
        self.words
            .get(bit / 64)
            .is_some_and(|word| (word >> (bit % 64)) & 1 != 0)
    }

    pub fn clear_bit(&mut self, bit: usize) {
        if let Some(word) = self.words.get_mut(bit / 64) {
            *word &= !(1u64 << (bit % 64));
        }
    }
}
