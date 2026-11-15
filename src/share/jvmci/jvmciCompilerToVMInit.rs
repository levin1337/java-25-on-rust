/*
 * Port of OpenJDK 25 HotSpot JVMCI CompilerToVMInit
 * (share/jvmci/jvmciCompilerToVMInit.cpp)
 *
 * Initialises the CompilerToVM native method bindings and related JVMCI
 * constants when the JVMCI Java runtime is about to be used.
 */

use crate::share::jvmci::jvmciCompilerToVM::CompilerToVM;

/// `CompilerToVMInit::initialize()` analogue: populate the native
/// dispatch table.  In C++ this is driven by `VM_Version::get_jvmci_version`;
/// this port keeps the registration idempotent and side-effect free.
pub fn initialize() {
    CompilerToVM::register_natives();
}

/// Check that a JVMCI Java runtime class can be located (used by
/// `JVMCIRuntime` bootstrap before attempting `getRuntime`).
pub fn jvmci_java_runtime_available() -> bool {
    // `jdk.vm.ci.hotspot.HotSpotJVMCIRuntime` availability is decided
    // by the Java heap; no JVMCI Java module is bundled with this VM
    // image, so the honest answer is `false` until one is present.
    false
}
