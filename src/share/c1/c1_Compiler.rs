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
        let cfg = GraphBuilder::build_full(bytecode, max_locals, metadata.clone());
        let cfg = self.compile_hir(cfg);
        // Executable-tier gate: the backend lowers field/array/allocation/
        // call/monitor ops to raw memory accesses and runtime stubs that
        // do not exist yet. Only methods that stay within the primitive
        // subset may produce runnable code; everything else bails out so
        // the broker marks the method not-compilable instead of emitting
        // a blob that would corrupt the heap if ever entered.
        if !cfg_executable(&cfg) {
            return Err(BackendError::Unsupported(
                "method requires heap access or runtime stubs".to_string(),
            ));
        }
        // Compiled frames are invisible to the GC root snapshot, so a
        // method whose signature passes a reference (receiver included)
        // cannot be safely JIT-compiled yet.
        if params.iter().any(|p| p.kind == LirKind::Ref) {
            return Err(BackendError::Unsupported(
                "method signature carries references".to_string(),
            ));
        }
        let program = LirGenerator::generate(&cfg, &metadata);
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

fn parse_descriptor_params(descriptor: &str, is_static: bool) -> Vec<ParamInfo> {
    let bytes = descriptor.as_bytes();
    let mut params = Vec::new();
    let mut slot = 0usize;
    if !is_static {
        params.push(ParamInfo { local_slot: slot, kind: LirKind::Ref });
        slot += 1;
    }
    if bytes.first() != Some(&b'(') { return params; }
    let mut i = 1usize;
    while i < bytes.len() && bytes[i] != b')' {
        let (kind, width) = match bytes[i] {
            b'B' | b'S' | b'C' | b'I' | b'Z' => (ValueKind::Int32, 1),
            b'J' => (ValueKind::Int64, 2),
            b'F' => (ValueKind::Float32, 1),
            b'D' => (ValueKind::Float64, 2),
            b'L' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b';' { j += 1; }
                i = j;
                (ValueKind::Ref, 1)
            }
            b'[' => {
                let mut depth = 0usize;
                let mut j = i;
                while j < bytes.len() {
                    match bytes[j] {
                        b'[' => depth += 1,
                        b'L' => {
                            let mut k = j + 1;
                            while k < bytes.len() && bytes[k] != b';' { k += 1; }
                            j = k;
                            break;
                        }
                        b';' if depth > 0 => break,
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
                (ValueKind::Ref, 1)
            }
            _ => (ValueKind::Int32, 1),
        };
        params.push(ParamInfo { local_slot: slot, kind: LirKind::from_value_kind(kind) });
        slot += width;
        i += 1;
    }
    params
}

/// True when every instruction in the CFG stays within the executable
/// subset: pure primitive computation, control flow and returns.
fn cfg_executable(cfg: &ControlFlowGraph) -> bool {
    cfg.blocks
        .iter()
        .all(|block| block.instructions.iter().all(hir_executable))
}

/// Recursive gate mirroring the unsafe backend lowerings: anything that
/// dereferences an object/array pointer, allocates, calls out, or needs
/// a runtime stub is rejected. Nested operand expressions are checked
/// the same way.
fn hir_executable(inst: &HirInstruction) -> bool {
    use HirInstruction as H;
    match inst {
        H::ConstantInt(_)
        | H::ConstantLong(_)
        | H::ConstantFloat(_)
        | H::ConstantDouble(_)
        | H::ConstantNull
        | H::Goto(_)
        | H::Neg(_) => {
            // Neg carries an operand; check it.
            match inst {
                H::Neg(v) => hir_executable(v),
                _ => true,
            }
        }
        // Reference-typed frame slots are invisible to the GC root
        // snapshot (which only scans interpreter frames), so a compiled
        // method must never hold a Ref in a local or stack slot.
        H::LoadLocal { kind, .. } | H::LoadStack { kind, .. } => *kind != ValueKind::Ref,
        H::StoreLocal { kind, value, .. } | H::StoreStack { kind, value, .. } => {
            *kind != ValueKind::Ref && hir_executable(value)
        }
        // Monitor enter/exit and athrow lower to `CallRuntime` stubs
        // that operate on object-arena handles — they do not dereference
        // raw pointers, so they are safe to compile.
        H::MonitorEnter(v) | H::MonitorExit(v) | H::Throw(v) => hir_executable(v),
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
        // Heap access, allocation, calls, monitors, casts and throws all
        // require runtime support that does not exist yet.
        H::LoadField { .. }
        | H::StoreField { .. }
        | H::GetStatic { .. }
        | H::PutStatic { .. }
        | H::LoadIndexed { .. }
        | H::StoreIndexed { .. }
        | H::ArrayLength(_)
        | H::NewArray { .. }
        | H::NewObject { .. }
        | H::AnewArray { .. }
        | H::Call { .. }
        | H::CheckCast { .. }
        | H::InstanceOf { .. }
        | H::Unsupported { .. } => false,
    }
}

impl AbstractCompiler for C1Compiler {
    fn compiler_type(&self) -> CompilerType { CompilerType::C1 }

    fn compile_method(&mut self, task: &CompileTask) -> Result<CompileOutcome, CompileError> {
        self.compiled_methods += 1;
        let method = task.method_obj.as_ref().ok_or_else(|| CompileError::new(FailureReason::Unresolved, "missing Method handle", CompLevel::Simple))?;
        let bytecode = method.bytecodes();
        if bytecode.is_empty() {
            return Err(CompileError::new(FailureReason::MethodNotCompilable, "empty bytecode", CompLevel::Simple));
        }
        let max_locals = method.max_locals() as usize;
        // The interpreter<->nmethod bridge can only marshal the
        // executable signature subset (primitive int/long, no floats
        // or references); refuse anything else up front instead of
        // installing code that could never be invoked.
        if !crate::share::jitruntime::signature_supported(
            task.method.descriptor.as_str().as_bytes(),
            method.is_static(),
        ) {
            return Err(CompileError::new(
                FailureReason::Unsupported,
                format!(
                    "signature {} outside executable subset",
                    task.method.descriptor.as_str()
                ),
                CompLevel::Simple,
            ));
        }
        let params = parse_descriptor_params(task.method.descriptor.as_str(), method.is_static());
        let metadata = task.jit_metadata.clone().unwrap_or_default();
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
