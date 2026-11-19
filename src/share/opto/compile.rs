/*
 * Port of OpenJDK 25 HotSpot C2 Compiler Context (share/opto/compile.hpp / compile.cpp)
 *
 * Coordinates C2 compilation for a method: parses bytecode into the initial
 * Ideal Graph (Parse analog), runs optimization phases (iterative GVN,
 * dead-code elimination with renumbering), and exposes the optimized graph.
 * Scheduling, instruction selection, register allocation and code emission
 * are not yet ported.
 */

use super::builder::{BuildResult, BuilderContext, BytecodeGraphBuilder, MethodCode};
use super::codegen::{BackendError, BackendOptions, compile_schedule};
use super::gvn::PhaseGVN;
use super::node::{NodeId, Opcode};
use super::phase::PhaseIdealLoopRun;
use super::phase::{
    IdealLoopResult, Phase, PhaseCCP, PhaseCanonicalizerRun, PhaseIterGVN, PhaseMacroExpandRun,
    PhaseNumber, PhaseRemoveUselessAndRenumber, PhaseTyperRun,
};
use super::regalloc::RegisterAllocator;
use super::scheduler::Scheduler;
use super::r#type::Type;

pub struct Compile {
    pub method_name: String,
    pub gvn: PhaseGVN,
    pub root: NodeId,
    pub start: NodeId,
    phases_run: Vec<Phase>,
    pub build: Option<BuildResult>,
    pub optimized: bool,
    pub node_types: Vec<Type>,
    pub loops: Option<IdealLoopResult>,
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
            node_types: Vec::new(),
            loops: None,
        };
        c.build_graph(method, ctx);
        c
    }

    pub fn build_graph(&mut self, method: &MethodCode, ctx: &BuilderContext) {
        let mut phase = Phase::new(PhaseNumber::Parser);
        let res = BytecodeGraphBuilder::build(&mut self.gvn, method, ctx, self.start, self.root);
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
            let mut phase = Phase::with_name(PhaseNumber::GVN, "typer");
            let typer = PhaseTyperRun::optimize(&self.gvn);
            self.node_types = typer.types;
            phase.complete();
            self.phases_run.push(phase);
        }
        {
            let mut phase = Phase::with_name(PhaseNumber::Optimistic, "canonicalizer");
            PhaseCanonicalizerRun::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        {
            let mut phase = Phase::with_name(PhaseNumber::MacroExpand, "macroExpand");
            let mut me = PhaseMacroExpandRun::new();
            me.optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        {
            let mut phase = Phase::new(PhaseNumber::IdealLoop);
            self.loops = Some(PhaseIdealLoopRun::optimize(&self.gvn));
            phase.complete();
            self.phases_run.push(phase);
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
            .backend(
                &method(code, 0),
                &BuilderContext::empty(),
                &BackendOptions::default(),
            )
            .expect("backend should emit code");
        assert!(!emitted.code.is_empty());
        assert!(emitted.code.iter().any(|&b| b == 0xc3)); // ret
        assert_eq!(emitted.code[emitted.code.len() - 2..], [0x0f, 0x0b]);
    }

    #[test]
    fn compiles_add_and_return() {
        // iload_0 ; iload_0 ; iadd ; ireturn   (static (I)I, param slot 0)
        let code: &[u8] = &[0x1a, 0x1a, 0x60, 0xac];
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(
                &method(code, 1),
                &BuilderContext::empty(),
                &BackendOptions::default(),
            )
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

    fn stub_map(
        entries: &[(crate::share::c1::c1_LIR::RuntimeStub, u64)],
    ) -> std::collections::BTreeMap<crate::share::c1::c1_LIR::RuntimeStub, u64> {
        entries.iter().copied().collect()
    }

    #[test]
    fn compiles_idiv_with_arith_stub() {
        // iload_0 ; iload_1 ; idiv ; ireturn   (static (I,I)I)
        let code: &[u8] = &[0x1a, 0x1b, 0x6c, 0xac];
        let params = vec![
            super::super::codegen::ParamSlot {
                local_slot: 0,
                kind: super::super::codegen::ParamKind::I32,
            },
            super::super::codegen::ParamSlot {
                local_slot: 1,
                kind: super::super::codegen::ParamKind::I32,
            },
        ];
        let options = BackendOptions {
            params,
            stubs: stub_map(&[(
                crate::share::c1::c1_LIR::RuntimeStub::ArithmeticException,
                0x1111,
            )]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 2), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 2), &BuilderContext::empty(), &options)
            .expect("backend should emit idiv");
        assert!(
            emitted.code.windows(2).any(|w| w == [0xf7, 0xf9]),
            "idiv ecx missing: {:02x?}",
            emitted.code
        );
        assert!(
            emitted.code.windows(2).any(|w| w == [0xff, 0xd0]),
            "stub call missing"
        );
        assert!(
            emitted.code.windows(2).any(|w| w == [0x0f, 0x0b]),
            "ud2 trap missing"
        );
    }

    #[test]
    fn idiv_requires_arith_stub() {
        let code: &[u8] = &[0x1a, 0x1b, 0x6c, 0xac];
        let params = vec![
            super::super::codegen::ParamSlot {
                local_slot: 0,
                kind: super::super::codegen::ParamKind::I32,
            },
            super::super::codegen::ParamSlot {
                local_slot: 1,
                kind: super::super::codegen::ParamKind::I32,
            },
        ];
        let options = BackendOptions {
            params,
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 2), &BuilderContext::empty());
        assert!(
            c.backend(&method(code, 2), &BuilderContext::empty(), &options)
                .is_err()
        );
    }

    #[test]
    fn compiles_iaload_via_stub() {
        // aload_0 ; iconst_0 ; iaload ; ireturn   (static ([I)I)
        let code: &[u8] = &[0x2a, 0x03, 0x2e, 0xac];
        let params = vec![super::super::codegen::ParamSlot {
            local_slot: 0,
            kind: super::super::codegen::ParamKind::Ref,
        }];
        let options = BackendOptions {
            params,
            stubs: stub_map(&[(crate::share::c1::c1_LIR::RuntimeStub::ArrayLoad, 0x2222)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("backend should emit array load");
        assert!(
            emitted
                .code
                .windows(6)
                .any(|w| w == [0x41, 0xb8, 0x00, 0x00, 0x00, 0x00]),
            "mov r8d,kind(0) missing: {:02x?}",
            emitted.code
        );
        assert!(
            emitted.code.windows(2).any(|w| w == [0xff, 0xd0]),
            "stub call missing"
        );
    }

    #[test]
    fn compiles_iastore_via_stub() {
        // aload_0 ; iconst_0 ; iconst_5 ; iastore ; return   (static ([II)V)
        let code: &[u8] = &[0x2a, 0x03, 0x08, 0x4f, 0xb1];
        let params = vec![super::super::codegen::ParamSlot {
            local_slot: 0,
            kind: super::super::codegen::ParamKind::Ref,
        }];
        let options = BackendOptions {
            params,
            stubs: stub_map(&[(crate::share::c1::c1_LIR::RuntimeStub::ArrayStore, 0x3333)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("backend should emit array store");
        assert!(
            emitted
                .code
                .windows(6)
                .any(|w| w == [0x41, 0xb9, 0x00, 0x00, 0x00, 0x00]),
            "mov r9d,kind(0) missing: {:02x?}",
            emitted.code
        );
        assert!(
            emitted.code.windows(2).any(|w| w == [0xff, 0xd0]),
            "stub call missing"
        );
    }

    #[test]
    fn compiles_arraylength_via_stub() {
        // aload_0 ; arraylength ; ireturn   (static ([I)I)
        let code: &[u8] = &[0x2a, 0xbe, 0xac];
        let params = vec![super::super::codegen::ParamSlot {
            local_slot: 0,
            kind: super::super::codegen::ParamKind::Ref,
        }];
        let options = BackendOptions {
            params,
            stubs: stub_map(&[(crate::share::c1::c1_LIR::RuntimeStub::ArrayLength, 0x4444)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("backend should emit arraylength");
        assert!(
            emitted.code.windows(2).any(|w| w == [0xff, 0xd0]),
            "stub call missing"
        );
    }

    #[test]
    fn array_ops_require_stubs() {
        let code: &[u8] = &[0x2a, 0xbe, 0xac];
        let params = vec![super::super::codegen::ParamSlot {
            local_slot: 0,
            kind: super::super::codegen::ParamKind::Ref,
        }];
        let options = BackendOptions {
            params,
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        assert!(
            c.backend(&method(code, 1), &BuilderContext::empty(), &options)
                .is_err()
        );
    }

    use super::super::builder::SiteInfo;
    use std::collections::HashMap;

    fn sites(entries: &[(u16, &str, &str, &str)]) -> HashMap<u16, SiteInfo> {
        entries
            .iter()
            .map(|&(cp, class, name, desc)| {
                (
                    cp,
                    SiteInfo {
                        class: class.to_string(),
                        name: name.to_string(),
                        desc: desc.to_string(),
                    },
                )
            })
            .collect()
    }

    fn ref_param() -> Vec<super::super::codegen::ParamSlot> {
        vec![super::super::codegen::ParamSlot {
            local_slot: 0,
            kind: super::super::codegen::ParamKind::Ref,
        }]
    }

    fn has_stub_call(code: &[u8], addr: u64) -> bool {
        code.windows(12).any(|w| {
            w[0] == 0x48
                && w[1] == 0xb8
                && u64::from_le_bytes(w[2..10].try_into().unwrap()) == addr
                && w[10] == 0xff
                && w[11] == 0xd0
        })
    }

    fn backend_with(
        code: &'static [u8],
        param_slots: usize,
        ctx: &BuilderContext,
        stub: crate::share::c1::c1_LIR::RuntimeStub,
        addr: u64,
    ) -> Result<super::super::codegen::EmitResult, BackendError> {
        let params = if param_slots > 0 {
            ref_param()
        } else {
            Vec::new()
        };
        let options = BackendOptions {
            params,
            stubs: stub_map(&[(stub, addr)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, param_slots), ctx);
        c.backend(&method(code, param_slots), ctx, &options)
    }

    const FIELD_SITE: &[(u16, &str, &str, &str)] = &[(1, "pkg/Foo", "x", "I")];
    const CLASS_SITE: &[(u16, &str, &str, &str)] = &[(1, "pkg/Foo", "", "")];
    const CALL_SITE: &[(u16, &str, &str, &str)] = &[(1, "pkg/Foo", "m", "(I)V")];

    #[test]
    fn compiles_getfield_via_stub() {
        // aload_0 ; getfield #1 ; pop ; return
        let code: &[u8] = &[0x2a, 0xb4, 0x00, 0x01, 0x57, 0xb1];
        let s = sites(FIELD_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            1,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::GetField,
            0x9001,
        )
        .expect("getfield should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9001),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_putfield_via_stub() {
        // aload_0 ; iconst_5 ; putfield #1 ; return
        let code: &[u8] = &[0x2a, 0x08, 0xb5, 0x00, 0x01, 0xb1];
        let s = sites(FIELD_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            1,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::PutField,
            0x9002,
        )
        .expect("putfield should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9002),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_static_field_via_stub() {
        // getstatic #1 ; pop ; return
        let code: &[u8] = &[0xb2, 0x00, 0x01, 0x57, 0xb1];
        let s = sites(FIELD_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            0,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::GetStatic,
            0x9003,
        )
        .expect("getstatic should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9003),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_putstatic_via_stub() {
        // iconst_5 ; putstatic #1 ; return
        let code: &[u8] = &[0x08, 0xb3, 0x00, 0x01, 0xb1];
        let s = sites(FIELD_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            0,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::PutStatic,
            0x9004,
        )
        .expect("putstatic should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9004),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_aaload_via_stub() {
        // aload_0 ; iconst_0 ; aaload ; pop ; return
        let code: &[u8] = &[0x2a, 0x03, 0x32, 0x57, 0xb1];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[(crate::share::c1::c1_LIR::RuntimeStub::ArrayLoad, 0x9010)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("aaload should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9010),
            "{:02x?}",
            emitted.code
        );
        assert!(
            emitted
                .code
                .windows(6)
                .any(|w| w == [0x41, 0xb8, 0x0c, 0x00, 0x00, 0x00]),
            "mov r8d,kind(12) missing: {:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_aastore_via_stub() {
        // aload_0 ; iconst_0 ; aload_0 ; aastore ; return
        let code: &[u8] = &[0x2a, 0x03, 0x2a, 0x53, 0xb1];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[(crate::share::c1::c1_LIR::RuntimeStub::ArrayStore, 0x9011)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("aastore should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9011),
            "{:02x?}",
            emitted.code
        );
        assert!(
            emitted
                .code
                .windows(6)
                .any(|w| w == [0x41, 0xb9, 0x0c, 0x00, 0x00, 0x00]),
            "mov r9d,kind(12) missing: {:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_new_via_stub() {
        // new #1 ; pop ; return
        let code: &[u8] = &[0xbb, 0x00, 0x01, 0x57, 0xb1];
        let s = sites(CLASS_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            0,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::NewInstance,
            0x9020,
        )
        .expect("new should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9020),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_newarray_via_stub() {
        // iconst_5 ; newarray 10 ; pop ; return
        let code: &[u8] = &[0x08, 0xbc, 0x0a, 0x57, 0xb1];
        let emitted = backend_with(
            code,
            0,
            &BuilderContext::empty(),
            crate::share::c1::c1_LIR::RuntimeStub::NewArray,
            0x9021,
        )
        .expect("newarray should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9021),
            "{:02x?}",
            emitted.code
        );
        assert!(
            emitted
                .code
                .windows(5)
                .any(|w| w == [0xba, 0x0a, 0x00, 0x00, 0x00]),
            "mov edx,10 missing: {:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_anewarray_via_stub() {
        // iconst_5 ; anewarray #1 ; pop ; return
        let code: &[u8] = &[0x08, 0xbd, 0x00, 0x01, 0x57, 0xb1];
        let s = sites(CLASS_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            0,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::AnewArray,
            0x9022,
        )
        .expect("anewarray should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9022),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_multianewarray_via_stub() {
        // iconst_5 ; iconst_5 ; multianewarray #1 2 ; pop ; return
        let code: &[u8] = &[0x08, 0x08, 0xc5, 0x00, 0x01, 0x02, 0x57, 0xb1];
        let s = sites(CLASS_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            0,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::MultianewArray,
            0x9023,
        )
        .expect("multianewarray should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9023),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_checkcast_via_stub() {
        // aload_0 ; checkcast #1 ; pop ; return
        let code: &[u8] = &[0x2a, 0xc0, 0x00, 0x01, 0x57, 0xb1];
        let s = sites(CLASS_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            1,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::CheckCast,
            0x9030,
        )
        .expect("checkcast should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9030),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_instanceof_via_stub() {
        // aload_0 ; instanceof #1 ; pop ; return
        let code: &[u8] = &[0x2a, 0xc1, 0x00, 0x01, 0x57, 0xb1];
        let s = sites(CLASS_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            1,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::InstanceOf,
            0x9031,
        )
        .expect("instanceof should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9031),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_invokevirtual_via_stub() {
        // aload_0 ; iconst_5 ; invokevirtual #1 ; return
        let code: &[u8] = &[0x2a, 0x08, 0xb6, 0x00, 0x01, 0xb1];
        let s = sites(CALL_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            1,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::UnresolvedCall,
            0x9040,
        )
        .expect("invokevirtual should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9040),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_invokestatic_via_stub() {
        // iconst_5 ; invokestatic #1 ; return
        let code: &[u8] = &[0x08, 0xb8, 0x00, 0x01, 0xb1];
        let s = sites(CALL_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let emitted = backend_with(
            code,
            0,
            &ctx,
            crate::share::c1::c1_LIR::RuntimeStub::UnresolvedCall,
            0x9041,
        )
        .expect("invokestatic should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9041),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_athrow_via_stub() {
        // aload_0 ; athrow
        let code: &[u8] = &[0x2a, 0xbf];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[(crate::share::c1::c1_LIR::RuntimeStub::Throw, 0x9050)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("athrow should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9050),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_monitors_via_stub() {
        // aload_0 ; monitorenter ; monitorexit ; return
        let code: &[u8] = &[0x2a, 0xc2, 0xc3, 0xb1];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[
                (crate::share::c1::c1_LIR::RuntimeStub::MonitorEnter, 0x9060),
                (crate::share::c1::c1_LIR::RuntimeStub::MonitorExit, 0x9061),
            ]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("monitors should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9060),
            "{:02x?}",
            emitted.code
        );
        assert!(
            has_stub_call(&emitted.code, 0x9061),
            "{:02x?}",
            emitted.code
        );
    }

    #[test]
    fn field_ops_require_stubs() {
        let code: &[u8] = &[0x2a, 0xb4, 0x00, 0x01, 0x57, 0xb1];
        let s = sites(FIELD_SITE);
        let ctx = BuilderContext::with_sites(&s);
        let mut c = Compile::compile(&method(code, 1), &ctx);
        assert!(
            c.backend(
                &method(code, 1),
                &ctx,
                &BackendOptions {
                    params: ref_param(),
                    ..BackendOptions::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn compiles_drem_via_stub() {
        // aload_0 ; iconst_0 ; daload ; aload_0 ; iconst_1 ; daload ; drem ; dreturn
        let code: &[u8] = &[0x2a, 0x03, 0x31, 0x2a, 0x04, 0x31, 0x73, 0xaf];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[
                (crate::share::c1::c1_LIR::RuntimeStub::ArrayLoad, 0x9100),
                (crate::share::c1::c1_LIR::RuntimeStub::FloatRem, 0x9101),
            ]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("drem should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9100),
            "array load stub missing: {:02x?}",
            emitted.code
        );
        assert!(
            has_stub_call(&emitted.code, 0x9101),
            "float rem stub missing: {:02x?}",
            emitted.code
        );
        assert!(emitted.code.iter().any(|&b| b == 0xc3));
    }

    #[test]
    fn compiles_frem_via_stub() {
        // aload_0 ; iconst_0 ; faload ; aload_0 ; iconst_1 ; faload ; frem ; freturn
        let code: &[u8] = &[0x2a, 0x03, 0x30, 0x2a, 0x04, 0x30, 0x72, 0xae];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[
                (crate::share::c1::c1_LIR::RuntimeStub::ArrayLoad, 0x9110),
                (crate::share::c1::c1_LIR::RuntimeStub::FloatRem, 0x9111),
            ]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("frem should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9111),
            "float rem stub missing: {:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_f2i_via_stub() {
        // aload_0 ; iconst_0 ; faload ; f2i ; ireturn
        let code: &[u8] = &[0x2a, 0x03, 0x30, 0x8b, 0xac];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[
                (crate::share::c1::c1_LIR::RuntimeStub::ArrayLoad, 0x9120),
                (crate::share::c1::c1_LIR::RuntimeStub::FloatConvert, 0x9121),
            ]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("f2i should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9121),
            "float convert stub missing: {:02x?}",
            emitted.code
        );
    }

    #[test]
    fn compiles_d2l_via_stub() {
        // aload_0 ; iconst_0 ; daload ; d2l ; lreturn
        let code: &[u8] = &[0x2a, 0x03, 0x31, 0x8f, 0xad];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[
                (crate::share::c1::c1_LIR::RuntimeStub::ArrayLoad, 0x9130),
                (crate::share::c1::c1_LIR::RuntimeStub::FloatConvert, 0x9131),
            ]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, 1), &BuilderContext::empty(), &options)
            .expect("d2l should emit");
        assert!(
            has_stub_call(&emitted.code, 0x9131),
            "float convert stub missing: {:02x?}",
            emitted.code
        );
    }

    #[test]
    fn float_rem_requires_stub() {
        // Same drem program without the FloatRem stub must be rejected.
        let code: &[u8] = &[0x2a, 0x03, 0x31, 0x2a, 0x04, 0x31, 0x73, 0xaf];
        let options = BackendOptions {
            params: ref_param(),
            stubs: stub_map(&[(crate::share::c1::c1_LIR::RuntimeStub::ArrayLoad, 0x9140)]),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, 1), &BuilderContext::empty());
        assert!(
            c.backend(&method(code, 1), &BuilderContext::empty(), &options)
                .is_err()
        );
    }
}
