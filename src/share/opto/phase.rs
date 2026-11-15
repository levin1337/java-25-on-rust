/*
 * Port of OpenJDK 25 HotSpot C2 Compiler Phase (share/opto/phase.hpp / phase.cpp)
 *
 * Phase framework: phase numbering, timing, and the driver routines that run
 * real transformations over the ideal graph (PhaseIterGVN analog, dead-code
 * elimination). Phases that are not yet ported (IdealLoop, MacroExpand,
 * Matcher, scheduler, register allocation, code output) are represented in
 * the PhaseNumber enum but have no implementation.
 */

use super::gvn::PhaseGVN;
use super::loop_analysis::{self, LoopTree};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhaseNumber {
    Compiler,
    Parser,
    RemoveUseless,
    RemoveUselessAndRenumberLive,
    Optimistic,
    GVN,
    InsSelect,
    CFG,
    BlockLayout,
    RegisterAllocation,
    LIVE,
    StringOpts,
    InterferenceGraph,
    Coalesce,
    IdealLoop,
    MacroExpand,
    Peephole,
    Vector,
    Output,
    LastPhase,
}

impl PhaseNumber {
    pub fn name(self) -> &'static str {
        match self {
            PhaseNumber::Compiler => "compiler",
            PhaseNumber::Parser => "parse",
            PhaseNumber::RemoveUseless => "removeUseless",
            PhaseNumber::RemoveUselessAndRenumberLive => "removeUselessAndRenumberLive",
            PhaseNumber::Optimistic => "optimistic",
            PhaseNumber::GVN => "gvn",
            PhaseNumber::InsSelect => "insSelect",
            PhaseNumber::CFG => "cfg",
            PhaseNumber::BlockLayout => "blockLayout",
            PhaseNumber::RegisterAllocation => "regAlloc",
            PhaseNumber::LIVE => "live",
            PhaseNumber::StringOpts => "stringOpts",
            PhaseNumber::InterferenceGraph => "ifg",
            PhaseNumber::Coalesce => "coalesce",
            PhaseNumber::IdealLoop => "idealLoop",
            PhaseNumber::MacroExpand => "macroExpand",
            PhaseNumber::Peephole => "peephole",
            PhaseNumber::Vector => "vector",
            PhaseNumber::Output => "output",
            PhaseNumber::LastPhase => "last",
        }
    }
}

pub struct Phase {
    pub number: PhaseNumber,
    pub name: &'static str,
    start_time: Instant,
    elapsed: Duration,
}

impl Phase {
    pub fn new(number: PhaseNumber) -> Self {
        Self {
            number,
            name: number.name(),
            start_time: Instant::now(),
            elapsed: Duration::ZERO,
        }
    }

    pub fn with_name(number: PhaseNumber, name: &'static str) -> Self {
        Self {
            number,
            name,
            start_time: Instant::now(),
            elapsed: Duration::ZERO,
        }
    }

    pub fn complete(&mut self) -> Duration {
        self.elapsed = self.start_time.elapsed();
        self.elapsed
    }

    pub fn elapsed(&self) -> Duration {
        if self.elapsed == Duration::ZERO {
            self.start_time.elapsed()
        } else {
            self.elapsed
        }
    }
}

pub struct PhaseIterGVN;

impl PhaseIterGVN {
    pub fn optimize(gvn: &mut PhaseGVN) -> usize {
        gvn.iter_gvn()
    }
}

pub struct PhaseRemoveUseless;

impl PhaseRemoveUseless {
    pub fn optimize(gvn: &mut PhaseGVN) -> usize {
        gvn.dead_code_elimination()
    }
}

pub struct PhaseRemoveUselessAndRenumber;

impl PhaseRemoveUselessAndRenumber {
    pub fn optimize(gvn: &mut PhaseGVN) -> usize {
        gvn.dead_code_elimination()
    }
}

pub struct PhaseCCP;

impl PhaseCCP {
    pub fn optimize(gvn: &mut PhaseGVN) -> usize {
        gvn.iter_gvn()
    }
}

pub struct PhaseTyperRun;

impl PhaseTyperRun {
    pub fn optimize(gvn: &PhaseGVN) -> super::typers::PhaseTyper<'_> {
        let mut typer = super::typers::PhaseTyper::new(gvn);
        typer.optimize();
        typer
    }
}

pub struct PhaseCanonicalizerRun;

impl PhaseCanonicalizerRun {
    pub fn optimize(gvn: &mut PhaseGVN) -> usize {
        super::canonicalizer::PhaseCanonicalizer::optimize(gvn)
    }
}

pub struct PhaseMacroExpandRun<'a> {
    inner: super::macroexpand::PhaseMacroExpand<'a>,
}

impl<'a> PhaseMacroExpandRun<'a> {
    pub fn new() -> Self {
        Self {
            inner: super::macroexpand::PhaseMacroExpand::new(),
        }
    }

    pub fn optimize(&mut self, gvn: &mut PhaseGVN) -> usize {
        self.inner.optimize(gvn)
    }
}

impl Default for PhaseMacroExpandRun<'_> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PhaseIdealLoopRun;

pub struct IdealLoopResult {
    pub tree: LoopTree,
    pub invariants: Vec<std::collections::HashSet<super::node::NodeId>>,
}

impl PhaseIdealLoopRun {
    pub fn optimize(gvn: &PhaseGVN) -> IdealLoopResult {
        let tree = loop_analysis::LoopAnalysis::analyze(gvn);
        let invariants = tree
            .loops
            .iter()
            .map(|l| loop_analysis::loop_invariant_nodes(gvn, &l.body))
            .collect();
        IdealLoopResult { tree, invariants }
    }
}

pub struct PhaseScheduleBoundary;

impl PhaseScheduleBoundary {
    pub fn run_scheduler(gvn: &PhaseGVN) -> super::scheduler::Schedule {
        super::scheduler::Scheduler::new(gvn).schedule()
    }

    pub fn run_regalloc(
        gvn: &PhaseGVN,
        schedule: &super::scheduler::Schedule,
        param_slots: usize,
    ) -> Result<super::regalloc::Allocation, String> {
        super::regalloc::RegisterAllocator::new(gvn, schedule).allocate(param_slots)
    }
}
