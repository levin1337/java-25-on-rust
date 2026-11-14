// C1 Graph Builder ported from HotSpot c1_GraphBuilder.hpp / .cpp
// Builds HIR ControlFlowGraph with BlockBegin and BlockEnd from Java bytecode.

use super::c1_IR::{BasicBlock, ControlFlowGraph};
use super::c1_Instruction::{BlockBegin, Condition, HirInstruction};
use std::collections::{BTreeMap, BTreeSet};

pub struct GraphBuilder<'a> {
    pub code: &'a [u8],
    pub max_locals: usize,
}

impl<'a> GraphBuilder<'a> {
    pub fn new(code: &'a [u8], max_locals: usize) -> Self {
        Self { code, max_locals }
    }

    pub fn max_locals(&self) -> usize {
        self.max_locals
    }

    pub fn build(code: &'a [u8], max_locals: usize) -> ControlFlowGraph {
        let builder = GraphBuilder::new(code, max_locals);
        builder.build_cfg()
    }

    pub fn build_cfg(&self) -> ControlFlowGraph {
        let mut cfg = ControlFlowGraph::new();
        if self.code.is_empty() {
            return cfg;
        }

        // Pass 1: Identify basic block leaders (start BCIs)
        let leaders = self.identify_leaders();
        let bci_to_block_id: BTreeMap<usize, usize> = leaders
            .iter()
            .enumerate()
            .map(|(id, &bci)| (bci, id))
            .collect();

        // Create basic blocks in CFG
        for (id, &bci) in leaders.iter().enumerate() {
            let mut block = BasicBlock::with_bci(id, bci);
            block.begin = BlockBegin::new(id, bci);
            cfg.blocks.push(block);
        }

        // Pass 2: Translate bytecodes into HIR instructions for each block
        let num_leaders = leaders.len();
        for i in 0..num_leaders {
            let start_bci = leaders[i];
            let end_bci = if i + 1 < num_leaders {
                leaders[i + 1]
            } else {
                self.code.len()
            };

            self.build_block(&mut cfg, i, start_bci, end_bci, &bci_to_block_id);
        }

        cfg.compute_predecessors();
        cfg
    }

    fn identify_leaders(&self) -> Vec<usize> {
        let mut leaders = BTreeSet::new();
        leaders.insert(0);

        let mut bci = 0;
        while bci < self.code.len() {
            let op = self.code[bci];
            let next_bci = self.next_bci(bci);

            match op {
                // Unconditional jumps
                0xa7 => {
                    // goto
                    if bci + 2 < self.code.len() {
                        let offset =
                            i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
                        let target = (bci as i32 + offset).max(0) as usize;
                        if target < self.code.len() {
                            leaders.insert(target);
                        }
                    }
                    if next_bci < self.code.len() {
                        leaders.insert(next_bci);
                    }
                }
                0xc8 => {
                    // goto_w
                    if bci + 4 < self.code.len() {
                        let offset = i32::from_be_bytes([
                            self.code[bci + 1],
                            self.code[bci + 2],
                            self.code[bci + 3],
                            self.code[bci + 4],
                        ]);
                        let target = (bci as i32 + offset).max(0) as usize;
                        if target < self.code.len() {
                            leaders.insert(target);
                        }
                    }
                    if next_bci < self.code.len() {
                        leaders.insert(next_bci);
                    }
                }
                // Conditional branches
                0x99..=0xa6 | 0xc6 | 0xc7 => {
                    if bci + 2 < self.code.len() {
                        let offset =
                            i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
                        let target = (bci as i32 + offset).max(0) as usize;
                        if target < self.code.len() {
                            leaders.insert(target);
                        }
                    }
                    if next_bci < self.code.len() {
                        leaders.insert(next_bci);
                    }
                }
                // Returns and throw
                0xac..=0xb1 | 0xbf => {
                    if next_bci < self.code.len() {
                        leaders.insert(next_bci);
                    }
                }
                _ => {}
            }

            bci = next_bci;
        }

        leaders.into_iter().collect()
    }

    fn next_bci(&self, bci: usize) -> usize {
        if bci >= self.code.len() {
            return self.code.len();
        }
        let op = self.code[bci];
        let len = match op {
            0x00..=0x0f
            | 0x1a..=0x35
            | 0x3b..=0x56
            | 0x57..=0x83
            | 0x85..=0x98
            | 0xac..=0xb1
            | 0xbe
            | 0xbf
            | 0xc2
            | 0xc3 => 1,
            0x10 | 0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xbc => 2,
            0x11
            | 0x13
            | 0x14
            | 0x84
            | 0x99..=0xa8
            | 0xb2..=0xb8
            | 0xbb
            | 0xbd
            | 0xc0
            | 0xc1
            | 0xc6
            | 0xc7 => 3,
            0xb9 | 0xba | 0xc8 => 5,
            _ => 1,
        };
        (bci + len).min(self.code.len())
    }

    fn build_block(
        &self,
        cfg: &mut ControlFlowGraph,
        block_id: usize,
        start_bci: usize,
        end_bci: usize,
        bci_to_block_id: &BTreeMap<usize, usize>,
    ) {
        let mut stack: Vec<HirInstruction> = Vec::new();
        let mut bci = start_bci;
        let mut terminated = false;

        while bci < end_bci && !terminated {
            let op = self.code[bci];
            let next = self.next_bci(bci);

            match op {
                // nop
                0x00 => {}

                // aconst_null
                0x01 => stack.push(HirInstruction::ConstantNull),

                // iconst_m1..=iconst_5
                0x02..=0x08 => {
                    let val = (op as i32) - 0x03;
                    stack.push(HirInstruction::ConstantInt(val));
                }

                // lconst_0, lconst_1
                0x09 | 0x0a => {
                    let val = (op as i64) - 0x09;
                    stack.push(HirInstruction::ConstantLong(val));
                }

                // bipush
                0x10 => {
                    if bci + 1 < self.code.len() {
                        let val = self.code[bci + 1] as i8 as i32;
                        stack.push(HirInstruction::ConstantInt(val));
                    }
                }

                // sipush
                0x11 => {
                    if bci + 2 < self.code.len() {
                        let val =
                            i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
                        stack.push(HirInstruction::ConstantInt(val));
                    }
                }

                // iload
                0x15 => {
                    let idx = self.code.get(bci + 1).copied().unwrap_or(0) as usize;
                    stack.push(HirInstruction::LoadLocal(idx));
                }

                // lload
                0x16 => {
                    let idx = self.code.get(bci + 1).copied().unwrap_or(0) as usize;
                    stack.push(HirInstruction::LoadLocal(idx));
                }

                // aload
                0x19 => {
                    let idx = self.code.get(bci + 1).copied().unwrap_or(0) as usize;
                    stack.push(HirInstruction::LoadLocal(idx));
                }

                // iload_0..=iload_3
                0x1a..=0x1d => {
                    let idx = (op - 0x1a) as usize;
                    stack.push(HirInstruction::LoadLocal(idx));
                }

                // lload_0..=lload_3
                0x1e..=0x21 => {
                    let idx = (op - 0x1e) as usize;
                    stack.push(HirInstruction::LoadLocal(idx));
                }

                // aload_0..=aload_3
                0x2a..=0x2d => {
                    let idx = (op - 0x2a) as usize;
                    stack.push(HirInstruction::LoadLocal(idx));
                }

                // istore
                0x36 => {
                    let idx = self.code.get(bci + 1).copied().unwrap_or(0) as usize;
                    let val = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    cfg.blocks[block_id].push(HirInstruction::StoreLocal(idx, Box::new(val)));
                }

                // lstore
                0x37 => {
                    let idx = self.code.get(bci + 1).copied().unwrap_or(0) as usize;
                    let val = stack.pop().unwrap_or(HirInstruction::ConstantLong(0));
                    cfg.blocks[block_id].push(HirInstruction::StoreLocal(idx, Box::new(val)));
                }

                // astore
                0x3a => {
                    let idx = self.code.get(bci + 1).copied().unwrap_or(0) as usize;
                    let val = stack.pop().unwrap_or(HirInstruction::ConstantNull);
                    cfg.blocks[block_id].push(HirInstruction::StoreLocal(idx, Box::new(val)));
                }

                // istore_0..=istore_3
                0x3b..=0x3e => {
                    let idx = (op - 0x3b) as usize;
                    let val = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    cfg.blocks[block_id].push(HirInstruction::StoreLocal(idx, Box::new(val)));
                }

                // lstore_0..=lstore_3
                0x3f..=0x42 => {
                    let idx = (op - 0x3f) as usize;
                    let val = stack.pop().unwrap_or(HirInstruction::ConstantLong(0));
                    cfg.blocks[block_id].push(HirInstruction::StoreLocal(idx, Box::new(val)));
                }

                // astore_0..=astore_3
                0x4b..=0x4e => {
                    let idx = (op - 0x4b) as usize;
                    let val = stack.pop().unwrap_or(HirInstruction::ConstantNull);
                    cfg.blocks[block_id].push(HirInstruction::StoreLocal(idx, Box::new(val)));
                }

                // iadd, ladd
                0x60 | 0x61 => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Add(Box::new(l), Box::new(r)));
                }

                // isub, lsub
                0x64 | 0x65 => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Sub(Box::new(l), Box::new(r)));
                }

                // imul, lmul
                0x68 | 0x69 => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(1));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Mul(Box::new(l), Box::new(r)));
                }

                // idiv, ldiv
                0x6c | 0x6d => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(1));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Div(Box::new(l), Box::new(r)));
                }

                // irem, lrem
                0x70 | 0x71 => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(1));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Rem(Box::new(l), Box::new(r)));
                }

                // ineg, lneg
                0x74 | 0x75 => {
                    let v = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Neg(Box::new(v)));
                }

                // ishl, lshl
                0x78 | 0x79 => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Shl(Box::new(l), Box::new(r)));
                }

                // ishr, lshr
                0x7a | 0x7b => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Shr(Box::new(l), Box::new(r)));
                }

                // iushr, lushr
                0x7c | 0x7d => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Ushr(Box::new(l), Box::new(r)));
                }

                // iand, land
                0x7e | 0x7f => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::And(Box::new(l), Box::new(r)));
                }

                // ior, lor
                0x80 | 0x81 => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Or(Box::new(l), Box::new(r)));
                }

                // ixor, lxor
                0x82 | 0x83 => {
                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    stack.push(HirInstruction::Xor(Box::new(l), Box::new(r)));
                }

                // iinc
                0x84 => {
                    if bci + 2 < self.code.len() {
                        let idx = self.code[bci + 1] as usize;
                        let inc = self.code[bci + 2] as i8 as i32;
                        let load = HirInstruction::LoadLocal(idx);
                        let add = HirInstruction::Add(
                            Box::new(load),
                            Box::new(HirInstruction::ConstantInt(inc)),
                        );
                        cfg.blocks[block_id].push(HirInstruction::StoreLocal(idx, Box::new(add)));
                    }
                }

                // arraylength
                0xbe => {
                    let arr = stack.pop().unwrap_or(HirInstruction::ConstantNull);
                    stack.push(HirInstruction::ArrayLength(Box::new(arr)));
                }

                // goto
                0xa7 => {
                    let offset =
                        i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
                    let target_bci = (bci as i32 + offset).max(0) as usize;
                    if let Some(&target_block) = bci_to_block_id.get(&target_bci) {
                        cfg.blocks[block_id].push(HirInstruction::Goto(target_block));
                    }
                    terminated = true;
                }

                // goto_w
                0xc8 => {
                    let offset = i32::from_be_bytes([
                        self.code[bci + 1],
                        self.code[bci + 2],
                        self.code[bci + 3],
                        self.code[bci + 4],
                    ]);
                    let target_bci = (bci as i32 + offset).max(0) as usize;
                    if let Some(&target_block) = bci_to_block_id.get(&target_bci) {
                        cfg.blocks[block_id].push(HirInstruction::Goto(target_block));
                    }
                    terminated = true;
                }

                // ifeq..=ifle
                0x99..=0x9e => {
                    let cond = match op {
                        0x99 => Condition::Eq,
                        0x9a => Condition::Ne,
                        0x9b => Condition::Lt,
                        0x9c => Condition::Ge,
                        0x9d => Condition::Gt,
                        _ => Condition::Le,
                    };
                    let offset =
                        i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
                    let target_bci = (bci as i32 + offset).max(0) as usize;
                    let fallthrough_bci = next;

                    let true_block = bci_to_block_id
                        .get(&target_bci)
                        .copied()
                        .unwrap_or(block_id + 1);
                    let false_block = bci_to_block_id
                        .get(&fallthrough_bci)
                        .copied()
                        .unwrap_or(block_id + 1);

                    let left = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let right = HirInstruction::ConstantInt(0);

                    cfg.blocks[block_id].push(HirInstruction::If {
                        left: Box::new(left),
                        cond,
                        right: Box::new(right),
                        true_block,
                        false_block,
                    });
                    terminated = true;
                }

                // if_icmpeq..=if_icmple
                0x9f..=0xa4 => {
                    let cond = match op {
                        0x9f => Condition::Eq,
                        0xa0 => Condition::Ne,
                        0xa1 => Condition::Lt,
                        0xa2 => Condition::Ge,
                        0xa3 => Condition::Gt,
                        _ => Condition::Le,
                    };
                    let offset =
                        i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
                    let target_bci = (bci as i32 + offset).max(0) as usize;
                    let fallthrough_bci = next;

                    let true_block = bci_to_block_id
                        .get(&target_bci)
                        .copied()
                        .unwrap_or(block_id + 1);
                    let false_block = bci_to_block_id
                        .get(&fallthrough_bci)
                        .copied()
                        .unwrap_or(block_id + 1);

                    let r = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));
                    let l = stack.pop().unwrap_or(HirInstruction::ConstantInt(0));

                    cfg.blocks[block_id].push(HirInstruction::If {
                        left: Box::new(l),
                        cond,
                        right: Box::new(r),
                        true_block,
                        false_block,
                    });
                    terminated = true;
                }

                // ifnull, ifnonnull
                0xc6 | 0xc7 => {
                    let cond = if op == 0xc6 {
                        Condition::Eq
                    } else {
                        Condition::Ne
                    };
                    let offset =
                        i16::from_be_bytes([self.code[bci + 1], self.code[bci + 2]]) as i32;
                    let target_bci = (bci as i32 + offset).max(0) as usize;
                    let fallthrough_bci = next;

                    let true_block = bci_to_block_id
                        .get(&target_bci)
                        .copied()
                        .unwrap_or(block_id + 1);
                    let false_block = bci_to_block_id
                        .get(&fallthrough_bci)
                        .copied()
                        .unwrap_or(block_id + 1);

                    let val = stack.pop().unwrap_or(HirInstruction::ConstantNull);

                    cfg.blocks[block_id].push(HirInstruction::If {
                        left: Box::new(val),
                        cond,
                        right: Box::new(HirInstruction::ConstantNull),
                        true_block,
                        false_block,
                    });
                    terminated = true;
                }

                // ireturn, lreturn, areturn
                0xac | 0xad | 0xb0 => {
                    let val = stack.pop();
                    cfg.blocks[block_id].push(HirInstruction::Return(val.map(Box::new)));
                    terminated = true;
                }

                // return
                0xb1 => {
                    cfg.blocks[block_id].push(HirInstruction::Return(None));
                    terminated = true;
                }

                // athrow
                0xbf => {
                    let val = stack.pop().unwrap_or(HirInstruction::ConstantNull);
                    cfg.blocks[block_id].push(HirInstruction::Throw(Box::new(val)));
                    terminated = true;
                }

                _ => {}
            }

            bci = next;
        }

        // If block ended without explicit terminator, emit fallthrough Goto to next block
        if !terminated && block_id + 1 < cfg.blocks.len() {
            cfg.blocks[block_id].push(HirInstruction::Goto(block_id + 1));
        }
    }
}
