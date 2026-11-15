

use super::edge::Edge;
use std::collections::VecDeque;

#[derive(Default)]
pub struct EdgeQueue {
    queue: VecDeque<Edge>,
}

impl EdgeQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, edge: Edge) {
        self.queue.push_back(edge);
    }

    pub fn pop(&mut self) -> Option<Edge> {
        self.queue.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}
