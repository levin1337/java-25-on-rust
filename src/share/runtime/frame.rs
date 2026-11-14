use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::method::Code;

pub struct StackFrame<'a> {
    pub code: Code<'a>,
    pub bci: usize,
    pub locals: Vec<Value<'a>>,
    pub stack: Vec<Value<'a>>,
}

impl<'a> StackFrame<'a> {
    pub fn new(code: Code<'a>, locals: Vec<Value<'a>>) -> Self {
        Self {
            stack: Vec::with_capacity(code.maxStack),
            code,
            bci: 0,
            locals,
        }
    }

    pub fn bci(&self) -> usize {
        self.bci
    }

    pub fn set_bci(&mut self, bci: usize) {
        self.bci = bci;
    }
}
