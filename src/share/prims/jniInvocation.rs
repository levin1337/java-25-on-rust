/*
 * Port of OpenJDK 25 HotSpot JNI Invocation API
 * (share/prims/jni.cpp, jni.hpp and generated/jnifiles/jni.h)
 *
 * Provides ABI-compatible JavaVM/JNIEnv objects for Windows x64:
 * - JNIInvokeInterface (DestroyJavaVM, AttachCurrentThread[AsDaemon],
 *   DetachCurrentThread, GetEnv)
 * - JNINativeInterface with all 236 slots in the exact jni.h order
 * - Local/global/weak JNI references bound to JniReferences + GC objects
 * - String, array, field and method access over the interpreter object model
 * - RegisterNatives/UnregisterNatives and JNI_OnLoad/JNI_OnUnload support
 * - Variadic CallXxxMethod/NewObject via generated x64 capture stubs
 */

#![allow(warnings)]

use super::jni::{
    JNI_EDETACHED, JNI_EEXIST, JNI_EINVAL, JNI_ENOMEM, JNI_ERR, JNI_EVERSION, JNI_OK,
    JNI_VERSION_1_1, JNI_VERSION_1_2, JNI_VERSION_1_4, JNI_VERSION_1_6, JNI_VERSION_1_8,
    JNI_VERSION_9, JNI_VERSION_19, JNI_VERSION_21, JNI_VERSION_25,
};
use super::jniReferences::{JniRefTarget, JniRefType, JniReference, JniReferences};
use super::jniTrampoline::{
    self, ArgKind, CallContext, CallPlan, RetKind, VAR_CALL_INSTANCE, VAR_CALL_NEWOBJECT,
    VAR_CALL_NONVIRTUAL, VAR_CALL_STATIC, VAR_RET_F32, VAR_RET_F64, VAR_RET_GP, VarArgsCtx,
    VarArgsHandler,
};
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::objArrayOop::ObjArrayOop;
use crate::share::oops::oop::Oop;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use crate::share::runtime::javaThread::{JavaThread, JavaThreadState};
use crate::share::runtime::signature;
use crate::share::runtime::threads::Threads;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_void};
use std::io::{self, Error};
use std::marker::PhantomPinned;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::{self, ThreadId};

pub type JniObject = *mut c_void;
pub type JniClass = *mut c_void;
pub type JniString = *mut c_void;
pub type JniArray = *mut c_void;
pub type JniByteArray = *mut c_void;
pub type JniBooleanArray = *mut c_void;
pub type JniCharArray = *mut c_void;
pub type JniShortArray = *mut c_void;
pub type JniIntArray = *mut c_void;
pub type JniLongArray = *mut c_void;
pub type JniFloatArray = *mut c_void;
pub type JniDoubleArray = *mut c_void;
pub type JniObjectArray = *mut c_void;
pub type JniWeak = *mut c_void;
pub type JniMethodID = *mut c_void;
pub type JniFieldID = *mut c_void;
pub type JniThrowable = *mut c_void;

const JNI_COMMIT: i32 = 0;
const JNI_ABORT: i32 = 2;

// ---------------------------------------------------------------------------
// VM state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NativeCode {
    pub func: *const c_void,
    pub trampoline: *const c_void,
    pub plan: CallPlan,
}

#[derive(Clone)]
struct JniMethodId {
    class: Vec<u8>,
    name: Vec<u8>,
    sig: Vec<u8>,
    is_static: bool,
}

#[derive(Clone)]
struct JniFieldId {
    class: Vec<u8>,
    name: Vec<u8>,
    desc: Vec<u8>,
    is_static: bool,
}

struct ClassEntry {
    name: Vec<u8>,
    primitive: bool,
}

#[derive(Default)]
struct JvmState {
    loader: usize,
    version: i32,
    classes: Vec<ClassEntry>,
    class_ids: HashMap<Vec<u8>, u64>,
    methods: HashMap<u64, JniMethodId>,
    fields: HashMap<u64, JniFieldId>,
    next_id: u64,
    natives: HashMap<(Vec<u8>, Vec<u8>, Vec<u8>), NativeCode>,
    trampoline_cache: HashMap<(usize, Vec<u8>), NativeCode>,
    attached: HashSet<ThreadId>,
    monitors: HashMap<(u8, usize), Arc<Mutex<()>>>,
}

// repr(C) is required: a JavaVM* is a pointer to the InvocationTable, which
// must occupy offset 0 for the C ABI double indirection to work.
#[repr(C)]
pub struct JavaVm {
    functions: *const InvocationTable,
    id: u64,
    pin: PhantomPinned,
    references: JniReferences,
    state: Mutex<JvmState>,
}

// The VM object is only shared through raw pointers across JNI threads;
// all mutable state is behind Mutex and references behind their own lock.
unsafe impl Send for JavaVm {}
unsafe impl Sync for JavaVm {}

static nextVmId: AtomicU64 = AtomicU64::new(1);
static GLOBAL_VM: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static environments: RefCell<HashMap<u64, Box<NativeEnvironment>>> =
        RefCell::new(HashMap::new());
    static CURRENT_ENV: RefCell<usize> = const { RefCell::new(0) };
}

#[repr(C)]
struct NativeEnvironment {
    functions: *const JniNativeTable,
    vm: *mut JavaVm,
    version: i32,
    exception: RefCell<PendingException>,
    /// Pinned buffers for GetStringChars/GetXxxArrayElements results.
    pinned: RefCell<Vec<PinnedBuffer>>,
    /// Monitors held by this thread, in acquisition order (for reentrancy).
    held_monitors: RefCell<Vec<MonitorHold>>,
}

struct MonitorHold {
    key: (u8, usize),
    owner: ThreadId,
    monitor: Arc<Mutex<()>>,
    guard: MutexGuard<'static, ()>,
    depth: u32,
}

struct PinnedBuffer {
    ptr: *mut u8,
    len: usize,
    /// For string buffers: owned bytes to free on release.
    owner: Option<Box<[u8]>>,
}

#[derive(Clone)]
enum PendingException {
    None,
    Object(Oop),
    New {
        class: Vec<u8>,
        message: Option<String>,
    },
}

impl Default for PendingException {
    fn default() -> Self {
        Self::None
    }
}

// ---------------------------------------------------------------------------
// JavaVM (JNIInvokeInterface)
// ---------------------------------------------------------------------------

#[repr(C)]
struct InvocationTable {
    reserved0: *mut c_void,
    reserved1: *mut c_void,
    reserved2: *mut c_void,
    destroy: unsafe extern "system" fn(*mut JavaVm) -> i32,
    attach: unsafe extern "system" fn(*mut JavaVm, *mut *mut c_void, *mut c_void) -> i32,
    detach: unsafe extern "system" fn(*mut JavaVm) -> i32,
    getEnv: unsafe extern "system" fn(*mut JavaVm, *mut *mut c_void, i32) -> i32,
    attachDaemon: unsafe extern "system" fn(*mut JavaVm, *mut *mut c_void, *mut c_void) -> i32,
}

unsafe impl Sync for InvocationTable {}
unsafe impl Send for InvocationTable {}

static invocationTable: InvocationTable = InvocationTable {
    reserved0: ptr::null_mut(),
    reserved1: ptr::null_mut(),
    reserved2: ptr::null_mut(),
    destroy: jni_destroy_java_vm,
    attach: jni_attach_current_thread,
    detach: jni_detach_current_thread,
    getEnv: jni_get_env,
    attachDaemon: jni_attach_current_thread,
};

impl JavaVm {
    pub fn new() -> Self {
        Self {
            functions: &invocationTable,
            id: nextVmId.fetch_add(1, Ordering::Relaxed),
            pin: PhantomPinned,
            references: JniReferences::new(),
            state: Mutex::new(JvmState {
                version: JNI_VERSION_25,
                ..JvmState::default()
            }),
        }
    }

    pub fn pinned() -> std::pin::Pin<Box<Self>> {
        Box::pin(Self::new())
    }

    /// Returns the process-wide JavaVM instance without creating one.
    /// Used by the garbage collector to enumerate JNI roots.
    pub fn global_if_exists() -> Option<*mut JavaVm> {
        let raw = GLOBAL_VM.load(Ordering::Acquire);
        (raw != 0).then(|| unsafe { &mut *(raw as *mut JavaVm) } as *mut JavaVm)
    }

    /// Returns the process-wide JavaVM instance, creating it on first use.
    /// The pointer stays valid for the lifetime of the process.
    pub fn global() -> *mut JavaVm {
        let raw = GLOBAL_VM.load(Ordering::Acquire);
        if raw != 0 {
            return unsafe { &mut *(raw as *mut JavaVm) };
        }
        let vm = Box::new(Self::new());
        let ptr = Box::into_raw(vm) as *mut JavaVm;
        match GLOBAL_VM.compare_exchange(0, ptr as u64, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => ptr,
            Err(existing) => {
                // Another thread won the race; drop our instance.
                unsafe { drop(Box::from_raw(ptr)) };
                unsafe { &mut *(existing as *mut JavaVm) }
            }
        }
    }

    pub fn references(&self) -> &JniReferences {
        &self.references
    }

    pub fn state(&self) -> MutexGuard<'_, JvmState> {
        self.state.lock().unwrap()
    }

    /// Binds the interpreter ClassLoader so JNI functions can resolve classes,
    /// methods and objects. The pointer must remain valid while the VM runs.
    pub fn bind_loader(&self, loader: &ClassLoader<'_>) {
        self.state().loader = loader as *const ClassLoader<'_> as usize;
    }

    pub(crate) fn class_loader(&self) -> Option<&'static ClassLoader<'static>> {
        let ptr = self.state().loader;
        (ptr != 0).then(|| unsafe { &*(ptr as *const ClassLoader<'static>) })
    }

    /// Ensures the calling thread is attached and returns its JNIEnv pointer.
    pub fn ensure_attached(&self) -> *mut c_void {
        if let Some(env) = current_env() {
            return env as *mut c_void;
        }
        let mut out = ptr::null_mut();
        let args = AttachArguments {
            version: JNI_VERSION_1_8,
            name: ptr::null(),
            group: ptr::null_mut(),
        };
        unsafe {
            jni_attach_current_thread(
                self as *const JavaVm as *mut JavaVm,
                &mut out,
                &args as *const AttachArguments as *mut c_void,
            );
        }
        out
    }
}

impl Default for JavaVm {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for JavaVm {
    fn drop(&mut self) {
        environments.with(|values| {
            values.borrow_mut().remove(&self.id);
        });
    }
}

fn current_env() -> Option<*mut NativeEnvironment> {
    CURRENT_ENV.with(|cell| {
        let raw = *cell.borrow();
        (raw != 0).then(|| raw as *mut NativeEnvironment)
    })
}

fn set_current_env(env: *mut NativeEnvironment) {
    CURRENT_ENV.with(|cell| *cell.borrow_mut() = env as usize);
}

fn clear_current_env() {
    CURRENT_ENV.with(|cell| *cell.borrow_mut() = 0);
}

unsafe fn vm_of(env: *mut NativeEnvironment) -> Option<&'static JavaVm> {
    if env.is_null() {
        return None;
    }
    let vm = unsafe { (*env).vm };
    (!vm.is_null()).then(|| unsafe { &*vm })
}

fn supported_version(version: i32) -> bool {
    matches!(
        version,
        JNI_VERSION_1_1
            | JNI_VERSION_1_2
            | JNI_VERSION_1_4
            | JNI_VERSION_1_6
            | JNI_VERSION_1_8
            | JNI_VERSION_9
            | 0x000a0000
            | 0x00110000
            | JNI_VERSION_19
            | 0x00140000
            | JNI_VERSION_21
            | 0x00180000
            | JNI_VERSION_25
    )
}

unsafe extern "system" fn jni_destroy_java_vm(vm: *mut JavaVm) -> i32 {
    if vm.is_null() {
        return JNI_EINVAL;
    }
    let attached = (*vm).state().attached.clone();
    if attached.len() > 1 || (attached.len() == 1 && !attached.contains(&thread::current().id())) {
        return JNI_ERR;
    }
    (*vm).state().attached.clear();
    (*vm).references.detach();
    clear_current_env();
    JNI_OK
}

#[repr(C)]
struct AttachArguments {
    version: i32,
    name: *const c_char,
    group: *mut c_void,
}

unsafe extern "system" fn jni_attach_current_thread(
    vm: *mut JavaVm,
    environment: *mut *mut c_void,
    arguments: *mut c_void,
) -> i32 {
    if environment.is_null() || vm.is_null() {
        return JNI_EINVAL;
    }
    unsafe { environment.write(ptr::null_mut()) };
    let mut version = JNI_VERSION_1_8;
    let mut name = String::from("AttachedThread");
    if !arguments.is_null() {
        let args = unsafe { &*(arguments as *const AttachArguments) };
        version = args.version;
        if !supported_version(version) {
            return JNI_EVERSION;
        }
        if !args.name.is_null()
            && let Ok(cstr) = unsafe { CStr::from_ptr(args.name) }.to_str()
        {
            name = cstr.to_string();
        }
    }
    environments.with(|values| {
        let mut values = values.borrow_mut();
        let id = unsafe { (*vm).id };
        let entry = values.entry(id).or_insert_with(|| {
            Box::new(NativeEnvironment {
                functions: native_table(),
                vm,
                version,
                exception: RefCell::new(PendingException::None),
                pinned: RefCell::new(Vec::new()),
                held_monitors: RefCell::new(Vec::new()),
            })
        });
        let env_ptr = &mut **entry as *mut NativeEnvironment;
        unsafe { environment.write(env_ptr.cast()) };
        set_current_env(env_ptr);
        (*vm).references.ensureBaseFrame();
        (*vm).state().attached.insert(thread::current().id());
        if !Threads::global().find_by_id(id) {
            let mut jt = JavaThread::new(name, true);
            jt.set_thread_state(JavaThreadState::InNative);
            Threads::global().add(jt);
        }
    });
    JNI_OK
}

unsafe extern "system" fn jni_detach_current_thread(vm: *mut JavaVm) -> i32 {
    if vm.is_null() {
        return JNI_EINVAL;
    }
    environments.with(|values| {
        let id = unsafe { (*vm).id };
        if values.borrow_mut().remove(&id).is_none() {
            return JNI_EDETACHED;
        }
        (*vm).references.detach();
        (*vm).state().attached.remove(&thread::current().id());
        clear_current_env();
        JNI_OK
    })
}

unsafe extern "system" fn jni_get_env(
    vm: *mut JavaVm,
    environment: *mut *mut c_void,
    version: i32,
) -> i32 {
    if environment.is_null() {
        return JNI_EINVAL;
    }
    unsafe { environment.write(ptr::null_mut()) };
    if vm.is_null() {
        return JNI_EINVAL;
    }
    if !supported_version(version) || version > (*vm).state().version {
        return JNI_EVERSION;
    }
    environments.with(|values| match values.borrow().get(&unsafe { (*vm).id }) {
        Some(env) => {
            unsafe { environment.write((env.as_ref() as *const NativeEnvironment).cast_mut().cast()) };
            set_current_env(unsafe { &**env } as *const NativeEnvironment as *mut NativeEnvironment);
            JNI_OK
        }
        None => JNI_EDETACHED,
    })
}

// ---------------------------------------------------------------------------
// JNIEnv table
// ---------------------------------------------------------------------------

const JNI_TABLE_SLOTS: usize = 236;

struct JniNativeTable {
    slots: [*mut c_void; JNI_TABLE_SLOTS],
}

// The tables are immutable after construction; raw pointers are only invoked,
// never dereferenced by Rust code.
unsafe impl Sync for JniNativeTable {}
unsafe impl Send for JniNativeTable {}

fn native_table() -> &'static JniNativeTable {
    static TABLE: OnceLock<JniNativeTable> = OnceLock::new();
    TABLE.get_or_init(build_native_table)
}

fn slot_of(function_index: usize) -> usize {
    function_index + 4 // reserved0..reserved3
}

macro_rules! put {
    ($t:ident, $idx:expr, $f:expr) => {
        $t.slots[slot_of($idx)] = $f as usize as *mut c_void
    };
}

fn unsupported_native() -> u64 {
    0
}

macro_rules! put_varargs {
    ($t:ident, $idx:expr, $kind:expr, $ret:expr) => {{
        let tag = jniTrampoline::varargs_tag(
            $kind,
            match $ret {
                RetKind::Float => VAR_RET_F32,
                RetKind::Double => VAR_RET_F64,
                RetKind::Gp => VAR_RET_GP,
            },
        );
        let stub = jniTrampoline::build_varargs_stub(varargs_handler as VarArgsHandler, tag, $ret)
            .unwrap_or(ptr::null_mut());
        $t.slots[slot_of($idx)] = stub as usize as *mut c_void;
    }};
}

fn build_native_table() -> JniNativeTable {
    let mut t = JniNativeTable {
        slots: [ptr::null_mut(); JNI_TABLE_SLOTS],
    };
    let fallback = unsupported_native as usize as *mut c_void;
    for slot in t.slots.iter_mut().skip(4) {
        *slot = fallback;
    }

    // Indices follow generated/jnifiles/jni.h JNINativeInterface_ order.
    put!(t, 0, jni_get_version); // GetVersion
    put!(t, 1, jni_define_class); // DefineClass
    put!(t, 2, jni_find_class); // FindClass
    put!(t, 3, jni_from_reflected_method); // FromReflectedMethod
    put!(t, 4, jni_from_reflected_field); // FromReflectedField
    put!(t, 5, jni_to_reflected_method); // ToReflectedMethod
    put!(t, 6, jni_get_superclass); // GetSuperclass
    put!(t, 7, jni_is_assignable_from); // IsAssignableFrom
    put!(t, 8, jni_to_reflected_field); // ToReflectedField
    put!(t, 9, jni_throw); // Throw
    put!(t, 10, jni_throw_new); // ThrowNew
    put!(t, 11, jni_exception_occurred); // ExceptionOccurred
    put!(t, 12, jni_exception_describe); // ExceptionDescribe
    put!(t, 13, jni_exception_clear); // ExceptionClear
    put!(t, 14, jni_fatal_error); // FatalError
    put!(t, 15, jni_push_local_frame); // PushLocalFrame
    put!(t, 16, jni_pop_local_frame); // PopLocalFrame
    put!(t, 17, jni_new_global_ref); // NewGlobalRef
    put!(t, 18, jni_delete_global_ref); // DeleteGlobalRef
    put!(t, 19, jni_delete_local_ref); // DeleteLocalRef
    put!(t, 20, jni_is_same_object); // IsSameObject
    put!(t, 21, jni_new_local_ref); // NewLocalRef
    put!(t, 22, jni_ensure_local_capacity); // EnsureLocalCapacity
    put!(t, 23, jni_alloc_object); // AllocObject
    put_varargs!(t, 24, VAR_CALL_NEWOBJECT, RetKind::Gp); // NewObject
    put!(t, 25, jni_new_object_v); // NewObjectV
    put!(t, 26, jni_new_object_a); // NewObjectA
    put!(t, 27, jni_get_object_class); // GetObjectClass
    put!(t, 28, jni_is_instance_of); // IsInstanceOf
    put!(t, 29, jni_get_method_id); // GetMethodID

    // CallXxxMethod family: instance(30..57), nonvirtual(60..87), static(110..137)
    for (base, kind) in [
        (30usize, VAR_CALL_INSTANCE),
        (60, VAR_CALL_NONVIRTUAL),
        (110, VAR_CALL_STATIC),
    ] {
        for (i, name) in [
            "Object", "Boolean", "Byte", "Char", "Short", "Int", "Long", "Float", "Double", "Void",
        ]
        .iter()
        .enumerate()
        {
            let ret = match *name {
                "Float" => RetKind::Float,
                "Double" => RetKind::Double,
                _ => RetKind::Gp,
            };
            put_varargs!(t, base + i * 3, kind, ret); // CallXxxMethod (varargs)
            let v_fn = call_v_fn(kind, ret);
            let a_fn = call_a_fn(kind, ret);
            put!(t, base + i * 3 + 1, v_fn); // CallXxxMethodV
            put!(t, base + i * 3 + 2, a_fn); // CallXxxMethodA
        }
    }

    put!(t, 90, jni_get_field_id); // GetFieldID
    put!(t, 91, jni_get_object_field); // GetObjectField
    put!(t, 92, jni_get_boolean_field); // GetBooleanField
    put!(t, 93, jni_get_byte_field); // GetByteField
    put!(t, 94, jni_get_char_field); // GetCharField
    put!(t, 95, jni_get_short_field); // GetShortField
    put!(t, 96, jni_get_int_field); // GetIntField
    put!(t, 97, jni_get_long_field); // GetLongField
    put!(t, 98, jni_get_float_field); // GetFloatField
    put!(t, 99, jni_get_double_field); // GetDoubleField
    put!(t, 100, jni_set_object_field); // SetObjectField
    put!(t, 101, jni_set_boolean_field); // SetBooleanField
    put!(t, 102, jni_set_byte_field); // SetByteField
    put!(t, 103, jni_set_char_field); // SetCharField
    put!(t, 104, jni_set_short_field); // SetShortField
    put!(t, 105, jni_set_int_field); // SetIntField
    put!(t, 106, jni_set_long_field); // SetLongField
    put!(t, 107, jni_set_float_field); // SetFloatField
    put!(t, 108, jni_set_double_field); // SetDoubleField
    put!(t, 109, jni_get_static_method_id); // GetStaticMethodID

    put!(t, 140, jni_get_static_field_id); // GetStaticFieldID
    put!(t, 141, jni_get_static_object_field); // GetStaticObjectField
    put!(t, 142, jni_get_static_boolean_field); // GetStaticBooleanField
    put!(t, 143, jni_get_static_byte_field); // GetStaticByteField
    put!(t, 144, jni_get_static_char_field); // GetStaticCharField
    put!(t, 145, jni_get_static_short_field); // GetStaticShortField
    put!(t, 146, jni_get_static_int_field); // GetStaticIntField
    put!(t, 147, jni_get_static_long_field); // GetStaticLongField
    put!(t, 148, jni_get_static_float_field); // GetStaticFloatField
    put!(t, 149, jni_get_static_double_field); // GetStaticDoubleField
    put!(t, 150, jni_set_static_object_field); // SetStaticObjectField
    put!(t, 151, jni_set_static_boolean_field); // SetStaticBooleanField
    put!(t, 152, jni_set_static_byte_field); // SetStaticByteField
    put!(t, 153, jni_set_static_char_field); // SetStaticCharField
    put!(t, 154, jni_set_static_short_field); // SetStaticShortField
    put!(t, 155, jni_set_static_int_field); // SetStaticIntField
    put!(t, 156, jni_set_static_long_field); // SetStaticLongField
    put!(t, 157, jni_set_static_float_field); // SetStaticFloatField
    put!(t, 158, jni_set_static_double_field); // SetStaticDoubleField

    put!(t, 159, jni_new_string); // NewString
    put!(t, 160, jni_get_string_length); // GetStringLength
    put!(t, 161, jni_get_string_chars); // GetStringChars
    put!(t, 162, jni_release_string_chars); // ReleaseStringChars
    put!(t, 163, jni_new_string_utf); // NewStringUTF
    put!(t, 164, jni_get_string_utf_length); // GetStringUTFLength
    put!(t, 165, jni_get_string_utf_chars); // GetStringUTFChars
    put!(t, 166, jni_release_string_utf_chars); // ReleaseStringUTFChars
    put!(t, 167, jni_get_array_length); // GetArrayLength
    put!(t, 168, jni_new_object_array); // NewObjectArray
    put!(t, 169, jni_get_object_array_element); // GetObjectArrayElement
    put!(t, 170, jni_set_object_array_element); // SetObjectArrayElement

    // Primitive array constructors: Boolean(171) Byte(172) Char(173) Short(174)
    // Int(175) Long(176) Float(177) Double(178)
    put!(t, 171, jni_new_boolean_array);
    put!(t, 172, jni_new_byte_array);
    put!(t, 173, jni_new_char_array);
    put!(t, 174, jni_new_short_array);
    put!(t, 175, jni_new_int_array);
    put!(t, 176, jni_new_long_array);
    put!(t, 177, jni_new_float_array);
    put!(t, 178, jni_new_double_array);

    // GetXxxArrayElements: Boolean(179)..Double(186)
    put!(t, 179, jni_get_boolean_array_elements);
    put!(t, 180, jni_get_byte_array_elements);
    put!(t, 181, jni_get_char_array_elements);
    put!(t, 182, jni_get_short_array_elements);
    put!(t, 183, jni_get_int_array_elements);
    put!(t, 184, jni_get_long_array_elements);
    put!(t, 185, jni_get_float_array_elements);
    put!(t, 186, jni_get_double_array_elements);
    // ReleaseXxxArrayElements: 187..194
    put!(t, 187, jni_release_boolean_array_elements);
    put!(t, 188, jni_release_byte_array_elements);
    put!(t, 189, jni_release_char_array_elements);
    put!(t, 190, jni_release_short_array_elements);
    put!(t, 191, jni_release_int_array_elements);
    put!(t, 192, jni_release_long_array_elements);
    put!(t, 193, jni_release_float_array_elements);
    put!(t, 194, jni_release_double_array_elements);
    // GetXxxArrayRegion: 195..202
    put!(t, 195, jni_get_boolean_array_region);
    put!(t, 196, jni_get_byte_array_region);
    put!(t, 197, jni_get_char_array_region);
    put!(t, 198, jni_get_short_array_region);
    put!(t, 199, jni_get_int_array_region);
    put!(t, 200, jni_get_long_array_region);
    put!(t, 201, jni_get_float_array_region);
    put!(t, 202, jni_get_double_array_region);
    // SetXxxArrayRegion: 203..210
    put!(t, 203, jni_set_boolean_array_region);
    put!(t, 204, jni_set_byte_array_region);
    put!(t, 205, jni_set_char_array_region);
    put!(t, 206, jni_set_short_array_region);
    put!(t, 207, jni_set_int_array_region);
    put!(t, 208, jni_set_long_array_region);
    put!(t, 209, jni_set_float_array_region);
    put!(t, 210, jni_set_double_array_region);

    put!(t, 211, jni_register_natives); // RegisterNatives
    put!(t, 212, jni_unregister_natives); // UnregisterNatives
    put!(t, 213, jni_monitor_enter); // MonitorEnter
    put!(t, 214, jni_monitor_exit); // MonitorExit
    put!(t, 215, jni_get_java_vm); // GetJavaVM
    put!(t, 216, jni_get_string_region); // GetStringRegion
    put!(t, 217, jni_get_string_utf_region); // GetStringUTFRegion
    put!(t, 218, jni_get_primitive_array_critical); // GetPrimitiveArrayCritical
    put!(t, 219, jni_release_primitive_array_critical); // ReleasePrimitiveArrayCritical
    put!(t, 220, jni_get_string_critical); // GetStringCritical
    put!(t, 221, jni_release_string_critical); // ReleaseStringCritical
    put!(t, 222, jni_new_weak_global_ref); // NewWeakGlobalRef
    put!(t, 223, jni_delete_weak_global_ref); // DeleteWeakGlobalRef
    put!(t, 224, jni_exception_check); // ExceptionCheck
    put!(t, 225, jni_new_direct_byte_buffer); // NewDirectByteBuffer
    put!(t, 226, jni_get_direct_buffer_address); // GetDirectBufferAddress
    put!(t, 227, jni_get_direct_buffer_capacity); // GetDirectBufferCapacity
    put!(t, 228, jni_get_object_ref_type); // GetObjectRefType
    put!(t, 229, jni_get_module); // GetModule
    put!(t, 230, jni_is_virtual_thread); // IsVirtualThread
    put!(t, 231, jni_get_string_utf_length_as_long); // GetStringUTFLengthAsLong

    t
}

fn call_v_fn(kind: u64, ret: RetKind) -> usize {
    match (kind, ret) {
        (VAR_CALL_INSTANCE, RetKind::Gp) => call_v_instance_gp as usize,
        (VAR_CALL_INSTANCE, RetKind::Float) => call_v_instance_f32 as usize,
        (VAR_CALL_INSTANCE, RetKind::Double) => call_v_instance_f64 as usize,
        (VAR_CALL_NONVIRTUAL, RetKind::Gp) => call_v_nonvirtual_gp as usize,
        (VAR_CALL_NONVIRTUAL, RetKind::Float) => call_v_nonvirtual_f32 as usize,
        (VAR_CALL_NONVIRTUAL, RetKind::Double) => call_v_nonvirtual_f64 as usize,
        (VAR_CALL_STATIC, RetKind::Gp) => call_v_static_gp as usize,
        (VAR_CALL_STATIC, RetKind::Float) => call_v_static_f32 as usize,
        (VAR_CALL_STATIC, RetKind::Double) => call_v_static_f64 as usize,
        _ => unsupported_native as usize,
    }
}

fn call_a_fn(kind: u64, ret: RetKind) -> usize {
    match (kind, ret) {
        (VAR_CALL_INSTANCE, RetKind::Gp) => call_a_instance_gp as usize,
        (VAR_CALL_INSTANCE, RetKind::Float) => call_a_instance_f32 as usize,
        (VAR_CALL_INSTANCE, RetKind::Double) => call_a_instance_f64 as usize,
        (VAR_CALL_NONVIRTUAL, RetKind::Gp) => call_a_nonvirtual_gp as usize,
        (VAR_CALL_NONVIRTUAL, RetKind::Float) => call_a_nonvirtual_f32 as usize,
        (VAR_CALL_NONVIRTUAL, RetKind::Double) => call_a_nonvirtual_f64 as usize,
        (VAR_CALL_STATIC, RetKind::Gp) => call_a_static_gp as usize,
        (VAR_CALL_STATIC, RetKind::Float) => call_a_static_f32 as usize,
        (VAR_CALL_STATIC, RetKind::Double) => call_a_static_f64 as usize,
        _ => unsupported_native as usize,
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn cstr_bytes(ptr: *const c_char) -> Option<&'static [u8]> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok().map(|s| {
        // Leak-free approach is impossible for arbitrary C strings; copy into
        // a thread-local cache is overkill, so return the borrowed bytes with
        // an unbounded lifetime (the DLL keeps them alive during the call).
        unsafe { std::mem::transmute::<&[u8], &'static [u8]>(s.as_bytes()) }
    })
}

fn pending(env: *mut NativeEnvironment) -> bool {
    !env.is_null()
        && !matches!(
            unsafe { &*env }.exception.borrow().clone(),
            PendingException::None
        )
}

fn set_pending(env: *mut NativeEnvironment, exc: PendingException) {
    if !env.is_null() {
        *unsafe { &*env }.exception.borrow_mut() = exc;
    }
}

fn throw_error(env: *mut NativeEnvironment, class: &[u8], message: Option<&str>) {
    set_pending(
        env,
        PendingException::New {
            class: class.to_vec(),
            message: message.map(str::to_owned),
        },
    );
}

fn clear_pending(env: *mut NativeEnvironment) {
    if !env.is_null() {
        *unsafe { &*env }.exception.borrow_mut() = PendingException::None;
    }
}

fn io_to_pending(env: *mut NativeEnvironment, error: &Error) {
    let msg = error.to_string();
    let (class, rest) = if let Some((c, m)) = msg.split_once(": ") {
        (c.as_bytes().to_vec(), Some(m.to_string()))
    } else {
        (msg.as_bytes().to_vec(), None)
    };
    // Normalize "java.lang.X" style names produced by the interpreter.
    let class = if class.starts_with(b"java.lang.") {
        class
            .iter()
            .map(|&b| if b == b'.' { b'/' } else { b })
            .collect::<Vec<u8>>()
    } else {
        class
    };
    set_pending(
        env,
        PendingException::New {
            class: if class.is_empty() {
                b"java/lang/Error".to_vec()
            } else {
                class
            },
            message: rest,
        },
    );
}

fn has_pending_or(env: *mut NativeEnvironment) -> bool {
    pending(env)
}

/// Resolves a jobject token to its referent.
fn resolve_target(env: *mut NativeEnvironment, token: *mut c_void) -> io::Result<JniRefTarget> {
    let vm = unsafe { vm_of(env) }.ok_or_else(|| Error::other("Detached JNI environment"))?;
    let reference = JniReference::fromRaw(token);
    vm.references().resolveTarget(reference)
}

fn resolve_oop(env: *mut NativeEnvironment, token: *mut c_void) -> io::Result<Oop> {
    match resolve_target(env, token)? {
        JniRefTarget::Object(oop) => Ok(oop),
        JniRefTarget::Class(_) => Err(Error::other("Expected object, got class")),
    }
}

/// Creates a local reference for a referent and returns the raw jobject.
fn new_local(env: *mut NativeEnvironment, target: JniRefTarget) -> *mut c_void {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    JniReference::intoRaw(vm.references().newLocalTarget(target).unwrap_or(None))
}

fn new_global(env: *mut NativeEnvironment, target: JniRefTarget, weak: bool) -> *mut c_void {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let result = if weak {
        vm.references().newWeakGlobal(target)
    } else {
        vm.references().newGlobalTarget(target)
    };
    JniReference::intoRaw(result.unwrap_or(None))
}

fn delete_ref(env: *mut NativeEnvironment, token: *mut c_void, kind: RefKind) {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let reference = JniReference::fromRaw(token);
    let _ = match kind {
        RefKind::Local => vm.references().deleteLocal(reference),
        RefKind::Global => vm.references().delete(reference, true),
        RefKind::Weak => vm.references().deleteWeak(reference),
    };
}

#[derive(Clone, Copy)]
enum RefKind {
    Local,
    Global,
    Weak,
}

fn oop_to_value<'a>(loader: &ClassLoader<'a>, oop: Oop) -> Value<'a> {
    match oop {
        Oop::Null => Value::Null,
        Oop::Instance(handle) => Value::Object(handle),
        Oop::TypeArray(handle) => Value::PrimitiveArray(handle),
        Oop::ObjArray(handle) => Value::ObjectArray(handle),
    }
}

fn class_name_of(env: *mut NativeEnvironment, token: *mut c_void) -> io::Result<Vec<u8>> {
    match resolve_target(env, token)? {
        JniRefTarget::Class(id) => {
            let vm =
                unsafe { vm_of(env) }.ok_or_else(|| Error::other("Detached JNI environment"))?;
            let state = vm.state();
            state
                .classes
                .get(id as usize)
                .map(|entry| entry.name.clone())
                .ok_or_else(|| Error::other("Invalid class reference"))
        }
        JniRefTarget::Object(_) => Err(Error::other("Expected class, got object")),
    }
}

fn class_id_of(vm: &JavaVm, name: &[u8]) -> u64 {
    let mut state = vm.state();
    if let Some(&id) = state.class_ids.get(name) {
        return id;
    }
    let id = state.next_id;
    state.next_id += 1;
    state.classes.push(ClassEntry {
        name: name.to_vec(),
        primitive: name.starts_with(b":"),
    });
    state.class_ids.insert(name.to_vec(), id);
    id
}

fn class_token(env: *mut NativeEnvironment, name: &[u8]) -> *mut c_void {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let id = class_id_of(vm, name);
    new_local(env, JniRefTarget::Class(id))
}

fn class_klass<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
) -> io::Result<Option<std::rc::Rc<crate::share::oops::instanceKlass::InstanceKlass<'a>>>> {
    if name.starts_with(b":") {
        return Ok(None);
    }
    loader.load(name).map(Some)
}

/// Converts an interpreter Value into a JNI referent (local ref token).
fn value_to_target(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'static>,
    value: Value<'static>,
) -> io::Result<JniRefTarget> {
    Ok(match value {
        Value::Null | Value::Empty | Value::Output | Value::ReturnAddress(_) => {
            JniRefTarget::Object(Oop::Null)
        }
        Value::Object(handle) => JniRefTarget::Object(Oop::Instance(handle)),
        Value::ObjectArray(handle) => JniRefTarget::Object(Oop::ObjArray(handle)),
        Value::PrimitiveArray(handle) => JniRefTarget::Object(Oop::TypeArray(handle)),
        Value::String(_) | Value::HostString(_) => {
            let materialized =
                crate::share::prims::stringNatives::materialize(loader, value, &mut io::sink(), 0)?
                    .ok_or_else(|| Error::other("Cannot materialize string"))?;
            match materialized {
                Value::Object(handle) => JniRefTarget::Object(Oop::Instance(handle)),
                _ => return Err(Error::other("String materialization failed")),
            }
        }
        Value::ClassMirror(name) => {
            let vm =
                unsafe { vm_of(env) }.ok_or_else(|| Error::other("Detached JNI environment"))?;
            JniRefTarget::Class(class_id_of(vm, name))
        }
        Value::Arguments(args) => {
            // Materialize as Object[] of Strings, mirroring the interpreter.
            let mut array = ObjArrayOop::withComponent(args.len() as i32, b"java/lang/String")?;
            for (i, arg) in args.iter().enumerate() {
                array.elements[i] = Value::HostString(loader.storage.alloc_str(arg));
            }
            let handle = loader.alloc_obj_array(array);
            JniRefTarget::Object(Oop::ObjArray(handle))
        }
        Value::Int(_) | Value::Long(_) | Value::Float(_) | Value::Double(_) => {
            return Err(Error::other("Primitive value is not a JNI reference"));
        }
    })
}

/// Converts a JNI referent token back into an interpreter Value.
fn target_to_value<'a>(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'a>,
    target: JniRefTarget,
) -> io::Result<Value<'a>> {
    Ok(match target {
        JniRefTarget::Object(oop) => oop_to_value(loader, oop),
        JniRefTarget::Class(id) => {
            let vm =
                unsafe { vm_of(env) }.ok_or_else(|| Error::other("Detached JNI environment"))?;
            let name = {
                let state = vm.state();
                state
                    .classes
                    .get(id as usize)
                    .map(|entry| entry.name.clone())
                    .ok_or_else(|| Error::other("Invalid class reference"))?
            };
            Value::ClassMirror(loader.storage.alloc_slice_copy(&name))
        }
    })
}

fn token_to_value<'a>(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'a>,
    token: *mut c_void,
) -> io::Result<Value<'a>> {
    let target = resolve_target(env, token)?;
    target_to_value(env, loader, target)
}

fn value_to_token(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'static>,
    value: Value<'static>,
) -> io::Result<*mut c_void> {
    let target = value_to_target(env, loader, value)?;
    Ok(new_local(env, target))
}

// ---------------------------------------------------------------------------
// Exceptions
// ---------------------------------------------------------------------------

fn materialize_exception(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'static>,
) -> io::Result<Oop> {
    let pending = unsafe { &*env }.exception.borrow().clone();
    match pending {
        PendingException::Object(oop) => Ok(oop),
        PendingException::New { class, message } => {
            let klass = loader.load(&class).map_err(|_| {
                Error::other(format!(
                    "Exception class unavailable: {}",
                    String::from_utf8_lossy(&class)
                ))
            })?;
            crate::share::interpreter::bytecodeInterpreter::initialize(
                loader,
                &klass,
                &mut io::sink(),
                0,
            )?;
            let object = InstanceOop::new(klass)?;
            let handle = { loader.alloc_object(object) };
            if let Some(message) = message {
                // Best-effort detailMessage field.
                let objects = loader.borrow_objects();
                if let Some(obj) = objects.get(handle)
                    && let Ok(index) = obj
                        .klass
                        .instanceField(b"detailMessage", b"Ljava/lang/String;")
                {
                    let text = loader.storage.alloc_str(&message);
                    drop(objects);
                    if let Some(value) = crate::share::prims::stringNatives::materialize(
                        loader,
                        Value::HostString(text),
                        &mut io::sink(),
                        0,
                    )? {
                        let mut objects = loader.borrow_objects_mut();
                        if let Some(obj) = objects.get_mut(handle) {
                            obj.fields[index] = value;
                        }
                    }
                }
            }
            Ok(Oop::Instance(handle))
        }
        PendingException::None => Ok(Oop::Null),
    }
}

/// If a pending exception exists, converts it into the interpreter error
/// channel so the Java frame throws the real Throwable object.
fn take_pending_error(env: *mut NativeEnvironment, loader: &ClassLoader<'static>) -> Option<Error> {
    if !pending(env) {
        return None;
    }
    match materialize_exception(env, loader) {
        Ok(Oop::Instance(handle)) => {
            clear_pending(env);
            Some(Error::other(format!("Uncaught exception Object({handle})")))
        }
        Ok(_) => {
            let pending = unsafe { &*env }.exception.borrow().clone();
            clear_pending(env);
            let desc = match pending {
                PendingException::New { class, message } => format!(
                    "{}: {}",
                    String::from_utf8_lossy(&class),
                    message.unwrap_or_default()
                ),
                _ => "JNI exception".to_string(),
            };
            Some(Error::other(format!("Uncaught exception {desc}")))
        }
        Err(_) => {
            let pending = unsafe { &*env }.exception.borrow().clone();
            clear_pending(env);
            let desc = match pending {
                PendingException::New { class, .. } => String::from_utf8_lossy(&class).into_owned(),
                _ => "JNI exception".to_string(),
            };
            Some(Error::other(format!("Uncaught exception {desc}")))
        }
    }
}

// ---------------------------------------------------------------------------
// Class registry helpers for jclass <-> InstanceKlass
// ---------------------------------------------------------------------------

fn class_entry_name(vm: &JavaVm, id: u64) -> Option<Vec<u8>> {
    vm.state()
        .classes
        .get(id as usize)
        .map(|entry| entry.name.clone())
}

fn jclass_klass<'a>(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'a>,
    token: *mut c_void,
) -> io::Result<Option<std::rc::Rc<crate::share::oops::instanceKlass::InstanceKlass<'a>>>> {
    let name = class_name_of(env, token)?;
    class_klass(loader, &name)
}

fn find_method<'a>(
    klass: &crate::share::oops::instanceKlass::InstanceKlass<'a>,
    name: &[u8],
    sig: &[u8],
    is_static: bool,
) -> bool {
    let mut current = Some(klass);
    while let Some(k) = current {
        if k.class.methods.iter().any(|method| {
            method.name == name
                && method.descriptor == sig
                && (method.accessFlags & 0x0008 != 0) == is_static
        }) {
            return true;
        }
        current = k.super_klass.as_deref();
    }
    false
}

fn register_method_id(
    env: *mut NativeEnvironment,
    class_name: Vec<u8>,
    name: Vec<u8>,
    sig: Vec<u8>,
    is_static: bool,
) -> *mut c_void {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let mut state = vm.state();
    let id = state.next_id;
    state.next_id += 1;
    state.methods.insert(
        id,
        JniMethodId {
            class: class_name,
            name,
            sig,
            is_static,
        },
    );
    std::ptr::without_provenance_mut(id as usize)
}

fn method_id_of(env: *mut NativeEnvironment, token: *mut c_void) -> Option<JniMethodId> {
    let vm = unsafe { vm_of(env) }?;
    let id = token.addr() as u64;
    vm.state().methods.get(&id).cloned()
}

fn register_field_id(
    env: *mut NativeEnvironment,
    class_name: Vec<u8>,
    name: Vec<u8>,
    desc: Vec<u8>,
    is_static: bool,
) -> *mut c_void {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let mut state = vm.state();
    let id = state.next_id;
    state.next_id += 1;
    state.fields.insert(
        id,
        JniFieldId {
            class: class_name,
            name,
            desc,
            is_static,
        },
    );
    std::ptr::without_provenance_mut(id as usize)
}

fn field_id_of(env: *mut NativeEnvironment, token: *mut c_void) -> Option<JniFieldId> {
    let vm = unsafe { vm_of(env) }?;
    let id = token.addr() as u64;
    vm.state().fields.get(&id).cloned()
}

// ---------------------------------------------------------------------------
// Invocation functions (fixed arity)
// ---------------------------------------------------------------------------

unsafe extern "system" fn jni_get_version(_env: *mut NativeEnvironment) -> i32 {
    JNI_VERSION_25
}

unsafe extern "system" fn jni_define_class(
    env: *mut NativeEnvironment,
    _name: *const c_char,
    _loader: JniObject,
    _buf: *const u8,
    _len: i32,
) -> JniClass {
    if pending(env) {
        return ptr::null_mut();
    }
    ptr::null_mut()
}

unsafe extern "system" fn jni_find_class(
    env: *mut NativeEnvironment,
    name: *const c_char,
) -> JniClass {
    if pending(env) {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        throw_error(
            env,
            b"java/lang/NoClassDefFoundError",
            Some("no class loader bound"),
        );
        return ptr::null_mut();
    };
    let Some(name) = cstr_bytes(name) else {
        throw_error(env, b"java/lang/NoClassDefFoundError", Some("invalid name"));
        return ptr::null_mut();
    };
    match loader.load(name) {
        Ok(_) => class_token(env, name),
        Err(_) => {
            throw_error(
                env,
                b"java/lang/NoClassDefFoundError",
                Some(&String::from_utf8_lossy(name)),
            );
            ptr::null_mut()
        }
    }
}

unsafe extern "system" fn jni_from_reflected_method(
    _env: *mut NativeEnvironment,
    _method: JniObject,
) -> JniMethodID {
    ptr::null_mut()
}

unsafe extern "system" fn jni_from_reflected_field(
    _env: *mut NativeEnvironment,
    _field: JniObject,
) -> JniFieldID {
    ptr::null_mut()
}

unsafe extern "system" fn jni_to_reflected_method(
    _env: *mut NativeEnvironment,
    _cls: JniClass,
    _methodID: JniMethodID,
    _isStatic: u8,
) -> JniObject {
    ptr::null_mut()
}

unsafe extern "system" fn jni_get_superclass(
    env: *mut NativeEnvironment,
    sub: JniClass,
) -> JniClass {
    if pending(env) {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let Ok(Some(klass)) = jclass_klass(env, loader, sub) else {
        return ptr::null_mut();
    };
    match &klass.super_klass {
        Some(super_k) => class_token(env, super_k.class.name),
        None => ptr::null_mut(),
    }
}

unsafe extern "system" fn jni_is_assignable_from(
    env: *mut NativeEnvironment,
    sub: JniClass,
    sup: JniClass,
) -> u8 {
    if pending(env) {
        return 0;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    let (Ok(sub_name), Ok(sup_name)) = (class_name_of(env, sub), class_name_of(env, sup)) else {
        return 0;
    };
    if sub_name == sup_name {
        return 1;
    }
    if sub_name.starts_with(b":") || sup_name.starts_with(b":") {
        return u8::from(sub_name == sup_name);
    }
    let assignable =
        (|| -> io::Result<bool> {
            let sub_k = loader.load(&sub_name)?;
            Ok(crate::share::classfile::systemDictionary::SystemDictionary::is_subclass_of(
            &sub_k, &sup_name,
        ) || crate::share::classfile::systemDictionary::SystemDictionary::implements_interface(
            &sub_k, &sup_name,
        ))
        })()
        .unwrap_or(false);
    u8::from(assignable)
}

unsafe extern "system" fn jni_to_reflected_field(
    _env: *mut NativeEnvironment,
    _cls: JniClass,
    _fieldID: JniFieldID,
    _isStatic: u8,
) -> JniObject {
    ptr::null_mut()
}

unsafe extern "system" fn jni_throw(env: *mut NativeEnvironment, obj: JniThrowable) -> i32 {
    if obj.is_null() {
        return JNI_ERR;
    }
    match resolve_oop(env, obj) {
        Ok(oop) => {
            set_pending(env, PendingException::Object(oop));
            JNI_OK
        }
        Err(_) => JNI_ERR,
    }
}

unsafe extern "system" fn jni_throw_new(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    message: *const c_char,
) -> i32 {
    if pending(env) {
        return JNI_ERR;
    }
    let Ok(class) = class_name_of(env, clazz) else {
        return JNI_ERR;
    };
    let msg = if message.is_null() {
        None
    } else {
        cstr_bytes(message).map(|b| String::from_utf8_lossy(b).into_owned())
    };
    set_pending(
        env,
        PendingException::New {
            class,
            message: msg,
        },
    );
    JNI_OK
}

unsafe extern "system" fn jni_exception_occurred(env: *mut NativeEnvironment) -> JniThrowable {
    if !pending(env) {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    match materialize_exception(env, loader) {
        Ok(oop) if !oop.is_null() => new_local(env, JniRefTarget::Object(oop)),
        _ => ptr::null_mut(),
    }
}

unsafe extern "system" fn jni_exception_describe(env: *mut NativeEnvironment) {
    if !pending(env) {
        return;
    }
    let pending = unsafe { &*env }.exception.borrow().clone();
    match pending {
        PendingException::Object(_) => eprintln!("JNI exception pending (object)"),
        PendingException::New { class, message } => {
            eprintln!(
                "Exception in thread \"main\" {}: {}",
                String::from_utf8_lossy(&class).replace('/', "."),
                message.unwrap_or_default()
            );
        }
        PendingException::None => {}
    }
}

unsafe extern "system" fn jni_exception_clear(env: *mut NativeEnvironment) {
    clear_pending(env);
}

unsafe extern "system" fn jni_fatal_error(env: *mut NativeEnvironment, message: *const c_char) {
    let msg = cstr_bytes(message)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_else(|| "JNI FatalError".to_string());
    eprintln!("JNI FatalError: {msg}");
    let _ = env;
    std::process::abort();
}

unsafe extern "system" fn jni_push_local_frame(env: *mut NativeEnvironment, capacity: i32) -> i32 {
    if capacity < 0 {
        return JNI_ERR;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return JNI_EINVAL;
    };
    vm.references().pushFrame();
    if vm.references().ensureCapacity(capacity as usize).is_err() {
        let _ = vm.references().popFrameVoid();
        return JNI_ERR;
    }
    JNI_OK
}

unsafe extern "system" fn jni_pop_local_frame(
    env: *mut NativeEnvironment,
    result: *mut c_void,
) -> *mut c_void {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let reference = JniReference::fromRaw(result);
    match vm.references().popFrame(reference) {
        Ok(popped) => JniReference::intoRaw(popped),
        Err(_) => {
            let _ = vm.references().popFrameVoid();
            JniReference::intoRaw(reference)
        }
    }
}

unsafe extern "system" fn jni_new_global_ref(
    env: *mut NativeEnvironment,
    obj: JniObject,
) -> JniObject {
    if obj.is_null() {
        return ptr::null_mut();
    }
    match resolve_target(env, obj) {
        Ok(target) => new_global(env, target, false),
        Err(_) => ptr::null_mut(),
    }
}

unsafe extern "system" fn jni_delete_global_ref(env: *mut NativeEnvironment, gref: JniObject) {
    delete_ref(env, gref, RefKind::Global);
}

unsafe extern "system" fn jni_delete_local_ref(env: *mut NativeEnvironment, ref_: JniObject) {
    delete_ref(env, ref_, RefKind::Local);
}

unsafe extern "system" fn jni_is_same_object(
    env: *mut NativeEnvironment,
    obj1: JniObject,
    obj2: JniObject,
) -> u8 {
    if obj1.is_null() && obj2.is_null() {
        return 1;
    }
    if obj1.is_null() || obj2.is_null() {
        return 0;
    }
    match (resolve_target(env, obj1), resolve_target(env, obj2)) {
        (Ok(a), Ok(b)) => u8::from(a == b),
        _ => 0,
    }
}

unsafe extern "system" fn jni_new_local_ref(
    env: *mut NativeEnvironment,
    ref_: JniObject,
) -> JniObject {
    if ref_.is_null() {
        return ptr::null_mut();
    }
    match resolve_target(env, ref_) {
        Ok(target) => new_local(env, target),
        Err(_) => ptr::null_mut(),
    }
}

unsafe extern "system" fn jni_ensure_local_capacity(
    env: *mut NativeEnvironment,
    capacity: i32,
) -> i32 {
    if capacity < 0 {
        return JNI_ERR;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return JNI_EINVAL;
    };
    if vm.references().ensureCapacity(capacity as usize).is_ok() {
        JNI_OK
    } else {
        JNI_ERR
    }
}

unsafe extern "system" fn jni_alloc_object(
    env: *mut NativeEnvironment,
    clazz: JniClass,
) -> JniObject {
    if pending(env) {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let Ok(Some(klass)) = jclass_klass(env, loader, clazz) else {
        return ptr::null_mut();
    };
    match InstanceOop::new(klass) {
        Ok(object) => {
            let handle = loader.alloc_object(object);
            new_local(env, JniRefTarget::Object(Oop::Instance(handle)))
        }
        Err(_) => ptr::null_mut(),
    }
}

unsafe extern "system" fn jni_get_object_class(
    env: *mut NativeEnvironment,
    obj: JniObject,
) -> JniClass {
    if obj.is_null() || pending(env) {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let name: Vec<u8> = match resolve_target(env, obj) {
        Ok(JniRefTarget::Object(oop)) => match oop {
            Oop::Instance(handle) => loader
                .objects
                .borrow()
                .get(handle)
                .map(|o| o.klass.class.name.to_vec())
                .unwrap_or_default(),
            Oop::TypeArray(handle) => loader
                .arrays
                .borrow()
                .get(handle)
                .map(|a| a.descriptor().to_vec())
                .unwrap_or_default(),
            Oop::ObjArray(handle) => {
                let component = loader
                    .obj_arrays
                    .borrow()
                    .get(handle)
                    .map(|a| a.component)
                    .unwrap_or(b"java/lang/Object");
                let mut name = Vec::with_capacity(component.len() + 3);
                name.push(b'[');
                if component.starts_with(b"[") {
                    name.extend_from_slice(component);
                } else {
                    name.push(b'L');
                    name.extend_from_slice(component);
                    name.push(b';');
                }
                name
            }
            Oop::Null => return ptr::null_mut(),
        },
        Ok(JniRefTarget::Class(id)) => class_entry_name(vm, id).unwrap_or_default(),
        Err(_) => return ptr::null_mut(),
    };
    if name.is_empty() {
        return ptr::null_mut();
    }
    class_token(env, &name)
}

unsafe extern "system" fn jni_is_instance_of(
    env: *mut NativeEnvironment,
    obj: JniObject,
    clazz: JniClass,
) -> u8 {
    if obj.is_null() {
        return 0;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    let Ok(target_name) = class_name_of(env, clazz) else {
        return 0;
    };
    let value = match token_to_value(env, loader, obj) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    loader
        .referenceAssignable(value, &target_name)
        .unwrap_or(false) as u8
}

unsafe extern "system" fn jni_get_method_id(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    name: *const c_char,
    sig: *const c_char,
) -> JniMethodID {
    jni_get_method_id_impl(env, clazz, name, sig, false)
}

unsafe extern "system" fn jni_get_static_method_id(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    name: *const c_char,
    sig: *const c_char,
) -> JniMethodID {
    jni_get_method_id_impl(env, clazz, name, sig, true)
}

unsafe fn jni_get_method_id_impl(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    name: *const c_char,
    sig: *const c_char,
    is_static: bool,
) -> JniMethodID {
    if pending(env) {
        return ptr::null_mut();
    }
    let (Some(name), Some(sig)) = (cstr_bytes(name), cstr_bytes(sig)) else {
        return ptr::null_mut();
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let Ok(class_name) = class_name_of(env, clazz) else {
        return ptr::null_mut();
    };
    match class_klass(loader, &class_name) {
        Ok(Some(klass)) => {
            if find_method(&klass, name, sig, is_static) {
                register_method_id(env, class_name, name.to_vec(), sig.to_vec(), is_static)
            } else {
                throw_error(env, b"java/lang/NoSuchMethodError", None);
                ptr::null_mut()
            }
        }
        _ => {
            throw_error(env, b"java/lang/NoSuchMethodError", None);
            ptr::null_mut()
        }
    }
}

unsafe extern "system" fn jni_get_field_id(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    name: *const c_char,
    sig: *const c_char,
) -> JniFieldID {
    jni_get_field_id_impl(env, clazz, name, sig, false)
}

unsafe extern "system" fn jni_get_static_field_id(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    name: *const c_char,
    sig: *const c_char,
) -> JniFieldID {
    jni_get_field_id_impl(env, clazz, name, sig, true)
}

unsafe fn jni_get_field_id_impl(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    name: *const c_char,
    sig: *const c_char,
    is_static: bool,
) -> JniFieldID {
    if pending(env) {
        return ptr::null_mut();
    }
    let (Some(name), Some(sig)) = (cstr_bytes(name), cstr_bytes(sig)) else {
        return ptr::null_mut();
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let Ok(class_name) = class_name_of(env, clazz) else {
        return ptr::null_mut();
    };
    let found = match class_klass(loader, &class_name) {
        Ok(Some(klass)) => {
            if is_static {
                klass.staticField(name, sig).is_ok()
            } else {
                klass.instanceField(name, sig).is_ok()
            }
        }
        _ => false,
    };
    if found {
        register_field_id(env, class_name, name.to_vec(), sig.to_vec(), is_static)
    } else {
        throw_error(env, b"java/lang/NoSuchFieldError", None);
        ptr::null_mut()
    }
}

// ---------------------------------------------------------------------------
// Field access
// ---------------------------------------------------------------------------

fn get_instance_field(env: *mut NativeEnvironment, obj: JniObject, id: JniFieldID) -> u64 {
    let Some(field) = field_id_of(env, id) else {
        return 0;
    };
    if field.is_static {
        return 0;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    let Ok(oop) = resolve_oop(env, obj) else {
        return 0;
    };
    let Oop::Instance(handle) = oop else {
        return 0;
    };
    let objects = loader.borrow_objects();
    let Some(object) = objects.get(handle) else {
        return 0;
    };
    let Ok(slot) = object.klass.instanceField(&field.name, &field.desc) else {
        return 0;
    };
    let value = object.fields.get(slot).copied().unwrap_or(Value::Null);
    drop(objects);
    value_bits_g2j(value)
}

fn set_instance_field(
    env: *mut NativeEnvironment,
    obj: JniObject,
    id: JniFieldID,
    raw: u64,
    kind: u8,
) {
    let Some(field) = field_id_of(env, id) else {
        return;
    };
    if field.is_static {
        return;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let Some(loader) = vm.class_loader() else {
        return;
    };
    let Ok(oop) = resolve_oop(env, obj) else {
        return;
    };
    let Oop::Instance(handle) = oop else {
        return;
    };
    let value = raw_to_value(env, loader, &field.desc, raw, kind);
    let mut objects = loader.borrow_objects_mut();
    if let Some(object) = objects.get_mut(handle)
        && let Ok(slot) = object.klass.instanceField(&field.name, &field.desc)
    {
        object.fields[slot] = value;
    }
}

fn get_static_field(env: *mut NativeEnvironment, id: JniFieldID) -> u64 {
    let Some(field) = field_id_of(env, id) else {
        return 0;
    };
    if !field.is_static {
        return 0;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    let Ok(klass) = loader.load(&field.class) else {
        return 0;
    };
    let Ok(cell) = klass.staticField(&field.name, &field.desc) else {
        return 0;
    };
    value_bits_g2j(cell.get())
}

fn set_static_field(env: *mut NativeEnvironment, id: JniFieldID, raw: u64, kind: u8) {
    let Some(field) = field_id_of(env, id) else {
        return;
    };
    if !field.is_static {
        return;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let Some(loader) = vm.class_loader() else {
        return;
    };
    let Ok(klass) = loader.load(&field.class) else {
        return;
    };
    let Ok(cell) = klass.staticField(&field.name, &field.desc) else {
        return;
    };
    let value = raw_to_value(env, loader, &field.desc, raw, kind);
    cell.set(value);
}

/// kind: 0=int family, 1=long, 2=float, 3=double, 4=reference
fn raw_to_value<'a>(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'a>,
    desc: &[u8],
    raw: u64,
    kind: u8,
) -> Value<'a> {
    let _ = desc;
    match kind {
        1 => Value::Long(raw as i64),
        2 => Value::Float(f32::from_bits(raw as u32)),
        3 => Value::Double(f64::from_bits(raw)),
        4 => {
            // raw is a jobject token
            match resolve_target(env, raw as *mut c_void) {
                Ok(target) => target_to_value(env, loader, target).unwrap_or(Value::Null),
                Err(_) => Value::Null,
            }
        }
        _ => Value::Int(raw as u32 as i32),
    }
}

/// Converts an interpreter Value into raw bits for GP/float returns.
/// For reference kinds the caller must use value_to_token instead.
fn value_bits_g2j(value: Value<'_>) -> u64 {
    match value {
        Value::Int(v) => v as u32 as u64,
        Value::Long(v) => v as u64,
        Value::Float(f) => f.to_bits() as u64,
        Value::Double(d) => d.to_bits(),
        Value::Null => 0,
        _ => 0, // reference handled separately
    }
}

fn value_bits_ref(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'static>,
    value: Value<'static>,
) -> u64 {
    match value_to_target(env, loader, value) {
        Ok(target) => new_local(env, target) as usize as u64,
        Err(_) => 0,
    }
}

macro_rules! field_getters {
    ($name:ident, $static_:literal, $kind:expr) => {
        unsafe extern "system" fn $name(
            env: *mut NativeEnvironment,
            obj: JniObject,
            id: JniFieldID,
        ) -> u64 {
            if pending(env) {
                return 0;
            }
            if $static_ {
                get_static_field(env, id)
            } else {
                get_instance_field(env, obj, id)
            }
        }
    };
}

// Object field getters (instance). Return types differ per JNI type, but all
// GP results travel in RAX; float/double need their own entries below.
unsafe extern "system" fn jni_get_object_field(
    env: *mut NativeEnvironment,
    obj: JniObject,
    id: JniFieldID,
) -> JniObject {
    if pending(env) {
        return ptr::null_mut();
    }
    let Some(field) = field_id_of(env, id) else {
        return ptr::null_mut();
    };
    if field.is_static {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let Ok(oop) = resolve_oop(env, obj) else {
        return ptr::null_mut();
    };
    let Oop::Instance(handle) = oop else {
        return ptr::null_mut();
    };
    let value = {
        let objects = loader.borrow_objects();
        let Some(object) = objects.get(handle) else {
            return ptr::null_mut();
        };
        let Ok(slot) = object.klass.instanceField(&field.name, &field.desc) else {
            return ptr::null_mut();
        };
        object.fields.get(slot).copied().unwrap_or(Value::Null)
    };
    value_to_token(env, loader, value).unwrap_or(ptr::null_mut())
}

unsafe extern "system" fn jni_get_static_object_field(
    env: *mut NativeEnvironment,
    id: JniFieldID,
) -> JniObject {
    if pending(env) {
        return ptr::null_mut();
    }
    let Some(field) = field_id_of(env, id) else {
        return ptr::null_mut();
    };
    if !field.is_static {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let Ok(klass) = loader.load(&field.class) else {
        return ptr::null_mut();
    };
    let Ok(cell) = klass.staticField(&field.name, &field.desc) else {
        return ptr::null_mut();
    };
    let value = cell.get();
    value_to_token(env, loader, value).unwrap_or(ptr::null_mut())
}

macro_rules! scalar_field_accessors {
    ($get:ident, $set:ident, $get_ret:ty, $set_val:ty, $static_get:ident, $static_set:ident, $kind:expr) => {
        unsafe extern "system" fn $get(
            env: *mut NativeEnvironment,
            obj: JniObject,
            id: JniFieldID,
        ) -> $get_ret {
            if pending(env) {
                return 0 as $get_ret;
            }
            get_instance_field(env, obj, id) as $get_ret
        }
        unsafe extern "system" fn $set(
            env: *mut NativeEnvironment,
            obj: JniObject,
            id: JniFieldID,
            value: $set_val,
        ) {
            if pending(env) {
                return;
            }
            set_instance_field(env, obj, id, value as u64, $kind);
        }
        unsafe extern "system" fn $static_get(
            env: *mut NativeEnvironment,
            id: JniFieldID,
        ) -> $get_ret {
            if pending(env) {
                return 0 as $get_ret;
            }
            get_static_field(env, id) as $get_ret
        }
        unsafe extern "system" fn $static_set(
            env: *mut NativeEnvironment,
            id: JniFieldID,
            value: $set_val,
        ) {
            if pending(env) {
                return;
            }
            set_static_field(env, id, value as u64, $kind);
        }
    };
}

scalar_field_accessors!(
    jni_get_boolean_field,
    jni_set_boolean_field,
    u8,
    u8,
    jni_get_static_boolean_field,
    jni_set_static_boolean_field,
    0
);
scalar_field_accessors!(
    jni_get_byte_field,
    jni_set_byte_field,
    i8,
    i8,
    jni_get_static_byte_field,
    jni_set_static_byte_field,
    0
);
scalar_field_accessors!(
    jni_get_char_field,
    jni_set_char_field,
    u16,
    u16,
    jni_get_static_char_field,
    jni_set_static_char_field,
    0
);
scalar_field_accessors!(
    jni_get_short_field,
    jni_set_short_field,
    i16,
    i16,
    jni_get_static_short_field,
    jni_set_static_short_field,
    0
);
scalar_field_accessors!(
    jni_get_int_field,
    jni_set_int_field,
    i32,
    i32,
    jni_get_static_int_field,
    jni_set_static_int_field,
    0
);
scalar_field_accessors!(
    jni_get_long_field,
    jni_set_long_field,
    i64,
    i64,
    jni_get_static_long_field,
    jni_set_static_long_field,
    1
);
scalar_field_accessors!(
    jni_get_float_field,
    jni_set_float_field,
    f32,
    f32,
    jni_get_static_float_field,
    jni_set_static_float_field,
    2
);
scalar_field_accessors!(
    jni_get_double_field,
    jni_set_double_field,
    f64,
    f64,
    jni_get_static_double_field,
    jni_set_static_double_field,
    3
);

unsafe extern "system" fn jni_set_object_field(
    env: *mut NativeEnvironment,
    obj: JniObject,
    id: JniFieldID,
    value: JniObject,
) {
    if pending(env) {
        return;
    }
    let Some(field) = field_id_of(env, id) else {
        return;
    };
    if field.is_static {
        return;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let Some(loader) = vm.class_loader() else {
        return;
    };
    let Ok(oop) = resolve_oop(env, obj) else {
        return;
    };
    let Oop::Instance(handle) = oop else { return };
    let new_value = match resolve_target(env, value) {
        Ok(target) => target_to_value(env, loader, target).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let mut objects = loader.borrow_objects_mut();
    if let Some(object) = objects.get_mut(handle)
        && let Ok(slot) = object.klass.instanceField(&field.name, &field.desc)
    {
        object.fields[slot] = new_value;
    }
}

unsafe extern "system" fn jni_set_static_object_field(
    env: *mut NativeEnvironment,
    id: JniFieldID,
    value: JniObject,
) {
    if pending(env) {
        return;
    }
    let Some(field) = field_id_of(env, id) else {
        return;
    };
    if !field.is_static {
        return;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let Some(loader) = vm.class_loader() else {
        return;
    };
    let Ok(klass) = loader.load(&field.class) else {
        return;
    };
    let Ok(cell) = klass.staticField(&field.name, &field.desc) else {
        return;
    };
    let new_value = match resolve_target(env, value) {
        Ok(target) => target_to_value(env, loader, target).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    cell.set(new_value);
}

// ---------------------------------------------------------------------------
// Method calls
// ---------------------------------------------------------------------------

/// Signature kinds from runtime::signature::method: 0=int family, 1=long,
/// 2=float, 3=double, 4=reference.
fn sig_parts<'a>(sig: &'a [u8]) -> io::Result<(Vec<u8>, Option<u8>)> {
    let (params, ret) = signature::method(sig)?;
    let mut kinds = Vec::new();
    for kind in params {
        kinds.push(kind?);
    }
    Ok((kinds, ret))
}

fn arg_kind_to_jni(kind: u8) -> ArgKind {
    match kind {
        2 => ArgKind::Float,
        3 => ArgKind::Double,
        _ => ArgKind::Gp,
    }
}

fn value_to_slot(value: &Value<'_>, kind: u8) -> u64 {
    match (kind, value) {
        (1, Value::Long(v)) => *v as u64,
        (2, Value::Float(f)) => f.to_bits() as u64,
        (3, Value::Double(d)) => d.to_bits(),
        (4, _) => value_to_slot_ref(value),
        (_, Value::Int(v)) => *v as u32 as u64,
        (_, Value::Long(v)) => *v as u64,
        (_, Value::Float(f)) => f.to_bits() as u64,
        (_, Value::Double(d)) => d.to_bits(),
        _ => 0,
    }
}

fn value_to_slot_ref(value: &Value<'_>) -> u64 {
    // Reference values are converted to tokens by the caller which has env.
    // This fallback is only used for non-reference kinds.
    let _ = value;
    0
}

/// Builds the CallContext for invoking `code` and runs the trampoline.
pub(crate) fn call_native_code(
    env: *mut NativeEnvironment,
    code: &NativeCode,
    receiver: usize,
    args: &[Value<'static>],
    kinds: &[u8],
) -> u64 {
    let mut ctx = CallContext {
        func: code.func,
        env: env as *mut c_void,
        receiver,
        args: [0; 512],
    };
    let n = args.len().min(510);
    for (i, (value, &kind)) in args.iter().zip(kinds.iter()).take(n).enumerate() {
        ctx.args[i] = match kind {
            4 => value_to_token_quiet(env, *value),
            _ => value_to_slot(value, kind),
        };
    }
    let trampoline: unsafe extern "system" fn(*const CallContext) -> u64 =
        unsafe { std::mem::transmute(code.trampoline) };
    unsafe { trampoline(&ctx) }
}

fn value_to_token_quiet(env: *mut NativeEnvironment, value: Value<'static>) -> u64 {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    match value_to_target(env, loader, value) {
        Ok(target) => new_local(env, target) as usize as u64,
        Err(_) => 0,
    }
}

/// Converts raw u64 slots (as produced by varargs/A/V capture) into Values.
fn slots_to_values(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'static>,
    kinds: &[u8],
    slots: &[u64],
) -> Vec<Value<'static>> {
    kinds
        .iter()
        .zip(slots.iter())
        .map(|(&kind, &raw)| match kind {
            1 => Value::Long(raw as i64),
            2 => Value::Float(f32::from_bits(raw as u32)),
            3 => Value::Double(f64::from_bits(raw)),
            4 => {
                let token = raw as *mut c_void;
                match resolve_target(env, token) {
                    Ok(target) => target_to_value(env, loader, target).unwrap_or(Value::Null),
                    Err(_) => Value::Null,
                }
            }
            _ => Value::Int(raw as u32 as i32),
        })
        .collect()
}

/// Core CallXxxMethod implementation shared by varargs/A/V entries.
/// `call_kind`: VAR_CALL_INSTANCE / NONVIRTUAL / STATIC / NEWOBJECT.
/// Returns raw result bits (object results are jobject tokens).
fn invoke_java_method(
    env: *mut NativeEnvironment,
    call_kind: u64,
    receiver: *mut c_void,
    clazz: *mut c_void,
    method: *mut c_void,
    arg_slots: &[u64],
) -> u64 {
    if env.is_null() || pending(env) {
        return 0;
    }
    let Some(method_id) = method_id_of(env, method) else {
        return 0;
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    let Ok((param_kinds, ret_kind)) = sig_parts(&method_id.sig) else {
        return 0;
    };
    let values = slots_to_values(env, loader, &param_kinds, arg_slots);

    // Determine the klass used for resolution and the argument list.
    let mut call_args: Vec<Value<'static>> = Vec::with_capacity(values.len() + 1);
    let result: io::Result<Option<Value<'static>>> = (|| {
        match call_kind {
            VAR_CALL_STATIC => {
                let class_name = if !clazz.is_null() {
                    class_name_of(env, clazz)?
                } else {
                    method_id.class.clone()
                };
                let klass = loader
                    .load(&class_name)
                    .map_err(|_| Error::other("NoClassDefFoundError"))?;
                crate::share::interpreter::bytecodeInterpreter::initialize(
                    loader,
                    &klass,
                    &mut io::sink(),
                    0,
                )?;
                let mut args = JavaCallArguments::new();
                for v in values {
                    args.push_oop(v);
                }
                JavaCalls::call_static(
                    loader,
                    &klass,
                    &method_id.name,
                    &method_id.sig,
                    &args,
                    &mut io::sink(),
                )
            }
            VAR_CALL_NEWOBJECT => {
                let class_name = if !clazz.is_null() {
                    class_name_of(env, clazz)?
                } else {
                    method_id.class.clone()
                };
                let klass = loader
                    .load(&class_name)
                    .map_err(|_| Error::other("NoClassDefFoundError"))?;
                crate::share::interpreter::bytecodeInterpreter::initialize(
                    loader,
                    &klass,
                    &mut io::sink(),
                    0,
                )?;
                let object = InstanceOop::new(klass.clone())?;
                let handle = loader.alloc_object(object);
                let mut args = JavaCallArguments::with_receiver(Value::Object(handle));
                for v in values {
                    args.push_oop(v);
                }
                JavaCalls::call_special(
                    loader,
                    &klass,
                    &method_id.name,
                    &method_id.sig,
                    &args,
                    &mut io::sink(),
                )?;
                return Ok(Some(Value::Object(handle)));
            }
            _ => {
                // instance (virtual) or nonvirtual
                let receiver_value = if receiver.is_null() {
                    Value::Null
                } else {
                    token_to_value(env, loader, receiver)?
                };
                let declaring = if call_kind == VAR_CALL_NONVIRTUAL && !clazz.is_null() {
                    Some(class_name_of(env, clazz)?)
                } else {
                    None
                };
                call_args.push(receiver_value);
                call_args.extend(values);
                let klass = match &receiver_value {
                    Value::Object(handle) => loader
                        .objects
                        .borrow()
                        .get(*handle)
                        .map(|o| std::rc::Rc::clone(&o.klass))
                        .ok_or_else(|| Error::other("Invalid receiver"))?,
                    Value::ObjectArray(handle) => {
                        // Arrays only support clone(); handled by interpreter.
                        return Err(Error::other("AbstractMethodError"));
                    }
                    Value::PrimitiveArray(_) => {
                        return Err(Error::other("AbstractMethodError"));
                    }
                    Value::ClassMirror(name) => loader.load(name)?,
                    _ => loader.load(b"java/lang/Object")?,
                };
                if let Some(name) = declaring {
                    let klass = loader.load(&name)?;
                    crate::share::interpreter::bytecodeInterpreter::initialize(
                        loader,
                        &klass,
                        &mut io::sink(),
                        0,
                    )?;
                    let mut args = JavaCallArguments::new();
                    for v in call_args {
                        args.push_oop(v);
                    }
                    JavaCalls::call_special(
                        loader,
                        &klass,
                        &method_id.name,
                        &method_id.sig,
                        &args,
                        &mut io::sink(),
                    )
                } else {
                    crate::share::interpreter::bytecodeInterpreter::initialize(
                        loader,
                        &klass,
                        &mut io::sink(),
                        0,
                    )?;
                    let mut args = JavaCallArguments::new();
                    for v in call_args {
                        args.push_oop(v);
                    }
                    JavaCalls::call_virtual(
                        loader,
                        &klass,
                        &method_id.name,
                        &method_id.sig,
                        &args,
                        &mut io::sink(),
                    )
                }
            }
        }
    })();

    match result {
        Ok(value) => {
            if let Some(err) = take_pending_error(env, loader) {
                let _ = err;
                return 0;
            }
            let Some(value) = value else {
                return 0;
            };
            match ret_kind {
                None => 0,
                Some(4) => value_to_token_quiet(env, value),
                Some(k) => value_bits_g2j_for_kind(value, k),
            }
        }
        Err(error) => {
            io_to_pending(env, &error);
            // Also try to surface as interpreter error on next boundary.
            0
        }
    }
}

fn value_bits_g2j_for_kind(value: Value<'_>, kind: u8) -> u64 {
    match (kind, value) {
        (1, Value::Long(v)) => v as u64,
        (2, Value::Float(f)) => f.to_bits() as u64,
        (3, Value::Double(d)) => d.to_bits(),
        (_, Value::Int(v)) => v as u32 as u64,
        (_, Value::Long(v)) => v as u64,
        (_, Value::Float(f)) => f.to_bits() as u64,
        (_, Value::Double(d)) => d.to_bits(),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Varargs capture handler (CallXxxMethod..., NewObject)
// ---------------------------------------------------------------------------

unsafe extern "system" fn varargs_handler(ctx: *const VarArgsCtx) -> u64 {
    if ctx.is_null() {
        return 0;
    }
    let ctx = &*ctx;
    let kind = ctx.tag >> 8;
    let env = ctx.gp[0] as *mut NativeEnvironment;
    let receiver = ctx.gp[1] as *mut c_void;
    let method = ctx.gp[2] as *mut c_void;
    let Some(method_id) = method_id_of(env, method) else {
        return 0;
    };
    let (Ok((param_kinds, _)), Some(vm), Some(loader)) = (
        sig_parts(&method_id.sig),
        unsafe { vm_of(env) },
        unsafe { vm_of(env) }.and_then(|vm| vm.class_loader()),
    ) else {
        return 0;
    };
    let _ = loader;

    // Reconstruct argument slots: vararg j sits at ABI position j+3.
    let n = param_kinds.len();
    let mut slots: Vec<u64> = Vec::with_capacity(n);
    for (j, &kind) in param_kinds.iter().enumerate() {
        let pos = j + 3;
        let is_fp = matches!(kind, 2 | 3);
        let raw = if pos < 4 {
            if is_fp { ctx.xmm[pos][0] } else { ctx.gp[pos] }
        } else {
            let offset = 8 * (pos - 4);
            if ctx.stack_args.is_null() {
                0
            } else {
                unsafe { (ctx.stack_args.add(offset) as *const u64).read_unaligned() }
            }
        };
        // Normalize float slots to their low 32 bits.
        let raw = if kind == 2 { raw & 0xffff_ffff } else { raw };
        slots.push(raw);
    }
    let clazz =
        if kind == VAR_CALL_STATIC || kind == VAR_CALL_NEWOBJECT || kind == VAR_CALL_NONVIRTUAL {
            receiver
        } else {
            ptr::null_mut()
        };
    invoke_java_method(env, kind, receiver, clazz, method, &slots)
}

// ---------------------------------------------------------------------------
// CallXxxMethodV / CallXxxMethodA Rust entries
// ---------------------------------------------------------------------------

fn va_arg_slots(va_list: *const u8, kinds: &[u8]) -> Vec<u64> {
    let mut slots = Vec::with_capacity(kinds.len());
    let mut ptr = va_list as usize;
    for &kind in kinds {
        let raw = if ptr == 0 {
            0
        } else {
            unsafe { (ptr as *const u64).read_unaligned() }
        };
        ptr += 8;
        slots.push(if kind == 2 { raw & 0xffff_ffff } else { raw });
    }
    slots
}

fn jvalue_slots(args: *const u64, kinds: &[u8]) -> Vec<u64> {
    let mut slots = Vec::with_capacity(kinds.len());
    for (i, &kind) in kinds.iter().enumerate() {
        let raw = if args.is_null() {
            0
        } else {
            unsafe { args.add(i).read_unaligned() }
        };
        slots.push(if kind == 2 { raw & 0xffff_ffff } else { raw });
    }
    slots
}

macro_rules! call_entries {
    ($v_gp:ident, $v_f32:ident, $v_f64:ident, $a_gp:ident, $a_f32:ident, $a_f64:ident,
     $kind:expr, $has_clazz:expr, $va:expr) => {
        unsafe extern "system" fn $v_gp(env: *mut NativeEnvironment, obj: JniObject, mid: JniMethodID $(, clazz: JniClass)?) -> u64 {
            call_typed_entry(env, obj, mid, $(clazz,)? ArgSource::VaList, $kind)
        }
    };
}

/// Reads `kinds.len()` consecutive 8-byte argument slots starting at `ptr`.
/// Used for both CallXxxMethodA (jvalue array) and CallXxxMethodV (va_list,
/// which on Win64 x64 is a flat sequence of 8-byte slots).
fn slots_from_ptr(kinds: &[u8], ptr: *const u8) -> Vec<u64> {
    kinds
        .iter()
        .enumerate()
        .map(|(i, &kind)| {
            let raw = if ptr.is_null() {
                0
            } else {
                unsafe { (ptr.add(8 * i) as *const u64).read_unaligned() }
            };
            if kind == 2 { raw & 0xffff_ffff } else { raw }
        })
        .collect()
}

/// Shared dispatcher for CallXxxMethodV/A and NewObjectV/A.
/// `ref_arg` is the jobject (instance) or jclass (static/nonvirtual/newobject)
/// parameter; `clazz_arg` is only used by the nonvirtual family.
fn typed_dispatch(
    env: *mut NativeEnvironment,
    kind: u64,
    ref_arg: *mut c_void,
    clazz_arg: *mut c_void,
    method: *mut c_void,
    args_ptr: *const u8,
) -> u64 {
    if env.is_null() || pending(env) {
        return 0;
    }
    let Some(method_id) = method_id_of(env, method) else {
        return 0;
    };
    let Ok((kinds, _)) = sig_parts(&method_id.sig) else {
        return 0;
    };
    let slots = slots_from_ptr(&kinds, args_ptr);
    let (receiver, clazz) = match kind {
        VAR_CALL_INSTANCE => (ref_arg, ptr::null_mut()),
        VAR_CALL_STATIC | VAR_CALL_NEWOBJECT => (ptr::null_mut(), ref_arg),
        _ => (ref_arg, clazz_arg),
    };
    invoke_java_method(env, kind, receiver, clazz, method, &slots)
}

/// Generates the V and A entries for one Call family. V and A share the same
/// slot reader on Win64, so both table entries point at the same function.
macro_rules! call_family {
    ($v_gp:ident, $f32:ident, $f64:ident, $kind:expr, $nonvirtual:expr) => {
        unsafe extern "system" fn $v_gp(
            env: *mut NativeEnvironment,
            obj: JniObject,
            mid: JniMethodID,
            args: *const u8,
        ) -> u64 {
            typed_dispatch(env, $kind, obj, ptr::null_mut(), mid, args)
        }
        unsafe extern "system" fn $f32(
            env: *mut NativeEnvironment,
            obj: JniObject,
            mid: JniMethodID,
            args: *const u8,
        ) -> f32 {
            f32::from_bits(typed_dispatch(env, $kind, obj, ptr::null_mut(), mid, args) as u32)
        }
        unsafe extern "system" fn $f64(
            env: *mut NativeEnvironment,
            obj: JniObject,
            mid: JniMethodID,
            args: *const u8,
        ) -> f64 {
            f64::from_bits(typed_dispatch(env, $kind, obj, ptr::null_mut(), mid, args))
        }
    };
}

macro_rules! call_family_nv {
    ($v_gp:ident, $f32:ident, $f64:ident) => {
        unsafe extern "system" fn $v_gp(
            env: *mut NativeEnvironment,
            obj: JniObject,
            clazz: JniClass,
            mid: JniMethodID,
            args: *const u8,
        ) -> u64 {
            typed_dispatch(env, VAR_CALL_NONVIRTUAL, obj, clazz, mid, args)
        }
        unsafe extern "system" fn $f32(
            env: *mut NativeEnvironment,
            obj: JniObject,
            clazz: JniClass,
            mid: JniMethodID,
            args: *const u8,
        ) -> f32 {
            f32::from_bits(typed_dispatch(env, VAR_CALL_NONVIRTUAL, obj, clazz, mid, args) as u32)
        }
        unsafe extern "system" fn $f64(
            env: *mut NativeEnvironment,
            obj: JniObject,
            clazz: JniClass,
            mid: JniMethodID,
            args: *const u8,
        ) -> f64 {
            f64::from_bits(typed_dispatch(
                env,
                VAR_CALL_NONVIRTUAL,
                obj,
                clazz,
                mid,
                args,
            ))
        }
    };
}

// Instance family
call_family!(
    call_v_instance_gp,
    call_v_instance_f32,
    call_v_instance_f64,
    VAR_CALL_INSTANCE,
    false
);
call_family!(
    call_a_instance_gp,
    call_a_instance_f32,
    call_a_instance_f64,
    VAR_CALL_INSTANCE,
    false
);
// Nonvirtual family
call_family_nv!(
    call_v_nonvirtual_gp,
    call_v_nonvirtual_f32,
    call_v_nonvirtual_f64
);
call_family_nv!(
    call_a_nonvirtual_gp,
    call_a_nonvirtual_f32,
    call_a_nonvirtual_f64
);
// Static family
call_family!(
    call_v_static_gp,
    call_v_static_f32,
    call_v_static_f64,
    VAR_CALL_STATIC,
    false
);
call_family!(
    call_a_static_gp,
    call_a_static_f32,
    call_a_static_f64,
    VAR_CALL_STATIC,
    false
);

unsafe extern "system" fn jni_new_object_v(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    mid: JniMethodID,
    args: *const u8,
) -> JniObject {
    typed_dispatch(env, VAR_CALL_NEWOBJECT, clazz, ptr::null_mut(), mid, args) as *mut c_void
}

unsafe extern "system" fn jni_new_object_a(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    mid: JniMethodID,
    args: *const u64,
) -> JniObject {
    jni_new_object_v(env, clazz, mid, args.cast())
}

// ---------------------------------------------------------------------------
// Strings (modified UTF-8 + UTF-16)
// ---------------------------------------------------------------------------

/// Encodes a Rust str into JNI modified UTF-8.
fn to_modified_utf8(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 8);
    for c in text.chars() {
        let cp = c as u32;
        match cp {
            0 => out.extend_from_slice(&[0xC0, 0x80]),
            0x01..=0x7F => out.push(cp as u8),
            0x80..=0x7FF => {
                out.push(0xC0 | (cp >> 6) as u8);
                out.push(0x80 | (cp & 0x3F) as u8);
            }
            0x800..=0xFFFF => {
                out.push(0xE0 | (cp >> 12) as u8);
                out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
                out.push(0x80 | (cp & 0x3F) as u8);
            }
            _ => {
                // Supplementary characters are encoded as surrogate pairs,
                // each in 3-byte modified UTF-8 form.
                let v = cp - 0x10000;
                for unit in [0xD800 + ((v >> 10) & 0x3FF), 0xDC00 + (v & 0x3FF)] {
                    out.push(0xE0 | (unit >> 12) as u8);
                    out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                    out.push(0x80 | (unit & 0x3F) as u8);
                }
            }
        }
    }
    out
}

fn decode_modified_utf8(bytes: &[u8]) -> Option<String> {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        if b0 & 0x80 == 0 {
            out.push(b0 as char);
            i += 1;
        } else if b0 & 0xE0 == 0xC0 {
            if i + 1 >= bytes.len() {
                return None;
            }
            let cp = (((b0 & 0x1F) as u32) << 6) | ((bytes[i + 1] & 0x3F) as u32);
            if cp == 0 {
                out.push('\0');
            } else {
                out.push(char::from_u32(cp)?);
            }
            i += 2;
        } else if b0 & 0xF0 == 0xE0 {
            if i + 2 >= bytes.len() {
                return None;
            }
            let cp = (((b0 & 0x0F) as u32) << 12)
                | (((bytes[i + 1] & 0x3F) as u32) << 6)
                | ((bytes[i + 2] & 0x3F) as u32);
            out.push(char::from_u32(cp)?);
            i += 3;
        } else {
            return None;
        }
    }
    Some(out)
}

/// Reads the text of a jstring referent.
fn jstring_text<'a>(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'a>,
    token: *mut c_void,
) -> io::Result<Option<std::borrow::Cow<'a, str>>> {
    let value = token_to_value(env, loader, token)?;
    crate::share::prims::stringNatives::string(loader, value)
}

/// Creates a java.lang.String object from text and returns its jobject.
fn make_jstring(
    env: *mut NativeEnvironment,
    loader: &ClassLoader<'static>,
    text: &str,
) -> io::Result<*mut c_void> {
    let storage_text = loader.storage.alloc_str(text);
    let value = crate::share::prims::stringNatives::materialize(
        loader,
        Value::HostString(storage_text),
        &mut io::sink(),
        0,
    )?
    .ok_or_else(|| Error::other("Failed to create java.lang.String"))?;
    value_to_token(env, loader, value)
}

unsafe extern "system" fn jni_new_string(
    env: *mut NativeEnvironment,
    unicode_chars: *const u16,
    len: i32,
) -> JniString {
    if pending(env) || unicode_chars.is_null() || len < 0 {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let units = unsafe { std::slice::from_raw_parts(unicode_chars, len as usize) };
    let text = match char::decode_utf16(units.iter().copied()).collect::<Result<String, _>>() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };
    make_jstring(env, loader, &text).unwrap_or(ptr::null_mut())
}

unsafe extern "system" fn jni_get_string_length(
    env: *mut NativeEnvironment,
    string: JniString,
) -> i32 {
    if string.is_null() || pending(env) {
        return 0;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    match jstring_text(env, loader, string) {
        Ok(Some(text)) => text.encode_utf16().count() as i32,
        _ => 0,
    }
}

fn pin_buffer(env: *mut NativeEnvironment, bytes: Vec<u8>) -> *mut u8 {
    let boxed: Box<[u8]> = bytes.into_boxed_slice();
    let ptr = boxed.as_ptr() as *mut u8;
    let len = boxed.len();
    if !env.is_null() {
        unsafe { &*env }.pinned.borrow_mut().push(PinnedBuffer {
            ptr,
            len,
            owner: Some(boxed),
        });
    }
    ptr
}

fn unpin_buffer(env: *mut NativeEnvironment, ptr: *mut u8) {
    if env.is_null() || ptr.is_null() {
        return;
    }
    let mut pinned = unsafe { &*env }.pinned.borrow_mut();
    if let Some(index) = pinned.iter().position(|b| b.ptr == ptr) {
        pinned.swap_remove(index);
    }
}

unsafe extern "system" fn jni_get_string_chars(
    env: *mut NativeEnvironment,
    string: JniString,
    is_copy: *mut u8,
) -> *const u16 {
    if string.is_null() || pending(env) {
        return ptr::null();
    }
    if !is_copy.is_null() {
        unsafe { is_copy.write(1) };
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null();
    };
    let Ok(Some(text)) = jstring_text(env, loader, string) else {
        return ptr::null();
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for unit in units {
        bytes.extend_from_slice(&unit.to_ne_bytes());
    }
    pin_buffer(env, bytes).cast()
}

unsafe extern "system" fn jni_release_string_chars(
    env: *mut NativeEnvironment,
    _string: JniString,
    chars: *const u16,
) {
    unpin_buffer(env, chars as *mut u8);
}

unsafe extern "system" fn jni_new_string_utf(
    env: *mut NativeEnvironment,
    utf: *const c_char,
) -> JniString {
    if utf.is_null() || pending(env) {
        return ptr::null_mut();
    }
    let Some(bytes) = cstr_bytes(utf) else {
        return ptr::null_mut();
    };
    let Some(text) = decode_modified_utf8(bytes) else {
        return ptr::null_mut();
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    make_jstring(env, loader, &text).unwrap_or(ptr::null_mut())
}

unsafe extern "system" fn jni_get_string_utf_length(
    env: *mut NativeEnvironment,
    string: JniString,
) -> i32 {
    jni_get_string_utf_length_as_long(env, string) as i32
}

unsafe extern "system" fn jni_get_string_utf_length_as_long(
    env: *mut NativeEnvironment,
    string: JniString,
) -> i64 {
    if string.is_null() || pending(env) {
        return 0;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    match jstring_text(env, loader, string) {
        Ok(Some(text)) => to_modified_utf8(&text).len() as i64,
        _ => 0,
    }
}

unsafe extern "system" fn jni_get_string_utf_chars(
    env: *mut NativeEnvironment,
    string: JniString,
    is_copy: *mut u8,
) -> *const c_char {
    if string.is_null() || pending(env) {
        return ptr::null();
    }
    if !is_copy.is_null() {
        unsafe { is_copy.write(1) };
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null();
    };
    let Ok(Some(text)) = jstring_text(env, loader, string) else {
        return ptr::null();
    };
    let mut bytes = to_modified_utf8(&text);
    bytes.push(0);
    pin_buffer(env, bytes).cast()
}

unsafe extern "system" fn jni_release_string_utf_chars(
    env: *mut NativeEnvironment,
    _string: JniString,
    utf: *const c_char,
) {
    unpin_buffer(env, utf as *mut u8);
}

unsafe extern "system" fn jni_get_string_region(
    env: *mut NativeEnvironment,
    string: JniString,
    start: i32,
    len: i32,
    buf: *mut u16,
) {
    if string.is_null() || buf.is_null() || len < 0 || pending(env) {
        return;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let Some(loader) = vm.class_loader() else {
        return;
    };
    let Ok(Some(text)) = jstring_text(env, loader, string) else {
        return;
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = start.max(0) as usize;
    let end = (start + len as usize).min(units.len());
    if start >= units.len() {
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(units[start..end].as_ptr(), buf, end - start);
    }
}

unsafe extern "system" fn jni_get_string_utf_region(
    env: *mut NativeEnvironment,
    string: JniString,
    start: i32,
    len: i32,
    buf: *mut c_char,
) {
    if string.is_null() || buf.is_null() || len < 0 || pending(env) {
        return;
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let Some(loader) = vm.class_loader() else {
        return;
    };
    let Ok(Some(text)) = jstring_text(env, loader, string) else {
        return;
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = start.max(0) as usize;
    let end = (start + len as usize).min(units.len());
    if start >= units.len() {
        return;
    }
    let sub: String = match char::decode_utf16(units[start..end].iter().copied())
        .collect::<Result<String, _>>()
    {
        Ok(s) => s,
        Err(_) => return,
    };
    let bytes = to_modified_utf8(&sub);
    let n = bytes.len().min(len as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, n);
        (buf as *mut u8).add(n).write(0);
    }
}

unsafe extern "system" fn jni_get_string_critical(
    env: *mut NativeEnvironment,
    string: JniString,
    is_copy: *mut u8,
) -> *const u16 {
    jni_get_string_chars(env, string, is_copy)
}

unsafe extern "system" fn jni_release_string_critical(
    env: *mut NativeEnvironment,
    string: JniString,
    cstring: *const u16,
) {
    jni_release_string_chars(env, string, cstring);
}

// ---------------------------------------------------------------------------
// Arrays
// ---------------------------------------------------------------------------

fn array_len(env: *mut NativeEnvironment, array: JniArray) -> i32 {
    if array.is_null() || pending(env) {
        return 0;
    }
    let Ok(oop) = resolve_oop(env, array) else {
        return 0;
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    let Some(loader) = vm.class_loader() else {
        return 0;
    };
    match oop {
        Oop::TypeArray(handle) => loader
            .arrays
            .borrow()
            .get(handle)
            .map(|a| a.len() as i32)
            .unwrap_or(0),
        Oop::ObjArray(handle) => loader
            .obj_arrays
            .borrow()
            .get(handle)
            .map(|a| a.elements.len() as i32)
            .unwrap_or(0),
        _ => 0,
    }
}

unsafe extern "system" fn jni_get_array_length(
    env: *mut NativeEnvironment,
    array: JniArray,
) -> i32 {
    array_len(env, array)
}

fn push_type_array(env: *mut NativeEnvironment, array: TypeArrayOop) -> JniArray {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let handle = loader.alloc_type_array(array);
    new_local(env, JniRefTarget::Object(Oop::TypeArray(handle)))
}

fn push_obj_array(env: *mut NativeEnvironment, array: ObjArrayOop<'static>) -> JniArray {
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let handle = loader.alloc_obj_array(array);
    new_local(env, JniRefTarget::Object(Oop::ObjArray(handle)))
}

macro_rules! new_primitive_arrays {
    ($bool_fn:ident, $byte_fn:ident, $char_fn:ident, $short_fn:ident,
     $int_fn:ident, $long_fn:ident, $float_fn:ident, $double_fn:ident) => {
        unsafe extern "system" fn $bool_fn(
            env: *mut NativeEnvironment,
            len: i32,
        ) -> JniBooleanArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Boolean(vec![0; len as usize]))
        }
        unsafe extern "system" fn $byte_fn(env: *mut NativeEnvironment, len: i32) -> JniByteArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Byte(vec![0; len as usize]))
        }
        unsafe extern "system" fn $char_fn(env: *mut NativeEnvironment, len: i32) -> JniCharArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Char(vec![0; len as usize]))
        }
        unsafe extern "system" fn $short_fn(
            env: *mut NativeEnvironment,
            len: i32,
        ) -> JniShortArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Short(vec![0; len as usize]))
        }
        unsafe extern "system" fn $int_fn(env: *mut NativeEnvironment, len: i32) -> JniIntArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Int(vec![0; len as usize]))
        }
        unsafe extern "system" fn $long_fn(env: *mut NativeEnvironment, len: i32) -> JniLongArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Long(vec![0; len as usize]))
        }
        unsafe extern "system" fn $float_fn(
            env: *mut NativeEnvironment,
            len: i32,
        ) -> JniFloatArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Float(vec![0.0; len as usize]))
        }
        unsafe extern "system" fn $double_fn(
            env: *mut NativeEnvironment,
            len: i32,
        ) -> JniDoubleArray {
            if len < 0 || pending(env) {
                return ptr::null_mut();
            }
            push_type_array(env, TypeArrayOop::Double(vec![0.0; len as usize]))
        }
    };
}

new_primitive_arrays!(
    jni_new_boolean_array,
    jni_new_byte_array,
    jni_new_char_array,
    jni_new_short_array,
    jni_new_int_array,
    jni_new_long_array,
    jni_new_float_array,
    jni_new_double_array
);

unsafe extern "system" fn jni_new_object_array(
    env: *mut NativeEnvironment,
    length: i32,
    element_class: JniClass,
    initial_element: JniObject,
) -> JniObjectArray {
    if length < 0 || pending(env) {
        return ptr::null_mut();
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let component = if element_class.is_null() {
        b"java/lang/Object".to_vec()
    } else {
        match class_name_of(env, element_class) {
            Ok(name) => name,
            Err(_) => return ptr::null_mut(),
        }
    };
    // Arrays store component as internal name or descriptor.
    let component: &'static [u8] = loader.storage.alloc_slice_copy(&component);
    let mut array = match ObjArrayOop::withComponent(length, component) {
        Ok(a) => a,
        Err(_) => return ptr::null_mut(),
    };
    if !initial_element.is_null() {
        let value = token_to_value(env, loader, initial_element).unwrap_or(Value::Null);
        for element in array.elements.iter_mut() {
            *element = value;
        }
    }
    push_obj_array(env, array)
}

unsafe extern "system" fn jni_get_object_array_element(
    env: *mut NativeEnvironment,
    array: JniObjectArray,
    index: i32,
) -> JniObject {
    if array.is_null() || index < 0 || pending(env) {
        return ptr::null_mut();
    }
    let Ok(oop) = resolve_oop(env, array) else {
        return ptr::null_mut();
    };
    let Oop::ObjArray(handle) = oop else {
        return ptr::null_mut();
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let value = {
        let arrays = loader.obj_arrays.borrow();
        let Some(a) = arrays.get(handle) else {
            return ptr::null_mut();
        };
        if index as usize >= a.elements.len() {
            return ptr::null_mut();
        }
        a.elements[index as usize]
    };
    // Values with 'static loader lifetime coerce fine.
    value_to_token(env, loader, value).unwrap_or(ptr::null_mut())
}

unsafe extern "system" fn jni_set_object_array_element(
    env: *mut NativeEnvironment,
    array: JniObjectArray,
    index: i32,
    value: JniObject,
) {
    if array.is_null() || index < 0 || pending(env) {
        return;
    }
    let Ok(oop) = resolve_oop(env, array) else {
        return;
    };
    let Oop::ObjArray(handle) = oop else {
        return;
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return;
    };
    let Some(loader) = vm.class_loader() else {
        return;
    };
    let new_value = if value.is_null() {
        Value::Null
    } else {
        token_to_value(env, loader, value).unwrap_or(Value::Null)
    };
    let mut arrays = loader.obj_arrays.borrow_mut();
    if let Some(a) = arrays.get_mut(handle)
        && (index as usize) < a.elements.len()
    {
        a.elements[index as usize] = new_value;
    }
}

// Typed array element access. Each TypeArrayOop variant stores a Vec<T>;
// we return pointers into those buffers. The Vec heap allocation is stable
// as long as the array is not resized (JNI never resizes arrays).

macro_rules! array_elements {
    ($get_fn:ident, $rel_fn:ident, $variant:ident, $elem:ty) => {
        unsafe extern "system" fn $get_fn(
            env: *mut NativeEnvironment,
            array: JniArray,
            is_copy: *mut u8,
        ) -> *mut $elem {
            if array.is_null() || pending(env) {
                return ptr::null_mut();
            }
            if !is_copy.is_null() {
                unsafe { is_copy.write(0) };
            }
            let Ok(oop) = resolve_oop(env, array) else {
                return ptr::null_mut();
            };
            let Oop::TypeArray(handle) = oop else {
                return ptr::null_mut();
            };
            let Some(vm) = (unsafe { vm_of(env) }) else {
                return ptr::null_mut();
            };
            let Some(loader) = vm.class_loader() else {
                return ptr::null_mut();
            };
            let arrays = loader.arrays.borrow();
            let Some(TypeArrayOop::$variant(values)) = arrays.get(handle) else {
                return ptr::null_mut();
            };
            values.as_ptr() as *mut $elem
        }
        unsafe extern "system" fn $rel_fn(
            _env: *mut NativeEnvironment,
            _array: JniArray,
            _elements: *mut $elem,
            _mode: i32,
        ) {
            // Elements point directly into the TypeArrayOop storage; writes are
            // already visible to the VM, so release is a no-op.
        }
    };
}

array_elements!(
    jni_get_boolean_array_elements,
    jni_release_boolean_array_elements,
    Boolean,
    u8
);
array_elements!(
    jni_get_byte_array_elements,
    jni_release_byte_array_elements,
    Byte,
    i8
);
array_elements!(
    jni_get_char_array_elements,
    jni_release_char_array_elements,
    Char,
    u16
);
array_elements!(
    jni_get_short_array_elements,
    jni_release_short_array_elements,
    Short,
    i16
);
array_elements!(
    jni_get_int_array_elements,
    jni_release_int_array_elements,
    Int,
    i32
);
array_elements!(
    jni_get_long_array_elements,
    jni_release_long_array_elements,
    Long,
    i64
);
array_elements!(
    jni_get_float_array_elements,
    jni_release_float_array_elements,
    Float,
    f32
);
array_elements!(
    jni_get_double_array_elements,
    jni_release_double_array_elements,
    Double,
    f64
);

unsafe extern "system" fn jni_get_primitive_array_critical(
    env: *mut NativeEnvironment,
    array: JniArray,
    is_copy: *mut u8,
) -> *mut c_void {
    // Type-agnostic critical access: locate the array and return a pointer to
    // its first element as raw bytes.
    if array.is_null() || pending(env) {
        return ptr::null_mut();
    }
    if !is_copy.is_null() {
        unsafe { is_copy.write(0) };
    }
    let Ok(oop) = resolve_oop(env, array) else {
        return ptr::null_mut();
    };
    let Oop::TypeArray(handle) = oop else {
        return ptr::null_mut();
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return ptr::null_mut();
    };
    let Some(loader) = vm.class_loader() else {
        return ptr::null_mut();
    };
    let arrays = loader.arrays.borrow();
    let Some(array_ref) = arrays.get(handle) else {
        return ptr::null_mut();
    };
    match array_ref {
        TypeArrayOop::Boolean(v) => v.as_ptr() as *mut c_void,
        TypeArrayOop::Byte(v) => v.as_ptr() as *mut c_void,
        TypeArrayOop::Char(v) => v.as_ptr() as *mut c_void,
        TypeArrayOop::Short(v) => v.as_ptr() as *mut c_void,
        TypeArrayOop::Int(v) => v.as_ptr() as *mut c_void,
        TypeArrayOop::Long(v) => v.as_ptr() as *mut c_void,
        TypeArrayOop::Float(v) => v.as_ptr() as *mut c_void,
        TypeArrayOop::Double(v) => v.as_ptr() as *mut c_void,
    }
}

unsafe extern "system" fn jni_release_primitive_array_critical(
    _env: *mut NativeEnvironment,
    _array: JniArray,
    _carray: *mut c_void,
    _mode: i32,
) {
}

// Region get/set: copy between Java array and a raw C buffer.

macro_rules! array_region {
    ($get_fn:ident, $set_fn:ident, $variant:ident, $elem:ty) => {
        unsafe extern "system" fn $get_fn(
            env: *mut NativeEnvironment,
            array: JniArray,
            start: i32,
            len: i32,
            buf: *mut $elem,
        ) {
            if array.is_null() || buf.is_null() || len < 0 || start < 0 || pending(env) {
                return;
            }
            let Ok(oop) = resolve_oop(env, array) else {
                return;
            };
            let Oop::TypeArray(handle) = oop else {
                return;
            };
            let Some(vm) = (unsafe { vm_of(env) }) else {
                return;
            };
            let Some(loader) = vm.class_loader() else {
                return;
            };
            let arrays = loader.arrays.borrow();
            let Some(TypeArrayOop::$variant(values)) = arrays.get(handle) else {
                return;
            };
            let s = start as usize;
            let l = len as usize;
            if s + l > values.len() {
                return;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(values[s..s + l].as_ptr(), buf, l);
            }
        }
        unsafe extern "system" fn $set_fn(
            env: *mut NativeEnvironment,
            array: JniArray,
            start: i32,
            len: i32,
            buf: *const $elem,
        ) {
            if array.is_null() || buf.is_null() || len < 0 || start < 0 || pending(env) {
                return;
            }
            let Ok(oop) = resolve_oop(env, array) else {
                return;
            };
            let Oop::TypeArray(handle) = oop else {
                return;
            };
            let Some(vm) = (unsafe { vm_of(env) }) else {
                return;
            };
            let Some(loader) = vm.class_loader() else {
                return;
            };
            let mut arrays = loader.arrays.borrow_mut();
            let Some(TypeArrayOop::$variant(values)) = arrays.get_mut(handle) else {
                return;
            };
            let s = start as usize;
            let l = len as usize;
            if s + l > values.len() {
                return;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(buf, values[s..s + l].as_mut_ptr(), l);
            }
        }
    };
}

array_region!(
    jni_get_boolean_array_region,
    jni_set_boolean_array_region,
    Boolean,
    u8
);
array_region!(
    jni_get_byte_array_region,
    jni_set_byte_array_region,
    Byte,
    i8
);
array_region!(
    jni_get_char_array_region,
    jni_set_char_array_region,
    Char,
    u16
);
array_region!(
    jni_get_short_array_region,
    jni_set_short_array_region,
    Short,
    i16
);
array_region!(jni_get_int_array_region, jni_set_int_array_region, Int, i32);
array_region!(
    jni_get_long_array_region,
    jni_set_long_array_region,
    Long,
    i64
);
array_region!(
    jni_get_float_array_region,
    jni_set_float_array_region,
    Float,
    f32
);
array_region!(
    jni_get_double_array_region,
    jni_set_double_array_region,
    Double,
    f64
);

// ---------------------------------------------------------------------------
// RegisterNatives / UnregisterNatives
// ---------------------------------------------------------------------------

#[repr(C)]
struct JniNativeMethod {
    name: *const c_char,
    signature: *const c_char,
    fn_ptr: *mut c_void,
}

unsafe extern "system" fn jni_register_natives(
    env: *mut NativeEnvironment,
    clazz: JniClass,
    methods: *const JniNativeMethod,
    count: i32,
) -> i32 {
    if methods.is_null() || count < 0 || pending(env) {
        return JNI_ERR;
    }
    let Ok(class_name) = class_name_of(env, clazz) else {
        return JNI_ERR;
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return JNI_ERR;
    };
    for i in 0..count as usize {
        let method = unsafe { &*methods.add(i) };
        let (Some(name), Some(sig)) = (cstr_bytes(method.name), cstr_bytes(method.signature))
        else {
            return JNI_ERR;
        };
        if method.fn_ptr.is_null() {
            return JNI_ERR;
        }
        let plan = match build_plan_for_signature(method.fn_ptr, sig, true) {
            Ok(plan) => plan,
            Err(_) => return JNI_ERR,
        };
        let trampoline = match jniTrampoline::build_call_trampoline(&plan) {
            Ok(t) => t,
            Err(_) => return JNI_ERR,
        };
        vm.state().natives.insert(
            (class_name.clone(), name.to_vec(), sig.to_vec()),
            NativeCode {
                func: method.fn_ptr,
                trampoline,
                plan,
            },
        );
    }
    JNI_OK
}

unsafe extern "system" fn jni_unregister_natives(
    env: *mut NativeEnvironment,
    clazz: JniClass,
) -> i32 {
    if pending(env) {
        return JNI_ERR;
    }
    let Ok(class_name) = class_name_of(env, clazz) else {
        return JNI_ERR;
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return JNI_ERR;
    };
    vm.state()
        .natives
        .retain(|(class, _, _), _| class != &class_name);
    JNI_OK
}

// ---------------------------------------------------------------------------
// Monitors
// ---------------------------------------------------------------------------

fn monitor_key(env: *mut NativeEnvironment, obj: JniObject) -> Option<(u8, usize)> {
    match resolve_target(env, obj).ok()? {
        JniRefTarget::Object(Oop::Instance(h)) => Some((0, h)),
        JniRefTarget::Object(Oop::TypeArray(h)) => Some((1, h)),
        JniRefTarget::Object(Oop::ObjArray(h)) => Some((2, h)),
        JniRefTarget::Object(Oop::Null) => None,
        JniRefTarget::Class(id) => Some((3, id as usize)),
    }
}

unsafe extern "system" fn jni_monitor_enter(env: *mut NativeEnvironment, obj: JniObject) -> i32 {
    if obj.is_null() || pending(env) {
        return JNI_ERR;
    }
    let Some(key) = monitor_key(env, obj) else {
        return JNI_ERR;
    };
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return JNI_ERR;
    };
    let owner = thread::current().id();
    {
        let mut held = unsafe { &*env }.held_monitors.borrow_mut();
        if let Some(hold) = held
            .iter_mut()
            .rev()
            .find(|h| h.key == key && h.owner == owner)
        {
            hold.depth += 1;
            return JNI_OK;
        }
    }
    let monitor = {
        let mut state = vm.state();
        state
            .monitors
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    // Lock through a 'static reference: the Arc<Mutex<()>> is stored in the
    // MonitorHold alongside the guard, so the mutex outlives the guard.
    let mutex: &'static Mutex<()> =
        unsafe { &*(std::sync::Arc::as_ptr(&monitor) as *const Mutex<()>) };
    let guard = mutex.lock().unwrap();
    unsafe { &*env }
        .held_monitors
        .borrow_mut()
        .push(MonitorHold {
            key,
            owner,
            monitor,
            guard,
            depth: 1,
        });
    JNI_OK
}

unsafe extern "system" fn jni_monitor_exit(env: *mut NativeEnvironment, obj: JniObject) -> i32 {
    if obj.is_null() {
        return JNI_ERR;
    }
    let Some(key) = monitor_key(env, obj) else {
        return JNI_ERR;
    };
    let owner = thread::current().id();
    let mut held = unsafe { &*env }.held_monitors.borrow_mut();
    if let Some(pos) = held.iter().rposition(|h| h.key == key && h.owner == owner) {
        if held[pos].depth > 1 {
            held[pos].depth -= 1;
        } else {
            held.remove(pos);
        }
        JNI_OK
    } else {
        JNI_ERR
    }
}

// ---------------------------------------------------------------------------
// Remaining JNIEnv entries
// ---------------------------------------------------------------------------

unsafe extern "system" fn jni_get_java_vm(
    env: *mut NativeEnvironment,
    vm: *mut *mut JavaVm,
) -> i32 {
    if env.is_null() || vm.is_null() {
        return JNI_EINVAL;
    }
    unsafe { vm.write((*env).vm) };
    JNI_OK
}

unsafe extern "system" fn jni_new_weak_global_ref(
    env: *mut NativeEnvironment,
    obj: JniObject,
) -> JniWeak {
    if obj.is_null() {
        return ptr::null_mut();
    }
    match resolve_target(env, obj) {
        Ok(target) => new_global(env, target, true),
        Err(_) => ptr::null_mut(),
    }
}

unsafe extern "system" fn jni_delete_weak_global_ref(env: *mut NativeEnvironment, ref_: JniWeak) {
    delete_ref(env, ref_, RefKind::Weak);
}

unsafe extern "system" fn jni_exception_check(env: *mut NativeEnvironment) -> u8 {
    u8::from(pending(env))
}

unsafe extern "system" fn jni_new_direct_byte_buffer(
    _env: *mut NativeEnvironment,
    _address: *mut c_void,
    _capacity: i64,
) -> JniObject {
    ptr::null_mut()
}

unsafe extern "system" fn jni_get_direct_buffer_address(
    _env: *mut NativeEnvironment,
    _buf: JniObject,
) -> *mut c_void {
    ptr::null_mut()
}

unsafe extern "system" fn jni_get_direct_buffer_capacity(
    _env: *mut NativeEnvironment,
    _buf: JniObject,
) -> i64 {
    -1
}

unsafe extern "system" fn jni_get_object_ref_type(
    env: *mut NativeEnvironment,
    obj: JniObject,
) -> i32 {
    if obj.is_null() {
        return 0; // JNIInvalidRefType
    }
    let Some(vm) = (unsafe { vm_of(env) }) else {
        return 0;
    };
    match vm.references().refType(JniReference::fromRaw(obj)) {
        JniRefType::Invalid => 0,
        JniRefType::Local => 1,
        JniRefType::Global => 2,
        JniRefType::WeakGlobal => 3,
    }
}

unsafe extern "system" fn jni_get_module(
    _env: *mut NativeEnvironment,
    _clazz: JniClass,
) -> JniObject {
    ptr::null_mut()
}

unsafe extern "system" fn jni_is_virtual_thread(
    _env: *mut NativeEnvironment,
    _obj: JniObject,
) -> u8 {
    0
}

// ---------------------------------------------------------------------------
// Public API used by nativeLookup / nativeLibraries
// ---------------------------------------------------------------------------

/// Builds a CallPlan + trampoline for a raw native function pointer and a
/// Java method descriptor. Results are cached per (func, descriptor).
pub fn trampoline_for(
    vm: &JavaVm,
    func: *const c_void,
    descriptor: &[u8],
    is_static: bool,
) -> io::Result<NativeCode> {
    let key = (func as usize, descriptor.to_vec());
    if let Some(code) = vm.state().trampoline_cache.get(&key) {
        return Ok(code.clone());
    }
    let plan = build_plan_for_signature(func, descriptor, is_static)?;
    let trampoline = jniTrampoline::build_call_trampoline(&plan)?;
    let code = NativeCode {
        func,
        trampoline,
        plan,
    };
    vm.state().trampoline_cache.insert(key, code.clone());
    Ok(code)
}

fn build_plan_for_signature(
    func: *const c_void,
    descriptor: &[u8],
    is_static: bool,
) -> io::Result<CallPlan> {
    let (kinds, ret) = sig_parts(descriptor)?;
    let mut params = Vec::with_capacity(kinds.len() + 2);
    params.push(ArgKind::Gp); // JNIEnv*
    params.push(ArgKind::Gp); // jclass / jobject
    for kind in &kinds {
        params.push(arg_kind_to_jni(*kind));
    }
    let stack_args = params.len().saturating_sub(4);
    let ret_kind = match ret {
        Some(2) => RetKind::Float,
        Some(3) => RetKind::Double,
        _ => RetKind::Gp,
    };
    let _ = is_static;
    Ok(CallPlan {
        func,
        params,
        ret: ret_kind,
        stack_args,
    })
}

/// Looks up a native method registered through RegisterNatives.
pub fn find_registered_native(
    vm: &JavaVm,
    class_name: &[u8],
    name: &[u8],
    sig: &[u8],
) -> Option<NativeCode> {
    vm.state()
        .natives
        .get(&(class_name.to_vec(), name.to_vec(), sig.to_vec()))
        .cloned()
}

/// Registers a native implementation discovered in a DLL.
pub fn register_resolved_native(
    vm: &JavaVm,
    class_name: &[u8],
    name: &[u8],
    sig: &[u8],
    func: *const c_void,
) -> io::Result<()> {
    let plan = build_plan_for_signature(func, sig, false)?;
    let trampoline = jniTrampoline::build_call_trampoline(&plan)?;
    vm.state().natives.insert(
        (class_name.to_vec(), name.to_vec(), sig.to_vec()),
        NativeCode {
            func,
            trampoline,
            plan,
        },
    );
    Ok(())
}

/// Returns the JNIEnv pointer for the current thread, attaching if needed.
pub fn ensure_env() -> *mut c_void {
    unsafe { &*JavaVm::global() }.ensure_attached()
}

/// Binds the interpreter class loader for subsequent JNI calls.
pub fn bind_loader(loader: &ClassLoader<'_>) {
    unsafe { &*JavaVm::global() }.bind_loader(loader);
}

/// Converts an interpreter Value into a jobject token (local ref).
pub(crate) fn value_to_jobject(env: *mut c_void, value: Value<'static>) -> io::Result<*mut c_void> {
    let env = env as *mut NativeEnvironment;
    let vm = unsafe { vm_of(env) }.ok_or_else(|| Error::other("Detached JNI environment"))?;
    let loader = vm
        .class_loader()
        .ok_or_else(|| Error::other("No class loader bound"))?;
    value_to_token(env, loader, value)
}

/// Converts a jobject token back into an interpreter Value.
pub(crate) fn jobject_to_value<'a>(
    env: *mut c_void,
    loader: &ClassLoader<'a>,
    token: *mut c_void,
) -> io::Result<Value<'a>> {
    token_to_value(env as *mut NativeEnvironment, loader, token)
}

/// Checks whether the current thread has a pending JNI exception and, if so,
/// returns it as an interpreter error carrying the real Throwable object.
pub(crate) fn take_pending_exception(loader: &ClassLoader<'static>) -> Option<Error> {
    let env = current_env()? as *mut NativeEnvironment;
    take_pending_error(env, loader)
}

/// Clears any pending JNI exception.
pub(crate) fn clear_pending_exception() {
    if let Some(env) = current_env() {
        clear_pending(env as *mut NativeEnvironment);
    }
}

/// Returns true when a native implementation exists for the given method:
/// either registered via RegisterNatives or exported by a loaded DLL.
pub fn has_native_implementation(vm: &JavaVm, class_name: &[u8], name: &[u8], sig: &[u8]) -> bool {
    vm.state()
        .natives
        .contains_key(&(class_name.to_vec(), name.to_vec(), sig.to_vec()))
}

/// Resolves and invokes a native method implemented by a loaded DLL.
///
/// Lookup order mirrors HotSpot's NativeLookup: first methods registered via
/// `RegisterNatives`, then short/long JNI symbol names exported by loaded
/// libraries (resolved once and cached). Returns `Ok(None)` when no native
/// implementation exists so the caller can fall back to other handlers.
///
/// `arguments` follows the interpreter convention: for instance methods the
/// receiver is `arguments[0]`; for static methods it is absent.
pub fn invoke_dll_native(
    loader: &ClassLoader<'_>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    is_static: bool,
    arguments: &[Value<'_>],
) -> io::Result<Option<Option<Value<'static>>>> {
    let vm = unsafe { &*JavaVm::global() };
    vm.bind_loader(loader);
    let env = vm.ensure_attached() as *mut NativeEnvironment;
    if env.is_null() {
        return Err(Error::other("Failed to attach JNI environment"));
    }
    let loader = vm
        .class_loader()
        .ok_or_else(|| Error::other("No class loader bound"))?;

    let key = (owner.to_vec(), name.to_vec(), descriptor.to_vec());
    let code =
        if let Some(code) = vm.state().natives.get(&key).cloned() {
            code
        } else {
            let class_name = String::from_utf8_lossy(owner).into_owned();
            let method_name = String::from_utf8_lossy(name).into_owned();
            let sig = String::from_utf8_lossy(descriptor).into_owned();
            let Some(func) = super::nativeLibraries::NativeLibraries::global()
                .resolve_native_function(&class_name, &method_name, &sig)
            else {
                return Ok(None);
            };
            let code = trampoline_for(vm, func, descriptor, is_static)?;
            vm.state().natives.insert(key, code.clone());
            code
        };

    let (param_kinds, ret_kind) = sig_parts(descriptor)?;

    // Split receiver/arguments and convert to raw u64 slots.
    let (receiver_value, java_args) = if is_static {
        (None, arguments)
    } else {
        match arguments.split_first() {
            Some((&receiver, rest)) => (Some(receiver), rest),
            None => (None, &[][..]),
        }
    };

    let mut ctx = CallContext {
        func: code.func,
        env: env as *mut c_void,
        receiver: match receiver_value {
            Some(value) => value_to_token_quiet(env, unsafe {
                std::mem::transmute::<Value<'_>, Value<'static>>(value)
            }) as usize,
            None => class_token(env, owner) as usize,
        },
        args: [0; 512],
    };
    let n = java_args.len().min(param_kinds.len()).min(510);
    for (i, value) in java_args.iter().take(n).enumerate() {
        let value = unsafe { std::mem::transmute::<Value<'_>, Value<'static>>(*value) };
        let kind = param_kinds[i];
        ctx.args[i] = if kind == 4 {
            value_to_token_quiet(env, value)
        } else {
            value_to_slot(&value, kind)
        };
    }

    if std::env::var_os("JVM25_TRACE_DLL").is_some() {
        eprintln!(
            "[dll-native] {}.{}{} static={is_static}",
            String::from_utf8_lossy(owner),
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor),
        );
    }
    let trampoline: unsafe extern "system" fn(*const CallContext) -> u64 =
        unsafe { std::mem::transmute(code.trampoline) };
    let raw = unsafe { trampoline(&ctx) };

    if pending(env)
        && let Some(error) = take_pending_error(env, loader)
    {
        return Err(error);
    }

    let value = match ret_kind {
        None => return Ok(Some(None)),
        Some(1) => Value::Long(raw as i64),
        Some(2) => Value::Float(f32::from_bits(raw as u32)),
        Some(3) => Value::Double(f64::from_bits(raw)),
        Some(4) => {
            let token = raw as *mut c_void;
            token_to_value(env, loader, token).unwrap_or(Value::Null)
        }
        _ => Value::Int(raw as u32 as i32),
    };
    Ok(Some(Some(value)))
}
