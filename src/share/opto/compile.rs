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
use super::codegen::{compile_schedule, BackendOptions, BackendError};
use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode};
use super::phase::{Phase, PhaseCCP, PhaseIterGVN, PhaseNumber, PhaseRemoveUselessAndRenumber};
use super::regalloc::RegisterAllocator;
use super::scheduler::Scheduler;

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

    /// Runs the ported C2 backend: schedule the optimized ideal graph into
    /// blocks, allocate registers/stack slots with the linear-scan allocator,
    /// and emit x86-64 machine code plus frame/pc metadata.
    ///
    /// Returns the emitter result; unsupported graph features surface as
    /// `BackendError::Unsupported` instead of incorrect code.
    pub fn backend(
        &mut self,
        method: &MethodCode,
        _ctx: &BuilderContext,
        options: &BackendOptions,
    ) -> Result<super::codegen::EmitResult, BackendError> {
        self.optimize();
        let schedule = Scheduler::new(&self.gvn).schedule();
        if !schedule.unsupported.is_empty() {
            return Err(BackendError::Unsupported(format!(
                "scheduler rejected graph: {}",
                schedule.unsupported.join("; ")
            )));
        }
        let alloc = RegisterAllocator::new(&self.gvn, &schedule)
            .allocate(method.param_slots)
            .map_err(BackendError::InvalidGraph)?;
        let buf = super::codegen::default_buffer()?;
        compile_schedule(&buf, &self.gvn, &schedule, &alloc, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method(code: &'static [u8], param_slots: usize) -> MethodCode<'static> {
        MethodCode {
            name: "test",
            max_locals: param_slots.max(1),
            max_stack: 8,
            param_slots,
            code,
            handlers: &[],
        }
    }

    #[test]
    fn compiles_iconst_ireturn() {
        // iconst_5 ; ireturn
        let code: &[u8] = &[0x08, 0xac];
        let mut c = Compile::compile(&method(code, 0), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 0), &BuilderContext::empty(), &BackendOptions::default())
            .expect("backend should emit code");
        assert!(!emitted.code.is_empty());
        assert!(emitted.code.iter().any(|&b| b == 0xc3)); // ret
        assert_eq!(emitted.code[emitted.code.len() - 2..], [0x0f, 0x0b]);
    }

    #[test]
    fn compiles_add_and_return() {
        // iconst_2 ; iconst_3 ; iadd ; ireturn
        let code: &[u8] = &[0x05, 0x06, 0x60, 0xac];
        let mut c = Compile::compile(&method(code, 0), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 0), &BuilderContext::empty(), &BackendOptions::default())
            .expect("backend should emit code");
        assert!(emitted.code.iter().any(|&b| b == 0x03)); // add r,r/m
        assert!(emitted.code.iter().any(|&b| b == 0xc3));
    }

    #[test]
    fn compiles_param_add() {
        // iload_0 ; iconst_1 ; iadd ; ireturn   (static (I)I, param slot 0)
        let code: &[u8] = &[0x1a, 0x04, 0x60, 0xac];
        let params = vec![super::super::codegen::ParamSlot {
            local_slot: 0,
            kind: super::super::codegen::ParamKind::I32,
        }];
        let options = BackendOptions {
            params,
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("backend should emit code");
        assert!(emitted.code.iter().any(|&b| b == 0xc3));
    }
}
