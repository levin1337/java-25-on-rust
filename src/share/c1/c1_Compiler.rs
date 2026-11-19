use super::asm::codeBuffer::{CodeBuffer, DEFAULT_BUFFER_SIZE};
use super::asm::x86::{emit_program, Abi, BackendError, BackendOptions, ParamInfo};
use super::c1_GraphBuilder::{GraphBuilder, MethodMetadata};
use super::c1_IR::ControlFlowGraph;
use super::c1_LIR::{LirGenerator, LirKind};
use super::c1_Optimizer::Optimizer;
use crate::share::code::pcDesc::PcDesc;
use crate::share::compiler::abstractCompiler::{AbstractCompiler, CompileError, CompilerType};
use crate::share::compiler::compileTask::{CompileOutcome, CompileTask, FailureReason};
use crate::share::compiler::compilerDefinitions::CompLevel;
use crate::share::jitruntime::runtime::{SlotKind, parse_signature};
use super::c1_Instruction::{HirInstruction, ValueKind};

thread_local! {
    static CURRENT_METHOD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub struct C1Compiler {
    pub compiled_methods: usize,
}

impl C1Compiler {
    pub fn new() -> Self {
        Self { compiled_methods: 0 }
    }

    pub fn compile_hir(&mut self, mut cfg: ControlFlowGraph) -> ControlFlowGraph {
        Optimizer::optimize_cfg(&mut cfg);
        self.compiled_methods += 1;
        cfg
    }

    pub fn compile_bytecode(&mut self, bytecode: &[u8], max_locals: usize) -> ControlFlowGraph {
        let cfg = GraphBuilder::build(bytecode, max_locals);
        self.compile_hir(cfg)
    }

    pub fn compile_to_code(&mut self, bytecode: &[u8], max_locals: usize, params: Vec<ParamInfo>) -> Result<(Vec<u8>, usize, Vec<PcDesc>), BackendError> {
        self.compile_to_code_meta(bytecode, max_locals, params, MethodMetadata::default())
    }

    /// `compile_to_code` with pre-resolved method metadata (ldc
    /// constants, field/call-site info, exception handlers) collected
    /// by the compile broker from the owning class.
    pub fn compile_to_code_meta(&mut self, bytecode: &[u8], max_locals: usize, params: Vec<ParamInfo>, metadata: MethodMetadata) -> Result<(Vec<u8>, usize, Vec<PcDesc>), BackendError> {
        let trace = std::env::var_os("JVM25_TRACE_COMPILE").is_some();
        if trace { eprintln!("[jit-stage] build_full len={}", bytecode.len()); }
        let cfg = GraphBuilder::build_full(bytecode, max_locals, metadata.clone());
        if trace { eprintln!("[jit-stage] optimize_cfg"); }
        let cfg = self.compile_hir(cfg);
        if trace { eprintln!("[jit-stage] cfg_executable"); }
        // Executable-tier gate: heap operations lower to `CallRuntime`
        // stubs backed by real handlers in `jitruntime::stubs`, and
        // invokes lower to either a direct nmethod call
        // (`CallInfo.target`) or the `UnresolvedCall` stub, which
        // resolves the target and runs it compiled or interpreted —
        // pending outcomes cascade through `invoke_raw` boundaries, so
        // non-leaf methods are supported.  `Unsupported` nodes and
        // >4-operand runtime stubs stay rejected.
        if !cfg_executable(&cfg) {
            return Err(BackendError::Unsupported(
                "method requires unsupported instructions".to_string(),
            ));
        }
        if trace { eprintln!("[jit-stage] lir_generate"); }
        let program = LirGenerator::generate(&cfg, &metadata);
        if trace { eprintln!("[jit-stage] emit_program"); }
        if std::env::var("JVM25_DUMP_LIR").is_ok() {
            eprintln!("[lir:{}] {:?}", CURRENT_METHOD.with(|m| m.borrow().clone()), program);
        }
        let options = BackendOptions {
            abi: Abi::WindowsX64,
            stubs: crate::share::jitruntime::stub_map(),
            emit_safepoints: true,
            params,
            ..BackendOptions::default()
        };
        let buf = CodeBuffer::new(DEFAULT_BUFFER_SIZE)?;
        let emitted = emit_program(&buf, &program, &options)?;
        buf.flush_icache();
        let code = buf.code_slice().to_vec();
        let pc_descs = emitted.pc_descs.iter().map(|p| {
            let flags = if p.is_safepoint { PcDesc::PCDESC_REEXECUTE } else { 0 };
            PcDesc::new(p.pc_offset as i32, p.bci, flags)
        }).collect();
        Ok((code, emitted.frame_size, pc_descs))
    }
}

impl Default for C1Compiler {
    fn default() -> Self { Self::new() }
}

/// True when every instruction in the CFG stays within the executable
/// subset: primitive computation, control flow, returns and heap
/// operations that lower to real `CallRuntime` stub handlers.
fn cfg_executable(cfg: &ControlFlowGraph) -> bool {
    cfg.blocks
        .iter()
        .all(|block| block.instructions.iter().all(hir_executable))
}

/// Recursive gate mirroring the executable backend lowerings.  Heap
/// operations are allowed because `jitruntime::stubs` implements real
/// handlers for them; `Float64` field/array accesses stay rejected
/// because runtime-stub results travel through `rax` only (no xmm
/// return channel).  Method calls are allowed: resolved targets emit
/// a direct call, unresolved ones go through the `UnresolvedCall`
/// stub whose handler re-dispatches compiled or interpreted code.
/// Nested operand expressions are checked the same way.
fn hir_executable(inst: &HirInstruction) -> bool {
    use HirInstruction as H;
    match inst {
        H::ConstantInt(_)
        | H::ConstantLong(_)
        | H::ConstantFloat(_)
        | H::ConstantDouble(_)
        | H::ConstantNull
        | H::Goto(_) => true,
        H::Neg(v) => hir_executable(v),
        H::LoadLocal { .. } | H::LoadStack { .. } => true,
        H::StoreLocal { value, .. } | H::StoreStack { value, .. } => hir_executable(value),
        H::MonitorEnter(v) | H::MonitorExit(v) | H::Throw(v) | H::ArrayLength(v) => {
            hir_executable(v)
        }
        H::Add(l, r)
        | H::Sub(l, r)
        | H::Mul(l, r)
        | H::Div(l, r)
        | H::Rem(l, r)
        | H::And(l, r)
        | H::Or(l, r)
        | H::Xor(l, r)
        | H::Shl(l, r)
        | H::Shr(l, r)
        | H::Ushr(l, r)
        | H::Compare { left: l, right: r, .. } => hir_executable(l) && hir_executable(r),
        H::Convert { value, .. } => hir_executable(value),
        H::If { left, right, .. } => hir_executable(left) && hir_executable(right),
        H::Return(value) => value.as_deref().map_or(true, hir_executable),
        H::TableSwitch { index, .. } => hir_executable(index),
        H::LookupSwitch { key, .. } => hir_executable(key),
        H::LoadField { obj, kind, .. } => {
            *kind != ValueKind::Float64 && hir_executable(obj)
        }
        H::StoreField { obj, value, kind, .. } => {
            *kind != ValueKind::Float64 && hir_executable(obj) && hir_executable(value)
        }
        H::GetStatic { kind, .. } => *kind != ValueKind::Float64,
        H::PutStatic { value, kind, .. } => {
            *kind != ValueKind::Float64 && hir_executable(value)
        }
        H::LoadIndexed { array, index, elem } => {
            elem.value_kind() != ValueKind::Float64
                && hir_executable(array)
                && hir_executable(index)
        }
        H::StoreIndexed { array, index, value, elem } => {
            elem.value_kind() != ValueKind::Float64
                && hir_executable(array)
                && hir_executable(index)
                && hir_executable(value)
        }
        H::NewArray { length, .. } | H::AnewArray { length, .. } => hir_executable(length),
        H::NewObject { .. } => true,
        H::CheckCast { value, .. } | H::InstanceOf { value, .. } => hir_executable(value),
        H::Call { operands, .. } => operands.iter().all(|o| hir_executable(o)),
        H::Unsupported { .. } => false,
    }
}

impl AbstractCompiler for C1Compiler {
    fn compiler_type(&self) -> CompilerType { CompilerType::C1 }

    fn compile_method(&mut self, task: &CompileTask) -> Result<CompileOutcome, CompileError> {
        // C1 has no OSR entry support: it would compile the whole method
        // yet the broker installs the result under the OSR bci, where the
        // interpreter would invoke it with the marshal-buffer ABI.
        if task.is_osr() {
            return Err(CompileError::new(
                FailureReason::Unsupported,
                "C1 does not support OSR compilation",
                task.comp_level(),
            ));
        }
        self.compiled_methods += 1;
        let method = task.method_obj.as_ref().ok_or_else(|| CompileError::new(FailureReason::Unresolved, "missing Method handle", CompLevel::Simple))?;
        let bytecode = method.bytecodes();
        if bytecode.is_empty() {
            return Err(CompileError::new(FailureReason::MethodNotCompilable, "empty bytecode", CompLevel::Simple));
        }
        let max_locals = method.max_locals() as usize;
        let descriptor = task.method.descriptor.as_str();
        let Some(signature) = parse_signature(descriptor.as_bytes(), method.is_static()) else {
            return Err(CompileError::new(
                FailureReason::Unsupported,
                format!("signature {descriptor} outside executable subset"),
                CompLevel::Simple,
            ));
        };
        let mut params =
            Vec::with_capacity(signature.params().len() + usize::from(signature.is_instance));
        let mut slot = 0;
        if signature.is_instance {
            params.push(ParamInfo {
                local_slot: slot,
                kind: LirKind::Ref,
            });
            slot += 1;
        }
        for &kind in signature.params() {
            let (kind, width) = match kind {
                SlotKind::Int32 => (ValueKind::Int32, 1),
                SlotKind::Int64 => (ValueKind::Int64, 2),
                SlotKind::Float32 => (ValueKind::Float32, 1),
                SlotKind::Float64 => (ValueKind::Float64, 2),
                SlotKind::Ref => (ValueKind::Ref, 1),
                SlotKind::Void => unreachable!(),
            };
            params.push(ParamInfo {
                local_slot: slot,
                kind: LirKind::from_value_kind(kind),
            });
            slot += width;
        }
        let mut metadata = task.jit_metadata.clone().unwrap_or_default();
        // Heap-op sites and field kinds are resolved from the owning
        // class here (the broker only pre-resolves `ldc` constants),
        // so `CallRuntime` stubs receive real `StubSite` pointers.
        if let Some(loader) = crate::share::runtime::threading::jvm_loader()
            && let Some(klass) = loader
                .classes
                .borrow()
                .get(task.method.class_name.as_bytes())
                .cloned()
        {
            crate::share::c1::c1_GraphBuilder::resolve_heap_sites(
                &klass,
                bytecode,
                &mut metadata,
            );
        }
        CURRENT_METHOD.with(|m| *m.borrow_mut() = task.method.method_key.clone());
        let (code, frame_size, pc_descs) = self
            .compile_to_code_meta(bytecode, max_locals, params, metadata)
            .map_err(|e| match e {
            BackendError::MissingCallTarget | BackendError::MissingRuntimeStub(_) => CompileError::new(FailureReason::Unresolved, e.to_string(), CompLevel::Simple),
            BackendError::Unsupported(_) => CompileError::new(FailureReason::Unsupported, e.to_string(), CompLevel::Simple),
            other => CompileError::new(FailureReason::InternalError, other.to_string(), CompLevel::Simple),
        })?;
        if std::env::var("JVM25_DUMP_JIT").is_ok() {
            eprintln!("[jit-code] {} -> {} bytes frame={}", task.method.method_key, code.len(), frame_size);
            for chunk in code.chunks(16) {
                eprint!("  ");
                for b in chunk { eprint!("{:02x} ", b); }
                eprintln!();
            }
        }
        Ok(CompileOutcome { code_bytes: code, frame_size, pc_descs, ..CompileOutcome::default() })
    }

    fn supports_level(&self, level: CompLevel) -> bool { level.is_c1() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_iconst_ireturn() {
        let mut compiler = C1Compiler::new();
        let bytecode = [0x04, 0xac];
        let (code, frame_size, _) = compiler.compile_to_code(&bytecode, 1, Vec::new()).unwrap();
        assert!(!code.is_empty());
        assert_eq!(code[0], 0x55);
        assert!(code.iter().any(|&b| b == 0xc3));
        assert_eq!(&code[code.len() - 2..], &[0x0f, 0x0b]);
        assert_eq!(frame_size % 16, 0);
    }

    #[test]
    fn compiles_add_param() {
        let mut compiler = C1Compiler::new();
        let bytecode = [0x1a, 0x04, 0x60, 0xac];
        let params = vec![ParamInfo { local_slot: 0, kind: LirKind::Int32 }];
        let (code, _, _) = compiler.compile_to_code(&bytecode, 1, params).unwrap();
        assert!(code.iter().any(|&b| b == 0x03));
        assert!(code.iter().any(|&b| b == 0xc3));
    }
}
