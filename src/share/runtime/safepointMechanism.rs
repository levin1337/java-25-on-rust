/*
 * Port of OpenJDK 25 HotSpot SafepointMechanism (share/runtime/safepointMechanism.hpp)
 *
 * Implements safepoint polling and thread-local polling mechanisms for Stop-The-World
 * VM operations (GC, deoptimization, biased locking revocation, class redefinition).
 *
 * Two safepoint sources cooperate here:
 *  - the GC protocol in `share::memory::heap` (pending flag + GC_CV), which
 *    `heap::safepoint()` parks executing threads for; and
 *  - this module's VM-level safepoint (`GLOBAL_POLL_ARMED`) used by
 *    non-GC stop-the-world operations such as deoptimization and
 *    class redefinition, coordinated by `SafepointSynchronize`.
 * `process_if_requested` participates in both: it first parks inside
 * `heap::safepoint()` (no-op when no collection is pending) and then
 * waits on the VM-level poll barrier until the operation completes.
 */

use super::javaThread::{JavaThread, JavaThreadState};
use super::safepoint::SafepointSynchronize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

static GLOBAL_POLL_ARMED: AtomicBool = AtomicBool::new(false);
static GLOBAL_POLL_ID: AtomicU64 = AtomicU64::new(0);
static GLOBAL_POLL_LOCK: Mutex<()> = Mutex::new(());
static GLOBAL_POLL_CV: Condvar = Condvar::new();

pub struct SafepointMechanism;

impl SafepointMechanism {
    #[inline]
    pub fn arm_global_poll() {
        GLOBAL_POLL_ARMED.store(true, Ordering::SeqCst);
        GLOBAL_POLL_ID.fetch_add(1, Ordering::SeqCst);
    }

    #[inline]
    pub fn disarm_global_poll() {
        GLOBAL_POLL_ARMED.store(false, Ordering::SeqCst);
        let _guard = GLOBAL_POLL_LOCK.lock().unwrap();
        GLOBAL_POLL_CV.notify_all();
    }

    /// Called by `SafepointSynchronize::end` after the synchronize state
    /// has been reset to `NotSynchronized`, so threads parked in
    /// `process_if_requested` observe both predicates cleared.
    pub fn notify_safepoint_end() {
        let _guard = GLOBAL_POLL_LOCK.lock().unwrap();
        GLOBAL_POLL_CV.notify_all();
    }

    #[inline]
    pub fn global_poll() -> bool {
        GLOBAL_POLL_ARMED.load(Ordering::SeqCst)
    }

    /// Safepoint generation counter, analogous to HotSpot's
    /// `SafepointSynchronize::safepoint_counter` — lets a thread detect
    /// that it missed a completed safepoint.
    #[inline]
    pub fn global_poll_id() -> u64 {
        GLOBAL_POLL_ID.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn local_poll_armed(thread: &JavaThread) -> bool {
        thread.is_safepoint_poll_armed()
    }

    #[inline]
    pub fn arm_local_poll(thread: &mut JavaThread) {
        thread.arm_safepoint_poll();
    }

    #[inline]
    pub fn disarm_local_poll(thread: &mut JavaThread) {
        thread.disarm_safepoint_poll();
    }

    /// Determines if the thread should suspend or process a safepoint/handshake.
    pub fn should_process(thread: &JavaThread) -> bool {
        if !Self::global_poll() && !Self::local_poll_armed(thread) {
            return false;
        }
        thread.is_in_java() || thread.is_in_vm()
    }

    /// Safepoint polling check at bytecode/method entry/exit and loop
    /// back-edges.  If a safepoint is active the thread transitions to
    /// Blocked, reports its roots to the GC protocol via
    /// `heap::safepoint()` and sleeps on the poll barrier until the
    /// operation is disarmed, then resumes in Java.
    ///
    /// The initiating thread (the one inside
    /// `SafepointSynchronize::begin/end`) never reaches this wait: VM
    /// operations run on dedicated threads that do not poll while
    /// holding the safepoint.
    pub fn process_if_requested(thread: &mut JavaThread) {
        if !Self::should_process(thread) {
            return;
        }
        let previous_state = thread.thread_state();
        thread.set_thread_state(JavaThreadState::Blocked);

        // Park until both safepoint sources are clear.  The GC
        // safepoint is re-entered on every wakeup (it returns
        // immediately when no collection is pending): a thread parked
        // here must still be able to quiesce for the collector, which
        // waits for every registered thread to leave `Running`.
        loop {
            crate::share::memory::heap::safepoint();
            let guard = GLOBAL_POLL_LOCK.lock().unwrap();
            if !GLOBAL_POLL_ARMED.load(Ordering::SeqCst)
                && !SafepointSynchronize::global().is_synchronizing()
            {
                break;
            }
            let _ = GLOBAL_POLL_CV.wait_timeout(
                guard,
                std::time::Duration::from_millis(1),
            )
            .unwrap();
        }

        thread.disarm_safepoint_poll();
        thread.set_thread_state(previous_state);
    }
}
