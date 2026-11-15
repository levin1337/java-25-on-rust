

use crate::share::jfr::leakprofiler::utilities::root_type::UnifiedOopRef;
use std::collections::HashSet;
use std::sync::Mutex;

static MARKED: Mutex<Option<HashSet<UnifiedOopRef>>> = Mutex::new(None);

fn marked() -> std::sync::MutexGuard<'static, Option<HashSet<UnifiedOopRef>>> {
    MARKED.lock().unwrap()
}

pub fn mark(object: UnifiedOopRef) {
    marked().get_or_insert_with(HashSet::new).insert(object);
}

pub fn clear() {
    if let Some(set) = marked().as_mut() {
        set.clear();
    }
}

pub fn is_marked(object: UnifiedOopRef) -> bool {
    marked()
        .as_ref()
        .is_some_and(|set| set.contains(&object))
}
