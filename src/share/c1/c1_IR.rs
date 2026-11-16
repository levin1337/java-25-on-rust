// C1 Intermediate Representation ported from HotSpot c1_IR.hpp / .cpp

use super::c1_Instruction::{BlockBegin, HirInstruction};
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub struct BasicBlock {
    pub id: usize,
    pub begin: BlockBegin,
    pub instructions: Vec<HirInstruction>,
    pub successors: Vec<usize>,
}

impl BasicBlock {
    pub fn new(id: usize) -> Self {
        Self::with_bci(id, 0)
    }

    pub fn with_bci(id: usize, bci: usize) -> Self {
        Self {
            id,
            begin: BlockBegin::new(id, bci),
            instructions: Vec::new(),
            successors: Vec::new(),
        }
    }

    pub fn push(&mut self, instr: HirInstruction) {
        if instr.is_block_end() {
            self.successors = instr.successors();
            self.begin.successors = self.successors.clone();
        }
        self.instructions.push(instr);
    }

    pub fn set_end(&mut self, end_instr: HirInstruction) {
        if let Some(pos) = self.instructions.iter().rposition(|i| i.is_block_end()) {
            self.instructions[pos] = end_instr;
        } else {
            self.instructions.push(end_instr);
        }
        if let Some(last) = self.instructions.last() {
            self.successors = last.successors();
            self.begin.successors = self.successors.clone();
        }
    }

    pub fn end(&self) -> Option<&HirInstruction> {
        self.instructions.iter().rev().find(|i| i.is_block_end())
    }

    pub fn end_mut(&mut self) -> Option<&mut HirInstruction> {
        self.instructions
            .iter_mut()
            .rev()
            .find(|i| i.is_block_end())
    }

    pub fn is_terminated(&self) -> bool {
        self.end().is_some()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ControlFlowGraph {
    pub blocks: Vec<BasicBlock>,
    pub entry_block: usize,
    pub osr_entry_block: Option<usize>,
    pub max_locals: usize,
    pub max_stack_entries: usize,
}

impl ControlFlowGraph {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            entry_block: 0,
            osr_entry_block: None,
            max_locals: 0,
            max_stack_entries: 0,
        }
    }

    pub fn add_block(&mut self) -> usize {
        let id = self.blocks.len();
        self.blocks.push(BasicBlock::new(id));
        id
    }

    pub fn add_block_with_bci(&mut self, bci: usize) -> usize {
        let id = self.blocks.len();
        self.blocks.push(BasicBlock::with_bci(id, bci));
        id
    }

    pub fn block(&self, id: usize) -> Option<&BasicBlock> {
        self.blocks.get(id)
    }

    pub fn block_mut(&mut self, id: usize) -> Option<&mut BasicBlock> {
        self.blocks.get_mut(id)
    }

    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    pub fn add_edge(&mut self, from: usize, to: usize) {
        if let Some(b) = self.blocks.get_mut(from) {
            if !b.successors.contains(&to) {
                b.successors.push(to);
                b.begin.add_successor(to);
            }
        }
        if let Some(b) = self.blocks.get_mut(to) {
            b.begin.add_predecessor(from);
        }
    }

    pub fn compute_predecessors(&mut self) {
        for b in &mut self.blocks {
            b.begin.predecessors.clear();
        }

        let n = self.blocks.len();
        for i in 0..n {
            let sux = self.blocks[i].successors.clone();
            for to in sux {
                if let Some(target) = self.blocks.get_mut(to) {
                    target.begin.add_predecessor(i);
                }
            }
        }
    }

    pub fn preorder(&self) -> Vec<usize> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        self.dfs_preorder(self.entry_block, &mut visited, &mut order);
        order
    }

    fn dfs_preorder(&self, block_id: usize, visited: &mut HashSet<usize>, order: &mut Vec<usize>) {
        if !visited.insert(block_id) {
            return;
        }
        order.push(block_id);
        if let Some(b) = self.block(block_id) {
            for &sux in &b.successors {
                self.dfs_preorder(sux, visited, order);
            }
        }
    }

    pub fn postorder(&self) -> Vec<usize> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        self.dfs_postorder(self.entry_block, &mut visited, &mut order);
        order
    }

    fn dfs_postorder(&self, block_id: usize, visited: &mut HashSet<usize>, order: &mut Vec<usize>) {
        if !visited.insert(block_id) {
            return;
        }
        if let Some(b) = self.block(block_id) {
            for &sux in &b.successors {
                self.dfs_postorder(sux, visited, order);
            }
        }
        order.push(block_id);
    }

    pub fn reverse_postorder(&self) -> Vec<usize> {
        let mut order = self.postorder();
        order.reverse();
        order
    }

    pub fn eliminate_dead_blocks(&mut self) {
        if self.blocks.is_empty() {
            return;
        }

        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(self.entry_block);
        reachable.insert(self.entry_block);

        if let Some(osr) = self.osr_entry_block {
            queue.push_back(osr);
            reachable.insert(osr);
        }

        while let Some(curr) = queue.pop_front() {
            if let Some(b) = self.block(curr) {
                for &sux in &b.successors {
                    if reachable.insert(sux) {
                        queue.push_back(sux);
                    }
                }
            }
        }

        if reachable.len() == self.blocks.len() {
            return;
        }

        let mut new_id = vec![None; self.blocks.len()];
        let mut compacted = Vec::with_capacity(reachable.len());

        for (old_idx, block) in self.blocks.iter().enumerate() {
            if reachable.contains(&old_idx) {
                new_id[old_idx] = Some(compacted.len());
                let mut new_b = block.clone();
                new_b.id = compacted.len();
                new_b.begin.block_id = compacted.len();
                compacted.push(new_b);
            }
        }

        for block in &mut compacted {
            let old_sux = block.successors.clone();
            block.successors = old_sux
                .into_iter()
                .filter_map(|s| new_id.get(s).copied().flatten())
                .collect();
            block.begin.successors = block.successors.clone();

            for instr in &mut block.instructions {
                if instr.is_block_end() {
                    let mut oldTargets = instr.successors();
                    oldTargets.sort_unstable();
                    for old_target in oldTargets {
                        if let Some(new_target) = new_id.get(old_target).copied().flatten() {
                            instr.substitute_successor(old_target, new_target);
                        }
                    }
                }
            }
        }

        self.blocks = compacted;
        self.entry_block = new_id[self.entry_block].unwrap_or(0);
        self.osr_entry_block = self
            .osr_entry_block
            .and_then(|o| new_id.get(o).copied().flatten());
        self.compute_predecessors();
    }
}
