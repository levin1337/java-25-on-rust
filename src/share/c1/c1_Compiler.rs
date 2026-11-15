use super::asm::codeBuffer::{CodeBuffer, DEFAULT_BUFFER_SIZE};
use super::asm::x86::{emit_program, BackendError, BackendOptions, ParamInfo};
use super::c1_GraphBuilder::GraphBuilder;
use super::c1_IR::ControlFlowGraph;
use super::c1_LIR::{LirGenerator, LirKind};
use super::c1_Optimizer::Optimizer;
use crate::share::code::pcDesc::PcDesc;
use crate::share::compiler::abstractCompiler::{AbstractCompiler, CompileError, CompilerType};
use crate::share::compiler::compileTask::{CompileOutcome, CompileTask, FailureReason};
use crate::share::compiler::compilerDefinitions::CompLevel;
use super::c1_Instruction::ValueKind;
use std::io;

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
        let cfg = GraphBuilder::build(bytecode, max_locals);
        let cfg = self.compile_hir(cfg);
        let program = LirGenerator::generate(&cfg);
        let options = BackendOptions { params, ..BackendOptions::default() };
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
        let params = parse_descriptor_params(task.method.descriptor.as_str(), method.is_static());
        let (code, frame_size, pc_descs) = self.compile_to_code(bytecode, max_locals, params).map_err(|e| match e {
            BackendError::MissingCallTarget | BackendError::MissingRuntimeStub(_) => CompileError::new(FailureReason::Unresolved, e.to_string(), CompLevel::Simple),
            BackendError::Unsupported(_) => CompileError::new(FailureReason::Unsupported, e.to_string(), CompLevel::Simple),
            other => CompileError::new(FailureReason::InternalError, other.to_string(), CompLevel::Simple),
        })?;
        Ok(CompileOutcome { code_bytes: code, frame_size, pc_descs, ..CompileOutcome::default() })
    }

    fn supports_level(&self, level: CompLevel) -> bool { level.is_c1() }
}
