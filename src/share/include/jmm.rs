use super::jni::{
    JNIEnvPtr, JValue, Jboolean, Jint, Jlong, jstring, jlongArray, jobject, jobjectArray,
};
use std::ffi::{c_char, c_void};

pub const JMM_VERSION_1: i32 = 0x20010000;
pub const JMM_VERSION_1_0: i32 = 0x20010000;
pub const JMM_VERSION_1_1: i32 = 0x20010100;
pub const JMM_VERSION_1_2: i32 = 0x20010200;
pub const JMM_VERSION_1_2_1: i32 = 0x20010201;
pub const JMM_VERSION_1_2_2: i32 = 0x20010202;
pub const JMM_VERSION_2: i32 = 0x20020000;
pub const JMM_VERSION_3: i32 = 0x20030000;
pub const JMM_VERSION_4: i32 = 0x20040000;
pub const JMM_VERSION: i32 = JMM_VERSION_4;

pub const OPTIONAL_SUPPORT_LOW_MEMORY_DETECTION: u32 = 1 << 0;
pub const OPTIONAL_SUPPORT_COMPILATION_TIME_MONITORING: u32 = 1 << 1;
pub const OPTIONAL_SUPPORT_THREAD_CONTENTION_MONITORING: u32 = 1 << 2;
pub const OPTIONAL_SUPPORT_CURRENT_THREAD_CPU_TIME: u32 = 1 << 3;
pub const OPTIONAL_SUPPORT_OTHER_THREAD_CPU_TIME: u32 = 1 << 4;
pub const OPTIONAL_SUPPORT_OBJECT_MONITOR_USAGE: u32 = 1 << 5;
pub const OPTIONAL_SUPPORT_SYNCHRONIZER_USAGE: u32 = 1 << 6;
pub const OPTIONAL_SUPPORT_THREAD_ALLOCATED_MEMORY: u32 = 1 << 7;
pub const OPTIONAL_SUPPORT_REMOTE_DIAGNOSTIC_COMMANDS: u32 = 1 << 8;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JmmOptionalSupport {
    pub bits: u32,
}

impl JmmOptionalSupport {
    pub fn is_supported(&self, flag: u32) -> bool {
        self.bits & flag != 0
    }

    pub fn is_low_memory_detection_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_LOW_MEMORY_DETECTION)
    }

    pub fn is_compilation_time_monitoring_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_COMPILATION_TIME_MONITORING)
    }

    pub fn is_thread_contention_monitoring_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_THREAD_CONTENTION_MONITORING)
    }

    pub fn is_current_thread_cpu_time_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_CURRENT_THREAD_CPU_TIME)
    }

    pub fn is_other_thread_cpu_time_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_OTHER_THREAD_CPU_TIME)
    }

    pub fn is_object_monitor_usage_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_OBJECT_MONITOR_USAGE)
    }

    pub fn is_synchronizer_usage_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_SYNCHRONIZER_USAGE)
    }

    pub fn is_thread_allocated_memory_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_THREAD_ALLOCATED_MEMORY)
    }

    pub fn is_remote_diagnostic_commands_supported(&self) -> bool {
        self.is_supported(OPTIONAL_SUPPORT_REMOTE_DIAGNOSTIC_COMMANDS)
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JmmLongAttribute {
    ClassLoadedCount = 1,
    ClassUnloadedCount = 2,
    ThreadTotalCount = 3,
    ThreadLiveCount = 4,
    ThreadPeakCount = 5,
    ThreadDaemonCount = 6,
    JvmInitDoneTimeMs = 7,
    CompileTotalTimeMs = 8,
    GcTimeMs = 9,
    GcCount = 10,
    JvmUptimeMs = 11,
    ClassLoadedBytes = 101,
    ClassUnloadedBytes = 102,
    TotalClassloadTimeMs = 103,
    VmGlobalCount = 104,
    SafepointCount = 105,
    TotalSafepointSyncTimeMs = 106,
    TotalStoppedTimeMs = 107,
    TotalAppTimeMs = 108,
    VmThreadCount = 109,
    ClassInitTotalCount = 110,
    ClassInitTotalTimeMs = 111,
    MethodDataSizeBytes = 112,
    ClassVerifyTotalTimeMs = 113,
    SharedClassLoadedCount = 114,
    SharedClassUnloadedCount = 115,
    SharedClassLoadedBytes = 116,
    SharedClassUnloadedBytes = 117,
    OsProcessId = 201,
    OsMemTotalPhysicalBytes = 202,
    GcExtAttributeInfoSize = 401,
}

impl JmmLongAttribute {
    pub const INTERNAL_ATTRIBUTE_INDEX: i32 = 100;
    pub const OS_ATTRIBUTE_INDEX: i32 = 200;
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JmmBoolAttribute {
    VerboseGc = 21,
    VerboseClass = 22,
    ThreadContentionMonitoring = 23,
    ThreadCpuTime = 24,
    ThreadAllocatedMemory = 25,
}

pub const JMM_THREAD_STATE_FLAG_SUSPENDED: u32 = 0x00100000;
pub const JMM_THREAD_STATE_FLAG_NATIVE: u32 = 0x00400000;
pub const JMM_THREAD_STATE_FLAG_MASK: u32 = 0xFFF00000;

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JmmStatisticType {
    PeakThreadCount = 801,
    ThreadContentionCount = 802,
    ThreadContentionTime = 803,
    ThreadContentionStat = 804,
    PeakPoolUsage = 805,
    GcStat = 806,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JmmThresholdType {
    UsageThresholdHigh = 901,
    UsageThresholdLow = 902,
    CollectionUsageThresholdHigh = 903,
    CollectionUsageThresholdLow = 904,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JmmVmGlobalType {
    Unknown = 0,
    Jboolean = 1,
    jstring = 2,
    Jlong = 3,
    Jdouble = 4,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JmmVmGlobalOrigin {
    Default = 1,
    CommandLine = 2,
    Management = 3,
    EnvironVar = 4,
    ConfigFile = 5,
    Ergonomic = 6,
    AttachOnDemand = 7,
    Other = 99,
}

pub const VM_GLOBAL_FLAG_WRITEABLE: u32 = 1 << 0;
pub const VM_GLOBAL_FLAG_EXTERNAL: u32 = 1 << 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct JmmVmGlobal {
    pub name: jstring,
    pub value: JValue,
    pub vm_global_type: JmmVmGlobalType,
    pub origin: JmmVmGlobalOrigin,
    pub flags: u32,
    pub reserved1: *mut c_void,
    pub reserved2: *mut c_void,
}

impl JmmVmGlobal {
    pub fn is_writeable(&self) -> bool {
        self.flags & VM_GLOBAL_FLAG_WRITEABLE != 0
    }

    pub fn is_external(&self) -> bool {
        self.flags & VM_GLOBAL_FLAG_EXTERNAL != 0
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct JmmExtAttributeInfo {
    pub name: *const c_char,
    pub type_: u8,
    pub description: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct JmmGcStat {
    pub gc_index: Jlong,
    pub start_time: Jlong,
    pub end_time: Jlong,
    pub usage_before_gc: jobjectArray,
    pub usage_after_gc: jobjectArray,
    pub gc_ext_attribute_values_size: Jint,
    pub gc_ext_attribute_values: *mut JValue,
    pub num_gc_ext_attributes: Jint,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DcmdInfo {
    pub name: *const c_char,
    pub description: *const c_char,
    pub impact: *const c_char,
    pub permission_class: *const c_char,
    pub permission_name: *const c_char,
    pub permission_action: *const c_char,
    pub num_arguments: Jint,
    pub enabled: Jboolean,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DcmdArgInfo {
    pub name: *const c_char,
    pub description: *const c_char,
    pub type_: *const c_char,
    pub default_string: *const c_char,
    pub mandatory: Jboolean,
    pub option: Jboolean,
    pub multiple: Jboolean,
    pub position: Jint,
}

type GetVersionFn = unsafe extern "C" fn(env: JNIEnvPtr) -> Jint;
type GetOptionalSupportFn =
    unsafe extern "C" fn(env: JNIEnvPtr, support: *mut JmmOptionalSupport) -> Jint;
type GetThreadInfoFn =
    unsafe extern "C" fn(env: JNIEnvPtr, ids: jlongArray, max_depth: Jint, info: jobjectArray) -> Jint;
type GetMemoryPoolsFn = unsafe extern "C" fn(env: JNIEnvPtr, mgr: jobject) -> jobjectArray;
type GetMemoryManagersFn = unsafe extern "C" fn(env: JNIEnvPtr, pool: jobject) -> jobjectArray;
type GetMemoryPoolUsageFn = unsafe extern "C" fn(env: JNIEnvPtr, pool: jobject) -> jobject;
type GetPeakMemoryPoolUsageFn = unsafe extern "C" fn(env: JNIEnvPtr, pool: jobject) -> jobject;
type GetTotalThreadAllocatedMemoryFn = unsafe extern "C" fn(env: JNIEnvPtr) -> Jlong;
type GetOneThreadAllocatedMemoryFn = unsafe extern "C" fn(env: JNIEnvPtr, thread_id: Jlong) -> Jlong;
type GetThreadAllocatedMemoryFn =
    unsafe extern "C" fn(env: JNIEnvPtr, ids: jlongArray, sizes: jlongArray);
type GetMemoryUsageFn = unsafe extern "C" fn(env: JNIEnvPtr, heap: Jboolean) -> jobject;
type GetLongAttributeFn =
    unsafe extern "C" fn(env: JNIEnvPtr, obj: jobject, att: JmmLongAttribute) -> Jlong;
type GetBoolAttributeFn =
    unsafe extern "C" fn(env: JNIEnvPtr, att: JmmBoolAttribute) -> Jboolean;
type SetBoolAttributeFn =
    unsafe extern "C" fn(env: JNIEnvPtr, att: JmmBoolAttribute, flag: Jboolean) -> Jboolean;
type GetLongAttributesFn = unsafe extern "C" fn(
    env: JNIEnvPtr,
    obj: jobject,
    atts: *const JmmLongAttribute,
    count: Jint,
    result: *mut Jlong,
) -> Jint;
type FindCircularBlockedThreadsFn = unsafe extern "C" fn(env: JNIEnvPtr) -> jobjectArray;
type GetThreadCpuTimeFn = unsafe extern "C" fn(env: JNIEnvPtr, thread_id: Jlong) -> Jlong;
type GetVmGlobalNamesFn = unsafe extern "C" fn(env: JNIEnvPtr) -> jobjectArray;
type GetVmGlobalsFn = unsafe extern "C" fn(
    env: JNIEnvPtr,
    names: jobjectArray,
    globals: *mut JmmVmGlobal,
    count: Jint,
) -> Jint;
type GetInternalThreadTimesFn =
    unsafe extern "C" fn(env: JNIEnvPtr, names: jobjectArray, times: jlongArray) -> Jint;
type ResetStatisticFn =
    unsafe extern "C" fn(env: JNIEnvPtr, obj: JValue, stat_type: JmmStatisticType) -> Jboolean;
type SetPoolSensorFn =
    unsafe extern "C" fn(env: JNIEnvPtr, pool: jobject, threshold: JmmThresholdType, sensor: jobject);
type SetPoolThresholdFn = unsafe extern "C" fn(
    env: JNIEnvPtr,
    pool: jobject,
    threshold: JmmThresholdType,
    value: Jlong,
) -> Jlong;
type GetPoolCollectionUsageFn = unsafe extern "C" fn(env: JNIEnvPtr, pool: jobject) -> jobject;
type GetGcExtAttributeInfoFn = unsafe extern "C" fn(
    env: JNIEnvPtr,
    mgr: jobject,
    ext_info: *mut JmmExtAttributeInfo,
    count: Jint,
) -> Jint;
type GetLastGcStatFn = unsafe extern "C" fn(env: JNIEnvPtr, mgr: jobject, gc_stat: *mut JmmGcStat);
type GetThreadCpuTimeWithKindFn =
    unsafe extern "C" fn(env: JNIEnvPtr, thread_id: Jlong, user_sys: Jboolean) -> Jlong;
type GetThreadCpuTimesWithKindFn =
    unsafe extern "C" fn(env: JNIEnvPtr, ids: jlongArray, times: jlongArray, user_sys: Jboolean);
type DumpHeap0Fn =
    unsafe extern "C" fn(env: JNIEnvPtr, output_file: jstring, live: Jboolean) -> Jint;
type FindDeadlocksFn =
    unsafe extern "C" fn(env: JNIEnvPtr, object_monitors_only: Jboolean) -> jobjectArray;
type SetVmGlobalFn =
    unsafe extern "C" fn(env: JNIEnvPtr, flag_name: jstring, new_value: JValue);
type DumpThreadsFn = unsafe extern "C" fn(
    env: JNIEnvPtr,
    ids: jlongArray,
    locked_monitors: Jboolean,
    locked_synchronizers: Jboolean,
    max_depth: Jint,
) -> jobjectArray;
type SetGcNotificationEnabledFn =
    unsafe extern "C" fn(env: JNIEnvPtr, mgr: jobject, enabled: Jboolean);
type GetDiagnosticCommandsFn = unsafe extern "C" fn(env: JNIEnvPtr) -> jobjectArray;
type GetDiagnosticCommandInfoFn = unsafe extern "C" fn(
    env: JNIEnvPtr,
    cmds: jobjectArray,
    info: *mut DcmdInfo,
);
type GetDiagnosticCommandArgumentsInfoFn = unsafe extern "C" fn(
    env: JNIEnvPtr,
    command_name: jstring,
    info: *mut DcmdArgInfo,
    count: Jint,
);
type ExecuteDiagnosticCommandFn =
    unsafe extern "C" fn(env: JNIEnvPtr, command: jstring) -> jstring;
type SetDiagnosticFrameworkNotificationEnabledFn =
    unsafe extern "C" fn(env: JNIEnvPtr, enabled: Jboolean);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct JmmInterface {
    pub reserved1: *mut c_void,
    pub reserved2: *mut c_void,
    pub get_version: Option<GetVersionFn>,
    pub get_optional_support: Option<GetOptionalSupportFn>,
    pub get_thread_info: Option<GetThreadInfoFn>,
    pub get_memory_pools: Option<GetMemoryPoolsFn>,
    pub get_memory_managers: Option<GetMemoryManagersFn>,
    pub get_memory_pool_usage: Option<GetMemoryPoolUsageFn>,
    pub get_peak_memory_pool_usage: Option<GetPeakMemoryPoolUsageFn>,
    pub get_total_thread_allocated_memory: Option<GetTotalThreadAllocatedMemoryFn>,
    pub get_one_thread_allocated_memory: Option<GetOneThreadAllocatedMemoryFn>,
    pub get_thread_allocated_memory: Option<GetThreadAllocatedMemoryFn>,
    pub get_memory_usage: Option<GetMemoryUsageFn>,
    pub get_long_attribute: Option<GetLongAttributeFn>,
    pub get_bool_attribute: Option<GetBoolAttributeFn>,
    pub set_bool_attribute: Option<SetBoolAttributeFn>,
    pub get_long_attributes: Option<GetLongAttributesFn>,
    pub find_circular_blocked_threads: Option<FindCircularBlockedThreadsFn>,
    pub get_thread_cpu_time: Option<GetThreadCpuTimeFn>,
    pub get_vm_global_names: Option<GetVmGlobalNamesFn>,
    pub get_vm_globals: Option<GetVmGlobalsFn>,
    pub get_internal_thread_times: Option<GetInternalThreadTimesFn>,
    pub reset_statistic: Option<ResetStatisticFn>,
    pub set_pool_sensor: Option<SetPoolSensorFn>,
    pub set_pool_threshold: Option<SetPoolThresholdFn>,
    pub get_pool_collection_usage: Option<GetPoolCollectionUsageFn>,
    pub get_gc_ext_attribute_info: Option<GetGcExtAttributeInfoFn>,
    pub get_last_gc_stat: Option<GetLastGcStatFn>,
    pub get_thread_cpu_time_with_kind: Option<GetThreadCpuTimeWithKindFn>,
    pub get_thread_cpu_times_with_kind: Option<GetThreadCpuTimesWithKindFn>,
    pub dump_heap0: Option<DumpHeap0Fn>,
    pub find_deadlocks: Option<FindDeadlocksFn>,
    pub set_vm_global: Option<SetVmGlobalFn>,
    pub reserved6: *mut c_void,
    pub dump_threads: Option<DumpThreadsFn>,
    pub set_gc_notification_enabled: Option<SetGcNotificationEnabledFn>,
    pub get_diagnostic_commands: Option<GetDiagnosticCommandsFn>,
    pub get_diagnostic_command_info: Option<GetDiagnosticCommandInfoFn>,
    pub get_diagnostic_command_arguments_info: Option<GetDiagnosticCommandArgumentsInfoFn>,
    pub execute_diagnostic_command: Option<ExecuteDiagnosticCommandFn>,
    pub set_diagnostic_framework_notification_enabled:
        Option<SetDiagnosticFrameworkNotificationEnabledFn>,
}
