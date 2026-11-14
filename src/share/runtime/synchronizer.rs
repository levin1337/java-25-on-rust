/*
 * Port of OpenJDK 25 HotSpot ObjectSynchronizer (share/runtime/synchronizer.hpp / synchronizer.cpp)
 *
 * Implements JVM synchronization coordinating object monitors, lock inflation,
 * recursive monitorenter/monitorexit, and monitor deflation.
 */

use super::objectMonitor::ObjectMonitor;
use std::collections::HashMap;
use std::io::{self, Error};

#[derive(Default)]
pub struct Synchronizer {
    monitors: HashMap<usize, ObjectMonitor>,
}

pub type ObjectSynchronizer = Synchronizer;

impl Synchronizer {
    pub const DEFAULT_THREAD_ID: u64 = 1;

    pub fn new() -> Self {
        Self {
            monitors: HashMap::new(),
        }
    }

    /// Default thread enter for simple single-threaded or test scenarios
    pub fn enter(&mut self, handle: usize) {
        self.enter_thread(handle, Self::DEFAULT_THREAD_ID);
    }

    /// Default thread exit returning io::Result for compatibility with legacy tests
    pub fn exit(&mut self, handle: usize) -> io::Result<()> {
        match self.exit_thread(handle, Self::DEFAULT_THREAD_ID) {
            Ok(_) => Ok(()),
            Err(msg) => Err(Error::other(msg)),
        }
    }

    /// Acquires or re-enters the monitor for the given thread_id.
    pub fn enter_thread(&mut self, handle: usize, thread_id: u64) -> bool {
        let monitor = self
            .monitors
            .entry(handle)
            .or_insert_with(|| ObjectMonitor::new(handle));
        monitor.enter(thread_id)
    }

    /// Non-blocking attempt to acquire the monitor.
    pub fn try_enter_thread(&mut self, handle: usize, thread_id: u64) -> bool {
        let monitor = self
            .monitors
            .entry(handle)
            .or_insert_with(|| ObjectMonitor::new(handle));
        monitor.try_enter(thread_id)
    }

    /// Releases or decrements recursion count of the monitor.
    pub fn exit_thread(&mut self, handle: usize, thread_id: u64) -> Result<bool, &'static str> {
        let monitor = self
            .monitors
            .get_mut(&handle)
            .ok_or("IllegalMonitorStateException")?;
        let fully_released = monitor.exit(thread_id)?;
        if fully_released && !monitor.is_busy() {
            self.monitors.remove(&handle);
        }
        Ok(fully_released)
    }

    /// Waits on the monitor, releasing the lock.
    pub fn wait_thread(&mut self, handle: usize, thread_id: u64) -> Result<u32, &'static str> {
        let monitor = self
            .monitors
            .get_mut(&handle)
            .ok_or("IllegalMonitorStateException")?;
        monitor.wait(thread_id)
    }

    /// Re-enters the monitor after wait completes.
    pub fn reenter_thread(&mut self, handle: usize, thread_id: u64, recursions: u32) -> bool {
        if let Some(monitor) = self.monitors.get_mut(&handle) {
            monitor.reenter_after_wait(thread_id, recursions)
        } else {
            false
        }
    }

    /// Notifies one waiting thread.
    pub fn notify_thread(&mut self, handle: usize, thread_id: u64) -> Result<bool, &'static str> {
        let monitor = self
            .monitors
            .get_mut(&handle)
            .ok_or("IllegalMonitorStateException")?;
        monitor.notify(thread_id)
    }

    /// Notifies all waiting threads.
    pub fn notify_all_thread(
        &mut self,
        handle: usize,
        thread_id: u64,
    ) -> Result<usize, &'static str> {
        let monitor = self
            .monitors
            .get_mut(&handle)
            .ok_or("IllegalMonitorStateException")?;
        monitor.notify_all(thread_id)
    }

    /// Checks if a thread currently holds the lock for the given object.
    #[inline]
    pub fn current_thread_holds_lock(&self, handle: usize, thread_id: u64) -> bool {
        self.monitors
            .get(&handle)
            .map(|m| m.is_owned_by(thread_id))
            .unwrap_or(false)
    }

    #[inline]
    pub fn is_locked(&self, handle: usize) -> bool {
        self.monitors
            .get(&handle)
            .map(|m| m.is_locked())
            .unwrap_or(false)
    }

    #[inline]
    pub fn get_monitor(&self, handle: usize) -> Option<&ObjectMonitor> {
        self.monitors.get(&handle)
    }

    #[inline]
    pub fn get_monitor_mut(&mut self, handle: usize) -> Option<&mut ObjectMonitor> {
        self.monitors.get_mut(&handle)
    }

    #[inline]
    pub fn monitor_count(&self) -> usize {
        self.monitors.len()
    }

    /// Deflates idle (unowned, uncontended, no waiters) monitors.
    pub fn deflate_idle_monitors(&mut self) -> usize {
        let initial_count = self.monitors.len();
        self.monitors.retain(|_, monitor| monitor.is_busy());
        initial_count.saturating_sub(self.monitors.len())
    }
}
