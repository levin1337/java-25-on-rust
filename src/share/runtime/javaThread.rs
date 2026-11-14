/*
 * Port of OpenJDK 25 HotSpot JavaThread (share/runtime/javaThread.hpp / javaThread.cpp)
 *
 * JavaThread represents an active thread of Java bytecode execution,
 * managing thread state transitions, stack limits, priorities, pending exceptions,
 * virtual thread (Loom) pinning detection, and safepoint coordination.
 */

use super::stackOverflow::StackOverflow;
use super::thread::{Thread, ThreadType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JavaThreadState {
    Uninitialized = 0,
    New = 2,
    NewTrans = 3,
    InNative = 4,
    InNativeTrans = 5,
    InVm = 6,
    InVmTrans = 7,
    InJava = 8,
    InJavaTrans = 9,
    Blocked = 10,
    BlockedTrans = 11,
    Waiting = 12,
    Terminated = 13,
}

impl JavaThreadState {
    #[inline]
    pub fn is_in_java(self) -> bool {
        matches!(self, Self::InJava | Self::InJavaTrans)
    }

    #[inline]
    pub fn is_in_vm(self) -> bool {
        matches!(self, Self::InVm | Self::InVmTrans)
    }

    #[inline]
    pub fn is_in_native(self) -> bool {
        matches!(self, Self::InNative | Self::InNativeTrans)
    }

    #[inline]
    pub fn is_blocked(self) -> bool {
        matches!(self, Self::Blocked | Self::BlockedTrans)
    }

    #[inline]
    pub fn is_safepoint_safe(self) -> bool {
        self.is_in_native() || self.is_blocked() || self == Self::Terminated
    }
}

#[derive(Debug)]
pub struct JavaThread {
    base: Thread,
    state: JavaThreadState,
    priority: i32,
    is_daemon: bool,
    stack_size: usize,
    stack_base: usize,
    stack_sp: usize,
    pending_exception: Option<String>,
    is_vthread: bool,
    held_monitor_count: usize,
    current_pending_monitor: Option<usize>,
    safepoint_poll_armed: bool,
}

impl JavaThread {
    pub const DEFAULT_STACK_SIZE: usize = 1024 * 1024; // 1MB

    pub fn new(name: String, is_daemon: bool) -> Self {
        Self {
            base: Thread::new(name, ThreadType::JavaThread),
            state: JavaThreadState::New,
            priority: 5,
            is_daemon,
            stack_size: Self::DEFAULT_STACK_SIZE,
            stack_base: 0,
            stack_sp: 0,
            pending_exception: None,
            is_vthread: false,
            held_monitor_count: 0,
            current_pending_monitor: None,
            safepoint_poll_armed: false,
        }
    }

    pub fn new_virtual(name: String) -> Self {
        let mut thread = Self::new(name, true);
        thread.is_vthread = true;
        thread
    }

    #[inline]
    pub fn id(&self) -> u64 {
        self.base.id()
    }

    #[inline]
    pub fn os_thread_id(&self) -> u64 {
        self.base.os_thread_id()
    }

    #[inline]
    pub fn set_os_thread_id(&mut self, os_id: u64) {
        self.base.set_os_thread_id(os_id);
    }

    #[inline]
    pub fn name(&self) -> &str {
        self.base.name()
    }

    #[inline]
    pub fn set_name(&mut self, name: String) {
        self.base.set_name(name);
    }

    #[inline]
    pub fn thread_state(&self) -> JavaThreadState {
        self.state
    }

    #[inline]
    pub fn set_thread_state(&mut self, state: JavaThreadState) {
        self.state = state;
    }

    #[inline]
    pub fn is_in_java(&self) -> bool {
        self.state.is_in_java()
    }

    #[inline]
    pub fn is_in_vm(&self) -> bool {
        self.state.is_in_vm()
    }

    #[inline]
    pub fn is_in_native(&self) -> bool {
        self.state.is_in_native()
    }

    #[inline]
    pub fn is_blocked(&self) -> bool {
        self.state.is_blocked()
    }

    #[inline]
    pub fn is_safepoint_safe(&self) -> bool {
        self.state.is_safepoint_safe()
    }

    #[inline]
    pub fn priority(&self) -> i32 {
        self.priority
    }

    #[inline]
    pub fn set_priority(&mut self, priority: i32) {
        self.priority = priority.clamp(1, 10);
    }

    #[inline]
    pub fn is_daemon(&self) -> bool {
        self.is_daemon
    }

    #[inline]
    pub fn set_daemon(&mut self, daemon: bool) {
        self.is_daemon = daemon;
    }

    #[inline]
    pub fn stack_size(&self) -> usize {
        self.stack_size
    }

    #[inline]
    pub fn set_stack_size(&mut self, size: usize) {
        self.stack_size = size;
    }

    #[inline]
    pub fn stack_base(&self) -> usize {
        self.stack_base
    }

    #[inline]
    pub fn set_stack_base(&mut self, base: usize) {
        self.stack_base = base;
    }

    #[inline]
    pub fn stack_sp(&self) -> usize {
        self.stack_sp
    }

    pub fn canReserveStack(&self, frameBytes: usize, reserved: usize) -> bool {
        StackOverflow::new(self.stack_base, self.stack_size, reserved)
            .is_some_and(|bounds| bounds.fits(self.stack_sp, frameBytes))
    }

    #[inline]
    pub fn set_stack_sp(&mut self, sp: usize) {
        self.stack_sp = sp;
    }

    #[inline]
    pub fn allocated_bytes(&self) -> u64 {
        self.base.allocated_bytes()
    }

    #[inline]
    pub fn record_allocation(&mut self, bytes: u64) {
        self.base.record_allocation(bytes);
    }

    #[inline]
    pub fn has_pending_exception(&self) -> bool {
        self.pending_exception.is_some()
    }

    #[inline]
    pub fn pending_exception(&self) -> Option<&str> {
        self.pending_exception.as_deref()
    }

    #[inline]
    pub fn set_pending_exception(&mut self, ex: String) {
        self.pending_exception = Some(ex);
    }

    #[inline]
    pub fn clear_pending_exception(&mut self) -> Option<String> {
        self.pending_exception.take()
    }

    #[inline]
    pub fn is_vthread(&self) -> bool {
        self.is_vthread
    }

    #[inline]
    pub fn set_vthread(&mut self, vthread: bool) {
        self.is_vthread = vthread;
    }

    #[inline]
    pub fn held_monitor_count(&self) -> usize {
        self.held_monitor_count
    }

    #[inline]
    pub fn inc_held_monitor_count(&mut self) {
        self.held_monitor_count = self.held_monitor_count.saturating_add(1);
    }

    #[inline]
    pub fn dec_held_monitor_count(&mut self) {
        self.held_monitor_count = self.held_monitor_count.saturating_sub(1);
    }

    #[inline]
    pub fn set_held_monitor_count(&mut self, count: usize) {
        self.held_monitor_count = count;
    }

    #[inline]
    pub fn current_pending_monitor(&self) -> Option<usize> {
        self.current_pending_monitor
    }

    #[inline]
    pub fn set_current_pending_monitor(&mut self, monitor: usize) {
        self.current_pending_monitor = Some(monitor);
    }

    #[inline]
    pub fn clear_current_pending_monitor(&mut self) {
        self.current_pending_monitor = None;
    }

    #[inline]
    pub fn is_safepoint_poll_armed(&self) -> bool {
        self.safepoint_poll_armed
    }

    #[inline]
    pub fn arm_safepoint_poll(&mut self) {
        self.safepoint_poll_armed = true;
    }

    #[inline]
    pub fn disarm_safepoint_poll(&mut self) {
        self.safepoint_poll_armed = false;
    }
}
