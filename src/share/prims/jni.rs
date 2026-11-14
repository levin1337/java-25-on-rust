/*
 * Port of OpenJDK 25 HotSpot JNI (share/prims/jni.hpp / jni.cpp)
 *
 * Implements JNI Invocation API:
 * - JNI_CreateJavaVM
 * - JavaVM interface (DestroyJavaVM, AttachCurrentThread, AttachCurrentThreadAsDaemon, DetachCurrentThread, GetEnv)
 * - JNIEnv interface and native method registry
 */

use crate::share::oops::symbol::Symbol;
use crate::share::runtime::init::VMInit;
use crate::share::runtime::javaThread::{JavaThread, JavaThreadState};
use crate::share::runtime::threads::Threads;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub const JNI_OK: i32 = 0;
pub const JNI_ERR: i32 = -1;
pub const JNI_EDETACHED: i32 = -2;
pub const JNI_EVERSION: i32 = -3;
pub const JNI_ENOMEM: i32 = -4;
pub const JNI_EEXIST: i32 = -5;
pub const JNI_EINVAL: i32 = -6;

pub const JNI_VERSION_1_1: i32 = 0x00010001;
pub const JNI_VERSION_1_2: i32 = 0x00010002;
pub const JNI_VERSION_1_4: i32 = 0x00010004;
pub const JNI_VERSION_1_6: i32 = 0x00010006;
pub const JNI_VERSION_1_8: i32 = 0x00010008;
pub const JNI_VERSION_9: i32 = 0x00090000;
pub const JNI_VERSION_19: i32 = 0x00130000;
pub const JNI_VERSION_21: i32 = 0x00150000;
pub const JNI_VERSION_25: i32 = 0x00190000;

pub type NativeMethodFn = fn() -> i32;

static VM_CREATED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct JavaVMOption {
    pub option_string: String,
}

#[derive(Debug, Clone)]
pub struct JavaVMInitArgs {
    pub version: i32,
    pub options: Vec<JavaVMOption>,
    pub ignore_unrecognized: bool,
}

impl JavaVMInitArgs {
    pub fn new(version: i32) -> Self {
        Self {
            version,
            options: Vec::new(),
            ignore_unrecognized: false,
        }
    }

    pub fn with_option(mut self, opt: &str) -> Self {
        self.options.push(JavaVMOption {
            option_string: opt.to_string(),
        });
        self
    }

    pub fn with_ignore_unrecognized(mut self, ignore: bool) -> Self {
        self.ignore_unrecognized = ignore;
        self
    }
}

impl Default for JavaVMInitArgs {
    fn default() -> Self {
        Self::new(JNI_VERSION_25)
    }
}

#[derive(Clone, Default)]
pub struct JniEnv {
    version: i32,
    native_methods: HashMap<(Symbol, Symbol, Symbol), NativeMethodFn>,
}

impl JniEnv {
    pub fn new() -> Self {
        Self {
            version: JNI_VERSION_25,
            native_methods: HashMap::new(),
        }
    }

    pub fn with_version(version: i32) -> Self {
        Self {
            version,
            native_methods: HashMap::new(),
        }
    }

    #[inline]
    pub fn get_version(&self) -> i32 {
        self.version
    }

    pub fn register_native(
        &mut self,
        class_name: Symbol,
        method_name: Symbol,
        signature: Symbol,
        func: NativeMethodFn,
    ) {
        self.native_methods
            .insert((class_name, method_name, signature), func);
    }

    pub fn lookup_native(
        &self,
        class_name: &Symbol,
        method_name: &Symbol,
        signature: &Symbol,
    ) -> Option<NativeMethodFn> {
        self.native_methods
            .get(&(class_name.clone(), method_name.clone(), signature.clone()))
            .copied()
    }

    pub fn unregister_natives(&mut self, class_name: &Symbol) {
        self.native_methods
            .retain(|(cls, _, _), _| cls != class_name);
    }

    pub fn native_count(&self) -> usize {
        self.native_methods.len()
    }
}

pub struct JavaVM {
    version: i32,
    main_thread_id: u64,
}

impl JavaVM {
    pub fn new(version: i32, main_thread_id: u64) -> Self {
        Self {
            version,
            main_thread_id,
        }
    }

    #[inline]
    pub fn version(&self) -> i32 {
        self.version
    }

    #[inline]
    pub fn main_thread_id(&self) -> u64 {
        self.main_thread_id
    }

    pub fn attach_current_thread(
        &self,
        name: Option<&str>,
        is_daemon: bool,
    ) -> Result<JniEnv, i32> {
        let thread_name = name.unwrap_or("AttachedThread").to_string();
        let mut jt = JavaThread::new(thread_name, is_daemon);
        jt.set_thread_state(JavaThreadState::InNative);
        Threads::global().add(jt);

        Ok(JniEnv::with_version(self.version))
    }

    pub fn attach_current_thread_as_daemon(&self, name: Option<&str>) -> Result<JniEnv, i32> {
        self.attach_current_thread(name, true)
    }

    pub fn detach_current_thread(&self, thread_id: u64) -> i32 {
        if Threads::global().remove(thread_id).is_some() {
            JNI_OK
        } else {
            JNI_EDETACHED
        }
    }

    pub fn get_env(&self, version: i32) -> Result<JniEnv, i32> {
        if version > self.version {
            return Err(JNI_EVERSION);
        }
        Ok(JniEnv::with_version(version))
    }

    pub fn destroy_java_vm(&mut self) -> i32 {
        VMInit::reset();
        Threads::global().clear();
        VM_CREATED.store(false, Ordering::SeqCst);
        JNI_OK
    }

    #[inline]
    pub fn is_created() -> bool {
        VM_CREATED.load(Ordering::SeqCst)
    }
}

/// JNI_GetCreatedJavaVMs entry point.
pub fn jni_get_created_java_vms() -> Vec<i32> {
    if VM_CREATED.load(Ordering::SeqCst) {
        vec![JNI_VERSION_25]
    } else {
        vec![]
    }
}

/// JNI_CreateJavaVM entry point matching OpenJDK HotSpot specification.
pub fn jni_create_java_vm(args: &JavaVMInitArgs) -> Result<(JavaVM, JniEnv), i32> {
    // 1. Verify JNI version
    if args.version < JNI_VERSION_1_1 || args.version > JNI_VERSION_25 {
        return Err(JNI_EVERSION);
    }

    // 2. Ensure only one VM instance is active at a time (matching HotSpot atomic check)
    if VM_CREATED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(JNI_EEXIST);
    }

    // 3. Initialize global subsystems
    if VMInit::init_globals().is_err() {
        VM_CREATED.store(false, Ordering::SeqCst);
        return Err(JNI_ERR);
    }

    // 4. Create and attach main JavaThread
    let mut main_thread = JavaThread::new("main".to_string(), false);
    main_thread.set_thread_state(JavaThreadState::InNative);
    let main_thread_id = main_thread.id();
    Threads::global().add(main_thread);

    // 5. Register well-known native routines
    let mut env = JniEnv::with_version(args.version);
    register_default_natives(&mut env);

    // 6. Mark VM as fully completed
    VMInit::set_init_completed();

    let vm = JavaVM::new(args.version, main_thread_id);
    Ok((vm, env))
}

fn register_default_natives(env: &mut JniEnv) {
    // Register System.currentTimeMillis, nanoTime, etc.
    env.register_native(
        Symbol::from("java/lang/System"),
        Symbol::from("currentTimeMillis"),
        Symbol::from("()J"),
        || 0,
    );
    env.register_native(
        Symbol::from("java/lang/System"),
        Symbol::from("nanoTime"),
        Symbol::from("()J"),
        || 0,
    );
}
