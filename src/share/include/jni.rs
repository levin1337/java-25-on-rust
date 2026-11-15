use std::ffi::{c_char, c_void};

pub use crate::share::prims::jni::{
    JNI_EDETACHED, JNI_EEXIST, JNI_EINVAL, JNI_ENOMEM, JNI_ERR, JNI_EVERSION, JNI_OK,
    JNI_VERSION_1_1, JNI_VERSION_1_2, JNI_VERSION_1_4, JNI_VERSION_1_6, JNI_VERSION_1_8,
    JNI_VERSION_19, JNI_VERSION_21, JNI_VERSION_25, JNI_VERSION_9,
};

pub const JNI_VERSION_10: i32 = 0x000A0000;
pub const JNI_VERSION_11: i32 = 0x000B0000;
pub const JNI_VERSION_12: i32 = 0x000C0000;
pub const JNI_VERSION_13: i32 = 0x000D0000;
pub const JNI_VERSION_14: i32 = 0x000E0000;
pub const JNI_VERSION_15: i32 = 0x000F0000;
pub const JNI_VERSION_16: i32 = 0x00100000;
pub const JNI_VERSION_17: i32 = 0x00110000;
pub const JNI_VERSION_18: i32 = 0x00120000;
pub const JNI_VERSION_20: i32 = 0x00140000;
pub const JNI_VERSION_22: i32 = 0x00160000;
pub const JNI_VERSION_23: i32 = 0x00170000;
pub const JNI_VERSION_24: i32 = 0x00180000;

pub const JNI_TRUE: u8 = 1;
pub const JNI_FALSE: u8 = 0;
pub const JNI_COMMIT: i32 = 1;
pub const JNI_ABORT: i32 = 2;

pub type Jboolean = u8;
pub type Jbyte = i8;
pub type Jchar = u16;
pub type Jshort = i16;
pub type Jint = i32;
pub type Jlong = i64;
pub type Jfloat = f32;
pub type Jdouble = f64;
pub type Jsize = Jint;

macro_rules! jni_ref {
    ($struct:ident, $alias:ident) => {
        #[repr(C)]
        pub struct $struct {
            _private: [u8; 0],
        }
        pub type $alias = *mut $struct;
    };
}

jni_ref!(Jobject, jobject);
jni_ref!(Jclass, jclass);
jni_ref!(Jstring, jstring);
jni_ref!(Jarray, jarray);
jni_ref!(JbooleanArray, jbooleanArray);
jni_ref!(JbyteArray, jbyteArray);
jni_ref!(JcharArray, jcharArray);
jni_ref!(JshortArray, jshortArray);
jni_ref!(JintArray, jintArray);
jni_ref!(JlongArray, jlongArray);
jni_ref!(JfloatArray, jfloatArray);
jni_ref!(JdoubleArray, jdoubleArray);
jni_ref!(JobjectArray, jobjectArray);
jni_ref!(Jthrowable, jthrowable);
jni_ref!(Jweak, jweak);
jni_ref!(JfieldID, jfieldID);
jni_ref!(JmethodID, jmethodID);

#[repr(C)]
pub struct JNINativeInterface {
    _private: [u8; 0],
}

#[repr(C)]
pub struct JNIInvokeInterface {
    _private: [u8; 0],
}

pub type JNIEnv = *mut *const JNINativeInterface;
pub type JavaVM = *mut *const JNIInvokeInterface;
pub type JNIEnvPtr = *mut JNIEnv;
pub type JavaVMPtr = *mut JavaVM;

#[derive(Clone, Copy)]
#[repr(C)]
pub union JValue {
    pub z: Jboolean,
    pub b: Jbyte,
    pub c: Jchar,
    pub s: Jshort,
    pub i: Jint,
    pub j: Jlong,
    pub f: Jfloat,
    pub d: Jdouble,
    pub l: *mut c_void,
}

impl std::fmt::Debug for JValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SAFETY-free debug: JValue is a C union; print its raw i64 bits.
        f.debug_tuple("JValue").field(&unsafe { self.j }).finish()
    }
}

#[repr(C)]
pub struct JNINativeMethod {
    pub name: *const c_char,
    pub signature: *const c_char,
    pub fn_ptr: *mut c_void,
}

#[repr(C)]
pub struct JavaVMOption {
    pub option_string: *const c_char,
    pub extra_info: *mut c_void,
}

#[repr(C)]
pub struct JavaVMInitArgs {
    pub version: Jint,
    pub n_options: Jint,
    pub options: *mut JavaVMOption,
    pub ignore_unrecognized: Jboolean,
}

unsafe extern "C" {
    pub fn JNI_GetDefaultJavaVMInitArgs(args: *mut JavaVMInitArgs) -> Jint;
    pub fn JNI_CreateJavaVM(
        vm: *mut JavaVMPtr,
        env: *mut *mut c_void,
        args: *mut JavaVMInitArgs,
    ) -> Jint;
    pub fn JNI_GetCreatedJavaVMs(
        vm_buf: *mut JavaVMPtr,
        buf_len: Jsize,
        n_vms: *mut Jsize,
    ) -> Jint;
}
