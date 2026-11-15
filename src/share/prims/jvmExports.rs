/*
 * Port of OpenJDK 25 HotSpot JNI C entry points
 * (share/prims/jni.cpp / jvm.cpp exports for jvm.dll)
 *
 * Provides the process-visible JNI Invocation API exports that java.exe /
 * any JNI client links against in jvm.dll:
 *
 * - JNI_CreateJavaVM(JavaVM**, void**, void* args)
 * - JNI_GetCreatedJavaVMs(JavaVM**, jsize, jsize*)
 * - JNI_GetDefaultJavaVMInitArgs(void*)
 *
 * The VM object itself is the real ABI-compatible `JavaVm` implemented in
 * `jniInvocation.rs`: a `JavaVM*` points at the `JNIInvokeInterface` table
 * (offset 0) and a `JNIEnv*` points at the `JNINativeInterface` table
 * (offset 0), exactly as HotSpot's `main_vm` / `jni_environment()` do.
 * No stubs: attach/detach/GetEnv and all 236 JNIEnv slots are the real
 * implementations already present in `jniInvocation.rs`.
 */

use super::jni::{
    JNI_EEXIST, JNI_ERR, JNI_EVERSION, JNI_OK, JNI_VERSION_1_1,
    JNI_VERSION_1_2, JNI_VERSION_1_4, JNI_VERSION_1_6, JNI_VERSION_1_8, JNI_VERSION_9,
    JNI_VERSION_19, JNI_VERSION_21, JNI_VERSION_25,
};
use super::jniInvocation::JavaVm;
use crate::share::runtime::init::VMInit;
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Mutex;

// Extra JNI versions known to HotSpot 25 (not named constants in jni.rs).
const JNI_VERSION_10: i32 = 0x000a0000;
const JNI_VERSION_17: i32 = 0x00110000;
const JNI_VERSION_20: i32 = 0x00140000;
const JNI_VERSION_24: i32 = 0x00180000;

const JNI_VERSION_MAX: i32 = JNI_VERSION_25;

fn supported_jni_version(version: i32) -> bool {
    matches!(
        version,
        JNI_VERSION_1_1
            | JNI_VERSION_1_2
            | JNI_VERSION_1_4
            | JNI_VERSION_1_6
            | JNI_VERSION_1_8
            | JNI_VERSION_9
            | JNI_VERSION_10
            | JNI_VERSION_17
            | JNI_VERSION_19
            | JNI_VERSION_20
            | JNI_VERSION_21
            | JNI_VERSION_24
            | JNI_VERSION_25
    )
}

// ---------------------------------------------------------------------------
// C ABI structures (jni.h: JavaVMOption / JavaVMInitArgs / JavaVMAttachArgs /
// JNIInvokeInterface_)
// ---------------------------------------------------------------------------

/// `JavaVMOption` — `char *optionString; void *extraInfo;`
#[repr(C)]
pub struct JavaVMOption {
    pub option_string: *mut c_char,
    pub extra_info: *mut c_void,
}

/// `JavaVMInitArgs` — `jint version; jint nOptions; JavaVMOption *options;
/// jboolean ignoreUnrecognized;` (`jboolean` is `unsigned char`).
#[repr(C)]
pub struct JavaVMInitArgs {
    pub version: i32,
    pub n_options: i32,
    pub options: *mut JavaVMOption,
    pub ignore_unrecognized: u8,
}

/// `JavaVMAttachArgs` — `jint version; char *name; jobject group;`
#[repr(C)]
struct JavaVMAttachArgs {
    version: i32,
    name: *mut c_char,
    group: *mut c_void,
}

/// Mirror of `JNIInvokeInterface_`. `JavaVm`'s first field is a pointer to a
/// function table with exactly this layout, so casting is ABI-correct.
#[repr(C)]
struct JniInvokeInterface {
    reserved0: *mut c_void,
    reserved1: *mut c_void,
    reserved2: *mut c_void,
    destroy_java_vm: Option<unsafe extern "system" fn(*mut c_void) -> i32>,
    attach_current_thread:
        Option<unsafe extern "system" fn(*mut c_void, *mut *mut c_void, *mut c_void) -> i32>,
    detach_current_thread: Option<unsafe extern "system" fn(*mut c_void) -> i32>,
    get_env: Option<unsafe extern "system" fn(*mut c_void, *mut *mut c_void, i32) -> i32>,
    attach_current_thread_as_daemon:
        Option<unsafe extern "system" fn(*mut c_void, *mut *mut c_void, *mut c_void) -> i32>,
}

/// Reads the `JNIInvokeInterface` table behind a `JavaVM*`. The first word of
/// `JavaVm` is `functions: *const InvocationTable`, whose layout is identical.
unsafe fn invoke_interface(vm: *mut c_void) -> *const JniInvokeInterface {
    unsafe { (*(vm as *const *const c_void)) as *const JniInvokeInterface }
}

// ---------------------------------------------------------------------------
// VM creation state (mirrors HotSpot's vm_created: NOT_CREATED / IN_PROGRESS /
// COMPLETE)
// ---------------------------------------------------------------------------

const VM_NOT_CREATED: u8 = 0;
const VM_IN_PROGRESS: u8 = 1;
const VM_COMPLETE: u8 = 2;

static VM_CREATED: AtomicU8 = AtomicU8::new(VM_NOT_CREATED);
static CREATED_VM: AtomicUsize = AtomicUsize::new(0);

/// VM options recorded from `JNI_CreateJavaVM`'s `JavaVMInitArgs`. HotSpot
/// feeds these to `Arguments`; we keep them real and queryable so later VM
/// stages (class loader bootstrap, system properties) can consume them.
static VM_OPTIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static VM_PROPERTIES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Returns the system properties (`-Dname=value`) recorded by
/// `JNI_CreateJavaVM`, in the order they were passed by the launcher.
pub fn recorded_system_properties() -> Vec<(String, String)> {
    VM_PROPERTIES.lock().unwrap().clone()
}

/// Returns all raw VM options recorded by `JNI_CreateJavaVM`.
pub fn recorded_vm_options() -> Vec<String> {
    VM_OPTIONS.lock().unwrap().clone()
}

fn record_init_args(args: *const JavaVMInitArgs, ignore_unrecognized: bool) {
    if args.is_null() {
        return;
    }
    let args = unsafe { &*args };
    let n = args.n_options.max(0).min(4096) as usize;
    if args.options.is_null() || n == 0 {
        return;
    }
    let mut options = VM_OPTIONS.lock().unwrap();
    let mut properties = VM_PROPERTIES.lock().unwrap();
    for i in 0..n {
        let option = unsafe { &*args.options.add(i) };
        if option.option_string.is_null() {
            continue;
        }
        let text = unsafe { CStr::from_ptr(option.option_string) }
            .to_string_lossy()
            .into_owned();
        options.push(text.clone());
        // `-D<name>=<value>` sets a system property, per jni.h docs.
        if let Some(rest) = text.strip_prefix("-D") {
            let (key, value) = rest.split_once('=').unwrap_or((rest, ""));
            if !key.is_empty() {
                properties.push((key.to_string(), value.to_string()));
            }
        } else if !ignore_unrecognized
            && !text.starts_with("-X")
            && !text.starts_with("-verbose")
            && !text.starts_with('-')
        {
            // Positional/main-class style arguments are not VM options; they
            // are simply not consumed here. JVM-specific flags we do not yet
            // implement are still recorded above rather than failing.
        }
    }
}

// ---------------------------------------------------------------------------
// Exported JNI entry points
// ---------------------------------------------------------------------------

/// `JNI_GetDefaultJavaVMInitArgs(void *args)` — jni.cpp.
///
/// Validates that `args->version` names a supported JNI version; like
/// HotSpot, the other fields are left untouched for the caller to fill.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn JNI_GetDefaultJavaVMInitArgs(args: *mut c_void) -> i32 {
    if args.is_null() {
        return JNI_ERR;
    }
    let version = unsafe { &*(args as *const JavaVMInitArgs) }.version;
    if supported_jni_version(version) {
        JNI_OK
    } else {
        JNI_EVERSION
    }
}

/// `JNI_CreateJavaVM(JavaVM **vm, void **penv, void *args)` — jni.cpp.
///
/// On success `*vm` receives the process `JavaVM*` (whose first word points
/// at the `JNIInvokeInterface` table) and `*penv` a `JNIEnv*` for the calling
/// thread (first word points at the `JNINativeInterface` table). The calling
/// thread is attached automatically, matching HotSpot's `create_vm`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn JNI_CreateJavaVM(
    vm_out: *mut *mut c_void,
    penv_out: *mut *mut c_void,
    args: *mut c_void,
) -> i32 {
    if vm_out.is_null() || penv_out.is_null() || args.is_null() {
        return JNI_ERR;
    }
    unsafe {
        vm_out.write(ptr::null_mut());
        penv_out.write(ptr::null_mut());
    }

    let init_args = args as *const JavaVMInitArgs;
    let version = unsafe { (*init_args).version };
    if !supported_jni_version(version) || version > JNI_VERSION_MAX {
        return JNI_EVERSION;
    }

    // Single-VM semantics, same as HotSpot's vm_created state machine.
    if VM_CREATED
        .compare_exchange(VM_NOT_CREATED, VM_IN_PROGRESS, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return JNI_EEXIST;
    }

    record_init_args(init_args, unsafe { (*init_args).ignore_unrecognized != 0 });

    // Initialize global subsystems (idempotent, guarded internally).
    if VMInit::init_globals().is_err() {
        VM_CREATED.store(VM_NOT_CREATED, Ordering::SeqCst);
        return JNI_ERR;
    }
    VMInit::set_init_completed();

    let vm = JavaVm::global() as *mut c_void;

    // Attach the calling thread through the real JNIInvokeInterface so the
    // returned JNIEnv* carries the requested JNI version.
    let invoke = unsafe { invoke_interface(vm) };
    let attach = unsafe { &*invoke }.attach_current_thread;
    let Some(attach) = attach else {
        VM_CREATED.store(VM_NOT_CREATED, Ordering::SeqCst);
        return JNI_ERR;
    };
    let attach_args = JavaVMAttachArgs {
        version,
        name: ptr::null_mut(),
        group: ptr::null_mut(),
    };
    let mut env: *mut c_void = ptr::null_mut();
    let rc = unsafe {
        attach(
            vm,
            &mut env,
            &attach_args as *const JavaVMAttachArgs as *mut c_void,
        )
    };
    if rc != JNI_OK || env.is_null() {
        VM_CREATED.store(VM_NOT_CREATED, Ordering::SeqCst);
        return rc;
    }

    CREATED_VM.store(vm as usize, Ordering::Release);
    VM_CREATED.store(VM_COMPLETE, Ordering::Release);
    unsafe {
        vm_out.write(vm);
        penv_out.write(env);
    }
    JNI_OK
}

/// `JNI_GetCreatedJavaVMs(JavaVM **vm_buf, jsize bufLen, jsize *numVMs)` —
/// jni.cpp. Returns JNI_OK and reports 0 or 1 created VMs.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn JNI_GetCreatedJavaVMs(
    vm_buf: *mut *mut c_void,
    buf_len: i32,
    num_vms: *mut i32,
) -> i32 {
    if num_vms.is_null() || buf_len < 0 || (buf_len > 0 && vm_buf.is_null()) {
        return JNI_ERR;
    }
    let created = VM_CREATED.load(Ordering::Acquire) == VM_COMPLETE;
    let total: i32 = if created { 1 } else { 0 };
    let written = total.min(buf_len);
    unsafe { num_vms.write(written) };
    if written > 0 {
        let vm = CREATED_VM.load(Ordering::Acquire) as *mut c_void;
        unsafe { *vm_buf = vm };
    }
    JNI_OK
}
