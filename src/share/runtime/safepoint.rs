/*
 * Port of OpenJDK 25 HotSpot SafepointSynchronize (share/runtime/safepoint.hpp / safepoint.cpp)
 *
 * Implements Stop-The-World (STW) safepoint coordination for GC pauses,
 * deoptimization, class redefinition, and thread handshakes.
 */

use super::safepointMechanism::SafepointMechanism;
use super::threads::Threads;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SynchronizeState {
    NotSynchronized = 0,
    Synchronizing = 1,
    Synchronized = 2,
}

#[derive(Debug)]
struct SafepointInner {
    state: AtomicU8,
    safepoint_id: AtomicU64,
    safepoint_counter: AtomicU64,
}

#[derive(Clone)]
pub struct SafepointSynchronize {
    inner: Arc<SafepointInner>,
}

static GLOBAL_SAFEPOINT: OnceLock<SafepointSynchronize> = OnceLock::new();

impl SafepointSynchronize {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SafepointInner {
                state: AtomicU8::new(SynchronizeState::NotSynchronized as u8),
                safepoint_id: AtomicU64::new(0),
                safepoint_counter: AtomicU64::new(0),
            }),
        }
    }

    pub fn global() -> &'static SafepointSynchronize {
        GLOBAL_SAFEPOINT.get_or_init(Self::new)
    }

    #[inline]
    pub fn state(&self) -> SynchronizeState {
        match self.inner.state.load(Ordering::SeqCst) {
            1 => SynchronizeState::Synchronizing,
            2 => SynchronizeState::Synchronized,
            _ => SynchronizeState::NotSynchronized,
        }
    }

    #[inline]
    pub fn is_at_safepoint(&self) -> bool {
        self.inner.state.load(Ordering::SeqCst) == SynchronizeState::Synchronized as u8
    }

    #[inline]
    pub fn is_synchronizing(&self) -> bool {
        self.inner.state.load(Ordering::SeqCst) == SynchronizeState::Synchronizing as u8
    }

    #[inline]
    pub fn safepoint_id(&self) -> u64 {
        self.inner.safepoint_id.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn safepoint_counter(&self) -> u64 {
        self.inner.safepoint_counter.load(Ordering::SeqCst)
    }

    /// Begins a global Stop-The-World safepoint, arming polling across all threads.
    pub fn begin(&self) -> u64 {
        self.begin_with_threads(Threads::global())
    }

    /// Begins a Stop-The-World safepoint using a specific Threads registry.
    pub fn begin_with_threads(&self, threads: &Threads) -> u64 {
        self.inner
            .state
            .store(SynchronizeState::Synchronizing as u8, Ordering::SeqCst);
        self.inner.safepoint_counter.fetch_add(1, Ordering::SeqCst);

        // Arm safepoint polling mechanism
        SafepointMechanism::arm_global_poll();
        threads.arm_all_safepoint_polls();

        let id = self.inner.safepoint_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner
            .state
            .store(SynchronizeState::Synchronized as u8, Ordering::SeqCst);
        id
    }

    /// Ends the global Stop-The-World safepoint, resuming execution.
    pub fn end(&self) {
        self.end_with_threads(Threads::global());
    }

    /// Ends the safepoint using a specific Threads registry.
    pub fn end_with_threads(&self, threads: &Threads) {
        SafepointMechanism::disarm_global_poll();
        threads.disarm_all_safepoint_polls();

        self.inner.safepoint_counter.fetch_add(1, Ordering::SeqCst);
        self.inner
            .state
            .store(SynchronizeState::NotSynchronized as u8, Ordering::SeqCst);
        // Wake threads parked in `SafepointMechanism::process_if_requested`
        // now that both the poll flag and the synchronize state are clear.
        SafepointMechanism::notify_safepoint_end();
    }

    pub fn tracker(&self) -> SafepointStateTracker {
        SafepointStateTracker {
            inner: Some(self.inner.clone()),
            safepoint_id: self.safepoint_id(),
            at_safepoint: self.is_at_safepoint(),
        }
    }
}

impl Default for SafepointSynchronize {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SafepointStateTracker {
    inner: Option<Arc<SafepointInner>>,
    safepoint_id: u64,
    at_safepoint: bool,
}

impl SafepointStateTracker {
    pub fn new(safepoint_id: u64, at_safepoint: bool) -> Self {
        Self {
            inner: None,
            safepoint_id,
            at_safepoint,
        }
    }

    pub fn safepoint_state_changed(&self) -> bool {
        if let Some(ref inner) = self.inner {
            let current_id = inner.safepoint_id.load(Ordering::SeqCst);
            let current_at =
                inner.state.load(Ordering::SeqCst) == SynchronizeState::Synchronized as u8;
            current_id != self.safepoint_id || current_at != self.at_safepoint
        } else {
            let global = SafepointSynchronize::global();
            global.safepoint_id() != self.safepoint_id
                || global.is_at_safepoint() != self.at_safepoint
        }
    }
}
