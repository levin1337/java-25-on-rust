

use super::{edge::Edge, edge_utils, jfrbitset::JfrBitSet, root_set_closure};
use crate::share::{
    classfile::classLoader::ClassLoader,
    jfr::leakprofiler::utilities::root_type::UnifiedOopRef,
};

#[derive(Clone, Debug, Default)]
pub struct DfsResult {
    pub path: Vec<Edge>,
    pub root: Option<root_set_closure::Root>,
    pub visited: usize,
}

fn bit(object: UnifiedOopRef) -> usize {
    match object {
        UnifiedOopRef::Object(h)
        | UnifiedOopRef::ObjectArray(h)
        | UnifiedOopRef::TypeArray(h) => h,
        UnifiedOopRef::Null => usize::MAX,
    }
}

pub fn dfs_to_root<'a>(
    loader: &ClassLoader<'a>,
    start: UnifiedOopRef,
    max_depth: usize,
    max_visited: usize,
) -> DfsResult {
    let roots = root_set_closure::gc_roots(loader);
    let mut result = DfsResult::default();
    let mut visited = JfrBitSet::new();
    dfs(loader, start, 0, max_depth, max_visited, &roots, &mut visited, &mut Vec::new(), &mut result);
    result
}

fn dfs<'a>(
    loader: &ClassLoader<'a>,
    object: UnifiedOopRef,
    depth: usize,
    max_depth: usize,
    max_visited: usize,
    roots: &[root_set_closure::Root],
    visited: &mut JfrBitSet,
    stack: &mut Vec<Edge>,
    result: &mut DfsResult,
) {
    if result.root.is_some() || depth > max_depth || result.visited >= max_visited {
        return;
    }
    let key = bit(object);
    if key == usize::MAX || visited.get(key) {
        return;
    }
    visited.set(key);
    result.visited += 1;

    if let Some(root) = roots.iter().find(|r| r.object == object) {
        result.root = Some(*root);
        result.path = stack.clone();
        return;
    }

    for target in edge_utils::outgoing_edges(loader, object) {
        stack.push(Edge::new(object, target));
        dfs(loader, target, depth + 1, max_depth, max_visited, roots, visited, stack, result);
        stack.pop();
        if result.root.is_some() {
            return;
        }
    }
}
