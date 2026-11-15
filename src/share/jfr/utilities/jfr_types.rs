/*
 * OpenJDK 25 HotSpot jfr/utilities/jfrTypes.hpp port
 */

pub type TraceId = u64;
pub type FioFd = i32;

pub const INVALID_FD: FioFd = -1;
pub const INVALID_OFFSET: i64 = -1;
pub const INVALID_TIME: i64 = -1;
pub const STACK_DEPTH_DEFAULT: u32 = 64;
pub const MIN_STACK_DEPTH: u32 = 1;
pub const MAX_STACK_DEPTH: u32 = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ReservedEvent {
    Metadata = 0,
    Checkpoint = 1,
}

pub const EVENT_METADATA: u64 = ReservedEvent::Metadata as u64;
pub const EVENT_CHECKPOINT: u64 = ReservedEvent::Checkpoint as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum EventStartTime {
    Untimed = 0,
    Timed = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum JfrCheckpointType {
    Generic = 0,
    Flush = 1,
    Header = 2,
    Statics = 4,
    Threads = 8,
}

impl JfrCheckpointType {
    pub fn bits(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JfrCheckpointContext {
    pub offset: i64,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum JfrCheckpointBufferKind {
    Global = 0,
    ThreadLocal = 1,
    VirtualThreadLocal = 2,
}

// JfrTypeId — generated equivalent of jfrfiles/jfrTypes.hpp (JDK 25).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u64)]
pub enum JfrTypeId {
    VirtualSpace = 173,
    ObjectSpace = 174,
    MetaspaceSizes = 175,
    CopyFailed = 176,
    G1EvacuationStatistics = 177,
    CalleeMethod = 178,
    ShenandoahHeapRegionState = 179,
    DeoptimizationReason = 180,
    DeoptimizationAction = 181,
    VmThreadState = 182,
    Bytecode = 183,
    CompilerType = 184,
    ZPageTypeType = 185,
    ZStatisticsCounterType = 186,
    ZStatisticsSamplerType = 187,
    NetworkInterfaceName = 188,
    Thread = 189,
    ThreadGroup = 190,
    Class = 191,
    ClassLoader = 192,
    Method = 193,
    Symbol = 194,
    ThreadState = 195,
    GcName = 196,
    GcCause = 197,
    GcWhen = 198,
    G1HeapRegionType = 199,
    G1YcType = 200,
    GcThresholdUpdater = 201,
    ReferenceType = 202,
    MetadataType = 203,
    MetaspaceObjectType = 204,
    NarrowOopMode = 205,
    VmOperationType = 206,
    CompilerPhaseType = 207,
    FlagValueOrigin = 208,
    CodeBlobType = 209,
    InflateCause = 210,
    Module = 211,
    Package = 212,
    StackTrace = 213,
    FrameType = 214,
    OldObjectRootSystem = 215,
    OldObjectRootType = 216,
    OldObjectGcRoot = 217,
    OldObjectArray = 218,
    OldObjectField = 219,
    OldObject = 220,
    Reference = 221,
    StackFrame = 222,
    ChunkHeader = 223,
    NmtType = 224,
    JavaMonitorAddress = 225,
    SafePointId = 226,
    GcId = 227,
    CompileId = 228,
    SweepId = 229,
    FlushId = 230,
    ClassRedefinitionId = 231,
    Long = 232,
    Int = 233,
    Short = 234,
    Byte = 235,
    Double = 236,
    Float = 237,
    Boolean = 238,
    Char = 239,
    String = 240,
}

impl JfrTypeId {
    pub const FIRST_TYPE_ID: JfrTypeId = JfrTypeId::VirtualSpace;
    pub const LAST_TYPE_ID: JfrTypeId = JfrTypeId::String;

    pub fn as_u64(self) -> u64 {
        self as u64
    }

    // JfrType::name_to_id
    pub fn name_to_id(type_name: &str) -> Option<JfrTypeId> {
        Some(match type_name {
            "java.lang.Class" => JfrTypeId::Class,
            "java.lang.Thread" => JfrTypeId::Thread,
            "long" => JfrTypeId::Long,
            "int" => JfrTypeId::Int,
            "short" => JfrTypeId::Short,
            "byte" => JfrTypeId::Byte,
            "double" => JfrTypeId::Double,
            "float" => JfrTypeId::Float,
            "boolean" => JfrTypeId::Boolean,
            "char" => JfrTypeId::Char,
            "java.lang.String" => JfrTypeId::String,
            "jdk.types.StackTrace" => JfrTypeId::StackTrace,
            _ => return None,
        })
    }

    // Resolve a metadata.xml field/type name to a JfrTypeId.
    // Constant pool and simple types map by name; everything else is an event type id.
    pub fn from_metadata_name(name: &str) -> Option<JfrTypeId> {
        Some(match name {
            "VirtualSpace" => JfrTypeId::VirtualSpace,
            "ObjectSpace" => JfrTypeId::ObjectSpace,
            "MetaspaceSizes" => JfrTypeId::MetaspaceSizes,
            "CopyFailed" => JfrTypeId::CopyFailed,
            "G1EvacuationStatistics" => JfrTypeId::G1EvacuationStatistics,
            "CalleeMethod" => JfrTypeId::CalleeMethod,
            "ShenandoahHeapRegionState" => JfrTypeId::ShenandoahHeapRegionState,
            "DeoptimizationReason" => JfrTypeId::DeoptimizationReason,
            "DeoptimizationAction" => JfrTypeId::DeoptimizationAction,
            "VMThreadState" => JfrTypeId::VmThreadState,
            "Bytecode" => JfrTypeId::Bytecode,
            "CompilerType" => JfrTypeId::CompilerType,
            "ZPageTypeType" => JfrTypeId::ZPageTypeType,
            "ZStatisticsCounterType" => JfrTypeId::ZStatisticsCounterType,
            "ZStatisticsSamplerType" => JfrTypeId::ZStatisticsSamplerType,
            "NetworkInterfaceName" => JfrTypeId::NetworkInterfaceName,
            "Thread" => JfrTypeId::Thread,
            "ThreadGroup" => JfrTypeId::ThreadGroup,
            "Class" => JfrTypeId::Class,
            "ClassLoader" => JfrTypeId::ClassLoader,
            "Method" => JfrTypeId::Method,
            "Symbol" => JfrTypeId::Symbol,
            "ThreadState" => JfrTypeId::ThreadState,
            "GCName" => JfrTypeId::GcName,
            "GCCause" => JfrTypeId::GcCause,
            "GCWhen" => JfrTypeId::GcWhen,
            "G1HeapRegionType" => JfrTypeId::G1HeapRegionType,
            "G1YCType" => JfrTypeId::G1YcType,
            "GCThresholdUpdater" => JfrTypeId::GcThresholdUpdater,
            "ReferenceType" => JfrTypeId::ReferenceType,
            "MetadataType" => JfrTypeId::MetadataType,
            "MetaspaceObjectType" => JfrTypeId::MetaspaceObjectType,
            "NarrowOopMode" => JfrTypeId::NarrowOopMode,
            "VMOperationType" => JfrTypeId::VmOperationType,
            "CompilerPhaseType" => JfrTypeId::CompilerPhaseType,
            "FlagValueOrigin" => JfrTypeId::FlagValueOrigin,
            "CodeBlobType" => JfrTypeId::CodeBlobType,
            "InflateCause" => JfrTypeId::InflateCause,
            "Module" => JfrTypeId::Module,
            "Package" => JfrTypeId::Package,
            "StackTrace" => JfrTypeId::StackTrace,
            "FrameType" => JfrTypeId::FrameType,
            "OldObjectRootSystem" => JfrTypeId::OldObjectRootSystem,
            "OldObjectRootType" => JfrTypeId::OldObjectRootType,
            "OldObjectGcRoot" => JfrTypeId::OldObjectGcRoot,
            "OldObjectArray" => JfrTypeId::OldObjectArray,
            "OldObjectField" => JfrTypeId::OldObjectField,
            "OldObject" => JfrTypeId::OldObject,
            "Reference" => JfrTypeId::Reference,
            "StackFrame" => JfrTypeId::StackFrame,
            "ChunkHeader" => JfrTypeId::ChunkHeader,
            "NMTType" => JfrTypeId::NmtType,
            "JavaMonitorAddress" => JfrTypeId::JavaMonitorAddress,
            "SafepointId" => JfrTypeId::SafePointId,
            "GcId" => JfrTypeId::GcId,
            "CompileId" => JfrTypeId::CompileId,
            "SweepId" => JfrTypeId::SweepId,
            "FlushId" => JfrTypeId::FlushId,
            "ClassRedefinitionId" => JfrTypeId::ClassRedefinitionId,
            "long" | "Tickspan" | "Ticks" => JfrTypeId::Long,
            "int" | "uint" => JfrTypeId::Int,
            "short" | "ushort" => JfrTypeId::Short,
            "byte" | "ubyte" => JfrTypeId::Byte,
            "double" => JfrTypeId::Double,
            "float" => JfrTypeId::Float,
            "boolean" => JfrTypeId::Boolean,
            "char" => JfrTypeId::Char,
            "string" => JfrTypeId::String,
            _ => return None,
        })
    }
}


// JfrEventId — numeric event id (u16 space, generated by events.xml).
pub type JfrEventId = u16;

// Generated event name -> id table (subset matching the key events and
// framework events ported so far; extend as events are implemented).
pub const EVENT_NAME_TO_ID: &[(&str, JfrEventId)] = &[
    ("jdk.Metadata", 0),
    ("jdk.Checkpoint", 1),
    ("jdk.ThreadStart", 56),
    ("jdk.ThreadEnd", 57),
    ("jdk.JavaMonitorEnter", 63),
    ("jdk.ClassLoadingStatistics", 67),
    ("jdk.ExecutionSample", 69),
    ("jdk.ThreadCPULoad", 78),
    ("jdk.GCPhasePause", 30),
    ("jdk.GCPhaseConcurrent", 31),
];

impl JfrTypeId {
    // All type ids in declaration order.
    pub fn all() -> Vec<JfrTypeId> {
        vec![
            JfrTypeId::VirtualSpace, JfrTypeId::ObjectSpace, JfrTypeId::MetaspaceSizes,
            JfrTypeId::CopyFailed, JfrTypeId::G1EvacuationStatistics, JfrTypeId::CalleeMethod,
            JfrTypeId::ShenandoahHeapRegionState, JfrTypeId::DeoptimizationReason,
            JfrTypeId::DeoptimizationAction, JfrTypeId::VmThreadState, JfrTypeId::Bytecode,
            JfrTypeId::CompilerType, JfrTypeId::ZPageTypeType, JfrTypeId::ZStatisticsCounterType,
            JfrTypeId::ZStatisticsSamplerType, JfrTypeId::NetworkInterfaceName, JfrTypeId::Thread,
            JfrTypeId::ThreadGroup, JfrTypeId::Class, JfrTypeId::ClassLoader, JfrTypeId::Method,
            JfrTypeId::Symbol, JfrTypeId::ThreadState, JfrTypeId::GcName, JfrTypeId::GcCause,
            JfrTypeId::GcWhen, JfrTypeId::G1HeapRegionType, JfrTypeId::G1YcType,
            JfrTypeId::GcThresholdUpdater, JfrTypeId::ReferenceType, JfrTypeId::MetadataType,
            JfrTypeId::MetaspaceObjectType, JfrTypeId::NarrowOopMode, JfrTypeId::VmOperationType,
            JfrTypeId::CompilerPhaseType, JfrTypeId::FlagValueOrigin, JfrTypeId::CodeBlobType,
            JfrTypeId::InflateCause, JfrTypeId::Module, JfrTypeId::Package, JfrTypeId::StackTrace,
            JfrTypeId::FrameType, JfrTypeId::OldObjectRootSystem, JfrTypeId::OldObjectRootType,
            JfrTypeId::OldObjectGcRoot, JfrTypeId::OldObjectArray, JfrTypeId::OldObjectField,
            JfrTypeId::OldObject, JfrTypeId::Reference, JfrTypeId::StackFrame, JfrTypeId::ChunkHeader,
            JfrTypeId::NmtType, JfrTypeId::JavaMonitorAddress, JfrTypeId::SafePointId,
            JfrTypeId::GcId, JfrTypeId::CompileId, JfrTypeId::SweepId, JfrTypeId::FlushId,
            JfrTypeId::ClassRedefinitionId, JfrTypeId::Long, JfrTypeId::Int, JfrTypeId::Short,
            JfrTypeId::Byte, JfrTypeId::Double, JfrTypeId::Float, JfrTypeId::Boolean,
            JfrTypeId::Char, JfrTypeId::String,
        ]
    }

    pub fn metadata_name(self) -> &'static str {
        match self {
            JfrTypeId::VirtualSpace => "VirtualSpace",
            JfrTypeId::ObjectSpace => "ObjectSpace",
            JfrTypeId::MetaspaceSizes => "MetaspaceSizes",
            JfrTypeId::CopyFailed => "CopyFailed",
            JfrTypeId::G1EvacuationStatistics => "G1EvacuationStatistics",
            JfrTypeId::CalleeMethod => "CalleeMethod",
            JfrTypeId::ShenandoahHeapRegionState => "ShenandoahHeapRegionState",
            JfrTypeId::DeoptimizationReason => "DeoptimizationReason",
            JfrTypeId::DeoptimizationAction => "DeoptimizationAction",
            JfrTypeId::VmThreadState => "VMThreadState",
            JfrTypeId::Bytecode => "Bytecode",
            JfrTypeId::CompilerType => "CompilerType",
            JfrTypeId::ZPageTypeType => "ZPageTypeType",
            JfrTypeId::ZStatisticsCounterType => "ZStatisticsCounterType",
            JfrTypeId::ZStatisticsSamplerType => "ZStatisticsSamplerType",
            JfrTypeId::NetworkInterfaceName => "NetworkInterfaceName",
            JfrTypeId::Thread => "Thread",
            JfrTypeId::ThreadGroup => "ThreadGroup",
            JfrTypeId::Class => "Class",
            JfrTypeId::ClassLoader => "ClassLoader",
            JfrTypeId::Method => "Method",
            JfrTypeId::Symbol => "Symbol",
            JfrTypeId::ThreadState => "ThreadState",
            JfrTypeId::GcName => "GCName",
            JfrTypeId::GcCause => "GCCause",
            JfrTypeId::GcWhen => "GCWhen",
            JfrTypeId::G1HeapRegionType => "G1HeapRegionType",
            JfrTypeId::G1YcType => "G1YCType",
            JfrTypeId::GcThresholdUpdater => "GCThresholdUpdater",
            JfrTypeId::ReferenceType => "ReferenceType",
            JfrTypeId::MetadataType => "MetadataType",
            JfrTypeId::MetaspaceObjectType => "MetaspaceObjectType",
            JfrTypeId::NarrowOopMode => "NarrowOopMode",
            JfrTypeId::VmOperationType => "VMOperationType",
            JfrTypeId::CompilerPhaseType => "CompilerPhaseType",
            JfrTypeId::FlagValueOrigin => "FlagValueOrigin",
            JfrTypeId::CodeBlobType => "CodeBlobType",
            JfrTypeId::InflateCause => "InflateCause",
            JfrTypeId::Module => "Module",
            JfrTypeId::Package => "Package",
            JfrTypeId::StackTrace => "StackTrace",
            JfrTypeId::FrameType => "FrameType",
            JfrTypeId::OldObjectRootSystem => "OldObjectRootSystem",
            JfrTypeId::OldObjectRootType => "OldObjectRootType",
            JfrTypeId::OldObjectGcRoot => "OldObjectGcRoot",
            JfrTypeId::OldObjectArray => "OldObjectArray",
            JfrTypeId::OldObjectField => "OldObjectField",
            JfrTypeId::OldObject => "OldObject",
            JfrTypeId::Reference => "Reference",
            JfrTypeId::StackFrame => "StackFrame",
            JfrTypeId::ChunkHeader => "ChunkHeader",
            JfrTypeId::NmtType => "NMTType",
            JfrTypeId::JavaMonitorAddress => "JavaMonitorAddress",
            JfrTypeId::SafePointId => "SafepointId",
            JfrTypeId::GcId => "GcId",
            JfrTypeId::CompileId => "CompileId",
            JfrTypeId::SweepId => "SweepId",
            JfrTypeId::FlushId => "FlushId",
            JfrTypeId::ClassRedefinitionId => "ClassRedefinitionId",
            JfrTypeId::Long => "long",
            JfrTypeId::Int => "int",
            JfrTypeId::Short => "short",
            JfrTypeId::Byte => "byte",
            JfrTypeId::Double => "double",
            JfrTypeId::Float => "float",
            JfrTypeId::Boolean => "boolean",
            JfrTypeId::Char => "char",
            JfrTypeId::String => "string",
        }
    }
}

/// Number of JFR worker threads hint (JFR gets one per CPU, capped).
pub fn thread_count_hint() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// VM-initialized flag (Jfr::is_vm_initialized analogue): set once the
/// runtime has completed VM init; JFR recorder service waits on it.
static VM_INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn vm_initialized() -> bool {
    VM_INITIALIZED.load(std::sync::atomic::Ordering::Acquire)
}

pub fn set_vm_initialized(v: bool) {
    VM_INITIALIZED.store(v, std::sync::atomic::Ordering::Release);
}
