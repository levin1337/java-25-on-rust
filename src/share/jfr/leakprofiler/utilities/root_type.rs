

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RootType {
    Unknown,
    JavaFrame,
    NativeFrame,
    StackLocal,
    GlobalHandle,
    JniLocal,
    JniGlobal,
    ClassStatic,
    ThreadLocal,
    Synchronizer,
    SystemDictionary,
    CodeCache,
    Universe,
    CHeap,
    ConstantPool,
    ModuleEntry,
    ObjectSynchronizer,
    JvmtiEnv,
    JvmtiObjectTagging,
    Management,
    Jfr,
    Serviceability,
    Other,
}

impl RootType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Unknown,
            1 => Self::JavaFrame,
            2 => Self::NativeFrame,
            3 => Self::StackLocal,
            4 => Self::GlobalHandle,
            5 => Self::JniLocal,
            6 => Self::JniGlobal,
            7 => Self::ClassStatic,
            8 => Self::ThreadLocal,
            9 => Self::Synchronizer,
            10 => Self::SystemDictionary,
            11 => Self::CodeCache,
            12 => Self::Universe,
            13 => Self::CHeap,
            14 => Self::ConstantPool,
            15 => Self::ModuleEntry,
            16 => Self::ObjectSynchronizer,
            17 => Self::JvmtiEnv,
            18 => Self::JvmtiObjectTagging,
            19 => Self::Management,
            20 => Self::Jfr,
            21 => Self::Serviceability,
            _ => Self::Other,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::JavaFrame => 1,
            Self::NativeFrame => 2,
            Self::StackLocal => 3,
            Self::GlobalHandle => 4,
            Self::JniLocal => 5,
            Self::JniGlobal => 6,
            Self::ClassStatic => 7,
            Self::ThreadLocal => 8,
            Self::Synchronizer => 9,
            Self::SystemDictionary => 10,
            Self::CodeCache => 11,
            Self::Universe => 12,
            Self::CHeap => 13,
            Self::ConstantPool => 14,
            Self::ModuleEntry => 15,
            Self::ObjectSynchronizer => 16,
            Self::JvmtiEnv => 17,
            Self::JvmtiObjectTagging => 18,
            Self::Management => 19,
            Self::Jfr => 20,
            Self::Serviceability => 21,
            Self::Other => 22,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnifiedOopRef {
    Object(usize),
    ObjectArray(usize),
    TypeArray(usize),
    Null,
}

impl UnifiedOopRef {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

impl From<crate::share::interpreter::bytecodeInterpreter::Value<'_>> for UnifiedOopRef {
    fn from(value: crate::share::interpreter::bytecodeInterpreter::Value<'_>) -> Self {
        use crate::share::interpreter::bytecodeInterpreter::Value;
        match value {
            Value::Object(h) => Self::Object(h),
            Value::ObjectArray(h) => Self::ObjectArray(h),
            Value::PrimitiveArray(h) => Self::TypeArray(h),
            _ => Self::Null,
        }
    }
}
