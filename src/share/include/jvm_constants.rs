use crate::share::utilities::accessFlags::AccessFlags;

pub const JVM_RECOGNIZED_CLASS_MODIFIERS: u16 = AccessFlags::JVM_ACC_PUBLIC
    | AccessFlags::JVM_ACC_FINAL
    | AccessFlags::JVM_ACC_SUPER
    | AccessFlags::JVM_ACC_INTERFACE
    | AccessFlags::JVM_ACC_ABSTRACT
    | AccessFlags::JVM_ACC_ANNOTATION
    | AccessFlags::JVM_ACC_ENUM
    | AccessFlags::JVM_ACC_SYNTHETIC;

pub const JVM_RECOGNIZED_FIELD_MODIFIERS: u16 = AccessFlags::JVM_ACC_PUBLIC
    | AccessFlags::JVM_ACC_PRIVATE
    | AccessFlags::JVM_ACC_PROTECTED
    | AccessFlags::JVM_ACC_STATIC
    | AccessFlags::JVM_ACC_FINAL
    | AccessFlags::JVM_ACC_VOLATILE
    | AccessFlags::JVM_ACC_TRANSIENT
    | AccessFlags::JVM_ACC_ENUM
    | AccessFlags::JVM_ACC_SYNTHETIC;

pub const JVM_RECOGNIZED_METHOD_MODIFIERS: u16 = AccessFlags::JVM_ACC_PUBLIC
    | AccessFlags::JVM_ACC_PRIVATE
    | AccessFlags::JVM_ACC_PROTECTED
    | AccessFlags::JVM_ACC_STATIC
    | AccessFlags::JVM_ACC_FINAL
    | AccessFlags::JVM_ACC_SYNCHRONIZED
    | AccessFlags::JVM_ACC_BRIDGE
    | AccessFlags::JVM_ACC_VARARGS
    | AccessFlags::JVM_ACC_NATIVE
    | AccessFlags::JVM_ACC_ABSTRACT
    | AccessFlags::JVM_ACC_STRICT
    | AccessFlags::JVM_ACC_SYNTHETIC;

pub const JVM_CLASS_ACCESS_FLAGS_MASK: u16 = 0x1FFF;

pub const JVM_STACKWALK_CLASS_INFO_ONLY: i32 = 0x2;
pub const JVM_STACKWALK_SHOW_HIDDEN_FRAMES: i32 = 0x20;
pub const JVM_STACKWALK_FILL_LIVE_STACK_FRAMES: i32 = 0x100;

pub const JVM_SIGINT: i32 = 2;
pub const JVM_SIGTERM: i32 = 15;
