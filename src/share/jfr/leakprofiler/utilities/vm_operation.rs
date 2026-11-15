

pub trait VmOperation {
    fn name(&self) -> &'static str;
    fn doit(&mut self);
}

pub fn execute_operation(op: &mut dyn VmOperation) {
    op.doit();
}
