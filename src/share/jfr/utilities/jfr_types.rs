/*
 * OpenJDK 25 HotSpot jfr/utilities/jfrTypes.hpp port
 */

pub type TraceId = u8;
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
        let upper = match name {
            "VirtualSpace" => "VIRTUALSPACE",
            "ObjectSpace" => "OBJECTSPACE",
            "MetaspaceSizes" => "METASPACESIZES",
            "CopyFailed" => "COPYFAILED",
            "G1EvacuationStatistics" => "G1EVACUATIONSTATISTICS",
            "CalleeMethod" => "CALLEEMETHOD",
            "ShenandoahHeapRegionState" => "SHENANDOAHHEAPREGIONSTATE",
            "DeoptimizationReason" => "DEOPTIMIZATIONREASON",
            "DeoptimizationAction" => "DEOPTIMIZATIONACTION",
            "VMThreadState" => "VMTHREADSTATE",
            "Bytecode" => "BYTECODE",
            "CompilerType" => "COMPILERTYPE",
            "ZPageTypeType" => "ZPAGETYPETYPE",
            "ZStatisticsCounterType" => "ZSTATISTICSCOUNTERTYPE",
            "ZStatisticsSamplerType" => "ZSTATISTICSSAMPLERTYPE",
            "NetworkInterfaceName" => "NETWORKINTERFACENAME",
            "Thread" => "THREAD",
            "ThreadGroup" => "THREADGROUP",
            "Class" => "CLASS",
            "ClassLoader" => "CLASSLOADER",
            "Method" => "METHOD",
            "Symbol" => "SYMBOL",
            "ThreadState" => "THREADSTATE",
            "GCName" => "GCNAME",
            "GCCause" => "GCCAUSE",
            "GCWhen" => "GCWHEN",
            "G1HeapRegionType" => "G1HEAPREGIONTYPE",
            "G1YCType" => "G1YCTYPE",
            "GCThresholdUpdater" => "GCTHRESHOLDUPDATER",
            "ReferenceType" => "REFERENCETYPE",
            "MetadataType" => "METADATATYPE",
            "MetaspaceObjectType" => "METASPACEOBJECTTYPE",
            "NarrowOopMode" => "NARROWOOPMODE",
            "VMOperationType" => "VMOPERATIONTYPE",
            "CompilerPhaseType" => "COMPILERPHASETYPE",
            "FlagValueOrigin" => "FLAGVALUEORIGIN",
            "CodeBlobType" => "CODEBLOBTYPE",
            "InflateCause" => "INFLATECAUSE",
            "Module" => "MODULE",
            "Package" => "PACKAGE",
            "StackTrace" => "STACKTRACE",
            "FrameType" => "FRAMETYPE",
            "OldObjectRootSystem" => "OLDOBJECTROOTSYSTEM",
            "OldObjectRootType" => "OLDOBJECTROOTTYPE",
            "OldObjectGcRoot" => "OLDOBJECTGCROOT",
            "OldObjectArray" => "OLDOBJECTARRAY",
            "OldObjectField" => "OLDOBJECTFIELD",
            "OldObject" => "OLDOBJECT",
            "Reference" => "REFERENCE",
            "StackFrame" => "STACKFRAME",
            "ChunkHeader" => "CHUNKHEADER",
            "NMTType" => "NMTTYPE",
            "JavaMonitorAddress" => "JAVAMONITORADDRESS",
            "SafepointId" => "SAFEPOINTID",
            "GcId" => "GCID",
            "CompileId" => "COMPILEID",
            "SweepId" => "SWEEPID",
            "FlushId" => "FLUSHID",
            "ClassRedefinitionId" => "CLASSREDEFINITIONID",
            "long" => "LONG",
            "int" => "INT",
            "short" => "SHORT",
            "byte" => "BYTE",
            "double" => "DOUBLE",
            "float" => "FLOAT",
            "boolean" => "BOOLEAN",
            "char" => "CHAR",
            "string" => "STRING",
            _ => return None,
        };
        let _ = upper;
        // fallthrough handled by caller via const lookup
        None.or(match name {
            "VirtualSpace" => Some(JfrTypeId::VirtualSpace),
            "ObjectSpace" => Some(JfrTypeId::ObjectSpace),
            "MetaspaceSizes" => Some(JfrTypeId::MetaspaceSizes),
            "CopyFailed" => Some(JfrTypeId::CopyFailed),
            "G1EvacuationStatistics" => Some(JfrTypeId::G1EvacuationStatistics),
            "CalleeMethod" => Some(JfrTypeId::CalleeMethod),
            "ShenandoahHeapRegionState" => Some(JfrTypeId::ShenandoahHeapRegionState),
            "DeoptimizationReason" => Some(JfrTypeId::DeoptimizationReason),
            "DeoptimizationAction" => Some(JfrTypeId::DeoptimizationAction),
            "VMThreadState" => Some(JfrTypeId::VmThreadState),
            "Bytecode" => Some(JfrTypeId::Bytecode),
            "CompilerType" => Some(JfrTypeId::CompilerType),
            "ZPageTypeType" => Some(JfrTypeId::ZPageTypeType),
            "ZStatisticsCounterType" => Some(JfrTypeId::ZStatisticsCounterType),
            "ZStatisticsSamplerType" => Some(JfrTypeId::ZStatisticsSamplerType),
            "NetworkInterfaceName" => Some(JfrTypeId::NetworkInterfaceName),
            "Thread" => Some(JfrTypeId::Thread),
            "ThreadGroup" => Some(JfrTypeId::ThreadGroup),
            "Class" => Some(JfrTypeId::Class),
            "ClassLoader" => Some(JfrTypeId::ClassLoader),
            "Method" => Some(JfrTypeId::Method),
            "Symbol" => Some(JfrTypeId::Symbol),
            "ThreadState" => Some(JfrTypeId::ThreadState),
            "GCName" => Some(JfrTypeId::GcName),
            "GCCause" => Some(JfrTypeId::GcCause),
            "GCWhen" => Some(JfrTypeId::GcWhen),
            "G1HeapRegionType" => Some(JfrTypeId::G1HeapRegionType),
            "G1YCType" => Some(JfrTypeId::G1YcType),
            "GCThresholdUpdater" => Some(JfrTypeId::GcThresholdUpdater),
            "ReferenceType" => Some(JfrTypeId::ReferenceType),
            "MetadataType" => Some(JfrTypeId::MetadataType),
            "MetaspaceObjectType" => Some(JfrTypeId::MetaspaceObjectType),
            "NarrowOopMode" => Some(JfrTypeId::NarrowOopMode),
            "VMOperationType" => Some(JfrTypeId::VmOperationType),
            "CompilerPhaseType" => Some(JfrTypeId::CompilerPhaseType),
            "FlagValueOrigin" => Some(JfrTypeId::FlagValueOrigin),
            "CodeBlobType" => Some(JfrTypeId::CodeBlobType),
            "InflateCause" => Some(JfrTypeId::InflateCause),
            "Module" => Some(JfrTypeId::Module),
            "Package" => Some(JfrTypeId::Package),
            "StackTrace" => Some(JfrTypeId::StackTrace),
            "FrameType" => Some(JfrTypeId::FrameType),
            "OldObjectRootSystem" => Some(JfrTypeId::OldObjectRootSystem),
            "OldObjectRootType" => Some(JfrTypeId::OldObjectRootType),
            "OldObjectGcRoot" => Some(JfrTypeId::OldObjectGcRoot),
            "OldObjectArray" => Some(JfrTypeId::OldObjectArray),
            "OldObjectField" => Some(JfrTypeId::OldObjectField),
            "OldObject" => Some(JfrTypeId::OldObject),
            "Reference" => Some(JfrTypeId::Reference),
            "StackFrame" => Some(JfrTypeId::StackFrame),
            "ChunkHeader" => Some(JfrTypeId::ChunkHeader),
            "NMTType" => Some(JfrTypeId::NmtType),
            "JavaMonitorAddress" => Some(JfrTypeId::JavaMonitorAddress),
            "SafepointId" => Some(JfrTypeId::SafePointId),
            "GcId" => Some(JfrTypeId::GcId),
            "CompileId" => Some(JfrTypeId::CompileId),
            "SweepId" => Some(JfrTypeId::SweepId),
            "FlushId" => Some(JfrTypeId::FlushId),
            "ClassRedefinitionId" => Some(JfrTypeId::ClassRedefinitionId),
            "long" | "Tickspan" | "Ticks" => Some(JfrTypeId::Long),
            "int" | "uint" => Some(JfrTypeId::Int),
            "short" | "ushort" => Some(JfrTypeId::Short),
            "byte" | "ubyte" => Some(JfrTypeId::Byte),
            "double" => Some(JfrTypeId::Double),
            "float" => Some(JfrTypeId::Float),
            "boolean" => Some(JfrTypeId::Boolean),
            "char" => Some(JfrTypeId::Char),
            "string" => Some(JfrTypeId::String),
            _ => None,
        })
    }
}
