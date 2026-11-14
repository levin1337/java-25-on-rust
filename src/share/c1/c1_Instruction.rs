// C1 HIR Instructions ported from HotSpot c1_Instruction.hpp / .cpp

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Condition {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Condition {
    pub fn negate(&self) -> Self {
        match self {
            Self::Eq => Self::Ne,
            Self::Ne => Self::Eq,
            Self::Lt => Self::Ge,
            Self::Le => Self::Gt,
            Self::Gt => Self::Le,
            Self::Ge => Self::Lt,
        }
    }

    pub fn mirror(&self) -> Self {
        match self {
            Self::Eq => Self::Eq,
            Self::Ne => Self::Ne,
            Self::Lt => Self::Gt,
            Self::Le => Self::Ge,
            Self::Gt => Self::Lt,
            Self::Ge => Self::Le,
        }
    }

    pub fn evaluate_i32(&self, left: i32, right: i32) -> bool {
        match self {
            Self::Eq => left == right,
            Self::Ne => left != right,
            Self::Lt => left < right,
            Self::Le => left <= right,
            Self::Gt => left > right,
            Self::Ge => left >= right,
        }
    }

    pub fn evaluate_i64(&self, left: i64, right: i64) -> bool {
        match self {
            Self::Eq => left == right,
            Self::Ne => left != right,
            Self::Lt => left < right,
            Self::Le => left <= right,
            Self::Gt => left > right,
            Self::Ge => left >= right,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockBegin {
    pub block_id: usize,
    pub bci: usize,
    pub depth_first_number: i32,
    pub linear_scan_number: i32,
    pub loop_depth: usize,
    pub is_entry: bool,
    pub is_osr_entry: bool,
    pub is_exception_entry: bool,
    pub predecessors: Vec<usize>,
    pub successors: Vec<usize>,
}

impl BlockBegin {
    pub fn new(block_id: usize, bci: usize) -> Self {
        Self {
            block_id,
            bci,
            depth_first_number: -1,
            linear_scan_number: -1,
            loop_depth: 0,
            is_entry: block_id == 0,
            is_osr_entry: false,
            is_exception_entry: false,
            predecessors: Vec::new(),
            successors: Vec::new(),
        }
    }

    pub fn block_id(&self) -> usize {
        self.block_id
    }

    pub fn bci(&self) -> usize {
        self.bci
    }

    pub fn add_predecessor(&mut self, pred: usize) {
        if !self.predecessors.contains(&pred) {
            self.predecessors.push(pred);
        }
    }

    pub fn remove_predecessor(&mut self, pred: usize) {
        self.predecessors.retain(|&p| p != pred);
    }

    pub fn add_successor(&mut self, sux: usize) {
        if !self.successors.contains(&sux) {
            self.successors.push(sux);
        }
    }

    pub fn substitute_successor(&mut self, old_sux: usize, new_sux: usize) {
        for sux in &mut self.successors {
            if *sux == old_sux {
                *sux = new_sux;
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockEnd {
    Goto(usize),
    If {
        left: Box<HirInstruction>,
        cond: Condition,
        right: Box<HirInstruction>,
        true_block: usize,
        false_block: usize,
    },
    Return(Option<Box<HirInstruction>>),
    Throw(Box<HirInstruction>),
    TableSwitch {
        index: Box<HirInstruction>,
        low: i32,
        high: i32,
        default_block: usize,
        targets: Vec<usize>,
    },
    LookupSwitch {
        key: Box<HirInstruction>,
        pairs: Vec<(i32, usize)>,
        default_block: usize,
    },
}

impl BlockEnd {
    pub fn successors(&self) -> Vec<usize> {
        match self {
            Self::Goto(target) => vec![*target],
            Self::If {
                true_block,
                false_block,
                ..
            } => vec![*true_block, *false_block],
            Self::Return(_) | Self::Throw(_) => Vec::new(),
            Self::TableSwitch {
                default_block,
                targets,
                ..
            } => {
                let mut sux = targets.clone();
                sux.push(*default_block);
                sux
            }
            Self::LookupSwitch {
                pairs,
                default_block,
                ..
            } => {
                let mut sux: Vec<usize> = pairs.iter().map(|(_, target)| *target).collect();
                sux.push(*default_block);
                sux
            }
        }
    }

    pub fn substitute_successor(&mut self, old_sux: usize, new_sux: usize) {
        match self {
            Self::Goto(target) => {
                if *target == old_sux {
                    *target = new_sux;
                }
            }
            Self::If {
                true_block,
                false_block,
                ..
            } => {
                if *true_block == old_sux {
                    *true_block = new_sux;
                }
                if *false_block == old_sux {
                    *false_block = new_sux;
                }
            }
            Self::Return(_) | Self::Throw(_) => {}
            Self::TableSwitch {
                default_block,
                targets,
                ..
            } => {
                if *default_block == old_sux {
                    *default_block = new_sux;
                }
                for t in targets {
                    if *t == old_sux {
                        *t = new_sux;
                    }
                }
            }
            Self::LookupSwitch {
                pairs,
                default_block,
                ..
            } => {
                if *default_block == old_sux {
                    *default_block = new_sux;
                }
                for (_, t) in pairs {
                    if *t == old_sux {
                        *t = new_sux;
                    }
                }
            }
        }
    }

    pub fn to_instruction(self) -> HirInstruction {
        match self {
            Self::Goto(target) => HirInstruction::Goto(target),
            Self::If {
                left,
                cond,
                right,
                true_block,
                false_block,
            } => HirInstruction::If {
                left,
                cond,
                right,
                true_block,
                false_block,
            },
            Self::Return(val) => HirInstruction::Return(val),
            Self::Throw(val) => HirInstruction::Throw(val),
            Self::TableSwitch {
                index,
                low,
                high,
                default_block,
                targets,
            } => HirInstruction::TableSwitch {
                index,
                low,
                high,
                default_block,
                targets,
            },
            Self::LookupSwitch {
                key,
                pairs,
                default_block,
            } => HirInstruction::LookupSwitch {
                key,
                pairs,
                default_block,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirInstruction {
    // Constants
    ConstantInt(i32),
    ConstantLong(i64),
    ConstantFloat(u32),
    ConstantDouble(u64),
    ConstantNull,

    // Locals
    LoadLocal(usize),
    StoreLocal(usize, Box<HirInstruction>),

    // Arithmetic & Logic
    Add(Box<HirInstruction>, Box<HirInstruction>),
    Sub(Box<HirInstruction>, Box<HirInstruction>),
    Mul(Box<HirInstruction>, Box<HirInstruction>),
    Div(Box<HirInstruction>, Box<HirInstruction>),
    Rem(Box<HirInstruction>, Box<HirInstruction>),
    Neg(Box<HirInstruction>),
    And(Box<HirInstruction>, Box<HirInstruction>),
    Or(Box<HirInstruction>, Box<HirInstruction>),
    Xor(Box<HirInstruction>, Box<HirInstruction>),
    Shl(Box<HirInstruction>, Box<HirInstruction>),
    Shr(Box<HirInstruction>, Box<HirInstruction>),
    Ushr(Box<HirInstruction>, Box<HirInstruction>),

    // Control flow / BlockEnd
    Goto(usize),
    If {
        left: Box<HirInstruction>,
        cond: Condition,
        right: Box<HirInstruction>,
        true_block: usize,
        false_block: usize,
    },
    Return(Option<Box<HirInstruction>>),
    Throw(Box<HirInstruction>),
    TableSwitch {
        index: Box<HirInstruction>,
        low: i32,
        high: i32,
        default_block: usize,
        targets: Vec<usize>,
    },
    LookupSwitch {
        key: Box<HirInstruction>,
        pairs: Vec<(i32, usize)>,
        default_block: usize,
    },

    // Memory / Object operations
    ArrayLength(Box<HirInstruction>),
    LoadIndexed {
        array: Box<HirInstruction>,
        index: Box<HirInstruction>,
    },
    StoreIndexed {
        array: Box<HirInstruction>,
        index: Box<HirInstruction>,
        value: Box<HirInstruction>,
    },
    LoadField {
        obj: Box<HirInstruction>,
        offset: usize,
        field_name: String,
    },
    StoreField {
        obj: Box<HirInstruction>,
        offset: usize,
        value: Box<HirInstruction>,
        field_name: String,
    },

    // Block start marker
    BlockBegin(BlockBegin),
}

impl HirInstruction {
    pub fn is_constant(&self) -> bool {
        matches!(
            self,
            Self::ConstantInt(_)
                | Self::ConstantLong(_)
                | Self::ConstantFloat(_)
                | Self::ConstantDouble(_)
                | Self::ConstantNull
        )
    }

    pub fn as_int_constant(&self) -> Option<i32> {
        match self {
            Self::ConstantInt(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_long_constant(&self) -> Option<i64> {
        match self {
            Self::ConstantLong(v) => Some(*v),
            _ => None,
        }
    }

    pub fn is_block_end(&self) -> bool {
        matches!(
            self,
            Self::Goto(_)
                | Self::If { .. }
                | Self::Return(_)
                | Self::Throw(_)
                | Self::TableSwitch { .. }
                | Self::LookupSwitch { .. }
        )
    }

    pub fn successors(&self) -> Vec<usize> {
        match self {
            Self::Goto(target) => vec![*target],
            Self::If {
                true_block,
                false_block,
                ..
            } => vec![*true_block, *false_block],
            Self::Return(_) | Self::Throw(_) => Vec::new(),
            Self::TableSwitch {
                default_block,
                targets,
                ..
            } => {
                let mut sux = targets.clone();
                sux.push(*default_block);
                sux
            }
            Self::LookupSwitch {
                pairs,
                default_block,
                ..
            } => {
                let mut sux: Vec<usize> = pairs.iter().map(|(_, target)| *target).collect();
                sux.push(*default_block);
                sux
            }
            _ => Vec::new(),
        }
    }

    pub fn substitute_successor(&mut self, old_sux: usize, new_sux: usize) {
        match self {
            Self::Goto(target) => {
                if *target == old_sux {
                    *target = new_sux;
                }
            }
            Self::If {
                true_block,
                false_block,
                ..
            } => {
                if *true_block == old_sux {
                    *true_block = new_sux;
                }
                if *false_block == old_sux {
                    *false_block = new_sux;
                }
            }
            Self::TableSwitch {
                default_block,
                targets,
                ..
            } => {
                if *default_block == old_sux {
                    *default_block = new_sux;
                }
                for t in targets {
                    if *t == old_sux {
                        *t = new_sux;
                    }
                }
            }
            Self::LookupSwitch {
                pairs,
                default_block,
                ..
            } => {
                if *default_block == old_sux {
                    *default_block = new_sux;
                }
                for (_, t) in pairs {
                    if *t == old_sux {
                        *t = new_sux;
                    }
                }
            }
            _ => {}
        }
    }

    pub fn as_block_end(&self) -> Option<BlockEnd> {
        match self {
            Self::Goto(target) => Some(BlockEnd::Goto(*target)),
            Self::If {
                left,
                cond,
                right,
                true_block,
                false_block,
            } => Some(BlockEnd::If {
                left: left.clone(),
                cond: *cond,
                right: right.clone(),
                true_block: *true_block,
                false_block: *false_block,
            }),
            Self::Return(val) => Some(BlockEnd::Return(val.clone())),
            Self::Throw(val) => Some(BlockEnd::Throw(val.clone())),
            Self::TableSwitch {
                index,
                low,
                high,
                default_block,
                targets,
            } => Some(BlockEnd::TableSwitch {
                index: index.clone(),
                low: *low,
                high: *high,
                default_block: *default_block,
                targets: targets.clone(),
            }),
            Self::LookupSwitch {
                key,
                pairs,
                default_block,
            } => Some(BlockEnd::LookupSwitch {
                key: key.clone(),
                pairs: pairs.clone(),
                default_block: *default_block,
            }),
            _ => None,
        }
    }
}
