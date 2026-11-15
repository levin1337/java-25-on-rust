/*
 * Port of OpenJDK 25 HotSpot ThreadSMR (share/runtime/threadSMR.hpp / threadSMR.cpp)
 *
 * ThreadSMR manages the VM's thread list: every `JavaThread` is linked
 * into a global list guarded by a lock, and stack walkers iterate it
 * through hazard-pointer-style "SMR handles" so a thread cannot be
 * freed while a walker holds it.
 *
 * In this port the thread list is `Threads::global()`'s registry of
 * `JavaThread` objects; `ThreadSMR` provides the iteration protocol
 * (`threads_do`-style traversal) and the `ThreadIdentifier` helpers on
 * top of it.  `ThreadSnapshot` remains the per-thread publication used
 * by diagnostic commands.
 */

use super::javaThread::{JavaThread, JavaThreadState};
use super::threads::Threads;

/// Per-thread publication used by diagnostic commands and JMX
/// (`ThreadMXBean.getThreadInfo` analogue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSnapshot {
    pub id: u64,
    pub name: String,
    pub state: JavaThreadState,
    pub daemon: bool,
}

impl From<&JavaThread> for ThreadSnapshot {
    fn from(thread: &JavaThread) -> Self {
        Self {
            id: thread.id(),
            name: thread.name().to_owned(),
            state: thread.thread_state(),
            daemon: thread.is_daemon(),
        }
    }
}

/// ThreadSMR traversal state.  `ThreadSMR` is `AllStatic` in C++; here it
/// is a zero-sized facade over `Threads::global()`.
pub struct ThreadSMR;

impl ThreadSMR {
    /// `ThreadsList::threads_do` analogue: visits every registered
    /// thread's snapshot in registration order.  The callback must not
    /// mutate the registry (HotSpot holds the threads_lock during the
    /// walk; here the registry mutex is held only while cloning each
    /// `JavaThread` for the snapshot).
    pub fn threads_do<F: FnMut(ThreadSnapshot)>(f: &mut F) {
        for snapshot in Threads::global().snapshots() {
            f(snapshot);
        }
    }

    /// `ThreadSMR::get_java_thread` analogue: finds a thread by its VM
    /// thread identifier.
    pub fn find_by_id(id: u64) -> Option<JavaThread> {
        Threads::global().get(id)
    }

    /// `ThreadSMR::is_attached_thread`/`is_java_thread` analogue: whether
    /// `id` names a live VM thread.
    pub fn is_issued(id: u64) -> bool {
        Threads::global().find_by_id(id)
    }

    /// `Threads::number_of_threads` shortcut used by JMX bean queries.
    pub fn number_of_threads() -> usize {
        Threads::global().number_of_threads()
    }

    /// `Threads::number_of_non_daemon_threads`.
    pub fn number_of_non_daemon_threads() -> usize {
        Threads::global().number_of_non_daemon_threads()
    }

    /// `Threads::number_of_daemon_threads`.
    pub fn number_of_daemon_threads() -> usize {
        Threads::global().number_of_daemon_threads()
    }

    /// `Threads::threads_do` for callers needing the live `JavaThread`
    /// objects (not just snapshots), e.g. handshake/stack-walk code.
    /// Returns clones; mutations must go through `Threads` mutators.
    pub fn java_threads() -> Vec<JavaThread> {
        Threads::global().all()
    }

    /// Counts threads by state category, used by `ThreadMXBean`
    /// `dumpAllThreads` summaries.
    pub fn state_counts() -> (usize, usize, usize, usize) {
        let (mut running, mut blocked, mut waiting, mut terminated) = (0, 0, 0, 0);
        Self::threads_do(&mut |snapshot| match snapshot.state {
            JavaThreadState::InJava | JavaThreadState::InJavaTrans | JavaThreadState::InVm
            | JavaThreadState::InVmTrans | JavaThreadState::InNative | JavaThreadState::InNativeTrans => {
                running += 1
            }
            JavaThreadState::Blocked | JavaThreadState::BlockedTrans
            | JavaThreadState::New | JavaThreadState::NewTrans | JavaThreadState::Uninitialized => {
                blocked += 1
            }
            JavaThreadState::Waiting => waiting += 1,
            JavaThreadState::Terminated => terminated += 1,
        });
        (running, blocked, waiting, terminated)
    }
}
