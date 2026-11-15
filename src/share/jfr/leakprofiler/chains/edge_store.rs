

use super::edge::Edge;
use crate::share::jfr::leakprofiler::utilities::root_type::UnifiedOopRef;
use std::collections::HashMap;

#[derive(Default)]
pub struct EdgeStore {
    edges: HashMap<UnifiedOopRef, Vec<Edge>>,
    all: Vec<Edge>,
}

impl EdgeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, edge: Edge) {
        self.edges.entry(edge.from).or_default().push(edge);
        self.all.push(edge);
    }

    pub fn edges_of(&self, from: UnifiedOopRef) -> &[Edge] {
        self.edges.get(&from).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn all(&self) -> &[Edge] {
        &self.all
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn clear(&mut self) {
        self.edges.clear();
        self.all.clear();
    }
}
