

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    jfr::leakprofiler::utilities::root_type::{RootType, UnifiedOopRef},
    runtime::threading,
};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Root {
    pub object: UnifiedOopRef,
    pub root_type: RootType,
}

pub fn gc_roots<'a>(loader: &ClassLoader<'a>) -> Vec<Root> {
    let mut roots: Vec<Root> = Vec::new();
    let mut seen: HashSet<UnifiedOopRef> = HashSet::new();

    let mut push = |object: UnifiedOopRef, root_type: RootType| {
        if !object.is_null() && seen.insert(object) {
            roots.push(Root { object, root_type });
        }
    };

    for value in loader.gc_extra_roots.borrow().iter() {
        push(UnifiedOopRef::from(*value), RootType::JvmtiEnv);
    }

    let names: Vec<&'a [u8]> = {
        let classes = loader.classes.borrow();
        classes.keys().copied().collect()
    };
    for name in names {
        let Ok(klass) = loader.load(name) else {
            continue;
        };
        for cell in &klass.static_fields {
            let value = cell.get();
            push(UnifiedOopRef::from(value), RootType::ClassStatic);
        }
    }

    if let Some(loader_arc) = threading::jvm_loader() {
        let _ = loader_arc;
    }

    roots
}

pub fn is_root(object: UnifiedOopRef, roots: &[Root]) -> bool {
    roots.iter().any(|root| root.object == object)
}
