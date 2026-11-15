/*
 * Port of OpenJDK 25 HotSpot JVMCI VM symbols
 * (share/jvmci/vmSymbols_jvmci.hpp)
 *
 * Internal names and signatures of the jdk.vm.ci.* classes and of the
 * JVMCI-related methods/signatures referenced by VM code.
 */

/// Class internal names (JVMCI_VM_SYMBOLS_DO `template` entries).
pub mod classes {
    pub const SERVICES: &str = "jdk/vm/ci/services/Services";
    pub const JVMCI: &str = "jdk/vm/ci/runtime/JVMCI";
    pub const HOTSPOT_COMPILED_CODE: &str = "jdk/vm/ci/hotspot/HotSpotCompiledCode";
    pub const HOTSPOT_COMPILED_NMETHOD: &str = "jdk/vm/ci/hotspot/HotSpotCompiledNmethod";
    pub const COMPILER_TO_VM: &str = "jdk/vm/ci/hotspot/CompilerToVM";
    pub const HOTSPOT_INSTALLED_CODE: &str = "jdk/vm/ci/hotspot/HotSpotInstalledCode";
    pub const HOTSPOT_NMETHOD: &str = "jdk/vm/ci/hotspot/HotSpotNmethod";
    pub const HOTSPOT_RESOLVED_JAVA_METHOD_IMPL: &str =
        "jdk/vm/ci/hotspot/HotSpotResolvedJavaMethodImpl";
    pub const HOTSPOT_RESOLVED_OBJECT_TYPE_IMPL: &str =
        "jdk/vm/ci/hotspot/HotSpotResolvedObjectTypeImpl";
    pub const HOTSPOT_RESOLVED_OBJECT_TYPE_IMPL_FIELD_INFO: &str =
        "jdk/vm/ci/hotspot/HotSpotResolvedObjectTypeImpl$FieldInfo";
    pub const HOTSPOT_RESOLVED_PRIMITIVE_TYPE: &str =
        "jdk/vm/ci/hotspot/HotSpotResolvedPrimitiveType";
    pub const HOTSPOT_RESOLVED_JAVA_FIELD_IMPL: &str =
        "jdk/vm/ci/hotspot/HotSpotResolvedJavaFieldImpl";
    pub const HOTSPOT_COMPRESSED_NULL_CONSTANT: &str =
        "jdk/vm/ci/hotspot/HotSpotCompressedNullConstant";
    pub const HOTSPOT_OBJECT_CONSTANT_IMPL: &str =
        "jdk/vm/ci/hotspot/HotSpotObjectConstantImpl";
    pub const HOTSPOT_METHOD_DATA: &str = "jdk/vm/ci/hotspot/HotSpotMethodData";
    pub const DIRECT_HOTSPOT_OBJECT_CONSTANT_IMPL: &str =
        "jdk/vm/ci/hotspot/DirectHotSpotObjectConstantImpl";
    pub const INDIRECT_HOTSPOT_OBJECT_CONSTANT_IMPL: &str =
        "jdk/vm/ci/hotspot/IndirectHotSpotObjectConstantImpl";
    pub const HOTSPOT_STACK_FRAME_REFERENCE: &str =
        "jdk/vm/ci/hotspot/HotSpotStackFrameReference";
    pub const HOTSPOT_CONSTANT_POOL: &str = "jdk/vm/ci/hotspot/HotSpotConstantPool";
    pub const HOTSPOT_JVMCI_RUNTIME: &str = "jdk/vm/ci/hotspot/HotSpotJVMCIRuntime";
    pub const HOTSPOT_SPECULATION_LOG: &str = "jdk/vm/ci/hotspot/HotSpotSpeculationLog";
    pub const HOTSPOT_COMPILATION_REQUEST_RESULT: &str =
        "jdk/vm/ci/hotspot/HotSpotCompilationRequestResult";
    pub const VM_FIELD: &str = "jdk/vm/ci/hotspot/VMField";
    pub const VM_FLAG: &str = "jdk/vm/ci/hotspot/VMFlag";
    pub const VM_INTRINSIC_METHOD: &str = "jdk/vm/ci/hotspot/VMIntrinsicMethod";
    pub const RESOLVED_JAVA_METHOD: &str = "jdk/vm/ci/meta/ResolvedJavaMethod";
    pub const JAVA_CONSTANT: &str = "jdk/vm/ci/meta/JavaConstant";
    pub const PRIMITIVE_CONSTANT: &str = "jdk/vm/ci/meta/PrimitiveConstant";
    pub const RAW_CONSTANT: &str = "jdk/vm/ci/meta/RawConstant";
    pub const NULL_CONSTANT: &str = "jdk/vm/ci/meta/NullConstant";
    pub const EXCEPTION_HANDLER: &str = "jdk/vm/ci/meta/ExceptionHandler";
    pub const JAVA_KIND: &str = "jdk/vm/ci/meta/JavaKind";
    pub const VALUE_KIND: &str = "jdk/vm/ci/meta/ValueKind";
    pub const VALUE: &str = "jdk/vm/ci/meta/Value";
    pub const ASSUMPTIONS_CONCRETE_SUBTYPE: &str = "jdk/vm/ci/meta/Assumptions$ConcreteSubtype";
    pub const ASSUMPTIONS_LEAF_TYPE: &str = "jdk/vm/ci/meta/Assumptions$LeafType";
    pub const ASSUMPTIONS_NO_FINALIZABLE_SUBCLASS: &str =
        "jdk/vm/ci/meta/Assumptions$NoFinalizableSubclass";
    pub const ASSUMPTIONS_CONCRETE_METHOD: &str = "jdk/vm/ci/meta/Assumptions$ConcreteMethod";
    pub const ASSUMPTIONS_CALL_SITE_TARGET_VALUE: &str =
        "jdk/vm/ci/meta/Assumptions$CallSiteTargetValue";
    pub const ARCHITECTURE: &str = "jdk/vm/ci/code/Architecture";
    pub const BYTECODE_FRAME: &str = "jdk/vm/ci/code/BytecodeFrame";
    pub const BYTECODE_POSITION: &str = "jdk/vm/ci/code/BytecodePosition";
    pub const INSTALLED_CODE: &str = "jdk/vm/ci/code/InstalledCode";
    pub const INVALID_INSTALLED_CODE_EXCEPTION: &str =
        "jdk/vm/ci/code/InvalidInstalledCodeException";
    pub const INSPECTED_FRAME_VISITOR: &str = "jdk/vm/ci/code/stack/InspectedFrameVisitor";
    pub const JVMCI_ERROR: &str = "jdk/vm/ci/common/JVMCIError";
}

/// Method names and signatures used by the JVMCI upcall path.
pub mod methods {
    pub const VISIT_FRAME_NAME: &str = "visitFrame";
    pub const VISIT_FRAME_SIGNATURE: &str =
        "(Ljdk/vm/ci/code/stack/InspectedFrame;)Ljava/lang/Object;";
    pub const COMPILE_METHOD_NAME: &str = "compileMethod";
    pub const COMPILE_METHOD_SIGNATURE: &str =
        "(Ljdk/vm/ci/hotspot/HotSpotResolvedJavaMethod;IJI)Ljdk/vm/ci/hotspot/HotSpotCompilationRequestResult;";
    pub const IS_GC_SUPPORTED_NAME: &str = "isGCSupported";
    pub const IS_INTRINSIC_SUPPORTED_NAME: &str = "isIntrinsicSupported";
    pub const FROM_METASPACE_NAME: &str = "fromMetaspace";
    pub const METHOD_FROM_METASPACE_SIGNATURE: &str =
        "(JLjdk/vm/ci/hotspot/HotSpotResolvedObjectTypeImpl;)Ljdk/vm/ci/hotspot/HotSpotResolvedJavaMethod;";
    pub const CONSTANT_POOL_FROM_METASPACE_SIGNATURE: &str =
        "(J)Ljdk/vm/ci/hotspot/HotSpotConstantPool;";
    pub const KLASS_FROM_METASPACE_SIGNATURE: &str =
        "(J)Ljdk/vm/ci/hotspot/HotSpotResolvedObjectTypeImpl;";
    pub const PRIMITIVE_FROM_METASPACE_SIGNATURE: &str =
        "(Ljdk/vm/ci/hotspot/HotSpotObjectConstantImpl;C)Ljdk/vm/ci/hotspot/HotSpotResolvedPrimitiveType;";
    pub const GET_RUNTIME_NAME: &str = "getRuntime";
    pub const GET_RUNTIME_SIGNATURE: &str = "()Ljdk/vm/ci/runtime/JVMCIRuntime;";
    pub const INITIALIZE_RUNTIME_NAME: &str = "initializeRuntime";
    pub const INITIALIZE_RUNTIME_SIGNATURE: &str = GET_RUNTIME_SIGNATURE;
    pub const RUNTIME_NAME: &str = "runtime";
    pub const RUNTIME_SIGNATURE: &str = "()Ljdk/vm/ci/hotspot/HotSpotJVMCIRuntime;";
    pub const GET_COMPILER_NAME: &str = "getCompiler";
    pub const GET_COMPILER_SIGNATURE: &str = "()Ljdk/vm/ci/runtime/JVMCICompiler;";
    pub const EXCEPTION_TO_STRING_NAME: &str = "exceptionToString";
    pub const EXCEPTION_TO_STRING_SIGNATURE: &str =
        "(Ljava/lang/Throwable;ZZ)[Ljava/lang/String;";
    pub const POST_TRANSLATION_NAME: &str = "postTranslation";
    pub const GET_NAME_NAME: &str = "getName";
    pub const BOOTSTRAP_FINISHED_NAME: &str = "bootstrapFinished";
    pub const FOR_PRIMITIVE_NAME: &str = "forPrimitive";
    pub const FOR_PRIMITIVE_SIGNATURE: &str =
        "(CJ)Ljdk/vm/ci/meta/PrimitiveConstant;";
    pub const METHOD_STRING_BOOL_LONG_SIGNATURE: &str =
        "(Ljdk/vm/ci/hotspot/HotSpotResolvedJavaMethodImpl;Ljava/lang/String;ZJ)V";
}
