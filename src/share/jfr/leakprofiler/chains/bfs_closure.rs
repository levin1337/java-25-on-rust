

use super::{
    edge::Edge,
    edge_utils,
    jfrbitset::JfrBitSet,
    root_set_closure::{self, Root},
};
use crate::share::{
    classfile::classLoader::ClassLoader,
    jfr::leakprofiler::utilities::root_type::UnifiedOopRef,
};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, Default)]
pub struct BfsResult {
    pub path: Vec<Edge>,
    pub root: Option<Root>,
    pub visited: usize,
}

pub fn bfs_to_root<'a>(
    loader: &ClassLoader<'a>,
    start: UnifiedOopRef,
    max_depth: usize,
    max_visited: usize,
) -> BfsResult {
    let roots = root_set_closure::gc_roots(loader);
    let mut result = BfsResult::default();
    if root_set_closure::is_root(start, &roots) {
        if let Some(root) = roots.iter().find(|r| r.object == start) {
            result.root = Some(*root);
        }
        return result;
    }

    let mut visited = JfrBitSet::new();
    let mut queue: VecDeque<(UnifiedOopRef, usize)> = VecDeque::new();
    let mut parents: HashMap<UnifiedOopRef, UnifiedOopRef> = HashMap::new();
    queue.push_back((start, 0));

    while let Some((object, depth)) = queue.pop_front() {
        if depth > max_depth || result.visited >= max_visited {
            break;
        }
        let key = match object {
            UnifiedOopRef::Object(h) | UnifiedOopRef::ObjectArray(h) | UnifiedOopRef::TypeArray(h) => h,
            UnifiedOopRef::Null => continue,
        };
        visited.set(key);
        result.visited += 1;

        for target in edge_utils::outgoing_edges(loader, object) {
            if let Some(root) = roots.iter().find(|r| r.object == target) {
                parents.insert(target, object);
                let mut path = Vec::new();
                let mut cursor = target;
                while let Some(&holder) = parents.get(&cursor) {
                    path.push(Edge::new(holder, cursor));
                    if holder == start {
                        break;
                    }
                    cursor = holder;
                }
                path.reverse();
                result.path = path;
                result.root = Some(*root);
                return result;
            }
            if visited.get(match target {
                UnifiedOopRef::Object(h)
                | UnifiedOopRef::ObjectArray(h)
                | UnifiedOopRef::TypeArray(h) => h,
                UnifiedOopRef::Null => usize::MAX,
            }) {
                continue;
            }
            parents.entry(target).or_insert(object);
            queue.push_back((target, depth + 1));
        }
    }
    result
}
