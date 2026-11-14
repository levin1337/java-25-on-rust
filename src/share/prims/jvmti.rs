/*
 * Port of OpenJDK 25 HotSpot JVM Tool Interface (JVMTI)
 * (share/prims/jvmtiEnv.cpp / jvmtiExport.cpp / jvmti.h)
 *
 * Implements JVMTI environment, capability negotiation,
 * breakpoint registry, event notification masks, and frame inspection.
 */

use crate::share::oops::symbol::Symbol;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum JvmtiError {
    None = 0,
    InvalidThread = 10,
    InvalidThreadGroup = 11,
    InvalidPriority = 12,
    ThreadNotSuspended = 13,
    ThreadSuspended = 14,
    ThreadNotAlive = 15,
    InvalidObject = 20,
    InvalidClass = 21,
    ClassNotPrepared = 22,
    InvalidMethodId = 23,
    InvalidLocation = 24,
    InvalidFieldId = 25,
    NoMoreFrames = 31,
    OpaqueFrame = 32,
    NotAvailable = 98,
    MustPossessCapability = 99,
    NullPointer = 100,
    IllegalArgument = 103,
    OutOfMemory = 110,
    Internal = 113,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JvmtiEvent {
    VmInit = 50,
    VmDeath = 51,
    ThreadStart = 52,
    ThreadEnd = 53,
    ClassFileLoadHook = 54,
    ClassLoad = 55,
    ClassPrepare = 56,
    Breakpoint = 62,
    FieldAccess = 63,
    FieldModification = 64,
    SingleStep = 65,
    FramePop = 66,
    MethodEntry = 67,
    MethodExit = 68,
    Exception = 69,
    GarbageCollectionStart = 81,
    GarbageCollectionFinish = 82,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmtiEventMode {
    Disable = 0,
    Enable = 1,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JvmtiCapabilities {
    pub can_tag_objects: bool,
    pub can_generate_field_modification_events: bool,
    pub can_generate_field_access_events: bool,
    pub can_get_bytecodes: bool,
    pub can_get_synthetic_attribute: bool,
    pub can_get_owned_monitor_info: bool,
    pub can_get_current_contended_monitor: bool,
    pub can_get_monitor_info: bool,
    pub can_pop_frame: bool,
    pub can_redefine_classes: bool,
    pub can_signal_thread: bool,
    pub can_get_source_file_name: bool,
    pub can_get_line_numbers: bool,
    pub can_get_source_debug_extension: bool,
    pub can_access_local_variables: bool,
    pub can_maintain_original_method_order: bool,
    pub can_generate_single_step_events: bool,
    pub can_generate_exception_events: bool,
    pub can_generate_frame_pop_events: bool,
    pub can_generate_breakpoint_events: bool,
    pub can_suspend: bool,
    pub can_redefine_any_class: bool,
    pub can_get_current_thread_cpu_time: bool,
    pub can_get_thread_cpu_time: bool,
    pub can_generate_method_entry_events: bool,
    pub can_generate_method_exit_events: bool,
    pub can_generate_all_class_hook_events: bool,
    pub can_generate_compiled_method_load_events: bool,
    pub can_generate_monitor_events: bool,
    pub can_generate_vm_object_alloc_events: bool,
    pub can_generate_native_method_bind_events: bool,
    pub can_generate_garbage_collection_events: bool,
    pub can_generate_object_free_events: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JvmtiBreakpoint {
    pub class_name: Symbol,
    pub method_name: Symbol,
    pub method_signature: Symbol,
    pub bci: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmtiFrameInfo {
    pub method_name: Symbol,
    pub location: i64, // bytecode index
}

pub struct JvmtiEnv {
    capabilities: RwLock<JvmtiCapabilities>,
    enabled_events: RwLock<HashSet<JvmtiEvent>>,
    breakpoints: RwLock<HashSet<JvmtiBreakpoint>>,
    thread_call_stacks: RwLock<HashMap<u64, Vec<JvmtiFrameInfo>>>,
}

impl JvmtiEnv {
    pub fn new() -> Self {
        Self {
            capabilities: RwLock::new(JvmtiCapabilities::default()),
            enabled_events: RwLock::new(HashSet::new()),
            breakpoints: RwLock::new(HashSet::new()),
            thread_call_stacks: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_potential_capabilities() -> JvmtiCapabilities {
        JvmtiCapabilities {
            can_tag_objects: true,
            can_generate_field_modification_events: true,
            can_generate_field_access_events: true,
            can_get_bytecodes: true,
            can_get_synthetic_attribute: true,
            can_get_owned_monitor_info: true,
            can_get_current_contended_monitor: true,
            can_get_monitor_info: true,
            can_pop_frame: true,
            can_redefine_classes: true,
            can_signal_thread: true,
            can_get_source_file_name: true,
            can_get_line_numbers: true,
            can_get_source_debug_extension: true,
            can_access_local_variables: true,
            can_maintain_original_method_order: true,
            can_generate_single_step_events: true,
            can_generate_exception_events: true,
            can_generate_frame_pop_events: true,
            can_generate_breakpoint_events: true,
            can_suspend: true,
            can_redefine_any_class: true,
            can_get_current_thread_cpu_time: true,
            can_get_thread_cpu_time: true,
            can_generate_method_entry_events: true,
            can_generate_method_exit_events: true,
            can_generate_all_class_hook_events: true,
            can_generate_compiled_method_load_events: true,
            can_generate_monitor_events: true,
            can_generate_vm_object_alloc_events: true,
            can_generate_native_method_bind_events: true,
            can_generate_garbage_collection_events: true,
            can_generate_object_free_events: true,
        }
    }

    pub fn add_capabilities(&self, caps: &JvmtiCapabilities) -> Result<(), JvmtiError> {
        let mut cur = self.capabilities.write().unwrap();
        // Merge requested capabilities
        if caps.can_tag_objects {
            cur.can_tag_objects = true;
        }
        if caps.can_generate_breakpoint_events {
            cur.can_generate_breakpoint_events = true;
        }
        if caps.can_get_bytecodes {
            cur.can_get_bytecodes = true;
        }
        if caps.can_get_source_file_name {
            cur.can_get_source_file_name = true;
        }
        if caps.can_get_line_numbers {
            cur.can_get_line_numbers = true;
        }
        if caps.can_generate_exception_events {
            cur.can_generate_exception_events = true;
        }
        if caps.can_generate_method_entry_events {
            cur.can_generate_method_entry_events = true;
        }
        if caps.can_generate_method_exit_events {
            cur.can_generate_method_exit_events = true;
        }
        if caps.can_generate_single_step_events {
            cur.can_generate_single_step_events = true;
        }
        if caps.can_suspend {
            cur.can_suspend = true;
        }
        if caps.can_pop_frame {
            cur.can_pop_frame = true;
        }
        if caps.can_redefine_classes {
            cur.can_redefine_classes = true;
        }
        if caps.can_generate_garbage_collection_events {
            cur.can_generate_garbage_collection_events = true;
        }
        Ok(())
    }

    pub fn get_capabilities(&self) -> JvmtiCapabilities {
        self.capabilities.read().unwrap().clone()
    }

    pub fn set_event_notification_mode(
        &self,
        mode: JvmtiEventMode,
        event_type: JvmtiEvent,
    ) -> Result<(), JvmtiError> {
        let caps = self.capabilities.read().unwrap();
        // Verify capability if needed
        match event_type {
            JvmtiEvent::Breakpoint if !caps.can_generate_breakpoint_events => {
                return Err(JvmtiError::MustPossessCapability);
            }
            JvmtiEvent::MethodEntry if !caps.can_generate_method_entry_events => {
                return Err(JvmtiError::MustPossessCapability);
            }
            JvmtiEvent::MethodExit if !caps.can_generate_method_exit_events => {
                return Err(JvmtiError::MustPossessCapability);
            }
            JvmtiEvent::SingleStep if !caps.can_generate_single_step_events => {
                return Err(JvmtiError::MustPossessCapability);
            }
            JvmtiEvent::Exception if !caps.can_generate_exception_events => {
                return Err(JvmtiError::MustPossessCapability);
            }
            JvmtiEvent::GarbageCollectionStart | JvmtiEvent::GarbageCollectionFinish
                if !caps.can_generate_garbage_collection_events =>
            {
                return Err(JvmtiError::MustPossessCapability);
            }
            _ => {}
        }

        let mut events = self.enabled_events.write().unwrap();
        match mode {
            JvmtiEventMode::Enable => {
                events.insert(event_type);
            }
            JvmtiEventMode::Disable => {
                events.remove(&event_type);
            }
        }
        Ok(())
    }

    pub fn is_event_enabled(&self, event_type: JvmtiEvent) -> bool {
        self.enabled_events.read().unwrap().contains(&event_type)
    }

    pub fn set_breakpoint(
        &self,
        class_name: Symbol,
        method_name: Symbol,
        method_signature: Symbol,
        bci: i64,
    ) -> Result<(), JvmtiError> {
        let caps = self.capabilities.read().unwrap();
        if !caps.can_generate_breakpoint_events {
            return Err(JvmtiError::MustPossessCapability);
        }
        if bci < 0 {
            return Err(JvmtiError::InvalidLocation);
        }

        let bp = JvmtiBreakpoint {
            class_name,
            method_name,
            method_signature,
            bci,
        };
        self.breakpoints.write().unwrap().insert(bp);
        Ok(())
    }

    pub fn clear_breakpoint(
        &self,
        class_name: &Symbol,
        method_name: &Symbol,
        method_signature: &Symbol,
        bci: i64,
    ) -> Result<(), JvmtiError> {
        let bp = JvmtiBreakpoint {
            class_name: class_name.clone(),
            method_name: method_name.clone(),
            method_signature: method_signature.clone(),
            bci,
        };
        if self.breakpoints.write().unwrap().remove(&bp) {
            Ok(())
        } else {
            Err(JvmtiError::NotAvailable)
        }
    }

    pub fn has_breakpoint(
        &self,
        class_name: &Symbol,
        method_name: &Symbol,
        method_signature: &Symbol,
        bci: i64,
    ) -> bool {
        let bp = JvmtiBreakpoint {
            class_name: class_name.clone(),
            method_name: method_name.clone(),
            method_signature: method_signature.clone(),
            bci,
        };
        self.breakpoints.read().unwrap().contains(&bp)
    }

    pub fn register_thread_stack(&self, thread_id: u64, frames: Vec<JvmtiFrameInfo>) {
        self.thread_call_stacks
            .write()
            .unwrap()
            .insert(thread_id, frames);
    }

    pub fn get_frame_count(&self, thread_id: u64) -> Result<usize, JvmtiError> {
        let stacks = self.thread_call_stacks.read().unwrap();
        if let Some(frames) = stacks.get(&thread_id) {
            Ok(frames.len())
        } else {
            Err(JvmtiError::InvalidThread)
        }
    }

    pub fn get_stack_trace(
        &self,
        thread_id: u64,
        start_depth: usize,
        max_frame_count: usize,
    ) -> Result<Vec<JvmtiFrameInfo>, JvmtiError> {
        let stacks = self.thread_call_stacks.read().unwrap();
        if let Some(frames) = stacks.get(&thread_id) {
            if start_depth > frames.len() {
                return Err(JvmtiError::IllegalArgument);
            }
            let slice = &frames[start_depth..];
            let count = max_frame_count.min(slice.len());
            Ok(slice[..count].to_vec())
        } else {
            Err(JvmtiError::InvalidThread)
        }
    }
}

impl Default for JvmtiEnv {
    fn default() -> Self {
        Self::new()
    }
}

// Global HotSpot JVMTI Export functions
pub struct JvmtiExport;

impl JvmtiExport {
    pub fn should_post_breakpoint(env: &JvmtiEnv) -> bool {
        env.is_event_enabled(JvmtiEvent::Breakpoint)
    }

    pub fn should_post_method_entry(env: &JvmtiEnv) -> bool {
        env.is_event_enabled(JvmtiEvent::MethodEntry)
    }

    pub fn should_post_method_exit(env: &JvmtiEnv) -> bool {
        env.is_event_enabled(JvmtiEvent::MethodExit)
    }

    pub fn should_post_exception(env: &JvmtiEnv) -> bool {
        env.is_event_enabled(JvmtiEvent::Exception)
    }

    pub fn should_post_garbage_collection(env: &JvmtiEnv) -> bool {
        env.is_event_enabled(JvmtiEvent::GarbageCollectionStart)
            || env.is_event_enabled(JvmtiEvent::GarbageCollectionFinish)
    }
}
