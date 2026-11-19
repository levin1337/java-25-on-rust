/*
 * Port of OpenJDK 25 HotSpot ObjectMonitor (share/runtime/objectMonitor.hpp / objectMonitor.cpp)
 *
 * Real blocking monitor: ownership, recursion, entry waiters and the
 * wait/notify set are coordinated by a mutex + condition variable so Java
 * threads on separate OS threads can contend, wait and notify correctly.
 */

use parking_lot::{Condvar, Mutex, MutexGuard};
use std::collections::VecDeque;

#[derive(Debug)]
struct Waiter {
    tid: u64,
    notified: bool,
}

#[derive(Debug, Default)]
struct MonitorState {
    owner: Option<u64>,
    recursions: u32,
    /// Threads blocked in `wait()` (monitor released while waiting).
    waiting: VecDeque<Waiter>,
    /// Threads blocked in `enter()` waiting for the owner to release.
    entry_waiters: usize,
}

/// A single Java object monitor. All state transitions happen under
/// `state`; threads sleep on `cond` and re-check their predicate after
/// every wakeup (spurious wakeups are legal).
pub struct ObjectMonitor {
    state: Mutex<MonitorState>,
    cond: Condvar,
}

impl ObjectMonitor {
    pub fn new(_object_handle: usize) -> Self {
        Self {
            state: Mutex::new(MonitorState::default()),
            cond: Condvar::new(),
        }
    }

    #[inline]
    pub fn is_owned_by(&self, thread_id: u64) -> bool {
        self.state.lock().owner == Some(thread_id)
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.state.lock().owner.is_some()
    }

    #[inline]
    pub fn waiters_count(&self) -> usize {
        self.state.lock().waiting.len()
    }

    /// Fast non-blocking acquire. Returns true when the monitor was taken
    /// (fresh or recursive).
    pub fn try_enter(&self, thread_id: u64) -> bool {
        let mut state = self.state.lock();
        match state.owner {
            None => {
                state.owner = Some(thread_id);
                state.recursions = 0;
                true
            }
            Some(owner) if owner == thread_id => {
                state.recursions = state.recursions.saturating_add(1);
                true
            }
            Some(_) => false,
        }
    }

    /// Blocking acquire: waits on the condition variable while another
    /// thread owns the monitor.
    pub fn enter_blocking(&self, thread_id: u64) {
        if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
            eprintln!("[block] enter_blocking tid={thread_id}");
        }
        let mut state = self.state.lock();
        let mut blocked = false;
        loop {
            match state.owner {
                None => {
                    state.owner = Some(thread_id);
                    state.recursions = 0;
                    if blocked {
                        crate::share::memory::heap::unquiesce();
                    }
                    return;
                }
                Some(owner) if owner == thread_id => {
                    state.recursions = state.recursions.saturating_add(1);
                    if blocked {
                        crate::share::memory::heap::unquiesce();
                    }
                    return;
                }
                Some(_) => {
                    state.entry_waiters += 1;
                    if !blocked {
                        // Contended monitor entry is a GC-quiesced block:
                        // the thread's frames are frozen while it waits.
                        crate::share::memory::heap::quiesce();
                        blocked = true;
                    }
                    self.cond.wait(&mut state);
                    state.entry_waiters = state.entry_waiters.saturating_sub(1);
                }
            }
        }
    }

    /// Releases one level of ownership. Returns `Ok(true)` when the monitor
    /// was fully released, `Ok(false)` on recursive exit, or an error when
    /// the caller does not own the monitor.
    pub fn exit(&self, thread_id: u64) -> Result<bool, &'static str> {
        let mut state = self.state.lock();
        if state.owner != Some(thread_id) {
            return Err("IllegalMonitorStateException");
        }
        let fully_released = if state.recursions > 0 {
            state.recursions -= 1;
            false
        } else {
            state.owner = None;
            true
        };
        // Wake entry waiters; wait-set members re-check `notified` and go
        // back to sleep, which is harmless.
        self.cond.notify_all();
        Ok(fully_released)
    }

    /// `Object.wait`: verifies ownership, fully releases the monitor,
    /// blocks until notified or the deadline expires, then re-acquires the
    /// monitor (restoring the saved recursion depth) before returning.
    ///
    /// `timeout_millis <= 0` waits indefinitely. Returns the saved
    /// recursion count.
    pub fn wait_blocking(
        &self,
        thread_id: u64,
        timeout_millis: i64,
    ) -> Result<u32, &'static str> {
        if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
            eprintln!("[block] wait_blocking tid={thread_id} timeout={timeout_millis}");
        }
        let mut state = self.state.lock();
        if state.owner != Some(thread_id) {
            return Err("IllegalMonitorStateException");
        }
        let saved_recursions = state.recursions;
        // Release the monitor completely, as the JMM requires.
        state.owner = None;
        state.recursions = 0;
        state.waiting.push_back(Waiter {
            tid: thread_id,
            notified: false,
        });
        self.cond.notify_all();

        let deadline = if timeout_millis > 0 {
            Some(std::time::Instant::now() + std::time::Duration::from_millis(timeout_millis as u64))
        } else {
            None
        };

        // Sleep until our waiter entry is notified or the deadline hits.
        loop {
            let idx = state.waiting.iter().position(|w| w.tid == thread_id);
            let Some(idx) = idx else {
                // Entry vanished (should not happen; treat as notified).
                break;
            };
            if state.waiting[idx].notified {
                break;
            }
            if let Some(d) = deadline {
                let now = std::time::Instant::now();
                if now >= d {
                    break;
                }
                let remaining = d - now;
                let _ = self.cond.wait_for(&mut state, remaining);
            } else {
                // JLS permits spurious wakeups from Object.wait. Return to
                // Java at least every 100ms so conditions checked there
                // (e.g. Thread.join's isAlive loop) are re-evaluated even
                // when a notify fired between the check and the wait.
                let _ = self
                    .cond
                    .wait_for(&mut state, std::time::Duration::from_millis(100));
                break;
            }
        }
        state.waiting.retain(|w| w.tid != thread_id);

        // Re-acquire the monitor, restoring the saved recursion depth.
        loop {
            match state.owner {
                None => {
                    state.owner = Some(thread_id);
                    state.recursions = saved_recursions;
                    return Ok(saved_recursions);
                }
                Some(owner) if owner == thread_id => {
                    state.recursions = saved_recursions;
                    return Ok(saved_recursions);
                }
                Some(_) => {
                    self.cond.wait(&mut state);
                }
            }
        }
    }

    /// `Object.notify`: wakes a single waiting thread. The caller must own
    /// the monitor.
    pub fn notify(&self, thread_id: u64) -> Result<bool, &'static str> {
        let mut state = self.lock_owned(thread_id)?;
        let idx = state.waiting.iter().position(|w| !w.notified);
        if let Some(idx) = idx {
            state.waiting[idx].notified = true;
            self.cond.notify_all();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// `Object.notifyAll`: wakes every waiting thread. The caller must own
    /// the monitor.
    pub fn notify_all(&self, thread_id: u64) -> Result<usize, &'static str> {
        let mut state = self.lock_owned(thread_id)?;
        let count = state.waiting.iter().filter(|w| !w.notified).count();
        for w in state.waiting.iter_mut() {
            w.notified = true;
        }
        self.cond.notify_all();
        Ok(count)
    }

    /// Runtime-internal release of every recursion level owned by
    /// `thread_id`, used when a Java thread terminates (JLS: monitors held
    /// by a terminating thread are released). Wakes entry waiters.
    pub fn force_release(&self, thread_id: u64) {
        let mut state = self.state.lock();
        if state.owner == Some(thread_id) {
            state.owner = None;
            state.recursions = 0;
            self.cond.notify_all();
        }
    }

    /// Runtime-internal broadcast that does not require monitor ownership.
    /// Used by thread termination so `join()` waiters observe the state
    /// change (equivalent to HotSpot's `Thread::exit` notifyAll).
    pub fn force_notify_all(&self) {
        let mut state = self.state.lock();
        for w in state.waiting.iter_mut() {
            w.notified = true;
        }
        self.cond.notify_all();
    }

    fn lock_owned(&self, thread_id: u64) -> Result<MutexGuard<'_, MonitorState>, &'static str> {
        let state = self.state.lock();
        if state.owner != Some(thread_id) {
            return Err("IllegalMonitorStateException");
        }
        Ok(state)
    }
}
