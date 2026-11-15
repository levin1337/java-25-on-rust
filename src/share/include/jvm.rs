use super::jni::*;
use super::jvm_constants::*;
use std::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct JvmExceptionTableEntry {
    pub start_pc: Jint,
    pub end_pc: Jint,
    pub handler_pc: Jint,
    pub catch_type: Jint,
}

#[repr(C)]
pub struct Jdk1_1InitArgs {
    pub version: Jint,
    pub properties: *mut *mut c_char,
    pub check_source: Jint,
    pub native_stack_size: Jint,
    pub java_stack_size: Jint,
    pub min_heap_size: Jint,
    pub max_heap_size: Jint,
    pub verify_mode: Jint,
    pub classpath: *mut c_char,
    pub vfprintf: Option<unsafe extern "C" fn(*mut c_void, *const c_char, ...) -> Jint>,
    pub exit: Option<unsafe extern "C" fn(Jint)>,
    pub abort: Option<unsafe extern "C" fn()>,
    pub enable_class_gc: Jint,
    pub enable_verbose_gc: Jint,
    pub disable_async_gc: Jint,
    pub verbose: Jint,
    pub debugging: Jboolean,
    pub debug_port: Jint,
}

unsafe extern "C" {
    // java.lang.Object
    pub fn JVM_IHashCode(env: JNIEnvPtr, obj: jobject) -> Jint;
    pub fn JVM_MonitorWait(env: JNIEnvPtr, obj: jobject, ms: Jlong);
    pub fn JVM_MonitorNotify(env: JNIEnvPtr, obj: jobject);
    pub fn JVM_MonitorNotifyAll(env: JNIEnvPtr, obj: jobject);
    pub fn JVM_Clone(env: JNIEnvPtr, obj: jobject) -> jobject;

    // java.lang.String
    pub fn JVM_InternString(env: JNIEnvPtr, str: jstring) -> jstring;

    // java.lang.System / Runtime
    pub fn JVM_AOTEndRecording(env: JNIEnvPtr) -> Jboolean;
    pub fn JVM_CurrentTimeMillis(env: JNIEnvPtr, ignored: jclass) -> Jlong;
    pub fn JVM_NanoTime(env: JNIEnvPtr, ignored: jclass) -> Jlong;
    pub fn JVM_GetNanoTimeAdjustment(env: JNIEnvPtr, ignored: jclass, offset_secs: Jlong) -> Jlong;
    pub fn JVM_ArrayCopy(
        env: JNIEnvPtr,
        ignored: jclass,
        src: jobject,
        src_pos: Jint,
        dst: jobject,
        dst_pos: Jint,
        length: Jint,
    );
    pub fn JVM_GetProperties(env: JNIEnvPtr) -> jobjectArray;
    pub fn JVM_BeforeHalt();
    pub fn JVM_Halt(code: Jint);
    pub fn JVM_GC();
    pub fn JVM_MaxObjectInspectionAge() -> Jlong;
    pub fn JVM_TotalMemory() -> Jlong;
    pub fn JVM_FreeMemory() -> Jlong;
    pub fn JVM_MaxMemory() -> Jlong;
    pub fn JVM_ActiveProcessorCount() -> Jint;
    pub fn JVM_IsUseContainerSupport() -> Jboolean;
    pub fn JVM_IsContainerized() -> Jboolean;
    pub fn JVM_LoadZipLibrary() -> *mut c_void;
    pub fn JVM_LoadLibrary(name: *const c_char, throw_exception: Jboolean) -> *mut c_void;
    pub fn JVM_UnloadLibrary(handle: *mut c_void);
    pub fn JVM_FindLibraryEntry(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    pub fn JVM_IsSupportedJNIVersion(version: Jint) -> Jboolean;
    pub fn JVM_GetVmArguments(env: JNIEnvPtr) -> jobjectArray;
    pub fn JVM_IsPreviewEnabled() -> Jboolean;
    pub fn JVM_IsContinuationsSupported() -> Jboolean;
    pub fn JVM_IsForeignLinkerSupported() -> Jboolean;
    pub fn JVM_IsStaticallyLinked() -> Jboolean;
    pub fn JVM_InitializeFromArchive(env: JNIEnvPtr, cls: jclass);
    pub fn JVM_RegisterLambdaProxyClassForArchiving(
        env: JNIEnvPtr,
        caller: jclass,
        interface_method_name: jstring,
        factory_type: jobject,
        interface_method_type: jobject,
        implementation_member: jobject,
        dynamic_method_type: jobject,
        lambda_proxy_class: jclass,
    );
    pub fn JVM_LookupLambdaProxyClassFromArchive(
        env: JNIEnvPtr,
        caller: jclass,
        interface_method_name: jstring,
        factory_type: jobject,
        interface_method_type: jobject,
        implementation_member: jobject,
        dynamic_method_type: jobject,
    ) -> jclass;
    pub fn JVM_GetCDSConfigStatus() -> Jint;
    pub fn JVM_GetRandomSeedForDumping() -> Jlong;
    pub fn JVM_LogLambdaFormInvoker(env: JNIEnvPtr, line: jstring);
    pub fn JVM_DumpClassListToFile(env: JNIEnvPtr, file_name: jstring);
    pub fn JVM_DumpDynamicArchive(env: JNIEnvPtr, archive_name: jstring);
    pub fn JVM_NeedsClassInitBarrierForCDS(env: JNIEnvPtr, cls: jclass) -> Jboolean;

    // java.lang.Throwable / StackTraceElement / NPE
    pub fn JVM_FillInStackTrace(env: JNIEnvPtr, throwable: jobject);
    pub fn JVM_InitStackTraceElementArray(
        env: JNIEnvPtr,
        elements: jobjectArray,
        backtrace: jobject,
        depth: Jint,
    );
    pub fn JVM_InitStackTraceElement(env: JNIEnvPtr, element: jobject, stack_frame_info: jobject);
    pub fn JVM_GetExtendedNPEMessage(env: JNIEnvPtr, throwable: jthrowable) -> jstring;

    // java.lang.StackWalker
    pub fn JVM_ExpandStackFrameInfo(env: JNIEnvPtr, obj: jobject);
    pub fn JVM_CallStackWalk(
        env: JNIEnvPtr,
        stack_stream: jobject,
        mode: Jint,
        skip_frames: Jint,
        cont_scope: jobject,
        cont: jobject,
        buffer_size: Jint,
        start_index: Jint,
        frames: jobjectArray,
    ) -> jobject;
    pub fn JVM_MoreStackWalk(
        env: JNIEnvPtr,
        stack_stream: jobject,
        mode: Jint,
        anchor: Jlong,
        last_batch_count: Jint,
        buffer_size: Jint,
        start_index: Jint,
        frames: jobjectArray,
    ) -> Jint;
    pub fn JVM_SetStackWalkContinuation(
        env: JNIEnvPtr,
        stack_stream: jobject,
        anchor: Jlong,
        frames: jobjectArray,
        cont: jobject,
    );

    // java.lang.Thread
    pub fn JVM_StartThread(env: JNIEnvPtr, thread: jobject);
    pub fn JVM_SetThreadPriority(env: JNIEnvPtr, thread: jobject, prio: Jint);
    pub fn JVM_Yield(env: JNIEnvPtr, thread_class: jclass);
    pub fn JVM_SleepNanos(env: JNIEnvPtr, thread_class: jclass, nanos: Jlong);
    pub fn JVM_CurrentCarrierThread(env: JNIEnvPtr, thread_class: jclass) -> jobject;
    pub fn JVM_CurrentThread(env: JNIEnvPtr, thread_class: jclass) -> jobject;
    pub fn JVM_SetCurrentThread(env: JNIEnvPtr, this_thread: jobject, the_thread: jobject);
    pub fn JVM_Interrupt(env: JNIEnvPtr, thread: jobject);
    pub fn JVM_HoldsLock(env: JNIEnvPtr, thread_class: jclass, obj: jobject) -> Jboolean;
    pub fn JVM_GetStackTrace(env: JNIEnvPtr, thread: jobject) -> jobject;
    pub fn JVM_CreateThreadSnapshot(env: JNIEnvPtr, thread: jobject) -> jobject;
    pub fn JVM_GetAllThreads(env: JNIEnvPtr, dummy: jclass) -> jobjectArray;
    pub fn JVM_SetNativeThreadName(env: JNIEnvPtr, jthread: jobject, name: jstring);
    pub fn JVM_DumpThreads(env: JNIEnvPtr, thread_class: jclass, threads: jobjectArray) -> jobjectArray;
    pub fn JVM_ScopedValueCache(env: JNIEnvPtr, thread_class: jclass) -> jobject;
    pub fn JVM_SetScopedValueCache(env: JNIEnvPtr, thread_class: jclass, cache: jobject);
    pub fn JVM_FindScopedValueBindings(env: JNIEnvPtr, thread_class: jclass) -> jobject;
    pub fn JVM_GetNextThreadIdOffset(env: JNIEnvPtr, thread_class: jclass) -> Jlong;

    // jdk.internal.vm.Continuation
    pub fn JVM_RegisterContinuationMethods(env: JNIEnvPtr, cls: jclass);

    // java.lang.Package / ref.Reference / PhantomReference
    pub fn JVM_GetSystemPackage(env: JNIEnvPtr, name: jstring) -> jstring;
    pub fn JVM_GetSystemPackages(env: JNIEnvPtr) -> jobjectArray;
    pub fn JVM_GetAndClearReferencePendingList(env: JNIEnvPtr) -> jobject;
    pub fn JVM_HasReferencePendingList(env: JNIEnvPtr) -> Jboolean;
    pub fn JVM_WaitForReferencePendingList(env: JNIEnvPtr);
    pub fn JVM_ReferenceGet(env: JNIEnvPtr, ref: jobject) -> jobject;
    pub fn JVM_ReferenceRefersTo(env: JNIEnvPtr, ref: jobject, o: jobject) -> Jboolean;
    pub fn JVM_ReferenceClear(env: JNIEnvPtr, ref: jobject);
    pub fn JVM_PhantomReferenceRefersTo(env: JNIEnvPtr, refr: jobject, o: jobject) -> Jboolean;

    // java.io.ObjectInputStream / java.lang.reflect.Array
    pub fn JVM_LatestUserDefinedLoader(env: JNIEnvPtr) -> jobject;
    pub fn JVM_GetArrayLength(env: JNIEnvPtr, arr: jobject) -> Jint;
    pub fn JVM_GetArrayElement(env: JNIEnvPtr, arr: jobject, index: Jint) -> jobject;
    pub fn JVM_GetPrimitiveArrayElement(
        env: JNIEnvPtr,
        arr: jobject,
        index: Jint,
        w_code: Jint,
    ) -> JValue;
    pub fn JVM_SetArrayElement(env: JNIEnvPtr, arr: jobject, index: Jint, val: jobject);
    pub fn JVM_SetPrimitiveArrayElement(
        env: JNIEnvPtr,
        arr: jobject,
        index: Jint,
        v: JValue,
        v_code: Jboolean,
    );
    pub fn JVM_NewArray(env: JNIEnvPtr, elt_class: jclass, length: Jint) -> jobject;
    pub fn JVM_NewMultiArray(env: JNIEnvPtr, elt_class: jclass, dim: jintArray) -> jobject;

    // Caller / class lookup
    pub fn JVM_GetCallerClass(env: JNIEnvPtr) -> jclass;
    pub fn JVM_FindPrimitiveClass(env: JNIEnvPtr, utf: *const c_char) -> jclass;
    pub fn JVM_FindClassFromBootLoader(env: JNIEnvPtr, name: *const c_char) -> jclass;
    pub fn JVM_FindClassFromCaller(
        env: JNIEnvPtr,
        name: *const c_char,
        init: Jboolean,
        loader: jobject,
        caller: jclass,
    ) -> jclass;
    pub fn JVM_FindClassFromClass(
        env: JNIEnvPtr,
        name: *const c_char,
        init: Jboolean,
        from: jclass,
    ) -> jclass;
    pub fn JVM_FindLoadedClass(env: JNIEnvPtr, loader: jobject, name: jstring) -> jclass;
    pub fn JVM_DefineClass(
        env: JNIEnvPtr,
        name: *const c_char,
        loader: jobject,
        buf: *const Jbyte,
        len: Jsize,
        pd: jobject,
    ) -> jclass;
    pub fn JVM_DefineClassWithSource(
        env: JNIEnvPtr,
        name: *const c_char,
        loader: jobject,
        buf: *const Jbyte,
        len: Jsize,
        pd: jobject,
        source: *const c_char,
    ) -> jclass;
    pub fn JVM_LookupDefineClass(
        env: JNIEnvPtr,
        lookup: jclass,
        name: *const c_char,
        buf: *const Jbyte,
        len: Jsize,
        pd: jobject,
        init: Jboolean,
        flags: Jint,
        class_data: jobject,
    ) -> jclass;

    // java.lang.Module
    pub fn JVM_DefineModule(
        env: JNIEnvPtr,
        module: jobject,
        is_open: Jboolean,
        version: jstring,
        location: jstring,
        packages: jobjectArray,
    );
    pub fn JVM_SetBootLoaderUnnamedModule(env: JNIEnvPtr, module: jobject);
    pub fn JVM_AddModuleExports(env: JNIEnvPtr, from_module: jobject, package: jstring, to_module: jobject);
    pub fn JVM_AddModuleExportsToAllUnnamed(env: JNIEnvPtr, from_module: jobject, package: jstring);
    pub fn JVM_AddModuleExportsToAll(env: JNIEnvPtr, from_module: jobject, package: jstring);
    pub fn JVM_AddReadsModule(env: JNIEnvPtr, from_module: jobject, source_module: jobject);
    pub fn JVM_DefineArchivedModules(env: JNIEnvPtr, platform_loader: jobject, system_loader: jobject);

    // Reflection
    pub fn JVM_InitClassName(env: JNIEnvPtr, cls: jclass) -> jstring;
    pub fn JVM_GetClassInterfaces(env: JNIEnvPtr, cls: jclass) -> jobjectArray;
    pub fn JVM_IsInterface(env: JNIEnvPtr, cls: jclass) -> Jboolean;
    pub fn JVM_IsHiddenClass(env: JNIEnvPtr, cls: jclass) -> Jboolean;
    pub fn JVM_GetDeclaredClasses(env: JNIEnvPtr, of_class: jclass) -> jobjectArray;
    pub fn JVM_GetDeclaringClass(env: JNIEnvPtr, of_class: jclass) -> jclass;
    pub fn JVM_GetSimpleBinaryName(env: JNIEnvPtr, of_class: jclass) -> jstring;
    pub fn JVM_GetClassSignature(env: JNIEnvPtr, cls: jclass) -> jstring;
    pub fn JVM_GetClassAnnotations(env: JNIEnvPtr, cls: jclass) -> jbyteArray;
    pub fn JVM_GetClassTypeAnnotations(env: JNIEnvPtr, cls: jclass) -> jbyteArray;
    pub fn JVM_GetFieldTypeAnnotations(env: JNIEnvPtr, field: jobject) -> jbyteArray;
    pub fn JVM_GetMethodTypeAnnotations(env: JNIEnvPtr, method: jobject) -> jbyteArray;
    pub fn JVM_GetClassDeclaredMethods(
        env: JNIEnvPtr,
        of_class: jclass,
        public_only: Jboolean,
    ) -> jobjectArray;
    pub fn JVM_GetClassDeclaredFields(
        env: JNIEnvPtr,
        of_class: jclass,
        public_only: Jboolean,
    ) -> jobjectArray;
    pub fn JVM_GetClassDeclaredConstructors(
        env: JNIEnvPtr,
        of_class: jclass,
        public_only: Jboolean,
    ) -> jobjectArray;
    pub fn JVM_GetClassAccessFlags(env: JNIEnvPtr, cls: jclass) -> Jint;
    pub fn JVM_AreNestMates(env: JNIEnvPtr, current: jclass, member: jclass) -> Jboolean;
    pub fn JVM_GetNestHost(env: JNIEnvPtr, current: jclass) -> jclass;
    pub fn JVM_GetNestMembers(env: JNIEnvPtr, current: jclass) -> jobjectArray;
    pub fn JVM_IsRecord(env: JNIEnvPtr, cls: jclass) -> Jboolean;
    pub fn JVM_GetRecordComponents(env: JNIEnvPtr, of_class: jclass) -> jobjectArray;
    pub fn JVM_GetPermittedSubclasses(env: JNIEnvPtr, current: jclass) -> jobjectArray;
    pub fn JVM_InvokeMethod(
        env: JNIEnvPtr,
        method: jobject,
        obj: jobject,
        args0: jobjectArray,
    ) -> jobject;
    pub fn JVM_NewInstanceFromConstructor(
        env: JNIEnvPtr,
        c: jobject,
        args0: jobjectArray,
    ) -> jobject;
    pub fn JVM_GetClassConstantPool(env: JNIEnvPtr, cls: jclass) -> jobject;

    // Constant pool
    pub fn JVM_ConstantPoolGetSize(env: JNIEnvPtr, unused: jobject, jcpool: jobject) -> Jint;
    pub fn JVM_ConstantPoolGetClassAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> jclass;
    pub fn JVM_ConstantPoolGetClassAtIfLoaded(
        env: JNIEnvPtr,
        unused: jobject,
        jcpool: jobject,
        index: Jint,
    ) -> jclass;
    pub fn JVM_ConstantPoolGetClassRefIndexAt(env: JNIEnvPtr, obj: jobject, unused: jobject, index: Jint) -> Jint;
    pub fn JVM_ConstantPoolGetMethodAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> jobject;
    pub fn JVM_ConstantPoolGetMethodAtIfLoaded(
        env: JNIEnvPtr,
        unused: jobject,
        jcpool: jobject,
        index: Jint,
    ) -> jobject;
    pub fn JVM_ConstantPoolGetFieldAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> jobject;
    pub fn JVM_ConstantPoolGetFieldAtIfLoaded(
        env: JNIEnvPtr,
        unused: jobject,
        jcpool: jobject,
        index: Jint,
    ) -> jobject;
    pub fn JVM_ConstantPoolGetMemberRefInfoAt(
        env: JNIEnvPtr,
        unused: jobject,
        jcpool: jobject,
        index: Jint,
    ) -> jobjectArray;
    pub fn JVM_ConstantPoolGetNameAndTypeRefIndexAt(
        env: JNIEnvPtr,
        obj: jobject,
        unused: jobject,
        index: Jint,
    ) -> Jint;
    pub fn JVM_ConstantPoolGetNameAndTypeRefInfoAt(
        env: JNIEnvPtr,
        obj: jobject,
        unused: jobject,
        index: Jint,
    ) -> jobjectArray;
    pub fn JVM_ConstantPoolGetIntAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> Jint;
    pub fn JVM_ConstantPoolGetLongAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> Jlong;
    pub fn JVM_ConstantPoolGetFloatAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> Jfloat;
    pub fn JVM_ConstantPoolGetDoubleAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> Jdouble;
    pub fn JVM_ConstantPoolGetStringAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> jstring;
    pub fn JVM_ConstantPoolGetUTF8At(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> jstring;
    pub fn JVM_ConstantPoolGetTagAt(env: JNIEnvPtr, unused: jobject, jcpool: jobject, index: Jint) -> Jbyte;

    // Parameter reflection
    pub fn JVM_GetMethodParameters(env: JNIEnvPtr, method: jobject) -> jobjectArray;
    pub fn JVM_EnsureMaterializedForStackWalk_func(
        env: JNIEnvPtr,
        vthread: jobject,
        value: jobject,
    );

    // Signals
    pub fn JVM_RegisterSignal(sig: Jint, handler: *mut c_void) -> *mut c_void;
    pub fn JVM_RaiseSignal(sig: Jint) -> Jboolean;
    pub fn JVM_FindSignal(name: *const c_char) -> Jint;

    // Assertions
    pub fn JVM_DesiredAssertionStatus(env: JNIEnvPtr, unused: jclass, cls: jclass) -> Jboolean;
    pub fn JVM_AssertionStatusDirectives(env: JNIEnvPtr, unused: jclass) -> jobject;

    // java.lang.ref.Finalizer
    pub fn JVM_ReportFinalizationComplete(env: JNIEnvPtr, finalizee: jobject);
    pub fn JVM_IsFinalizationEnabled(env: JNIEnvPtr) -> Jboolean;

    // Part 2: verifier / class file format support
    pub fn JVM_GetClassNameUTF(env: JNIEnvPtr, cb: jclass) -> *const c_char;
    pub fn JVM_GetClassCPTypes(env: JNIEnvPtr, cb: jclass, types: *mut Jbyte);
    pub fn JVM_GetClassCPEntriesCount(env: JNIEnvPtr, cb: jclass) -> Jint;
    pub fn JVM_GetClassFieldsCount(env: JNIEnvPtr, cb: jclass) -> Jint;
    pub fn JVM_GetClassMethodsCount(env: JNIEnvPtr, cb: jclass) -> Jint;
    pub fn JVM_GetMethodIxExceptionIndexes(
        env: JNIEnvPtr,
        cb: jclass,
        method_index: Jint,
        exceptions: *mut Jshort,
    );
    pub fn JVM_GetMethodIxExceptionsCount(env: JNIEnvPtr, cb: jclass, method_index: Jint) -> Jint;
    pub fn JVM_GetMethodIxByteCode(env: JNIEnvPtr, cb: jclass, method_index: Jint, code: *mut Jbyte);
    pub fn JVM_GetMethodIxByteCodeLength(env: JNIEnvPtr, cb: jclass, method_index: Jint) -> Jint;
    pub fn JVM_GetMethodIxExceptionTableEntry(
        env: JNIEnvPtr,
        cb: jclass,
        method_index: Jint,
        entry_index: Jint,
        entry: *mut JvmExceptionTableEntry,
    );
    pub fn JVM_GetMethodIxExceptionTableLength(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jint;
    pub fn JVM_GetFieldIxModifiers(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jint;
    pub fn JVM_GetMethodIxModifiers(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jint;
    pub fn JVM_GetMethodIxLocalsCount(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jint;
    pub fn JVM_GetMethodIxArgsSize(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jint;
    pub fn JVM_GetMethodIxMaxStack(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jint;
    pub fn JVM_IsConstructorIx(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jboolean;
    pub fn JVM_IsVMGeneratedMethodIx(env: JNIEnvPtr, cb: jclass, index: Jint) -> Jboolean;
    pub fn JVM_GetMethodIxNameUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetMethodIxSignatureUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPFieldNameUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPMethodNameUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPMethodSignatureUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPFieldSignatureUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPClassNameUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPFieldClassNameUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPMethodClassNameUTF(env: JNIEnvPtr, cb: jclass, index: Jint) -> *const c_char;
    pub fn JVM_GetCPFieldModifiers(env: JNIEnvPtr, cb: jclass, index: Jint, called_class: jclass) -> Jint;
    pub fn JVM_GetCPMethodModifiers(env: JNIEnvPtr, cb: jclass, index: Jint, called_class: jclass) -> Jint;
    pub fn JVM_ReleaseUTF(utf: *const c_char);
    pub fn JVM_IsSameClassPackage(env: JNIEnvPtr, class1: jclass, class2: jclass) -> Jboolean;

    // Part 3: I/O, network and VM management
    pub fn JVM_NativePath(path: *mut c_char) -> *mut c_char;
    pub fn JVM_RawMonitorCreate() -> *mut c_void;
    pub fn JVM_RawMonitorDestroy(mon: *mut c_void);
    pub fn JVM_RawMonitorEnter(mon: *mut c_void) -> Jint;
    pub fn JVM_RawMonitorExit(mon: *mut c_void);
    pub fn JVM_GetManagement(version: Jint) -> *mut c_void;
    pub fn JVM_InitAgentProperties(env: JNIEnvPtr, agent_props: jobject) -> jobject;
    pub fn JVM_GetTemporaryDirectory(env: JNIEnvPtr) -> jstring;
    pub fn JVM_GetEnclosingMethodInfo(env: JNIEnvPtr, of_class: jclass) -> jobjectArray;
    pub fn JVM_VirtualThreadStart(env: JNIEnvPtr, vthread: jobject);
    pub fn JVM_VirtualThreadEnd(env: JNIEnvPtr, vthread: jobject);
    pub fn JVM_VirtualThreadMount(env: JNIEnvPtr, vthread: jobject, hide: Jboolean);
    pub fn JVM_VirtualThreadUnmount(env: JNIEnvPtr, vthread: jobject, hide: Jboolean);
    pub fn JVM_VirtualThreadDisableSuspend(env: JNIEnvPtr, clazz: jclass, enter: Jboolean);
    pub fn JVM_VirtualThreadPinnedEvent(env: JNIEnvPtr, clazz: jclass, op: jstring);
    pub fn JVM_TakeVirtualThreadListToUnblock(env: JNIEnvPtr, ignored: jclass) -> jobject;
    pub fn JVM_GetClassFileVersion(env: JNIEnvPtr, current: jclass) -> Jint;
    pub fn JVM_PrintWarningAtDynamicAgentLoad() -> Jboolean;
}
