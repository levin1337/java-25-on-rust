/*
 * Port of OpenJDK 25 HotSpot ObjectSynchronizer (share/runtime/synchronizer.hpp / synchronizer.cpp)
 *
 * Registry of per-object monitors. The map itself is guarded by the
 * caller's `Share` borrow and is only held for short lookups; all blocking
 * happens inside `ObjectMonitor`'s own mutex/condvar so Java threads on
 * separate OS threads never hold the registry lock while sleeping.
 */

use super::objectMonitor::ObjectMonitor;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct Synchronizer {
    monitors: HashMap<usize, Arc<ObjectMonitor>>,
}

pub type ObjectSynchronizer = Synchronizer;

impl Synchronizer {
    pub const DEFAULT_THREAD_ID: u64 = 1;

    pub fn new() -> Self {
        Self {
            monitors: HashMap::new(),
        }
    }

    /// Returns the monitor for `handle`, creating it on first use.
    /// Caller must drop the map borrow before any blocking monitor op.
    pub fn monitor(&mut self, handle: usize) -> Arc<ObjectMonitor> {
        Arc::clone(
            self.monitors
                .entry(handle)
                .or_insert_with(|| Arc::new(ObjectMonitor::new(handle))),
        )
    }

    /// Looks up an existing monitor without creating one.
    pub fn find(&self, handle: usize) -> Option<Arc<ObjectMonitor>> {
        self.monitors.get(&handle).cloned()
    }

    /// Non-blocking acquire attempt.
    pub fn try_enter(&mut self, handle: usize, thread_id: u64) -> bool {
        self.monitor(handle).try_enter(thread_id)
    }

    /// Releases one level of monitor ownership.
    pub fn exit(&mut self, handle: usize, thread_id: u64) -> Result<bool, &'static str> {
        match self.monitors.get(&handle) {
            Some(monitor) => monitor.exit(thread_id),
            None => Err("IllegalMonitorStateException"),
        }
    }

    /// Whether `thread_id` currently owns the monitor for `handle`.
    pub fn is_owned_by(&self, handle: usize, thread_id: u64) -> bool {
        self.monitors
            .get(&handle)
            .map(|m| m.is_owned_by(thread_id))
            .unwrap_or(false)
    }

    /// Releases every monitor owned by `thread_id`. Called when a Java
    /// thread terminates so its locks cannot wedge other threads.
    pub fn release_thread_monitors(&mut self, thread_id: u64) {
        for monitor in self.monitors.values() {
            monitor.force_release(thread_id);
        }
    }

    pub fn monitor_count(&self) -> usize {
        self.monitors.len()
    }
}
