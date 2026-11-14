/*
 * Port of OpenJDK 25 HotSpot Java thread scheduling (share/runtime/thread.cpp)
 *
 * The interpreter is single OS-threaded, so Java threads are implemented as
 * cooperative green threads: `Thread.start0` enqueues the thread object and
 * the interpreter safepoint (plus blocking natives) drives a run queue by
 * recursively executing `Thread.run()` on the shared heap.
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use crate::share::runtime::synchronizer::Synchronizer;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// Scheduling state of a registered Java thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    /// Enqueued, waiting for a safepoint to run.
    Ready,
    /// Currently executing (only one at a time).
    Running,
    /// Blocked in `Object.wait`.
    Waiting,
    /// `run()` returned or the thread died.
    Finished,
}

#[derive(Clone, Debug)]
pub struct JavaThreadInfo {
    /// Unique id used for monitor ownership (object handle based).
    pub tid: u64,
    pub state: ThreadState,
    /// Monitor object handle the thread is waiting on.
    pub waiting_on: Option<usize>,
    /// Absolute deadline for timed waits (Instant-based bookkeeping).
    pub wait_deadline: Option<Instant>,
    /// Set by notify/notifyAll to wake the waiter.
    pub notified: bool,
    /// Monitor recursion count saved by `ObjectMonitor::wait`.
    pub wait_recursions: u32,
    /// `LockSupport.unpark` permit: a park() call consumes it immediately.
    pub park_permit: bool,
}

impl JavaThreadInfo {
    pub fn new(tid: u64) -> Self {
        Self {
            tid,
            state: ThreadState::Ready,
            waiting_on: None,
            wait_deadline: None,
            notified: false,
            wait_recursions: 0,
            park_permit: false,
        }
    }
}

/// Registry key used for the primordial (main) thread's wait/sleep state.
pub const MAIN_THREAD_KEY: usize = usize::MAX;

/// Maximum nested scheduler activations. Each level costs interpreter
/// frames on the Rust stack; beyond this queued threads run at outer
/// safepoints instead.
const MAX_SCHEDULE_DEPTH: u32 = 8;

/// Id used for the primordial (main) thread in monitor ownership.
pub fn main_tid() -> u64 {
    Synchronizer::DEFAULT_THREAD_ID
}

/// Monitor id of the thread whose Java code is currently executing.
pub fn current_tid(loader: &ClassLoader<'_>) -> u64 {
    match *loader.current_java_thread.borrow() {
        Some(handle) => loader
            .java_threads
            .borrow()
            .get(&handle)
            .map_or(main_tid(), |info| info.tid),
        None => main_tid(),
    }
}

/// Handle of the currently executing Java thread (`None` = main thread).
pub fn current_thread_handle(loader: &ClassLoader<'_>) -> Option<usize> {
    *loader.current_java_thread.borrow()
}

/// Registers a started `java.lang.Thread` object: assigns a monitor id,
/// enqueues it and keeps it alive as a GC root.
pub fn register(loader: &ClassLoader<'_>, handle: usize) -> io::Result<u64> {
    let tid = loader.next_java_tid.get();
    loader.next_java_tid.set(tid + 1);
    loader
        .java_threads
        .borrow_mut()
        .insert(handle, JavaThreadInfo::new(tid));
    loader.thread_ready.borrow_mut().push_back(handle);
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(handle));
    Ok(tid)
}

/// Looks up thread info, returning `None` for the main thread and unknown
/// objects.
pub fn info(loader: &ClassLoader<'_>, handle: usize) -> Option<JavaThreadInfo> {
    loader.java_threads.borrow().get(&handle).cloned()
}

/// Marks a thread finished after `run()` returned (or died). Clears the
/// `eetop`/`threadStatus` fields so `isAlive()`/`getState()` observe it.
pub fn finish(loader: &ClassLoader<'_>, handle: usize) {
    if let Some(info) = loader.java_threads.borrow_mut().get_mut(&handle) {
        info.state = ThreadState::Finished;
        info.waiting_on = None;
        info.notified = false;
    }
    let eetop = {
        let objects = loader.borrow_objects();
        objects.get(handle).and_then(|object| {
            object
                .klass
                .instanceField(b"eetop", b"J")
                .ok()
                .map(|slot| (slot, object.klass.clone()))
        })
    };
    if let Some((slot, _)) = eetop {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle)
            && slot < object.fields.len()
        {
            object.fields[slot] = Value::Long(0);
        }
    }
    let holder = {
        let objects = loader.borrow_objects();
        objects.get(handle).and_then(|object| {
            let slot = object
                .klass
                .instanceField(b"holder", b"Ljava/lang/Thread$FieldHolder;")
                .ok()?;
            object.fields.get(slot).copied()
        })
    };
    if let Some(Value::Object(holder_handle)) = holder {
        let status = {
            let objects = loader.borrow_objects();
            objects.get(holder_handle).and_then(|object| {
                object
                    .klass
                    .instanceField(b"threadStatus", b"I")
                    .ok()
                    .map(|slot| (slot, object.klass.clone()))
            })
        };
        if let Some((slot, _)) = status {
            let mut objects = loader.borrow_objects_mut();
            if let Some(object) = objects.get_mut(holder_handle)
                && slot < object.fields.len()
            {
                object.fields[slot] = Value::Int(5);
            }
        }
    }
}

/// Reads `Thread$FieldHolder.daemon` for a thread object handle.
/// JDK infrastructure daemon threads that must not run: they loop forever
/// waiting for GC/finalization work that this VM performs synchronously.
/// User and pool daemon threads (e.g. `ForkJoinWorkerThread`) DO run.
fn is_infra_thread(loader: &ClassLoader<'_>, handle: usize) -> bool {
    const INFRA: &[&[u8]] = &[
        b"java/lang/ref/Reference$ReferenceHandler",
        b"java/lang/ref/Finalizer$FinalizerThread",
        b"jdk/internal/ref/CleanerImpl$CleanerThread",
        b"jdk/internal/ref/CleanerFactory$CleanerThread",
        b"jdk/internal/misc/Signal$SignalDispatcher",
        b"java/lang/ProcessImpl$ProcessReaper",
        b"java/lang/management/NotificationThread",
        b"jdk/internal/misc/InnocuousThread",
        b"jdk/internal/misc/InnocuousForkJoinWorkerThread",
    ];
    let objects = loader.borrow_objects();
    let Some(object) = objects.get(handle) else {
        return false;
    };
    INFRA.contains(&object.klass.class.name)
}

fn is_daemon(loader: &ClassLoader<'_>, handle: usize) -> bool {
    let holder = {
        let objects = loader.borrow_objects();
        let Some(object) = objects.get(handle) else {
            return false;
        };
        let Ok(slot) = object
            .klass
            .instanceField(b"holder", b"Ljava/lang/Thread$FieldHolder;")
        else {
            return false;
        };
        object.fields.get(slot).copied()
    };
    let Some(Value::Object(holder_handle)) = holder else {
        return false;
    };
    let objects = loader.borrow_objects();
    let Some(object) = objects.get(holder_handle) else {
        return false;
    };
    let Ok(slot) = object.klass.instanceField(b"daemon", b"Z") else {
        return false;
    };
    matches!(object.fields.get(slot), Some(Value::Int(flag)) if *flag != 0)
}

/// Runs the next ready thread, if any. Returns `true` when a thread body
/// was executed. Called from interpreter safepoints and blocking natives.
pub fn run_next<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<bool> {
    loop {
        let Some(handle) = loader.thread_ready.borrow_mut().pop_front() else {
            return Ok(false);
        };
        if handle == MAIN_THREAD_KEY {
            continue;
        }
        let runnable = {
            let mut threads = loader.java_threads.borrow_mut();
            match threads.get_mut(&handle) {
                Some(info) if info.state == ThreadState::Ready => {
                    info.state = ThreadState::Running;
                    true
                }
                _ => false,
            }
        };
        if !runnable {
            continue;
        }
        // JDK infrastructure daemons (ReferenceHandler, Finalizer, cleaners,
        // signal dispatcher, process reaper) loop forever on work our
        // stop-the-world GC performs synchronously, so retire them. Other
        // daemon threads — notably ForkJoinPool workers — run normally.
        if is_infra_thread(loader, handle) {
            if std::env::var_os("JVM25_TRACE").is_some() {
                eprintln!("[schedule] retire infra thread handle={handle}");
            }
            finish(loader, handle);
            continue;
        }
        run_thread(loader, handle, output, depth)?;
        return Ok(true);
    }
}

/// Safepoint entry: runs at most one queued thread per activation to keep
/// scheduler recursion bounded.
pub fn schedule<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<()> {
    if loader.schedule_depth.get() >= MAX_SCHEDULE_DEPTH {
        return Ok(());
    }
    if loader.thread_ready.borrow().is_empty() {
        return Ok(());
    }
    run_next(loader, output, depth)?;
    Ok(())
}

/// Executes `Thread.run()` for `handle` with proper current-thread
/// bookkeeping, then marks the thread finished.
fn run_thread<'a>(
    loader: &ClassLoader<'a>,
    handle: usize,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<()> {
    let klass = {
        let objects = loader.borrow_objects();
        objects.get(handle).map(|object| object.klass.clone())
    };
    let Some(klass) = klass else {
        finish(loader, handle);
        return Ok(());
    };

    if std::env::var_os("JVM25_TRACE").is_some() {
        eprintln!(
            "[sched] run_thread handle={handle} klass={}",
            String::from_utf8_lossy(&klass.class.name)
        );
    }
    let previous = *loader.current_java_thread.borrow();
    *loader.current_java_thread.borrow_mut() = Some(handle);
    loader.schedule_depth.set(loader.schedule_depth.get() + 1);

    let mut args = JavaCallArguments::new();
    args.push_oop(Value::Object(handle));
    let mut out = output;
    let result = JavaCalls::call_virtual(
        loader,
        &klass,
        b"run",
        b"()V",
        &args,
        &mut out,
    );

    loader.schedule_depth.set(loader.schedule_depth.get().saturating_sub(1));
    *loader.current_java_thread.borrow_mut() = previous;

    match result {
        Ok(_) => {
            loader.suspend_streak.set(0);
            finish(loader, handle);
        }
        Err(error) if is_suspended(&error) => {
            // Worker-loop thread parked with nothing runnable: abandon this
            // activation (no Java finally) and re-queue `run()` so the
            // worker re-scans its queue on the next activation.
            loader.suspend_streak.set(loader.suspend_streak.get() + 1);
            {
                let mut threads = loader.java_threads.borrow_mut();
                if let Some(info) = threads.get_mut(&handle) {
                    info.state = ThreadState::Ready;
                    info.waiting_on = None;
                    info.notified = false;
                    info.wait_deadline = None;
                }
            }
            loader.thread_ready.borrow_mut().push_back(handle);
        }
        Err(error) => {
            loader.suspend_streak.set(0);
            finish(loader, handle);
            if std::env::var_os("JVM25_TRACE").is_some() {
                eprintln!("[thread] java.lang.Thread.run died: {error}");
            }
        }
    }
    Ok(())
}

/// Drives the scheduler until `current` is notified, its deadline expires,
/// or no runnable threads remain. Used by `Object.wait` and `Thread.sleep`.
/// Outcome of `drive_until`: woken by notify/unpark, deadline expiry, or
/// scheduler starvation (nothing runnable).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DriveResult {
    Woke,
    Timeout,
    Empty,
}

fn drive_until<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
    current: usize,
    deadline: Option<Instant>,
) -> io::Result<DriveResult> {
    loop {
        let status = {
            let threads = loader.java_threads.borrow();
            match threads.get(&current) {
                Some(info) => {
                    if info.notified {
                        Some(DriveResult::Woke)
                    } else if deadline.is_some_and(|d| Instant::now() >= d) {
                        Some(DriveResult::Timeout)
                    } else {
                        None
                    }
                }
                None => Some(DriveResult::Woke),
            }
        };
        if let Some(result) = status {
            // `notify`/`unpark` also push the woken thread onto the ready
            // queue, but this thread resumes directly through the flag —
            // drop the stale entry so `run_next` cannot re-enter `run()`.
            loader
                .thread_ready
                .borrow_mut()
                .retain(|entry| *entry != current);
            return Ok(result);
        }
        // Bound scheduler recursion: each nested run_thread adds a full
        // interpreter activation to the Rust stack.
        if loader.schedule_depth.get() >= MAX_SCHEDULE_DEPTH
            || loader.suspend_streak.get() >= 8
        {
            return Ok(DriveResult::Empty);
        }
        if !run_next(loader, output, depth)? {
            // No runnable threads: nothing can notify us until the outer
            // driver resumes.
            return Ok(DriveResult::Empty);
        }
    }
}

/// Throws `java.lang.ThreadDeath` to suspend a nested Java thread whose
/// blocking call found no runnable threads. `ThreadGroup.uncaughtException`
/// ignores `ThreadDeath`, so the thread exits quietly after running its
/// `finally` blocks; executors replace it on demand.
fn suspend<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Error {
    if is_resumable_worker(loader) {
        return io::Error::other(ThreadSuspended);
    }
    match crate::share::interpreter::bytecodeInterpreter::throwableObject(
        loader,
        output,
        depth,
        b"java/lang/ThreadDeath",
        Some("thread suspended: no runnable threads"),
    ) {
        Ok(handle) => io::Error::other(
            crate::share::interpreter::bytecodeInterpreter::JavaThrowable(handle),
        ),
        Err(_) => io::Error::other("java.lang.ThreadDeath"),
    }
}

/// Returns `true` when `current` is a nested Java thread (not the primordial
/// driver), i.e. it was started via `Thread.start` and is running inside
/// another thread's scheduler activation.
fn is_nested(loader: &ClassLoader<'_>) -> bool {
    loader.current_java_thread.borrow().is_some()
}

/// Marker error for suspend-and-reschedule: the thread's `run()` activation
/// is abandoned mid-block WITHOUT running Java `finally` blocks, and the
/// thread is re-queued so `run()` starts over. Only safe for worker-loop
/// threads whose `run()` is a re-entrant work loop.
#[derive(Debug)]
pub struct ThreadSuspended;

impl std::fmt::Display for ThreadSuspended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("thread suspended")
    }
}

impl std::error::Error for ThreadSuspended {}

pub fn is_suspended(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ThreadSuspended>())
        .is_some()
}

/// Classes whose `run()` is a re-entrant worker loop and can be safely
/// restarted after a mid-block suspend.
fn is_resumable_worker(loader: &ClassLoader<'_>) -> bool {
    let Some(handle) = current_thread_handle(loader) else {
        return false;
    };
    let objects = loader.borrow_objects();
    let Some(object) = objects.get(handle) else {
        return false;
    };
    let name = object.klass.class.name;
    name == b"java/util/concurrent/ThreadPoolExecutor$Worker"
        || name == b"java/util/concurrent/ForkJoinWorkerThread"
        || name.starts_with(b"java/util/concurrent/ForkJoinPool$")
        || (name.starts_with(b"java/util/concurrent/")
            && [b"Worker".as_slice(), b"Thread".as_slice()]
                .iter()
                .any(|needle| {
                    name.windows(needle.len()).any(|window| window == *needle)
                }))
}

/// `Object.wait0(millis)`: releases `monitor`, blocks the current Java
/// thread cooperatively, then re-acquires the monitor before returning.
pub fn wait_object<'a>(
    loader: &ClassLoader<'a>,
    monitor_key: usize,
    timeout_millis: i64,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<()> {
    let tid = current_tid(loader);
    let current = current_thread_handle(loader);
    let recursions = {
        loader
            .synchronizer
            .borrow_mut()
            .wait_thread(monitor_key, tid)
    }
    .map_err(|message| {
        io::Error::other(format!("java.lang.IllegalMonitorStateException: {message}"))
    })?;

    let drive_key = current.unwrap_or(MAIN_THREAD_KEY);
    {
        let mut threads = loader.java_threads.borrow_mut();
        let info = threads
            .entry(drive_key)
            .or_insert_with(|| JavaThreadInfo::new(tid));
        info.state = ThreadState::Waiting;
        info.waiting_on = Some(monitor_key);
        info.notified = false;
        info.wait_recursions = recursions;
        info.wait_deadline = if timeout_millis > 0 {
            Some(Instant::now() + Duration::from_millis(timeout_millis as u64))
        } else {
            None
        };
    }

    let deadline = if timeout_millis > 0 {
        Some(Instant::now() + Duration::from_millis(timeout_millis as u64))
    } else {
        None
    };
    // The monitor was already released by `wait_thread`; drive the
    // scheduler until this thread is notified or its deadline expires.
    // When the ready queue runs dry `drive_until` reports `false`; per the
    // JMM `wait` may wake spuriously, so we simply continue — the monitor
    // is re-acquired below and `wait` returns normally.
    let _ = drive_until(loader, output, depth, drive_key, deadline)?;

    // Remove ourselves from the monitor wait set (covers timeouts where
    // no notify consumed the entry) and clear waiting state.
    {
        loader
            .synchronizer
            .borrow_mut()
            .remove_waiter(monitor_key, tid);
    }
    {
        let mut threads = loader.java_threads.borrow_mut();
        if let Some(info) = threads.get_mut(&drive_key) {
            info.state = ThreadState::Running;
            info.waiting_on = None;
            info.notified = false;
            info.wait_deadline = None;
        }
    }

    // Re-acquire the monitor, restoring saved recursion depth. Other
    // threads may hold it; keep driving the scheduler until it frees.
    loop {
        if loader
            .synchronizer
            .borrow_mut()
            .reenter_thread(monitor_key, tid, recursions)
        {
            return Ok(());
        }
        if !run_next(loader, output, depth)? {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// `Object.notify`/`notifyAll`: wakes Java threads waiting on `monitor`.
pub fn notify_object(loader: &ClassLoader<'_>, monitor_key: usize, all: bool) -> io::Result<()> {
    let tid = current_tid(loader);
    let mut synchronizer = loader.synchronizer.borrow_mut();
    if all {
        synchronizer.notify_all_thread(monitor_key, tid).map(|_| ())
    } else {
        synchronizer.notify_thread(monitor_key, tid).map(|_| ())
    }
    .map_err(|message| {
        io::Error::other(format!("java.lang.IllegalMonitorStateException: {message}"))
    })?;
    drop(synchronizer);

    // Mark matching waiters notified and re-queue them. `notify` wakes a
    // single waiter; HashMap order approximates FIFO, which is sufficient
    // for cooperative scheduling.
    let mut threads = loader.java_threads.borrow_mut();
    let mut ready = loader.thread_ready.borrow_mut();
    for (handle, info) in threads.iter_mut() {
        if info.state == ThreadState::Waiting
            && info.waiting_on == Some(monitor_key)
            && !info.notified
        {
            info.notified = true;
            info.state = ThreadState::Ready;
            // The primordial driver is not a runnable object; its `notified`
            // flag is consumed by `drive_until` directly.
            if *handle != MAIN_THREAD_KEY {
                ready.push_back(*handle);
            }
            if !all {
                break;
            }
        }
    }
    Ok(())
}

/// `Thread.sleep0(nanos)`: parks the current thread cooperatively,
/// letting queued threads run until the deadline. The JDK passes
/// nanoseconds (see `Thread.sleep` -> `TimeUnit.toNanos`).
pub fn sleep_nanos<'a>(
    loader: &ClassLoader<'a>,
    nanos: i64,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<()> {
    if nanos <= 0 {
        return Ok(());
    }
    if std::env::var_os("JVM25_TRACE").is_some() {
        eprintln!("[sleep] {nanos}ns");
    }
    let deadline = Instant::now() + Duration::from_nanos(nanos as u64);
    // Sleep has no monitor and no waiter entry; drive the scheduler until
    // the deadline, idling when nothing is runnable.
    loop {
        if Instant::now() >= deadline {
            return Ok(());
        }
        if !run_next(loader, output, depth)? {
            let remaining = deadline.saturating_duration_since(Instant::now());
            std::thread::sleep(remaining.min(Duration::from_millis(1)));
        }
    }
}

/// Synthetic `waiting_on` key used by `LockSupport.park` (no monitor).
pub const PARK_KEY: usize = usize::MAX - 1;

/// `Unsafe.park(isAbsolute, time)`: cooperatively parks the current Java
/// thread until `unpark`ed, the deadline expires, or the scheduler runs out
/// of runnable threads (spurious wakeup, allowed by the JMM). `time` is in
/// nanoseconds for relative waits and milliseconds-since-epoch for absolute
/// waits, matching `Unsafe.park`.
pub fn park<'a>(
    loader: &ClassLoader<'a>,
    is_absolute: bool,
    time: i64,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<()> {
    let deadline = if time <= 0 {
        None
    } else if is_absolute {
        let target = std::time::UNIX_EPOCH + Duration::from_millis(time as u64);
        match target.duration_since(std::time::SystemTime::now()) {
            Ok(remaining) => Some(Instant::now() + remaining),
            Err(_) => return Ok(()),
        }
    } else {
        Some(Instant::now() + Duration::from_nanos(time as u64))
    };

    let current = current_thread_handle(loader);
    let drive_key = current.unwrap_or(MAIN_THREAD_KEY);
    {
        let tid = current_tid(loader);
        let mut threads = loader.java_threads.borrow_mut();
        let info = threads
            .entry(drive_key)
            .or_insert_with(|| JavaThreadInfo::new(tid));
        if info.park_permit {
            info.park_permit = false;
            return Ok(());
        }
        info.state = ThreadState::Waiting;
        info.waiting_on = Some(PARK_KEY);
        info.notified = false;
        info.wait_deadline = deadline;
    }

    let woke = drive_until(loader, output, depth, drive_key, deadline)?;

    {
        let mut threads = loader.java_threads.borrow_mut();
        if let Some(info) = threads.get_mut(&drive_key) {
            info.state = ThreadState::Running;
            info.waiting_on = None;
            info.notified = false;
            info.wait_deadline = None;
        }
    }
    if !woke && is_nested(loader) {
        return Err(suspend(loader, output, depth));
    }
    Ok(())
}

/// `Unsafe.unpark(thread)`: makes the permit available to `thread` and, if
/// it is parked, wakes it. A `null` thread is a no-op.
pub fn unpark(loader: &ClassLoader<'_>, value: Value<'_>) -> io::Result<()> {
    let Value::Object(handle) = value else {
        return Ok(());
    };
    // Resolve the scheduler key: registered threads key by object handle;
    // the primordial main thread keys by MAIN_THREAD_KEY.
    let key = {
        let threads = loader.java_threads.borrow();
        if threads.contains_key(&handle) {
            handle
        } else if *loader.current_thread_object_handle().borrow() == Some(handle) {
            // The cached current-thread object belongs to the primordial
            // thread, which parks under MAIN_THREAD_KEY.
            MAIN_THREAD_KEY
        } else {
            handle
        }
    };
    let mut requeue = false;
    {
        let tid = loader.next_java_tid.get();
        loader.next_java_tid.set(tid + 1);
        let mut threads = loader.java_threads.borrow_mut();
        let info = threads
            .entry(key)
            .or_insert_with(|| JavaThreadInfo::new(tid));
        info.park_permit = true;
        if info.state == ThreadState::Waiting
            && info.waiting_on == Some(PARK_KEY)
            && !info.notified
        {
            info.notified = true;
            info.state = ThreadState::Ready;
            requeue = true;
        }
    }
    if requeue && key != MAIN_THREAD_KEY {
        loader.thread_ready.borrow_mut().push_back(key);
    }
    Ok(())
}

/// `Thread.yield0`: runs one queued thread if any.
pub fn yield_thread<'a>(
    loader: &ClassLoader<'a>,
    output: &mut dyn Write,
    depth: usize,
) -> io::Result<()> {
    run_next(loader, output, depth)?;
    Ok(())
}

/// Wakes every Java thread currently waiting (used by `notifyAll`-style
/// broadcasts and shutdown paths).
#[allow(dead_code)]
pub fn wake_all(loader: &ClassLoader<'_>) {
    let mut threads = loader.java_threads.borrow_mut();
    let mut ready = loader.thread_ready.borrow_mut();
    for (handle, info) in threads.iter_mut() {
        if info.state == ThreadState::Waiting {
            info.notified = true;
            info.state = ThreadState::Ready;
            ready.push_back(*handle);
        }
    }
}
