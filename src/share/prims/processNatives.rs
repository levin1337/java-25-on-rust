//! Native methods of `java.lang.ProcessImpl`, `java.lang.ProcessHandleImpl`
//! and `ProcessHandleImpl.Info` on Windows, backed by real Win32 process
//! and toolhelp APIs.
//!
//! `ProcessImpl.create` runs `CreateProcessW` with `STARTF_USESTDHANDLES`:
//! each `-1` slot in the Java `stdHandles` array becomes an anonymous pipe
//! whose parent end is registered in the shared handle table (so the JDK's
//! `FileDispatcher` read/write natives work on it unchanged); the child end
//! is inherited by the new process. The returned value is the raw process
//! `HANDLE`, used by the wait/exit-code natives.

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use crate::share::prims::nioFileNatives::{alloc_handle, alloc_handle_unseekable, EntryKind};
use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::ptr;

type Handle = *mut c_void;

const INVALID_HANDLE: Handle = ptr::null_mut();
const STILL_ACTIVE: u32 = 259;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x102;
const INFINITE: u32 = 0xFFFF_FFFF;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
const HANDLE_FLAG_INHERIT: u32 = 0x1;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_ALL: u32 = 0x7;
const OPEN_ALWAYS: u32 = 4;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const PROCESS_TERMINATE: u32 = 0x1;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
const SYNCHRONIZE: u32 = 0x0010_0000;
const MAX_PATH: usize = 260;

unsafe extern "system" {
    fn CreatePipe(read: *mut Handle, write: *mut Handle, sa: *mut c_void, size: u32) -> i32;
    fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    fn CreateProcessW(
        application: *const u16,
        command: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit_handles: i32,
        creation_flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup_info: *const StartupInfo,
        process_info: *mut ProcessInformation,
    ) -> i32;
    fn GetExitCodeProcess(handle: Handle, code: *mut u32) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn TerminateProcess(handle: Handle, exit_code: u32) -> i32;
    fn GetProcessId(handle: Handle) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn CreateFileW(
        name: *const u16,
        desired: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags_and_attributes: u32,
        template: Handle,
    ) -> Handle;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn GetProcessTimes(
        handle: Handle,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn QueryFullProcessImageNameW(
        handle: Handle,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_count_chars: u32,
    y_count_chars: u32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
    reserved2: u16,
    reserved2_ptr: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}

#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    _pad: u32,
    default_heap: u64,
    module: u32,
    threads: u32,
    parent_pid: u32,
    pri_class_base: i32,
    flags: u32,
    name: [u16; MAX_PATH],
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

fn wide(text: &str) -> Vec<u16> {
    let mut out: Vec<u16> = text.encode_utf16().collect();
    out.push(0);
    out
}

fn arg_string<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
    index: usize,
) -> io::Result<Option<String>> {
    let value = arguments.get(index).copied().unwrap_or(Value::Null);
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    super::stringNatives::string(loader, value).map(|cow| cow.map(|c| c.into_owned()))
}

fn arg_i64(arguments: &[Value<'_>], index: usize) -> i64 {
    match arguments.get(index).copied().unwrap_or(Value::Null) {
        Value::Long(value) => value,
        Value::Int(value) => i64::from(value),
        _ => 0,
    }
}

/// Reads/writes the `long[]` handle array passed to `ProcessImpl.create`.
fn with_handle_array<'a, R>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    f: impl FnOnce(&mut Vec<i64>) -> R,
) -> io::Result<R> {
    let Value::PrimitiveArray(handle) = value else {
        return Err(invalid("Expected long[] handles"));
    };
    let mut arrays = loader.arrays.borrow_mut();
    let array = arrays.get_mut(handle).ok_or_else(|| invalid("Invalid handles array"))?;
    let TypeArrayOop::Long(values) = array else {
        return Err(invalid("Expected long[] handles"));
    };
    Ok(f(values))
}

fn host_string<'a>(loader: &ClassLoader<'a>, text: &str) -> Value<'a> {
    Value::HostString(loader.storage.alloc_str(text))
}

fn file_time_to_unix_ms(filetime: u64) -> i64 {
    // FILETIME is 100ns intervals since 1601-01-01.
    const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
    if filetime <= EPOCH_DIFF_100NS {
        return 0;
    }
    ((filetime - EPOCH_DIFF_100NS) / 10_000) as i64
}

/// Opens a process for `access` rights; `None` when it does not exist.
fn open_process(pid: u32, access: u32) -> Handle {
    unsafe { OpenProcess(access, 0, pid) }
}

/// Process start time in ms since epoch, `0` if undeterminable, `-1` if the
/// pid does not exist or the process already exited.
fn process_start_time(pid: u32) -> i64 {
    let handle = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION);
    if handle.is_null() {
        return -1;
    }
    let mut code = 0u32;
    let alive = unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == STILL_ACTIVE;
    let mut times = [0u64; 4];
    let has_times = unsafe {
        GetProcessTimes(
            handle,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    } != 0;
    unsafe { CloseHandle(handle) };
    if !alive {
        return -1;
    }
    if has_times {
        file_time_to_unix_ms(times[0])
    } else {
        0
    }
}

/// One Toolhelp32 process snapshot entry.
struct ProcInfo {
    pid: u32,
    ppid: u32,
    name: String,
}

fn process_snapshot() -> io::Result<Vec<ProcInfo>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot as isize == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut out = Vec::new();
    let mut entry = ProcessEntry {
        size: std::mem::size_of::<ProcessEntry>() as u32,
        usage: 0,
        pid: 0,
        _pad: 0,
        default_heap: 0,
        module: 0,
        threads: 0,
        parent_pid: 0,
        pri_class_base: 0,
        flags: 0,
        name: [0; MAX_PATH],
    };
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let len = entry
            .name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.name.len());
        out.push(ProcInfo {
            pid: entry.pid,
            ppid: entry.parent_pid,
            name: String::from_utf16_lossy(&entry.name[..len]),
        });
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    Ok(out)
}

fn set_long_array<'a>(loader: &ClassLoader<'a>, value: Value<'a>, f: impl FnOnce(&mut Vec<i64>)) -> io::Result<()> {
    with_handle_array(loader, value, |values| f(values))
}

/// `ProcessImpl` statics and `ProcessHandleImpl`/`Info` natives.
pub(crate) fn statik<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let result = match (owner, name, descriptor) {
        (b"java/lang/ProcessImpl", b"getStillActive", b"()I") => Value::Int(STILL_ACTIVE as i32),
        (b"java/lang/ProcessImpl", b"getExitCodeProcess", b"(J)I") => {
            let mut code = 0u32;
            let handle = arg_i64(arguments, 0) as usize as Handle;
            if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
                return Err(invalid("GetExitCodeProcess failed"));
            }
            Value::Int(code as i32)
        }
        (b"java/lang/ProcessImpl", b"waitForInterruptibly", b"(J)V") => {
            let handle = arg_i64(arguments, 0) as usize as Handle;
            unsafe { WaitForSingleObject(handle, INFINITE) };
            return Ok(Some(Value::Null));
        }
        (b"java/lang/ProcessImpl", b"waitForTimeoutInterruptibly", b"(JJ)V") => {
            let handle = arg_i64(arguments, 0) as usize as Handle;
            let timeout = arg_i64(arguments, 1).max(0) as u32;
            unsafe { WaitForSingleObject(handle, timeout) };
            return Ok(Some(Value::Null));
        }
        (b"java/lang/ProcessImpl", b"terminateProcess", b"(J)V") => {
            let handle = arg_i64(arguments, 0) as usize as Handle;
            if unsafe { TerminateProcess(handle, 1) } == 0 {
                return Err(invalid("TerminateProcess failed"));
            }
            return Ok(Some(Value::Null));
        }
        (b"java/lang/ProcessImpl", b"getProcessId0", b"(J)I") => {
            let handle = arg_i64(arguments, 0) as usize as Handle;
            Value::Int(unsafe { GetProcessId(handle) } as i32)
        }
        (b"java/lang/ProcessImpl", b"isProcessAlive", b"(J)Z") => {
            let handle = arg_i64(arguments, 0) as usize as Handle;
            Value::Int(i32::from(
                unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT,
            ))
        }
        (b"java/lang/ProcessImpl", b"create", b"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[JZ)J") => {
            return create_process(loader, arguments).map(Some);
        }
        (b"java/lang/ProcessImpl", b"openForAtomicAppend", b"(Ljava/lang/String;)J") => {
            let Some(path) = arg_string(loader, arguments, 0)? else {
                return Err(invalid("Null path"));
            };
            let name = wide(&path.replace('/', "\\"));
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_WRITE | 0x4, // FILE_APPEND_DATA
                    FILE_SHARE_ALL,
                    ptr::null_mut(),
                    OPEN_ALWAYS,
                    FILE_ATTRIBUTE_NORMAL,
                    ptr::null_mut(),
                )
            };
            if handle.is_null() || handle as isize == -1 {
                return Err(invalid("CreateFile failed for append"));
            }
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            Value::Long(handle as usize as i64)
        }
        (b"java/lang/ProcessImpl", b"closeHandle", b"(J)Z") => {
            let handle = arg_i64(arguments, 0) as usize as Handle;
            Value::Int(i32::from(unsafe { CloseHandle(handle) } != 0))
        }
        (b"java/lang/ProcessImpl", b"initNative", b"()V")
        | (b"java/lang/ProcessHandleImpl", b"initNative", b"()V") => return Ok(Some(Value::Null)),
        (b"java/lang/ProcessImpl", b"waitForProcessExit0", b"(JZ)I") => {
            let handle = arg_i64(arguments, 0) as usize as Handle;
            unsafe { WaitForSingleObject(handle, INFINITE) };
            let mut code = 0u32;
            if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
                return Err(invalid("GetExitCodeProcess failed"));
            }
            Value::Int(code as i32)
        }
        (b"java/lang/ProcessHandleImpl", b"waitForProcessExit0", b"(JZ)I") => {
            // ProcessHandleImpl passes a pid, not a process handle.
            let pid = arg_i64(arguments, 0) as u32;
            let handle = open_process(pid, SYNCHRONIZE);
            if handle.is_null() {
                return Ok(Some(Value::Int(-1)));
            }
            unsafe { WaitForSingleObject(handle, INFINITE) };
            let mut code = 0u32;
            let ok = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
            unsafe { CloseHandle(handle) };
            if !ok {
                return Err(invalid("GetExitCodeProcess failed"));
            }
            Value::Int(code as i32)
        }
        (b"java/lang/ProcessHandleImpl", b"getCurrentPid0", b"()J") => {
            Value::Long(unsafe { GetCurrentProcessId() } as i64)
        }
        (b"java/lang/ProcessHandleImpl", b"isAlive0", b"(J)J") => {
            Value::Long(process_start_time(arg_i64(arguments, 0) as u32))
        }
        (b"java/lang/ProcessHandleImpl", b"parent0", b"(JJ)J") => {
            let pid = arg_i64(arguments, 0) as u32;
            let start = arg_i64(arguments, 1);
            let actual = process_start_time(pid);
            if actual < 0 || (start != 0 && actual != 0 && actual != start) {
                return Ok(Some(Value::Long(-1)));
            }
            let ppid = process_snapshot()
                .ok()
                .and_then(|procs| procs.into_iter().find(|p| p.pid == pid))
                .map(|p| p.ppid as i64)
                .unwrap_or(-1);
            Value::Long(ppid)
        }
        (b"java/lang/ProcessHandleImpl", b"destroy0", b"(JJZ)Z") => {
            let pid = arg_i64(arguments, 0) as u32;
            let start = arg_i64(arguments, 1);
            let _force = arg_i64(arguments, 2) != 0;
            let actual = process_start_time(pid);
            if actual < 0 || (start != 0 && actual != 0 && actual != start) {
                return Ok(Some(Value::Int(0)));
            }
            let handle = open_process(pid, PROCESS_TERMINATE);
            if handle.is_null() {
                return Ok(Some(Value::Int(0)));
            }
            let ok = unsafe { TerminateProcess(handle, 1) } != 0;
            unsafe { CloseHandle(handle) };
            return Ok(Some(Value::Int(i32::from(ok))));
        }
        (b"java/lang/ProcessHandleImpl", b"getProcessPids0", b"(J[J[J[J)I") => {
            return get_process_pids(loader, arguments).map(Some);
        }
        (b"java/lang/ProcessHandleImpl$Info", b"initIDs", b"()V") => return Ok(Some(Value::Null)),
        (b"java/lang/ProcessHandleImpl$Info", b"info0", b"(J)V") => {
            return info0(loader, arguments).map(|_| Some(Value::Null));
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// `ProcessImpl.create(cmdstr, envblock, dir, stdHandles, redirectErrorStream)`.
fn create_process<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let cmd = arg_string(loader, arguments, 0)?.ok_or_else(|| invalid("Null command"))?;
    let envblock = arg_string(loader, arguments, 1)?.unwrap_or_default();
    let dir = arg_string(loader, arguments, 2)?;
    let redirect = arg_i64(arguments, 4) != 0;
    let handles_value = arguments.get(3).copied().unwrap_or(Value::Null);

    // Child-side std handles; -1 means "create a pipe".
    let mut child = [ptr::null_mut::<c_void>(); 3];
    let mut pipe_parents: Vec<Handle> = Vec::new();
    let requested = with_handle_array(loader, handles_value, |values| {
        values.iter().take(3).map(|v| *v).collect::<Vec<_>>()
    })?;

    for (i, &requested_handle) in requested.iter().enumerate() {
        if requested_handle == -1 {
            let mut read = ptr::null_mut::<c_void>();
            let mut write = ptr::null_mut::<c_void>();
            if unsafe { CreatePipe(&mut read, &mut write, ptr::null_mut(), 0) } == 0 {
                return Err(invalid("CreatePipe failed"));
            }
            // stdin: child reads, parent writes. stdout/stderr: child writes,
            // parent reads.
            let (child_end, parent_end) = if i == 0 { (read, write) } else { (write, read) };
            unsafe {
                SetHandleInformation(child_end, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
                // Parent ends must NOT be inherited by the child.
                SetHandleInformation(parent_end, HANDLE_FLAG_INHERIT, 0);
            }
            child[i] = child_end;
            pipe_parents.push(parent_end);
        } else {
            let handle = requested_handle as usize as Handle;
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            child[i] = handle;
        }
    }
    if redirect {
        child[2] = child[1];
    }

    let mut startup = StartupInfo {
        cb: std::mem::size_of::<StartupInfo>() as u32,
        reserved: ptr::null_mut(),
        desktop: ptr::null_mut(),
        title: ptr::null_mut(),
        x: 0,
        y: 0,
        x_size: 0,
        y_size: 0,
        x_count_chars: 0,
        y_count_chars: 0,
        fill_attribute: 0,
        flags: STARTF_USESTDHANDLES,
        show_window: 0,
        reserved2: 0,
        reserved2_ptr: ptr::null_mut(),
        stdin: child[0],
        stdout: child[1],
        stderr: child[2],
    };

    let mut command_line = wide(&cmd);
    let mut environment: Vec<u16> = envblock.encode_utf16().collect();
    if !environment.is_empty() && *environment.last().unwrap() != 0 {
        environment.push(0);
    }
    environment.push(0);
    let directory = dir.as_deref().map(wide);

    let mut info = ProcessInformation {
        process: ptr::null_mut(),
        thread: ptr::null_mut(),
        pid: 0,
        tid: 0,
    };
    let ok = unsafe {
        CreateProcessW(
            ptr::null(),
            command_line.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            1,
            CREATE_UNICODE_ENVIRONMENT,
            if environment.len() > 1 {
                environment.as_ptr() as *const c_void
            } else {
                ptr::null()
            },
            directory.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
            &startup,
            &mut info,
        )
    };

    // Close our copies of the child ends and the thread handle; the child
    // holds its inherited duplicates.
    for i in 0..3 {
        if requested.get(i).copied().unwrap_or(-1) == -1 && !(redirect && i == 2) {
            unsafe { CloseHandle(child[i]) };
        }
    }
    if !info.thread.is_null() {
        unsafe { CloseHandle(info.thread) };
    }
    if ok == 0 || info.process.is_null() {
        for handle in pipe_parents {
            unsafe { CloseHandle(handle) };
        }
        return Err(invalid("CreateProcess failed"));
    }

    // Register parent pipe ends in the handle table and write the table ids
    // back into the Java `long[]` (JDK contract: non -1 elements are parent
    // handles for FileDispatcher).
    let mut index = 0usize;
    set_long_array(loader, handles_value, |values| {
        for i in 0..3 {
            if requested.get(i).copied().unwrap_or(-1) == -1 {
                if let Some(parent) = pipe_parents.get(index) {
                    let file = unsafe { std::fs::File::from_raw_handle(*parent as RawHandle) };
                    let id = alloc_handle_unseekable(EntryKind::File(file), String::new());
                    if i < values.len() {
                        values[i] = id as i64;
                    }
                    index += 1;
                }
            }
        }
    })?;

    Ok(Value::Long(info.process as usize as i64))
}

/// `ProcessHandleImpl.getProcessPids0(pid, pids, ppids, starttimes)`.
fn get_process_pids<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Value<'a>> {
    let parent = arg_i64(arguments, 0);
    let mut out_pids = Vec::new();
    let snapshot = process_snapshot()?;
    for proc in &snapshot {
        if parent != 0 && proc.ppid as i64 != parent {
            continue;
        }
        out_pids.push((proc.pid as i64, proc.ppid as i64, process_start_time(proc.pid)));
    }
    let needed = out_pids.len();
    for (index, arg_index) in [1usize, 2, 3].into_iter().enumerate() {
        let value = arguments.get(arg_index).copied().unwrap_or(Value::Null);
        if matches!(value, Value::Null) {
            continue;
        }
        let Value::PrimitiveArray(handle) = value else {
            return Err(invalid("Expected long[] argument"));
        };
        let mut arrays = loader.arrays.borrow_mut();
        let Some(TypeArrayOop::Long(values)) = arrays.get_mut(handle) else {
            return Err(invalid("Expected long[] argument"));
        };
        let take = needed.min(values.len());
        for (slot, entry) in values.iter_mut().take(take).enumerate() {
            *entry = match index {
                0 => out_pids[slot].0,
                1 => out_pids[slot].1,
                _ => out_pids[slot].2,
            };
        }
    }
    Ok(Value::Int(needed as i32))
}

/// `ProcessHandleImpl.Info.info0(pid)` — fills command/startTime fields on
/// the receiver.
fn info0<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<()> {
    let receiver = match arguments.first().copied().unwrap_or(Value::Null) {
        Value::Object(handle) => handle,
        _ => return Err(invalid("Expected Info receiver")),
    };
    let pid = arg_i64(arguments, 1) as u32;
    let start = process_start_time(pid);
    let mut command: Option<String> = None;
    if start >= 0 {
        let handle = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION);
        if !handle.is_null() {
            let mut buf = vec![0u16; MAX_PATH * 4];
            let mut size = buf.len() as u32;
            if unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size) } != 0 {
                let len = (size as usize).min(buf.len());
                command = Some(String::from_utf16_lossy(&buf[..len]));
            }
            unsafe { CloseHandle(handle) };
        }
    }
    let (command_slot, start_slot) = {
        let objects = loader.borrow_objects();
        let Some(object) = objects.get(receiver) else {
            return Err(invalid("Invalid receiver"));
        };
        let command_slot = object.klass.instanceField(b"command", b"Ljava/lang/String;").ok();
        let start_slot = object.klass.instanceField(b"startTime", b"J").ok();
        (command_slot, start_slot)
    };
    let mut objects = loader.borrow_objects_mut();
    let object = objects.get_mut(receiver).ok_or_else(|| invalid("Invalid receiver"))?;
    if let Some(slot) = command_slot
        && slot < object.fields.len()
        && let Some(text) = command
    {
        object.fields[slot] = host_string(loader, &text);
    }
    if let Some(slot) = start_slot
        && slot < object.fields.len()
        && start >= 0
    {
        object.fields[slot] = Value::Long(start);
    }
    Ok(())
}
