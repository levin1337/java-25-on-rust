/*
 * Port of OpenJDK 25 HotSpot Thread (share/runtime/thread.hpp / thread.cpp)
 *
 * Base Thread class representing VM, Java, GC, Compiler, and Watcher execution threads.
 */

use super::threadIdentifier::ThreadIdentifier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadType {
    VMThread,
    JavaThread,
    CompilerThread,
    GCThread,
    WatcherThread,
}

#[derive(Debug)]
pub struct Thread {
    id: u64,
    os_thread_id: u64,
    name: String,
    thread_type: ThreadType,
    allocated_bytes: u64,
}

impl Thread {
    pub fn new(name: String, thread_type: ThreadType) -> Self {
        let id = ThreadIdentifier::allocate();
        Self {
            id,
            os_thread_id: id,
            name,
            thread_type,
            allocated_bytes: 0,
        }
    }

    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub fn os_thread_id(&self) -> u64 {
        self.os_thread_id
    }

    #[inline]
    pub fn set_os_thread_id(&mut self, os_id: u64) {
        self.os_thread_id = os_id;
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    #[inline]
    pub fn thread_type(&self) -> ThreadType {
        self.thread_type
    }

    #[inline]
    pub fn is_java_thread(&self) -> bool {
        self.thread_type == ThreadType::JavaThread
    }

    #[inline]
    pub fn is_vm_thread(&self) -> bool {
        self.thread_type == ThreadType::VMThread
    }

    #[inline]
    pub fn is_compiler_thread(&self) -> bool {
        self.thread_type == ThreadType::CompilerThread
    }

    #[inline]
    pub fn is_gc_thread(&self) -> bool {
        self.thread_type == ThreadType::GCThread
    }

    #[inline]
    pub fn is_watcher_thread(&self) -> bool {
        self.thread_type == ThreadType::WatcherThread
    }

    #[inline]
    pub fn allocated_bytes(&self) -> u64 {
        self.allocated_bytes
    }

    #[inline]
    pub fn record_allocation(&mut self, bytes: u64) {
        self.allocated_bytes = self.allocated_bytes.saturating_add(bytes);
    }
}
