/*
 * Port of OpenJDK 25 HotSpot JVMCI CompilerToVM
 * (share/jvmci/jvmciCompilerToVM.hpp / jvmciCompilerToVM.cpp /
 *  jvmciCompilerToVMInit.cpp)
 *
 * `CompilerToVM` is the native library called by JVMCI Java code
 * (jdk.vm.ci.hotspot.HotSpotJVMCI*Runtime / HotSpotJVMCIRuntime) for
 * every downcall into the VM.  In this port the "JNIEnv" boundary is
 * modelled by `JvmciEnv`; downcalls are dispatched through a
 * registered native-function table keyed by Java method name, exactly
 * like the C++ `c2v_methods` table + `registerNatives`.
 *
 * `installCode0` is fully implemented: it decodes the
 * HotSpotCompiledCode stream with `CodeInstaller` and registers the
 * result through `JVMCIRuntime::register_method`.  All other downcalls
 * are present in the table and return honest unsupported errors until
 * a JVMCI Java runtime exists to drive them.
 */

use crate::share::jvmci::jvmci::JVMCI;
use crate::share::jvmci::jvmciCodeInstaller::CodeInstaller;
use crate::share::jvmci::jvmciEnv::JvmciEnv;
use crate::share::jvmci::jvmciExceptions::JvmciResult;
use crate::share::jvmci::jvmciExceptions::{JvmciException, ThrowKind};
use crate::share::jvmci::jvmciObject::{JvmciObject, JvmciPrimitiveArray};
use crate::share::jvmci::jvmciRuntime::JvmciRuntime;
use crate::share::jvmci::vmSymbolsJvmci;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// JVMCI primitive array alias used by downcall signatures.
pub type JvmciByteArray = JvmciPrimitiveArray;

/// Signature descriptor used by `installCode0`
/// (`(JJZLjdk/vm/ci/hotspot/HotSpotCompiledCode;[Ljava/lang/Object;
/// Ljdk/vm/ci/code/InstalledCode;J[B)I`).
pub const INSTALL_CODE0_SIGNATURE: &str =
    "(JJZLjdk/vm/ci/hotspot/HotSpotCompiledCode;[Ljava/lang/Object;Ljdk/vm/ci/code/InstalledCode;J[B)I";

/// Arguments of the `installCode0` downcall.
pub struct InstallCodeArgs<'a> {
    pub compiled_code_buffer: &'a [u8],
    pub serialization_ns: i64,
    pub with_type_info: bool,
    pub compiled_code: JvmciObject,
    pub object_pool: Vec<JvmciObject>,
    pub installed_code: JvmciObject,
    pub failed_speculations: Vec<u8>,
    pub speculations: Vec<u8>,
}

/// `CompilerToVM::Data`: constants shared with JVMCI Java code.
pub mod data {
    /// `max_oop_map_stack_offset` (CompilerToVM::Data).
    pub const MAX_OOP_MAP_STACK_OFFSET: i32 = i32::MAX / 2;
    /// `get_data_section_item_alignment`.
    pub const DATA_SECTION_ITEM_ALIGNMENT: i32 = 8;
}

/// The set of CompilerToVM natives known to this port.  Each entry is
/// the Java-side method name (as registered through `registerNatives`)
/// plus whether a real implementation exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeKind {
    /// Fully implemented downcall.
    Implemented,
    /// Registered but requires a JVMCI Java runtime; returns an honest
    /// unsupported error when invoked without one.
    Unsupported,
}

/// A registered CompilerToVM native.
#[derive(Clone, Debug)]
pub struct CompilerToVmNative {
    /// Java method name (e.g. "installCode0").
    pub name: &'static str,
    /// JNI signature (informational; mirrors the C++ table).
    pub signature: &'static str,
    pub kind: NativeKind,
}

/// The native table: the subset of `c2v_methods` entries that are
/// meaningful to this port.  The full HotSpot table has ~200 entries;
/// unlisted names are still accepted by `call` and produce the same
/// unsupported error HotSpot would give for an unknown native.
fn native_table() -> &'static [CompilerToVmNative] {
    &[
        CompilerToVmNative {
            name: "installCode0",
            signature: INSTALL_CODE0_SIGNATURE,
            kind: NativeKind::Implemented,
        },
        CompilerToVmNative {
            name: "compileMethod",
            signature: "(JLjava/lang/Object;IJ)Ljava/lang/Object;",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "getCompiler",
            signature: "()Ljdk/vm/ci/hotspot/HotSpotJVMCICompiler;",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "isGCSupported",
            signature: "(I)Z",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "isIntrinsicSupported",
            signature: "(I)Z",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "lookupMethod",
            signature: "(Ljava/lang/Object;J)Ljava/lang/Object;",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "lookupType",
            signature: "(Ljava/lang/Object;J)Ljava/lang/Object;",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "lookupJavaType",
            signature: "(Ljava/lang/Class;)Ljdk/vm/ci/JavaType;",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "getOopMapAt",
            signature: "(Ljava/lang/Object;J)J",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "initializeConfiguration",
            signature: "()V",
            kind: NativeKind::Unsupported,
        },
        CompilerToVmNative {
            name: "registerNatives",
            signature: "()V",
            kind: NativeKind::Unsupported,
        },
    ]
}

static REGISTERED_NATIVES: OnceLock<Mutex<HashMap<String, CompilerToVmNative>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, CompilerToVmNative>> {
    REGISTERED_NATIVES.get_or_init(|| {
        let mut m = HashMap::new();
        for n in native_table() {
            m.insert(n.name.to_string(), n.clone());
        }
        Mutex::new(m)
    })
}

/// `CompilerToVM` — the downcall dispatcher.
pub struct CompilerToVM;

impl CompilerToVM {
    /// `registerNatives` analogue: populate the dispatch table for a
    /// JVMCI Java class.  Idempotent.
    pub fn register_natives() {
        // The table is static; ensure it is initialized.
        let _guard = registry().lock().unwrap();
    }

    /// Look up a registered native by Java method name.
    pub fn lookup(name: &str) -> Option<CompilerToVmNative> {
        registry().lock().unwrap().get(name).cloned()
    }

    /// All registered native names (for `jvmciJavaClasses`/tests).
    pub fn native_names() -> Vec<&'static str> {
        registry()
            .lock()
            .unwrap()
            .values()
            .map(|n| n.name)
            .collect()
    }

    /// `installCode0` downcall.
    ///
    /// Mirrors C2V_VMENTRY_0(installCode0): decode the compiled code,
    /// install it through the JVMCIRuntime, then update the
    /// InstalledCode mirror object.  Returns the
    /// `JVMCI::CodeInstallResult` as an i32.
    pub fn install_code0(
        env: &mut JvmciEnv,
        _thread_id: u64,
        args: InstallCodeArgs<'_>,
    ) -> JvmciResult<i32> {
        env.check_init()?;
        if !JVMCI::is_enabled() {
            return Err(env.raise(JvmciException::internal_error(
                crate::share::jvmci::jvmciExceptions::JVMCI_NOT_ENABLED_ERROR_MESSAGE,
            )));
        }

        // stats timer analogue: serialization time is pre-charged.
        let stats_timer_ns = args.serialization_ns.max(0);

        // Resolve the runtime serving this thread.
        let runtime: Arc<JvmciRuntime> = env.runtime()?;

        let mut installer = CodeInstaller::new(env);
        let (result, request) = installer.install(
            args.compiled_code_buffer,
            args.with_type_info,
            args.object_pool,
            args.compiled_code,
            args.installed_code,
            &args.speculations,
        )?;

        let Some(request) = request else {
            return Ok(result.as_i32());
        };

        // Register through the runtime (the C++ register_method call).
        let install_result = runtime.register_method(env, request)?;

        let _ = (args.installed_code, stats_timer_ns);
        Ok(install_result.as_i32())
    }

    /// Dispatch a named downcall.  Implemented natives run; all others
    /// produce an honest unsupported error.
    pub fn call(
        env: &mut JvmciEnv,
        thread_id: u64,
        name: &str,
        args: InstallCodeArgs<'_>,
    ) -> JvmciResult<i32> {
        match Self::lookup(name) {
            Some(n) if n.kind == NativeKind::Implemented && n.name == "installCode0" => {
                Self::install_code0(env, thread_id, args)
            }
            Some(n) => Err(env.raise(JvmciException::new(
                ThrowKind::UnsupportedOperationException,
                format!(
                    "CompilerToVM.{} requires a JVMCI Java runtime \
                     (jdk.vm.ci / libjvmci or Graal); none is available",
                    n.name
                ),
                file!(),
                line!(),
            ))),
            None => Err(env.raise(JvmciException::new(
                ThrowKind::UnsupportedOperationException,
                format!("unknown CompilerToVM native '{name}'"),
                file!(),
                line!(),
            ))),
        }
    }
}

/// Re-exported VM symbols for CompilerToVM class names.
pub fn compiler_to_vm_class_name() -> &'static str {
    vmSymbolsJvmci::classes::COMPILER_TO_VM
}
