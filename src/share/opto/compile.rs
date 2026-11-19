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

    /// OSR compilation: graph entry is `osr.entry_bci` seeded from
    /// marshal-buffer inputs instead of method parameters.
    pub fn compile_osr(
        method: &MethodCode,
        ctx: &BuilderContext,
        osr: &super::builder::OsrSpec,
    ) -> Self {
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
        let mut phase = Phase::new(PhaseNumber::Parser);
        let res = BytecodeGraphBuilder::build_osr(
            &mut c.gvn,
            method,
            ctx,
            c.start,
            c.root,
            osr,
        );
        phase.complete();
        c.phases_run.push(phase);
        c.build = Some(res);
        c
    }

    pub fn build_graph(&mut self, method: &MethodCode, ctx: &BuilderContext) {
        let mut phase = Phase::new(PhaseNumber::Parser);
        let res = BytecodeGraphBuilder::build(&mut self.gvn, method, ctx, self.start, self.root);
        phase.complete();
        self.phases_run.push(phase);
        self.build = Some(res);
    }

    fn dbg_dump(&self, tag: &str) {
        if std::env::var("JVM25_DBG_PHASES").is_ok() {
            eprintln!("=== {} ===", tag);
            for n in &self.gvn.nodes {
                eprintln!("N{} {:?} in={:?}", n.id, n.op, n.inputs);
            }
        }
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
        self.dbg_dump("itergvn1");
        {
            let mut phase = Phase::with_name(PhaseNumber::GVN, "typer");
            let typer = PhaseTyperRun::optimize(&self.gvn);
            self.node_types = typer.types;
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("typer");
        {
            let mut phase = Phase::with_name(PhaseNumber::Optimistic, "canonicalizer");
            PhaseCanonicalizerRun::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("canonicalizer");
        {
            let mut phase = Phase::with_name(PhaseNumber::MacroExpand, "macroExpand");
            let mut me = PhaseMacroExpandRun::new();
            me.optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("macroexpand");
        {
            let mut phase = Phase::new(PhaseNumber::IdealLoop);
            self.loops = Some(PhaseIdealLoopRun::optimize(&self.gvn));
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("idealloop");
        {
            let mut phase = Phase::new(PhaseNumber::GVN);
            PhaseIterGVN::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("itergvn2");
        {
            let mut phase = Phase::new(PhaseNumber::RemoveUselessAndRenumberLive);
            PhaseRemoveUselessAndRenumber::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("dce1");
        {
            let mut phase = Phase::new(PhaseNumber::Optimistic);
            PhaseCCP::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("ccp");
        {
            let mut phase = Phase::new(PhaseNumber::RemoveUseless);
            PhaseRemoveUselessAndRenumber::optimize(&mut self.gvn);
            phase.complete();
            self.phases_run.push(phase);
        }
        self.dbg_dump("dce2");
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
    use crate::share::jitruntime::{
        CompiledResult, publish_executable, stub_map as jit_stub_map,
        stub_map_no_unwind as jit_stub_map_no_unwind, try_invoke,
    };
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

    fn fparam(slot: usize) -> ParamSlot {
        ParamSlot {
            local_slot: slot,
            kind: ParamKind::F32,
        }
    }

    fn dparam(slot: usize) -> ParamSlot {
        ParamSlot {
            local_slot: slot,
            kind: ParamKind::F64,
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
            stubs_no_unwind: jit_stub_map_no_unwind(),
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

    #[test]
    fn e2e_int_bitops() {
        // (p << 2) | (p >>> 1) -> (I)I
        // ishl, iushr and ior in one expression; iushr on a negative
        // operand exercises the logical (zero-fill) shift path.
        let result = run_c2(
            &[0x1a, 0x05, 0x78, 0x1a, 0x04, 0x7c, 0x80, 0xac],
            1,
            vec![iparam(0)],
            b"(I)I",
            &[Value::Int(-8)],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, -4),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_long_bitops() {
        // (a * b) | (a ^ b) -> (JJ)J
        let a: i64 = 0x1234_5678_9abc_def0;
        let b: i64 = -0x0fed_cba9_8765_4321;
        let result = run_c2(
            &[0x1e, 0x20, 0x69, 0x1e, 0x20, 0x83, 0x81, 0xad],
            4,
            vec![lparam(0), lparam(2)],
            b"(JJ)J",
            &[Value::Long(a), Value::Long(b)],
        )
        .unwrap();
        let expect = a.wrapping_mul(b) | (a ^ b);
        match result {
            CompiledResult::Value(Some(Value::Long(v))) => assert_eq!(v, expect),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_lcmp() {
        // a <=> b -> (JJ)I  lcmp pushes -1/0/1
        let result = run_c2(
            &[0x1e, 0x20, 0x94, 0xac],
            4,
            vec![lparam(0), lparam(2)],
            b"(JJ)I",
            &[Value::Long(5), Value::Long(9)],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, -1),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_i2d_and_i2c() {
        // iload_0 ; i2d ; dreturn -> (I)D
        let result = run_c2(&[0x1a, 0x87, 0xaf], 1, vec![iparam(0)], b"(I)D", &[Value::Int(-3)])
            .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Double(v))) => assert_eq!(v, -3.0),
            other => panic!("unexpected result: {:?}", other),
        }
        // iload_0 ; i2c ; ireturn -> (I)I  zero-extends through char
        let result = run_c2(&[0x1a, 0x92, 0xac], 1, vec![iparam(0)], b"(I)I", &[Value::Int(-1)])
            .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 65535),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_float_arith() {
        // fload_0 ; fload_0 ; fconst_2 ; fmul ; fadd ; freturn -> (F)F
        // p + p * 2.0 = 3p
        let result = run_c2(
            &[0x22, 0x22, 0x0d, 0x6a, 0x62, 0xae],
            1,
            vec![fparam(0)],
            b"(F)F",
            &[Value::Float(1.5)],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Float(v))) => assert_eq!(v, 4.5),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_double_arith() {
        // dload_0 ; dload_0 ; dadd ; dload_0 ; ddiv ; dreturn -> (D)D
        // (p + p) / p = 2.0
        let result = run_c2(
            &[0x26, 0x26, 0x63, 0x26, 0x6f, 0xaf],
            2,
            vec![dparam(0)],
            b"(D)D",
            &[Value::Double(3.5)],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Double(v))) => assert_eq!(v, 2.0),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    // -----------------------------------------------------------------
    // Heap-operation e2e tests: real ClassLoader + registered klasses,
    // so field/array/cast stubs resolve against live objects exactly
    // like the C1 nmethod tests in jitruntime::runtime.
    // -----------------------------------------------------------------

    use crate::share::classfile::classFileParser::{ClassFile, Member};
    use crate::share::classfile::classLoader::ClassLoader;
    use crate::share::oops::instanceKlass::InstanceKlass;
    use crate::share::oops::instanceOop::InstanceOop;
    use crate::share::runtime::syncBump::SyncBump;
    use crate::share::runtime::threading;
    use std::sync::{Arc, OnceLock};

    fn test_loader() -> Arc<ClassLoader<'static>> {
        static LOADER: OnceLock<Arc<ClassLoader<'static>>> = OnceLock::new();
        LOADER
            .get_or_init(|| {
                let arena: &'static SyncBump = Box::leak(Box::new(SyncBump::new()));
                let loader =
                    Arc::new(ClassLoader::open(std::path::Path::new("."), arena).unwrap());
                threading::install_loader(loader.clone());
                loader
            })
            .clone()
    }

    fn register_klass(
        loader: &ClassLoader<'static>,
        name: &'static [u8],
        fields: &[(&'static [u8], &'static [u8], u16)],
    ) -> Arc<InstanceKlass<'static>> {
        let class = ClassFile {
            majorVersion: 65,
            minorVersion: 0,
            accessFlags: 0x0001,
            name,
            superName: Some(b"java/lang/Object"),
            interfaces: Vec::new(),
            constants: Vec::new(),
            fields: fields
                .iter()
                .map(|&(n, d, f)| Member {
                    accessFlags: f,
                    name: n,
                    descriptor: d,
                    attributes: Vec::new(),
                })
                .collect(),
            methods: Vec::new(),
            attributes: Vec::new(),
        };
        let klass = Arc::new(InstanceKlass::new(class, None, Vec::new()).unwrap());
        loader.classes.borrow_mut().insert(name, klass.clone());
        klass
    }

    fn run_c2_ctx(
        code: &'static [u8],
        param_slots: usize,
        params: Vec<ParamSlot>,
        descriptor: &'static [u8],
        args: &[Value<'static>],
        ctx: &BuilderContext,
    ) -> Option<CompiledResult<'static>> {
        let options = BackendOptions {
            params,
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&method(code, param_slots), ctx);
        let emitted = c
            .backend(&method(code, param_slots), ctx, &options)
            .expect("c2 backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        try_invoke(addr, descriptor, true, args, 0)
    }

    #[test]
    fn e2e_getfield_putfield() {
        // aload_0 ; bipush 42 ; putfield #1 ; aload_0 ; getfield #1 ; ireturn
        // -> (LT;)I  stores then reloads field x through real stubs.
        let loader = test_loader();
        let klass = register_klass(&loader, b"T", &[(b"x", b"I", 0)]);
        let object = InstanceOop::new(klass).unwrap();
        let handle = loader.alloc_object(object);

        let code: &[u8] = &[
            0x2a, // 0: aload_0
            0x10, 0x2a, // 1: bipush 42
            0xb5, 0x00, 0x01, // 3: putfield #1
            0x2a, // 6: aload_0
            0xb4, 0x00, 0x01, // 7: getfield #1
            0xac, // 10: ireturn
        ];
        let s = sites(&[(1, "T", "x", "I")]);
        let ctx = BuilderContext::with_sites(&s);
        let result = run_c2_ctx(
            code,
            1,
            ref_param(),
            b"(LT;)I",
            &[Value::Object(handle)],
            &ctx,
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 42),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_newarray_iastore_iaload() {
        // The array stubs resolve through the JVM loader; make sure it
        // is installed before the compiled code runs (tests are
        // parallel, so do not rely on another test having done it).
        let _loader = test_loader();
        // bipush 3 ; newarray int ; astore_0 ;
        // aload_0 ; iconst_0 ; bipush 42 ; iastore ;
        // aload_0 ; iconst_0 ; iaload ; ireturn  -> ()I
        let code: &[u8] = &[
            0x10, 0x03, // 0: bipush 3
            0xbc, 0x0a, // 2: newarray int
            0x4b, // 4: astore_0
            0x2a, // 5: aload_0
            0x03, // 6: iconst_0
            0x10, 0x2a, // 7: bipush 42
            0x4f, // 9: iastore
            0x2a, // 10: aload_0
            0x03, // 11: iconst_0
            0x2e, // 12: iaload
            0xac, // 13: ireturn
        ];
        let result = run_c2(
            code,
            0,
            vec![],
            b"()I",
            &[],
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 42),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_arraylength() {
        let _loader = test_loader();
        // bipush 5 ; newarray int ; astore_0 ; aload_0 ; arraylength ; ireturn
        let code: &[u8] = &[
            0x10, 0x05, // 0: bipush 5
            0xbc, 0x0a, // 2: newarray int
            0x4b, // 4: astore_0
            0x2a, // 5: aload_0
            0xbe, // 6: arraylength
            0xac, // 7: ireturn
        ];
        let result = run_c2(code, 0, vec![], b"()I", &[]).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 5),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_instanceof_checkcast() {
        // aload_0 ; instanceof #1 ; ireturn -> (LT;)I  (expects 1)
        // aload_0 ; checkcast #1 ; pop ; iconst_1 ; ireturn -> (LT;)I
        let loader = test_loader();
        let klass = register_klass(&loader, b"T", &[]);
        let object = InstanceOop::new(klass).unwrap();
        let handle = loader.alloc_object(object);

        let s = sites(&[(1, "T", "", "")]);
        let ctx = BuilderContext::with_sites(&s);
        let result = run_c2_ctx(
            &[0x2a, 0xc1, 0x00, 0x01, 0xac],
            1,
            ref_param(),
            b"(LT;)I",
            &[Value::Object(handle)],
            &ctx,
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 1),
            other => panic!("unexpected instanceof result: {:?}", other),
        }

        let result = run_c2_ctx(
            &[0x2a, 0xc0, 0x00, 0x01, 0x57, 0x04, 0xac],
            1,
            ref_param(),
            b"(LT;)I",
            &[Value::Object(handle)],
            &ctx,
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 1),
            other => panic!("unexpected checkcast result: {:?}", other),
        }
    }

    #[test]
    fn e2e_getstatic_putstatic() {
        // iconst_5 ; putstatic #1 ; getstatic #1 ; ireturn -> ()I
        let loader = test_loader();
        register_klass(&loader, b"S", &[(b"v", b"I", 0x0008)]);
        let code: &[u8] = &[
            0x08, // 0: iconst_5
            0xb3, 0x00, 0x01, // 1: putstatic #1
            0xb2, 0x00, 0x01, // 4: getstatic #1
            0xac, // 7: ireturn
        ];
        let s = sites(&[(1, "S", "v", "I")]);
        let ctx = BuilderContext::with_sites(&s);
        let result = run_c2_ctx(code, 0, vec![], b"()I", &[], &ctx).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 5),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_athrow() {
        // aload_0 ; athrow -> (LT;)V  propagates the throwable handle.
        let loader = test_loader();
        let klass = register_klass(&loader, b"java/lang/Throwable", &[]);
        let object = InstanceOop::new(klass).unwrap();
        let handle = loader.alloc_object(object);
        let result = run_c2(
            &[0x2a, 0xbf],
            1,
            ref_param(),
            b"(Ljava/lang/Object;)V",
            &[Value::Object(handle)],
        )
        .unwrap();
        assert!(matches!(result, CompiledResult::Thrown(h) if h == handle));
    }

    /// Registers a klass whose `methods` carry a real `Code` attribute
    /// so `h_call_method` can run the callee interpreted.
    fn register_method_klass(
        loader: &ClassLoader<'static>,
        name: &'static [u8],
        method_name: &'static [u8],
        descriptor: &'static [u8],
        access_flags: u16,
        bytecode: &'static [u8],
        max_locals: u16,
    ) {
        use crate::share::classfile::classFileParser::Attribute;
        let mut code_attr = Vec::with_capacity(8 + bytecode.len() + 2);
        code_attr.extend_from_slice(&[0, 4]); // max_stack = 4
        code_attr.extend_from_slice(&max_locals.to_be_bytes());
        code_attr.extend_from_slice(&(bytecode.len() as u32).to_be_bytes());
        code_attr.extend_from_slice(bytecode);
        code_attr.extend_from_slice(&[0, 0]); // exception_table_length = 0
        let code_attr: &'static [u8] = Box::leak(code_attr.into_boxed_slice());
        let class = ClassFile {
            majorVersion: 65,
            minorVersion: 0,
            accessFlags: 0x0001,
            name,
            superName: Some(b"java/lang/Object"),
            interfaces: Vec::new(),
            constants: Vec::new(),
            fields: Vec::new(),
            methods: vec![Member {
                accessFlags: access_flags,
                name: method_name,
                descriptor,
                attributes: vec![Attribute {
                    name: b"Code",
                    bytes: code_attr,
                }],
            }],
            attributes: Vec::new(),
        };
        let klass = Arc::new(InstanceKlass::new(class, None, Vec::new()).unwrap());
        loader.classes.borrow_mut().insert(name, klass);
    }

    #[test]
    fn e2e_invokestatic() {
        // Caller: iload_0 ; iload_1 ; invokestatic #1 ; ireturn -> (II)I
        // Callee C.add(II)I runs interpreted: iload_0; iload_1; iadd; ireturn
        let loader = test_loader();
        register_method_klass(
            &loader,
            b"C",
            b"add",
            b"(II)I",
            0x0009,
            Box::leak(vec![0x1a, 0x1b, 0x60, 0xac].into_boxed_slice()),
            2,
        );
        let s = sites(&[(1, "C", "add", "(II)I")]);
        let ctx = BuilderContext::with_sites(&s);
        let result = run_c2_ctx(
            &[0x1a, 0x1b, 0xb8, 0x00, 0x01, 0xac],
            2,
            vec![iparam(0), iparam(1)],
            b"(II)I",
            &[Value::Int(19), Value::Int(23)],
            &ctx,
        )
        .unwrap();
        match result {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 42),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_tableswitch() {
        // iload_0 ; tableswitch 1..3 -> iconst_N ; ireturn ; default iconst_0
        // bci 1: opcode, bci 2-3 pad, default at bci 4 (aligned).
        // Offsets are relative to the tableswitch opcode bci (1).
        let code: &[u8] = &[
            0x1a, // 0: iload_0
            0xaa, 0x00, 0x00, // 1: tableswitch + 2 pad bytes
            0x00, 0x00, 0x00, 0x1b, // 4: default -> bci 28
            0x00, 0x00, 0x00, 0x01, // 8: low = 1
            0x00, 0x00, 0x00, 0x03, // 12: high = 3
            0x00, 0x00, 0x00, 0x1d, // 16: case 1 -> bci 30
            0x00, 0x00, 0x00, 0x1f, // 20: case 2 -> bci 32
            0x00, 0x00, 0x00, 0x21, // 24: case 3 -> bci 34
            0x03, 0xac, // 28: iconst_0 ; ireturn (default)
            0x04, 0xac, // 30: iconst_1 ; ireturn
            0x05, 0xac, // 32: iconst_2 ; ireturn
            0x06, 0xac, // 34: iconst_3 ; ireturn
        ];
        for (arg, want) in [(0, 0), (1, 1), (2, 2), (3, 3), (7, 0)] {
            let result = run_c2(code, 1, vec![iparam(0)], b"(I)I", &[Value::Int(arg)])
                .unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => {
                    assert_eq!(v, want, "arg={arg}")
                }
                other => panic!("unexpected result for {arg}: {:?}", other),
            }
        }
    }

    #[test]
    fn e2e_lookupswitch() {
        // iload_0 ; lookupswitch {10->1, 20->2} default->0
        let code: &[u8] = &[
            0x1a, // 0: iload_0
            0xab, 0x00, 0x00, // 1: lookupswitch + 2 pad bytes
            0x00, 0x00, 0x00, 0x1b, // 4: default -> bci 28
            0x00, 0x00, 0x00, 0x02, // 8: npairs = 2
            0x00, 0x00, 0x00, 0x0a, // 12: match 10
            0x00, 0x00, 0x00, 0x1d, // 16: -> bci 30
            0x00, 0x00, 0x00, 0x14, // 20: match 20
            0x00, 0x00, 0x00, 0x1f, // 24: -> bci 32
            0x03, 0xac, // 28: iconst_0 ; ireturn (default)
            0x04, 0xac, // 30: iconst_1 ; ireturn
            0x05, 0xac, // 32: iconst_2 ; ireturn
        ];
        for (arg, want) in [(0, 0), (10, 1), (20, 2), (30, 0)] {
            let result = run_c2(code, 1, vec![iparam(0)], b"(I)I", &[Value::Int(arg)])
                .unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => {
                    assert_eq!(v, want, "arg={arg}")
                }
                other => panic!("unexpected result for {arg}: {:?}", other),
            }
        }
    }

    #[test]
    fn e2e_try_catch_array_oob() {
        // try { a[i] } catch-all -> 7 ; in-bounds -> 9
        //
        //   0: bipush 3 ; 2: newarray int ; 4: astore_1
        //   5: aload_1 ; 6: iload_0 ; 7: iaload
        //   8: pop ; 9: bipush 9 ; 11: ireturn
        //  12: pop            <- handler_pc (catch_type = 0)
        //  13: bipush 7 ; 15: ireturn
        //
        // Exception table covers [5,8): the aload/iload cannot trap but
        // iaload at bci 7 can (ArrayIndexOutOfBounds).
        use super::super::builder::ExceptionHandler;
        // JVM_LOADER is a global OnceLock: whichever test installed first
        // wins, so register the throwable klass on the *active* loader.
        let _ = test_loader();
        let loader = crate::share::runtime::threading::jvm_loader().unwrap();
        register_klass(&loader, b"java/lang/ArrayIndexOutOfBoundsException", &[]);
        let code: &[u8] = &[
            0x10, 0x03, // 0: bipush 3
            0xbc, 0x0a, // 2: newarray int
            0x4c, // 4: astore_1
            0x2b, // 5: aload_1
            0x1a, // 6: iload_0
            0x2e, // 7: iaload
            0x57, // 8: pop
            0x10, 0x09, // 9: bipush 9
            0xac, // 11: ireturn
            0x57, // 12: pop (handler)
            0x10, 0x07, // 13: bipush 7
            0xac, // 15: ireturn
        ];
        let handlers = [ExceptionHandler {
            start_pc: 5,
            end_pc: 8,
            handler_pc: 12,
            catch_type: 0,
        }];
        let m = MethodCode {
            name: "test",
            max_locals: 2,
            max_stack: 8,
            param_slots: 2,
            code,
            handlers: &handlers,
        };
        let options = BackendOptions {
            params: vec![iparam(0)],
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &BuilderContext::empty());
        c.optimize();
        let emitted = c
            .backend(&m, &BuilderContext::empty(), &options)
            .expect("c2 backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        for (arg, want) in [(1, 9), (9, 7)] {
            let result = try_invoke(addr, b"(I)I", true, &[Value::Int(arg)], 0).unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => {
                    assert_eq!(v, want, "arg={arg}")
                }
                other => panic!("unexpected result for {arg}: {:?}", other),
            }
        }
    }

    #[test]
    fn e2e_try_catch_div_by_zero() {
        // try { x / y } catch-all -> 7 ; nonzero divisor -> quotient
        //
        //   0: iload_0 ; 1: iload_1 ; 2: idiv ; 3: ireturn
        //   4: pop          <- handler_pc (catch_type = 0)
        //   5: bipush 7 ; 7: ireturn
        //
        // idiv at bci 2 raises ArithmeticException when y == 0; the
        // exception edge must dispatch in-method instead of unwinding.
        use super::super::builder::ExceptionHandler;
        let _ = test_loader();
        let loader = crate::share::runtime::threading::jvm_loader().unwrap();
        register_klass(&loader, b"java/lang/ArithmeticException", &[]);
        let code: &[u8] = &[
            0x1a, // 0: iload_0
            0x1b, // 1: iload_1
            0x6c, // 2: idiv
            0xac, // 3: ireturn
            0x57, // 4: pop (handler)
            0x10, 0x07, // 5: bipush 7
            0xac, // 7: ireturn
        ];
        let handlers = [ExceptionHandler {
            start_pc: 0,
            end_pc: 3,
            handler_pc: 4,
            catch_type: 0,
        }];
        let m = MethodCode {
            name: "test",
            max_locals: 2,
            max_stack: 8,
            param_slots: 2,
            code,
            handlers: &handlers,
        };
        let options = BackendOptions {
            params: vec![iparam(0), iparam(1)],
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &BuilderContext::empty());
        c.optimize();
        let emitted = c
            .backend(&m, &BuilderContext::empty(), &options)
            .expect("c2 backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        for (x, y, want) in [(20, 4, 5), (20, 0, 7), (i32::MIN, -1, i32::MIN)] {
            let result = try_invoke(
                addr,
                b"(II)I",
                true,
                &[Value::Int(x), Value::Int(y)],
                0,
            )
            .unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => {
                    assert_eq!(v, want, "x={x} y={y}")
                }
                other => panic!("unexpected result for {x}/{y}: {:?}", other),
            }
        }
    }
    #[test]
    fn e2e_typed_catch() {
        // Typed exception handlers: the dispatch chain must run
        // InstanceOf against each covering catch type in exception-table
        // order — match -> handler, no match -> next handler / rethrow.
        //
        //   0: iload_0 ; 1: iload_1 ; 2: idiv ; 3: ireturn
        //   4: pop          <- handler_pc
        //   5: bipush 7 ; 7: ireturn
        use super::super::builder::{ExceptionHandler, SiteInfo};
        use std::collections::HashMap;
        let _ = test_loader();
        let loader = crate::share::runtime::threading::jvm_loader().unwrap();
        register_klass(&loader, b"java/lang/ArithmeticException", &[]);
        register_klass(&loader, b"java/lang/NullPointerException", &[]);
        let code: &[u8] = &[
            0x1a, // 0: iload_0
            0x1b, // 1: iload_1
            0x6c, // 2: idiv
            0xac, // 3: ireturn
            0x57, // 4: pop (handler)
            0x10, 0x07, // 5: bipush 7
            0xac, // 7: ireturn
        ];
        let site = |class: &str| SiteInfo {
            class: class.to_string(),
            name: String::new(),
            desc: String::new(),
        };
        let mut sites: HashMap<u16, SiteInfo> = HashMap::new();
        sites.insert(100, site("java/lang/ArithmeticException"));
        sites.insert(101, site("java/lang/NullPointerException"));
        let fields = HashMap::new();
        let methods = HashMap::new();
        let constants = HashMap::new();
        let ctx = super::super::builder::BuilderContext {
            fields: &fields,
            methods: &methods,
            constants: &constants,
            sites: &sites,
        };
        let options = BackendOptions {
            params: vec![iparam(0), iparam(1)],
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            ..BackendOptions::default()
        };
        let compile = |handlers: &[ExceptionHandler]| {
            let m = MethodCode {
                name: "test",
                max_locals: 2,
                max_stack: 8,
                param_slots: 2,
                code,
                handlers,
            };
            let mut c = Compile::compile(&m, &ctx);
            c.optimize();
            let emitted = c.backend(&m, &ctx, &options).expect("emit");
            publish_executable(&emitted.code, emitted.code.len()).expect("publish")
        };
        let invoke = |addr: usize| {
            try_invoke(addr, b"(II)I", true, &[Value::Int(20), Value::Int(0)], 0).unwrap()
        };
        // 1. catch(ArithmeticException): thrown AE matches -> 7.
        let h = [ExceptionHandler { start_pc: 0, end_pc: 3, handler_pc: 4, catch_type: 100 }];
        let addr = compile(&h);
        match invoke(addr) {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 7),
            other => panic!("matching typed catch: {:?}", other),
        }
        // 2. catch(NullPointerException) only: AE does not match -> rethrow.
        let h = [ExceptionHandler { start_pc: 0, end_pc: 3, handler_pc: 4, catch_type: 101 }];
        let addr = compile(&h);
        match invoke(addr) {
            CompiledResult::Thrown(_) => {}
            other => panic!("non-matching typed catch should rethrow: {:?}", other),
        }
        // 3. catch(NPE) then catch-all: typed fails, catch-all catches -> 7.
        let h = [
            ExceptionHandler { start_pc: 0, end_pc: 3, handler_pc: 4, catch_type: 101 },
            ExceptionHandler { start_pc: 0, end_pc: 3, handler_pc: 4, catch_type: 0 },
        ];
        let addr = compile(&h);
        match invoke(addr) {
            CompiledResult::Value(Some(Value::Int(v))) => assert_eq!(v, 7),
            other => panic!("typed-then-catchall: {:?}", other),
        }
    }

    #[test]
    fn e2e_try_catch_shared_handler_phi() {
        // Two adjacent protected ranges feed ONE catch-all handler, so the
        // handler entry is a Region merge of two IfException edges and the
        // exception object on the stack becomes a Phi copied on each edge.
        //
        //   0: bipush 3 ; 2: newarray int ; 4: astore_2
        //   5: aload_2 ; 6: iload_0 ; 7: iaload      <- try1 [5,8)
        //   8: pop
        //   9: aload_2 ; 10: iload_1 ; 11: iaload    <- try2 [9,12)
        //  12: pop ; 13: bipush 3 ; 15: ireturn
        //  16: astore_3                               <- handler_pc = 16
        //  17: bipush 7 ; 19: ireturn
        use super::super::builder::ExceptionHandler;
        let _ = test_loader();
        let loader = crate::share::runtime::threading::jvm_loader().unwrap();
        register_klass(&loader, b"java/lang/ArrayIndexOutOfBoundsException", &[]);
        let code: &[u8] = &[
            0x10, 0x03, // 0: bipush 3
            0xbc, 0x0a, // 2: newarray int
            0x4d, // 4: astore_2
            0x2c, // 5: aload_2
            0x1a, // 6: iload_0
            0x2e, // 7: iaload
            0x57, // 8: pop
            0x2c, // 9: aload_2
            0x1b, // 10: iload_1
            0x2e, // 11: iaload
            0x57, // 12: pop
            0x10, 0x03, // 13: bipush 3
            0xac, // 15: ireturn
            0x4e, // 16: astore_3 (handler)
            0x10, 0x07, // 17: bipush 7
            0xac, // 19: ireturn
        ];
        let handlers = [
            ExceptionHandler {
                start_pc: 5,
                end_pc: 8,
                handler_pc: 16,
                catch_type: 0,
            },
            ExceptionHandler {
                start_pc: 9,
                end_pc: 12,
                handler_pc: 16,
                catch_type: 0,
            },
        ];
        let m = MethodCode {
            name: "test",
            max_locals: 4,
            max_stack: 8,
            param_slots: 2,
            code,
            handlers: &handlers,
        };
        let options = BackendOptions {
            params: vec![iparam(0), iparam(1)],
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &BuilderContext::empty());
        c.optimize();
        let emitted = c
            .backend(&m, &BuilderContext::empty(), &options)
            .expect("c2 backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        for (x, y, want) in [(1, 1, 3), (9, 1, 7), (1, 9, 7), (9, 9, 7)] {
            let result = try_invoke(
                addr,
                b"(II)I",
                true,
                &[Value::Int(x), Value::Int(y)],
                0,
            )
            .unwrap();
            match result {
                CompiledResult::Value(Some(Value::Int(v))) => {
                    assert_eq!(v, want, "x={x} y={y}")
                }
                other => panic!("unexpected result for {x},{y}: {:?}", other),
            }
        }
    }


    #[test]
    fn e2e_long_loop_phis() {
        // Regression: production C2 run of this loop shape returned 0.
        //   s=0L; i=0L; while (i < 2000000L) { s += (i*2654435761L)>>>13;
        //   s ^= s<<7; if ((i&1023L)==0L) s+=i; i+=1L; } return s;
        use super::super::builder::ConstVal;
        use std::collections::HashMap;
        let _ = test_loader();
        let code: &[u8] = &[
            0x09, // 0: lconst_0
            0x3f, // 1: lstore_0
            0x09, // 2: lconst_0
            0x41, // 3: lstore_2
            0x20, // 4: lload_2
            0x14, 0x00, 0x0f, // 5: ldc2_w #15 (2000000)
            0x94, // 8: lcmp
            0x9c, 0x00, 0x2a, // 9: ifge 51
            0x1e, // 12: lload_0
            0x20, // 13: lload_2
            0x14, 0x00, 0x11, // 14: ldc2_w #17 (2654435761)
            0x69, // 17: lmul
            0x10, 0x0d, // 18: bipush 13
            0x7d, // 20: lushr
            0x61, // 21: ladd
            0x3f, // 22: lstore_0
            0x1e, // 23: lload_0
            0x1e, // 24: lload_0
            0x10, 0x07, // 25: bipush 7
            0x79, // 27: lshl
            0x83, // 28: lxor
            0x3f, // 29: lstore_0
            0x20, // 30: lload_2
            0x14, 0x00, 0x13, // 31: ldc2_w #19 (1023)
            0x7f, // 34: land
            0x09, // 35: lconst_0
            0x94, // 36: lcmp
            0x9a, 0x00, 0x07, // 37: ifne 44
            0x1e, // 40: lload_0
            0x20, // 41: lload_2
            0x61, // 42: ladd
            0x3f, // 43: lstore_0
            0x20, // 44: lload_2
            0x0a, // 45: lconst_1
            0x61, // 46: ladd
            0x41, // 47: lstore_2
            0xa7, 0xff, 0xd4, // 48: goto 4
            0x1e, // 51: lload_0
            0xad, // 52: lreturn
        ];
        let mut constants: HashMap<u16, ConstVal> = HashMap::new();
        constants.insert(15, ConstVal::Long(10));
        constants.insert(17, ConstVal::Long(2_654_435_761));
        constants.insert(19, ConstVal::Long(1023));
        let fields = HashMap::new();
        let methods = HashMap::new();
        let sites = HashMap::new();
        let ctx = super::super::builder::BuilderContext {
            fields: &fields,
            methods: &methods,
            constants: &constants,
            sites: &sites,
        };
        let m = MethodCode {
            name: "burn",
            max_locals: 4,
            max_stack: 8,
            param_slots: 0,
            code,
            handlers: &[],
        };
        let options = BackendOptions {
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &ctx);
        c.optimize();
        let emitted = c
            .backend(&m, &ctx, &options)
            .expect("c2 backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        let result = try_invoke(addr, b"()J", true, &[], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Long(v))) => {
                assert_eq!(v, 9_039_158_812_727_735_167i64)
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_osr_long_loop() {
        // OSR entry into the same burn-loop shape at the loop header
        // (bci 4): locals s/i arrive via the marshal buffer instead of
        // parameters.  Entering with s=0, i=0 must reproduce the
        // whole-method result.
        use super::super::builder::{ConstVal, OsrKind, OsrSpec};
        use std::collections::HashMap;
        let _ = test_loader();
        let code: &[u8] = &[
            0x09, // 0: lconst_0
            0x3f, // 1: lstore_0
            0x09, // 2: lconst_0
            0x41, // 3: lstore_2
            0x20, // 4: lload_2
            0x14, 0x00, 0x0f, // 5: ldc2_w #15 (2000000)
            0x94, // 8: lcmp
            0x9c, 0x00, 0x2a, // 9: ifge 51
            0x1e, // 12: lload_0
            0x20, // 13: lload_2
            0x14, 0x00, 0x11, // 14: ldc2_w #17 (2654435761)
            0x69, // 17: lmul
            0x10, 0x0d, // 18: bipush 13
            0x7d, // 20: lushr
            0x61, // 21: ladd
            0x3f, // 22: lstore_0
            0x1e, // 23: lload_0
            0x1e, // 24: lload_0
            0x10, 0x07, // 25: bipush 7
            0x79, // 27: lshl
            0x83, // 28: lxor
            0x3f, // 29: lstore_0
            0x20, // 30: lload_2
            0x14, 0x00, 0x13, // 31: ldc2_w #19 (1023)
            0x7f, // 34: land
            0x09, // 35: lconst_0
            0x94, // 36: lcmp
            0x9a, 0x00, 0x07, // 37: ifne 44
            0x1e, // 40: lload_0
            0x20, // 41: lload_2
            0x61, // 42: ladd
            0x3f, // 43: lstore_0
            0x20, // 44: lload_2
            0x0a, // 45: lconst_1
            0x61, // 46: ladd
            0x41, // 47: lstore_2
            0xa7, 0xff, 0xd4, // 48: goto 4
            0x1e, // 51: lload_0
            0xad, // 52: lreturn
        ];
        let mut constants: HashMap<u16, ConstVal> = HashMap::new();
        constants.insert(15, ConstVal::Long(10));
        constants.insert(17, ConstVal::Long(2_654_435_761));
        constants.insert(19, ConstVal::Long(1023));
        let fields = HashMap::new();
        let methods = HashMap::new();
        let sites = HashMap::new();
        let ctx = super::super::builder::BuilderContext {
            fields: &fields,
            methods: &methods,
            constants: &constants,
            sites: &sites,
        };
        let m = MethodCode {
            name: "burn",
            max_locals: 4,
            max_stack: 8,
            param_slots: 1, // slot 0 reserves the OSR buffer pointer
            code,
            handlers: &[],
        };
        let osr = OsrSpec {
            entry_bci: 4,
            local_kinds: vec![OsrKind::I64; 4],
            stack_kinds: vec![],
        };
        let options = BackendOptions {
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            osr: true,
            ..BackendOptions::default()
        };
        let mut c = Compile::compile_osr(&m, &ctx, &osr);
        c.optimize();
        let emitted = c
            .backend(&m, &ctx, &options)
            .expect("c2 osr backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        // Interpreter state at the loop header: s=0, i=0.
        let locals = vec![
            Value::Long(0),
            Value::Empty,
            Value::Long(0),
            Value::Empty,
        ];
        let result = crate::share::jitruntime::runtime::invoke_osr(
            addr,
            b"()J",
            true,
            &locals,
            &[],
        )
        .expect("invoke_osr should marshal");
        match result {
            CompiledResult::Value(Some(Value::Long(v))) => {
                assert_eq!(v, 9_039_158_812_727_735_167i64)
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn e2e_sumloop_shape() {
        // C2Smoke.sumLoop(J)J shape: s/i live at locals 2/4 (param n at 0/1),
        // generic lload/lstore index forms.  C2 returned n in production.
        use super::super::builder::ConstVal;
        use std::collections::HashMap;
        let _ = test_loader();
        let code: &[u8] = &[
            0x09, // 0: lconst_0
            0x41, // 1: lstore_2      (s)
            0x09, // 2: lconst_0
            0x37, 0x04, // 3: lstore 4  (i)
            0x16, 0x04, // 5: lload 4
            0x1e, // 7: lload_0       (n)
            0x94, // 8: lcmp
            0x9c, 0x00, 0x17, // 9: ifge 32
            0x20, // 12: lload_2      (s)
            0x16, 0x04, // 13: lload 4 (i)
            0x14, 0x00, 0x0b, // 15: ldc2_w #11 (3)
            0x69, // 18: lmul
            0x0a, // 19: lconst_1
            0x61, // 20: ladd
            0x61, // 21: ladd
            0x41, // 22: lstore_2
            0x16, 0x04, // 23: lload 4
            0x0a, // 25: lconst_1
            0x61, // 26: ladd
            0x37, 0x04, // 27: lstore 4
            0xa7, 0xff, 0xe8, // 29: goto 5
            0x20, // 32: lload_2
            0xad, // 33: lreturn
        ];
        let mut constants: HashMap<u16, ConstVal> = HashMap::new();
        constants.insert(11, ConstVal::Long(3));
        let fields = HashMap::new();
        let methods = HashMap::new();
        let sites = HashMap::new();
        let ctx = super::super::builder::BuilderContext {
            fields: &fields,
            methods: &methods,
            constants: &constants,
            sites: &sites,
        };
        let m = MethodCode {
            name: "sumLoop",
            max_locals: 6,
            max_stack: 4,
            param_slots: 2,
            code,
            handlers: &[],
        };
        let options = BackendOptions {
            stubs: jit_stub_map(),
            stubs_no_unwind: jit_stub_map_no_unwind(),
            params: vec![lparam(0)],
            ..BackendOptions::default()
        };
        let mut c = Compile::compile(&m, &ctx);
        c.optimize();
        let emitted = c
            .backend(&m, &ctx, &options)
            .expect("c2 backend should emit");
        let addr = publish_executable(&emitted.code, emitted.code.len())
            .expect("publish executable");
        let result = try_invoke(addr, b"(J)J", true, &[Value::Long(100)], 0).unwrap();
        match result {
            CompiledResult::Value(Some(Value::Long(v))) => {
                assert_eq!(v, 14_950i64)
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

}
