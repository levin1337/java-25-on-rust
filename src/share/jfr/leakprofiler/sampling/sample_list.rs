

use super::object_sample::ObjectSample;

pub struct SampleList {
    samples: Vec<Option<ObjectSample>>,
    free: Vec<usize>,
    last_resolved: Option<usize>,
    allocated: usize,
    limit: usize,
    cache_size: usize,
}

impl SampleList {
    pub fn new(limit: usize, cache_size: usize) -> Self {
        Self {
            samples: Vec::with_capacity(limit),
            free: Vec::with_capacity(cache_size.min(limit)),
            last_resolved: None,
            allocated: 0,
            limit,
            cache_size,
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn len(&self) -> usize {
        self.samples.iter().filter(|s| s.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&mut self) -> Option<usize> {
        if let Some(index) = self.free.pop() {
            if self.samples.get(index).is_some_and(|s| s.is_none()) {
                *self.samples.get_mut(index).unwrap() = Some(ObjectSample::new());
                self.allocated += 1;
                return Some(index);
            }
        }
        if self.len() >= self.limit {
            return None;
        }
        self.samples.push(Some(ObjectSample::new()));
        self.allocated += 1;
        Some(self.samples.len() - 1)
    }

    pub fn reuse(&mut self, sample_index: usize) -> Option<usize> {
        if sample_index >= self.samples.len() {
            return None;
        }
        if self.free.len() < self.cache_size.max(1) {
            self.free.push(sample_index);
        }
        self.get()
    }

    pub fn release(&mut self, index: usize) {
        if index < self.samples.len() {
            self.samples[index] = None;
            self.allocated = self.allocated.saturating_sub(1);
            if self.free.len() < self.cache_size.max(1) {
                self.free.push(index);
            }
        }
    }

    pub fn sample(&self, index: usize) -> Option<&ObjectSample> {
        self.samples.get(index)?.as_ref()
    }

    pub fn sample_mut(&mut self, index: usize) -> Option<&mut ObjectSample> {
        self.samples.get_mut(index)?.as_mut()
    }

    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.samples
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|_| i))
    }

    pub fn set_last_resolved(&mut self, index: Option<usize>) {
        self.last_resolved = index;
    }

    pub fn last_resolved(&self) -> Option<usize> {
        self.last_resolved
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.free.clear();
        self.last_resolved = None;
        self.allocated = 0;
    }
}
