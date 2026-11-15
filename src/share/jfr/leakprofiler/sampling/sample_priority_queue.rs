

use std::collections::BinaryHeap;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct QueueEntry {
    span: usize,
    sequence: u64,
    index: usize,
}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {

        other
            .span
            .cmp(&self.span)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct SamplePriorityQueue {
    heap: BinaryHeap<QueueEntry>,
    positions: Vec<Option<usize>>,
    spans: Vec<usize>,
    total: usize,
    sequence: u64,
}

impl SamplePriorityQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(capacity),
            positions: vec![None; capacity],
            spans: vec![0; capacity],
            total: 0,
            sequence: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn total(&self) -> usize {
        self.total
    }

    fn ensure_capacity(&mut self, index: usize) {
        if index >= self.positions.len() {
            self.positions.resize(index + 1, None);
            self.spans.resize(index + 1, 0);
        }
    }

    pub fn push(&mut self, index: usize, span: usize) {
        self.ensure_capacity(index);
        if self.positions[index].is_some() {
            self.remove(index);
        }
        self.sequence += 1;
        self.heap.push(QueueEntry {
            span,
            sequence: self.sequence,
            index,
        });
        self.positions[index] = Some(self.heap.len() - 1);
        self.spans[index] = span;
        self.total = self.total.saturating_add(span);
    }

    pub fn remove(&mut self, index: usize) -> Option<usize> {
        self.ensure_capacity(index);
        let span = self.spans[index];
        if self.positions[index].is_none() {
            return None;
        }
        self.heap.retain(|entry| entry.index != index);
        self.positions[index] = None;
        self.spans[index] = 0;
        self.total = self.total.saturating_sub(span);
        Some(span)
    }

    pub fn peek(&self) -> Option<usize> {
        self.heap.peek().map(|entry| entry.index)
    }

    pub fn pop(&mut self) -> Option<(usize, usize)> {
        let entry = self.heap.pop()?;
        self.positions[entry.index] = None;
        self.spans[entry.index] = 0;
        self.total = self.total.saturating_sub(entry.span);
        Some((entry.index, entry.span))
    }

    pub fn contains(&self, index: usize) -> bool {
        self.positions.get(index).is_some_and(|p| p.is_some())
    }

    pub fn span_of(&self, index: usize) -> usize {
        self.spans.get(index).copied().unwrap_or(0)
    }

    pub fn clear(&mut self) {
        self.heap.clear();
        self.positions.fill(None);
        self.spans.fill(0);
        self.total = 0;
    }
}
