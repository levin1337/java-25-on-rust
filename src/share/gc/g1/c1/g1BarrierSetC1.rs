/*
 * Port of OpenJDK 25 HotSpot G1BarrierSetC1
 * (share/gc/g1/c1/g1BarrierSetC1.hpp) — the C1 (client/compiler-1)
 * G1 barrier set.
 *
 * C1 barriers are generated per call-site via G1PreBarrierStub
 * (SATB pre-write: records the previous value into the SATB queue)
 * and G1PostBarrierStub (post-write: dirties the card and enqueues
 * the from-card for the remembered set). This port captures the
 * stub parameters and dispatch semantics; actual LIR/code emission
 * is a later integration point with `share/c1`.
 */

/// `G1PreBarrierStub` parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct G1PreBarrierStubDesc {
    /// Whether the stub emits a load of the previous value from the
    /// field address (`do_load`) or expects it in a register.
    pub do_load: bool,
    /// Field address operand (opaque handle in this port).
    pub addr: usize,
    /// Previous-value operand.
    pub pre_val: usize,
}

impl G1PreBarrierStubDesc {
    /// Version that does generate a load of the previous value.
    pub fn with_load(addr: usize, pre_val: usize) -> Self {
        Self { do_load: true, addr, pre_val }
    }

    /// Version that assumes the previous value was already loaded.
    pub fn without_load(pre_val: usize) -> Self {
        Self { do_load: false, addr: usize::MAX, pre_val }
    }
}

/// `G1PostBarrierStub` parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct G1PostBarrierStubDesc {
    /// Field address being written.
    pub addr: usize,
    /// New value being stored.
    pub new_val: usize,
    /// Store is a volatile/ordered store (affects barrier ordering).
    pub is_volatile: bool,
}

/// `G1BarrierSetC1`: C1 barrier set descriptor.
#[derive(Debug, Default)]
pub struct G1BarrierSetC1 {
    /// Runtime code blob handles for the shared pre/post barrier
    /// stubs (populated by `generate_c1_runtime_stubs`).
    pub pre_barrier_c1_runtime_code_blob: Option<usize>,
    pub post_barrier_c1_runtime_code_blob: Option<usize>,
}

impl G1BarrierSetC1 {
    pub fn new() -> Self {
        Self::default()
    }

    /// `pre_barrier`: invoked before a reference-field store. Returns
    /// the stub descriptor the C1 LIR builder should emit.
    pub fn pre_barrier(&self, addr: usize, pre_val: usize, already_loaded: bool) -> G1PreBarrierStubDesc {
        if already_loaded {
            G1PreBarrierStubDesc::without_load(pre_val)
        } else {
            G1PreBarrierStubDesc::with_load(addr, pre_val)
        }
    }

    /// `post_barrier`: invoked after a reference-field store.
    pub fn post_barrier(&self, addr: usize, new_val: usize, is_volatile: bool) -> G1PostBarrierStubDesc {
        G1PostBarrierStubDesc { addr, new_val, is_volatile }
    }

    /// `load_at_resolved`: C1 read barrier — G1 has no read barrier
    /// for SATB, only card-table marking on the reference's region
    /// (used for optional-region humongous checks). Returns whether a
    /// barrier is required for the loaded value.
    pub fn load_at_resolved_needs_barrier(&self, _addr: usize, _value: usize) -> bool {
        // G1 C1 emits a barrier only when the loaded value may point
        // into an optional/humongous region; conservatively we report
        // true and let the runtime filter.
        true
    }

    /// `generate_c1_runtime_stubs`: registers the shared runtime
    /// blobs; called once during code cache setup.
    pub fn generate_c1_runtime_stubs(&mut self, pre_blob: usize, post_blob: usize) {
        self.pre_barrier_c1_runtime_code_blob = Some(pre_blob);
        self.post_barrier_c1_runtime_code_blob = Some(post_blob);
    }
}
