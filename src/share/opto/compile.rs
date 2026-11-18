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

    // ---- end-to-end: emit C2 code, publish it and execute --------------
    //
    // These tests run the real pipeline (graph -> optimize -> schedule ->
    // regalloc -> x86-64 emission) and then execute the resulting bytes
    // through the same `try_invoke` trampoline C1 nmethods use, with the
    // real published runtime-stub table.

    use crate::share::interpreter::bytecodeInterpreter::Value;
    use crate::share::jitruntime::{CompiledResult, publish_executable, stub_map as jit_stub_map, try_invoke};
    use crate::share::opto::codegen::ParamKind;
    use crate::share::opto::codegen::ParamSlot;

    fn iparam(slot: usize) -> ParamSlot {
        ParamSlot {
            local_slot: slot,
            kind: ParamKind::I32,
        }
    }

    fn lparam(slot: usize) -> ParamSlot {
        ParamSlot {
            local_slot: slot,
            kind: ParamKind::I64,
        }
    }

    fn run_c2(
        code: &'static [u8],
        param_slots: usize,
        params: Vec<ParamSlot>,
        descriptor: &'static [u8],
        args: &[Value<'static>],
    ) -> Option<CompiledResult<'static>> {
        let options = BackendOptions {
            params,
            stubs: jit_stub_map(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, param_slots), &BuilderContext::empty());
        let emitted = c
            .backend(&method(code, param_slots), &BuilderContext::empty(), &options)
            .expect("c2 backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        try_invoke(addr, descriptor, true, args, 0)
    }

    #[test]
    fn e2e_iconst_ireturn() {
        // iconst_5 ; ireturn -> ()I
        let result = run_c2(&[0x08, 0xac], 0, vec![], b"()I", &[]).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 5),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_iadd_params() {
        // iload_0 ; iload_1 ; iadd ; ireturn -> (II)I
        let result = run_c2(
            &[0x1a, 0x1b, 0x60, 0xac],
            2,
            vec![iparam(0), iparam(1)],
            b"(II)I",
            &[Value::Int(20), Value::Int(22)],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 42),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_ladd_params() {
        // lload_0 ; lload_2 ; ladd ; lreturn -> (JJ)J
        let result = run_c2(
            &[0x1e, 0x20, 0x61, 0xad],
            4,
            vec![lparam(0), lparam(2)],
            b"(JJ)J",
            &[Value::Long(1_000_000_007), Value::Long(2_000_000_003)],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Long(v))) => {
                assert_eq!(v, 3_000_000_010)
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_branch_selects() {
        // iload_0 ; ifeq ->iconst_2 ; iconst_1 ; ireturn ; iconst_2 ; ireturn
        // -> (I)I  returns 1 when arg != 0, 2 when arg == 0.
        // Branch offsets are relative to the opcode bci: ifeq sits at
        // bci 1 and iconst_2 at bci 6, so the offset is +5.
        let code: &[u8] = &[
            0x1a, // 0: iload_0
            0x99, 0x00, 0x05, // 1: ifeq -> bci 6
            0x04, // 4: iconst_1
            0xac, // 5: ireturn
            0x05, // 6: iconst_2
            0xac, // 7: ireturn
        ];
        let r1 = run_c2(code, 1, vec![iparam(0)], b"(I)I", &[Value::Int(7)]).unwrap();
        match r1 {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 1),
            other => panic!("unexpected result: {:?}", other),
        }
        let r0 = run_c2(code, 1, vec![iparam(0)], b"(I)I", &[Value::Int(0)]).unwrap();
        match r0 {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 2),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_idiv_via_real_stub() {
        // iload_0 ; iload_1 ; idiv ; ireturn -> (II)I  84/2 = 42
        let result = run_c2(
            &[0x1a, 0x1b, 0x6c, 0xac],
            2,
            vec![iparam(0), iparam(1)],
            b"(II)I",
            &[Value::Int(84), Value::Int(2)],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 42),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_float_return_channel() {
        // dconst_1 ; dreturn -> ()D   exercises the xmm0 result path
        let result = run_c2(&[0x0f, 0xaf], 0, vec![], b"()D", &[]).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Double(v))) => assert_eq!(v, 1.0),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_frem_via_real_stub() {
        // fconst_1 ; fconst_2 ; frem ; freturn -> ()F  1.0 % 0.5 = 0.0? no:
        // fconst_1=1.0, fconst_2=2.0 -> frem = 1.0
        let result = run_c2(&[0x0c, 0x0d, 0x72, 0xae], 0, vec![], b"()F", &[]).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Float(v))) => assert_eq!(v, 1.0),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_drem_via_real_stub() {
        // dconst_1 ; dconst_0 ; drem ; dreturn -> ()D  1.0 % 0.0 = NaN
        // (x86 has no remainder instruction; the stub's fmod path must
        // produce the Java NaN result, not an SSE artifact)
        let result = run_c2(&[0x0f, 0x0e, 0x73, 0xad], 0, vec![], b"()D", &[]).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Double(v))) => assert!(v.is_nan()),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_f2i_via_real_stub() {
        // fconst_1 ; f2i ; ireturn -> ()I  trunc(1.0) = 1
        let result = run_c2(&[0x0c, 0x8b, 0xac], 0, vec![], b"()I", &[]).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 1),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_d2l_via_real_stub() {
        // dconst_1 ; d2l ; lreturn -> ()J  1L
        let result = run_c2(&[0x0f, 0x8f, 0xad], 0, vec![], b"()J", &[]).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Long(v))) => assert_eq!(v, 1),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_loop_sum() {
        // static int sum(int n): sum = 0; i = 0;
        // while (i < n) { sum += i; i++; } return sum;
        // Exercises loop phis, the backward edge and the safepoint poll
        // emitted at the loop header.
        //
        //  0: iconst_0
        //  1: istore_1          sum = 0
        //  2: iconst_0
        //  3: istore_2          i = 0
        //  4: iload_0           <- loop header
        //  5: iload_2
        //  6: if_icmple +13 -> 19   exit when n <= i
        //  9: iload_1
        // 10: iload_2
        // 11: iadd
        // 12: istore_1          sum += i
        // 13: iinc 2, 1         i++
        // 16: goto -12 -> 4
        // 19: iload_1
        // 20: ireturn
        let code: &[u8] = &[
            0x03, 0x3c, 0x03, 0x3d, 0x1a, 0x1c, 0xa4, 0x00, 0x0d, 0x1b, 0x1c, 0x60, 0x3c, 0x84,
            0x02, 0x01, 0xa7, 0xff, 0xf4, 0x1b, 0xac,
        ];
        let mut m = method(code, 1);
        m.max_locals = 3;
        let options = BackendOptions {
            params: vec![iparam(0)],
            stubs: jit_stub_map(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &BuilderContext::empty());
        let emitted = c
            .backend(&m, &BuilderContext::empty(), &options)
            .expect("c2 backend should emit loop");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        let result = try_invoke(addr, b"(I)I", true, &[Value::Int(5)], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 10),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_loop_if_sum() {
        // static int f(int n) {
        //   int sum = 0;
        //   for (int i = 0; i < n; i++)
        //     sum += (i & 1) == 0 ? i : 1;
        //   return sum;
        // }
        // The if/else inside the loop merges `sum` on both edges, so the
        // join block needs phi copies arriving through IfTrue/IfFalse
        // control edges, not just unconditional jumps.
        //
        //  0: iconst_0
        //  1: istore_1          sum = 0
        //  2: iconst_0
        //  3: istore_2          i = 0
        //  4: iload_0           <- loop header
        //  5: iload_2
        //  6: if_icmple +26 -> 32   exit when n <= i
        //  9: iload_2
        // 10: iconst_1
        // 11: iand
        // 12: ifeq +10 -> 22       even -> then
        // 15: iload_1              else: sum += 1
        // 16: iconst_1
        // 17: iadd
        // 18: istore_1
        // 19: goto +7 -> 26        -> join
        // 22: iload_1              then: sum += i
        // 23: iload_2
        // 24: iadd
        // 25: istore_1
        // 26: iinc 2, 1            <- join
        // 29: goto -25 -> 4
        // 32: iload_1
        // 33: ireturn
        let code: &[u8] = &[
            0x03, 0x3c, 0x03, 0x3d, 0x1a, 0x1c, 0xa4, 0x00, 0x1a, 0x1c, 0x04, 0x7e, 0x99, 0x00,
            0x0a, 0x1b, 0x04, 0x60, 0x3c, 0xa7, 0x00, 0x07, 0x1b, 0x1c, 0x60, 0x3c, 0x84, 0x02,
            0x01, 0xa7, 0xff, 0xe7, 0x1b, 0xac,
        ];
        let mut m = method(code, 1);
        m.max_locals = 3;
        let options = BackendOptions {
            params: vec![iparam(0)],
            stubs: jit_stub_map(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &BuilderContext::empty());
        let emitted = c
            .backend(&m, &BuilderContext::empty(), &options)
            .expect("c2 backend should emit loop");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        let result = try_invoke(addr, b"(I)I", true, &[Value::Int(5)], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 8),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_loop_break() {
        // static int f(int n) {
        //   int sum = 0;
        //   for (int i = 0; i < n; i++) {
        //     if (i == 3) break;
        //     sum += i;
        //   }
        //   return sum;
        // }
        // The break is an early exit from the loop body to the same return
        // block the loop condition uses — two different control edges into
        // one merge.
        //
        //  0: iconst_0
        //  1: istore_1          sum = 0
        //  2: iconst_0
        //  3: istore_2          i = 0
        //  4: iload_0           <- loop header
        //  5: iload_2
        //  6: if_icmple +18 -> 24   exit when n <= i
        //  9: iload_2
        // 10: iconst_3
        // 11: if_icmpeq +13 -> 24   break when i == 3
        // 14: iload_1
        // 15: iload_2
        // 16: iadd
        // 17: istore_1          sum += i
        // 18: iinc 2, 1         i++
        // 21: goto -17 -> 4
        // 24: iload_1
        // 25: ireturn
        let code: &[u8] = &[
            0x03, 0x3c, 0x03, 0x3d, 0x1a, 0x1c, 0xa4, 0x00, 0x12, 0x1c, 0x06, 0x9f, 0x00, 0x0d,
            0x1b, 0x1c, 0x60, 0x3c, 0x84, 0x02, 0x01, 0xa7, 0xff, 0xef, 0x1b, 0xac,
        ];
        let mut m = method(code, 1);
        m.max_locals = 3;
        let options = BackendOptions {
            params: vec![iparam(0)],
            stubs: jit_stub_map(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &BuilderContext::empty());
        let emitted = c
            .backend(&m, &BuilderContext::empty(), &options)
            .expect("c2 backend should emit loop");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        let result = try_invoke(addr, b"(I)I", true, &[Value::Int(10)], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 3),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_wide_locals() {
        // static int f(int p) {
        //   int x = 7;
        //   p += 5;          // wide iinc on local 0
        //   return x + p;    // wide iload on locals 0 and 1
        // }
        // Exercises the wide-prefixed encodings: u2 local index and an
        // s2 iinc constant (the operands sit right after the sub-opcode,
        // not after a phantom wide slot).
        //
        //  0: bipush 7
        //  2: wide istore 1        x = 7
        //  6: wide iinc 0, 5       p += 5
        // 12: wide iload 1
        // 16: wide iload 0
        // 20: iadd
        // 21: ireturn
        let code: &[u8] = &[
            0x10, 0x07, // bipush 7
            0xc4, 0x36, 0x00, 0x01, // wide istore 1
            0xc4, 0x84, 0x00, 0x00, 0x00, 0x05, // wide iinc 0, 5
            0xc4, 0x15, 0x00, 0x01, // wide iload 1
            0xc4, 0x15, 0x00, 0x00, // wide iload 0
            0x60, 0xac, // iadd ; ireturn
        ];
        let mut m = method(code, 1);
        m.max_locals = 2;
        let options = BackendOptions {
            params: vec![iparam(0)],
            stubs: jit_stub_map(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &BuilderContext::empty());
        let emitted = c
            .backend(&m, &BuilderContext::empty(), &options)
            .expect("c2 backend should emit wide locals");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        let result = try_invoke(addr, b"(I)I", true, &[Value::Int(10)], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 22),
            other => panic!("unexpected result: {:?}", other),
        }
    }
}
