use crate::share::gc::shared::barrierSet::BarrierSet;

/// SerialGC write barrier. HotSpot SerialGC is a non-moving
/// mark-sweep(-compact) over a contiguous heap; object slots here are
/// arena indices that never move, so no SATB, card-table or
/// generational barrier is required. The barrier set is intentionally
/// empty.
pub struct SerialBarrierSet;

impl BarrierSet for SerialBarrierSet {}
