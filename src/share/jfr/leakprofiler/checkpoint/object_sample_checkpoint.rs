

use crate::share::jfr::leakprofiler::sampling::object_sample::ObjectSample;
use crate::share::jfr::leakprofiler::utilities::root_type::UnifiedOopRef;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct CheckpointItem {
    pub sample: ObjectSample,
    pub referencers: Vec<UnifiedOopRef>,
    pub resolved: bool,
}

#[derive(Default)]
pub struct ObjectSampleCheckpoint {
    items: Vec<CheckpointItem>,
    by_object: HashMap<usize, usize>,
}

impl ObjectSampleCheckpoint {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.by_object.clear();
    }

    pub fn add(&mut self, sample: ObjectSample) {
        if let Some(handle) = sample.object
            && let Some(&index) = self.by_object.get(&handle)
        {
            let item = &mut self.items[index];
            item.sample.span = item.sample.span.saturating_add(sample.span);
            item.sample.allocated = item.sample.allocated.saturating_add(sample.allocated);
            return;
        }
        if let Some(handle) = sample.object {
            self.by_object.insert(handle, self.items.len());
        }
        self.items.push(CheckpointItem {
            sample,
            referencers: Vec::new(),
            resolved: false,
        });
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[CheckpointItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [CheckpointItem] {
        &mut self.items
    }

    pub fn unresolved_count(&self) -> usize {
        self.items.iter().filter(|i| !i.resolved).count()
    }
}

static CHECKPOINT: std::sync::Mutex<Option<ObjectSampleCheckpoint>> = std::sync::Mutex::new(None);

pub fn checkpoint() -> std::sync::MutexGuard<'static, Option<ObjectSampleCheckpoint>> {
    CHECKPOINT.lock().unwrap()
}
