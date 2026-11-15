/*
 * Port of OpenJDK 25 HotSpot ciTypeFlow (share/ci/ciTypeFlow.hpp/.cpp)
 *
 * `ciTypeFlow` performs the forward type-flow analysis the C2 parser
 * uses: it propagates `ciType`s through locals and the operand stack,
 * joins states at merge points (least upper bound via the klass
 * hierarchy) and records the resulting state at every instruction
 * boundary.  Exception-handler entries receive a state whose stack
 * holds the caught throwable type.
 *
 * The port models each stack/local slot as `Option<CIType>` where
 * `None` means "no type information" (uninitialized local, unknown
 * operand, or the `null` reference).  Two-slot types (long/double)
 * occupy one logical entry; the trailing local slot is cleared.
 */

use super::ciArrayKlass::CiArrayKlass;
use super::ciEnv::CiEnv;
use super::ciMethod::CiMethod;
use super::ciType::CIType;
use crate::share::classfile::classFileParser::Constant;
use crate::share::interpreter::bytecode::{BytecodeLookupswitch, BytecodeTableswitch};
use crate::share::interpreter::bytecodes::{self, Code};
use std::collections::{HashMap, HashSet, VecDeque};

/// `ciTypeFlow::State`: types of locals and the operand stack at one
/// program point.  `locals[i]`/`stack[j]` is `None` when the slot's
/// type is unknown, uninitialized, or the second half of a two-slot
/// value.
#[derive(Debug, Clone, PartialEq)]
pub struct CiFlowState<'a> {
    pub locals: Vec<Option<CIType<'a>>>,
    pub stack: Vec<Option<CIType<'a>>>,
}

impl<'a> CiFlowState<'a> {
    fn new(max_locals: usize) -> Self {
        Self {
            locals: vec![None; max_locals],
            stack: Vec::new(),
        }
    }
}

/// `ciTypeFlow`: computed type states for one method.
pub struct CiTypeFlow<'m, 'a> {
    method: &'m CiMethod<'a>,
    /// State *before* each instruction, keyed by instruction bci.
    states: HashMap<usize, CiFlowState<'a>>,
    /// Instruction start bcis in ascending order.
    instrs: Vec<usize>,
    /// Bcis that are merge points (more than one predecessor edge).
    merge_points: HashSet<usize>,
    /// Exception handler ranges `(start_pc, end_pc, handler_pc)`.
    handlers: Vec<(usize, usize, usize)>,
    /// Type pushed on handler entry stacks (`java/lang/Throwable`).
    throwable: Option<CIType<'a>>,
}

impl<'m, 'a> CiTypeFlow<'m, 'a> {
    /// `ciTypeFlow::ciTypeFlow` + `build`: run the analysis.
    /// Returns `None` when the method has no bytecode.
    pub fn analyze(method: &'m CiMethod<'a>) -> Option<Self> {
        if !method.has_code() {
            return None;
        }
        let bytes = method.bytecodes();
        let mut instrs = Vec::new();
        let mut bci = 0usize;
        while bci < bytes.len() {
            instrs.push(bci);
            let len = instruction_len(bytes, bci);
            let next = bci.saturating_add(len.max(1));
            if next <= bci {
                break;
            }
            bci = next;
        }
        if instrs.is_empty() {
            return None;
        }

        let handlers: Vec<(usize, usize, usize)> = method
            .exception_handlers()
            .iter()
            .map(|h| (h.start_pc, h.end_pc, h.handler_pc))
            .collect();

        let throwable = method
            .env
            .get_klass(b"java/lang/Throwable")
            .map(CIType::Instance);

        let mut flow = Self {
            method,
            states: HashMap::new(),
            instrs,
            merge_points: HashSet::new(),
            handlers,
            throwable,
        };
        flow.run();
        Some(flow)
    }

    /// `ciTypeFlow::state_at`: types before the instruction at `bci`.
    /// For a non-boundary bci the state of the enclosing instruction is
    /// returned (HotSpot requires instruction boundaries).
    pub fn state_at(&self, bci: usize) -> Option<&CiFlowState<'a>> {
        if let Some(s) = self.states.get(&bci) {
            return Some(s);
        }
        let idx = self.instrs.partition_point(|&i| i <= bci);
        if idx == 0 {
            return None;
        }
        self.states.get(&self.instrs[idx - 1])
    }

    /// `ciTypeFlow::local_type_at`: type of local `slot` before `bci`.
    pub fn local_type_at(&self, bci: usize, slot: usize) -> Option<CIType<'a>> {
        self.state_at(bci)?.locals.get(slot)?.clone()
    }

    /// `ciTypeFlow::stack_type_at`: type at stack depth `depth`
    /// (0 = top of stack) before `bci`.
    pub fn stack_type_at(&self, bci: usize, depth: usize) -> Option<CIType<'a>> {
        let stack = &self.state_at(bci)?.stack;
        stack
            .get(stack.len().checked_sub(depth + 1)?)
            .and_then(|t| t.clone())
    }

    /// Operand-stack depth before `bci`.
    pub fn stack_depth_at(&self, bci: usize) -> Option<usize> {
        self.state_at(bci).map(|s| s.stack.len())
    }

    /// `ciTypeFlow::is_merge_point`.
    pub fn is_merge_point(&self, bci: usize) -> bool {
        self.merge_points.contains(&bci)
    }

    /// All computed states (bci -> state), for the parser's
    /// `ciTypeFlow::state_vector` consumers.
    pub fn states(&self) -> &HashMap<usize, CiFlowState<'a>> {
        &self.states
    }

    /// Instruction boundaries.
    pub fn instruction_bcis(&self) -> &[usize] {
        &self.instrs
    }

    // ---- analysis ------------------------------------------------------------

    fn run(&mut self) {
        let env = self.method.env.clone();
        let max_locals = self.method.max_locals();
        let mut entry = CiFlowState::new(max_locals);
        self.init_entry(&mut entry);

        let mut work: VecDeque<(usize, CiFlowState<'a>)> = VecDeque::new();
        work.push_back((self.instrs[0], entry));

        let mut preds: HashMap<usize, usize> = HashMap::new();

        while let Some((bci, incoming)) = work.pop_front() {
            let new_state = match self.states.get(&bci) {
                None => Some(incoming),
                Some(existing) => join_states(existing, &incoming, &env),
            };
            let Some(state_to_store) = new_state else {
                continue;
            };
            self.states.insert(bci, state_to_store);

            let state = self.states.get(&bci).unwrap().clone();
            let bytes = self.method.bytecodes();
            let code = decode_code(bytes, bci);

            // Exception edges: a trapping instruction inside a handler
            // range may transfer to the handler; the handler sees the
            // pre-instruction locals and a fresh throwable on stack.
            for &(start, end, handler) in &self.handlers {
                if bci >= start && bci < end {
                    *preds.entry(handler).or_insert(0) += 1;
                    let mut hs = state.clone();
                    hs.stack.clear();
                    hs.stack.push(self.throwable.clone());
                    work.push_back((handler, hs));
                }
            }

            // Post-transfer state for normal successors.
            let mut st = state;
            let fallthrough = self.transfer(&mut st, bytes, bci, code, &env);
            let next_bci = next_instruction_bci(bytes, bci);

            for dest in self.branch_targets(bytes, bci, code) {
                *preds.entry(dest).or_insert(0) += 1;
                work.push_back((dest, st.clone()));
            }
            if fallthrough {
                if let Some(nb) = next_bci {
                    *preds.entry(nb).or_insert(0) += 1;
                    work.push_back((nb, st));
                }
            }
        }

        self.merge_points = preds
            .into_iter()
            .filter(|(_, c)| *c > 1)
            .map(|(bci, _)| bci)
            .collect();
    }

    /// Fill locals 0..n with the method's receiver and argument types.
    fn init_entry(&self, state: &mut CiFlowState<'a>) {
        let mut slot = 0usize;
        if !self.method.is_static() {
            if slot < state.locals.len() {
                state.locals[slot] = Some(CIType::Instance(self.method.holder().clone()));
            }
            slot += 1;
        }
        if let Some((params, _)) =
            CIType::parse_method_descriptor(&self.method.env, self.method.signature())
        {
            for ty in params {
                if slot >= state.locals.len() {
                    break;
                }
                state.locals[slot] = Some(ty.clone());
                slot += ty.size().max(1);
            }
        }
    }

    /// Stack/local effect of the instruction at `bci`.
    /// `code` is the *effective* opcode (wide sub-opcode resolved).
    /// Returns `true` when control may fall through.
    fn transfer(
        &self,
        st: &mut CiFlowState<'a>,
        bytes: &'a [u8],
        bci: usize,
        code: Code,
        env: &CiEnv<'a>,
    ) -> bool {
        use Code::*;
        let wide = bytes.get(bci) == Some(&0xc4);
        // Operand byte offset of the first operand: wide instructions
        // carry a two-byte prefix (0xc4 + sub-opcode).
        let base = if wide { 2usize } else { 1usize };

        // `_N` shortcut forms are contiguous opcode ranges; handle them
        // by raw opcode byte before the `Code` match (enum range
        // patterns are not allowed in Rust).
        let op = code as u8;
        match op {
            // iload_/lload_/fload_/dload_ N=0..3 (0x1a..=0x29)
            0x1a..=0x29 => {
                let ty = match (op - 0x1a) / 4 {
                    0 => CIType::Int,
                    1 => CIType::Long,
                    2 => CIType::Float,
                    _ => CIType::Double,
                };
                push(st, Some(ty));
                return true;
            }
            // aload_N (0x2a..=0x2d)
            0x2a..=0x2d => {
                let i = (op - 0x2a) as usize;
                push(st, local_get(st, i));
                return true;
            }
            // istore_/lstore_/fstore_/dstore_N (0x3b..=0x4a)
            0x3b..=0x4a => {
                let ty = match (op - 0x3b) / 4 {
                    0 => CIType::Int,
                    1 => CIType::Long,
                    2 => CIType::Float,
                    _ => CIType::Double,
                };
                pop(st);
                let s = ((op - 0x3b) % 4) as usize;
                let two_slot = ty.size() == 2;
                local_set(st, s, Some(ty));
                if two_slot {
                    local_set(st, s + 1, None);
                }
                return true;
            }
            // astore_N (0x4b..=0x4e)
            0x4b..=0x4e => {
                let v = pop(st);
                local_set(st, ((op - 0x4b) % 4) as usize, v);
                return true;
            }
            _ => {}
        }

        match code {
            // ---- constants -------------------------------------------------
            Nop => {}
            AconstNull => push(st, None),
            IconstM1 | Iconst0 | Iconst1 | Iconst2 | Iconst3 | Iconst4 | Iconst5
            | Bipush | Sipush => push(st, Some(CIType::Int)),
            Lconst0 | Lconst1 => push(st, Some(CIType::Long)),
            Fconst0 | Fconst1 | Fconst2 => push(st, Some(CIType::Float)),
            Dconst0 | Dconst1 => push(st, Some(CIType::Double)),
            Ldc | LdcW | Ldc2W => {
                let idx = if code == Ldc {
                    read_u1(bytes, bci, 1).map(u16::from)
                } else {
                    read_u2(bytes, bci, 1)
                }
                .unwrap_or(0);
                push(st, self.ldc_type(idx, code == Ldc2W, env));
            }

            // ---- loads -----------------------------------------------------
            // `_N` forms were handled above; only the indexed opcodes
            // reach this match.
            Iload => {
                push(st, Some(CIType::Int));
            }
            Lload => {
                push(st, Some(CIType::Long));
            }
            Fload => {
                push(st, Some(CIType::Float));
            }
            Dload => {
                push(st, Some(CIType::Double));
            }
            Aload => {
                let i = self.local_index(bytes, bci, code, wide, base);
                push(st, i.and_then(|s| local_get(st, s)));
            }

            // ---- stores ----------------------------------------------------
            Istore => {
                pop(st);
                if let Some(s) = self.local_index(bytes, bci, code, wide, base) {
                    local_set(st, s, Some(CIType::Int));
                }
            }
            Lstore => {
                pop(st);
                if let Some(s) = self.local_index(bytes, bci, code, wide, base) {
                    local_set(st, s, Some(CIType::Long));
                    local_set(st, s + 1, None);
                }
            }
            Fstore => {
                pop(st);
                if let Some(s) = self.local_index(bytes, bci, code, wide, base) {
                    local_set(st, s, Some(CIType::Float));
                }
            }
            Dstore => {
                pop(st);
                if let Some(s) = self.local_index(bytes, bci, code, wide, base) {
                    local_set(st, s, Some(CIType::Double));
                    local_set(st, s + 1, None);
                }
            }
            Astore => {
                let v = pop(st);
                if let Some(s) = self.local_index(bytes, bci, code, wide, base) {
                    local_set(st, s, v);
                }
            }
            Iinc => {
                if let Some(s) = self.local_index(bytes, bci, code, wide, base) {
                    local_set(st, s, Some(CIType::Int));
                }
            }
            Ret => {
                pop(st);
                return false;
            }

            // ---- stack manipulation ----------------------------------------
            Pop => {
                pop(st);
            }
            Pop2 => {
                popn(st, 2);
            }
            Dup => {
                let t = st.stack.last().cloned().flatten();
                push(st, t);
            }
            DupX1 => {
                let n = st.stack.len();
                if n >= 2 {
                    let pair = st.stack.split_off(n - 2);
                    st.stack.push(pair[1].clone());
                    st.stack.push(pair[0].clone());
                    st.stack.push(pair[1].clone());
                }
            }
            DupX2 => {
                let n = st.stack.len();
                if n >= 3 {
                    let triple = st.stack.split_off(n - 3);
                    st.stack.push(triple[2].clone());
                    st.stack.push(triple[0].clone());
                    st.stack.push(triple[1].clone());
                    st.stack.push(triple[2].clone());
                }
            }
            Dup2 => {
                let n = st.stack.len();
                if n >= 2 {
                    let pair = st.stack.split_off(n - 2);
                    st.stack.extend(pair.iter().cloned());
                    st.stack.extend(pair);
                }
            }
            Dup2X1 => {
                let n = st.stack.len();
                if n >= 3 {
                    let triple = st.stack.split_off(n - 3);
                    st.stack.extend(triple[1..].iter().cloned());
                    st.stack.extend(triple);
                }
            }
            Dup2X2 => {
                let n = st.stack.len();
                if n >= 4 {
                    let quad = st.stack.split_off(n - 4);
                    st.stack.extend(quad[2..].iter().cloned());
                    st.stack.extend(quad);
                }
            }
            Swap => {
                let n = st.stack.len();
                if n >= 2 {
                    st.stack.swap(n - 1, n - 2);
                }
            }

            // ---- arithmetic --------------------------------------------------
            Iadd | Isub | Imul | Idiv | Irem | Ishl | Ishr | Iushr | Iand | Ior | Ixor => {
                popn(st, 2);
                push(st, Some(CIType::Int));
            }
            Ladd | Lsub | Lmul | Ldiv | Lrem | Lshl | Lshr | Lushr | Land | Lor | Lxor => {
                popn(st, 2);
                push(st, Some(CIType::Long));
            }
            Fadd | Fsub | Fmul | Fdiv | Frem => {
                popn(st, 2);
                push(st, Some(CIType::Float));
            }
            Dadd | Dsub | Dmul | Ddiv | Drem => {
                popn(st, 2);
                push(st, Some(CIType::Double));
            }
            Ineg => {
                pop(st);
                push(st, Some(CIType::Int));
            }
            Lneg => {
                pop(st);
                push(st, Some(CIType::Long));
            }
            Fneg => {
                pop(st);
                push(st, Some(CIType::Float));
            }
            Dneg => {
                pop(st);
                push(st, Some(CIType::Double));
            }
            I2l => {
                pop(st);
                push(st, Some(CIType::Long));
            }
            I2f => {
                pop(st);
                push(st, Some(CIType::Float));
            }
            I2d => {
                pop(st);
                push(st, Some(CIType::Double));
            }
            L2i => {
                pop(st);
                push(st, Some(CIType::Int));
            }
            L2f => {
                pop(st);
                push(st, Some(CIType::Float));
            }
            L2d => {
                pop(st);
                push(st, Some(CIType::Double));
            }
            F2i => {
                pop(st);
                push(st, Some(CIType::Int));
            }
            F2l => {
                pop(st);
                push(st, Some(CIType::Long));
            }
            F2d => {
                pop(st);
                push(st, Some(CIType::Double));
            }
            D2i => {
                pop(st);
                push(st, Some(CIType::Int));
            }
            D2l => {
                pop(st);
                push(st, Some(CIType::Long));
            }
            D2f => {
                pop(st);
                push(st, Some(CIType::Float));
            }
            I2b | I2c | I2s => {
                pop(st);
                push(st, Some(CIType::Int));
            }
            Lcmp | Fcmpl | Fcmpg | Dcmpl | Dcmpg => {
                popn(st, 2);
                push(st, Some(CIType::Int));
            }

            // ---- branches -----------------------------------------------------
            Ifeq | Ifne | Iflt | Ifge | Ifgt | Ifle | Ifnull | Ifnonnull => {
                pop(st);
                ()
            }
            IfIcmpeq | IfIcmpne | IfIcmplt | IfIcmpge | IfIcmpgt | IfIcmple | IfAcmpeq
            | IfAcmpne => {
                popn(st, 2);
                ()
            }
            Tableswitch | Lookupswitch => {
                pop(st);
                ()
            }
            Goto | GotoW => (),
            Jsr | JsrW => {
                // pushes a return address (untyped) then branches.
                push(st, None);
                ()
            }

            // ---- returns ------------------------------------------------------
            Ireturn | Lreturn | Freturn | Dreturn | Areturn => {
                pop(st);
                ()
            }
            Return => (),
            Athrow => {
                pop(st);
                ()
            }

            // ---- fields -------------------------------------------------------
            Getstatic | Putstatic | Getfield | Putfield => {
                self.transfer_field(st, bytes, bci, code, env);
                ()
            }

            // ---- invokes ------------------------------------------------------
            Invokevirtual | Invokespecial | Invokestatic | Invokeinterface
            | Invokedynamic => {
                self.transfer_invoke(st, bytes, bci, code, env);
                ()
            }

            // ---- objects/arrays -----------------------------------------------
            New => {
                let idx = read_u2(bytes, bci, 1).unwrap_or(0);
                push(st, self.class_type(idx, env));
            }
            Newarray => {
                pop(st);
                let atype = bytes.get(bci + 1).copied().unwrap_or(0x08);
                let elem = match atype {
                    0x04 => CIType::Boolean,
                    0x05 => CIType::Byte,
                    0x06 => CIType::Char,
                    0x07 => CIType::Short,
                    0x08 => CIType::Int,
                    0x09 => CIType::Long,
                    0x0a => CIType::Float,
                    0x0b => CIType::Double,
                    _ => CIType::Int,
                };
                push(st, Some(CIType::Array(CiArrayKlass::new(elem, 1))));
            }
            Anewarray => {
                pop(st);
                let idx = read_u2(bytes, bci, 1).unwrap_or(0);
                let elem = self.class_type(idx, env).unwrap_or(CIType::Void);
                push(st, Some(CIType::Array(CiArrayKlass::new(elem, 1))));
            }
            Arraylength => {
                pop(st);
                push(st, Some(CIType::Int));
            }
            Iaload | Laload | Faload | Daload | Aaload | Baload | Caload | Saload => {
                pop(st); // index
                let arr = pop(st);
                push(st, array_element(arr));
            }
            Iastore | Lastore | Fastore | Dastore | Aastore | Bastore | Castore
            | Sastore => {
                pop(st); // value
                pop(st); // index
                pop(st); // arrayref
            }
            Multianewarray => {
                let dims = bytes.get(bci + 3).copied().unwrap_or(0) as usize;
                popn(st, dims.max(1));
                let idx = read_u2(bytes, bci, 1).unwrap_or(0);
                let name = self.method.holder().class_name_of(idx).unwrap_or(b"");
                push(st, CIType::from_descriptor(env, name));
            }
            Checkcast => {
                let idx = read_u2(bytes, bci, 1).unwrap_or(0);
                let ty = self.class_type(idx, env);
                if let Some(t) = ty {
                    if let Some(top) = st.stack.last_mut() {
                        *top = Some(t);
                    }
                }
            }
            Instanceof => {
                pop(st);
                push(st, Some(CIType::Int));
            }
            Monitorenter | Monitorexit => {
                pop(st);
            }

            _ => {}
        }
        true
    }

    /// `ldc`/`ldc2_w` constant type.
    fn ldc_type(&self, index: u16, two_word: bool, env: &CiEnv<'a>) -> Option<CIType<'a>> {
        let entry = self.method.holder().constant(index)?;
        match entry {
            Constant::Integer(_) => Some(CIType::Int),
            Constant::Float(_) => Some(CIType::Float),
            Constant::Long(_) => Some(CIType::Long),
            Constant::Double(_) => Some(CIType::Double),
            Constant::String(_) => env.get_klass(b"java/lang/String").map(CIType::Instance),
            Constant::Class(name_index) => {
                let name = self.method.holder().utf8(*name_index).unwrap_or(b"");
                if name.first() == Some(&b'[') {
                    CIType::from_descriptor(env, name)
                } else {
                    env.get_klass(name).map(CIType::Instance)
                }
            }
            _ => {
                if two_word {
                    None
                } else {
                    env.get_klass(b"java/lang/Object").map(CIType::Instance)
                }
            }
        }
    }

    /// Type named by a `CONSTANT_Class` index (`new`, `anewarray`,
    /// `checkcast`, `multianewarray`): array descriptors resolve
    /// through `from_descriptor`, plain names through `get_klass`.
    fn class_type(&self, index: u16, env: &CiEnv<'a>) -> Option<CIType<'a>> {
        let name = self.method.holder().class_name_of(index)?;
        if name.first() == Some(&b'[') {
            CIType::from_descriptor(env, name)
        } else {
            env.get_klass(name).map(CIType::Instance)
        }
    }

    /// Resolve the field operand and apply get/put stack effects.
    fn transfer_field(
        &self,
        st: &mut CiFlowState<'a>,
        bytes: &'a [u8],
        bci: usize,
        code: Code,
        env: &CiEnv<'a>,
    ) {
        let idx = read_u2(bytes, bci, 1).unwrap_or(0);
        let field_ty = self
            .method
            .holder()
            .instance_klass()
            .memberRef(idx, true)
            .ok()
            .and_then(|[_, _, desc]| CIType::from_descriptor(env, desc));
        match code {
            Code::Getstatic => push(st, field_ty),
            Code::Putstatic => {
                pop(st);
            }
            Code::Getfield => {
                pop(st); // receiver
                push(st, field_ty);
            }
            Code::Putfield => {
                pop(st); // value
                pop(st); // receiver
            }
            _ => {}
        }
    }

    /// Resolve the invoke operand: pop args (+receiver), push return.
    fn transfer_invoke(
        &self,
        st: &mut CiFlowState<'a>,
        bytes: &'a [u8],
        bci: usize,
        code: Code,
        env: &CiEnv<'a>,
    ) {
        let idx = read_u2(bytes, bci, 1).unwrap_or(0);
        let desc = if code == Code::Invokedynamic {
            self.indy_descriptor(idx)
        } else {
            self.method
                .holder()
                .instance_klass()
                .memberRef(idx, false)
                .ok()
                .map(|[_, _, d]| d)
        };
        let Some(desc) = desc else {
            return;
        };
        let Some((params, ret)) = CIType::parse_method_descriptor(env, desc) else {
            return;
        };
        for _ in &params {
            pop(st);
        }
        let has_receiver = matches!(
            code,
            Code::Invokevirtual | Code::Invokespecial | Code::Invokeinterface
        );
        if has_receiver {
            pop(st);
        }
        if !ret.is_void() {
            push(st, Some(ret));
        }
    }

    /// Call-site descriptor of an `InvokeDynamic` constant:
    /// `InvokeDynamic(_, name_and_type)` -> `NameAndType(_, desc)`.
    fn indy_descriptor(&self, index: u16) -> Option<&'a [u8]> {
        let constants = &self.method.holder().instance_klass().class.constants;
        let nt = match constants.get(usize::from(index))? {
            Constant::InvokeDynamic(_, nt) | Constant::Dynamic(_, nt) => *nt,
            _ => return None,
        };
        match constants.get(usize::from(nt))? {
            Constant::NameAndType(_, desc) => self.method.holder().utf8(*desc),
            _ => None,
        }
    }

    /// Local variable index for load/store/iinc/ret, honouring `wide`
    /// (u2 index) and the `_0.._3` implicit forms.
    fn local_index(
        &self,
        bytes: &'a [u8],
        bci: usize,
        code: Code,
        wide: bool,
        base: usize,
    ) -> Option<usize> {
        use Code::*;
        if wide {
            return read_u2(bytes, bci, base).map(|v| v as usize);
        }
        let op = *bytes.get(bci)?;
        // `_N` forms (defensive; the transfer pre-pass already handled
        // them): loads 0x1a..=0x2d, stores 0x3b..=0x4e, index = N.
        if (0x1a..=0x2d).contains(&op) {
            return Some(((op - 0x1a) % 4) as usize);
        }
        if (0x3b..=0x4e).contains(&op) {
            return Some(((op - 0x3b) % 4) as usize);
        }
        match code {
            Iload | Lload | Fload | Dload | Aload | Istore | Lstore | Fstore | Dstore
            | Astore | Iinc | Ret => read_u1(bytes, bci, base).map(|v| v as usize),
            _ => None,
        }
    }

    /// Branch target bcis for the current instruction (excluding
    /// fallthrough, which the caller adds separately).
    fn branch_targets(&self, bytes: &'a [u8], bci: usize, code: Code) -> Vec<usize> {
        use Code::*;
        let mut out = Vec::new();
        match code {
            Ifeq | Ifne | Iflt | Ifge | Ifgt | Ifle | IfIcmpeq | IfIcmpne | IfIcmplt
            | IfIcmpge | IfIcmpgt | IfIcmple | IfAcmpeq | IfAcmpne | Ifnull | Ifnonnull
            | Goto | Jsr => {
                if let Some(off) = read_s2(bytes, bci, 1) {
                    out.push(bci.wrapping_add_signed(off as isize));
                }
            }
            GotoW | JsrW => {
                if let Some(off) = read_s4(bytes, bci, 1) {
                    out.push(bci.wrapping_add_signed(off as isize));
                }
            }
            Tableswitch => {
                if let Some(ts) = BytecodeTableswitch::new(bytes, bci) {
                    if let Some(def) = ts.default_offset() {
                        out.push(bci.wrapping_add_signed(def as isize));
                    }
                    if let (Some(lo), Some(hi)) = (ts.low_key(), ts.high_key()) {
                        for i in 0..=(hi.saturating_sub(lo)) {
                            if let Some(off) = ts.dest_offset_at(i as usize) {
                                out.push(bci.wrapping_add_signed(off as isize));
                            }
                        }
                    }
                }
            }
            Lookupswitch => {
                if let Some(ls) = BytecodeLookupswitch::new(bytes, bci) {
                    if let Some(def) = ls.default_offset() {
                        out.push(bci.wrapping_add_signed(def as isize));
                    }
                    if let Some(n) = ls.number_of_pairs() {
                        for i in 0..n {
                            if let Some(off) = ls.offset_at(i) {
                                out.push(bci.wrapping_add_signed(off as isize));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        out.retain(|&d| d < bytes.len());
        out.sort_unstable();
        out.dedup();
        out
    }
}

// ---- state join / lub -------------------------------------------------------

/// Merge `incoming` into `existing`: returns the joined state when it
/// differs from `existing`, `None` when `incoming` adds nothing new.
fn join_states<'a>(
    existing: &CiFlowState<'a>,
    incoming: &CiFlowState<'a>,
    env: &CiEnv<'a>,
) -> Option<CiFlowState<'a>> {
    let mut joined = existing.clone();
    let mut changed = false;
    for (j, inc) in incoming.locals.iter().enumerate() {
        if let Some(slot) = joined.locals.get_mut(j) {
            let new = join_type(slot.as_ref(), inc.as_ref(), env);
            if new != *slot {
                *slot = new;
                changed = true;
            }
        }
    }
    let n = joined.stack.len().min(incoming.stack.len());
    let mut stack = Vec::with_capacity(n);
    for i in 0..n {
        let new = join_type(joined.stack[i].as_ref(), incoming.stack[i].as_ref(), env);
        if new != joined.stack[i] {
            changed = true;
        }
        stack.push(new);
    }
    if joined.stack.len() != stack.len() {
        changed = true;
    }
    joined.stack = stack;
    changed.then_some(joined)
}

/// Join two optional types: equal types keep; differing references
/// widen to a common supertype; `None` (unknown) dominates.
fn join_type<'a>(
    a: Option<&CIType<'a>>,
    b: Option<&CIType<'a>>,
    env: &CiEnv<'a>,
) -> Option<CIType<'a>> {
    match (a, b) {
        (Some(x), Some(y)) => Some(lub(x, y, env)),
        _ => None,
    }
}

/// Least upper bound of two ciTypes via the klass hierarchy.
fn lub<'a>(a: &CIType<'a>, b: &CIType<'a>, env: &CiEnv<'a>) -> CIType<'a> {
    if a == b {
        return a.clone();
    }
    if is_subtype(b, a, env) {
        return a.clone();
    }
    if is_subtype(a, b, env) {
        return b.clone();
    }
    if a.is_reference() && b.is_reference() {
        if let Some(o) = env.get_klass(b"java/lang/Object") {
            return CIType::Instance(o);
        }
    }
    a.clone()
}

/// `sub <: sup` check on ciTypes (subclass / interface / array rules).
fn is_subtype<'a>(sub: &CIType<'a>, sup: &CIType<'a>, env: &CiEnv<'a>) -> bool {
    if sub == sup {
        return true;
    }
    match (sub, sup) {
        (CIType::Instance(s), CIType::Instance(p)) => p.is_assignable_from(s),
        (CIType::Array(sa), CIType::Array(pa)) => {
            sa.dimension() == pa.dimension()
                && is_subtype(sa.element_type(), pa.element_type(), env)
        }
        (CIType::Array(_), CIType::Instance(p)) => matches!(
            p.name(),
            b"java/lang/Object" | b"java/lang/Clone" | b"java/io/Serializable"
        ),
        (CIType::Instance(_), CIType::Array(_)) => false,
        _ => false,
    }
}

// ---- state stack helpers ----------------------------------------------------

fn push<'a>(st: &mut CiFlowState<'a>, ty: Option<CIType<'a>>) {
    st.stack.push(ty);
}

fn pop<'a>(st: &mut CiFlowState<'a>) -> Option<CIType<'a>> {
    st.stack.pop().flatten()
}

fn popn<'a>(st: &mut CiFlowState<'a>, n: usize) {
    for _ in 0..n {
        st.stack.pop();
    }
}

fn local_get<'a>(st: &CiFlowState<'a>, slot: usize) -> Option<CIType<'a>> {
    st.locals.get(slot).cloned().flatten()
}

fn local_set<'a>(st: &mut CiFlowState<'a>, slot: usize, ty: Option<CIType<'a>>) {
    if slot < st.locals.len() {
        st.locals[slot] = ty;
    }
}

/// Element type pushed by `*aload` for the popped array reference.
fn array_element<'a>(array: Option<CIType<'a>>) -> Option<CIType<'a>> {
    match array {
        Some(CIType::Array(k)) => Some(k.element_klass()),
        _ => None,
    }
}

// ---- bytecode decoding helpers ----------------------------------------------

fn decode_code(bytes: &[u8], bci: usize) -> Code {
    if bytes.get(bci) == Some(&0xc4) {
        if let Some(&sub) = bytes.get(bci + 1) {
            if let Some(c) = bytecodes::code_from_u8(sub) {
                return c;
            }
        }
        return Code::Wide;
    }
    bytes
        .get(bci)
        .and_then(|&op| bytecodes::code_from_u8(op))
        .unwrap_or(Code::Illegal)
}

fn instruction_len(bytes: &[u8], bci: usize) -> usize {
    let special = bytecodes::special_length_at(bytes, bci);
    if special > 0 {
        return special as usize;
    }
    match bytes.get(bci).and_then(|&op| bytecodes::code_from_u8(op)) {
        Some(code) => {
            let len = bytecodes::length_for_code_at(&code, bci, bytes);
            if len > 0 {
                len as usize
            } else {
                1
            }
        }
        None => 1,
    }
}

fn next_instruction_bci(bytes: &[u8], bci: usize) -> Option<usize> {
    let next = bci.saturating_add(instruction_len(bytes, bci));
    (next < bytes.len()).then_some(next)
}

fn read_u1(bytes: &[u8], bci: usize, offset: usize) -> Option<u8> {
    bytes.get(bci + offset).copied()
}

fn read_u2(bytes: &[u8], bci: usize, offset: usize) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(bci + offset..bci + offset + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(raw))
}

fn read_s2(bytes: &[u8], bci: usize, offset: usize) -> Option<i16> {
    read_u2(bytes, bci, offset).map(|v| v as i16)
}

fn read_s4(bytes: &[u8], bci: usize, offset: usize) -> Option<i32> {
    let raw: [u8; 4] = bytes.get(bci + offset..bci + offset + 4)?.try_into().ok()?;
    Some(i32::from_be_bytes(raw))
}
