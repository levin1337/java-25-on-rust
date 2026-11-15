/*
 * Port of OpenJDK 25 HotSpot Java thread scheduling (share/runtime/thread.cpp)
 *
 * Each `java.lang.Thread` runs on its own Rust OS thread. Blocking
 * operations (`Object.wait`, `Thread.sleep`, `LockSupport.park`, monitor
 * contention) block the OS thread on real mutexes/condition variables, so
 * Java threads make genuine parallel progress. The primordial thread runs
 * on the process's main OS thread.
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use crate::share::runtime::jvmOut::JvmOut;
use crate::share::runtime::synchronizer::Synchronizer;
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, OnceLock};
use parking_lot::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Scheduling state of a registered Java thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    /// Registered, OS thread not yet scheduled.
    Ready,
    /// Currently executing on its OS thread.
    Running,
    /// Blocked in `Object.wait` or `LockSupport.park`.
    Waiting,
    /// `run()` returned or the thread died.
    Finished,
}

#[derive(Clone, Debug)]
pub struct JavaThreadInfo {
    /// Unique id used for monitor ownership.
    pub tid: u64,
    pub state: ThreadState,
    /// Monitor object handle the thread is waiting on.
    pub waiting_on: Option<usize>,
    /// Set by notify/notifyAll to wake the waiter.
    pub notified: bool,
    /// `LockSupport.unpark` permit: a park() call consumes it immediately.
    pub park_permit: bool,
}

impl JavaThreadInfo {
    pub fn new(tid: u64) -> Self {
        Self {
            tid,
            state: ThreadState::Ready,
            waiting_on: None,
            notified: false,
            park_permit: false,
        }
    }
}

/// Registry key used for the primordial (main) thread.
pub const MAIN_THREAD_KEY: usize = usize::MAX;

thread_local! {
    /// True when the current OS thread executes Java code.
    static OS_THREAD: Cell<bool> = const { Cell::new(false) };
    /// Scheduler key of the current Java thread (`None` = primordial).
    static JAVA_THREAD: Cell<Option<usize>> = const { Cell::new(None) };
    /// Monitor id of the current Java thread (0 = unset).
    static JAVA_TID: Cell<u64> = const { Cell::new(0) };
    /// `java.lang.Thread` object handle of the current thread.
    static THREAD_OBJECT: Cell<Option<usize>> = const { Cell::new(None) };
    /// Lowest usable stack address for the current OS thread; 0 = unknown.
    static STACK_LIMIT: Cell<usize> = const { Cell::new(0) };
}

/// Declares the OS stack size backing the current thread and derives the
/// overflow limit. Called once per Java-capable OS thread right after it
/// starts, while its stack pointer is still near the top.
pub fn install_stack_bounds(stack_bytes: usize) {
    let marker = 0u8;
    let sp = std::hint::black_box(&marker) as *const u8 as usize;
    let reserve = (stack_bytes / 8).clamp(8 * 1024 * 1024, 64 * 1024 * 1024);
    let usable = stack_bytes.saturating_sub(reserve);
    STACK_LIMIT.with(|cell| cell.set(sp.saturating_sub(usable)));
}

/// Whether the current thread declared its OS stack bounds.
pub fn stack_bounds_known() -> bool {
    STACK_LIMIT.with(|cell| cell.get() != 0)
}

/// True when the current thread's OS stack is nearly exhausted. Threads
/// that never declared bounds (e.g. JNI-attached native threads) report
/// false and rely on the interpreter depth fallback instead.
pub fn stack_near_overflow() -> bool {
    let limit = STACK_LIMIT.with(Cell::get);
    if limit == 0 {
        return false;
    }
    let marker = 0u8;
    let sp = std::hint::black_box(&marker) as *const u8 as usize;
    sp < limit
}

/// Process-wide `Arc` to the single VM `ClassLoader`, needed to move it
/// into spawned OS threads.
static JVM_LOADER: OnceLock<Arc<ClassLoader<'static>>> = OnceLock::new();

/// Parking state for `LockSupport.park`/`unpark`: permit per thread id.
static PARK_STATE: std::sync::LazyLock<Mutex<HashMap<u64, bool>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static PARK_COND: Condvar = Condvar::new();

// ---------------------------------------------------------------------------
// Cross-thread hang diagnostics (enabled by JVM25_DUMP)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct DebugInfo {
    /// `Class.method@pc` sites of the thread's interpreter frames.
    frames: Vec<String>,
    /// Native method currently executing (empty between natives).
    native: String,
    /// Java thread id for correlation with [jthread]/[block] traces.
    java_tid: u64,
}

static THREAD_DEBUG: std::sync::LazyLock<Mutex<HashMap<std::thread::ThreadId, DebugInfo>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static WATCHDOG: OnceLock<()> = OnceLock::new();

/// Records the native method label executing on the calling thread.
pub fn debug_native(label: String) {
    if std::env::var_os("JVM25_DUMP").is_none() {
        return;
    }
    let id = crate::share::prims::jniReferences::current_thread_id();
    THREAD_DEBUG.lock().entry(id).or_default().native = label;
}

/// Records the calling thread's interpreter frame sites.
pub fn debug_frames(frames: Vec<String>) {
    if std::env::var_os("JVM25_DUMP").is_none() {
        return;
    }
    let id = crate::share::prims::jniReferences::current_thread_id();
    THREAD_DEBUG.lock().entry(id).or_default().frames = frames;
}

/// Records the calling thread's Java thread id.
pub fn debug_java_tid(tid: u64) {
    if std::env::var_os("JVM25_DUMP").is_none() {
        return;
    }
    let id = crate::share::prims::jniReferences::current_thread_id();
    THREAD_DEBUG.lock().entry(id).or_default().java_tid = tid;
}

/// Spawns the watchdog thread once; it dumps every Java thread's last
/// published frame/native state every few seconds.
pub fn ensure_watchdog() {
    if std::env::var_os("JVM25_DUMP").is_none() || WATCHDOG.set(()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("jvm-watchdog".to_string())
        .stack_size(1024 * 1024)
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(5));
            let snapshot: Vec<(std::thread::ThreadId, DebugInfo)> = THREAD_DEBUG
                .lock()
                .iter()
                .map(|(id, info)| (*id, DebugInfo {
                    frames: info.frames.clone(),
                    native: info.native.clone(),
                    java_tid: info.java_tid,
                }))
                .collect();
            for (id, info) in snapshot {
                eprintln!(
                    "[watchdog] os={id:?} java_tid={} native={} frames={:?}",
                    info.java_tid,
                    if info.native.is_empty() { "-".to_string() } else { info.native },
                    info.frames
                );
            }
        });
}

/// Installs the shared loader; called once during VM startup.
pub fn install_loader(loader: Arc<ClassLoader<'static>>) {
    ensure_watchdog();
    let _ = JVM_LOADER.set(loader);
}

/// Returns the JVM-wide `ClassLoader` installed at bootstrap, if any.
/// Used by compiler threads and GC engines that run outside a Java call
/// frame but still need to resolve classes / walk object graphs.
pub fn jvm_loader() -> Option<Arc<ClassLoader<'static>>> {
    JVM_LOADER.get().cloned()
}

/// Marks the calling OS thread as a Java thread executing `run()`.
fn mark_java_thread(handle: Option<usize>, tid: u64, thread_object: Option<usize>) {
    OS_THREAD.with(|c| c.set(true));
    JAVA_THREAD.with(|c| c.set(handle));
    JAVA_TID.with(|c| c.set(tid));
    THREAD_OBJECT.with(|c| c.set(thread_object));
    debug_java_tid(tid);
}

/// Marks the process main thread as the primordial Java thread.
pub fn mark_primordial() {
    mark_java_thread(None, main_tid(), None);
}

/// Whether the current OS thread runs Java code.
pub fn on_os_thread() -> bool {
    OS_THREAD.with(Cell::get)
}

/// Id used for the primordial (main) thread in monitor ownership.
pub fn main_tid() -> u64 {
    Synchronizer::DEFAULT_THREAD_ID
}

/// Monitor id of the thread whose Java code is currently executing.
pub fn current_tid(_loader: &ClassLoader<'_>) -> u64 {
    let tid = JAVA_TID.with(Cell::get);
    if tid != 0 {
        tid
    } else {
        main_tid()
    }
}

/// Handle of the currently executing Java thread (`None` = main thread).
pub fn current_thread_handle(_loader: &ClassLoader<'_>) -> Option<usize> {
    JAVA_THREAD.with(Cell::get)
}

/// Registers a started `java.lang.Thread` object: assigns a monitor id and
/// spawns a dedicated OS thread that runs `Thread.run()`. Falls back to the
/// cooperative queue when no shared loader is installed (unit tests).
pub fn register(loader: &ClassLoader<'_>, handle: usize) -> io::Result<u64> {
    let tid = loader.next_java_tid.get();
    loader.next_java_tid.set(tid + 1);
    loader
        .java_threads
        .borrow_mut()
        .insert(handle, JavaThreadInfo::new(tid));
    loader
        .gc_extra_roots
        .borrow_mut()
        .push(Value::Object(handle));

    // JDK infrastructure daemon threads loop forever waiting for GC work
    // this VM performs synchronously; retire them instead of spawning.
    if is_infra_thread(loader, handle) {
        finish(loader, handle);
        return Ok(tid);
    }

    let Some(arc) = jvm_loader() else {
        loader.thread_ready.borrow_mut().push_back(handle);
        return Ok(tid);
    };
    spawn_os_thread(arc, handle, tid)?;
    Ok(tid)
}

/// Spawns the OS thread backing a Java `Thread` object.
fn spawn_os_thread(
    loader: Arc<ClassLoader<'static>>,
    handle: usize,
    tid: u64,
) -> io::Result<()> {
    let out = crate::share::runtime::jvmOut::shared();
    // Java threads run the same recursive interpreter as the main worker;
    // they need a comparable stack (JVM25_STACK_MB, default 1 GiB).
    let stack_mb: usize = std::env::var("JVM25_STACK_MB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(128);
    std::thread::Builder::new()
        .name(format!("java-lang-Thread-{tid}"))
        .stack_size(stack_mb * 1024 * 1024)
        .spawn(move || {
            eprintln!("[jthread] start tid={tid} handle={handle}");
            install_stack_bounds(stack_mb * 1024 * 1024);
            mark_java_thread(Some(handle), tid, Some(handle));
            {
                let mut threads = loader.java_threads.borrow_mut();
                if let Some(info) = threads.get_mut(&handle) {
                    info.state = ThreadState::Running;
                }
            }
            // A Rust panic inside Java execution must not skip `finish()`:
            // join() waiters would otherwise block forever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_os_thread(&loader, handle, out.as_ref())
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    eprintln!("[thread {tid}] java.lang.Thread.run died: {error}");
                }
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    eprintln!("[thread {tid}] panicked: {message}");
                }
            }
            finish(&loader, handle);
        })?;
    Ok(())
}

/// Executes `Thread.run()` once on the calling OS thread.
fn run_os_thread(
    loader: &ClassLoader<'static>,
    handle: usize,
    out: Option<&JvmOut>,
) -> io::Result<()> {
    let klass = {
        let objects = loader.borrow_objects();
        objects.get(handle).map(|object| object.klass.clone())
    };
    let Some(klass) = klass else {
        return Ok(());
    };
    let mut args = JavaCallArguments::new();
    args.push_oop(Value::Object(handle));
    match out {
        Some(out) => {
            let mut writer = out.clone();
            JavaCalls::call_virtual(loader, &klass, b"run", b"()V", &args, &mut writer)?;
        }
        None => {
            let mut sink = io::sink();
            JavaCalls::call_virtual(loader, &klass, b"run", b"()V", &args, &mut sink)?;
        }
    }
    Ok(())
}

/// Looks up thread info, returning `None` for unknown objects.
pub fn info(loader: &ClassLoader<'_>, handle: usize) -> Option<JavaThreadInfo> {
    loader.java_threads.borrow().get(&handle).cloned()
}

/// Marks a thread finished after `run()` returned (or died). Clears the
/// `eetop`/`threadStatus` fields so `isAlive()`/`getState()` observe it,
/// then broadcasts on the thread object's monitor so `join()` waiters wake.
pub fn finish(loader: &ClassLoader<'_>, handle: usize) {
    crate::share::memory::heap::thread_finished();
    {
        let mut threads = loader.java_threads.borrow_mut();
        if let Some(info) = threads.get_mut(&handle) {
            info.state = ThreadState::Finished;
            info.waiting_on = None;
            info.notified = false;
        }
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
            if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
                eprintln!("[finish] clear eetop handle={handle} slot={slot} fields={}", object.fields.len());
            }
            object.fields[slot] = Value::Long(0);
        } else if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
            eprintln!("[finish] eetop slot={slot} out of range handle={handle}");
        }
    } else if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
        eprintln!("[finish] no eetop field handle={handle}");
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
    eprintln!("[finish] status done handle={handle}");
    // JLS: a terminating thread releases every monitor it still owns —
    // essential when `run()` died via panic/exception mid-`synchronized`.
    let tid = loader.java_threads.borrow().get(&handle).map(|info| info.tid);
    if let Some(tid) = tid {
        loader
            .synchronizer
            .borrow_mut()
            .release_thread_monitors(tid);
    }
    // Thread termination broadcasts on the thread object's monitor so
    // `join()` waiters wake up (HotSpot `Thread::exit` equivalent). The
    // monitor key for `Value::Object(handle)` is `handle` itself.
    let monitor = { loader.synchronizer.borrow().find(handle) };
    if let Some(monitor) = monitor {
        monitor.force_notify_all();
    }
    eprintln!("[finish] exit handle={handle}");
}

/// JDK infrastructure daemon threads that must not run: they loop forever
/// waiting for GC/finalization work that this VM performs synchronously.
/// User and pool daemon threads (e.g. `ForkJoinWorkerThread`) DO run.
fn is_infra_thread(loader: &ClassLoader<'_>, handle: usize) -> bool {
    const INFRA: &[&[u8]] = &[
        b"jdk/internal/misc/Signal$SignalDispatcher",
        b"java/lang/ProcessImpl$ProcessReaper",
        b"java/lang/management/NotificationThread",
        b"jdk/internal/misc/InnocuousThread",
        b"jdk/internal/misc/InnocuousForkJoinWorkerThread",
        // Additional JDK infrastructure threads that loop forever:
        b"java/lang/ref/Reference$ReferenceHandler",
        b"java/lang/ref/Finalizer$FinalizerThread",
        b"java/lang/Thread$ThreadGroup",
    ];
    let objects = loader.borrow_objects();
    let Some(object) = objects.get(handle) else {
        return false;
    };
    INFRA.contains(&object.klass.class.name)
}

/// `Object.wait0(millis)`: releases `monitor`, blocks the calling OS
/// thread until notified or the deadline expires, then re-acquires the
/// monitor before returning. `timeout_millis <= 0` waits indefinitely.
pub fn wait_object(
    loader: &ClassLoader<'_>,
    monitor_key: usize,
    timeout_millis: i64,
    _output: &mut dyn Write,
    _depth: usize,
) -> io::Result<()> {
    let tid = current_tid(loader);
    let monitor = { loader.synchronizer.borrow_mut().monitor(monitor_key) };
    if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
        let site = {
            let mut top = String::new();
            crate::share::interpreter::bytecodeInterpreter::for_each_active_frame(&mut |f| {
                if top.is_empty() {
                    top = f.site().to_string();
                }
            });
            top
        };
        eprintln!("[block] wait_object tid={tid} key={monitor_key} timeout={timeout_millis} at {site}");
    }
    crate::share::memory::heap::quiesce();
    let result = monitor.wait_blocking(tid, timeout_millis);
    crate::share::memory::heap::unquiesce();
    result
        .map(|_| ())
        .map_err(|message| {
            io::Error::other(format!("java.lang.IllegalMonitorStateException: {message}"))
        })
}

/// `Object.notify`/`notifyAll`: wakes Java threads waiting on `monitor`.
pub fn notify_object(loader: &ClassLoader<'_>, monitor_key: usize, all: bool) -> io::Result<()> {
    let tid = current_tid(loader);
    let monitor = { loader.synchronizer.borrow().find(monitor_key) };
    let Some(monitor) = monitor else {
        return Err(io::Error::other(
            "java.lang.IllegalMonitorStateException: no such monitor",
        ));
    };
    let result = if all {
        monitor.notify_all(tid).map(|_| ())
    } else {
        monitor.notify(tid).map(|_| ())
    };
    result.map_err(|message| {
        io::Error::other(format!("java.lang.IllegalMonitorStateException: {message}"))
    })
}

/// `Thread.sleep0(nanos)`: sleeps the calling OS thread. The JDK passes
/// nanoseconds (see `Thread.sleep` -> `TimeUnit.toNanos`).
pub fn sleep_nanos(
    _loader: &ClassLoader<'_>,
    nanos: i64,
    _output: &mut dyn Write,
    _depth: usize,
) -> io::Result<()> {
    if nanos > 0 {
        if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
            eprintln!("[block] sleep_nanos tid={} nanos={nanos}", current_tid(_loader));
        }
        crate::share::memory::heap::quiesce();
        std::thread::sleep(Duration::from_nanos(nanos as u64));
        crate::share::memory::heap::unquiesce();
    }
    Ok(())
}

/// `Unsafe.park(isAbsolute, time)`: parks the calling OS thread until
/// `unpark`ed or the deadline expires. `time` is in nanoseconds for
/// relative waits and milliseconds-since-epoch for absolute waits.
pub fn park(
    loader: &ClassLoader<'_>,
    is_absolute: bool,
    time: i64,
    _output: &mut dyn Write,
    _depth: usize,
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
    crate::share::memory::heap::quiesce();
    let result = park_os(current_tid(loader), deadline);
    crate::share::memory::heap::unquiesce();
    result
}

/// OS-thread park: sleeps on the global park condition variable until
/// `unpark` sets this thread's permit or the deadline hits.
fn park_os(tid: u64, deadline: Option<Instant>) -> io::Result<()> {
    if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
        eprintln!("[block] park_os tid={tid} deadline={deadline:?}");
    }
    let mut permits = PARK_STATE.lock();
    loop {
        if permits.get(&tid).copied().unwrap_or(false) {
            permits.insert(tid, false);
            return Ok(());
        }
        match deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    return Ok(());
                }
                let _ = PARK_COND.wait_for(&mut permits, d - now);
            }
            None => {
                PARK_COND.wait(&mut permits);
            }
        }
    }
}

/// `Unsafe.unpark(thread)`: makes the permit available to `thread` and, if
/// it is parked, wakes it. A `null` thread is a no-op.
pub fn unpark(loader: &ClassLoader<'_>, value: Value<'_>) -> io::Result<()> {
    let Value::Object(handle) = value else {
        return Ok(());
    };
    // Resolve the scheduler id: registered threads key by object handle;
    // the primordial thread unparks under `main_tid()`.
    let tid = {
        loader
            .java_threads
            .borrow()
            .get(&handle)
            .map(|info| info.tid)
    }
    .or_else(|| {
        (*loader.current_thread_object_handle().borrow() == Some(handle)).then(main_tid)
    });
    if let Some(tid) = tid {
        {
            let mut permits = PARK_STATE.lock();
            permits.insert(tid, true);
        }
        PARK_COND.notify_all();
    }
    Ok(())
}

/// JVM shutdown semantics: after `main` returns the VM stays alive until
/// every non-daemon Java thread finishes (daemon threads never block
/// exit). Polls the registry; infra threads were retired at `register`
/// time so they never appear here as pending work.
pub fn wait_non_daemon(loader: &ClassLoader<'_>) {
    // The primordial thread parks here in Rust code after `main` returns;
    // without a GC registration its stale `Running` entry would block
    // every stop-the-world cycle forever.
    crate::share::memory::heap::quiesce();
    loop {
        let pending = {
            let threads = loader.java_threads.borrow();
            if threads.is_empty() {
                false
            } else {
                let objects = loader.borrow_objects();
                threads.iter().any(|(handle, info)| {
                    if info.state == ThreadState::Finished {
                        return false;
                    }
                    // `daemon` is a `Z` field on java.lang.Thread; a
                    // missing/unreadable field is treated as non-daemon
                    // (the JDK default for user threads).
                    objects
                        .get(*handle)
                        .and_then(|object| {
                            let slot = object
                                .klass
                                .instanceField(b"daemon", b"Z")
                                .ok()?;
                            object.fields.get(slot).copied()
                        })
                        .map(|value| !matches!(value, Value::Int(1)))
                        .unwrap_or(true)
                })
            }
        };
        if !pending {
            return;
        }
        if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
            let threads = loader.java_threads.borrow();
            let pending: Vec<_> = threads
                .iter()
                .filter(|(_, info)| info.state != ThreadState::Finished)
                .map(|(h, i)| {
                    let name = loader
                        .borrow_objects()
                        .get(*h)
                        .map(|o| String::from_utf8_lossy(&o.klass.class.name).into_owned())
                        .unwrap_or_default();
                    (*h, i.state, i.tid, name)
                })
                .collect();
            eprintln!("[wait-nondaemon] pending={pending:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `Thread.yield0`: yields the calling OS thread.
pub fn yield_thread(
    _loader: &ClassLoader<'_>,
    _output: &mut dyn Write,
    _depth: usize,
) -> io::Result<()> {
    std::thread::yield_now();
    Ok(())
}

/// Legacy cooperative scheduler entry points: kept as no-ops so existing
/// interpreter safepoint call sites compile; real scheduling happens on
/// dedicated OS threads.
pub fn run_next<'a>(
    _loader: &ClassLoader<'a>,
    _output: &mut dyn Write,
    _depth: usize,
) -> io::Result<bool> {
    Ok(false)
}

pub fn schedule<'a>(
    _loader: &ClassLoader<'a>,
    _output: &mut dyn Write,
    _depth: usize,
) -> io::Result<()> {
    Ok(())
}

/// Legacy marker: the cooperative suspend mechanism no longer exists.
pub fn is_suspended(_error: &io::Error) -> bool {
    false
}

/// Wakes every registered Java thread (shutdown/broadcast path).
#[allow(dead_code)]
pub fn wake_all(loader: &ClassLoader<'_>) {
    {
        let mut threads = loader.java_threads.borrow_mut();
        let mut tids = Vec::with_capacity(threads.len() + 1);
        tids.push(main_tid());
        for info in threads.values_mut() {
            tids.push(info.tid);
            if info.state == ThreadState::Waiting {
                info.notified = true;
                info.state = ThreadState::Ready;
            }
        }
        // park_os() sleeps on per-tid permits: set them all or the
        // notify_all below only causes a spurious wakeup and the
        // parked threads go back to sleep.
        let mut permits = PARK_STATE.lock();
        for tid in tids {
            permits.insert(tid, true);
        }
    }
    PARK_COND.notify_all();
}
