/*
 * Port of OpenJDK 25 HotSpot C2 Compiler Context (share/opto/compile.hpp / compile.cpp)
 *
 * Coordinates C2 compilation for a method: parses bytecode into the initial
 * Ideal Graph (Parse analog), runs optimization phases (iterative GVN,
 * dead-code elimination with renumbering), and exposes the optimized graph.
 * Scheduling, instruction selection, register allocation and code emission
 * are not yet ported.
 */

use super::builder::{BytecodeGraphBuilder, BuildResult, BuilderContext, MethodCode};
use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode};
use super::phase::{Phase, PhaseCCP, PhaseIterGVN, PhaseNumber, PhaseRemoveUselessAndRenumber};

pub struct Compile {
    pub method_name: String,
    pub gvn: PhaseGVN,
    pub root: NodeId,
    pub start: NodeId,
    phases_run: Vec<Phase>,
    pub build: Option<BuildResult>,
    pub optimized: bool,
}

impl Compile {
    pub fn compile(method: &MethodCode, ctx: &BuilderContext) -> Self {
        let mut gvn = PhaseGVN::new();
        let start = gvn.transform(Opcode::Start, vec![]);
        let root = gvn.transform(Opcode::Root, vec![start]);
        let mut c = Self {
            method_name: method.name.to_string(),
            gvn,
            root,
            start,
            phases_run: Vec::new(),
            build: None,
            optimized: false,
        };
        c.build_graph(method, ctx);
        c
    }

    pub fn build_graph(&mut self, method: &MethodCode, ctx: &BuilderContext) {
        let mut phase = Phase::new(PhaseNumber::Parser);
        let res = BytecodeGraphBuilder::build(
            &mut self.gvn,
            method,
            ctx,
            self.start,
            self.root,
        );
        phase.complete();
        self.phases_run.push(phase);
        self.build = Some(res);
    }

    pub fn optimize(&mut self) {
        if self.optimized {
            return;
        }
        {
            let mut phase = Phase::new(PhaseNumber::GVN);
            PhaseIterGVN::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        {
            let mut phase = Phase::new(PhaseNumber::RemoveUselessAndRenumberLive);
            PhaseRemoveUselessAndRenumber::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        {
            let mut phase = Phase::new(PhaseNumber::Optimistic);
            PhaseCCP::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        {
            let mut phase = Phase::new(PhaseNumber::RemoveUseless);
            PhaseRemoveUselessAndRenumber::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        self.optimized = true;
    }

    pub fn node_count(&self) -> usize {
        self.gvn.node_count()
    }

    pub fn phases(&self) -> &[Phase] {
        &self.phases_run
    }

    pub fn has_pending_merges(&self) -> bool {
        self.gvn.has_pending_inputs() > 0
    }
}
