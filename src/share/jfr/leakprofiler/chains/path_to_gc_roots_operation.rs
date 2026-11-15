

use super::{bfs_closure, dfs_closure, edge::Edge, root_set_closure};
use crate::share::{
    classfile::classLoader::ClassLoader,
    jfr::leakprofiler::utilities::root_type::UnifiedOopRef,
};

pub const MAX_DEPTH: usize = 32;
pub const MAX_VISITED: usize = 10_000;

#[derive(Clone, Debug)]
pub struct PathToRoot {
    pub edges: Vec<Edge>,
    pub root: root_set_closure::Root,
}

pub fn resolve<'a>(
    loader: &ClassLoader<'a>,
    object: UnifiedOopRef,
) -> Option<PathToRoot> {
    let bfs = bfs_closure::bfs_to_root(loader, object, MAX_DEPTH, MAX_VISITED);
    if let Some(root) = bfs.root {
        return Some(PathToRoot {
            edges: bfs.path,
            root,
        });
    }
    let dfs = dfs_closure::dfs_to_root(loader, object, MAX_DEPTH.saturating_mul(4), MAX_VISITED);
    dfs.root.map(|root| PathToRoot {
        edges: dfs.path,
        root,
    })
}
