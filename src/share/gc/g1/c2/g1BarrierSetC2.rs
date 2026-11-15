/*
 * Port of OpenJDK 25 HotSpot G1BarrierSetC2
 * (share/gc/g1/c2/g1BarrierSetC2.hpp) — the C2 (server compiler)
 * G1 barrier set.
 *
 * C2 lowers G1 barriers to ideal-graph nodes then macro-expands them
 * into stub calls (G1PreBarrierStubC2 for SATB, G1PostBarrierStubC2
 * for card marking + remembered-set enqueue). This port models the
 * barrier kinds, the stub descriptors, and the optimization hooks
 * (dominated-barrier elimination, late barrier analysis) as data;
 * graph integration happens in `share/opto` later.
 */

/// `G1BarrierStubC2` kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum G1BarrierStubKind {
    /// SATB pre-write barrier.
    PreBarrier,
    /// Post-write card-marking barrier.
    PostBarrier,
    /// Runtime call fallback for complex cases (e.g. humongous,
    /// optional regions).
    RuntimeCall,
}

/// `G1PreBarrierStubC2` register assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct G1PreBarrierStubC2Desc {
    pub kind: G1BarrierStubKind,
    /// Object base register, pre-value register, thread register,
    /// scratch registers (indices into the C2 register map).
    pub obj: usize,
    pub pre_val: usize,
    pub thread: usize,
    pub tmp1: usize,
    pub tmp2: usize,
    /// Whether the stub loads the pre-value itself.
    pub do_load: bool,
}

/// `G1PostBarrierStubC2` register assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct G1PostBarrierStubC2Desc {
    pub kind: G1BarrierStubKind,
    pub thread: usize,
    pub tmp1: usize,
    pub tmp2: usize,
    pub tmp3: usize,
}

/// `G1BarrierSetC2`: C2 barrier set descriptor.
#[derive(Debug, Default)]
pub struct G1BarrierSetC2 {
    /// Dominating-barrier analysis results: node -> barrier kind that
    /// was eliminated as redundant.
    eliminated_barriers: Vec<(usize, G1BarrierStubKind)>,
}

impl G1BarrierSetC2 {
    pub fn new() -> Self {
        Self::default()
    }

    /// `pre_barrier` lowering: produces the stub descriptor for a
    /// pre-write barrier on `obj`/`pre_val`.
    pub fn pre_barrier_stub(&self, obj: usize, pre_val: usize, thread: usize) -> G1PreBarrierStubC2Desc {
        G1PreBarrierStubC2Desc {
            kind: G1BarrierStubKind::PreBarrier,
            obj,
            pre_val,
            thread,
            tmp1: usize::MAX,
            tmp2: usize::MAX,
            do_load: true,
        }
    }

    /// `post_barrier` lowering.
    pub fn post_barrier_stub(&self, thread: usize) -> G1PostBarrierStubC2Desc {
        G1PostBarrierStubC2Desc {
            kind: G1BarrierStubKind::PostBarrier,
            thread,
            tmp1: usize::MAX,
            tmp2: usize::MAX,
            tmp3: usize::MAX,
        }
    }

    /// `eliminate_gc_barrier`: records that `node`'s barrier of `kind`
    /// was proven redundant by dominating-barrier analysis.
    pub fn eliminate_gc_barrier(&mut self, node: usize, kind: G1BarrierStubKind) {
        self.eliminated_barriers.push((node, kind));
    }

    /// `eliminate_gc_barrier_data`.
    pub fn eliminate_gc_barrier_data(&mut self, node: usize) {
        self.eliminated_barriers.retain(|(n, _)| *n != node);
    }

    /// `elide_dominated_barrier`: whether `node`'s barrier was
    /// elided (dominated by an earlier equivalent barrier).
    pub fn is_barrier_elided(&self, node: usize, kind: G1BarrierStubKind) -> bool {
        self.eliminated_barriers.iter().any(|(n, k)| *n == node && *k == kind)
    }

    /// `analyze_dominating_barriers` hook: placeholder for the C2
    /// phase that proves redundancy; the analysis itself runs over
    /// the ideal graph in `share/opto`.
    pub fn analyze_dominating_barriers(&self) {}

    /// `emit_stubs`: stub emission is done by the C2 code generator;
    /// this only reports which stub kinds are needed.
    pub fn needed_stub_kinds(&self) -> Vec<G1BarrierStubKind> {
        let mut kinds = vec![G1BarrierStubKind::PreBarrier, G1BarrierStubKind::PostBarrier];
        kinds.sort_by_key(|k| *k as u8);
        kinds.dedup();
        kinds
    }
}
