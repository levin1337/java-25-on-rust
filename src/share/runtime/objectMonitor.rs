/*
 * Port of OpenJDK 25 HotSpot ObjectMonitor (share/runtime/objectMonitor.hpp / objectMonitor.cpp)
 *
 * Implements heavyweight Java monitors with recursive lock/unlock,
 * owner tracking, entry queues, and wait/notify coordination.
 */

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct ObjectMonitor {
    owner: Option<u64>,
    recursions: u32,
    object_handle: usize,
    entry_list: VecDeque<u64>,
    wait_set: VecDeque<u64>,
    contentions: u32,
    succ: Option<u64>,
}

impl ObjectMonitor {
    pub fn new(object_handle: usize) -> Self {
        Self {
            owner: None,
            recursions: 0,
            object_handle,
            entry_list: VecDeque::new(),
            wait_set: VecDeque::new(),
            contentions: 0,
            succ: None,
        }
    }

    #[inline]
    pub fn object_handle(&self) -> usize {
        self.object_handle
    }

    #[inline]
    pub fn owner(&self) -> Option<u64> {
        self.owner
    }

    #[inline]
    pub fn recursions(&self) -> u32 {
        self.recursions
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.owner.is_some()
    }

    #[inline]
    pub fn is_owned_by(&self, thread_id: u64) -> bool {
        self.owner == Some(thread_id)
    }

    #[inline]
    pub fn contentions(&self) -> u32 {
        self.contentions
    }

    #[inline]
    pub fn waiters_count(&self) -> usize {
        self.wait_set.len()
    }

    #[inline]
    pub fn entry_list_count(&self) -> usize {
        self.entry_list.len()
    }

    /// Determines if the monitor is currently in use and cannot be deflated.
    #[inline]
    pub fn is_busy(&self) -> bool {
        self.owner.is_some()
            || !self.wait_set.is_empty()
            || !self.entry_list.is_empty()
            || self.contentions > 0
    }

    /// Enter monitor for the specified thread.
    /// Returns true if lock was immediately acquired or re-entered,
    /// or false if contended and enqueued on entry_list.
    pub fn enter(&mut self, thread_id: u64) -> bool {
        match self.owner {
            None => {
                self.owner = Some(thread_id);
                self.recursions = 0;
                true
            }
            Some(curr_owner) if curr_owner == thread_id => {
                self.recursions = self.recursions.saturating_add(1);
                true
            }
            Some(_) => {
                self.entry_list.push_back(thread_id);
                self.contentions = self.contentions.saturating_add(1);
                false
            }
        }
    }

    /// Try non-blocking acquisition of the monitor.
    pub fn try_enter(&mut self, thread_id: u64) -> bool {
        match self.owner {
            None => {
                self.owner = Some(thread_id);
                self.recursions = 0;
                true
            }
            Some(curr_owner) if curr_owner == thread_id => {
                self.recursions = self.recursions.saturating_add(1);
                true
            }
            Some(_) => false,
        }
    }

    /// Exit monitor for the specified thread.
    /// Returns:
    /// - Ok(false) if monitor was decremented but is still held (reentrant).
    /// - Ok(true) if monitor was fully released.
    /// - Err if the calling thread does not hold the monitor.
    pub fn exit(&mut self, thread_id: u64) -> Result<bool, &'static str> {
        if self.owner != Some(thread_id) {
            return Err("IllegalMonitorStateException");
        }

        if self.recursions > 0 {
            self.recursions -= 1;
            Ok(false)
        } else {
            self.owner = None;
            if let Some(next_thread) = self.entry_list.pop_front() {
                self.contentions = self.contentions.saturating_sub(1);
                self.owner = Some(next_thread);
                self.recursions = 0;
                self.succ = Some(next_thread);
            } else {
                self.succ = None;
            }
            Ok(true)
        }
    }

    /// Waits on the monitor, releasing the lock and adding thread to wait_set.
    /// Returns saved recursions on success.
    pub fn wait(&mut self, thread_id: u64) -> Result<u32, &'static str> {
        if self.owner != Some(thread_id) {
            return Err("IllegalMonitorStateException");
        }

        let saved_recursions = self.recursions;
        self.recursions = 0;
        self.owner = None;
        self.wait_set.push_back(thread_id);

        // Hand off ownership to next entry waiter if available
        if let Some(next_thread) = self.entry_list.pop_front() {
            self.contentions = self.contentions.saturating_sub(1);
            self.owner = Some(next_thread);
            self.recursions = 0;
            self.succ = Some(next_thread);
        } else {
            self.succ = None;
        }

        Ok(saved_recursions)
    }

    /// Reenters the monitor after wait completes, restoring saved recursions.
    pub fn reenter_after_wait(&mut self, thread_id: u64, saved_recursions: u32) -> bool {
        match self.owner {
            None => {
                self.owner = Some(thread_id);
                self.recursions = saved_recursions;
                true
            }
            // The monitor was handed to us directly by `exit`.
            Some(owner) if owner == thread_id => {
                self.recursions = saved_recursions;
                true
            }
            Some(_) => {
                if !self.entry_list.contains(&thread_id) {
                    self.entry_list.push_back(thread_id);
                    self.contentions = self.contentions.saturating_add(1);
                }
                false
            }
        }
    }

    /// Removes a thread from both the wait set and the entry list. Used when
    /// a timed wait expires without a matching notify, or when a thread
    /// leaves the monitor for any other reason.
    pub fn remove_waiter(&mut self, thread_id: u64) -> bool {
        let before = self.wait_set.len() + self.entry_list.len();
        self.wait_set.retain(|&id| id != thread_id);
        self.entry_list.retain(|&id| id != thread_id);
        before != self.wait_set.len() + self.entry_list.len()
    }

    /// Notifies one waiting thread from wait_set.
    pub fn notify(&mut self, thread_id: u64) -> Result<bool, &'static str> {
        if self.owner != Some(thread_id) {
            return Err("IllegalMonitorStateException");
        }

        Ok(self.wait_set.pop_front().is_some())
    }

    /// Notifies all waiting threads from wait_set.
    pub fn notify_all(&mut self, thread_id: u64) -> Result<usize, &'static str> {
        if self.owner != Some(thread_id) {
            return Err("IllegalMonitorStateException");
        }

        let count = self.wait_set.len();
        self.wait_set.clear();
        Ok(count)
    }
}
