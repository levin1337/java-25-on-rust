// Abstract compiler interface ported from HotSpot abstractCompiler.hpp / .cpp

use super::compileTask::CompileTask;
use super::compilerDefinitions::CompLevel;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompilerType {
    None,
    C1,
    C2,
    JVMCI,
}

impl CompilerType {
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::C1 => "c1",
            Self::C2 => "c2",
            Self::JVMCI => "jvmci",
        }
    }
}

pub trait AbstractCompiler {
    fn compiler_type(&self) -> CompilerType;
    fn compile_method(&mut self, task: &CompileTask) -> io::Result<()>;
    fn supports_level(&self, level: CompLevel) -> bool;
}
