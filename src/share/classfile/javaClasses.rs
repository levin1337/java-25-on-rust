/*
 * Port of OpenJDK 25 HotSpot javaClasses (share/classfile/javaClasses.hpp, javaClasses.cpp)
 *
 * javaClasses manages field offsets and metadata accessors for well-known
 * core Java runtime classes:
 *   - java.lang.Object
 *   - java.lang.String
 *   - java.lang.Class
 *   - java.lang.Thread & ThreadGroup & VirtualThread
 *   - java.lang.Throwable & StackTraceElement
 *   - Boxing classes (Boolean, Byte, Character, Short, Integer, Long, Float, Double)
 *   - Reflection classes (Method, Constructor, Field, Parameter)
 *   - java.lang.Module, ClassLoader, System
 *   - java.lang.invoke (MethodHandle, MemberName, MethodType, CallSite, LambdaForm)
 *   - java.lang.ref (Reference, SoftReference)
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackTraceElement {
    pub declaring_class: String,
    pub method_name: String,
    pub file_name: Option<String>,
    pub line_number: i32,
}

pub type java_lang_StackTraceElement = StackTraceElement;

// java.lang.Object
pub struct JavaLangObject;
pub type java_lang_Object = JavaLangObject;

impl JavaLangObject {
    pub fn register_natives() {
        // HotSpot VM native registration marker
    }
}

// java.lang.String
pub struct JavaLangString;
pub type java_lang_String = JavaLangString;

impl JavaLangString {
    pub const CODER_LATIN1: u8 = 0;
    pub const CODER_UTF16: u8 = 1;

    pub const VALUE_OFFSET: usize = 0;
    pub const CODER_OFFSET: usize = 1;
    pub const HASH_OFFSET: usize = 2;
    pub const FLAGS_OFFSET: usize = 3;

    pub fn is_latin1(coder: u8) -> bool {
        coder == Self::CODER_LATIN1
    }

    pub fn hash_code(bytes: &[u8]) -> i32 {
        let mut h: i32 = 0;
        for &b in bytes {
            h = (31i32).wrapping_mul(h).wrapping_add(b as i32);
        }
        h
    }

    pub fn utf8_length(string: &str) -> usize {
        string.len()
    }

    pub fn create_from_str(s: &str) -> String {
        s.to_string()
    }

    pub fn as_utf8_string(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }
}

// java.lang.Class
pub struct JavaLangClass;
pub type java_lang_Class = JavaLangClass;

impl JavaLangClass {
    pub const KLASS_OFFSET: usize = 0;
    pub const MODULE_OFFSET: usize = 1;
    pub const NAME_OFFSET: usize = 2;
    pub const ARRAY_KLASS_OFFSET: usize = 3;

    pub fn as_external_name(name: &str) -> String {
        name.replace('/', ".")
    }

    pub fn as_internal_name(name: &str) -> String {
        name.replace('.', "/")
    }
}

// java.lang.Thread
pub struct JavaLangThread;
pub type java_lang_Thread = JavaLangThread;

impl JavaLangThread {
    pub const MIN_PRIORITY: i32 = 1;
    pub const NORM_PRIORITY: i32 = 5;
    pub const MAX_PRIORITY: i32 = 10;

    pub const NAME_OFFSET: usize = 0;
    pub const PRIORITY_OFFSET: usize = 1;
    pub const DAEMON_OFFSET: usize = 2;
    pub const EETOP_OFFSET: usize = 3;
    pub const THREAD_STATUS_OFFSET: usize = 4;
}

// java.lang.Thread.FieldHolder
pub struct JavaLangThreadFieldHolder;
pub type java_lang_Thread_FieldHolder = JavaLangThreadFieldHolder;

impl JavaLangThreadFieldHolder {
    pub const THREAD_STATUS_OFFSET: usize = 0;
    pub const PRIORITY_OFFSET: usize = 1;
    pub const DAEMON_OFFSET: usize = 2;
}

// java.lang.Thread.Constants
pub struct JavaLangThreadConstants;
pub type java_lang_Thread_Constants = JavaLangThreadConstants;

impl JavaLangThreadConstants {
    pub const RUNNABLE: i32 = 1;
    pub const TIMED_WAITING: i32 = 2;
    pub const WAITING: i32 = 3;
    pub const BLOCKED: i32 = 4;
    pub const TERMINATED: i32 = 5;
}

// java.lang.ThreadGroup
pub struct JavaLangThreadGroup;
pub type java_lang_ThreadGroup = JavaLangThreadGroup;

impl JavaLangThreadGroup {
    pub const PARENT_OFFSET: usize = 0;
    pub const NAME_OFFSET: usize = 1;
    pub const MAXPRIORITY_OFFSET: usize = 2;
    pub const DESTROYED_OFFSET: usize = 3;
    pub const DAEMON_OFFSET: usize = 4;
    pub const NTHREADS_OFFSET: usize = 5;
    pub const THREADS_OFFSET: usize = 6;
    pub const NGROUPS_OFFSET: usize = 7;
    pub const GROUPS_OFFSET: usize = 8;
}

// java.lang.VirtualThread
pub struct JavaLangVirtualThread;
pub type java_lang_VirtualThread = JavaLangVirtualThread;

impl JavaLangVirtualThread {
    pub const STATE_OFFSET: usize = 0;
    pub const CARRIER_THREAD_OFFSET: usize = 1;
    pub const CONTINUATION_OFFSET: usize = 2;
    pub const SCHEDULER_OFFSET: usize = 3;

    pub const NEW: i32 = 0;
    pub const STARTED: i32 = 1;
    pub const RUNNABLE: i32 = 2;
    pub const RUNNING: i32 = 3;
    pub const PARKING: i32 = 4;
    pub const PARKED: i32 = 5;
    pub const PINNED: i32 = 6;
    pub const YIELDING: i32 = 7;
    pub const TERMINATED: i32 = 99;
}

// java.lang.Throwable
pub struct JavaLangThrowable;
pub type java_lang_Throwable = JavaLangThrowable;

impl JavaLangThrowable {
    pub const DETAIL_MESSAGE_OFFSET: usize = 0;
    pub const CAUSE_OFFSET: usize = 1;
    pub const STACK_TRACE_OFFSET: usize = 2;
    pub const DEPTH_OFFSET: usize = 3;

    pub fn format_stack_trace(elements: &[StackTraceElement]) -> String {
        let mut buf = String::new();
        for el in elements {
            buf.push_str("    at ");
            buf.push_str(&el.declaring_class);
            buf.push('.');
            buf.push_str(&el.method_name);
            buf.push('(');
            if let Some(file) = &el.file_name {
                buf.push_str(file);
                if el.line_number >= 0 {
                    buf.push(':');
                    buf.push_str(&el.line_number.to_string());
                }
            } else if el.line_number == -2 {
                buf.push_str("Native Method");
            } else {
                buf.push_str("Unknown Source");
            }
            buf.push_str(")\n");
        }
        buf
    }
}

// Boxing classes
pub struct JavaLangBoxing;
pub type java_lang_boxing_object = JavaLangBoxing;

impl JavaLangBoxing {
    pub const BOOLEAN_CLASS: &'static str = "java/lang/Boolean";
    pub const BYTE_CLASS: &'static str = "java/lang/Byte";
    pub const CHAR_CLASS: &'static str = "java/lang/Character";
    pub const SHORT_CLASS: &'static str = "java/lang/Short";
    pub const INT_CLASS: &'static str = "java/lang/Integer";
    pub const LONG_CLASS: &'static str = "java/lang/Long";
    pub const FLOAT_CLASS: &'static str = "java/lang/Float";
    pub const DOUBLE_CLASS: &'static str = "java/lang/Double";

    pub fn is_box_class(class_name: &str) -> bool {
        matches!(
            class_name,
            Self::BOOLEAN_CLASS
                | Self::BYTE_CLASS
                | Self::CHAR_CLASS
                | Self::SHORT_CLASS
                | Self::INT_CLASS
                | Self::LONG_CLASS
                | Self::FLOAT_CLASS
                | Self::DOUBLE_CLASS
        )
    }

    pub fn primitive_type(class_name: &str) -> Option<&'static str> {
        match class_name {
            Self::BOOLEAN_CLASS => Some("Z"),
            Self::BYTE_CLASS => Some("B"),
            Self::CHAR_CLASS => Some("C"),
            Self::SHORT_CLASS => Some("S"),
            Self::INT_CLASS => Some("I"),
            Self::LONG_CLASS => Some("J"),
            Self::FLOAT_CLASS => Some("F"),
            Self::DOUBLE_CLASS => Some("D"),
            _ => None,
        }
    }

    pub fn box_class_for_primitive(prim: &str) -> Option<&'static str> {
        match prim {
            "Z" => Some(Self::BOOLEAN_CLASS),
            "B" => Some(Self::BYTE_CLASS),
            "C" => Some(Self::CHAR_CLASS),
            "S" => Some(Self::SHORT_CLASS),
            "I" => Some(Self::INT_CLASS),
            "J" => Some(Self::LONG_CLASS),
            "F" => Some(Self::FLOAT_CLASS),
            "D" => Some(Self::DOUBLE_CLASS),
            _ => None,
        }
    }
}

// Reflection support classes
pub struct JavaLangReflectAccessibleObject;
pub type java_lang_reflect_AccessibleObject = JavaLangReflectAccessibleObject;

impl JavaLangReflectAccessibleObject {
    pub const OVERRIDE_OFFSET: usize = 0;
}

pub struct JavaLangReflectMethod;
pub type java_lang_reflect_Method = JavaLangReflectMethod;

impl JavaLangReflectMethod {
    pub const CLAZZ_OFFSET: usize = 0;
    pub const NAME_OFFSET: usize = 1;
    pub const RETURN_TYPE_OFFSET: usize = 2;
    pub const PARAMETER_TYPES_OFFSET: usize = 3;
    pub const EXCEPTION_TYPES_OFFSET: usize = 4;
    pub const MODIFIERS_OFFSET: usize = 5;
    pub const SLOT_OFFSET: usize = 6;
    pub const SIGNATURE_OFFSET: usize = 7;
    pub const ANNOTATIONS_OFFSET: usize = 8;
}

pub struct JavaLangReflectConstructor;
pub type java_lang_reflect_Constructor = JavaLangReflectConstructor;

impl JavaLangReflectConstructor {
    pub const CLAZZ_OFFSET: usize = 0;
    pub const PARAMETER_TYPES_OFFSET: usize = 1;
    pub const EXCEPTION_TYPES_OFFSET: usize = 2;
    pub const MODIFIERS_OFFSET: usize = 3;
    pub const SLOT_OFFSET: usize = 4;
    pub const SIGNATURE_OFFSET: usize = 5;
    pub const ANNOTATIONS_OFFSET: usize = 6;
}

pub struct JavaLangReflectField;
pub type java_lang_reflect_Field = JavaLangReflectField;

impl JavaLangReflectField {
    pub const CLAZZ_OFFSET: usize = 0;
    pub const NAME_OFFSET: usize = 1;
    pub const TYPE_OFFSET: usize = 2;
    pub const MODIFIERS_OFFSET: usize = 3;
    pub const SLOT_OFFSET: usize = 4;
    pub const SIGNATURE_OFFSET: usize = 5;
    pub const ANNOTATIONS_OFFSET: usize = 6;
}

pub struct JavaLangReflectParameter;
pub type java_lang_reflect_Parameter = JavaLangReflectParameter;

impl JavaLangReflectParameter {
    pub const NAME_OFFSET: usize = 0;
    pub const MODIFIERS_OFFSET: usize = 1;
    pub const EXECUTABLE_OFFSET: usize = 2;
    pub const INDEX_OFFSET: usize = 3;
}

// java.lang.Module
pub struct JavaLangModule;
pub type java_lang_Module = JavaLangModule;

impl JavaLangModule {
    pub const LOADER_OFFSET: usize = 0;
    pub const NAME_OFFSET: usize = 1;
}

// java.lang.ClassLoader
pub struct JavaLangClassLoader;
pub type java_lang_ClassLoader = JavaLangClassLoader;

impl JavaLangClassLoader {
    pub const PARENT_OFFSET: usize = 0;
    pub const NAME_OFFSET: usize = 1;
    pub const UNNAMED_MODULE_OFFSET: usize = 2;
}

// java.lang.System
pub struct JavaLangSystem;
pub type java_lang_System = JavaLangSystem;

impl JavaLangSystem {
    pub const IN_OFFSET: usize = 0;
    pub const OUT_OFFSET: usize = 1;
    pub const ERR_OFFSET: usize = 2;
}

// MethodHandles & Invoke
pub struct JavaLangInvokeMethodHandle;
pub type java_lang_invoke_MethodHandle = JavaLangInvokeMethodHandle;

impl JavaLangInvokeMethodHandle {
    pub const FORM_OFFSET: usize = 0;
    pub const TYPE_OFFSET: usize = 1;
}

pub struct JavaLangInvokeDirectMethodHandle;
pub type java_lang_invoke_DirectMethodHandle = JavaLangInvokeDirectMethodHandle;

impl JavaLangInvokeDirectMethodHandle {
    pub const MEMBER_OFFSET: usize = 0;
}

pub struct JavaLangInvokeLambdaForm;
pub type java_lang_invoke_LambdaForm = JavaLangInvokeLambdaForm;

impl JavaLangInvokeLambdaForm {
    pub const VMENTRY_OFFSET: usize = 0;
}

pub struct JavaLangInvokeMemberName;
pub type java_lang_invoke_MemberName = JavaLangInvokeMemberName;

impl JavaLangInvokeMemberName {
    pub const CLAZZ_OFFSET: usize = 0;
    pub const NAME_OFFSET: usize = 1;
    pub const TYPE_OFFSET: usize = 2;
    pub const FLAGS_OFFSET: usize = 3;
    pub const METHOD_OFFSET: usize = 4;

    pub const MN_IS_METHOD: u32 = 0x00010000;
    pub const MN_IS_CONSTRUCTOR: u32 = 0x00020000;
    pub const MN_IS_FIELD: u32 = 0x00040000;
    pub const MN_IS_TYPE: u32 = 0x00080000;
}

pub struct JavaLangInvokeMethodType;
pub type java_lang_invoke_MethodType = JavaLangInvokeMethodType;

impl JavaLangInvokeMethodType {
    pub const RTYPE_OFFSET: usize = 0;
    pub const PTYPES_OFFSET: usize = 1;
}

pub struct JavaLangInvokeCallSite;
pub type java_lang_invoke_CallSite = JavaLangInvokeCallSite;

impl JavaLangInvokeCallSite {
    pub const TARGET_OFFSET: usize = 0;
}

pub struct JavaLangInvokeConstantCallSite;
pub type java_lang_invoke_ConstantCallSite = JavaLangInvokeConstantCallSite;

impl JavaLangInvokeConstantCallSite {
    pub const IS_FROZEN_OFFSET: usize = 0;
}

// References
pub struct JavaLangRefReference;
pub type java_lang_ref_Reference = JavaLangRefReference;

impl JavaLangRefReference {
    pub const REFERENT_OFFSET: usize = 0;
    pub const QUEUE_OFFSET: usize = 1;
    pub const NEXT_OFFSET: usize = 2;
    pub const DISCOVERED_OFFSET: usize = 3;
}

pub struct JavaLangRefSoftReference;
pub type java_lang_ref_SoftReference = JavaLangRefSoftReference;

impl JavaLangRefSoftReference {
    pub const TIMESTAMP_OFFSET: usize = 0;
}

// Assertion Directives & Misc
pub struct JavaLangAssertionStatusDirectives;
pub type java_lang_AssertionStatusDirectives = JavaLangAssertionStatusDirectives;

impl JavaLangAssertionStatusDirectives {
    pub const CLASSES_OFFSET: usize = 0;
    pub const CLASS_ENABLED_OFFSET: usize = 1;
    pub const PACKAGES_OFFSET: usize = 2;
    pub const PACKAGE_ENABLED_OFFSET: usize = 3;
    pub const DEFLT_OFFSET: usize = 4;
}

pub struct JavaLangInternalError;
pub type java_lang_InternalError = JavaLangInternalError;

pub struct JavaLangInteger;
pub type java_lang_Integer = JavaLangInteger;

impl JavaLangInteger {
    pub const VALUE_OFFSET: usize = 0;
}

pub struct JavaLangLong;
pub type java_lang_Long = JavaLangLong;

impl JavaLangLong {
    pub const VALUE_OFFSET: usize = 0;
}

pub struct JavaLangCharacter;
pub type java_lang_Character = JavaLangCharacter;

impl JavaLangCharacter {
    pub const VALUE_OFFSET: usize = 0;
}

pub struct JavaLangShort;
pub type java_lang_Short = JavaLangShort;

impl JavaLangShort {
    pub const VALUE_OFFSET: usize = 0;
}

pub struct JavaLangByte;
pub type java_lang_Byte = JavaLangByte;

impl JavaLangByte {
    pub const VALUE_OFFSET: usize = 0;
}

pub struct JavaLangBoolean;
pub type java_lang_Boolean = JavaLangBoolean;

impl JavaLangBoolean {
    pub const VALUE_OFFSET: usize = 0;
}
