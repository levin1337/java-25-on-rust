/*
 * Port of OpenJDK 25 HotSpot SafepointMechanism (share/runtime/safepointMechanism.hpp)
 *
 * Implements safepoint polling and thread-local polling mechanisms for Stop-The-World
 * VM operations (GC, deoptimization, biased locking revocation, class redefinition).
 */

use super::javaThread::{JavaThread, JavaThreadState};
use std::sync::atomic::{AtomicBool, Ordering};

static GLOBAL_POLL_ARMED: AtomicBool = AtomicBool::new(false);

pub struct SafepointMechanism;

impl SafepointMechanism {
    #[inline]
    pub fn arm_global_poll() {
        GLOBAL_POLL_ARMED.store(true, Ordering::SeqCst);
    }

    #[inline]
    pub fn disarm_global_poll() {
        GLOBAL_POLL_ARMED.store(false, Ordering::SeqCst);
    }

    #[inline]
    pub fn global_poll() -> bool {
        GLOBAL_POLL_ARMED.load(Ordering::SeqCst)
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

    /// Safepoint polling check at bytecode/method entry/exit and loop back-edges.
    /// If safepoint is active, the thread transitions to Blocked, waits, and resumes in Java.
    pub fn process_if_requested(thread: &mut JavaThread) {
        if Self::should_process(thread) {
            let previous_state = thread.thread_state();
            thread.set_thread_state(JavaThreadState::Blocked);

            // In real HotSpot, thread blocks on WaitBarrier.
            // Here in cooperative safepoint, thread stays blocked until global poll is disarmed.
            while Self::global_poll() {
                std::hint::spin_loop();
            }

            thread.disarm_safepoint_poll();
            thread.set_thread_state(previous_state);
        }
    }
}
