//! Native methods for `sun.management.VMManagementImpl` (java.management).
//!
//! HotSpot answers these through `JVM_GetManagementInfo`-style natives in
//! libjvm. Here they read the real process/runtime state: OS thread/process
//! APIs for ids and processor count, `Os` startup bookkeeping for uptime,
//! and the loader's class/thread registries for VM statistics.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::objArrayOop::ObjArrayOop;
use crate::share::runtime::{os::Os, threading::ThreadState};

const OWNER: &[u8] = b"sun/management/VMManagementImpl";
/// `sun.management.VMManagement` is the abstract API class; call sites
/// reference it as the constant-pool owner while the natives live on the
/// `VMManagementImpl` implementation.
const API_OWNER: &[u8] = b"sun/management/VMManagement";

static PEAK_THREADS: AtomicUsize = AtomicUsize::new(0);

fn live_thread_stats(loader: &ClassLoader<'_>) -> (usize, usize) {
    let threads = loader.java_threads.borrow();
    let mut live = 0usize;
    let mut daemon = 0usize;
    for info in threads.values() {
        if info.state != ThreadState::Finished {
            live += 1;
        }
    }
    drop(threads);
    // The primordial thread is not in the registry; count it as live.
    (live + 1, daemon)
}

fn string_array<'a>(
    loader: &ClassLoader<'a>,
    values: Vec<String>,
) -> io::Result<Value<'a>> {
    let mut array =
        ObjArrayOop::withComponent(values.len() as i32, b"java/lang/String")?;
    for (slot, value) in array.elements.iter_mut().zip(values) {
        *slot = Value::HostString(loader.storage.alloc_str(&value));
    }
    Ok(Value::ObjectArray(loader.alloc_obj_array(array)))
}

/// Instance natives of `VMManagementImpl`. `arguments[0]` is the receiver.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    _arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    let result = match (name, descriptor) {
        (b"isThreadContentionMonitoringEnabled", b"()Z") => Value::Int(0),
        (b"isThreadCpuTimeEnabled", b"()Z") => Value::Int(0),
        (b"isThreadAllocatedMemoryEnabled", b"()Z") => Value::Int(0),
        (b"getTotalClassCount", b"()J") => {
            Value::Long(loader.classes_loaded_count() as i64)
        }
        (b"getUnloadedClassCount", b"()J") => Value::Long(0),
        (b"getVerboseClass", b"()Z") => Value::Int(0),
        (b"getVerboseGC", b"()Z") => Value::Int(0),
        (b"getProcessId", b"()I") => Value::Int(std::process::id() as i32),
        (b"getVmArguments0", b"()[Ljava/lang/String;") => {
            let args: Vec<String> = loader
                .systemProperties
                .borrow()
                .iter()
                .map(|(key, value)| format!("-D{key}={value}"))
                .collect();
            return string_array(loader, args).map(Some).map(Some);
        }
        (b"getStartupTime", b"()J") => Value::Long(Os::vm_startup_time_ms()),
        (b"getUptime0", b"()J") => Value::Long(Os::vm_uptime_ms()),
        (b"getAvailableProcessors", b"()I") => {
            Value::Int(Os::processor_count() as i32)
        }
        (b"getTotalCompileTime", b"()J") => Value::Long(0),
        (b"getTotalThreadCount", b"()J") => {
            let (live, _) = live_thread_stats(loader);
            Value::Long(live as i64)
        }
        (b"getLiveThreadCount", b"()I") => {
            let (live, _) = live_thread_stats(loader);
            Value::Int(live as i32)
        }
        (b"getPeakThreadCount", b"()I") => {
            let (live, _) = live_thread_stats(loader);
            let peak = PEAK_THREADS.fetch_max(live, Ordering::Relaxed);
            Value::Int(peak.max(live) as i32)
        }
        (b"getDaemonThreadCount", b"()I") => {
            let (_, daemon) = live_thread_stats(loader);
            Value::Int(daemon as i32)
        }
        (b"getSafepointCount", b"()J") => Value::Long(0),
        (b"getTotalSafepointTime", b"()J") => Value::Long(0),
        (b"getSafepointSyncTime", b"()J") => Value::Long(0),
        (b"getTotalApplicationNonStoppedTime", b"()J") => Value::Long(0),
        (b"getLoadedClassSize", b"()J") => {
            Value::Long(loader.classes_loaded_count() as i64 * 1024)
        }
        (b"getUnloadedClassSize", b"()J") => Value::Long(0),
        (b"getClassLoadingTime", b"()J") => Value::Long(0),
        (b"getMethodDataSize", b"()J") => Value::Long(0),
        (b"getInitializedClassCount", b"()J") => {
            Value::Long(loader.classes_loaded_count() as i64)
        }
        (b"getClassInitializationTime", b"()J") => Value::Long(0),
        (b"getClassVerificationTime", b"()J") => Value::Long(0),
        _ => return Ok(None),
    };
    Ok(Some(Some(result)))
}

/// Static natives of `VMManagementImpl`.
pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    let _ = (loader, arguments);
    let result = match (name, descriptor) {
        (b"getVersion0", b"()Ljava/lang/String;") => {
            let version = loader
                .systemProperties
                .borrow()
                .get("java.vm.version")
                .copied()
                .unwrap_or("25+36-3489");
            Value::HostString(version)
        }
        (b"initOptionalSupportFields", b"()V") => return Ok(Some(None)),
        _ => return Ok(None),
    };
    Ok(Some(Some(result)))
}

/// Predicate for the dispatch tables in `nativeLookup`.
pub(crate) fn is_owner(owner: &[u8]) -> bool {
    owner == OWNER || owner == b"sun/management/VMManagement"
}
