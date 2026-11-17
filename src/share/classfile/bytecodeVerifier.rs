/*
 * Bytecode-level verifier, ported in spirit from OpenJDK 25 HotSpot
 * `share/classfile/verifier.cpp` (`ClassVerifier::verify_method`,
 * `verify_stackmap_table`, `verify_exception_handler_table`).
 *
 * Two verification modes mirror HotSpot:
 *
 *  - Stack-map mode (class file version >= 50.0): the `StackMapTable`
 *    attribute partitions the code into basic blocks. Every branch target,
 *    exception-handler entry and fall-through-required point must carry a
 *    frame; incoming states are checked for assignability against the
 *    declared frame and the frame becomes the authoritative state.
 *  - Type-inference mode (version < 50.0): a worklist dataflow computes the
 *    verification type of every local and stack slot, merging at control
 *    flow joins. Exception edges propagate `catch_type`/`Throwable`.
 *
 * Type assignability between reference types uses the loaded class
 * hierarchy when it is already available through the `ClassLoader`; types
 * that cannot be resolved are treated conservatively (accepted), matching
 * the fact that HotSpot resolves supertypes lazily during verification.
 */

use crate::share::classfile::classFileParser::{ClassFile, Constant, Member};
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::classfile::verifier::VerificationType as VT;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{self, Error};

/// A parsed field/method descriptor type.
#[derive(Clone, Debug, PartialEq)]
enum FieldType {
    /// Primitive tag: B C D F I J S Z (V only valid as return type).
    Prim(u8),
    /// Reference type stored as the internal name for plain classes
    /// ("java/lang/String") or the array descriptor ("[I", "[[Ljava/lang/String;").
    Ref(String),
}

impl FieldType {
    fn parse(bytes: &[u8], pos: &mut usize) -> Option<FieldType> {
        if *pos >= bytes.len() {
            return None;
        }
        match bytes[*pos] {
            b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' | b'V' => {
                let tag = bytes[*pos];
                *pos += 1;
                Some(FieldType::Prim(tag))
            }
            b'L' => {
                let start = *pos;
                *pos += 1;
                while *pos < bytes.len() && bytes[*pos] != b';' {
                    *pos += 1;
                }
                if *pos >= bytes.len() {
                    return None;
                }
                *pos += 1;
                let inner = String::from_utf8_lossy(&bytes[start + 1..*pos - 1]).into_owned();
                Some(FieldType::Ref(inner))
            }
            b'[' => {
                let start = *pos;
                while *pos < bytes.len() {
                    match bytes[*pos] {
                        b'[' => *pos += 1,
                        b'L' => {
                            *pos += 1;
                            while *pos < bytes.len() && bytes[*pos] != b';' {
                                *pos += 1;
                            }
                            if *pos >= bytes.len() {
                                return None;
                            }
                            *pos += 1;
                            break;
                        }
                        b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' => {
                            *pos += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                if *pos > bytes.len() || *pos == start {
                    return None;
                }
                Some(FieldType::Ref(
                    String::from_utf8_lossy(&bytes[start..*pos]).into_owned(),
                ))
            }
            _ => None,
        }
    }

    /// Parses a full method descriptor "(params)return".
    fn method(desc: &[u8]) -> Option<(Vec<FieldType>, FieldType)> {
        if desc.first() != Some(&b'(') {
            return None;
        }
        let mut pos = 1usize;
        let mut params = Vec::new();
        while pos < desc.len() && desc[pos] != b')' {
            let t = FieldType::parse(desc, &mut pos)?;
            if matches!(t, FieldType::Prim(b'V')) {
                return None;
            }
            params.push(t);
        }
        if pos >= desc.len() || desc[pos] != b')' {
            return None;
        }
        pos += 1;
        let ret = FieldType::parse(desc, &mut pos)?;
        if pos != desc.len() {
            return None;
        }
        Some((params, ret))
    }

    /// Parses a single field descriptor.
    fn field(desc: &[u8]) -> Option<FieldType> {
        let mut pos = 0usize;
        let t = FieldType::parse(desc, &mut pos)?;
        if pos != desc.len() || matches!(t, FieldType::Prim(b'V')) {
            return None;
        }
        Some(t)
    }

    /// Verification type for stack/local slots.
    fn vtype(&self) -> VT {
        match self {
            FieldType::Prim(b'J') => VT::Long,
            FieldType::Prim(b'F') => VT::Float,
            FieldType::Prim(b'D') => VT::Double,
            FieldType::Prim(_) => VT::Integer,
            FieldType::Ref(name) => VT::Object(name.clone()),
        }
    }
}

/// Hierarchy summary of a loaded class, used for assignability checks.
#[derive(Clone)]
struct KlassInfo {
    super_name: Option<Vec<u8>>,
    interfaces: Vec<Vec<u8>>,
    is_interface: bool,
}

impl KlassInfo {
    fn array() -> Self {
        Self {
            super_name: Some(b"java/lang/Object".to_vec()),
            interfaces: vec![
                b"java/lang/Cloneable".to_vec(),
                b"java/lang/Serializable".to_vec(),
            ],
            is_interface: false,
        }
    }
}

#[derive(Clone, Debug)]
struct Insn {
    bci: usize,
    len: usize,
    op: u8,
    /// `wide` prefix was present; operand decoding uses u2 indexes.
    wide: bool,
    /// Parsed branch targets (switch instructions only).
    targets: Vec<usize>,
}

#[derive(Clone)]
struct FrameState {
    locals: Vec<VT>,
    stack: Vec<VT>,
}

#[derive(Clone, Default, PartialEq)]
struct State {
    /// Per-slot verification types; category-2 values occupy two slots
    /// (Long/Double followed by Long2nd/Double2nd).
    locals: Vec<VT>,
    stack: Vec<VT>,
}

/// One exception-table entry.
#[derive(Clone)]
struct TryRange {
    start: usize,
    end: usize,
    handler: usize,
    /// catch_type internal name, or None for finally (catch_type 0).
    catch: Option<Vec<u8>>,
}

/// Control-flow outcome of executing one instruction.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    Fallthrough,
    NoFallthrough,
}

/// Which kind of value a store instruction expects.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StoreKind {
    Int,
    Long,
    Float,
    Double,
    Ref,
}

pub struct BytecodeVerifier<'m, 'a> {
    class: &'m ClassFile<'a>,
    method: &'m Member<'a>,
    constants: &'m [Constant<'a>],
    code: &'m [u8],
    max_stack: usize,
    max_locals: usize,
    insns: Vec<Insn>,
    /// bci -> instruction index for boundary checks.
    insn_at: HashMap<usize, usize>,
    /// bci -> parsed stack map frame (verification mode >= 50).
    frames: BTreeMap<usize, FrameState>,
    /// bcis that must carry a stack map frame.
    required_frames: HashSet<usize>,
    try_ranges: Vec<TryRange>,
    /// bci of `new` -> internal class name, for uninitialized tracking.
    new_classes: std::cell::RefCell<HashMap<usize, String>>,
    loader: Option<&'m ClassLoader<'a>>,
}

impl<'m, 'a> BytecodeVerifier<'m, 'a> {
    /// Verifies the `Code` attribute of `method`. `loader` optionally
    /// provides the already-loaded class hierarchy for precise
    /// assignability checks.
    pub fn verify(
        class: &'m ClassFile<'a>,
        method: &'m Member<'a>,
        loader: Option<&'m ClassLoader<'a>>,
    ) -> io::Result<()> {
        let Some(code_attr) = method.code_attribute() else {
            return Ok(());
        };
        Self::verify_code(class, method, code_attr, loader)
    }

    fn verify_code(
        class: &'m ClassFile<'a>,
        method: &'m Member<'a>,
        code_attr: &'a [u8],
        loader: Option<&'m ClassLoader<'a>>,
    ) -> io::Result<()> {
        if code_attr.len() < 8 {
            return Err(err(format!(
                "Code attribute too short in {}",
                String::from_utf8_lossy(method.name)
            )));
        }
        let max_stack = u16::from_be_bytes([code_attr[0], code_attr[1]]) as usize;
        let max_locals = u16::from_be_bytes([code_attr[2], code_attr[3]]) as usize;
        let code_length = u32::from_be_bytes([
            code_attr[4],
            code_attr[5],
            code_attr[6],
            code_attr[7],
        ]) as usize;
        if code_length == 0 {
            return Err(err("Code attribute has zero code_length"));
        }
        let code_end = 8usize
            .checked_add(code_length)
            .filter(|end| *end <= code_attr.len())
            .ok_or_else(|| err("Truncated code array in Code attribute"))?;
        let code = &code_attr[8..code_end];

        // Exception table.
        let mut pos = code_end;
        let read_u2 = |pos: &mut usize| -> io::Result<u16> {
            let b = code_attr
                .get(*pos..*pos + 2)
                .ok_or_else(|| err("Truncated exception table"))?;
            *pos += 2;
            Ok(u16::from_be_bytes([b[0], b[1]]))
        };
        let exc_count = read_u2(&mut pos)? as usize;
        let mut try_ranges = Vec::with_capacity(exc_count);
        for _ in 0..exc_count {
            let start_pc = read_u2(&mut pos)? as usize;
            let end_pc = read_u2(&mut pos)? as usize;
            let handler_pc = read_u2(&mut pos)? as usize;
            let catch_idx = read_u2(&mut pos)?;
            // JVMS 4.7.3: start_pc < end_pc <= code_length,
            // handler_pc < code_length.
            if start_pc >= end_pc || end_pc > code_length || handler_pc >= code_length {
                return Err(err(format!(
                    "Illegal exception table entry start={start_pc} end={end_pc} handler={handler_pc} code_length={code_length}"
                )));
            }
            // catch_type must reference a CONSTANT_Class.
            let catch = if catch_idx == 0 {
                None
            } else {
                match class.constants.get(catch_idx as usize) {
                    Some(Constant::Class(_)) => {}
                    _ => {
                        return Err(err(format!(
                            "Illegal catch_type index {catch_idx} in exception table"
                        )));
                    }
                }
                Some(class_name_of(&class.constants, catch_idx)?.to_vec())
            };
            try_ranges.push(TryRange {
                start: start_pc,
                end: end_pc,
                handler: handler_pc,
                catch,
            });
        }

        // Code attributes (StackMapTable etc.).
        let attr_count = read_u2(&mut pos)? as usize;
        let mut stackmap_bytes: Option<&'a [u8]> = None;
        for _ in 0..attr_count {
            let name_idx = read_u2(&mut pos)?;
            let len_bytes = code_attr
                .get(pos..pos + 4)
                .ok_or_else(|| err("Truncated Code attribute"))?;
            pos += 4;
            let len = u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
                as usize;
            let info = code_attr
                .get(pos..pos + len)
                .ok_or_else(|| err("Truncated Code attribute info"))?;
            pos += len;
            if utf8_of(&class.constants, name_idx)? == b"StackMapTable" {
                if stackmap_bytes.is_some() {
                    return Err(err("Duplicate StackMapTable attribute"));
                }
                stackmap_bytes = Some(info);
            }
        }
        if pos != code_attr.len() {
            return Err(err("Trailing bytes in Code attribute"));
        }

        let use_stackmaps = class.majorVersion >= 50;

        let mut verifier = BytecodeVerifier {
            class,
            method,
            constants: &class.constants,
            code,
            max_stack,
            max_locals,
            insns: Vec::new(),
            insn_at: HashMap::new(),
            frames: BTreeMap::new(),
            required_frames: HashSet::new(),
            try_ranges,
            new_classes: std::cell::RefCell::new(HashMap::new()),
            loader,
        };

        verifier.decode()?;

        if use_stackmaps {
            if let Some(bytes) = stackmap_bytes {
                verifier.parse_stackmap(bytes)?;
            }
            // Required frames: branch targets + handler pcs.
            for insn in &verifier.insns {
                for &t in &insn.targets {
                    verifier.required_frames.insert(t);
                }
            }
            for tr in &verifier.try_ranges {
                verifier.required_frames.insert(tr.handler);
            }
            verifier.verify_frames_present()?;
            verifier.verify_with_frames()?;
        } else {
            verifier.verify_with_inference()?;
        }
        Ok(())
    }

    // ---------- constant pool helpers ----------

    fn utf8(&self, index: u16) -> io::Result<&'a [u8]> {
        utf8_of(self.constants, index)
    }

    fn constant(&self, index: u16) -> io::Result<&Constant<'a>> {
        self.constants
            .get(index as usize)
            .filter(|c| !matches!(c, Constant::Unusable))
            .ok_or_else(|| err(format!("Invalid constant pool index {index}")))
    }

    fn class_name(&self, index: u16) -> io::Result<&'a [u8]> {
        match self.constant(index)? {
            Constant::Class(name_idx) => self.utf8(*name_idx),
            _ => Err(err(format!("Expected CONSTANT_Class at index {index}"))),
        }
    }

    fn name_and_type(&self, index: u16) -> io::Result<(&'a [u8], &'a [u8])> {
        match self.constant(index)? {
            Constant::NameAndType(name_idx, desc_idx) => {
                Ok((self.utf8(*name_idx)?, self.utf8(*desc_idx)?))
            }
            _ => Err(err(format!("Expected CONSTANT_NameAndType at index {index}"))),
        }
    }

    /// Loads a `CONSTANT_Class` operand and returns the internal name used
    /// for `VT::Object` (strips `L...;`, keeps array descriptors).
    fn object_type_name(&self, index: u16) -> io::Result<String> {
        let raw = self.class_name(index)?;
        Ok(normalize_object_name(raw))
    }

    // ---------- hierarchy / assignability ----------

    fn klass_info(&self, name: &[u8]) -> Option<KlassInfo> {
        if name.starts_with(b"[") {
            return Some(KlassInfo::array());
        }
        if name == self.class.name {
            return Some(KlassInfo {
                super_name: self.class.superName.map(|s| s.to_vec()),
                interfaces: self.class.interfaces.iter().map(|i| i.to_vec()).collect(),
                is_interface: self.class.accessFlags & 0x0200 != 0,
            });
        }
        let loader = self.loader?;
        let classes = loader.classes.borrow();
        let klass = classes.get(name)?;
        Some(KlassInfo {
            super_name: klass.super_klass.as_ref().map(|s| s.class.name.to_vec()),
            interfaces: klass.interfaces.iter().map(|i| i.class.name.to_vec()).collect(),
            is_interface: klass.class.accessFlags & 0x0200 != 0,
        })
    }

    /// `from_name` assignable to `to_name` (both internal names / array descs).
    fn type_assignable(&self, from: &[u8], to: &[u8]) -> bool {
        if from == to || to == b"java/lang/Object" {
            return true;
        }
        let from_arr = from.starts_with(b"[");
        let to_arr = to.starts_with(b"[");
        if from_arr || to_arr {
            if from_arr && to_arr {
                return self.array_element_assignable(from, to);
            }
            if from_arr {
                return matches!(
                    to,
                    b"java/lang/Object" | b"java/lang/Cloneable" | b"java/lang/Serializable"
                );
            }
            return false;
        }
        // HotSpot (resolve_and_check_assignability): an interface target is
        // treated as java.lang.Object for class-hierarchy assignability —
        // the strict array rules above already restricted array sources to
        // Cloneable/Serializable.
        if let Some(info) = self.klass_info(to)
            && info.is_interface
        {
            return true;
        }
        // Class hierarchy DFS.
        let mut visited = HashSet::new();
        let mut pending = vec![from.to_vec()];
        while let Some(current) = pending.pop() {
            if current == to {
                return true;
            }
            if !visited.insert(current.clone()) {
                continue;
            }
            let Some(info) = self.klass_info(&current) else {
                // Unresolvable: be conservative.
                return true;
            };
            if let Some(s) = info.super_name {
                pending.push(s);
            }
            pending.extend(info.interfaces);
        }
        false
    }

    /// Element-wise assignability for two array descriptors. JVMS §4.10.1.2:
    /// `[S` is assignable to `[T` iff `S` is assignable to `T`, comparing
    /// exactly one dimension at a time — `int[][]` is assignable to
    /// `Object[]` because `int[]` is assignable to `Object`.
    fn array_element_assignable(&self, from: &[u8], to: &[u8]) -> bool {
        self.descriptor_assignable(&from[1..], &to[1..])
    }

    /// Assignability between two element descriptors: primitives (`I`),
    /// object types (`Lpkg/Name;`) or nested array descriptors (`[...`).
    fn descriptor_assignable(&self, from: &[u8], to: &[u8]) -> bool {
        if from == to {
            return true;
        }
        let from_arr = from.starts_with(b"[");
        let to_arr = to.starts_with(b"[");
        if from_arr || to_arr {
            if from_arr && to_arr {
                return self.array_element_assignable(from, to);
            }
            if from_arr {
                return matches!(
                    to,
                    b"Ljava/lang/Object;" | b"Ljava/lang/Cloneable;" | b"Ljava/lang/Serializable;"
                );
            }
            return false;
        }
        match (from.first(), to.first()) {
            (Some(b'L'), Some(b'L')) => {
                let fname = normalize_object_name(&from[1..from.len().saturating_sub(1)]);
                let tname = normalize_object_name(&to[1..to.len().saturating_sub(1)]);
                self.type_assignable(fname.as_bytes(), tname.as_bytes())
            }
            _ => false,
        }
    }

    /// Is verification-type `actual` assignable to `expected`?
    fn vt_assignable(&self, expected: &VT, actual: &VT) -> bool {
        if expected == actual {
            return true;
        }
        use VT::*;
        match (expected, actual) {
            // Top is the "unusable" type: it is assignable in both
            // directions — an unusable incoming value imposes no constraint
            // on the frame (HotSpot accepts it; the slot is never read on
            // that path), and a Top frame slot accepts anything.
            (Top, _) | (_, Top) => true,
            (Null, Null) => true,
            (Null, _) => false,
            (Object(_), Null) => true,
            (Object(e), Object(a)) => self.type_assignable(a.as_bytes(), e.as_bytes()),
            (Object(e), UninitializedThis) => {
                let self_name = String::from_utf8_lossy(self.class.name).into_owned();
                self.type_assignable(self_name.as_bytes(), e.as_bytes())
            }
            (Object(_), Uninitialized(_)) => false,
            (UninitializedThis, UninitializedThis) => true,
            (Uninitialized(e), Uninitialized(a)) => e == a,
            (UninitializedThis, _) | (Uninitialized(_), _) => false,
            // Integer family (all int-like types merge to Integer).
            (Integer, Integer)
            | (Integer, Boolean)
            | (Integer, Byte)
            | (Integer, Short)
            | (Integer, Char)
            | (Boolean, Integer)
            | (Boolean, Boolean)
            | (Boolean, Byte)
            | (Boolean, Short)
            | (Boolean, Char)
            | (Byte, Integer)
            | (Byte, Boolean)
            | (Byte, Byte)
            | (Byte, Short)
            | (Byte, Char)
            | (Short, Integer)
            | (Short, Boolean)
            | (Short, Byte)
            | (Short, Short)
            | (Short, Char)
            | (Char, Integer)
            | (Char, Boolean)
            | (Char, Byte)
            | (Char, Short)
            | (Char, Char) => true,
            (Long, Long) | (Float, Float) | (Double, Double) => true,
            (Long2nd, Long2nd) | (Double2nd, Double2nd) => true,
            _ => false,
        }
    }

    // ---------- instruction decoding ----------

    fn decode(&mut self) -> io::Result<()> {
        let code_len = self.code.len();
        let mut bci = 0usize;
        while bci < code_len {
            let op = self.code[bci];
            if op == 0xc4 {
                // wide
                if bci + 1 >= code_len {
                    return Err(err_at(bci, "Truncated wide instruction"));
                }
                let sub = self.code[bci + 1];
                // `wide` may only prefix load/store/iinc; `ret` (and jsr)
                // are not part of the class file format at all.
                if sub == 0xa9 || sub == 0xa8 || sub == 0xc9 {
                    return Err(err_at(bci, "Bad instruction: jsr/ret not allowed"));
                }
                let len = crate::share::interpreter::bytecodes::wide_length_for(sub);
                if len <= 0 {
                    return Err(err_at(bci, "Bad wide instruction"));
                }
                let len = len as usize;
                if bci + len > code_len {
                    return Err(err_at(bci, "Truncated wide instruction"));
                }
                self.push_insn(bci, len, sub, true, Vec::new())?;
                bci += len;
                continue;
            }
            if !crate::share::interpreter::bytecodes::is_java_code(op)
                || !crate::share::interpreter::bytecodes::is_defined(op)
            {
                return Err(err_at(bci, format!("Bad instruction: {op:02x}")));
            }
            // jsr/ret/jsr_w are not part of the class file format since
            // Java 6 (see HotSpot verifier.cpp: "jsr and ret are not in the
            // new class file format").
            if matches!(op, 0xa8 | 0xa9 | 0xc9) {
                return Err(err_at(bci, "Bad instruction: jsr/ret not allowed"));
            }
            let mut targets = Vec::new();
            let len = match op {
                0xaa => {
                    // tableswitch: pad to 4-byte boundary from method start.
                    let pad = (4 - ((bci + 1) % 4)) % 4;
                    let base = bci + 1 + pad;
                    if base + 12 > code_len {
                        return Err(err_at(bci, "Truncated tableswitch"));
                    }
                    let default = self.s4_at(base) as isize;
                    let low = self.s4_at(base + 4) as isize;
                    let high = self.s4_at(base + 8) as isize;
                    if low > high || (high - low) as u64 > i32::MAX as u64 {
                        return Err(err_at(bci, "Bad tableswitch range"));
                    }
                    let count = (high - low + 1) as usize;
                    if base + 12 + count * 4 > code_len {
                        return Err(err_at(bci, "Truncated tableswitch"));
                    }
                    targets.push((bci as isize + default) as usize);
                    for i in 0..count {
                        let off = self.s4_at(base + 12 + i * 4) as isize;
                        targets.push((bci as isize + off) as usize);
                    }
                    1 + pad + 12 + count * 4
                }
                0xab => {
                    // lookupswitch
                    let pad = (4 - ((bci + 1) % 4)) % 4;
                    let base = bci + 1 + pad;
                    if base + 8 > code_len {
                        return Err(err_at(bci, "Truncated lookupswitch"));
                    }
                    let default = self.s4_at(base) as isize;
                    let npairs = self.s4_at(base + 4);
                    if npairs < 0 || npairs as usize > (code_len - base - 8) / 8 {
                        return Err(err_at(bci, "Truncated lookupswitch"));
                    }
                    targets.push((bci as isize + default) as usize);
                    let mut prev = i32::MIN;
                    for i in 0..npairs as usize {
                        let match_off = base + 8 + i * 8;
                        let key = self.s4_at(match_off);
                        let off = self.s4_at(match_off + 4) as isize;
                        if i > 0 && key <= prev {
                            return Err(err_at(bci, "lookupswitch keys not sorted"));
                        }
                        prev = key;
                        targets.push((bci as isize + off) as usize);
                    }
                    1 + pad + 8 + npairs as usize * 8
                }
                _ => {
                    let l = crate::share::interpreter::bytecodes::length_for(op);
                    if l <= 0 {
                        return Err(err_at(bci, format!("Bad instruction: {op:02x}")));
                    }
                    let len = l as usize;
                    if bci + len > code_len {
                        return Err(err_at(bci, "Truncated instruction"));
                    }
                    match op {
                        0x99..=0xa7 | 0xc6 | 0xc7 => {
                            let off = self.s2_at(bci + 1) as isize;
                            targets.push((bci as isize + off) as usize);
                        }
                        0xc8 => {
                            let off = self.s4_at(bci + 1) as isize;
                            targets.push((bci as isize + off) as usize);
                        }
                        _ => {}
                    }
                    len
                }
            };
            self.push_insn(bci, len, op, false, targets)?;
            bci += len;
        }
        // All branch targets must land on instruction boundaries.
        for insn in &self.insns {
            for &t in &insn.targets {
                if !self.insn_at.contains_key(&t) {
                    return Err(err_at(
                        insn.bci,
                        format!("Branch target {t} is not an instruction boundary"),
                    ));
                }
            }
        }
        // Exception handler pcs must be instruction boundaries.
        for tr in &self.try_ranges {
            if !self.insn_at.contains_key(&tr.handler) {
                return Err(err(format!(
                    "Exception handler pc {} is not an instruction boundary",
                    tr.handler
                )));
            }
        }
        Ok(())
    }

    fn push_insn(
        &mut self,
        bci: usize,
        len: usize,
        op: u8,
        wide: bool,
        targets: Vec<usize>,
    ) -> io::Result<()> {
        self.insns.push(Insn {
            bci,
            len,
            op,
            wide,
            targets,
        });
        self.insn_at.insert(bci, self.insns.len() - 1);
        Ok(())
    }

    fn s2_at(&self, pos: usize) -> i16 {
        i16::from_be_bytes([self.code[pos], self.code[pos + 1]])
    }

    fn s4_at(&self, pos: usize) -> i32 {
        i32::from_be_bytes([
            self.code[pos],
            self.code[pos + 1],
            self.code[pos + 2],
            self.code[pos + 3],
        ])
    }

    fn u1_at(&self, pos: usize) -> u8 {
        self.code[pos]
    }

    fn u2_at(&self, pos: usize) -> u16 {
        u16::from_be_bytes([self.code[pos], self.code[pos + 1]])
    }

    // ---------- stack map table ----------

    /// Parses the StackMapTable attribute into per-bci frame states.
    fn parse_stackmap(&mut self, bytes: &'a [u8]) -> io::Result<()> {
        if bytes.len() < 2 {
            return Err(err("StackMapTable attribute truncated"));
        }
        let num = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
        let mut pos = 2usize;
        let mut offset = 0usize;
        // JVMS 4.7.4: the first frame's bytecode offset is its offset_delta;
        // each later frame's offset is previous_offset + offset_delta + 1.
        let mut first_frame = true;
        // Previous frame state for delta decoding. The implicit frame
        // before the first entry is the method's initial locals (arguments
        // only, no Top padding).
        let mut prev_locals: Vec<VT> = self.initial_state().locals;
        while matches!(prev_locals.last(), Some(VT::Top)) {
            prev_locals.pop();
        }
        let mut prev_stack: Vec<VT> = Vec::new();
        for i in 0..num {
            let frame_type = *bytes
                .get(pos)
                .ok_or_else(|| err("StackMapTable truncated"))?;
            pos += 1;
            let (delta, locals, stack): (usize, Vec<VT>, Vec<VT>) =
                if frame_type <= 63 {
                    // same_frame
                    (frame_type as usize, prev_locals.clone(), Vec::new())
                } else if frame_type <= 127 {
                    // same_locals_1_stack_item
                    let vt = self.read_verification_type(bytes, &mut pos)?;
                    let (item, second) = expand_verification_type(vt);
                    let mut st = vec![item];
                    st.extend(second);
                    ((frame_type - 64) as usize, prev_locals.clone(), st)
                } else if frame_type == 247 {
                    // same_locals_1_stack_item_extended
                    let d = read_u2(bytes, &mut pos)? as usize;
                    let vt = self.read_verification_type(bytes, &mut pos)?;
                    let (item, second) = expand_verification_type(vt);
                    let mut st = vec![item];
                    st.extend(second);
                    (d, prev_locals.clone(), st)
                } else if (248..=250).contains(&frame_type) {
                    // chop_frame: k counts logical locals, while the
                    // expanded representation gives category-2 values two
                    // slots (primary + 2nd marker).
                    let d = read_u2(bytes, &mut pos)? as usize;
                    let k = (251 - frame_type) as usize;
                    let mut locals = prev_locals.clone();
                    for _ in 0..k {
                        if matches!(locals.pop(), Some(vt) if vt.is_category2_2nd()) {
                            locals.pop();
                        }
                    }
                    (d, locals, Vec::new())
                } else if frame_type == 251 {
                    // same_frame_extended
                    let d = read_u2(bytes, &mut pos)? as usize;
                    (d, prev_locals.clone(), Vec::new())
                } else if (252..=254).contains(&frame_type) {
                    // append_frame
                    let d = read_u2(bytes, &mut pos)? as usize;
                    let k = (frame_type - 251) as usize;
                    let mut locals = prev_locals.clone();
                    for _ in 0..k {
                        let vt = self.read_verification_type(bytes, &mut pos)?;
                        let (vt, second) = expand_verification_type(vt);
                        locals.push(vt);
                        locals.extend(second);
                    }
                    (d, locals, Vec::new())
                } else if frame_type == 255 {
                    // full_frame
                    let d = read_u2(bytes, &mut pos)? as usize;
                    let nlocals = read_u2(bytes, &mut pos)? as usize;
                    let mut locals = Vec::with_capacity(nlocals);
                    for _ in 0..nlocals {
                        let vt = self.read_verification_type(bytes, &mut pos)?;
                        let (vt, second) = expand_verification_type(vt);
                        locals.push(vt);
                        locals.extend(second);
                    }
                    let nstack = read_u2(bytes, &mut pos)? as usize;
                    let mut stack = Vec::with_capacity(nstack);
                    for _ in 0..nstack {
                        let vt = self.read_verification_type(bytes, &mut pos)?;
                        let (vt, second) = expand_verification_type(vt);
                        stack.push(vt);
                        stack.extend(second);
                    }
                    (d, locals, stack)
                } else {
                    // frame_type 128..=246 is reserved.
                    return Err(err(format!("Illegal stack map frame type {frame_type}")));
                };
            offset = if first_frame {
                first_frame = false;
                delta
            } else {
                offset + delta + 1
            };
            if offset >= self.code.len() {
                return Err(err(format!(
                    "StackMapTable frame {} offset {offset} out of bounds",
                    i + 1
                )));
            }
            if !self.insn_at.contains_key(&offset) {
                return Err(err(format!(
                    "StackMapTable frame at bci {offset} is not an instruction boundary"
                )));
            }
            if self
                .frames
                .insert(offset, FrameState { locals, stack })
                .is_some()
            {
                return Err(err(format!("Duplicate stack map frame at bci {offset}")));
            }
            if let Some(f) = self.frames.get(&offset) {
                prev_locals = f.locals.clone();
                prev_stack = f.stack.clone();
            }
        }
        let _ = prev_stack;
        if pos != bytes.len() {
            return Err(err("Trailing bytes in StackMapTable"));
        }
        Ok(())
    }

    /// Reads one verification_type_info.
    fn read_verification_type(&self, bytes: &[u8], pos: &mut usize) -> io::Result<VT> {
        let tag = *bytes.get(*pos).ok_or_else(|| err("StackMapTable truncated"))?;
        *pos += 1;
        Ok(match tag {
            0 => VT::Top,
            1 => VT::Integer,
            2 => VT::Float,
            3 => VT::Double,
            4 => VT::Long,
            5 => VT::Null,
            6 => VT::UninitializedThis,
            7 => {
                let idx = read_u2(bytes, pos)?;
                VT::Object(self.object_type_name(idx)?)
            }
            8 => {
                let off = read_u2(bytes, pos)? as u16;
                VT::Uninitialized(off)
            }
            _ => return Err(err(format!("Illegal verification type tag {tag}"))),
        })
    }

    /// Every required frame point must have a stack map frame (JVMS 4.7.4:
    /// "a stack map frame must appear at each basic block").
    fn verify_frames_present(&self) -> io::Result<()> {
        for &bci in &self.required_frames {
            if !self.frames.contains_key(&bci) {
                return Err(err(format!(
                    "Expecting a stackmap frame at branch target {bci}"
                )));
            }
        }
        for (bci, frame) in &self.frames {
            if frame.locals.len() > self.max_locals {
                return Err(err(format!(
                    "Stack map frame at {bci} has {} locals > max_locals {}",
                    frame.locals.len(),
                    self.max_locals
                )));
            }
        }
        Ok(())
    }

    // ---------- state helpers ----------

    fn initial_state(&self) -> State {
        let mut st = State::default();
        st.locals = vec![VT::Top; self.max_locals];
        if let Some((params, _)) = FieldType::method(self.method.descriptor) {
            let is_static = (self.method.accessFlags & 0x0008) != 0;
            if !is_static && self.max_locals == 0 {
                // Instance methods require at least one local for `this`.
                // Reported by the caller as a VerifyError.
                return State::default();
            }
            let mut slot = 0usize;
            if !is_static {
                if !st.locals.is_empty() {
                    st.locals[0] = if self.method.name == b"<init>" {
                        VT::UninitializedThis
                    } else {
                        VT::Object(String::from_utf8_lossy(self.class.name).into_owned())
                    };
                }
                slot = 1;
            }
            for p in params {
                if slot >= self.max_locals {
                    break;
                }
                let vt = p.vtype();
                st.locals[slot] = vt.clone();
                if vt.is_category2() {
                    if slot + 1 < self.max_locals {
                        st.locals[slot + 1] = match vt {
                            VT::Long => VT::Long2nd,
                            VT::Double => VT::Double2nd,
                            _ => VT::Top,
                        };
                    }
                    slot += 2;
                } else {
                    slot += 1;
                }
            }
        }
        st
    }

    fn push_val(st: &mut State, vt: VT) {
        st.stack.push(vt.clone());
        if vt.is_category2() {
            st.stack.push(match vt {
                VT::Long => VT::Long2nd,
                VT::Double => VT::Double2nd,
                _ => VT::Top,
            });
        }
    }

    /// Pushes a single stack entry (used by dup/swap forms).
    fn push_entry(st: &mut State, vt: VT) {
        st.stack.push(vt);
    }

    /// Pops a single stack entry.
    fn pop1(&self, st: &mut State, bci: usize) -> io::Result<VT> {
        let vt = st
            .stack
            .pop()
            .ok_or_else(|| err_at(bci, "Operand stack underflow"))?;
        if vt == VT::Top {
            return Err(err_at(bci, "Unusable (top) value on operand stack"));
        }
        Ok(vt)
    }

    /// Pops a single entry that must be category-1.
    fn pop_cat1(&self, st: &mut State, bci: usize) -> io::Result<VT> {
        let vt = self.pop1(st, bci)?;
        if !is_cat1(&vt) {
            return Err(err_at(bci, format!("Expecting category-1, found {vt:?}")));
        }
        Ok(vt)
    }

    /// Pops the primary word of a category-2 value whose 2nd marker was
    /// already popped as `second`.
    fn pop_cat2_primary(&self, st: &mut State, bci: usize, second: &VT) -> io::Result<VT> {
        let vt = self.pop1(st, bci)?;
        let matches = matches!(
            (second, &vt),
            (VT::Long2nd, VT::Long) | (VT::Double2nd, VT::Double)
        );
        if !matches {
            return Err(err_at(
                bci,
                format!("Malformed category-2 stack entry: {vt:?} / {second:?}"),
            ));
        }
        Ok(vt)
    }

    /// Pops one "dup unit": either a category-2 pair (primary+2nd) or two
    /// category-1 entries. Returns the entries bottom-to-top.
    fn pop_dup_unit(&self, st: &mut State, bci: usize) -> io::Result<Vec<VT>> {
        let top = self.pop1(st, bci)?;
        if top.is_category2_2nd() {
            let primary = self.pop_cat2_primary(st, bci, &top)?;
            Ok(vec![primary, top])
        } else {
            if !is_cat1(&top) {
                return Err(err_at(bci, format!("dup2 of invalid type {top:?}")));
            }
            let below = self.pop_cat1(st, bci)?;
            Ok(vec![below, top])
        }
    }

    fn pop_val(&self, st: &mut State, bci: usize) -> io::Result<VT> {
        let top = st
            .stack
            .pop()
            .ok_or_else(|| err_at(bci, "Operand stack underflow"))?;
        if top == VT::Top {
            return Err(err_at(bci, "Unusable (top) value on operand stack"));
        }
        if top.is_category2_2nd() {
            let first = st
                .stack
                .pop()
                .ok_or_else(|| err_at(bci, "Operand stack underflow"))?;
            if !first.is_category2() {
                return Err(err_at(bci, "Malformed category-2 stack entry"));
            }
            Ok(first)
        } else {
            Ok(top)
        }
    }

    fn pop_expect(&self, st: &mut State, bci: usize, required: &VT) -> io::Result<VT> {
        let actual = self.pop_val(st, bci)?;
        if !self.vt_assignable(required, &actual) {
            return Err(err_at(
                bci,
                format!("Expecting type {required:?} but found {actual:?} on stack"),
            ));
        }
        Ok(actual)
    }

    fn pop_int(&self, st: &mut State, bci: usize) -> io::Result<()> {
        self.pop_expect(st, bci, &VT::Integer).map(|_| ())
    }

    fn pop_ref(&self, st: &mut State, bci: usize) -> io::Result<VT> {
        let actual = self.pop_val(st, bci)?;
        if !actual.is_reference() {
            return Err(err_at(
                bci,
                format!("Expecting reference on stack but found {actual:?}"),
            ));
        }
        Ok(actual)
    }

    fn check_stack_depth(&self, st: &State, bci: usize) -> io::Result<()> {
        if st.stack.len() > self.max_stack {
            return Err(err_at(bci, "Operand stack overflow"));
        }
        Ok(())
    }

    fn local_get(&self, st: &State, idx: usize, bci: usize) -> io::Result<VT> {
        if idx >= self.max_locals {
            return Err(err_at(bci, format!("Local index {idx} out of bounds")));
        }
        st.locals
            .get(idx)
            .cloned()
            .ok_or_else(|| err_at(bci, format!("Local index {idx} out of bounds")))
    }

    fn local_set(&self, st: &mut State, idx: usize, vt: VT, bci: usize) -> io::Result<()> {
        if idx >= self.max_locals {
            return Err(err_at(bci, format!("Local index {idx} out of bounds")));
        }
        st.locals[idx] = vt.clone();
        if vt.is_category2() {
            if idx + 1 >= self.max_locals {
                return Err(err_at(bci, "Category-2 local out of bounds"));
            }
            st.locals[idx + 1] = match vt {
                VT::Long => VT::Long2nd,
                VT::Double => VT::Double2nd,
                _ => VT::Top,
            };
        }
        Ok(())
    }

    /// Loads a local for xload instructions, validating category-2 pairs.
    fn load_local(&self, st: &State, idx: usize, bci: usize) -> io::Result<VT> {
        let vt = self.local_get(st, idx, bci)?;
        if vt == VT::Top || vt.is_category2_2nd() {
            return Err(err_at(bci, format!("Reading unusable local {idx}")));
        }
        if vt.is_category2() {
            let second = self.local_get(st, idx + 1, bci).unwrap_or(VT::Top);
            let expected_second = match vt {
                VT::Long => VT::Long2nd,
                VT::Double => VT::Double2nd,
                _ => VT::Top,
            };
            if second != expected_second {
                return Err(err_at(bci, format!("Malformed category-2 local {idx}")));
            }
        }
        Ok(vt)
    }

    // ---------- instruction execution ----------

    /// Executes the instruction at `bci`, mutating `st` to the
    /// post-instruction state. Returns whether control falls through.
    fn exec(&self, st: &mut State, bci: usize) -> io::Result<Flow> {
        let insn_idx = *self
            .insn_at
            .get(&bci)
            .ok_or_else(|| err(format!("No instruction at bci {bci}")))?;
        let insn = &self.insns[insn_idx];
        let op = insn.op;
        let wide = insn.wide;
        // Operand base: normal insns have opcode at bci, operands at bci+1;
        // wide insns have opcode at bci+1, operands at bci+2.
        let base = if wide { bci + 2 } else { bci + 1 };
        let local_index = |this: &Self| -> io::Result<usize> {
            if wide {
                Ok(this.u2_at(base) as usize)
            } else {
                Ok(this.u1_at(base) as usize)
            }
        };

        match op {
            0x00 => Ok(Flow::Fallthrough), // nop
            0x01 => {
                Self::push_val(st, VT::Null);
                Ok(Flow::Fallthrough)
            }
            0x02..=0x08 => {
                Self::push_val(st, VT::Integer);
                Ok(Flow::Fallthrough)
            }
            0x09 | 0x0a => {
                Self::push_val(st, VT::Long);
                Ok(Flow::Fallthrough)
            }
            0x0b..=0x0d => {
                Self::push_val(st, VT::Float);
                Ok(Flow::Fallthrough)
            }
            0x0e | 0x0f => {
                Self::push_val(st, VT::Double);
                Ok(Flow::Fallthrough)
            }
            0x10 | 0x11 => {
                // bipush / sipush
                Self::push_val(st, VT::Integer);
                Ok(Flow::Fallthrough)
            }
            0x12 | 0x13 | 0x14 => {
                // ldc / ldc_w / ldc2_w
                let idx = if op == 0x12 {
                    self.u1_at(base) as u16
                } else {
                    self.u2_at(base)
                };
                let c = self.constant(idx)?;
                match c {
                    Constant::Integer(_) => Self::push_val(st, VT::Integer),
                    Constant::Float(_) => Self::push_val(st, VT::Float),
                    Constant::Long(_) => {
                        if op != 0x14 {
                            return Err(err_at(bci, "ldc cannot load long"));
                        }
                        Self::push_val(st, VT::Long)
                    }
                    Constant::Double(_) => {
                        if op != 0x14 {
                            return Err(err_at(bci, "ldc cannot load double"));
                        }
                        Self::push_val(st, VT::Double)
                    }
                    Constant::String(_) => {
                        Self::push_val(st, VT::Object("java/lang/String".into()))
                    }
                    Constant::Class(name_idx) => {
                        // ldc of a Class pushes java.lang.Class; the runtime
                        // type is always java.lang.Class regardless of which
                        // class literal is loaded. The payload is a Utf8
                        // name index, not a CONSTANT_Class index.
                        let _ = normalize_object_name(self.utf8(*name_idx)?);
                        Self::push_val(st, VT::Object("java/lang/Class".into()))
                    }
                    Constant::MethodType(_) => {
                        Self::push_val(st, VT::Object("java/lang/invoke/MethodType".into()))
                    }
                    Constant::MethodHandle(_, kind) => {
                        if !(1..=9).contains(kind) {
                            return Err(err_at(bci, "Illegal MethodHandle reference kind"));
                        }
                        Self::push_val(st, VT::Object("java/lang/invoke/MethodHandle".into()))
                    }
                    Constant::Dynamic(_, nt) => {
                        // condy: the resolved type is the field descriptor of
                        // the NameAndType.
                        if let Ok((_, desc)) = self.name_and_type(*nt)
                            && let Some(ft) = FieldType::field(desc)
                        {
                            Self::push_val(st, ft.vtype());
                            self.check_stack_depth(st, bci)?;
                            return Ok(Flow::Fallthrough);
                        }
                        Self::push_val(st, VT::Object("java/lang/Object".into()))
                    }
                    _ => {
                        return Err(err_at(bci, "Illegal ldc constant pool entry"));
                    }
                }
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Loads with explicit index: iload=0x15, lload=0x16, fload=0x17,
            // dload=0x18, aload=0x19.
            0x15..=0x19 => {
                let i = local_index(self)?;
                let vt = self.load_local(st, i, bci)?;
                self.check_load_kind(op, &vt, bci)?;
                Self::push_val(st, vt);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // xload_<n>: iload_0..3=0x1a-0x1d, lload_0..3=0x1e-0x21,
            // fload_0..3=0x22-0x25, dload_0..3=0x26-0x29, aload_0..3=0x2a-0x2d.
            0x1a..=0x2d => {
                let i = ((op - 0x1a) % 4) as usize;
                let vt = self.load_local(st, i, bci)?;
                self.check_load_kind(op, &vt, bci)?;
                Self::push_val(st, vt);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Array loads: ..., arrayref, index -> value. The result type is
            // fixed by the opcode; arrayref must be an array of the matching
            // element type or null.
            0x2e..=0x35 => {
                self.pop_int(st, bci)?;
                let arr = self.pop_ref(st, bci)?;
                self.check_array_ref(&arr, op, bci)?;
                let result = self.array_load_result(&arr, op);
                Self::push_val(st, result);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Stores with explicit index.
            0x36..=0x3a => {
                let i = local_index(self)?;
                let vt = self.pop_val(st, bci)?;
                self.check_store(store_kind(op), &vt, bci)?;
                self.local_set(st, i, vt, bci)?;
                Ok(Flow::Fallthrough)
            }
            // xstore_<n>: istore_0..3=0x3b-0x3e, lstore=0x3f-0x42,
            // fstore=0x43-0x46, dstore=0x47-0x4a, astore=0x4b-0x4e.
            0x3b..=0x4e => {
                let i = ((op - 0x3b) % 4) as usize;
                let vt = self.pop_val(st, bci)?;
                let kind = store_kind_n(op);
                self.check_store(kind, &vt, bci)?;
                self.local_set(st, i, vt, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Array stores: iastore=0x4f, lastore=0x50, fastore=0x51,
            // dastore=0x52, aastore=0x53, bastore=0x54, castore=0x55,
            // sastore=0x56. Stack: ..., arrayref, index, value.
            0x4f..=0x56 => {
                let value_vt = self.pop_val(st, bci)?;
                self.pop_int(st, bci)?;
                let arr = self.pop_ref(st, bci)?;
                self.check_array_store(&arr, op, &value_vt, bci)?;
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Stack manipulation (ported from HotSpot verifier.cpp
            // `_pop`..`_swap`: the stack stores category-2 values as
            // (primary, 2nd-marker) entry pairs).
            0x57 => {
                // pop
                self.pop_cat1(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x58 => {
                // pop2
                let top = self.pop1(st, bci)?;
                if is_cat1(&top) {
                    self.pop_cat1(st, bci)?;
                } else if top.is_category2_2nd() {
                    self.pop_cat2_primary(st, bci, &top)?;
                } else {
                    return Err(err_at(bci, "pop2 of invalid type"));
                }
                Ok(Flow::Fallthrough)
            }
            0x59 => {
                // dup
                let t = self.pop_cat1(st, bci)?;
                Self::push_entry(st, t.clone());
                Self::push_entry(st, t);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x5a => {
                // dup_x1: ..., v2, v1 -> ..., v1, v2, v1
                let v1 = self.pop_cat1(st, bci)?;
                let v2 = self.pop_cat1(st, bci)?;
                Self::push_entry(st, v1.clone());
                Self::push_entry(st, v2);
                Self::push_entry(st, v1);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x5b => {
                // dup_x2: ..., v3, v2, v1 -> ..., v1, v3, v2, v1
                // where (v2,v3) is either two cat-1 values or one cat-2.
                let v1 = self.pop_cat1(st, bci)?;
                let e2 = self.pop1(st, bci)?;
                if is_cat1(&e2) {
                    let v3 = self.pop_cat1(st, bci)?;
                    Self::push_entry(st, v1.clone());
                    Self::push_entry(st, v3);
                    Self::push_entry(st, e2);
                    Self::push_entry(st, v1);
                } else if e2.is_category2_2nd() {
                    let primary = self.pop_cat2_primary(st, bci, &e2)?;
                    Self::push_entry(st, v1.clone());
                    Self::push_entry(st, primary);
                    Self::push_entry(st, e2);
                    Self::push_entry(st, v1);
                } else {
                    return Err(err_at(bci, "dup_x2 of invalid type"));
                }
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x5c => {
                // dup2: duplicates the top unit — either two cat-1 values
                // or one cat-2 value.
                let unit = self.pop_dup_unit(st, bci)?;
                for v in &unit {
                    Self::push_entry(st, v.clone());
                }
                for v in &unit {
                    Self::push_entry(st, v.clone());
                }
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x5d => {
                // dup2_x1: ..., v3, unit -> ..., unit, v3, unit
                let unit = self.pop_dup_unit(st, bci)?;
                let v3 = self.pop_cat1(st, bci)?;
                for v in &unit {
                    Self::push_entry(st, v.clone());
                }
                Self::push_entry(st, v3);
                for v in &unit {
                    Self::push_entry(st, v.clone());
                }
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x5e => {
                // dup2_x2: ..., unit2, unit1 -> ..., unit1, unit2, unit1
                // (only the top unit is duplicated, inserted below unit2).
                let unit1 = self.pop_dup_unit(st, bci)?;
                let unit2 = self.pop_dup_unit(st, bci)?;
                for v in &unit1 {
                    Self::push_entry(st, v.clone());
                }
                for v in &unit2 {
                    Self::push_entry(st, v.clone());
                }
                for v in &unit1 {
                    Self::push_entry(st, v.clone());
                }
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x5f => {
                // swap
                let v1 = self.pop_cat1(st, bci)?;
                let v2 = self.pop_cat1(st, bci)?;
                Self::push_entry(st, v1);
                Self::push_entry(st, v2);
                Ok(Flow::Fallthrough)
            }
            // Arithmetic and iinc.
            0x60..=0x84 => {
                self.exec_arith(st, bci, op, base, wide)?;
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Conversions.
            0x85..=0x93 => {
                let actual = self.pop_val(st, bci)?;
                // Input type required by each conversion.
                let need = match op {
                    0x85..=0x87 => VT::Integer, // i2l i2f i2d
                    0x88..=0x8a => VT::Long,     // l2i l2f l2d
                    0x8b..=0x8d => VT::Float,    // f2i f2l f2d
                    0x8e..=0x90 => VT::Double,   // d2i d2l d2f
                    0x91..=0x93 => VT::Integer,  // i2b i2c i2s
                    _ => VT::Integer,
                };
                if !self.vt_assignable(&need, &actual) {
                    return Err(err_at(
                        bci,
                        format!("Bad conversion input type {actual:?} for op {op:02x}"),
                    ));
                }
                let result = match op {
                    0x85 => VT::Long,
                    0x86 => VT::Float,
                    0x87 => VT::Double,
                    0x88 => VT::Integer,
                    0x89 => VT::Float,
                    0x8a => VT::Double,
                    0x8b => VT::Integer,
                    0x8c => VT::Long,
                    0x8d => VT::Double,
                    0x8e => VT::Integer,
                    0x8f => VT::Long,
                    0x90 => VT::Float,
                    0x91 | 0x92 | 0x93 => VT::Integer,
                    _ => VT::Integer,
                };
                Self::push_val(st, result);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x94 => {
                // lcmp
                self.pop_expect(st, bci, &VT::Long)?;
                self.pop_expect(st, bci, &VT::Long)?;
                Self::push_val(st, VT::Integer);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x95 | 0x96 => {
                self.pop_expect(st, bci, &VT::Float)?;
                self.pop_expect(st, bci, &VT::Float)?;
                Self::push_val(st, VT::Integer);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x97 | 0x98 => {
                self.pop_expect(st, bci, &VT::Double)?;
                self.pop_expect(st, bci, &VT::Double)?;
                Self::push_val(st, VT::Integer);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Branches.
            0x99..=0x9e => {
                // ifeq..ifle
                self.pop_int(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0x9f..=0xa4 => {
                // if_icmp*
                self.pop_int(st, bci)?;
                self.pop_int(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xa5 | 0xa6 => {
                // if_acmp*
                self.pop_ref(st, bci)?;
                self.pop_ref(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xa7 => Ok(Flow::NoFallthrough), // goto
            0xaa | 0xab => {
                // tableswitch / lookupswitch: always branches.
                self.pop_int(st, bci)?;
                Ok(Flow::NoFallthrough)
            }
            // Returns: the opcode must match the method's declared return
            // type (JVMS 4.10.1.9).
            0xac..=0xb1 => {
                let expected_ret = FieldType::method(self.method.descriptor)
                    .map(|(_, r)| r)
                    .ok_or_else(|| err_at(bci, "Bad method descriptor"))?;
                let ok = match (op, &expected_ret) {
                    // ireturn covers every int-family type: int, boolean,
                    // byte, short, char (JVMS 4.10.1.9).
                    (0xac, FieldType::Prim(b'I' | b'Z' | b'B' | b'C' | b'S'))
                    | (0xad, FieldType::Prim(b'J'))
                    | (0xae, FieldType::Prim(b'F'))
                    | (0xaf, FieldType::Prim(b'D'))
                    | (0xb0, FieldType::Ref(_))
                    | (0xb1, FieldType::Prim(b'V')) => true,
                    _ => false,
                };
                if !ok {
                    return Err(err_at(
                        bci,
                        format!("Return opcode {op:02x} does not match method return type"),
                    ));
                }
                match op {
                    0xac => {
                        self.pop_expect(st, bci, &VT::Integer)?;
                    }
                    0xad => {
                        self.pop_expect(st, bci, &VT::Long)?;
                    }
                    0xae => {
                        self.pop_expect(st, bci, &VT::Float)?;
                    }
                    0xaf => {
                        self.pop_expect(st, bci, &VT::Double)?;
                    }
                    0xb0 => {
                        self.pop_ref(st, bci)?;
                    }
                    _ => {}
                }
                self.require_empty_stack(st, bci)?;
                Ok(Flow::NoFallthrough)
            }
            // Field access.
            0xb2 | 0xb3 | 0xb4 | 0xb5 => {
                let idx = self.u2_at(base);
                match self.constant(idx)? {
                    Constant::FieldRef(_, _) => {}
                    _ => return Err(err_at(bci, "Expected field reference")),
                }
                let (class, _name, desc) = self.ref_target(idx)?;
                let ftype = FieldType::field(desc)
                    .ok_or_else(|| err_at(bci, "Bad field descriptor"))?;
                let vt = ftype.vtype();
                match op {
                    0xb2 => {
                        // getstatic
                        Self::push_val(st, vt);
                    }
                    0xb3 => {
                        // putstatic
                        self.pop_expect(st, bci, &vt)?;
                    }
                    0xb4 => {
                        // getfield
                        let obj = self.pop_ref(st, bci)?;
                        self.check_receiver(&obj, class, bci)?;
                        Self::push_val(st, vt);
                    }
                    0xb5 => {
                        // putfield
                        self.pop_expect(st, bci, &vt)?;
                        let obj = self.pop_ref(st, bci)?;
                        self.check_receiver(&obj, class, bci)?;
                    }
                    _ => unreachable!(),
                }
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            // Invocations.
            0xb6 | 0xb7 | 0xb8 | 0xb9 | 0xba => {
                self.exec_invoke(st, bci, op, base)?;
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xbb => {
                // new
                let idx = self.u2_at(base);
                let name = self.object_type_name(idx)?;
                Self::push_val(st, VT::Uninitialized(bci as u16));
                self.new_classes.borrow_mut().insert(bci, name);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xbc => {
                // newarray
                self.pop_int(st, bci)?;
                let atype = self.u1_at(base);
                let desc = match atype {
                    4 => "[Z",
                    5 => "[C",
                    6 => "[F",
                    7 => "[D",
                    8 => "[B",
                    9 => "[S",
                    10 => "[I",
                    11 => "[J",
                    _ => return Err(err_at(bci, "Illegal newarray type")),
                };
                Self::push_val(st, VT::Object(desc.to_string()));
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xbd => {
                // anewarray
                self.pop_int(st, bci)?;
                let idx = self.u2_at(base);
                let elem = self.class_name(idx)?;
                let desc = if elem.starts_with(b"[") {
                    format!("[{}", String::from_utf8_lossy(elem))
                } else {
                    format!("[L{};", String::from_utf8_lossy(elem))
                };
                Self::push_val(st, VT::Object(desc));
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xbe => {
                // arraylength
                let arr = self.pop_ref(st, bci)?;
                match &arr {
                    VT::Null => {}
                    VT::Object(name) if name.starts_with('[') => {}
                    _ => return Err(err_at(bci, "arraylength on non-array")),
                }
                Self::push_val(st, VT::Integer);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xbf => {
                // athrow
                let ex = self.pop_ref(st, bci)?;
                self.check_throwable(&ex, bci)?;
                Ok(Flow::NoFallthrough)
            }
            0xc0 => {
                // checkcast
                self.pop_ref(st, bci)?;
                let idx = self.u2_at(base);
                let name = self.object_type_name(idx)?;
                Self::push_val(st, VT::Object(name));
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xc1 => {
                // instanceof
                self.pop_ref(st, bci)?;
                let idx = self.u2_at(base);
                let _ = self.object_type_name(idx)?;
                Self::push_val(st, VT::Integer);
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xc2 | 0xc3 => {
                // monitorenter/exit
                self.pop_ref(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xc5 => {
                // multianewarray
                let idx = self.u2_at(base);
                let dims = self.u1_at(base + 2) as usize;
                let class_name = self.class_name(idx)?;
                for _ in 0..dims {
                    self.pop_int(st, bci)?;
                }
                // The class constant name is already an array descriptor
                // like "[[I" or "Ljava/lang/Object;" (JVMS 6.5).
                let cname = String::from_utf8_lossy(class_name).into_owned();
                let desc = if cname.starts_with('[') {
                    cname.clone()
                } else if cname.starts_with('L') {
                    cname.clone()
                } else {
                    format!("L{cname};")
                };
                let have_dims = desc.bytes().take_while(|b| *b == b'[').count();
                if dims > have_dims {
                    return Err(err_at(bci, "multianewarray dimension overflow"));
                }
                Self::push_val(st, VT::Object(desc));
                self.check_stack_depth(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xc6 | 0xc7 => {
                // ifnull / ifnonnull
                self.pop_ref(st, bci)?;
                Ok(Flow::Fallthrough)
            }
            0xc8 => Ok(Flow::NoFallthrough), // goto_w
            _ => Err(err_at(bci, format!("Bad instruction: {op:02x}"))),
        }
    }

    /// Verifies that a value loaded from local `vt` matches the load
    /// instruction `op` (iload/lload/fload/dload/aload families).
    fn check_load_kind(&self, op: u8, vt: &VT, bci: usize) -> io::Result<()> {
        // kind: 0=int, 1=long, 2=float, 3=double, 4=ref
        let kind = if (0x15..=0x19).contains(&op) {
            op - 0x15
        } else {
            (op - 0x1a) / 4
        };
        let ok = match kind {
            0 => matches!(
                vt,
                VT::Integer | VT::Boolean | VT::Byte | VT::Short | VT::Char
            ),
            1 => matches!(vt, VT::Long),
            2 => matches!(vt, VT::Float),
            3 => matches!(vt, VT::Double),
            _ => vt.is_reference(),
        };
        if !ok {
            return Err(err_at(
                bci,
                format!("Bad local type {vt:?} for load opcode {op:02x}"),
            ));
        }
        Ok(())
    }

    /// Accepted element descriptor category for array load/store opcode `op`.
    /// All int-category primitives (B/C/S/Z/I) share the same verification
    /// type, so any of them may be accessed by any int-category array op.
    /// Returns `None` for aaload/aastore (any reference element).
    fn array_elem_category(op: u8) -> Option<&'static [&'static str]> {
        Some(match op {
            0x2e | 0x4f => &["I", "B", "C", "S", "Z"], // iaload / iastore
            0x33 | 0x54 => &["I", "B", "C", "S", "Z"], // baload / bastore
            0x34 | 0x55 => &["I", "B", "C", "S", "Z"], // caload / castore
            0x35 | 0x56 => &["I", "B", "C", "S", "Z"], // saload / sastore
            0x2f | 0x50 => &["J"],                      // laload / lastore
            0x30 | 0x51 => &["F"],                      // faload / fastore
            0x31 | 0x52 => &["D"],                      // daload / dastore
            0x32 | 0x53 => return None,                // aaload / aastore
            _ => return None,
        })
    }

    /// Validates that `arr` is usable as the array operand of array
    /// load/store opcode `op`: null or an array whose element type matches
    /// the opcode.
    fn check_array_operand(&self, arr: &VT, op: u8, bci: usize) -> io::Result<()> {
        match arr {
            VT::Null => return Ok(()),
            VT::Object(name) if name.starts_with('[') => {
                let elem = &name[1..];
                match Self::array_elem_category(op) {
                    Some(expected) => {
                        if !expected.contains(&elem) {
                            return Err(err_at(
                                bci,
                                format!("Array type {name} does not match opcode {op:02x}"),
                            ));
                        }
                    }
                    None => {
                        // aaload/aastore: element must be a reference type.
                        if !(elem.starts_with('L') || elem.starts_with('[')) {
                            return Err(err_at(
                                bci,
                                format!("aaload/aastore on non-reference array {name}"),
                            ));
                        }
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        Err(err_at(
            bci,
            format!("Array instruction on non-array reference {arr:?}"),
        ))
    }

    fn check_array_ref(&self, arr: &VT, op: u8, bci: usize) -> io::Result<()> {
        self.check_array_operand(arr, op, bci)
    }

    fn check_array_store(
        &self,
        arr: &VT,
        op: u8,
        value: &VT,
        bci: usize,
    ) -> io::Result<()> {
        self.check_array_operand(arr, op, bci)?;
        // Value type must match the opcode's element kind.
        let ok = match op {
            0x50 => matches!(value, VT::Long),   // lastore
            0x51 => matches!(value, VT::Float),  // fastore
            0x52 => matches!(value, VT::Double), // dastore
            0x53 => value.is_reference(),        // aastore
            // iastore/bastore/castore/sastore take int-family values.
            _ => matches!(
                value,
                VT::Integer | VT::Boolean | VT::Byte | VT::Short | VT::Char
            ),
        };
        if !ok {
            return Err(err_at(
                bci,
                format!("Bad value type {value:?} for array store opcode {op:02x}"),
            ));
        }
        Ok(())
    }

    /// Result verification type of an array load `op` on arrayref `arr`.
    fn array_load_result(&self, arr: &VT, op: u8) -> VT {
        match op {
            0x2f => VT::Long,
            0x30 => VT::Float,
            0x31 => VT::Double,
            0x2e | 0x33 | 0x34 | 0x35 => VT::Integer,
            0x32 => match arr {
                VT::Object(name) if name.starts_with('[') => {
                    let elem = &name[1..];
                    if elem.starts_with('[') {
                        VT::Object(elem.to_string())
                    } else {
                        VT::Object(normalize_object_name(elem.as_bytes()))
                    }
                }
                _ => VT::Object("java/lang/Object".into()),
            },
            _ => VT::Top,
        }
    }

    fn check_store(&self, kind: StoreKind, vt: &VT, bci: usize) -> io::Result<()> {
        let ok = match kind {
            StoreKind::Int => matches!(
                vt,
                VT::Integer | VT::Boolean | VT::Byte | VT::Short | VT::Char
            ),
            StoreKind::Long => matches!(vt, VT::Long),
            StoreKind::Float => matches!(vt, VT::Float),
            StoreKind::Double => matches!(vt, VT::Double),
            StoreKind::Ref => vt.is_reference(),
        };
        if !ok {
            return Err(err_at(
                bci,
                format!("Bad type {vt:?} for store instruction"),
            ));
        }
        Ok(())
    }

    fn exec_arith(
        &self,
        st: &mut State,
        bci: usize,
        op: u8,
        base: usize,
        wide: bool,
    ) -> io::Result<()> {
        if op == 0x84 {
            // iinc: local index (+ optional const), no stack effect.
            let idx = if wide {
                self.u2_at(base) as usize
            } else {
                self.u1_at(base) as usize
            };
            let vt = self.local_get(st, idx, bci)?;
            if !matches!(
                vt,
                VT::Integer | VT::Boolean | VT::Byte | VT::Short | VT::Char
            ) {
                return Err(err_at(bci, format!("iinc on non-int local {idx}")));
            }
            return Ok(());
        }
        // Binary ops 0x60..=0x83.
        let (need, result) = match op {
            // int binary/unary
            0x60 | 0x64 | 0x68 | 0x6c | 0x70 | 0x7e | 0x80 | 0x82 => {
                (VT::Integer, VT::Integer)
            }
            0x74 => (VT::Integer, VT::Integer), // ineg
            0x78 | 0x7a | 0x7c => (VT::Integer, VT::Integer), // ishl/ishr/iushr
            // long
            0x61 | 0x65 | 0x69 | 0x6d | 0x71 | 0x7f | 0x81 | 0x83 => {
                (VT::Long, VT::Long)
            }
            0x75 => (VT::Long, VT::Long), // lneg
            0x79 | 0x7b | 0x7d => (VT::Long, VT::Long), // lshl/lshr/lushr
            // float
            0x62 | 0x66 | 0x6a | 0x6e | 0x72 => (VT::Float, VT::Float),
            0x76 => (VT::Float, VT::Float), // fneg
            // double
            0x63 | 0x67 | 0x6b | 0x6f | 0x73 => (VT::Double, VT::Double),
            0x77 => (VT::Double, VT::Double), // dneg
            _ => return Err(err_at(bci, format!("Bad arithmetic opcode {op:02x}"))),
        };
        if matches!(op, 0x74 | 0x75 | 0x76 | 0x77) {
            // Unary negation.
            self.pop_expect(st, bci, &need)?;
        } else if matches!(op, 0x78..=0x7d) {
            // Shifts: value then count (count is int).
            self.pop_int(st, bci)?;
            self.pop_expect(st, bci, &need)?;
        } else {
            self.pop_expect(st, bci, &need)?;
            self.pop_expect(st, bci, &need)?;
        }
        Self::push_val(st, result);
        Ok(())
    }

    fn ref_target(&self, index: u16) -> io::Result<(&'a [u8], &'a [u8], &'a [u8])> {
        match self.constant(index)? {
            Constant::FieldRef(class_idx, nt_idx)
            | Constant::MethodRef(class_idx, nt_idx)
            | Constant::InterfaceMethodRef(class_idx, nt_idx) => {
                let class = self.class_name(*class_idx)?;
                let (name, desc) = self.name_and_type(*nt_idx)?;
                Ok((class, name, desc))
            }
            _ => Err(err(format!("Expected reference constant at index {index}"))),
        }
    }

    fn require_empty_stack(&self, st: &State, bci: usize) -> io::Result<()> {
        if !st.stack.is_empty() {
            return Err(err_at(bci, "Stack not empty at return"));
        }
        Ok(())
    }

    fn check_receiver(&self, obj: &VT, class: &[u8], bci: usize) -> io::Result<()> {
        match obj {
            VT::Object(name) => {
                if !self.type_assignable(name.as_bytes(), class) {
                    return Err(err_at(
                        bci,
                        format!("Receiver type {name} not assignable to {}", String::from_utf8_lossy(class)),
                    ));
                }
            }
            VT::Null | VT::UninitializedThis | VT::Uninitialized(_) => {}
            _ => {
                return Err(err_at(bci, "Non-reference receiver"));
            }
        }
        Ok(())
    }

    fn check_throwable(&self, ex: &VT, bci: usize) -> io::Result<()> {
        match ex {
            VT::Object(name) => {
                if !self.type_assignable(name.as_bytes(), b"java/lang/Throwable") {
                    return Err(err_at(
                        bci,
                        format!("athrow of non-Throwable type {name}"),
                    ));
                }
            }
            VT::Null => {}
            _ => {
                return Err(err_at(bci, "athrow of non-reference"));
            }
        }
        Ok(())
    }

    /// Executes an invoke* instruction.
    fn exec_invoke(&self, st: &mut State, bci: usize, op: u8, base: usize) -> io::Result<()> {
        let idx = self.u2_at(base);
        let (class, name, desc): (&'a [u8], &'a [u8], &'a [u8]) = match op {
            0xb6 | 0xb7 | 0xb8 => {
                match self.constant(idx)? {
                    Constant::MethodRef(_, _) => self.ref_target(idx)?,
                    Constant::InterfaceMethodRef(_, _) => self.ref_target(idx)?,
                    _ => return Err(err_at(bci, "Expected method reference")),
                }
            }
            0xb9 => {
                match self.constant(idx)? {
                    Constant::InterfaceMethodRef(_, _) => self.ref_target(idx)?,
                    _ => return Err(err_at(bci, "Expected interface method reference")),
                }
            }
            0xba => {
                match self.constant(idx)? {
                    Constant::InvokeDynamic(_, nt_idx) => {
                        let (n, d) = self.name_and_type(*nt_idx)?;
                        (&b""[..], n, d)
                    }
                    _ => return Err(err_at(bci, "Expected invokedynamic constant")),
                }
            }
            _ => unreachable!(),
        };
        // invokedynamic has two trailing zero bytes.
        if op == 0xba {
            if base + 4 > self.code.len()
                || self.code[base + 2] != 0
                || self.code[base + 3] != 0
            {
                return Err(err_at(bci, "Invalid invokedynamic operands"));
            }
        }
        // invokeinterface: count and zero bytes.
        if op == 0xb9 {
            let count = self.u1_at(base + 2);
            let zero = self.u1_at(base + 3);
            if zero != 0 {
                return Err(err_at(bci, "Invalid invokeinterface operands"));
            }
            // JVMS 4.10.1.9: the count operand includes the receiver and
            // counts stack words, so long/double arguments take two slots.
            if let Some((params, _)) = FieldType::method(desc) {
                let words: usize = params
                    .iter()
                    .map(|t| match t {
                        FieldType::Prim(b'J' | b'D') => 2,
                        _ => 1,
                    })
                    .sum::<usize>()
                    + 1;
                if count as usize != words {
                    return Err(err_at(
                        bci,
                        format!("invokeinterface count {count} != parameter count {words}"),
                    ));
                }
            }
        }

        // JVMS 6.5: <init> may only be invoked via invokespecial;
        // <clinit> may never be invoked by name.
        if name == b"<clinit>" {
            return Err(err_at(bci, "Illegal invocation of <clinit>"));
        }
        if name == b"<init>" && op != 0xb7 {
            return Err(err_at(bci, "<init> can only be invoked with invokespecial"));
        }
        if name == b"<init>" && op == 0xb8 {
            return Err(err_at(bci, "<init> cannot be invoked statically"));
        }

        let (params, ret) =
            FieldType::method(desc).ok_or_else(|| err_at(bci, "Bad method descriptor"))?;

        // Pop arguments in reverse order.
        for p in params.iter().rev() {
            let vt = p.vtype();
            self.pop_expect(st, bci, &vt)?;
        }
        // Receiver.
        let mut recv_vt: Option<VT> = None;
        match op {
            0xb6 | 0xb7 | 0xb9 => {
                let recv = self.pop_ref(st, bci)?;
                if op != 0xba {
                    self.check_receiver(&recv, class, bci)?;
                }
                recv_vt = Some(recv);
            }
            0xb8 | 0xba => {}
            _ => unreachable!(),
        }

        // invokespecial <init>: initialize uninitialized receivers.
        if op == 0xb7 && name == b"<init>" {
            if let Some(recv) = &recv_vt {
                match recv {
                    VT::UninitializedThis => {
                        if st
                            .locals
                            .first()
                            .map(|v| *v == VT::UninitializedThis)
                            .unwrap_or(false)
                        {
                            let self_name =
                                String::from_utf8_lossy(self.class.name).into_owned();
                            st.locals[0] = VT::Object(self_name);
                        }
                    }
                    VT::Uninitialized(new_bci) => {
                        if let Some(cls) = self.new_classes.borrow().get(&(*new_bci as usize)) {
                            let target = VT::Object(cls.clone());
                            for slot in st.locals.iter_mut() {
                                if *slot == VT::Uninitialized(*new_bci) {
                                    *slot = target.clone();
                                }
                            }
                            for slot in st.stack.iter_mut() {
                                if *slot == VT::Uninitialized(*new_bci) {
                                    *slot = target.clone();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Push return value.
        match ret {
            FieldType::Prim(b'V') => {}
            t => {
                let vt = t.vtype();
                Self::push_val(st, vt);
            }
        }
        Ok(())
    }

    // ---------- drivers ----------

    /// Stack-map mode: linear scan over basic blocks. Branch and exception
    /// edges are validated against their target frames at the point the
    /// edge is taken; fall-through predecessors are validated when the
    /// frame bci is reached.
    fn verify_with_frames(&self) -> io::Result<()> {
        let mut st = self.initial_state();
        let mut state_valid = true;
        let mut fell_off = false;
        let mut bci = 0usize;
        while bci < self.code.len() {
            let insn_idx = *self
                .insn_at
                .get(&bci)
                .ok_or_else(|| err(format!("No instruction at bci {bci}")))?;
            let insn = &self.insns[insn_idx];
            let next_bci = bci + insn.len;

            if let Some(frame) = self.frames.get(&bci) {
                if state_valid {
                    self.check_frame_match(&st, frame, bci)?;
                }
                // The frame is authoritative.
                let mut locals = frame.locals.clone();
                locals.resize(self.max_locals, VT::Top);
                st = State {
                    locals,
                    stack: frame.stack.clone(),
                };
                state_valid = true;
            } else if !state_valid {
                bci = next_bci;
                continue;
            }

            if state_valid {
                // Exception edges out of this instruction: each covering
                // handler frame must accept the exception state.
                let pre_locals = st.locals.clone();
                let flow = self.exec(&mut st, bci)?;
                for tr in &self.try_ranges {
                    if bci >= tr.start && bci < tr.end {
                        if let Some(hframe) = self.frames.get(&tr.handler) {
                            let ex_vt = tr
                                .catch
                                .as_ref()
                                .map(|n| VT::Object(String::from_utf8_lossy(n).into_owned()))
                                .unwrap_or_else(|| VT::Object("java/lang/Throwable".into()));
                            let mut exc_stack = State::default();
                            Self::push_val(&mut exc_stack, ex_vt);
                            // Locals not yet written (Top) adopt the
                            // handler frame's declared type: an exception
                            // may be raised before the local's first store,
                            // and the handler frame is authoritative for
                            // what it observes (matches HotSpot, which does
                            // not reject Top incoming locals on exception
                            // edges).
                            let mut exc_locals = pre_locals.clone();
                            for (i, slot) in exc_locals.iter_mut().enumerate() {
                                if *slot == VT::Top {
                                    *slot = hframe.locals.get(i).cloned().unwrap_or(VT::Top);
                                }
                            }
                            let exc_state = State {
                                locals: exc_locals,
                                stack: exc_stack.stack,
                            };
                            self.check_frame_match(&exc_state, hframe, tr.handler)?;
                        }
                    }
                }
                match flow {
                    Flow::Fallthrough => {
                        if next_bci >= self.code.len() {
                            fell_off = true;
                        }
                        bci = next_bci;
                    }
                    Flow::NoFallthrough => {
                        state_valid = false;
                        bci = next_bci;
                    }
                }
                // Validate branch edges against their target frames now.
                if !insn.targets.is_empty() {
                    for &t in &insn.targets {
                        if let Some(tf) = self.frames.get(&t) {
                            self.check_frame_match(&st, tf, t)?;
                        }
                    }
                }
            } else {
                bci = next_bci;
            }
        }
        if fell_off {
            return Err(err("Control flow falls through code end"));
        }
        Ok(())
    }

    /// Checks the propagated state against a declared stack map frame.
    fn check_frame_match(&self, st: &State, frame: &FrameState, bci: usize) -> io::Result<()> {
        for i in 0..self.max_locals {
            let expected = frame.locals.get(i).cloned().unwrap_or(VT::Top);
            let actual = st.locals.get(i).cloned().unwrap_or(VT::Top);
            if !self.vt_assignable(&expected, &actual) {
                return Err(err_at(
                    bci,
                    format!(
                        "Local {i} type mismatch: frame expects {expected:?}, state has {actual:?}"
                    ),
                ));
            }
        }
        if st.stack.len() != frame.stack.len() {
            return Err(err_at(
                bci,
                format!(
                    "Stack height mismatch at frame: state {} vs frame {}",
                    st.stack.len(),
                    frame.stack.len()
                ),
            ));
        }
        for (i, (expected, actual)) in frame.stack.iter().zip(&st.stack).enumerate() {
            if !self.vt_assignable(expected, actual) {
                return Err(err_at(
                    bci,
                    format!(
                        "Stack slot {i} type mismatch: frame expects {expected:?}, state has {actual:?}"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Type-inference mode for class file version < 50.
    fn verify_with_inference(&self) -> io::Result<()> {
        let mut states: HashMap<usize, State> = HashMap::new();
        let mut queue: VecDeque<usize> = VecDeque::new();
        states.insert(0, self.initial_state());
        queue.push_back(0);
        let mut iterations = 0usize;

        while let Some(bci) = queue.pop_front() {
            iterations += 1;
            if iterations > 200_000 {
                return Err(err("Verification dataflow did not converge"));
            }
            let Some(mut st) = states.get(&bci).cloned() else {
                continue;
            };
            let insn_idx = *self
                .insn_at
                .get(&bci)
                .ok_or_else(|| err(format!("No instruction at bci {bci}")))?;
            let insn = &self.insns[insn_idx];
            let next_bci = bci + insn.len;

            // Exception edges.
            let mut successors: Vec<(usize, State)> = Vec::new();
            for tr in &self.try_ranges {
                if bci >= tr.start && bci < tr.end {
                    let ex_vt = tr
                        .catch
                        .as_ref()
                        .map(|n| VT::Object(String::from_utf8_lossy(n).into_owned()))
                        .unwrap_or_else(|| VT::Object("java/lang/Throwable".into()));
                    let mut hst = State {
                        locals: st.locals.clone(),
                        stack: Vec::new(),
                    };
                    Self::push_val(&mut hst, ex_vt);
                    successors.push((tr.handler, hst));
                }
            }

            let flow = self.exec(&mut st, bci)?;
            match flow {
                Flow::Fallthrough => {
                    if next_bci >= self.code.len() {
                        return Err(err("Control flow falls through code end"));
                    }
                    successors.push((next_bci, st.clone()));
                }
                Flow::NoFallthrough => {}
            }
            for &t in &insn.targets {
                successors.push((t, st.clone()));
            }

            for (succ, sst) in successors {
                match states.get_mut(&succ) {
                    Some(existing) => {
                        let merged = merge_states(self, existing, &sst, self.max_locals);
                        if merged != *existing {
                            *existing = merged;
                            queue.push_back(succ);
                        }
                    }
                    None => {
                        let mut s = sst;
                        s.locals.resize(self.max_locals, VT::Top);
                        states.insert(succ, s);
                        queue.push_back(succ);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Category-1 check used by pop/dup/swap: anything that is not a
/// category-2 value or its 2nd marker.
fn is_cat1(vt: &VT) -> bool {
    !vt.is_category2() && !vt.is_category2_2nd()
}

fn merge_states<'m, 'a>(
    v: &BytecodeVerifier<'m, 'a>,
    existing: &mut State,
    incoming: &State,
    max_locals: usize,
) -> State {
    let mut locals = vec![VT::Top; max_locals];
    for i in 0..max_locals {
        let a = existing.locals.get(i).cloned().unwrap_or(VT::Top);
        let b = incoming.locals.get(i).cloned().unwrap_or(VT::Top);
        locals[i] = merge_vt(v, &a, &b);
    }
    let n = existing.stack.len().max(incoming.stack.len());
    let mut stack = Vec::with_capacity(n);
    for i in 0..n {
        let a = existing.stack.get(i).cloned().unwrap_or(VT::Top);
        let b = incoming.stack.get(i).cloned().unwrap_or(VT::Top);
        stack.push(merge_vt(v, &a, &b));
    }
    State { locals, stack }
}

/// Nearest common superclass of `x` and `y` (single-inheritance chain of `x`
/// walked upward; the first ancestor that `y` is assignable to wins).
fn common_superclass<'m, 'a>(
    v: &BytecodeVerifier<'m, 'a>,
    x: &str,
    y: &str,
) -> Option<String> {
    let mut current = v.klass_info(x.as_bytes())?.super_name?;
    loop {
        let name = String::from_utf8_lossy(&current).into_owned();
        if v.type_assignable(y.as_bytes(), &current) {
            return Some(name);
        }
        current = v.klass_info(&current)?.super_name?;
    }
}

/// Merges two verification types at a control-flow join.
fn merge_vt<'m, 'a>(v: &BytecodeVerifier<'m, 'a>, a: &VT, b: &VT) -> VT {
    use VT::*;
    if a == b {
        return a.clone();
    }
    match (a, b) {
        // Top is the top of the verification lattice: merging an unusable
        // value with anything yields unusable.
        (Top, _) | (_, Top) => Top,
        (Null, Object(o)) | (Object(o), Null) => Object(o.clone()),
        (Null, UninitializedThis) | (UninitializedThis, Null) => UninitializedThis,
        (Integer, Integer)
        | (Integer, Boolean)
        | (Integer, Byte)
        | (Integer, Short)
        | (Integer, Char)
        | (Boolean, Integer)
        | (Byte, Integer)
        | (Short, Integer)
        | (Char, Integer)
        | (Boolean, Boolean)
        | (Byte, Byte)
        | (Short, Short)
        | (Char, Char) => Integer,
        (Object(x), Object(y)) => {
            if x == y {
                return Object(x.clone());
            }
            // Least upper bound: if one side is assignable to the other,
            // the supertype wins (covers subclass/interface merges like
            // List + ArrayList -> List).
            if v.type_assignable(x.as_bytes(), y.as_bytes()) {
                return Object(y.clone());
            }
            if v.type_assignable(y.as_bytes(), x.as_bytes()) {
                return Object(x.clone());
            }
            // Unrelated classes: nearest common superclass.
            if let Some(lub) = common_superclass(v, x, y) {
                return Object(lub);
            }
            Object("java/lang/Object".into())
        }
        (Uninitialized(x), Uninitialized(y)) if x == y => Uninitialized(*x),
        (UninitializedThis, UninitializedThis) => UninitializedThis,
        (Long, Long) => Long,
        (Long2nd, Long2nd) => Long2nd,
        (Float, Float) => Float,
        (Double, Double) => Double,
        (Double2nd, Double2nd) => Double2nd,
        _ => Top,
    }
}

/// Store instruction kind for the indexed forms 0x36..=0x3a.
fn store_kind(op: u8) -> StoreKind {
    match op {
        0x36 => StoreKind::Int,
        0x37 => StoreKind::Long,
        0x38 => StoreKind::Float,
        0x39 => StoreKind::Double,
        _ => StoreKind::Ref,
    }
}

/// Store instruction kind for the _n forms 0x3b..=0x4e.
fn store_kind_n(op: u8) -> StoreKind {
    match (op - 0x3b) / 4 {
        0 => StoreKind::Int,
        1 => StoreKind::Long,
        2 => StoreKind::Float,
        3 => StoreKind::Double,
        _ => StoreKind::Ref,
    }
}

/// Normalizes a `CONSTANT_Class` name to the internal form used in
/// `VT::Object`: strips the `L...;` wrapper for plain classes, keeps array
/// descriptors.
fn normalize_object_name(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw).into_owned();
    if s.starts_with('[') {
        return s;
    }
    if let Some(inner) = s.strip_prefix('L').and_then(|r| r.strip_suffix(';')) {
        return inner.to_string();
    }
    s
}

fn expand_verification_type(vt: VT) -> (VT, Vec<VT>) {
    match vt {
        VT::Long => (VT::Long, vec![VT::Long2nd]),
        VT::Double => (VT::Double, vec![VT::Double2nd]),
        other => (other, Vec::new()),
    }
}

fn utf8_of<'a>(constants: &[Constant<'a>], index: u16) -> io::Result<&'a [u8]> {
    match constants.get(index as usize) {
        Some(Constant::Utf8(bytes)) => Ok(bytes),
        _ => Err(err(format!("Invalid constant pool index {index}"))),
    }
}

fn class_name_of<'a>(constants: &[Constant<'a>], index: u16) -> io::Result<&'a [u8]> {
    match constants.get(index as usize) {
        Some(Constant::Class(name_idx)) => utf8_of(constants, *name_idx),
        _ => Err(err(format!("Expected CONSTANT_Class at index {index}"))),
    }
}

fn read_u2(bytes: &[u8], pos: &mut usize) -> io::Result<u16> {
    let b = bytes
        .get(*pos..*pos + 2)
        .ok_or_else(|| err("StackMapTable truncated"))?;
    *pos += 2;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn err(msg: impl Into<String>) -> Error {
    Error::other(format!("VerifyError: {}", msg.into()))
}

fn err_at(bci: usize, msg: impl Into<String>) -> Error {
    Error::other(format!(
        "VerifyError at bytecode offset {bci}: {}",
        msg.into()
    ))
}

/// Public entry point used by `Verifier::verify_class`.
pub fn verify_method_code<'m, 'a>(
    class: &'m ClassFile<'a>,
    method: &'m Member<'a>,
    loader: Option<&'m ClassLoader<'a>>,
) -> io::Result<()> {
    BytecodeVerifier::verify(class, method, loader).map_err(|error| {
        Error::other(format!(
            "{}.{}{}: {}",
            String::from_utf8_lossy(class.name),
            String::from_utf8_lossy(method.name),
            String::from_utf8_lossy(method.descriptor),
            error
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::classfile::classFileParser::Attribute;

    /// Builds a minimal ClassFile with a single static method whose Code
    /// attribute wraps `code` (plus optional extra code sub-attributes).
    fn class_with_method(
        name: &'static [u8],
        desc: &'static [u8],
        major: u16,
        max_stack: u16,
        max_locals: u16,
        code: &'static [u8],
        sub_attrs: &[(u16, &[u8])],
        extra_constants: Vec<Constant<'static>>,
    ) -> (ClassFile<'static>, Member<'static>) {
        // Code attribute: max_stack, max_locals, code_length, code,
        // exception_table_length=0, attributes.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&max_stack.to_be_bytes());
        bytes.extend_from_slice(&max_locals.to_be_bytes());
        bytes.extend_from_slice(&(code.len() as u32).to_be_bytes());
        bytes.extend_from_slice(code);
        bytes.extend_from_slice(&0u16.to_be_bytes()); // exception table
        bytes.extend_from_slice(&(sub_attrs.len() as u16).to_be_bytes());
        for (name_idx, info) in sub_attrs {
            bytes.extend_from_slice(&name_idx.to_be_bytes());
            bytes.extend_from_slice(&(info.len() as u32).to_be_bytes());
            bytes.extend_from_slice(info);
        }
        let code_bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());

        let mut constants = vec![Constant::Unusable];
        constants.extend(extra_constants);
        // StackMapTable name index is expected at cp index 1 when used.
        let class = ClassFile {
            majorVersion: major,
            minorVersion: 0,
            accessFlags: 0x0021, // ACC_PUBLIC | ACC_SUPER
            name,
            superName: Some(b"java/lang/Object"),
            interfaces: Vec::new(),
            constants,
            fields: Vec::new(),
            methods: Vec::new(),
            attributes: Vec::new(),
        };
        let method = Member {
            accessFlags: 0x0009, // ACC_PUBLIC | ACC_STATIC
            name: b"m",
            descriptor: desc,
            attributes: vec![Attribute {
                name: b"Code",
                bytes: code_bytes,
            }],
        };
        (class, method)
    }

    fn verify(
        name: &'static [u8],
        desc: &'static [u8],
        major: u16,
        max_stack: u16,
        max_locals: u16,
        code: &'static [u8],
        sub_attrs: &[(u16, &[u8])],
        extra_constants: Vec<Constant<'static>>,
    ) -> io::Result<()> {
        let (class, method) = class_with_method(
            name,
            desc,
            major,
            max_stack,
            max_locals,
            code,
            sub_attrs,
            extra_constants,
        );
        verify_method_code(&class, &method, None)
    }

    const MAJOR_52: u16 = 52;

    #[test]
    fn accepts_simple_return() {
        // static void m() { return; }
        assert!(verify(b"T", b"()V", 45, 0, 0, &[0xb1], &[], vec![]).is_ok());
    }

    #[test]
    fn accepts_int_return() {
        // static int m() { iconst_5; ireturn; }
        assert!(verify(b"T", b"()I", 45, 1, 0, &[0x08, 0xac], &[], vec![]).is_ok());
    }

    #[test]
    fn accepts_boolean_return() {
        // static boolean m(String) { ...; ireturn; }
        assert!(verify(b"T", b"(Ljava/lang/String;)Z", 45, 1, 1, &[0x03, 0xac], &[], vec![]).is_ok());
    }

    #[test]
    fn rejects_stack_underflow() {
        // static int m() { ireturn; } — nothing on stack.
        assert!(verify(b"T", b"()I", 45, 0, 0, &[0xac], &[], vec![]).is_err());
    }

    #[test]
    fn rejects_wrong_return_type() {
        // static void m() { ireturn; } — ireturn in a void method.
        assert!(verify(b"T", b"()V", 45, 0, 0, &[0xac], &[], vec![]).is_err());
        // static int m() { return; } — return in an int method.
        assert!(verify(b"T", b"()I", 45, 0, 0, &[0xb1], &[], vec![]).is_err());
    }

    #[test]
    fn rejects_illegal_opcode() {
        // 0xca is the reserved breakpoint opcode.
        assert!(verify(b"T", b"()V", 45, 0, 0, &[0xca], &[], vec![]).is_err());
    }

    #[test]
    fn rejects_jsr_ret() {
        // jsr is not part of the class file format since Java 6.
        assert!(verify(b"T", b"()V", 45, 0, 0, &[0xa8, 0x00, 0x00], &[], vec![]).is_err());
        assert!(verify(b"T", b"()V", 45, 0, 1, &[0xa9, 0x00], &[], vec![]).is_err());
    }

    #[test]
    fn rejects_branch_into_instruction_middle() {
        // iconst_0; goto +1 lands on the second byte of `return`'s slot —
        // craft: bci0 iconst_0, bci1 goto -> bci3 (middle of nop+return).
        // bytes: 0x03 (iconst_0), 0xa7 0x00 0x01 (goto +1 -> bci4?) use
        // explicit: bci1 goto offset 2 -> target bci 1+2=3 which is inside
        // the 2-byte `bipush`? simpler: nop(1B) then ireturn needs stack.
        // Use: 0x00 (nop) at 0; 0xa7 0x00 0x01 at 1 -> target 3; byte 2 is
        // 0x10 (bipush) operand byte at 3 -> target lands on operand.
        let code: &[u8] = &[0x00, 0xa7, 0x00, 0x01, 0x10, 0x00, 0xb1];
        // target = 1 + 1 = 2? goto operand is s2 relative to opcode bci:
        // target = bci + off = 1 + 1 = 2 -> byte 2 is 0x00 (inside nop? no,
        // bci2 = 0x00 which IS an instruction boundary (nop is 1 byte at 0,
        // goto at 1-3, so bci4=0x10...). Recount: indices 0:nop,1:goto,2-3:off,
        // 4:bipush,5:imm,6:return. goto off bytes at 2,3 = 0x0001 -> target=1+1=2
        // which is the operand byte -> not a boundary -> VerifyError.
        assert!(verify(b"T", b"()V", MAJOR_52, 0, 0, code, &[], vec![]).is_err());
    }

    #[test]
    fn rejects_astore_of_int() {
        // static void m() { iconst_0; astore_0; return; } — astore needs ref.
        // max_locals=1. astore_0 = 0x4b.
        let code: &[u8] = &[0x03, 0x4b, 0xb1];
        assert!(verify(b"T", b"()V", MAJOR_52, 0, 1, code, &[], vec![]).is_err());
    }

    #[test]
    fn rejects_lload_of_int_local() {
        // static void m(int) { lload_0; pop2; return; } — local 0 is int.
        let code: &[u8] = &[0x1e, 0x58, 0xb1];
        assert!(verify(b"T", b"(I)V", MAJOR_52, 2, 1, code, &[], vec![]).is_err());
    }

    #[test]
    fn rejects_athrow_of_int() {
        // static void m() { iconst_0; athrow; }
        let code: &[u8] = &[0x03, 0xbf];
        assert!(verify(b"T", b"()V", MAJOR_52, 0, 0, code, &[], vec![]).is_err());
    }

    #[test]
    fn rejects_fallthrough_past_end() {
        // static void m() { nop; } — falls off the end.
        assert!(verify(b"T", b"()V", 45, 0, 0, &[0x00], &[], vec![]).is_err());
    }

    #[test]
    fn rejects_missing_stackmap_frame() {
        // v52: goto creates a branch target that requires a frame; none given.
        // static void m() { goto m; } infinite loop, no StackMapTable.
        let code: &[u8] = &[0xa7, 0xff, 0xfe]; // goto -2 -> bci 0
        assert!(verify(b"T", b"()V", MAJOR_52, 0, 0, code, &[], vec![]).is_err());
    }

    #[test]
    fn accepts_valid_stackmap_frame() {
        // static void m() {
        //   iconst_0;        // bci 0
        //   ifeq +4 -> bci 6;// bci 1 (09 00 04)
        //   nop;             // bci 4
        //   return;          // bci 5  -- wait ifeq target must be insn.
        // }
        // Layout: 0:iconst_0(0x03), 1:ifeq(0x99) -> target 1+4=5,
        // 4:nop(0x00), 5:return(0xb1).
        let smt: &[u8] = &[0x00, 0x01, 0xff, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00];
        let utf8 = Constant::Utf8(b"StackMapTable");
        let code: &[u8] = &[0x03, 0x99, 0x00, 0x04, 0x00, 0xb1];
        let r = verify(
            b"T",
            b"()V",
            MAJOR_52,
            0,
            0,
            code,
            &[(1, smt)],
            vec![utf8],
        );
        assert!(r.is_ok(), "expected ok, got {:?}", r.err());
    }

    #[test]
    fn rejects_stackmap_at_non_boundary() {
        // Same code but frame offset points into the middle of ifeq.
        let smt: &[u8] = &[0x00, 0x01, 0xff, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00];
        let utf8 = Constant::Utf8(b"StackMapTable");
        let code: &[u8] = &[0x03, 0x99, 0x00, 0x04, 0x00, 0xb1];
        assert!(verify(
            b"T",
            b"()V",
            MAJOR_52,
            0,
            0,
            code,
            &[(1, smt)],
            vec![utf8],
        )
        .is_err());
    }

    #[test]
    fn rejects_bad_exception_table() {
        // Exception entry with start_pc >= end_pc.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u16.to_be_bytes()); // max_stack
        bytes.extend_from_slice(&0u16.to_be_bytes()); // max_locals
        let code: &[u8] = &[0x03, 0x57, 0xb1];
        bytes.extend_from_slice(&(code.len() as u32).to_be_bytes());
        bytes.extend_from_slice(code);
        bytes.extend_from_slice(&1u16.to_be_bytes()); // 1 exception entry
        bytes.extend_from_slice(&2u16.to_be_bytes()); // start_pc=2
        bytes.extend_from_slice(&1u16.to_be_bytes()); // end_pc=1  (start>end)
        bytes.extend_from_slice(&3u16.to_be_bytes()); // handler_pc
        bytes.extend_from_slice(&0u16.to_be_bytes()); // catch_type=0
        bytes.extend_from_slice(&0u16.to_be_bytes()); // attr_count=0
        let code_bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let class = ClassFile {
            majorVersion: 45,
            minorVersion: 0,
            accessFlags: 0x0021,
            name: b"T",
            superName: Some(b"java/lang/Object"),
            interfaces: Vec::new(),
            constants: vec![Constant::Unusable],
            fields: Vec::new(),
            methods: Vec::new(),
            attributes: Vec::new(),
        };
        let method = Member {
            accessFlags: 0x0009,
            name: b"m",
            descriptor: b"()V",
            attributes: vec![Attribute {
                name: b"Code",
                bytes: code_bytes,
            }],
        };
        assert!(verify_method_code(&class, &method, None).is_err());
    }

    #[test]
    fn rejects_tableswitch_bad_range() {
        // tableswitch with low > high.
        let mut code = vec![0xaa, 0x00, 0x00, 0x00];
        code.extend_from_slice(&0i32.to_be_bytes()); // default
        code.extend_from_slice(&5i32.to_be_bytes()); // low
        code.extend_from_slice(&2i32.to_be_bytes()); // high (< low)
        code.extend_from_slice(&[0xb1]);
        let leaked: &'static [u8] = Box::leak(code.into_boxed_slice());
        assert!(verify(b"T", b"()V", 45, 0, 0, leaked, &[], vec![]).is_err());
    }

    #[test]
    fn rejects_pop_underflow() {
        // static void m() { pop; return; }
        assert!(verify(b"T", b"()V", 45, 0, 0, &[0x57, 0xb1], &[], vec![]).is_err());
    }

    #[test]
    fn rejects_invokeinterface_bad_count() {
        // invokeinterface with count != parameter count.
        // Needs an InterfaceMethodref constant; build minimal cp:
        // 1: Utf8 "I", 2: Utf8 "m", 3: Utf8 "()V", 4: NameAndType(1,3),
        // 5: Utf8 "I2", 6: Class(5), 7: InterfaceMethodRef(6,4)
        let code: &[u8] = &[0xb9, 0x00, 0x07, 0x05, 0x00, 0xb1];
        let constants = vec![
            Constant::Unusable,
            Constant::Utf8(b"I"),
            Constant::Utf8(b"m"),
            Constant::Utf8(b"()V"),
            Constant::NameAndType(1, 3),
            Constant::Utf8(b"I2"),
            Constant::Class(5),
            Constant::InterfaceMethodRef(6, 4),
        ];
        // count=5 but method takes 0 params -> error.
        assert!(verify(
            b"T",
            b"()V",
            MAJOR_52,
            0,
            0,
            code,
            &[],
            constants,
        )
        .is_err());
    }

    #[test]
    fn rejects_bad_methodref_constant() {
        // invokevirtual referencing a CONSTANT_Class instead of Methodref.
        let code: &[u8] = &[0xb6, 0x00, 0x01, 0xb1];
        let constants = vec![Constant::Unusable, Constant::Utf8(b"X")];
        // cp[1] is Utf8, not even Class/Methodref.
        assert!(verify(
            b"T",
            b"()V",
            MAJOR_52,
            0,
            0,
            code,
            &[],
            constants,
        )
        .is_err());
    }
}
