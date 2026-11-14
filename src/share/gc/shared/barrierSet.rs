use crate::share::oops::oop::Oop;

pub trait BarrierSet {
    fn write_ref_field_pre(&self, _field_addr: usize) {}
    fn write_ref_field_post(&self, _field_addr: usize, _new_val: Oop) {}
    fn read_ref_field(&self, _field_addr: usize) {}
}

pub struct ModRefBarrierSet;

impl BarrierSet for ModRefBarrierSet {}
