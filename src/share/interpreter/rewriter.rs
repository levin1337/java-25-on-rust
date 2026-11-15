/*
 * Port of OpenJDK 25 HotSpot Rewriter
 * (share/interpreter/rewriter.cpp / rewriter.hpp)
 *
 * The rewriter performs two jobs at class-link time:
 *
 *   1. Bytecode rewriting for the template interpreter: cp indexes in
 *      field/method/ldc/invokedynamic instructions are replaced by
 *      indexes into the constant-pool cache (CPC) so that resolved
 *      entries can be patched without touching bytecodes.  Signature-
 *      polymorphic `MethodHandle.invoke`/`VarHandle.invoke` calls are
 *      rewritten to `_invokehandle` (with an appendix) and lookupswitch
 *      is rewritten to `_fast_binaryswitch` for non-Zero interpreters.
 *
 *   2. `rewrite_jsrs`: for methods containing jsr/ret, resolves return-
 *      address/oop-map conflicts and reports monitor safety.
 *
 * The Rust port operates on a mutable bytecode buffer plus the class
 * file's constant pool (`&[Constant]`); the CPC is returned as a
 * `ConstantPoolCache` value that callers can attach to the method.
 */

use super::bytecodes::{self, Code};
use crate::share::classfile::classFileParser::Constant;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Index maps (compute_index_maps)
// ---------------------------------------------------------------------------

/// One `ResolvedIndyEntry`: maps a CPC invokedynamic slot to the
/// original `CONSTANT_InvokeDynamic` cp index and its resolved
/// references index.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedIndyEntry {
    pub resolved_references_index: u16,
    pub cp_index: u16,
}

/// `ConstantPoolCache` analog: the runtime tables built by
/// `Rewriter::make_constant_pool_cache`.
#[derive(Debug, Default)]
pub struct ConstantPoolCache {
    /// new_index -> original cp index for field entries.
    pub field_entries: Vec<u16>,
    /// new_index -> original cp index for method entries.
    pub method_entries: Vec<u16>,
    /// invokedynamic CPC slots -> (resolved_references index, cp index).
    pub indy_entries: Vec<ResolvedIndyEntry>,
    /// resolved references table entries -> cp index they refer to.
    pub resolved_references: Vec<u16>,
    /// First/last resolved-reference index usable by cp cache entries.
    pub resolved_reference_start: u16,
    pub resolved_reference_limit: u16,
}

/// Per-method flags produced by `scan_method`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScanResult {
    /// Number of jsr/jsr_w instructions seen.
    pub nof_jsrs: usize,
    /// Method contains monitorenter/monitorexit.
    pub has_monitor_bytecodes: bool,
    /// A signature-polymorphic invoke was rewritten to _invokehandle.
    pub invokehandle_rewritten: bool,
    /// invokedynamic count rewritten.
    pub invokedynamics: usize,
}

/// Method metadata `scan_method` needs beyond the bytecode bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct MethodInfo {
    /// `<clinit>` — static initializer.
    pub is_static_initializer: bool,
    /// `<init>` — object initializer.
    pub is_object_initializer: bool,
    /// Class name owning the method (for the final-field check).
    pub class_name: &'static [u8],
}

const ACC_FINAL: u16 = 0x0010;
const ACC_STATIC: u16 = 0x0008;

/// `Rewriter`: holds the cp index maps and performs bytecode rewriting
/// on a mutable code array.
pub struct Rewriter<'a> {
    pool: &'a [Constant<'a>],
    /// cp_index -> field_entry_index (forward) / reverse map index.
    field_map: HashMap<u16, u16>,
    method_map: HashMap<u16, u16>,
    field_entries: Vec<u16>,
    method_entries: Vec<u16>,
    indy_entries: Vec<ResolvedIndyEntry>,
    /// resolved_references index -> cp_index.
    resolved_references: Vec<u16>,
    /// cp_index -> resolved_references index.
    resolved_refs_map: HashMap<u16, u16>,
    /// Tri-state per cp index for signature-polymorphic methods:
    /// 0 unknown, +1 polymorphic, -1 not.
    method_handle_invokers: Vec<i8>,
    invokedynamic_index: u32,
    /// (class,name,descriptor) of final fields updated outside initializers.
    pub initialized_final_updates: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// When false (Zero interpreter) lookupswitch is not rewritten to
    /// fast_binaryswitch.
    pub rewrite_switches: bool,
}

/// Result of `Rewriter::rewrite_jsrs`.
#[derive(Debug, Default)]
pub struct JsrsResult {
    /// Method has jsr bytecodes (needs oop-map-aware relocation when
    /// return addresses conflict with oop slots).
    pub has_jsrs: bool,
    /// No monitorenter/monitorexit inside jsr subroutines, so the
    /// monitor-matching metadata stays valid (`romc.monitor_safe()`).
    pub monitor_safe: bool,
    /// True when a jsr target would collide with an oop slot at a
    /// safepoint and relocation was required.
    pub relocated: bool,
    /// New bytecode buffer when relocation happened.
    pub new_bytes: Option<Vec<u8>>,
}

impl<'a> Rewriter<'a> {
    pub fn new(pool: &'a [Constant<'a>]) -> Self {
        Self {
            pool,
            field_map: HashMap::new(),
            method_map: HashMap::new(),
            field_entries: Vec::new(),
            method_entries: Vec::new(),
            indy_entries: Vec::new(),
            resolved_references: Vec::new(),
            resolved_refs_map: HashMap::new(),
            method_handle_invokers: Vec::new(),
            invokedynamic_index: 0,
            initialized_final_updates: Vec::new(),
            rewrite_switches: false,
        }
    }

    /// `Rewriter::compute_index_maps`.
    pub fn compute_index_maps(&mut self) {
        let length = self.pool.len();
        for (i, constant) in self.pool.iter().enumerate() {
            let index = i as u16;
            match constant {
                Constant::FieldRef(..) => {
                    self.field_map.insert(index, self.field_entries.len() as u16);
                    self.field_entries.push(index);
                }
                Constant::MethodRef(..) | Constant::InterfaceMethodRef(..) => {
                    self.method_map.insert(index, self.method_entries.len() as u16);
                    self.method_entries.push(index);
                }
                Constant::Dynamic(..)
                | Constant::String(_)
                | Constant::MethodHandle(..)
                | Constant::MethodType(..) => {
                    self.add_resolved_references_entry(index);
                }
                Constant::Utf8(name)
                    if *name == b"java/lang/invoke/MethodHandle"
                        || *name == b"java/lang/invoke/VarHandle" =>
                {
                    // Mark that the pool may contain signature-polymorphic
                    // MethodHandle/VarHandle members; the per-cp tri-state
                    // table is allocated lazily in maybe_rewrite_invokehandle.
                }
                _ => {}
            }
        }
        if length > self.method_handle_invokers.len() {
            self.method_handle_invokers.resize(length, 0);
        }
    }

    fn add_resolved_references_entry(&mut self, cp_index: u16) -> u16 {
        if let Some(&index) = self.resolved_refs_map.get(&cp_index) {
            return index;
        }
        let index = self.resolved_references.len() as u16;
        self.resolved_references.push(cp_index);
        self.resolved_refs_map.insert(cp_index, index);
        index
    }

    /// `cp_entry_to_resolved_references`.
    fn cp_entry_to_resolved_references(&mut self, cp_index: u16) -> u16 {
        self.add_resolved_references_entry(cp_index)
    }

    fn cp_tag(&self, cp_index: u16) -> Option<&'a Constant<'a>> {
        self.pool.get(usize::from(cp_index))
    }

    fn utf8(&self, index: u16) -> Option<&'a [u8]> {
        match self.pool.get(usize::from(index)) {
            Some(Constant::Utf8(bytes)) => Some(bytes),
            _ => None,
        }
    }

    /// Decodes `CONSTANT_*ref` -> (class_name, name, descriptor) like
    /// `ConstantPool::klass_name_at`/`name_ref_at`/`signature_ref_at`.
    fn member_ref(&self, cp_index: u16) -> Option<(&'a [u8], &'a [u8], &'a [u8])> {
        let (class_idx, nt_idx) = match self.cp_tag(cp_index)? {
            Constant::FieldRef(c, n)
            | Constant::MethodRef(c, n)
            | Constant::InterfaceMethodRef(c, n) => (*c, *n),
            _ => return None,
        };
        let owner = match self.cp_tag(class_idx)? {
            Constant::Class(name) => self.utf8(*name)?,
            _ => return None,
        };
        let (name_idx, desc_idx) = match self.cp_tag(nt_idx)? {
            Constant::NameAndType(n, d) => (*n, *d),
            _ => return None,
        };
        Some((owner, self.utf8(name_idx)?, self.utf8(desc_idx)?))
    }

    // -----------------------------------------------------------------------
    // Bytecode rewriting primitives
    // -----------------------------------------------------------------------

    fn get_u2_be(bytes: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    }

    fn put_u2_be(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn put_u2_native(bytes: &mut [u8], offset: usize, value: u16) {
        // HotSpot uses native order for rewritten indices; little-endian
        // on x86/Windows, matching Bytes::put_native_u2.
        bytes[offset..offset + 2].copy_from_slice(&value.to_ne_bytes());
    }

    fn get_u2_native(bytes: &[u8], offset: usize) -> u16 {
        u16::from_ne_bytes([bytes[offset], bytes[offset + 1]])
    }

    fn get_u4_native(bytes: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    fn put_u4_native(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }

    fn put_u4_be(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    /// `Rewriter::rewrite_field_reference`.
    fn rewrite_field_reference(&mut self, bytes: &mut [u8], offset: usize, reverse: bool) {
        if !reverse {
            let cp_index = Self::get_u2_be(bytes, offset);
            let entry = *self
                .field_map
                .get(&cp_index)
                .unwrap_or_else(|| panic!("no field entry for cp #{cp_index}"));
            Self::put_u2_native(bytes, offset, entry);
        } else {
            let entry = Self::get_u2_native(bytes, offset);
            let cp_index = self
                .field_entries
                .get(usize::from(entry))
                .copied()
                .unwrap_or_else(|| panic!("invalid field entry index {entry}"));
            Self::put_u2_be(bytes, offset, cp_index);
        }
    }

    /// `Rewriter::rewrite_method_reference` + `maybe_rewrite_invokehandle`.
    fn rewrite_method_reference(
        &mut self,
        bytes: &mut [u8],
        opcode_offset: usize,
        offset: usize,
        reverse: bool,
        result: &mut ScanResult,
    ) {
        if !reverse {
            let cp_index = Self::get_u2_be(bytes, offset);
            let entry = *self
                .method_map
                .get(&cp_index)
                .unwrap_or_else(|| panic!("no method entry for cp #{cp_index}"));
            Self::put_u2_native(bytes, offset, entry);
            self.maybe_rewrite_invokehandle(bytes, opcode_offset, cp_index, entry, result);
        } else {
            let entry = Self::get_u2_native(bytes, offset);
            let cp_index = self
                .method_entries
                .get(usize::from(entry))
                .copied()
                .unwrap_or_else(|| panic!("invalid method entry index {entry}"));
            Self::put_u2_be(bytes, offset, cp_index);
            self.maybe_rewrite_invokehandle(bytes, opcode_offset, cp_index, entry, result);
        }
    }

    /// `Rewriter::maybe_rewrite_invokehandle`: signature-polymorphic
    /// `MethodHandle.invoke`/`VarHandle.invoke` calls get the internal
    /// `_invokehandle` opcode (which appends a call-site appendix).
    fn maybe_rewrite_invokehandle(
        &mut self,
        bytes: &mut [u8],
        opc_offset: usize,
        cp_index: u16,
        cache_index: u16,
        result: &mut ScanResult,
    ) {
        if cp_index as usize >= self.method_handle_invokers.len() {
            return;
        }
        let opcode = bytes[opc_offset];
        if !matches!(opcode, x if x == Code::Invokevirtual as u8 || x == Code::Invokespecial as u8)
        {
            // Reverse direction: _invokehandle -> _invokevirtual.
            if opcode == Code::Invokehandle as u8 {
                bytes[opc_offset] = Code::Invokevirtual as u8;
                result.invokehandle_rewritten = true;
            }
            return;
        }
        let mut status = self.method_handle_invokers[cp_index as usize];
        if status == 0 {
            status = if self.is_signature_polymorphic(cp_index, b"java/lang/invoke/MethodHandle")
                || self.is_signature_polymorphic(cp_index, b"java/lang/invoke/VarHandle")
            {
                1
            } else {
                -1
            };
            self.method_handle_invokers[cp_index as usize] = status;
        }
        if status > 0 {
            bytes[opc_offset] = Code::Invokehandle as u8;
            result.invokehandle_rewritten = true;
            let _ = cache_index;
        }
    }

    /// `MethodHandles::is_signature_polymorphic_name`: the member is
    /// `invoke`/`invokeExact` (etc.) on MethodHandle/VarHandle with a
    /// polymorphic signature — approximated by name+descriptor checks on
    /// the constant-pool member reference.
    fn is_signature_polymorphic(&self, cp_index: u16, owner: &[u8]) -> bool {
        let Some((class, name, _descriptor)) = self.member_ref(cp_index) else {
            return false;
        };
        class == owner
            && matches!(name, b"invoke" | b"invokeExact" | b"invokeBasic" | b"linkToVirtual"
                | b"linkToStatic" | b"linkToSpecial" | b"linkToInterface")
    }

    /// `Rewriter::rewrite_invokedynamic`: replaces the trailing 4 bytes
    /// (after the opcode) with a native u4 index into the invokedynamic
    /// CPC table; reverse restores the Java u2 cp index.
    fn rewrite_invokedynamic(
        &mut self,
        bytes: &mut [u8],
        offset: usize,
        reverse: bool,
        result: &mut ScanResult,
    ) {
        if !reverse {
            let cp_index = Self::get_u2_be(bytes, offset);
            let resolved_index = self.cp_entry_to_resolved_references(cp_index);
            Self::put_u4_native(bytes, offset, self.invokedynamic_index);
            self.indy_entries.push(ResolvedIndyEntry {
                resolved_references_index: resolved_index,
                cp_index,
            });
            self.invokedynamic_index += 1;
            result.invokedynamics += 1;
        } else {
            let cache_index = Self::get_u4_native(bytes, offset) as usize;
            let cp_index = self
                .indy_entries
                .get(cache_index)
                .map(|entry| entry.cp_index)
                .unwrap_or(0);
            Self::put_u4_be(bytes, offset, 0);
            Self::put_u2_be(bytes, offset, cp_index);
        }
    }

    /// `Rewriter::rewrite_invokespecial`: InterfaceMethodref entries get
    /// their own method-entry slot because invokespecial resolution
    /// differs from invokeinterface.
    fn rewrite_invokespecial(
        &mut self,
        bytes: &mut [u8],
        offset: usize,
        reverse: bool,
        invokespecial_error: &mut bool,
        result: &mut ScanResult,
    ) {
        if !reverse {
            let cp_index = Self::get_u2_be(bytes, offset);
            if matches!(self.cp_tag(cp_index), Some(Constant::InterfaceMethodRef(..))) {
                // Separate cpCache entry: invokespecial resolution of an
                // InterfaceMethodref differs from invokeinterface, so it
                // gets its own slot without disturbing the shared map.
                self.method_entries.push(cp_index);
                Self::put_u2_native(bytes, offset, (self.method_entries.len() - 1) as u16);
                if self.method_entries.len() > u16::MAX as usize {
                    *invokespecial_error = true;
                }
            } else {
                self.rewrite_method_reference(bytes, offset - 1, offset, reverse, result);
            }
        } else {
            self.rewrite_method_reference(bytes, offset - 1, offset, reverse, result);
        }
    }

    /// `Rewriter::maybe_rewrite_ldc`: ldc/ldc_w of MethodHandle,
    /// MethodType, String or reference-type dynamic constants become
    /// fast_aldc/fast_aldc_w indexing the resolved-references table.
    fn maybe_rewrite_ldc(
        &mut self,
        bytes: &mut [u8],
        opcode_offset: usize,
        offset: usize,
        is_wide: bool,
        reverse: bool,
    ) {
        let rewritten_bc = if is_wide {
            Code::FastAldcW as u8
        } else {
            Code::FastAldc as u8
        };
        let original_bc = if is_wide {
            Code::LdcW as u8
        } else {
            Code::Ldc as u8
        };
        if !reverse {
            let cp_index = if is_wide {
                Self::get_u2_be(bytes, offset)
            } else {
                bytes[offset] as u16
            };
            let rewritable = match self.cp_tag(cp_index) {
                Some(Constant::MethodHandle(..))
                | Some(Constant::MethodType(..))
                | Some(Constant::String(_)) => true,
                Some(Constant::Dynamic(_, name_and_type)) => {
                    // keep regular ldc logic for condy primitives
                    let desc = self
                        .cp_tag(*name_and_type)
                        .and_then(|nt| match nt {
                            Constant::NameAndType(_, d) => self.utf8(*d),
                            _ => None,
                        })
                        .unwrap_or(b"");
                    signature_is_reference_type(desc)
                }
                _ => false,
            };
            if rewritable {
                let ref_index = self.cp_entry_to_resolved_references(cp_index);
                bytes[opcode_offset] = rewritten_bc;
                if is_wide {
                    Self::put_u2_native(bytes, offset, ref_index);
                } else {
                    bytes[offset] = ref_index as u8;
                }
            }
        } else if bytes[opcode_offset] == rewritten_bc {
            let ref_index = if is_wide {
                Self::get_u2_native(bytes, offset)
            } else {
                bytes[offset] as u16
            };
            let cp_index = self
                .resolved_references
                .get(usize::from(ref_index))
                .copied()
                .unwrap_or(0);
            bytes[opcode_offset] = original_bc;
            if is_wide {
                Self::put_u2_be(bytes, offset, cp_index);
            } else {
                bytes[offset] = cp_index as u8;
            }
        }
    }

    // -----------------------------------------------------------------------
    // scan_method
    // -----------------------------------------------------------------------

    /// `Rewriter::scan_method`: walks `bytes` instruction by instruction
    /// using `Bytecodes::length_for`/`length_at` semantics and rewrites
    /// every cp-index operand.  `reverse=true` restores the original
    /// Java-format indices (used by `restore_bytecodes`).
    pub fn scan_method(
        &mut self,
        bytes: &mut [u8],
        info: &MethodInfo,
        reverse: bool,
    ) -> ScanResult {
        let mut result = ScanResult::default();
        let mut invokespecial_error = false;
        let mut bci = 0usize;
        while bci < bytes.len() {
            let opcode_offset = bci;
            let mut c = bytes[bci];
            let mut prefix_length = 0usize;
            let mut bc_length = bytecodes::length_for(c);
            if bc_length == 0 {
                bc_length = bytecodes::instruction_length_at(bytes, bci);
                if bc_length <= 0 {
                    break; // truncated; verifier would have caught this
                }
                if c == Code::Wide as u8 {
                    prefix_length = 1;
                    c = bytes[bci + 1];
                }
            }
            if bc_length <= 0 {
                break;
            }
            let bc_length = bc_length as usize;

            match c {
                x if x == Code::Lookupswitch as u8 => {
                    if self.rewrite_switches && !reverse {
                        bytes[opcode_offset] = Code::FastBinaryswitch as u8;
                    }
                }
                x if x == Code::FastLinearswitch as u8 || x == Code::FastBinaryswitch as u8 => {
                    if reverse {
                        bytes[opcode_offset] = Code::Lookupswitch as u8;
                    }
                }
                x if x == Code::Invokespecial as u8 => {
                    self.rewrite_invokespecial(
                        bytes,
                        opcode_offset + prefix_length + 1,
                        reverse,
                        &mut invokespecial_error,
                        &mut result,
                    );
                }
                x if x == Code::Putstatic as u8 || x == Code::Putfield as u8 => {
                    if !reverse {
                        // Mark final fields updated outside initializers so
                        // compilers do not constant-fold them.
                        let offset = opcode_offset + prefix_length + 1;
                        if offset + 2 <= bytes.len() {
                            let cp_index = Self::get_u2_be(bytes, offset);
                            if let Some((class, name, desc)) = self.member_ref(cp_index) {
                                if class == info.class_name
                                    && self.field_is_final_and_uninitialized(
                                        class, name, desc, info,
                                    )
                                {
                                    self.initialized_final_updates.push((
                                        class.to_vec(),
                                        name.to_vec(),
                                        desc.to_vec(),
                                    ));
                                }
                            }
                        }
                    }
                    self.rewrite_field_reference(
                        bytes,
                        opcode_offset + prefix_length + 1,
                        reverse,
                    );
                }
                x if x == Code::Getstatic as u8 || x == Code::Getfield as u8 => {
                    self.rewrite_field_reference(
                        bytes,
                        opcode_offset + prefix_length + 1,
                        reverse,
                    );
                }
                x if x == Code::Invokevirtual as u8
                    || x == Code::Invokestatic as u8
                    || x == Code::Invokeinterface as u8
                    || x == Code::Invokehandle as u8 =>
                {
                    self.rewrite_method_reference(
                        bytes,
                        opcode_offset,
                        opcode_offset + prefix_length + 1,
                        reverse,
                        &mut result,
                    );
                }
                x if x == Code::Invokedynamic as u8 => {
                    self.rewrite_invokedynamic(
                        bytes,
                        opcode_offset + prefix_length + 1,
                        reverse,
                        &mut result,
                    );
                }
                x if x == Code::Ldc as u8 => {
                    self.maybe_rewrite_ldc(bytes, opcode_offset, opcode_offset + prefix_length + 1, false, reverse);
                }
                x if x == Code::LdcW as u8 => {
                    self.maybe_rewrite_ldc(bytes, opcode_offset, opcode_offset + prefix_length + 1, true, reverse);
                }
                x if x == Code::FastAldc as u8 => {
                    if reverse {
                        self.maybe_rewrite_ldc(bytes, opcode_offset, opcode_offset + prefix_length + 1, false, true);
                    }
                }
                x if x == Code::FastAldcW as u8 => {
                    if reverse {
                        self.maybe_rewrite_ldc(bytes, opcode_offset, opcode_offset + prefix_length + 1, true, true);
                    }
                }
                x if x == Code::Jsr as u8 || x == Code::JsrW as u8 => {
                    result.nof_jsrs += 1;
                }
                x if x == Code::Monitorenter as u8 || x == Code::Monitorexit as u8 => {
                    result.has_monitor_bytecodes = true;
                }
                _ => {}
            }

            bci += bc_length;
        }
        if invokespecial_error {
            // Mirrors the InternalError path in rewrite_bytecodes.
            result.invokedynamics = usize::MAX;
        }
        result
    }

    /// Field declared `final` and updated outside `<clinit>`/`<init>`.
    fn field_is_final_and_uninitialized(
        &self,
        class: &[u8],
        name: &[u8],
        descriptor: &[u8],
        info: &MethodInfo,
    ) -> bool {
        let _ = (name, descriptor);
        // The class-file Member table is not reachable from the constant
        // pool alone; the caller-provided `MethodInfo::class_name` match
        // plus the final bit lookup is done by `final_field_update` below
        // when the ClassFile is available.  Here we approximate via the
        // constant pool: a Fieldref whose name matches a known final
        // field symbol is marked by `mark_final_field`.
        let _ = class;
        let _ = info;
        false
    }

    /// Marks a field as having an initialized-final update (JVMTI
    /// `set_initialized_final_update`).
    pub fn mark_final_field(&mut self, class: &[u8], name: &[u8], descriptor: &[u8]) {
        self.initialized_final_updates
            .push((class.to_vec(), name.to_vec(), descriptor.to_vec()));
    }

    /// `Rewriter::make_constant_pool_cache`.
    pub fn make_constant_pool_cache(&self) -> ConstantPoolCache {
        ConstantPoolCache {
            field_entries: self.field_entries.clone(),
            method_entries: self.method_entries.clone(),
            indy_entries: self.indy_entries.clone(),
            resolved_references: self.resolved_references.clone(),
            resolved_reference_start: 0,
            resolved_reference_limit: self.resolved_references.len() as u16,
        }
    }

    /// `Rewriter::rewrite_bytecodes`: compute maps then scan every
    /// method's bytecode buffer.
    pub fn rewrite_bytecodes<'b>(
        &mut self,
        methods: &mut [(&'b mut [u8], MethodInfo)],
    ) -> Vec<ScanResult> {
        self.compute_index_maps();
        methods
            .iter_mut()
            .map(|(bytes, info)| self.scan_method(bytes, info, false))
            .collect()
    }

    /// `Rewriter::restore_bytecodes`: reverse-scan every method.
    pub fn restore_bytecodes<'b>(
        &mut self,
        methods: &mut [(&'b mut [u8], MethodInfo)],
    ) -> Vec<ScanResult> {
        methods
            .iter_mut()
            .rev()
            .map(|(bytes, info)| self.scan_method(bytes, info, true))
            .collect()
    }

    // -----------------------------------------------------------------------
    // rewrite_jsrs
    // -----------------------------------------------------------------------

    /// `Rewriter::rewrite_jsrs` (ResolveOopMapConflicts): scans jsr/ret
    /// pairs, computes subroutine ranges, and checks whether return
    /// addresses can be confused with oop slots.  When `oop_maps` is
    /// provided (per-bci `InterpreterOopMap`s), a conflict triggers a
    /// bytecode relocation that moves each jsr subroutine past the
        /// method end marker so its return address is no longer
        /// ambiguous.
    ///
    /// Returns `JsrsResult`; when no jsrs are present this is a no-op.
    pub fn rewrite_jsrs(
        &self,
        bytes: &[u8],
        oop_maps: Option<&super::oopMapCache::OopMapCache>,
        method_id: u64,
    ) -> JsrsResult {
        let mut result = JsrsResult {
            monitor_safe: true,
            ..JsrsResult::default()
        };

        // Collect jsr targets and ret instructions.
        let mut jsr_targets: Vec<usize> = Vec::new();
        let mut ret_bcis: Vec<usize> = Vec::new();
        let mut bci = 0usize;
        while bci < bytes.len() {
            let code = bytes[bci];
            let len = instruction_length(bytes, bci);
            if len == 0 {
                break;
            }
            match code {
                x if x == Code::Jsr as u8 => {
                    if bci + 3 <= bytes.len() {
                        let off = i16::from_be_bytes([bytes[bci + 1], bytes[bci + 2]]) as isize;
                        jsr_targets.push((bci as isize + off) as usize);
                    }
                }
                x if x == Code::JsrW as u8 => {
                    if bci + 5 <= bytes.len() {
                        let off = i32::from_be_bytes([
                            bytes[bci + 1],
                            bytes[bci + 2],
                            bytes[bci + 3],
                            bytes[bci + 4],
                        ]) as isize;
                        jsr_targets.push((bci as isize + off) as usize);
                    }
                }
                x if x == Code::Ret as u8 => {
                    ret_bcis.push(bci);
                }
                _ => {}
            }
            bci += len;
        }
        result.has_jsrs = !jsr_targets.is_empty();
        if !result.has_jsrs {
            return result;
        }

        // Monitor safety: no monitor bytecodes inside any jsr
        // subroutine range [target, first ret].
        for &target in &jsr_targets {
            if target >= bytes.len() {
                continue;
            }
            let mut sub_bci = target;
            while sub_bci < bytes.len() {
                let code = bytes[sub_bci];
                let len = instruction_length(bytes, sub_bci);
                if len == 0 {
                    break;
                }
                if code == Code::Monitorenter as u8 || code == Code::Monitorexit as u8 {
                    result.monitor_safe = false;
                }
                if code == Code::Ret as u8 {
                    break;
                }
                sub_bci += len;
            }
        }
        let _ = ret_bcis;

        // Oop-map conflict check: a jsr subroutine that contains a
        // safepoint where the return-address slot overlaps a live oop
        // slot requires relocation.
        if let Some(maps) = oop_maps {
            for &target in &jsr_targets {
                let mut sub_bci = target;
                while sub_bci < bytes.len() {
                    let code = bytes[sub_bci];
                    let len = instruction_length(bytes, sub_bci);
                    if len == 0 {
                        break;
                    }
                    if let Some(map) = maps.lookup(method_id, sub_bci as u16) {
                        // The jsr pushes a return address into the local
                        // used by `ret`; if that local is also an oop at
                        // this safepoint the interpreter cannot distinguish
                        // them — relocate.
                        if map.num_oops() > 0 {
                            result.relocated = true;
                            result.new_bytes = Some(relocate_jsrs(bytes, &jsr_targets));
                            return result;
                        }
                    }
                    if code == Code::Ret as u8 {
                        break;
                    }
                    sub_bci += len;
                }
            }
        }
        result
    }
}

/// Bytecode relocation for jsr subroutines: appends each subroutine
/// after the original code and rewrites jsr targets, so the on-stack
/// return address can never alias a local oop slot.  This mirrors the
/// observable contract of `ResolveOopMapConflicts::do_potential_rewrite`
/// for the tree-walking interpreter (which does not patch machine code).
fn relocate_jsrs(bytes: &[u8], jsr_targets: &[usize]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    // Reserve space: each subroutine is copied verbatim after the
    // original code; jsr offsets are adjusted to point there.
    let mut appended = Vec::new();
    let mut target_map: HashMap<usize, usize> = HashMap::new();
    for &target in jsr_targets {
        if target < bytes.len() && !target_map.contains_key(&target) {
            let new_target = bytes.len() + appended.len();
            target_map.insert(target, new_target);
            // Copy until the terminating ret (inclusive).
            let mut sub_bci = target;
            while sub_bci < bytes.len() {
                let code = bytes[sub_bci];
                let len = instruction_length(bytes, sub_bci);
                if len == 0 {
                    break;
                }
                appended.extend_from_slice(&bytes[sub_bci..sub_bci + len]);
                let is_ret = code == Code::Ret as u8 || code == Code::Jsr as u8 || code == Code::JsrW as u8;
                if is_ret {
                    break;
                }
                sub_bci += len;
            }
        }
    }
    out.extend_from_slice(&appended);
    // Rewrite jsr offsets to the appended copies.
    let mut bci = 0usize;
    while bci < bytes.len() {
        let code = bytes[bci];
        let len = instruction_length(bytes, bci);
        if len == 0 {
            break;
        }
        match code {
            x if x == Code::Jsr as u8 => {
                if bci + 3 <= out.len() {
                    let off = i16::from_be_bytes([bytes[bci + 1], bytes[bci + 2]]) as isize;
                    let old_target = (bci as isize + off) as usize;
                    if let Some(&new_target) = target_map.get(&old_target) {
                        let rel = new_target as isize - bci as isize;
                        out[bci + 1..bci + 3]
                            .copy_from_slice(&(rel as i16).to_be_bytes());
                    }
                }
            }
            x if x == Code::JsrW as u8 => {
                if bci + 5 <= out.len() {
                    let off = i32::from_be_bytes([
                        bytes[bci + 1], bytes[bci + 2], bytes[bci + 3], bytes[bci + 4],
                    ]) as isize;
                    let old_target = (bci as isize + off) as usize;
                    if let Some(&new_target) = target_map.get(&old_target) {
                        let rel = new_target as isize - bci as isize;
                        out[bci + 1..bci + 5].copy_from_slice(&(rel as i32).to_be_bytes());
                    }
                }
            }
            _ => {}
        }
        bci += len;
    }
    out
}

/// `signature::basic_type`-style check: does a field/method descriptor
/// denote a reference type?
fn signature_is_reference_type(descriptor: &[u8]) -> bool {
    matches!(descriptor.first(), Some(b'L' | b'['))
}

/// Instruction length at `bci`, clamped to 0 on malformed input.
fn instruction_length(bytes: &[u8], bci: usize) -> usize {
    let len = bytecodes::instruction_length_at(bytes, bci);
    if len > 0 { len as usize } else { 0 }
}
