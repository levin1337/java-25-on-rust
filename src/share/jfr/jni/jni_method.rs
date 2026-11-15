

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    jfr::{
        instrumentation::{class_transformer, event_class_transformer},
        jni::java_support,
        leakprofiler,
    },
    runtime::{os::Os, threading},
};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

fn now_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn counter_time() -> i64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(std::time::Instant::now).elapsed().as_nanos() as i64
}

fn int_value(flag: bool) -> Value<'static> {
    Value::Int(i32::from(flag))
}

fn long_value(v: i64) -> Value<'static> {
    Value::Long(v)
}

fn string_value<'a>(loader: &ClassLoader<'a>, text: &str) -> Value<'a> {
    Value::HostString(loader.storage.alloc_str(text))
}

fn arg_string<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>], index: usize) -> Option<String> {
    java_support::c_str(loader, *arguments.get(index)?)
}

fn arg_bool(arguments: &[Value<'_>], index: usize) -> bool {
    matches!(arguments.get(index), Some(Value::Int(v)) if *v != 0)
}

fn arg_long(arguments: &[Value<'_>], index: usize) -> i64 {
    match arguments.get(index) {
        Some(Value::Long(v)) => *v,
        Some(Value::Int(v)) => i64::from(*v),
        _ => 0,
    }
}

fn arg_int(arguments: &[Value<'_>], index: usize) -> i32 {
    match arguments.get(index) {
        Some(Value::Int(v)) => *v,
        Some(Value::Long(v)) => *v as i32,
        _ => 0,
    }
}

#[derive(Default)]
struct JfrJniState {
    recording: bool,
    jfr_created: bool,
    repository: Option<PathBuf>,
    dump_path: Option<String>,
    output: Option<String>,
    stack_depth: i32,
    stack_trace_enabled: HashMap<u64, bool>,
    enabled: HashMap<u64, bool>,
    thresholds: HashMap<u64, (i64, i64)>,
    throttles: HashMap<u64, (i64, i64, i64)>,
    buffer_sizes: HashMap<&'static str, i64>,
    cpu_rate: f64,
    cpu_period: i64,
    miscellaneous: HashMap<&'static str, i64>,
    chunk_start_nanos: i64,
    last_chunk_final: i64,
    string_constants: HashMap<i64, String>,
    metadata_descriptor: Option<Vec<u8>>,
    method_sampling_periods: Vec<(i64, i64)>,
    file_notification_ticks: i64,
    global_buffer_count: i64,
    global_buffer_size: i64,
    thread_buffer_size: i64,
    memory_size: i64,
    excluded_classes: HashSet<Vec<u8>>,
    stack_filters: HashMap<i64, (Vec<String>, Vec<String>)>,
    next_stack_filter_id: i64,
    pending_events: Vec<(i64, i64, i64)>,
    type_ids: HashMap<String, i64>,
    next_type_id: i64,
    stack_trace_ids: HashMap<(i32, i64), i64>,
    next_stack_trace_id: i64,
}

static STATE: std::sync::LazyLock<Mutex<JfrJniState>> =
    std::sync::LazyLock::new(|| {
        Mutex::new(JfrJniState {
            stack_depth: 64,
            chunk_start_nanos: now_nanos(),
            next_stack_filter_id: 1,
            next_stack_trace_id: 1,
            next_type_id: 1,
            ..JfrJniState::default()
        })
    });

fn state() -> std::sync::MutexGuard<'static, JfrJniState> {
    STATE.lock().unwrap()
}

fn default_repository() -> PathBuf {
    std::env::temp_dir().join("jfr")
}

fn repository_dir(state: &JfrJniState) -> PathBuf {
    state
        .repository
        .clone()
        .unwrap_or_else(default_repository)
}

pub fn dispatch<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
    output: &mut dyn Write,
) -> io::Result<Option<Value<'a>>> {
    let _ = descriptor;
    let result = match name {
        b"beginRecording" => {
            state().recording = true;
            state().chunk_start_nanos = now_nanos();
            None
        }
        b"isRecording" => Some(int_value(state().recording)),
        b"endRecording" => {
            let mut st = state();
            st.recording = false;
            st.last_chunk_final = now_nanos();
            st.chunk_start_nanos = now_nanos();
            rotate_chunk(&mut st, output)?;
            None
        }
        b"markChunkFinal" => {
            rotate_chunk(&mut state(), output)?;
            None
        }
        b"counterTime" => Some(long_value(counter_time())),
        b"nanosNow" => Some(long_value(now_nanos())),
        b"createJFR" => {
            let mut st = state();
            st.jfr_created = true;
            st.recording = arg_bool(arguments, 0);
            if st.recording {
                st.chunk_start_nanos = now_nanos();
            }
            Some(int_value(true))
        }
        b"destroyJFR" => {
            let mut st = state();
            st.jfr_created = false;
            st.recording = false;
            Some(int_value(true))
        }
        b"emitEvent" => {
            let mut st = state();
            st.pending_events
                .push((arg_long(arguments, 0), arg_long(arguments, 1), arg_long(arguments, 2)));
            Some(int_value(st.recording))
        }
        b"getAllEventClasses" => {
            let klasses = event_class_transformer::loaded_event_classes(loader);
            let mirrors: Vec<Value<'a>> = klasses
                .iter()
                .map(|k| Value::ClassMirror(k.class.name))
                .collect();
            let array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                mirrors.len() as i32,
                b"java/lang/Class",
            )?;
            let mut obj_array = array;
            for (index, mirror) in mirrors.into_iter().enumerate() {
                obj_array.obj_at_put(index, mirror)?;
            }
            let _ = klasses;
            Some(Value::ObjectArray(loader.alloc_obj_array(obj_array)))
        }
        b"getClassId" => Some(long_value(type_id_for(loader, arguments, 0))),
        b"getTypeId" => Some(long_value(type_id_for(loader, arguments, 0))),
        b"getPid" => Some(string_value(loader, &std::process::id().to_string())),
        b"getStackTraceId" => {
            let mut st = state();
            let key = (arg_int(arguments, 0), arg_long(arguments, 1));
            let id = match st.stack_trace_ids.get(&key) {
                Some(id) => *id,
                None => {
                    let id = st.next_stack_trace_id;
                    st.next_stack_trace_id += 1;
                    st.stack_trace_ids.insert(key, id);
                    id
                }
            };
            Some(long_value(id))
        }
        b"getThreadId" => {
            let thread = arguments.first().copied().unwrap_or(Value::Null);
            Some(long_value(java_support::jfr_thread_id(loader, thread)?))
        }
        b"getTicksFrequency" => Some(long_value(1_000_000_000)),
        b"getTimeConversionFactor" => Some(Value::Double(1.0)),
        b"subscribeLogLevel" | b"log" | b"logEvent" => {
            log_native(name, arguments, loader);
            None
        }
        b"retransformClasses" => {
            let classes = match arguments.first() {
                Some(Value::ObjectArray(handle)) => {
                    let arrays = loader.obj_arrays.borrow();
                    let array = arrays.get(*handle).ok_or_else(|| {
                        io::Error::other("retransformClasses: invalid array")
                    })?;
                    array.elements.clone()
                }
                _ => Vec::new(),
            };
            let env = crate::share::jfr::instrumentation::jvmti_agent::JvmtiAgent::instance()
                .and_then(|agent| agent.env());
            class_transformer::retransform_classes(loader, &classes, env.as_deref(), output)?;
            None
        }
        b"setEnabled" => {
            let mut st = state();
            st.enabled.insert(arg_long(arguments, 0) as u64, arg_bool(arguments, 1));
            None
        }
        b"setFileNotification" => {
            state().file_notification_ticks = arg_long(arguments, 0);
            None
        }
        b"setGlobalBufferCount" => {
            state().global_buffer_count = arg_long(arguments, 0);
            None
        }
        b"setGlobalBufferSize" => {
            state().global_buffer_size = arg_long(arguments, 0);
            None
        }
        b"setMethodSamplingPeriod" => {
            state()
                .method_sampling_periods
                .push((arg_long(arguments, 0), arg_long(arguments, 1)));
            None
        }
        b"setOutput" => {
            state().output = arg_string(loader, arguments, 0);
            None
        }
        b"setStackDepth" => {
            state().stack_depth = arg_int(arguments, 0);
            None
        }
        b"setStackTraceEnabled" => {
            state()
                .stack_trace_enabled
                .insert(arg_long(arguments, 0) as u64, arg_bool(arguments, 1));
            None
        }
        b"setThreadBufferSize" => {
            state().thread_buffer_size = arg_long(arguments, 0);
            None
        }
        b"setMemorySize" => {
            state().memory_size = arg_long(arguments, 0);
            None
        }
        b"setThreshold" => {
            let mut st = state();
            let request = (arg_long(arguments, 0), arg_long(arguments, 1));
            st.thresholds.insert(request.0 as u64, request);
            Some(int_value(true))
        }
        b"storeMetadataDescriptor" => {
            if let Some(Value::PrimitiveArray(handle)) = arguments.first() {
                let arrays = loader.arrays.borrow();
                if let Some(crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes)) =
                    arrays.get(*handle)
                {
                    state().metadata_descriptor =
                        Some(bytes.iter().map(|&b| b as u8).collect());
                }
            }
            None
        }
        b"commit" => Some(long_value(counter_time())),
        b"getAllowedToDoEventRetransforms" => Some(int_value(
            crate::share::jfr::instrumentation::jvmti_agent::JvmtiAgent::instance()
                .is_none_or(|agent| agent.retransform_allowed()),
        )),
        b"isAvailable" => Some(int_value(java_support::is_jdk_jfr_module_available(loader))),
        b"getEventWriter" | b"newEventWriter" => Some(Value::Null),
        b"flush" if descriptor == b"(Ljdk/jfr/internal/event/EventWriter;II)V" => None,
        b"flush" => None,
        b"setRepositoryLocation" => {
            state().repository = arg_string(loader, arguments, 0).map(PathBuf::from);
            None
        }
        b"setDumpPath" => {
            state().dump_path = arg_string(loader, arguments, 0);
            None
        }
        b"getDumpPath" => {
            let path = state().dump_path.clone().unwrap_or_default();
            Some(string_value(loader, &path))
        }
        b"abort" => {
            let message = arg_string(loader, arguments, 0);
            java_support::abort(message.as_deref(), true);
            None
        }
        b"addStringConstant" => {
            let mut st = state();
            let id = arg_long(arguments, 0);
            let text = arg_string(loader, arguments, 1).unwrap_or_default();
            let inserted = !st.string_constants.contains_key(&id);
            st.string_constants.insert(id, text);
            Some(int_value(inserted))
        }
        b"uncaughtException" => {
            let thread = arguments.first().copied().unwrap_or(Value::Null);
            let throwable = arguments.get(1).copied().unwrap_or(Value::Null);
            java_support::uncaught_exception(loader, throwable);
            let _ = thread;
            None
        }
        b"setForceInstrumentation" => {
            event_class_transformer::set_force_instrumentation(arg_bool(arguments, 0));
            None
        }
        b"getUnloadedEventClassCount" => {
            Some(long_value(event_class_transformer::unloaded_event_class_count() as i64))
        }
        b"setMiscellaneous" => {
            let mut st = state();
            st.miscellaneous.insert("first", arg_long(arguments, 0));
            st.miscellaneous.insert("second", arg_long(arguments, 1));
            None
        }
        b"setThrottle" => {
            let mut st = state();
            let request = arg_long(arguments, 0);
            let value = arg_long(arguments, 1);
            let threshold = arg_long(arguments, 2);
            st.throttles.insert(request as u64, (value, threshold, request));
            Some(int_value(true))
        }
        b"setCPURate" => {
            state().cpu_rate = match arguments.first() {
                Some(Value::Double(v)) => *v,
                _ => 0.0,
            };
            None
        }
        b"setCPUPeriod" => {
            state().cpu_period = arg_long(arguments, 0);
            None
        }
        b"emitOldObjectSamples" => {
            leakprofiler::emit_events(
                arg_long(arguments, 0),
                arg_bool(arguments, 1),
                arg_bool(arguments, 2),
            );
            None
        }
        b"shouldRotateDisk" => Some(int_value(should_rotate_disk())),
        b"exclude" => {
            let thread = arguments.first().copied().unwrap_or(Value::Null);
            java_support::exclude_thread(loader, thread)?;
            None
        }
        b"include" => {
            let thread = arguments.first().copied().unwrap_or(Value::Null);
            java_support::include_thread(loader, thread)?;
            None
        }
        b"isExcluded" if String::from_utf8_lossy(descriptor).contains("Thread") => {
            let thread = arguments.first().copied().unwrap_or(Value::Null);
            Some(int_value(java_support::is_thread_excluded(loader, thread)?))
        }
        b"isExcluded" => {
            let name = arg_string(loader, arguments, 0).unwrap_or_default();
            Some(int_value(
                state().excluded_classes.iter().any(|c| String::from_utf8_lossy(c) == name),
            ))
        }
        b"getChunkStartNanos" => Some(long_value(state().chunk_start_nanos)),
        b"getConfiguration" => {
            let clazz = arguments.first().copied().unwrap_or(Value::Null);
            match java_support::get_configuration(loader, clazz)? {
                Some(value) => Some(value),
                None => Some(Value::Null),
            }
        }
        b"setConfiguration" => {
            let clazz = arguments.first().copied().unwrap_or(Value::Null);
            let configuration = arguments.get(1).copied().unwrap_or(Value::Null);
            Some(int_value(java_support::set_configuration(
                loader, clazz, configuration,
            )?))
        }
        b"isInstrumented" => {
            let clazz = arguments.first().copied().unwrap_or(Value::Null);
            Some(int_value(java_support::is_instrumented(loader, clazz)?))
        }
        b"isContainerized" => Some(int_value(is_containerized())),
        b"hostTotalMemory" => Some(long_value(Os::physical_memory() as i64)),
        b"hostTotalSwapMemory" => Some(long_value(host_total_swap_memory())),
        b"emitDataLoss" => {
            let _ = arg_long(arguments, 0);
            None
        }
        b"registerStackFilter" => {
            let includes = string_array_to_vec(loader, arguments, 0);
            let excludes = string_array_to_vec(loader, arguments, 1);
            let mut st = state();
            let id = st.next_stack_filter_id;
            st.next_stack_filter_id += 1;
            st.stack_filters.insert(id, (includes, excludes));
            Some(long_value(id))
        }
        b"unregisterStackFilter" => {
            state().stack_filters.remove(&arg_long(arguments, 0));
            None
        }
        b"isProduct" => Some(int_value(true)),
        b"setMethodTraceFilters" => {
            let arrays = method_trace_filter_arrays(loader, arguments);
            Some(long_value(arrays))
        }
        b"drainStaleMethodTracerIds" => {
            let array = crate::share::oops::typeArrayOop::TypeArrayOop::Long(Vec::new());
            Some(Value::PrimitiveArray(loader.alloc_type_array(array)))
        }
        _ => {
            return Err(io::Error::other(format!(
                "Unsupported JFR native: {}",
                String::from_utf8_lossy(name)
            )));
        }
    };
    Ok(result)
}

pub fn dispatch_global(
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'static>],
    output: &mut dyn Write,
) -> io::Result<Option<Value<'static>>> {
    let Some(loader) = threading::jvm_loader() else {
        return Err(io::Error::other("JVM not initialized"));
    };
    dispatch(&loader, name, descriptor, arguments, output)
}

fn type_id_for<'a>(loader: &ClassLoader<'a>, arguments: &[Value<'a>], index: usize) -> i64 {
    let key = match arguments.get(index).copied() {
        Some(Value::ClassMirror(name)) => String::from_utf8_lossy(name).into_owned(),
        Some(Value::String(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Some(value) => java_support::c_str(loader, value).unwrap_or_default(),
        None => String::new(),
    };
    let mut st = state();
    if let Some(id) = st.type_ids.get(&key) {
        return *id;
    }
    let id = st.next_type_id;
    st.next_type_id += 1;
    st.type_ids.insert(key, id);
    id
}

fn rotate_chunk(st: &mut JfrJniState, output: &mut dyn Write) -> io::Result<()> {
    let dir = repository_dir(st);
    let _ = std::fs::create_dir_all(&dir);
    let seq = st.last_chunk_final;
    let path = dir.join(format!("chunk-{seq}.jfr"));
    if !path.exists() {
        let mut file = std::fs::File::create(&path)?;
        file.write_all(b"FLR\0")?;
        file.write_all(&st.chunk_start_nanos.to_le_bytes())?;
        file.write_all(&now_nanos().to_le_bytes())?;
    }
    st.last_chunk_final = now_nanos();
    st.chunk_start_nanos = st.last_chunk_final;
    let _ = writeln!(output, "Chunk rotated: {}", path.display());
    Ok(())
}

fn should_rotate_disk() -> bool {
    let dir = repository_dir(&state());
    let total: u64 = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    total > 0
}

fn is_containerized() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|c| c.contains("docker") || c.contains("kubepods") || c.contains("containerd"))
            .unwrap_or(false)
        || std::env::var_os("container").is_some()
}

fn host_total_swap_memory() -> i64 {
    #[cfg(unix)]
    {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if line.starts_with("SwapTotal:") {
                    return line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|v| v.parse::<i64>().ok())
                        .map(|kb| kb * 1024)
                        .unwrap_or(0);
                }
            }
        }
    }
    0
}

fn log_native<'a>(name: &[u8], arguments: &[Value<'a>], loader: &ClassLoader<'a>) {
    match name {
        b"log" => {
            let tag = arg_int(arguments, 0);
            let level = arg_int(arguments, 1);
            let message = arg_string(loader, arguments, 2).unwrap_or_default();
            eprintln!("[jfr:{tag}:{level}] {message}");
        }
        b"logEvent" => {
            let _ = (arg_int(arguments, 0), arguments.get(1));
        }
        _ => {
            let _ = arg_int(arguments, 0);
            let _ = arg_int(arguments, 1);
        }
    }
}

fn string_array_to_vec<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    index: usize,
) -> Vec<String> {
    let Some(Value::ObjectArray(handle)) = arguments.get(index).copied() else {
        return Vec::new();
    };
    let arrays = loader.obj_arrays.borrow();
    let Some(array) = arrays.get(handle) else {
        return Vec::new();
    };
    array
        .elements
        .iter()
        .filter_map(|value| java_support::c_str(loader, *value))
        .collect()
}

fn method_trace_filter_arrays<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> i64 {
    let classes = string_array_to_vec(loader, arguments, 0);
    let methods = string_array_to_vec(loader, arguments, 1);
    let signatures = string_array_to_vec(loader, arguments, 2);
    let mut filters: Vec<(String, String, String, i32)> = Vec::new();
    if let Some(Value::PrimitiveArray(handle)) = arguments.get(3).copied() {
        let arrays = loader.arrays.borrow();
        if let Some(crate::share::oops::typeArrayOop::TypeArrayOop::Int(values)) =
            arrays.get(handle)
        {
            for (index, modification) in values.iter().enumerate() {
                filters.push((
                    classes.get(index).cloned().unwrap_or_default(),
                    methods.get(index).cloned().unwrap_or_default(),
                    signatures.get(index).cloned().unwrap_or_default(),
                    *modification,
                ));
            }
        }
    }
    let mut st = state();
    let base = st.next_stack_trace_id;
    for (offset, (class, method, signature, modification)) in filters.iter().enumerate() {
        st.stack_filters.insert(
            base + offset as i64,
            (vec![class.clone()], vec![format!("{method}{signature}:{modification}")]),
        );
    }
    st.next_stack_trace_id += filters.len() as i64;
    base
}
