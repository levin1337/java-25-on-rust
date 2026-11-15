

use crate::share::{
    classfile::classLoader::ClassLoader,
    jfr::leakprofiler::{
        chains::path_to_gc_roots_operation,
        utilities::root_type::UnifiedOopRef,
    },
};

pub fn resolve<'a>(
    loader: &ClassLoader<'a>,
    object: UnifiedOopRef,
) -> Option<path_to_gc_roots_operation::PathToRoot> {
    path_to_gc_roots_operation::resolve(loader, object)
}
