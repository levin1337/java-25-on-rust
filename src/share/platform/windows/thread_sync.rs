/*
 * Port of OpenJDK 25 HotSpot os:: thread primitives (osThread_windows.cpp,
 * os_windows.cpp os::pd_start_thread / SuspendedThreadTask suspend-resume).
 *
 * Covers:
 *   - os::create_thread / _beginthreadex equivalent via CreateThread with
 *     CREATE_SUSPENDED (HotSpot creates suspended, then os::pd_start_thread
 *     resumes — same two-phase contract)
 *   - SuspendThread / ResumeThread / TerminateThread (os::suspend_thread
 *     path used by the thread sampler and JNI suspend)
 *   - Event / Mutex / Semaphore wrappers (os::win32::Event, mutex_windows,
 *     semaphore_windows)
 *   - Processor group affinity (GetThreadGroupAffinity /
 *     SetThreadGroupAffinity) — required on >64-CPU machines
 */

use super::{GroupAffinity, Handle, OsWindows, last_error_io};
use std::ffi::c_void;
use std::io;

pub const CREATE_SUSPENDED: u32 = 0x0000_0004;
pub const STILL_ACTIVE: u32 = 259;

pub const THREAD_TERMINATE: u32 = 0x0001;
pub const THREAD_SUSPEND_RESUME: u32 = 0x0002;
pub const THREAD_GET_CONTEXT: u32 = 0x0008;
pub const THREAD_SET_CONTEXT: u32 = 0x0010;
pub const THREAD_SET_INFORMATION: u32 = 0x0020;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0040;
pub const THREAD_SET_THREAD_TOKEN: u32 = 0x0080;
pub const THREAD_IMPERSONATE: u32 = 0x0100;
pub const THREAD_DIRECT_IMPERSONATION: u32 = 0x0200;
pub const THREAD_ALL_ACCESS: u32 = 0x001F_FFFF;

const THREAD_SUSPEND_RESUME_CONTEXT: u32 =
    THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;

pub const WAIT_OBJECT_0: u32 = 0x0000_0000;
pub const WAIT_ABANDONED: u32 = 0x0000_0080;
pub const WAIT_TIMEOUT: u32 = 0x0000_0102;
pub const WAIT_FAILED: u32 = 0xFFFF_FFFF;
pub const INFINITE: u32 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Thread
// ---------------------------------------------------------------------------

struct ThreadPayload {
    f: Box<dyn FnOnce() + Send>,
}

unsafe extern "system" fn thread_trampoline(param: *mut c_void) -> u32 {
    if param.is_null() {
        return 1;
    }
    let payload = unsafe { Box::from_raw(param as *mut ThreadPayload) };
    let f = payload.f;
    f();
    0
}

/// Raw Win32 thread handle wrapper. Mirrors os::Thread's handle ownership:
/// dropping without join detaches (thread keeps running), join() waits for
/// exit and reaps the handle.
pub struct Thread {
    handle: Handle,
    id: u32,
}

impl Thread {
    /// CreateThread equivalent: optional stack size, optionally suspended
    /// (HotSpot's CREATE_SUSPENDED two-phase start).
    pub fn create<F>(stack_size: usize, suspended: bool, f: F) -> io::Result<Thread>
    where
        F: FnOnce() + Send + 'static,
    {
        let payload = Box::new(ThreadPayload { f: Box::new(f) });
        let raw = Box::into_raw(payload);
        let flags = if suspended { CREATE_SUSPENDED } else { 0 };
        let mut id: u32 = 0;
        let handle = unsafe {
            super::CreateThread(
                std::ptr::null_mut(),
                stack_size,
                thread_trampoline,
                raw as *mut _,
                flags,
                &mut id,
            )
        };
        if handle.is_null() || handle == super::INVALID_HANDLE_VALUE {
            // Free the payload so the trampoline never sees a dangling box.
            unsafe {
                drop(Box::from_raw(raw));
            }
            return Err(last_error_io("CreateThread"));
        }
        Ok(Thread { handle, id })
    }

    /// Spawn a running thread (Rust std::thread equivalent).
    pub fn spawn<F>(stack_size: usize, f: F) -> io::Result<Thread>
    where
        F: FnOnce() + Send + 'static,
    {
        Self::create(stack_size, false, f)
    }

    /// os::pd_start_thread: ResumeThread on a CREATE_SUSPENDED thread.
    /// Returns the previous suspend count (0 = was running).
    pub fn start(&self) -> io::Result<u32> {
        self.resume()
    }

    pub fn suspend(&self) -> io::Result<u32> {
        let prev = unsafe { super::SuspendThread(self.handle) };
        if prev == u32::MAX {
            return Err(last_error_io("SuspendThread"));
        }
        Ok(prev)
    }

    pub fn resume(&self) -> io::Result<u32> {
        let prev = unsafe { super::ResumeThread(self.handle) };
        if prev == u32::MAX {
            return Err(last_error_io("ResumeThread"));
        }
        Ok(prev)
    }

    /// TerminateThread — last resort, same as os::win32 abort paths.
    pub fn terminate(&self, exit_code: u32) -> io::Result<()> {
        unsafe {
            if super::TerminateThread(self.handle, exit_code) == 0 {
                return Err(last_error_io("TerminateThread"));
            }
        }
        Ok(())
    }

    pub fn wait(&self, timeout_ms: u32) -> io::Result<u32> {
        let r = unsafe { super::WaitForSingleObject(self.handle, timeout_ms) };
        Ok(r)
    }

    pub fn wait_infinite(&self) -> io::Result<u32> {
        self.wait(INFINITE)
    }

    /// Exit code; STILL_ACTIVE while the thread runs.
    pub fn exit_code(&self) -> io::Result<u32> {
        let mut code: u32 = 0;
        unsafe {
            if super::GetExitCodeThread(self.handle, &mut code) == 0 {
                return Err(last_error_io("GetExitCodeThread"));
            }
        }
        Ok(code)
    }

    pub fn is_finished(&self) -> bool {
        self.exit_code().map(|c| c != STILL_ACTIVE).unwrap_or(true)
    }

    /// Wait for thread exit and return its exit code.
    pub fn join(self) -> io::Result<u32> {
        self.wait_infinite()?;
        self.exit_code()
    }

    /// Detach: close our handle reference; thread keeps running.
    pub fn detach(mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = super::CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn raw_handle(&self) -> Handle {
        self.handle
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = super::CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// OsWindows impl additions: open_thread, group affinity, thread times
// ---------------------------------------------------------------------------

impl OsWindows {
    /// OpenThread(dwDesiredAccess, tid) — used by the sampler to grab a
    /// suspend/resume+context handle for another thread.
    pub fn open_thread(tid: u32, access: u32) -> io::Result<Handle> {
        let h = unsafe { super::OpenThread(access, 0, tid) };
        if h.is_null() {
            return Err(last_error_io("OpenThread"));
        }
        Ok(h)
    }

    /// OpenThread with the sampler's access mask
    /// (THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION).
    pub fn open_thread_for_sampling(tid: u32) -> io::Result<Handle> {
        Self::open_thread(tid, THREAD_SUSPEND_RESUME_CONTEXT)
    }

    /// SuspendThread on a raw HANDLE — returns previous suspend count.
    pub fn suspend_thread_handle(handle: Handle) -> io::Result<u32> {
        let prev = unsafe { super::SuspendThread(handle) };
        if prev == u32::MAX {
            return Err(last_error_io("SuspendThread"));
        }
        Ok(prev)
    }

    /// ResumeThread on a raw HANDLE — returns previous suspend count.
    pub fn resume_thread_handle(handle: Handle) -> io::Result<u32> {
        let prev = unsafe { super::ResumeThread(handle) };
        if prev == u32::MAX {
            return Err(last_error_io("ResumeThread"));
        }
        Ok(prev)
    }

    /// GetThreadGroupAffinity — which processor group + mask a thread uses.
    pub fn thread_group_affinity(handle: Handle) -> io::Result<GroupAffinity> {
        unsafe {
            let mut ga = GroupAffinity::default();
            if super::GetThreadGroupAffinity(handle, &mut ga) == 0 {
                return Err(last_error_io("GetThreadGroupAffinity"));
            }
            Ok(ga)
        }
    }

    /// SetThreadGroupAffinity — returns the previous affinity.
    pub fn set_thread_group_affinity(
        handle: Handle,
        ga: &GroupAffinity,
    ) -> io::Result<GroupAffinity> {
        unsafe {
            let mut prev = GroupAffinity::default();
            let mut new = *ga;
            if super::SetThreadGroupAffinity(handle, &mut new, &mut prev) == 0 {
                return Err(last_error_io("SetThreadGroupAffinity"));
            }
            Ok(prev)
        }
    }

    /// Current thread's group affinity (GetCurrentThread pseudo-handle).
    pub fn current_thread_group_affinity() -> io::Result<GroupAffinity> {
        Self::thread_group_affinity(unsafe { super::GetCurrentThread() })
    }

    /// GetThreadTimes for a raw handle → (kernel_100ns, user_100ns).
    pub fn thread_cpu_time(handle: Handle) -> io::Result<(i64, i64)> {
        unsafe {
            let mut created = super::FileTime::default();
            let mut exited = super::FileTime::default();
            let mut kernel = super::FileTime::default();
            let mut user = super::FileTime::default();
            if super::GetThreadTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) == 0
            {
                return Err(last_error_io("GetThreadTimes"));
            }
            Ok((kernel.ticks(), user.ticks()))
        }
    }
}

// ---------------------------------------------------------------------------
// Event (os::win32::Event / park_event)
// ---------------------------------------------------------------------------

pub struct Event {
    handle: Handle,
}

impl Event {
    /// CreateEventW(manual_reset, initial_state, name).
    pub fn new(manual_reset: bool, initial_state: bool) -> io::Result<Event> {
        Self::new_named(manual_reset, initial_state, None)
    }

    pub fn new_named(
        manual_reset: bool,
        initial_state: bool,
        name: Option<&str>,
    ) -> io::Result<Event> {
        let wide_name: Option<Vec<u16>> = name.map(|n| {
            std::ffi::OsStr::new(n)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        });
        let h = unsafe {
            super::CreateEventW(
                std::ptr::null_mut(),
                manual_reset as i32,
                initial_state as i32,
                wide_name.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            )
        };
        if h.is_null() {
            return Err(last_error_io("CreateEventW"));
        }
        Ok(Event { handle: h })
    }

    pub fn wait(&self, timeout_ms: u32) -> io::Result<u32> {
        let r = unsafe { super::WaitForSingleObject(self.handle, timeout_ms) };
        Ok(r)
    }

    pub fn wait_infinite(&self) -> io::Result<u32> {
        self.wait(INFINITE)
    }

    pub fn set(&self) -> io::Result<()> {
        unsafe {
            if super::SetEvent(self.handle) == 0 {
                return Err(last_error_io("SetEvent"));
            }
        }
        Ok(())
    }

    pub fn reset(&self) -> io::Result<()> {
        unsafe {
            if super::ResetEvent(self.handle) == 0 {
                return Err(last_error_io("ResetEvent"));
            }
        }
        Ok(())
    }

    /// PulseEvent — release one/all waiters then auto-reset.
    pub fn pulse(&self) -> io::Result<()> {
        unsafe {
            if super::PulseEvent(self.handle) == 0 {
                return Err(last_error_io("PulseEvent"));
            }
        }
        Ok(())
    }

    pub fn raw_handle(&self) -> Handle {
        self.handle
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = super::CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// Mutex (mutex_windows.hpp critical-section equivalent)
// ---------------------------------------------------------------------------

pub struct Mutex {
    handle: Handle,
}

impl Mutex {
    pub fn new() -> io::Result<Mutex> {
        Self::new_named(None)
    }

    pub fn new_named(name: Option<&str>) -> io::Result<Mutex> {
        let wide_name: Option<Vec<u16>> = name.map(|n| {
            std::ffi::OsStr::new(n)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        });
        let h = unsafe {
            super::CreateMutexW(
                std::ptr::null_mut(),
                0,
                wide_name.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            )
        };
        if h.is_null() {
            return Err(last_error_io("CreateMutexW"));
        }
        Ok(Mutex { handle: h })
    }

    /// Wait on the mutex (WAIT_ABANDONED counts as acquired).
    pub fn lock(&self, timeout_ms: u32) -> io::Result<u32> {
        let r = unsafe { super::WaitForSingleObject(self.handle, timeout_ms) };
        Ok(r)
    }

    pub fn lock_infinite(&self) -> io::Result<u32> {
        self.lock(INFINITE)
    }

    pub fn unlock(&self) -> io::Result<()> {
        unsafe {
            if super::ReleaseMutex(self.handle) == 0 {
                return Err(last_error_io("ReleaseMutex"));
            }
        }
        Ok(())
    }

    pub fn raw_handle(&self) -> Handle {
        self.handle
    }
}

impl Drop for Mutex {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = super::CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

// ---------------------------------------------------------------------------
// Semaphore (semaphore_windows.cpp)
// ---------------------------------------------------------------------------

pub struct Semaphore {
    handle: Handle,
    max_count: i32,
}

impl Semaphore {
    /// CreateSemaphoreW(initial_count, maximum_count, name).
    pub fn new(initial: i32, maximum: i32) -> io::Result<Semaphore> {
        Self::new_named(initial, maximum, None)
    }

    pub fn new_named(initial: i32, maximum: i32, name: Option<&str>) -> io::Result<Semaphore> {
        let wide_name: Option<Vec<u16>> = name.map(|n| {
            std::ffi::OsStr::new(n)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        });
        let h = unsafe {
            super::CreateSemaphoreW(
                std::ptr::null_mut(),
                initial,
                maximum,
                wide_name.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            )
        };
        if h.is_null() {
            return Err(last_error_io("CreateSemaphoreW"));
        }
        Ok(Semaphore {
            handle: h,
            max_count: maximum,
        })
    }

    pub fn wait(&self, timeout_ms: u32) -> io::Result<u32> {
        let r = unsafe { super::WaitForSingleObject(self.handle, timeout_ms) };
        Ok(r)
    }

    pub fn wait_infinite(&self) -> io::Result<u32> {
        self.wait(INFINITE)
    }

    /// signal(): ReleaseSemaphore by 1.
    pub fn signal(&self) -> io::Result<()> {
        self.release(1).map(|_| ())
    }

    /// ReleaseSemaphore(n).
    pub fn release(&self, count: i32) -> io::Result<i32> {
        let mut prev: i32 = 0;
        unsafe {
            if super::ReleaseSemaphore(self.handle, count, &mut prev) == 0 {
                return Err(last_error_io("ReleaseSemaphore"));
            }
        }
        Ok(prev)
    }

    pub fn max_count(&self) -> i32 {
        self.max_count
    }

    pub fn raw_handle(&self) -> Handle {
        self.handle
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = super::CloseHandle(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

// needed for encode_wide on OsStr in named constructors
use std::os::windows::ffi::OsStrExt;
