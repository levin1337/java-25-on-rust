

use super::utilities::vm_operation::VmOperation;
use super::sampling::object_sampler::ObjectSampler;

pub struct StartOperation {
    sample_count: usize,
}

impl StartOperation {
    pub fn new(sample_count: usize) -> Self {
        Self { sample_count }
    }
}

impl VmOperation for StartOperation {
    fn name(&self) -> &'static str {
        "StartOperation"
    }

    fn doit(&mut self) {
        ObjectSampler::create(self.sample_count);
    }
}
