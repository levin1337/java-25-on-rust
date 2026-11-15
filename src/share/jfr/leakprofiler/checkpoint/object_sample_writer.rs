

use super::{
    object_sample_checkpoint::ObjectSampleCheckpoint,
    object_sample_description::ObjectSampleDescription,
};
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::jfr::leakprofiler::{
    chains::{edge_utils, path_to_gc_roots_operation},
    utilities::root_type::UnifiedOopRef,
};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct OldObjectSampleEvent {
    pub description: ObjectSampleDescription,
    pub sample_index: usize,
}

static EMITTED: Mutex<Vec<OldObjectSampleEvent>> = Mutex::new(Vec::new());

pub fn emitted_events() -> std::sync::MutexGuard<'static, Vec<OldObjectSampleEvent>> {
    EMITTED.lock().unwrap()
}

pub fn clear_emitted() {
    EMITTED.lock().unwrap().clear();
}

pub fn write_samples<'a>(
    loader: &ClassLoader<'a>,
    checkpoint: &mut ObjectSampleCheckpoint,
    cutoff_ticks: i64,
    emit_all: bool,
    skip_bfs: bool,
) -> usize {
    let mut emitted = 0usize;
    let mut events = Vec::new();
    for (index, item) in checkpoint.items_mut().iter_mut().enumerate() {
        let sample = item.sample.clone();
        if !emit_all && sample.allocation_nanos > cutoff_ticks {
            continue;
        }
        if item.resolved {
            continue;
        }
        let object = sample.object.map(|handle| UnifiedOopRef::Object(handle));
        let path = object.and_then(|reference| {
            if skip_bfs {
                None
            } else {
                path_to_gc_roots_operation::resolve(loader, reference)
            }
        });
        let size = object.map(|r| edge_utils::object_size(loader, r)).unwrap_or(0);
        let class_name = object.and_then(|r| object_class_name(loader, r));
        let description = ObjectSampleDescription::from_sample(
            &sample,
            size,
            class_name,
            path,
            0,
        );
        events.push(OldObjectSampleEvent {
            description,
            sample_index: index,
        });
        item.resolved = true;
        emitted += 1;
    }
    EMITTED.lock().unwrap().extend(events);
    emitted
}

fn object_class_name(loader: &ClassLoader<'_>, object: UnifiedOopRef) -> Option<String> {
    match object {
        UnifiedOopRef::Object(handle) => {
            let objects = loader.borrow_objects();
            objects
                .get(handle)
                .map(|o| String::from_utf8_lossy(o.klass.class.name).replace('/', "."))
        }
        UnifiedOopRef::ObjectArray(handle) => {
            let arrays = loader.obj_arrays.borrow();
            arrays.get(handle).map(|a| {
                format!(
                    "{}[]",
                    String::from_utf8_lossy(a.component).replace('/', ".")
                )
            })
        }
        _ => None,
    }
}
