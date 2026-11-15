

use super::sampling::object_sampler::ObjectSampler;
use super::utilities::vm_operation::VmOperation;

pub struct StopOperation;

impl VmOperation for StopOperation {
    fn name(&self) -> &'static str {
        "StopOperation"
    }

    fn doit(&mut self) {
        ObjectSampler::destroy();
    }
}
