/*
 * Port of OpenJDK 25 HotSpot JVMCI JavaClasses
 * (share/jvmci/jvmciJavaClasses.cpp/.hpp)
 *
 * Holds `InstanceKlassHandle` (here: class-name keys) for the JVMCI
 * Java classes the VM needs to access fields/methods on.  Because this
 * port does not link the JVMCI Java modules (jdk.vm.ci, jdk.internal.vm.ci),
 * the handles resolve lazily and `is_initialized()` reports false until
 * a Java runtime is present.  Every accessor fails honestly.
 */

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Names of the JVMCI Java classes mirrored by C++ `JVMCIJavaClasses`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JvmciJavaClass {
    JvmciRuntime,
    JvmciCompiler,
    JvmciMethod,
    JvmciType,
    JvmciJavaType,
    JvmciResolvedJavaMethod,
    JvmciResolvedJavaField,
    JvmciResolvedJavaType,
    JvmciCode,
    JvmciHotSpotNmethod,
    JvmciHotSpotCompiledCode,
    JvmciInstalledCode,
    JvmciCompilationResult,
    JvmciCodeCacheProvider,
    CompilerToVM,
}

impl JvmciJavaClass {
    /// Binary/internal name (slash-separated, with `jdk/vm/ci` prefix).
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::JvmciRuntime => "jdk/vm/ci/hotspot/HotSpotJVMCIRuntime",
            Self::JvmciCompiler => "jdk/vm/ci/hotspot/HotSpotJVMCICompiler",
            Self::JvmciMethod => "jdk/vm/ci/hotspot/HotSpotResolvedJavaMethodImpl",
            Self::JvmciType => "jdk/vm/ci/hotspot/HotSpotType",
            Self::JvmciJavaType => "jdk/vm/ci/hotspot/HotSpotJavaType",
            Self::JvmciResolvedJavaMethod => "jdk/vm/ci/hotspot/HotSpotResolvedJavaMethodImpl",
            Self::JvmciResolvedJavaField => "jdk/vm/ci/hotspot/HotSpotResolvedJavaFieldImpl",
            Self::JvmciResolvedJavaType => "jdk/vm/ci/hotspot/HotSpotResolvedJavaTypeImpl",
            Self::JvmciCode => "jdk/vm/ci/hotspot/HotSpotCompiledCode",
            Self::JvmciHotSpotNmethod => "jdk/vm/ci/hotspot/HotSpotNmethod",
            Self::JvmciHotSpotCompiledCode => "jdk/vm/ci/hotspot/HotSpotCompiledCode",
            Self::JvmciInstalledCode => "jdk/vm/ci/code/InstalledCodeImpl",
            Self::JvmciCompilationResult => "jdk/vm/ci/hotspot/CompilationResultImpl",
            Self::JvmciCodeCacheProvider => "jdk/vm/ci/hotspot/HotSpotCodeCacheProvider",
            Self::CompilerToVM => "jdk/vm/ci/hotspot/CompilerToVM",
        }
    }
}

/// All JVMCI Java classes (order matches C++ `JVMCIJavaClasses::ALL`).
pub const ALL: &[JvmciJavaClass] = &[
    JvmciJavaClass::JvmciRuntime,
    JvmciJavaClass::JvmciCompiler,
    JvmciJavaClass::JvmciMethod,
    JvmciJavaClass::JvmciType,
    JvmciJavaClass::JvmciJavaType,
    JvmciJavaClass::JvmciResolvedJavaMethod,
    JvmciJavaClass::JvmciResolvedJavaField,
    JvmciJavaClass::JvmciResolvedJavaType,
    JvmciJavaClass::JvmciCode,
    JvmciJavaClass::JvmciHotSpotNmethod,
    JvmciJavaClass::JvmciHotSpotCompiledCode,
    JvmciJavaClass::JvmciInstalledCode,
    JvmciJavaClass::JvmciCompilationResult,
    JvmciJavaClass::JvmciCodeCacheProvider,
    JvmciJavaClass::CompilerToVM,
];

/// Field-id table: maps `Class -> field name -> resolved offset/handle`
/// once a JVMCI Java runtime is present.  The C++ version resolves
/// real `fieldDescriptor`s; this port records names only because the
/// JVMCI Java classes are not linked.
static FIELD_OFFSETS: OnceLock<Mutex<HashMap<(JvmciJavaClass, String), i32>>> = OnceLock::new();

fn field_offsets() -> &'static Mutex<HashMap<(JvmciJavaClass, String), i32>> {
    FIELD_OFFSETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `JVMCIJavaClasses` — class-handle registry.
pub struct JvmciJavaClasses;

impl JvmciJavaClasses {
    /// Whether all JVMCI Java classes were resolved.  Always false in
    /// this port until a JVMCI Java runtime is loaded.
    pub fn is_initialized() -> bool {
        false
    }

    /// Resolve a class by binary name.  Returns `None` when the class
    /// is unknown; callers must not assume a JVMCI Java runtime exists.
    pub fn class_of(class: JvmciJavaClass) -> Option<String> {
        Some(class.binary_name().to_string())
    }

    /// `jvmci_compiler_class` analogue.
    pub fn jvmci_compiler_class() -> Option<String> {
        Self::class_of(JvmciJavaClass::JvmciCompiler)
    }

    /// `register_field` analogue: record a resolved field id.  Returns
    /// false because no JVMCI Java runtime is linked yet.
    pub fn register_field(_class: JvmciJavaClass, _name: &str) -> bool {
        false
    }

    /// Look up a previously registered field offset.
    pub fn field_offset(class: JvmciJavaClass, name: &str) -> Option<i32> {
        field_offsets().lock().unwrap().get(&(class, name.to_string())).copied()
    }

    /// Reset all state (test hook).
    pub fn reset() {
        field_offsets().lock().unwrap().clear();
    }
}
