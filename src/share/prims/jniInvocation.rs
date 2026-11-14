use super::jni::{JNI_EDETACHED, JNI_EINVAL, JNI_ERR, JNI_EVERSION, JNI_OK, JNI_VERSION_25};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
pub struct JavaVm {
    functions: *const InvocationTable,
    id: u64,
    pin: PhantomPinned,
}

#[repr(C)]
struct NativeEnvironment {
    functions: *const NativeTable,
    vm: *mut JavaVm,
}

#[repr(C)]
struct AttachArguments {
    version: i32,
    name: *const std::ffi::c_char,
    group: *mut c_void,
}

#[repr(C)]
struct NativeTable {
    reserved: [*mut c_void; 4],
    getVersion: unsafe extern "system" fn(*mut NativeEnvironment) -> i32,
    beforeGetJavaVm: [*mut c_void; 214],
    getJavaVm: unsafe extern "system" fn(*mut NativeEnvironment, *mut *mut JavaVm) -> i32,
    remaining: [*mut c_void; 14],
}

unsafe impl Sync for NativeTable {}

static nativeTable: NativeTable = NativeTable {
    reserved: [std::ptr::null_mut(); 4],
    getVersion,
    beforeGetJavaVm: [std::ptr::null_mut(); 214],
    getJavaVm,
    remaining: [std::ptr::null_mut(); 14],
};

thread_local! {
    static environments: RefCell<HashMap<u64, Box<NativeEnvironment>>> = RefCell::new(HashMap::new());
}

static nextVmId: AtomicU64 = AtomicU64::new(1);

unsafe extern "system" fn getVersion(_env: *mut NativeEnvironment) -> i32 {
    JNI_VERSION_25
}

unsafe extern "system" fn getJavaVm(env: *mut NativeEnvironment, vm: *mut *mut JavaVm) -> i32 {
    if env.is_null() || vm.is_null() {
        return JNI_EINVAL;
    }
    unsafe { vm.write((*env).vm) };
    JNI_OK
}

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

static invocationTable: InvocationTable = InvocationTable {
    reserved0: std::ptr::null_mut(),
    reserved1: std::ptr::null_mut(),
    reserved2: std::ptr::null_mut(),
    destroy,
    attach,
    detach,
    getEnv,
    attachDaemon: attach,
};

impl JavaVm {
    pub fn new() -> Self {
        Self {
            functions: &invocationTable,
            id: nextVmId.fetch_add(1, Ordering::Relaxed),
            pin: PhantomPinned,
        }
    }

    pub fn pinned() -> Pin<Box<Self>> {
        Box::pin(Self::new())
    }
}

impl Drop for JavaVm {
    fn drop(&mut self) {
        environments.with(|values| {
            values.borrow_mut().remove(&self.id);
        });
    }
}

impl Default for JavaVm {
    fn default() -> Self {
        Self::new()
    }
}

unsafe extern "system" fn destroy(_vm: *mut JavaVm) -> i32 {
    JNI_ERR
}

unsafe extern "system" fn attach(
    vm: *mut JavaVm,
    environment: *mut *mut c_void,
    arguments: *mut c_void,
) -> i32 {
    if environment.is_null() {
        return JNI_EINVAL;
    }
    if vm.is_null() {
        unsafe { environment.write(std::ptr::null_mut()) };
        return JNI_EINVAL;
    }
    if !arguments.is_null() {
        let version = unsafe { (*(arguments as *const AttachArguments)).version };
        if !supportedVersion(version) {
            unsafe { environment.write(std::ptr::null_mut()) };
            return JNI_EVERSION;
        }
    }
    environments.with(|values| {
        let mut values = values.borrow_mut();
        let id = unsafe { (*vm).id };
        let env = values.entry(id).or_insert_with(|| {
            Box::new(NativeEnvironment {
                functions: &nativeTable,
                vm,
            })
        });
        unsafe { environment.write((&mut **env as *mut NativeEnvironment).cast()) };
    });
    JNI_OK
}

unsafe extern "system" fn detach(vm: *mut JavaVm) -> i32 {
    if vm.is_null() {
        return JNI_EINVAL;
    }
    environments.with(|values| {
        if values.borrow_mut().remove(&unsafe { (*vm).id }).is_some() {
            JNI_OK
        } else {
            JNI_EDETACHED
        }
    })
}

unsafe extern "system" fn getEnv(
    vm: *mut JavaVm,
    environment: *mut *mut c_void,
    version: i32,
) -> i32 {
    if environment.is_null() {
        return JNI_EINVAL;
    }
    unsafe { environment.write(std::ptr::null_mut()) };
    if vm.is_null() {
        return JNI_EINVAL;
    }
    if !supportedVersion(version) {
        return JNI_EVERSION;
    }
    environments.with(
        |values| match values.borrow_mut().get_mut(&unsafe { (*vm).id }) {
            Some(env) => {
                unsafe { environment.write((&mut **env as *mut NativeEnvironment).cast()) };
                JNI_OK
            }
            None => JNI_EDETACHED,
        },
    )
}

fn supportedVersion(version: i32) -> bool {
    matches!(
        version,
        0x00010001
            | 0x00010002
            | 0x00010004
            | 0x00010006
            | 0x00010008
            | 0x00090000
            | 0x000a0000
            | 0x00130000
            | 0x00140000
            | 0x00150000
            | 0x00180000
            | 0x00190000
    )
}
