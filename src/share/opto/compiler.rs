/*
 * Port of OpenJDK 25 HotSpot C2 compiler entry (share/opto/c2compiler.cpp)
 * reduced to the AbstractCompiler contract used by CompileBroker.
 *
 * `C2Compiler` drives the ported Opto pipeline for `CompLevel::FullOptimization`:
 *   bytecode -> ideal graph (builder) -> GVN/CCP/DCE -> schedule ->
 *   linear-scan register allocation -> x86-64 emission -> CompileOutcome.
 *
 * The executable subset is identical to the C1 tier: primitive int/long
 * signatures and pure local computation.  Heap, calls, monitors, casts,
 * exceptions and division/remainder are rejected by `codegen::validate_supported`
 * rather than emitting incorrect code.
 */

use super::builder::{BuilderContext, ExceptionHandler, MethodCode};
use super::codegen::{BackendError, BackendOptions, ParamKind, ParamSlot};
use super::compile::Compile;
use crate::share::compiler::abstractCompiler::{AbstractCompiler, CompileError, CompilerType};
use crate::share::compiler::compileTask::{CompileOutcome, CompileTask, FailureReason};
use crate::share::compiler::compilerDefinitions::CompLevel;
use crate::share::oops::objLayout::ObjLayoutMode;

pub struct C2Compiler {
    pub compiled_methods: usize,
}

impl C2Compiler {
    pub fn new() -> Self {
        Self {
            compiled_methods: 0,
        }
    }
}

impl Default for C2Compiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Parses the executable-subset descriptor into ABI argument slots.
/// Returns (params in ABI order, total JVM local slots for parameters).
/// `this` occupies local slot 0 for instance methods.
fn parse_params(descriptor: &str, is_static: bool) -> Option<(Vec<ParamSlot>, usize)> {
    let bytes = descriptor.as_bytes();
    let mut params = Vec::new();
    let mut slot = 0usize;
    if !is_static {
        params.push(ParamSlot {
            local_slot: slot,
            kind: ParamKind::Ref,
        });
        slot += 1;
    }
    if bytes.first() != Some(&b'(') {
        return None;
    }
    let mut i = 1usize;
    while i < bytes.len() && bytes[i] != b')' {
        let (kind, width) = match bytes[i] {
            b'B' | b'C' | b'I' | b'S' | b'Z' => (ParamKind::I32, 1usize),
            b'J' => (ParamKind::I64, 2usize),
            // Floats/references are outside the executable bridge subset.
            _ => return None,
        };
        params.push(ParamSlot {
            local_slot: slot,
            kind,
        });
        slot += width;
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b')' {
        return None;
    }
    Some((params, slot))
}

impl AbstractCompiler for C2Compiler {
    fn compiler_type(&self) -> CompilerType {
        CompilerType::C2
    }

    fn supports_level(&self, level: CompLevel) -> bool {
        level == CompLevel::FullOptimization
    }

    fn compile_method(&mut self, task: &CompileTask) -> Result<CompileOutcome, CompileError> {
        let method = task.method_obj.as_ref().ok_or_else(|| {
            CompileError::new(
                FailureReason::Unresolved,
                "missing Method handle",
                CompLevel::FullOptimization,
            )
        })?;
        if task.is_osr() {
            return Err(CompileError::unsupported(
                "C2 OSR compilation is not ported",
                CompLevel::FullOptimization,
            ));
        }
        let bytecode = method.bytecodes();
        if bytecode.is_empty() {
            return Err(CompileError::new(
                FailureReason::MethodNotCompilable,
                "empty bytecode",
                CompLevel::FullOptimization,
            ));
        }
        let descriptor = task.method.descriptor.as_str();
        if !crate::share::jitruntime::signature_supported(descriptor.as_bytes(), method.is_static())
        {
            return Err(CompileError::new(
                FailureReason::Unsupported,
                format!("signature {} outside executable subset", descriptor),
                CompLevel::FullOptimization,
            ));
        }
        let (params, param_slots) =
            parse_params(descriptor, method.is_static()).ok_or_else(|| {
                CompileError::new(
                    FailureReason::Unsupported,
                    format!("unsupported signature {}", descriptor),
                    CompLevel::FullOptimization,
                )
            })?;
        let handlers: Vec<ExceptionHandler> = method
            .const_method()
            .exception_table()
            .iter()
            .map(|e| ExceptionHandler {
                start_pc: e.start_pc as u32,
                end_pc: e.end_pc as u32,
                handler_pc: e.handler_pc as u32,
                catch_type: e.catch_type as u32,
            })
            .collect();
        // C2 dispatches every exception edge to the first covering
        // handler unconditionally; typed catches (catch_type != 0) need
        // runtime assignability checks that are not implemented yet, so
        // methods containing them stay in the interpreter.
        if handlers.iter().any(|h| h.catch_type != 0) {
            return Err(CompileError::unsupported(
                "typed exception handlers are not supported by C2 yet",
                CompLevel::FullOptimization,
            ));
        }
        let method_code = MethodCode {
            name: task.method.method_name.as_str(),
            max_locals: method.max_locals() as usize,
            max_stack: method.max_stack() as usize,
            param_slots,
            code: bytecode,
            handlers: &handlers,
        };
        let mut sites: std::collections::HashMap<u16, super::builder::SiteInfo> =
            std::collections::HashMap::new();
        if let Some(loader) = crate::share::runtime::threading::jvm_loader()
            && let Some(klass) = loader
                .classes
                .borrow()
                .get(task.method.class_name.as_bytes())
                .cloned()
        {
            sites = super::builder::resolve_sites(&klass, bytecode);
        }
        if let Some(md) = &task.jit_metadata {
            for (idx, s) in &md.sites {
                if *idx <= u16::MAX as u32 {
                    sites
                        .entry(*idx as u16)
                        .or_insert_with(|| super::builder::SiteInfo {
                            class: s.class.clone(),
                            name: s.name.clone(),
                            desc: s.desc.clone(),
                        });
                }
            }
        }
        let ctx = BuilderContext::with_sites(&sites);
        let options = BackendOptions {
            stubs: crate::share::jitruntime::stub_map(),
            stubs_no_unwind: crate::share::jitruntime::stub_map_no_unwind(),
            params,
            emit_safepoints: true,
            obj_layout: ObjLayoutMode::Uncompressed,
        };
        let mut compile = Compile::compile(&method_code, &ctx);
        let emitted = compile
            .backend(&method_code, &ctx, &options)
            .map_err(|e| match e {
                BackendError::MissingRuntimeStub(_) => CompileError::new(
                    FailureReason::Unresolved,
                    e.to_string(),
                    CompLevel::FullOptimization,
                ),
                BackendError::Unsupported(_) => CompileError::new(
                    FailureReason::Unsupported,
                    e.to_string(),
                    CompLevel::FullOptimization,
                ),
                other => CompileError::new(
                    FailureReason::InternalError,
                    other.to_string(),
                    CompLevel::FullOptimization,
                ),
            })?;
        self.compiled_methods += 1;
        Ok(CompileOutcome {
            code_bytes: emitted.code,
            frame_size: emitted.frame_size,
            pc_descs: emitted.pc_descs,
            scopes: Vec::new(),
            scope_data: Vec::new(),
            exception_table: Default::default(),
            pc_handlers: Default::default(),
            oop_maps: Default::default(),
            num_inlined_bytecodes: 0,
        })
    }
}
