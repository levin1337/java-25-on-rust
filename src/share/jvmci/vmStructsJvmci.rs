/*
 * Port of OpenJDK 25 HotSpot JVMCI VMStructs
 * (share/jvmci/vmStructs_jvmci.cpp/.hpp)
 *
 * The C++ file declares Serviceability-Agent-visible fields for the
 * JVMCI runtime/compiler state.  This port records the same field
 * names and owning types so SA-style tooling and tests can resolve
 * them symbolically; offsets are not meaningful for the Rust
 * structures and are therefore not fabricated.
 */

/// A VMStructs entry: (type name, field name, kind).
#[derive(Clone, Copy, Debug)]
pub struct VmStructField {
    pub type_name: &'static str,
    pub field_name: &'static str,
    /// "int", "bool", "uint64_t", "oop", ... (informational).
    pub kind: &'static str,
}

/// Fields declared by `vmStructs_jvmci.cpp` (non-exhaustive port of the
/// `VM_STRUCTS_DO` list, covering the runtime-visible JVMCI state).
pub const JVMCI_VM_STRUCTS: &[VmStructField] = &[
    VmStructField {
        type_name: "JVMCI",
        field_name: "_global_compilation_ticks",
        kind: "int64_t",
    },
    VmStructField {
        type_name: "JVMCI",
        field_name: "_num_compiler_runtimes",
        kind: "int",
    },
    VmStructField {
        type_name: "JVMCIRuntime",
        field_name: "_metadata_handles",
        kind: "MetadataHandles*",
    },
    VmStructField {
        type_name: "JVMCIRuntime",
        field_name: "_init_error",
        kind: "int",
    },
    VmStructField {
        type_name: "JVMCICompiler",
        field_name: "_methods_compiled",
        kind: "int",
    },
    VmStructField {
        type_name: "JVMCICompiler",
        field_name: "_bootstrapping",
        kind: "bool",
    },
    VmStructField {
        type_name: "JVMCICompiler",
        field_name: "_global_compilation_ticks",
        kind: "int64_t",
    },
    VmStructField {
        type_name: "HotSpotNmethod",
        field_name: "mirror",
        kind: "oop",
    },
];

/// Look up a VMStructs field by type and field name.
pub fn find(type_name: &str, field_name: &str) -> Option<&'static VmStructField> {
    JVMCI_VM_STRUCTS
        .iter()
        .find(|f| f.type_name == type_name && f.field_name == field_name)
}
