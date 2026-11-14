// C1 Compiler ported from HotSpot c1_Compiler.hpp / .cpp

use super::c1_GraphBuilder::GraphBuilder;
use super::c1_IR::ControlFlowGraph;
use super::c1_Optimizer::Optimizer;
use crate::share::compiler::abstractCompiler::{AbstractCompiler, CompilerType};
use crate::share::compiler::compileTask::CompileTask;
use crate::share::compiler::compilerDefinitions::CompLevel;
use std::io;

pub struct C1Compiler {
    pub compiled_methods: usize,
}

impl C1Compiler {
    pub fn new() -> Self {
        Self {
            compiled_methods: 0,
        }
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
}

impl Default for C1Compiler {
    fn default() -> Self {
        Self::new()
    }
}

impl AbstractCompiler for C1Compiler {
    fn compiler_type(&self) -> CompilerType {
        CompilerType::C1
    }

    fn compile_method(&mut self, _task: &CompileTask) -> io::Result<()> {
        self.compiled_methods += 1;
        Ok(())
    }

    fn supports_level(&self, level: CompLevel) -> bool {
        level.is_c1()
    }
}
