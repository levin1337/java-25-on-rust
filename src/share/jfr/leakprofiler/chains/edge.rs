

use crate::share::jfr::leakprofiler::utilities::root_type::UnifiedOopRef;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from: UnifiedOopRef,
    pub to: UnifiedOopRef,
}

impl Edge {
    pub fn new(from: UnifiedOopRef, to: UnifiedOopRef) -> Self {
        Self { from, to }
    }
}
