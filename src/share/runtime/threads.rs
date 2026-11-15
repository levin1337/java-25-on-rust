/*
 * Port of OpenJDK 25 HotSpot Threads (share/runtime/threads.hpp / threads.cpp)
 *
 * Threads manages the active thread registry in the JVM, tracking active threads,
 * daemon vs non-daemon threads, and facilitating thread traversal and safepoint pauses.
 */

use super::javaThread::JavaThread;
use super::threadSMR::ThreadSnapshot;
use std::sync::{Mutex, OnceLock};

pub struct Threads {
    threads: Mutex<Vec<JavaThread>>,
}

static GLOBAL_THREADS: OnceLock<Threads> = OnceLock::new();

impl Threads {
    pub fn new() -> Self {
        Self {
            threads: Mutex::new(Vec::new()),
        }
    }

    pub fn global() -> &'static Threads {
        GLOBAL_THREADS.get_or_init(Self::new)
    }

    pub fn add(&self, thread: JavaThread) {
        let mut guard = self.threads.lock().unwrap();
        guard.push(thread);
    }

    pub fn remove(&self, id: u64) -> Option<JavaThread> {
        let mut guard = self.threads.lock().unwrap();
        if let Some(pos) = guard.iter().position(|t| t.id() == id) {
            Some(guard.remove(pos))
        } else {
            None
        }
    }

    pub fn find_by_id(&self, id: u64) -> bool {
        let guard = self.threads.lock().unwrap();
        guard.iter().any(|t| t.id() == id)
    }

    /// Returns a clone of the thread registered under `id`, if any —
    /// the `ThreadSMR::get_java_thread` analogue for callers that need
    /// live thread state rather than a `ThreadSnapshot`.
    pub fn get(&self, id: u64) -> Option<JavaThread> {
        let guard = self.threads.lock().unwrap();
        guard.iter().find(|t| t.id() == id).cloned()
    }

    /// Clones of all registered threads; used by `ThreadSMR::java_threads`
    /// and handshake-style traversals that need mutable thread state.
    pub fn all(&self) -> Vec<JavaThread> {
        self.threads.lock().unwrap().clone()
    }

    pub fn number_of_threads(&self) -> usize {
        self.threads.lock().unwrap().len()
    }

    pub fn snapshots(&self) -> Vec<ThreadSnapshot> {
        self.threads
            .lock()
            .unwrap()
            .iter()
            .map(ThreadSnapshot::from)
            .collect()
    }

    pub fn number_of_non_daemon_threads(&self) -> usize {
        self.threads
            .lock()
            .unwrap()
            .iter()
            .filter(|t| !t.is_daemon())
            .count()
    }

    pub fn number_of_daemon_threads(&self) -> usize {
        self.threads
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.is_daemon())
            .count()
    }

    pub fn thread_names(&self) -> Vec<String> {
        self.threads
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    }

    pub fn clear(&self) {
        self.threads.lock().unwrap().clear();
    }

    pub fn arm_all_safepoint_polls(&self) {
        let mut guard = self.threads.lock().unwrap();
        for thread in guard.iter_mut() {
            thread.arm_safepoint_poll();
        }
    }

    pub fn disarm_all_safepoint_polls(&self) {
        let mut guard = self.threads.lock().unwrap();
        for thread in guard.iter_mut() {
            thread.disarm_safepoint_poll();
        }
    }

    pub fn all_threads_safepoint_safe(&self) -> bool {
        let guard = self.threads.lock().unwrap();
        guard.iter().all(|t| t.is_safepoint_safe())
    }
}

impl Default for Threads {
    fn default() -> Self {
        Self::new()
    }
}
