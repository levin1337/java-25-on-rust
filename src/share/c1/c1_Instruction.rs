#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueKind {
    Void,
    Int32,
    Int64,
    Float32,
    Float64,
    Ref,
}

impl ValueKind {
    pub fn stack_slots(self) -> usize {
        match self {
            Self::Void => 0,
            Self::Int64 | Self::Float64 => 2,
            _ => 1,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::Float32 | Self::Float64)
    }

    pub fn is_numeric(self) -> bool {
        !matches!(self, Self::Void | Self::Ref)
    }

    pub fn is_reference(self) -> bool {
        self == Self::Ref
    }

    pub fn from_descriptor(desc: &[u8]) -> Self {
        match desc.first().copied() {
            Some(b'V') => Self::Void,
            Some(b'J') => Self::Int64,
            Some(b'F') => Self::Float32,
            Some(b'D') => Self::Float64,
            Some(b'[') | Some(b'L') => Self::Ref,
            _ => Self::Int32,
        }
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompareKind {
    Int32,
    Int64,
    Ref,
    Float32NanLow,
    Float32NanHigh,
    Float64NanLow,
    Float64NanHigh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConvertKind {
    I2L,
    I2F,
    I2D,
    L2I,
    L2F,
    L2D,
    F2I,
    F2L,
    F2D,
    D2I,
    D2L,
    D2F,
    I2B,
    I2C,
    I2S,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArrayElemKind {
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Ref,
}

impl ArrayElemKind {
    pub fn value_kind(self) -> ValueKind {
        match self {
            Self::Long => ValueKind::Int64,
            Self::Float => ValueKind::Float32,
            Self::Double => ValueKind::Float64,
            Self::Ref => ValueKind::Ref,
            _ => ValueKind::Int32,
        }
    }

    pub fn element_size(self) -> usize {
        match self {
            Self::Boolean | Self::Byte => 1,
            Self::Char | Self::Short => 2,
            Self::Int | Self::Float | Self::Ref => 4,
            Self::Long | Self::Double => 8,
        }
    }

    pub fn element_align(self) -> usize {
        self.element_size()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallInfo {
    pub opcode: u8,
    pub cp_index: u32,
    pub name: String,
    pub arg_kinds: Vec<ValueKind>,
    pub return_kind: ValueKind,
    pub is_static: bool,
    pub is_interface: bool,
    pub target: Option<u64>,
    pub resolved: bool,
    /// Leaked `StubSite` pointer for the CallMethod stub; 0 when the
    /// constant-pool site was not resolved (stub deopts).
    pub site: u64,
}

impl CallInfo {
    pub fn unresolved(opcode: u8, cp_index: u32) -> Self {
        Self {
            opcode,
            cp_index,
            name: format!("cp#{}", cp_index),
            arg_kinds: Vec::new(),
            return_kind: ValueKind::Void,
            is_static: matches!(opcode, 0xb8 | 0xba),
            is_interface: opcode == 0xb9,
            target: None,
            resolved: false,
            site: 0,
        }
    }

    pub fn is_resolved(&self) -> bool {
        self.resolved
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
            Self::If { true_block, false_block, .. } => vec![*true_block, *false_block],
            Self::Return(_) | Self::Throw(_) => Vec::new(),
            Self::TableSwitch { default_block, targets, .. } => {
                let mut sux = targets.clone();
                sux.push(*default_block);
                sux
            }
            Self::LookupSwitch { pairs, default_block, .. } => {
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
            Self::If { true_block, false_block, .. } => {
                if *true_block == old_sux {
                    *true_block = new_sux;
                }
                if *false_block == old_sux {
                    *false_block = new_sux;
                }
            }
            Self::Return(_) | Self::Throw(_) => {}
            Self::TableSwitch { default_block, targets, .. } => {
                if *default_block == old_sux {
                    *default_block = new_sux;
                }
                for t in targets {
                    if *t == old_sux {
                        *t = new_sux;
                    }
                }
            }
            Self::LookupSwitch { pairs, default_block, .. } => {
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
            Self::If { left, cond, right, true_block, false_block } => HirInstruction::If {
                left,
                cond,
                right,
                true_block,
                false_block,
            },
            Self::Return(val) => HirInstruction::Return(val),
            Self::Throw(val) => HirInstruction::Throw(val),
            Self::TableSwitch { index, low, high, default_block, targets } => HirInstruction::TableSwitch {
                index,
                low,
                high,
                default_block,
                targets,
            },
            Self::LookupSwitch { key, pairs, default_block } => HirInstruction::LookupSwitch {
                key,
                pairs,
                default_block,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HirInstruction {
    ConstantInt(i32),
    ConstantLong(i64),
    ConstantFloat(u32),
    ConstantDouble(u64),
    ConstantNull,

    LoadLocal { slot: usize, kind: ValueKind },
    StoreLocal { slot: usize, kind: ValueKind, value: Box<HirInstruction> },
    LoadStack { slot: usize, kind: ValueKind },
    StoreStack { slot: usize, kind: ValueKind, value: Box<HirInstruction> },

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

    Convert { kind: ConvertKind, value: Box<HirInstruction> },
    Compare { kind: CompareKind, left: Box<HirInstruction>, right: Box<HirInstruction> },

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

    ArrayLength(Box<HirInstruction>),
    LoadIndexed {
        array: Box<HirInstruction>,
        index: Box<HirInstruction>,
        elem: ArrayElemKind,
    },
    StoreIndexed {
        array: Box<HirInstruction>,
        index: Box<HirInstruction>,
        value: Box<HirInstruction>,
        elem: ArrayElemKind,
    },
    NewArray { elem: ArrayElemKind, length: Box<HirInstruction> },
    AnewArray { cp_index: u32, length: Box<HirInstruction> },
    NewObject { cp_index: u32, name: String },

    LoadField {
        obj: Box<HirInstruction>,
        cp_index: u32,
        offset: Option<usize>,
        kind: ValueKind,
        name: String,
    },
    StoreField {
        obj: Box<HirInstruction>,
        cp_index: u32,
        offset: Option<usize>,
        value: Box<HirInstruction>,
        kind: ValueKind,
        name: String,
    },
    GetStatic {
        cp_index: u32,
        offset: Option<usize>,
        kind: ValueKind,
        name: String,
    },
    PutStatic {
        cp_index: u32,
        offset: Option<usize>,
        value: Box<HirInstruction>,
        kind: ValueKind,
        name: String,
    },

    Call {
        info: CallInfo,
        operands: Vec<Box<HirInstruction>>,
    },
    CheckCast { value: Box<HirInstruction>, cp_index: u32, name: String },
    InstanceOf { value: Box<HirInstruction>, cp_index: u32, name: String },
    MonitorEnter(Box<HirInstruction>),
    MonitorExit(Box<HirInstruction>),

    Unsupported { opcode: u8, bci: usize, reason: String },
}

impl HirInstruction {
    pub fn value_kind(&self) -> ValueKind {
        match self {
            Self::ConstantInt(_) => ValueKind::Int32,
            Self::ConstantLong(_) => ValueKind::Int64,
            Self::ConstantFloat(_) => ValueKind::Float32,
            Self::ConstantDouble(_) => ValueKind::Float64,
            Self::ConstantNull => ValueKind::Ref,
            Self::LoadLocal { kind, .. }
            | Self::LoadStack { kind, .. }
            | Self::LoadField { kind, .. }
            | Self::GetStatic { kind, .. } => *kind,
            Self::NewArray { .. } | Self::AnewArray { .. } | Self::NewObject { .. } | Self::CheckCast { .. } => ValueKind::Ref,
            Self::Call { info, .. } => info.return_kind,
            Self::ArrayLength(_) | Self::InstanceOf { .. } | Self::Compare { .. } => ValueKind::Int32,
            Self::LoadIndexed { elem, .. } => elem.value_kind(),
            Self::Add(l, r)
            | Self::Sub(l, r)
            | Self::Mul(l, r)
            | Self::Div(l, r)
            | Self::Rem(l, r)
            | Self::And(l, r)
            | Self::Or(l, r)
            | Self::Xor(l, r)
            | Self::Shl(l, r)
            | Self::Shr(l, r)
            | Self::Ushr(l, r) => binary_kind(l.value_kind(), r.value_kind()),
            Self::Neg(v) => match v.value_kind() {
                ValueKind::Int64 => ValueKind::Int64,
                ValueKind::Float32 => ValueKind::Float32,
                ValueKind::Float64 => ValueKind::Float64,
                _ => ValueKind::Int32,
            },
            Self::Convert { kind, .. } => match kind {
                ConvertKind::I2L | ConvertKind::F2L | ConvertKind::D2L => ValueKind::Int64,
                ConvertKind::I2F | ConvertKind::L2F | ConvertKind::D2F => ValueKind::Float32,
                ConvertKind::I2D | ConvertKind::L2D | ConvertKind::F2D => ValueKind::Float64,
                _ => ValueKind::Int32,
            },
            _ => ValueKind::Void,
        }
    }

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
            Self::If { true_block, false_block, .. } => vec![*true_block, *false_block],
            Self::Return(_) | Self::Throw(_) => Vec::new(),
            Self::TableSwitch { default_block, targets, .. } => {
                let mut sux = targets.clone();
                sux.push(*default_block);
                sux
            }
            Self::LookupSwitch { pairs, default_block, .. } => {
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
            Self::If { true_block, false_block, .. } => {
                if *true_block == old_sux {
                    *true_block = new_sux;
                }
                if *false_block == old_sux {
                    *false_block = new_sux;
                }
            }
            Self::TableSwitch { default_block, targets, .. } => {
                if *default_block == old_sux {
                    *default_block = new_sux;
                }
                for t in targets {
                    if *t == old_sux {
                        *t = new_sux;
                    }
                }
            }
            Self::LookupSwitch { pairs, default_block, .. } => {
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
            Self::If { left, cond, right, true_block, false_block } => Some(BlockEnd::If {
                left: left.clone(),
                cond: *cond,
                right: right.clone(),
                true_block: *true_block,
                false_block: *false_block,
            }),
            Self::Return(val) => Some(BlockEnd::Return(val.clone())),
            Self::Throw(val) => Some(BlockEnd::Throw(val.clone())),
            Self::TableSwitch { index, low, high, default_block, targets } => Some(BlockEnd::TableSwitch {
                index: index.clone(),
                low: *low,
                high: *high,
                default_block: *default_block,
                targets: targets.clone(),
            }),
            Self::LookupSwitch { key, pairs, default_block } => Some(BlockEnd::LookupSwitch {
                key: key.clone(),
                pairs: pairs.clone(),
                default_block: *default_block,
            }),
            _ => None,
        }
    }
}

fn binary_kind(l: ValueKind, r: ValueKind) -> ValueKind {
    match (l, r) {
        (ValueKind::Float64, _) | (_, ValueKind::Float64) => ValueKind::Float64,
        (ValueKind::Float32, _) | (_, ValueKind::Float32) => ValueKind::Float32,
        (ValueKind::Int64, _) | (_, ValueKind::Int64) => ValueKind::Int64,
        (ValueKind::Ref, ValueKind::Ref) => ValueKind::Ref,
        _ => ValueKind::Int32,
    }
}
