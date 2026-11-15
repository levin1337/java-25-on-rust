/*
 * Port of OpenJDK 25 HotSpot ciMethod (share/ci/ciMethod.hpp/.cpp)
 *
 * ciMethod is the compiler-interface view of an executable method:
 * bytecode access, exception handlers, liveness, compilation flags and
 * intrinsic identification.  It is a thin facade over the classfile
 * `Member` plus the parsed `Code` attribute, resolved through a
 * `CiInstanceKlass` holder.
 */

use super::ciEnv::CiEnv;
use super::ciInstanceKlass::CiInstanceKlass;
use super::ciLiveness::{self, CiLiveness, CiLivenessPoint};
use super::ciType::CIType;
use crate::share::classfile::classFileParser::{Constant, Member};
use crate::share::interpreter::bytecode::Bytecode;
use crate::share::interpreter::bytecodes::Code as BytecodeCode;
use crate::share::oops::constMethod::ConstMethod;
use crate::share::oops::method::{Code, Method};
use std::sync::{Arc, OnceLock};

/// `ciExceptionHandler`: one row of the method's exception table with
/// the catch type resolved to a class name when present.
#[derive(Debug, Clone)]
pub struct CiExceptionHandler<'a> {
    pub start_pc: usize,
    pub end_pc: usize,
    pub handler_pc: usize,
    /// Constant-pool index of the catch type (0 = finally/catch-all).
    pub catch_type: u16,
    /// Internal name of the catch type (`java/lang/Exception`), `None`
    /// for catch-all entries.
    pub catch_type_name: Option<&'a [u8]>,
}

/// `ciMethod`: compiler-interface view of a `Member` declared in a
/// `CiInstanceKlass`.
pub struct CiMethod<'a> {
    pub(crate) env: CiEnv<'a>,
    pub(crate) holder: CiInstanceKlass<'a>,
    member: Member<'a>,
    code: Option<Code<'a>>,
    liveness: OnceLock<CiLiveness>,
    method_obj: OnceLock<Option<Arc<Method>>>,
}

impl<'a> CiMethod<'a> {
    pub(crate) fn new(holder: CiInstanceKlass<'a>, member: Member<'a>) -> Self {
        let env = holder.env.clone();
        let code = Code::parse(&member).ok();
        Self {
            env,
            holder,
            member,
            code,
            liveness: OnceLock::new(),
            method_obj: OnceLock::new(),
        }
    }

    // ---- identity ---------------------------------------------------------

    /// `ciMethod::name`.
    #[inline]
    pub fn name(&self) -> &'a [u8] {
        self.member.name
    }

    /// `ciMethod::signature`.
    #[inline]
    pub fn signature(&self) -> &'a [u8] {
        self.member.descriptor
    }

    /// `ciMethod::holder` — the klass that declares this method.
    #[inline]
    pub fn holder(&self) -> &CiInstanceKlass<'a> {
        &self.holder
    }

    #[inline]
    pub fn access_flags(&self) -> u16 {
        self.member.accessFlags
    }

    #[inline]
    pub fn is_public(&self) -> bool {
        self.member.accessFlags & Method::ACC_PUBLIC != 0
    }

    #[inline]
    pub fn is_private(&self) -> bool {
        self.member.accessFlags & Method::ACC_PRIVATE != 0
    }

    #[inline]
    pub fn is_protected(&self) -> bool {
        self.member.accessFlags & Method::ACC_PROTECTED != 0
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        self.member.accessFlags & Method::ACC_STATIC != 0
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        self.member.accessFlags & Method::ACC_FINAL != 0
    }

    #[inline]
    pub fn is_synchronized(&self) -> bool {
        self.member.accessFlags & Method::ACC_SYNCHRONIZED != 0
    }

    #[inline]
    pub fn is_native(&self) -> bool {
        self.member.accessFlags & Method::ACC_NATIVE != 0
    }

    #[inline]
    pub fn is_abstract(&self) -> bool {
        self.member.accessFlags & Method::ACC_ABSTRACT != 0
    }

    #[inline]
    pub fn is_synthetic(&self) -> bool {
        self.member.accessFlags & Method::ACC_SYNTHETIC != 0
    }

    #[inline]
    pub fn is_bridge(&self) -> bool {
        self.member.accessFlags & Method::ACC_BRIDGE != 0
    }

    #[inline]
    pub fn is_varargs(&self) -> bool {
        self.member.accessFlags & Method::ACC_VARARGS != 0
    }

    #[inline]
    pub fn is_strict(&self) -> bool {
        self.member.accessFlags & Method::ACC_STRICT != 0
    }

    /// `ciMethod::is_static_initializer`.
    #[inline]
    pub fn is_static_initializer(&self) -> bool {
        self.name() == b"<clinit>"
    }

    /// `ciMethod::is_constructor` (`<init>`).
    #[inline]
    pub fn is_constructor(&self) -> bool {
        self.name() == b"<init>"
    }

    /// `ciMethod::is_loaded` — ci handles always wrap linked metadata.
    #[inline]
    pub fn is_loaded(&self) -> bool {
        true
    }

    // ---- code attributes ----------------------------------------------------

    /// Parsed Code attribute, when present (absent for native/abstract).
    #[inline]
    pub fn code(&self) -> Option<&Code<'a>> {
        self.code.as_ref()
    }

    #[inline]
    pub fn has_code(&self) -> bool {
        self.code.is_some()
    }

    /// `ciMethod::code_size`.
    #[inline]
    pub fn code_size(&self) -> usize {
        self.code.as_ref().map_or(0, |c| c.bytes.len())
    }

    /// `ciMethod::max_stack`.
    #[inline]
    pub fn max_stack(&self) -> usize {
        self.code.as_ref().map_or(0, |c| c.maxStack)
    }

    /// `ciMethod::max_locals`.
    #[inline]
    pub fn max_locals(&self) -> usize {
        self.code.as_ref().map_or(0, |c| c.maxLocals)
    }

    /// `ciMethod::bytecodes` — raw bytecode bytes (empty when no Code).
    #[inline]
    pub fn bytecodes(&self) -> &'a [u8] {
        self.code.as_ref().map_or(&[], |c| c.bytes)
    }

    /// `ciMethod::code` analogue returning the raw instruction bytes.
    #[inline]
    pub fn code_bytes(&self) -> &'a [u8] {
        self.bytecodes()
    }

    #[inline]
    pub fn bytecode_at(&self, bci: usize) -> Option<u8> {
        self.bytecodes().get(bci).copied()
    }

    /// `ciMethod::raw_code_at_bci`.
    #[inline]
    pub fn raw_code_at_bci(&self, bci: usize) -> Option<u8> {
        self.bytecode_at(bci)
    }

    /// `ciMethod::java_code_at_bci`: decoded `Bytecodes::Code`.
    pub fn java_code_at_bci(&self, bci: usize) -> Option<BytecodeCode> {
        let op = self.bytecode_at(bci)?;
        if op == 0xc4 {
            return self.bytecode_at(bci + 1).and_then(|s| bytes_code(s));
        }
        bytes_code(op)
    }

    /// `ciMethod::code_at` stream view at a bci.
    pub fn instruction_at(&self, bci: usize) -> Option<Bytecode<'a>> {
        Bytecode::new(self.bytecodes(), bci)
    }

    /// Argument slot count of the signature (no receiver) —
    /// `ciSignature::size`.
    pub fn arg_size_no_receiver(&self) -> usize {
        CIType::signature_slot_size(&self.env, self.signature()).unwrap_or(0)
    }

    /// `ciMethod::arg_size` — slots including the receiver for
    /// instance methods.
    pub fn arg_size(&self) -> usize {
        self.arg_size_no_receiver() + usize::from(!self.is_static())
    }

    /// `ciMethod::invoke_arg_size`: stack elements pushed by the caller
    /// for an invoke of `code` (opcode) at a call site of this method.
    pub fn invoke_arg_size(&self, code: u8) -> usize {
        if self.is_loaded() {
            self.arg_size()
        } else {
            let mut size = self.arg_size_no_receiver();
            if !matches!(code, 0xb8 | 0xba) {
                size += 1;
            }
            size
        }
    }

    /// `ciMethod::size_of_parameters`: parameter slots including the
    /// receiver (ConstMethod::size_of_parameters analogue).
    pub fn size_of_parameters(&self) -> usize {
        self.arg_size()
    }

    // ---- signature types -----------------------------------------------------

    /// `(param_types, return_type)` resolved through the env.
    /// Returns `None` when a referenced class cannot be loaded.
    pub fn signature_types(&self) -> Option<(Vec<CIType<'a>>, CIType<'a>)> {
        CIType::parse_method_descriptor(&self.env, self.signature())
    }

    /// `ciMethod::return_type`.
    pub fn return_type(&self) -> Option<CIType<'a>> {
        self.signature_types().map(|(_, r)| r)
    }

    /// Parameter types in order.
    pub fn parameter_types(&self) -> Option<Vec<CIType<'a>>> {
        self.signature_types().map(|(p, _)| p)
    }

    // ---- exception handlers ---------------------------------------------------

    /// `ciMethod::exception_handlers` / `ciExceptionHandlerStream`.
    pub fn exception_handlers(&self) -> Vec<CiExceptionHandler<'a>> {
        let Some(code) = &self.code else {
            return Vec::new();
        };
        code.exception_table
            .iter()
            .map(|e| CiExceptionHandler {
                start_pc: e.start_pc,
                end_pc: e.end_pc,
                handler_pc: e.handler_pc,
                catch_type: e.catch_type,
                catch_type_name: self.catch_type_name(e.catch_type),
            })
            .collect()
    }

    fn catch_type_name(&self, index: u16) -> Option<&'a [u8]> {
        if index == 0 {
            return None;
        }
        self.holder.instance_klass().className(index).ok()
    }

    #[inline]
    pub fn has_exception_handlers(&self) -> bool {
        self.code
            .as_ref()
            .is_some_and(|c| !c.exception_table.is_empty())
    }

    #[inline]
    pub fn exception_table_length(&self) -> usize {
        self.code.as_ref().map_or(0, |c| c.exception_table.len())
    }

    // ---- oops::Method bridge ---------------------------------------------------

    /// Lazily built `oops::method::Method` (ConstMethod + access flags
    /// + intrinsic id slot), mirroring `CompileBroker::resolve_method_obj`.
    /// Returns `None` for methods without a Code attribute.
    pub fn method_obj(&self) -> Option<Arc<Method>> {
        self.method_obj
            .get_or_init(|| self.build_method_obj())
            .clone()
    }

    fn build_method_obj(&self) -> Option<Arc<Method>> {
        let code = self.code.as_ref()?;
        let constants = &self.holder.instance_klass().class.constants;
        let name_index = self.utf8_index(self.name()).unwrap_or(0);
        let signature_index = self.utf8_index(self.signature()).unwrap_or(0);
        let const_method = ConstMethod::new(
            name_index,
            signature_index,
            code.maxStack.min(u16::MAX as usize) as u16,
            code.maxLocals.min(u16::MAX as usize) as u16,
            self.size_of_parameters().min(u16::MAX as usize) as u16,
            code.bytes.to_vec(),
            code.exception_table.clone(),
            Vec::new(),
            Vec::new(),
        );
        Some(Arc::new(Method::new(
            Arc::new(const_method),
            self.member.accessFlags,
        )))
    }

    /// Index of a Utf8 constant equal to `bytes` (for ConstMethod indices).
    fn utf8_index(&self, bytes: &[u8]) -> Option<u16> {
        self.holder
            .instance_klass()
            .class
            .constants
            .iter()
            .position(|c| matches!(c, Constant::Utf8(b) if *b == bytes))
            .map(|i| i as u16)
    }

    /// `ciMethod::intrinsic_id`.  Intrinsic ids are assigned by
    /// `vmIntrinsics` at link time; the port has no shared mutable
    /// Method store yet, so this reflects the lazily built Method.
    #[inline]
    pub fn intrinsic_id(&self) -> u16 {
        self.method_obj().map_or(0, |m| m.intrinsic_id())
    }

    #[inline]
    pub fn invocation_count(&self) -> u32 {
        self.method_obj().map_or(0, |m| m.invocation_count())
    }

    #[inline]
    pub fn backedge_count(&self) -> u32 {
        self.method_obj().map_or(0, |m| m.backedge_count())
    }

    #[inline]
    pub fn should_compile(&self, threshold: u32) -> bool {
        self.method_obj()
            .is_some_and(|m| m.should_compile(threshold))
    }

    #[inline]
    pub fn should_osr_compile(&self, threshold: u32) -> bool {
        self.method_obj()
            .is_some_and(|m| m.should_osr_compile(threshold))
    }

    #[inline]
    pub fn highest_comp_level(&self) -> u8 {
        self.method_obj().map_or(0, |m| m.highest_comp_level())
    }

    // ---- compilability ---------------------------------------------------------

    /// `ciMethod::can_be_parsed` — has bytecode to parse.
    #[inline]
    pub fn can_be_parsed(&self) -> bool {
        self.code.is_some()
    }

    /// `ciMethod::can_be_compiled` base check: non-native, non-abstract,
    /// with a Code attribute.
    #[inline]
    pub fn can_be_compiled(&self) -> bool {
        self.code.is_some() && !self.is_native() && !self.is_abstract()
    }

    /// `ciMethod::_is_c1_compilable`.
    #[inline]
    pub fn is_c1_compilable(&self) -> bool {
        self.can_be_compiled()
    }

    /// `ciMethod::_is_c2_compilable`.
    #[inline]
    pub fn is_c2_compilable(&self) -> bool {
        self.can_be_compiled()
    }

    /// `ciMethod::can_be_statically_bound`: final, private or static
    /// methods cannot be overridden.
    #[inline]
    pub fn can_be_statically_bound(&self) -> bool {
        self.is_final() || self.is_private() || self.is_static()
    }

    /// `ciMethod::is_overpass` — no overpass infrastructure in the port.
    #[inline]
    pub fn is_overpass(&self) -> bool {
        false
    }

    /// `ciMethod::needs_clinit_barrier`: a class-init barrier is needed
    /// while the holder is not fully initialized.
    #[inline]
    pub fn needs_clinit_barrier(&self) -> bool {
        !self.holder.is_initialized()
    }

    /// `ciMethod::caller_sensitive` — Method flag not modeled; methods
    /// named like the JDK sensitive set are conservatively reported.
    pub fn caller_sensitive(&self) -> bool {
        matches!(
            (self.holder.name(), self.name()),
            (
                b"java/lang/System",
                b"currentTimeMillis" | b"nanoTime" | b"identityHashCode"
            ) | (
                b"java/lang/Thread",
                b"currentThread" | b"holdsLock" | b"onSpinWait"
            )
        )
    }

    /// `ciMethod::force_inline` / `dont_inline` / `intrinsic_candidate`
    /// — driven by Method flags not yet modeled in the port; consult
    /// the intrinsic id table instead.
    #[inline]
    pub fn intrinsic_candidate(&self) -> bool {
        self.intrinsic_id() != 0
    }

    #[inline]
    pub fn force_inline(&self) -> bool {
        false
    }

    #[inline]
    pub fn dont_inline(&self) -> bool {
        false
    }

    // ---- monitors ---------------------------------------------------------------

    /// `ciMethod::_uses_monitors`: bytecode contains monitorenter/exit.
    pub fn has_monitor_bytecodes(&self) -> bool {
        self.bytecodes().iter().any(|&b| b == 0xc2 || b == 0xc3)
    }

    /// `ciMethod::has_balanced_monitors`: equal numbers of
    /// monitorenter and monitorexit instructions (syntactic check;
    /// HotSpot performs a flow analysis — noted approximation).
    pub fn has_balanced_monitors(&self) -> bool {
        let enters = self.bytecodes().iter().filter(|&&b| b == 0xc2).count();
        let exits = self.bytecodes().iter().filter(|&&b| b == 0xc3).count();
        enters == exits
    }

    // ---- line numbers --------------------------------------------------------------

    /// `ciMethod::has_linenumber_table`.
    pub fn has_linenumber_table(&self) -> bool {
        self.code_attributes_named(b"LineNumberTable")
    }

    /// Whether the Code attribute's sub-attributes contain `name`.
    fn code_attributes_named(&self, name: &[u8]) -> bool {
        let Some(code_attr) = self.member.code_attribute() else {
            return false;
        };
        // Layout: max_stack(2) max_locals(2) code_length(4) code
        // exception_table_length(2) entries(8*n) attributes_count(2)
        // attributes(u2 name,u4 len,bytes)*
        let Some(len_b) = code_attr.get(4..8) else {
            return false;
        };
        let code_len = u32::from_be_bytes([len_b[0], len_b[1], len_b[2], len_b[3]]) as usize;
        let mut pos = 8 + code_len;
        let Some(exc_b) = code_attr.get(pos..pos + 2) else {
            return false;
        };
        let exc_count = u16::from_be_bytes([exc_b[0], exc_b[1]]) as usize;
        pos += 2 + exc_count * 8;
        let Some(attr_b) = code_attr.get(pos..pos + 2) else {
            return false;
        };
        let attr_count = u16::from_be_bytes([attr_b[0], attr_b[1]]) as usize;
        pos += 2;
        let constants = &self.holder.instance_klass().class.constants;
        for _ in 0..attr_count {
            let Some(hdr) = code_attr.get(pos..pos + 6) else {
                return false;
            };
            let name_idx = u16::from_be_bytes([hdr[0], hdr[1]]);
            let len = u32::from_be_bytes([hdr[2], hdr[3], hdr[4], hdr[5]]) as usize;
            let is_match =
                matches!(constants.get(name_idx as usize), Some(Constant::Utf8(b)) if *b == name);
            if is_match {
                return true;
            }
            pos += 6 + len;
        }
        false
    }

    /// `ciMethod::line_number_from_bci`.
    pub fn line_number_from_bci(&self, bci: usize) -> Option<u16> {
        let code = self.code.as_ref()?;
        let constants = &self.holder.instance_klass().class.constants;
        code.lineNumber(constants, bci as u16)
    }

    // ---- liveness ---------------------------------------------------------------------

    /// Lazily computed `MethodLiveness` analogue for this method.
    pub fn liveness(&self) -> &CiLiveness {
        self.liveness.get_or_init(|| {
            ciLiveness::compute_liveness(
                self.bytecodes(),
                self.max_locals(),
                self.is_static(),
                self.signature(),
                &self
                    .code
                    .as_ref()
                    .map(|c| c.exception_table.clone())
                    .unwrap_or_default(),
                &self.holder.instance_klass().class.constants,
            )
        })
    }

    /// `ciMethod::liveness_at_bci` / `raw_liveness_at_bci`.
    pub fn liveness_at_bci(&self, bci: usize) -> CiLivenessPoint<'_> {
        self.liveness().at(bci, self.code_size())
    }

    #[inline]
    pub fn raw_liveness_at_bci(&self, bci: usize) -> CiLivenessPoint<'_> {
        self.liveness_at_bci(bci)
    }

    /// `ciMethod::live_local_oops_at_bci`: slots that may hold oops.
    pub fn live_local_oops_at_bci(&self, bci: usize) -> Vec<usize> {
        self.liveness().oop_slots(bci, self.code_size())
    }

    // ---- dispatch -------------------------------------------------------------------------

    /// `ciMethod::vtable_index`.
    pub fn vtable_index(&self) -> i32 {
        self.holder.vtable_index(self.name(), self.signature())
    }
}

fn bytes_code(op: u8) -> Option<BytecodeCode> {
    crate::share::interpreter::bytecodes::code_from_u8(op)
}

impl<'a> std::fmt::Debug for CiMethod<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ciMethod({}{}{})",
            String::from_utf8_lossy(self.holder.name()),
            String::from_utf8_lossy(self.name()),
            String::from_utf8_lossy(self.signature())
        )
    }
}
