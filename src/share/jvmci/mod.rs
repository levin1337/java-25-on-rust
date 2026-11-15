/*
 * OpenJDK 25 HotSpot jvmci/ port.
 *
 * This module ports the HotSpot JVMCI framework (share/jvmci in
 * srcJVMc++): the JVMCI<->VM environment, runtime, code installer,
 * compiler dispatch and CompilerToVM downcalls.  The Graal compiler
 * itself is not part of this port; without a JVMCI compiler backend
 * (libjvmci / jdk.vm.ci.hotspot.HotSpotJVMCIRuntime) all compiler
 * entry points fail with honest UnsupportedOperationException-style
 * JVMCI errors.
 */

pub mod jvmci;
pub mod jvmciCodeInstaller;
pub mod jvmciCompiler;
pub mod jvmciCompilerToVM;
pub mod jvmciCompilerToVMInit;
pub mod jvmciEnv;
pub mod jvmciExceptions;
pub mod jvmciGlobals;
pub mod jvmciJavaClasses;
pub mod jvmciObject;
pub mod jvmciRuntime;
pub mod metadataHandles;
pub mod vmStructsJvmci;
pub mod vmSymbolsJvmci;
