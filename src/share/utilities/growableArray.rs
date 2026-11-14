#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrowableArray<E> {
    data: Vec<E>,
}

impl<E> GrowableArray<E> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    pub fn push(&mut self, elem: E) {
        self.data.push(elem);
    }

    pub fn pop(&mut self) -> Option<E> {
        self.data.pop()
    }

    pub fn at(&self, index: usize) -> Option<&E> {
        self.data.get(index)
    }

    pub fn at_mut(&mut self, index: usize) -> Option<&mut E> {
        self.data.get_mut(index)
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn as_slice(&self) -> &[E] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [E] {
        &mut self.data
    }
}

impl<E> Default for GrowableArray<E> {
    fn default() -> Self {
        Self::new()
    }
}
