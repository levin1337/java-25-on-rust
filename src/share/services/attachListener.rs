/*
 * Port of OpenJDK 25 HotSpot Attach Listener (share/services/attachListener.*)
 * plus the Windows transport (os/windows/attachListener_windows.cpp).
 *
 * Implements the Attach API protocol (V1 and V2), the attach operation
 * queue and the Windows named-pipe transport. Exports JVM_EnqueueOperation
 * and JVM_EnqueueOperation_v2 for the remote attach thread.
 */

use crate::share::services::diagnosticCommand::print_thread_dump;
use crate::share::services::diagnosticFramework::Executor;
use crate::share::services::diagnosticFramework::DCmdSource;
use crate::share::services::heapDumper::HeapDumper;
use crate::share::services::management::Management;
use crate::share::services::writeableFlags::WriteableFlags;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr};
use std::io::Write;
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};

pub const JNI_OK: i32 = 0;
pub const JNI_ERR: i32 = -1;

pub const ATTACH_ERROR_BADVERSION: i32 = 101;
pub const ATTACH_ERROR_BADARGCOUNT: i32 = 103;
pub const ATTACH_ERROR_ILLEGALARG: i32 = 104;
pub const ATTACH_ERROR_INTERNAL: i32 = 107;
pub const ATTACH_ERROR_DISABLED: i32 = 108;
pub const ATTACH_ERROR_RESOURCE: i32 = 109;
pub const ATTACH_ERROR_NO_SUCH_FILE: i32 = 113;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttachListenerState {
    NotInitialized = 0,
    Initializing = 1,
    Initialized = 2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttachApiVersion {
    V1 = 1,
    V2 = 2,
}

// ---------------------------------------------------------------------------
// AttachOperation
// ---------------------------------------------------------------------------

pub trait RequestReader {
    fn read(&mut self, buf: &mut [u8]) -> isize;
}

pub trait ReplyWriter {
    fn write(&mut self, buf: &[u8]) -> isize;
    fn flush(&mut self) {}

    fn write_fully(&mut self, buf: &[u8]) -> bool {
        let mut off = 0usize;
        while off < buf.len() {
            let n = self.write(&buf[off..]);
            if n <= 0 {
                return false;
            }
            off += n as usize;
        }
        true
    }

    fn write_reply_msg(&mut self, result: i32, message: &[u8]) -> bool {
        let head = format!("{}\n", result).into_bytes();
        self.write_fully(&head) && self.write_fully(message)
    }

    fn write_reply_stream(&mut self, result: i32, payload: &[u8]) -> bool {
        self.write_reply_msg(result, payload)
    }
}

pub struct AttachOperation {
    name: String,
    args: Vec<String>,
    streaming_output: bool,
}

impl AttachOperation {
    pub const NAME_LENGTH_MAX: usize = 16;
    pub const ARG_LENGTH_MAX: usize = 1024;
    pub const ARG_COUNT_MAX: usize = 3;

    pub fn new() -> Self {
        Self {
            name: String::new(),
            args: Vec::new(),
            streaming_output: false,
        }
    }

    pub fn detachall_operation_name() -> &'static str {
        "detachall"
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    pub fn arg(&self, i: usize) -> Option<&str> {
        self.args.get(i).map(|s| s.as_str())
    }

    pub fn arg_count(&self) -> usize {
        self.args.len()
    }

    pub fn append_arg(&mut self, arg: &str) {
        self.args.push(arg.to_string());
    }

    pub fn set_arg(&mut self, i: usize, arg: &str) {
        while self.args.len() <= i {
            self.args.push(String::new());
        }
        self.args[i] = arg.to_string();
    }

    pub fn streaming_output(&self) -> bool {
        self.streaming_output
    }

    pub fn set_streaming_output(&mut self, streaming: bool) {
        self.streaming_output = streaming;
    }
}

// ---------------------------------------------------------------------------
// AttachStream — buffered/streaming output stream for attach replies.
// ---------------------------------------------------------------------------

pub struct BufferedStream {
    buf: Vec<u8>,
}

impl BufferedStream {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn reset(&mut self) {
        self.buf.clear();
    }
}

impl Write for BufferedStream {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
enum ResultState {
    Unset,
    Set,
    Written,
}

pub struct AttachStream<'a> {
    reply_writer: Option<&'a mut dyn ReplyWriter>,
    allow_streaming: bool,
    result_state: ResultState,
    result: i32,
    error: bool,
    buffer: Vec<u8>,
}

impl<'a> AttachStream<'a> {
    pub fn new(reply_writer: Option<&'a mut dyn ReplyWriter>, allow_streaming: bool) -> Self {
        let has_writer = reply_writer.is_some();
        Self {
            reply_writer,
            allow_streaming: has_writer && allow_streaming,
            result_state: ResultState::Unset,
            result: JNI_OK,
            error: false,
            buffer: Vec::new(),
        }
    }

    fn is_streaming(&self) -> bool {
        self.result_state != ResultState::Unset && self.allow_streaming
    }

    fn flush_reply(&mut self) {
        if self.error {
            return;
        }
        if let Some(w) = self.reply_writer.as_deref_mut() {
            if self.result_state != ResultState::Written {
                if w.write_reply_stream(self.result, &self.buffer) {
                    self.result_state = ResultState::Written;
                    self.buffer.clear();
                } else {
                    self.error = true;
                }
            } else if !self.buffer.is_empty() {
                self.error = !w.write_fully(&self.buffer);
                self.buffer.clear();
            }
        }
    }

    pub fn set_result(&mut self, result: i32) {
        if self.result_state == ResultState::Unset {
            self.result = result;
            self.result_state = ResultState::Set;
            if self.allow_streaming {
                self.flush_reply();
            }
        }
    }

    pub fn get_result(&self) -> i32 {
        self.result
    }

    pub fn complete(&mut self) {
        if self.reply_writer.is_some() {
            self.flush_reply();
        }
    }

    pub fn buffered(&self) -> &[u8] {
        &self.buffer
    }
}

impl Write for AttachStream<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.is_streaming() {
            if !self.error {
                if let Some(w) = self.reply_writer.as_deref_mut() {
                    self.error = !w.write_fully(b);
                }
            }
        } else {
            self.buffer.extend_from_slice(b);
        }
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.allow_streaming {
            self.flush_reply();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Request parsing (AttachOperation::RequestReader)
// ---------------------------------------------------------------------------

const MAX_VALUE: i64 = i32::MAX as i64 / 20;

pub fn read_uint(reader: &mut dyn RequestReader, may_be_empty: bool) -> i64 {
    let mut value: i64 = 0;
    let mut first = true;
    loop {
        let mut ch = [0u8; 1];
        let n = reader.read(&mut ch);
        if n != 1 {
            if n == 0 && (!may_be_empty || !first) {
                eprintln!("Failed to read int value: EOF");
            }
            return -1;
        }
        first = false;
        let c = ch[0];
        if c == 0 {
            return value;
        }
        if !(b'0'..=b'9').contains(&c) {
            eprintln!("Failed to read int value: unexpected symbol");
            return -1;
        }
        if value >= MAX_VALUE {
            eprintln!("Failed to read int value: too big");
            return -1;
        }
        value = value * 10 + (c - b'0') as i64;
    }
}

pub fn parse_options(op: &mut AttachOperation, s: &str) {
    for option in s.split(',') {
        if option.is_empty() {
            continue;
        }
        let (name, value) = match option.find('=') {
            Some(i) => (&option[..i], &option[i + 1..]),
            None => (option, ""),
        };
        if name == "streaming" {
            if value == "1" {
                op.set_streaming_output(true);
            } else if value == "0" {
                op.set_streaming_output(false);
            }
        }
    }
}

pub fn read_request_data(
    reader: &mut dyn RequestReader,
    op: &mut AttachOperation,
    buffer_size: usize,
    min_str_count: usize,
    min_read_size: usize,
) -> bool {
    let mut buffer = vec![0u8; buffer_size];
    let mut str_count = 0usize;
    let mut off = 0usize;
    let mut left = buffer_size;

    loop {
        let n = reader.read(&mut buffer[off..off + left]);
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        let n = n as usize;
        if min_str_count > 0 {
            for i in 0..n {
                if buffer[off + i] == 0 {
                    str_count += 1;
                }
            }
        }
        off += n;
        left -= n;
        if !(left > 0 && (off < min_read_size || str_count < min_str_count)) {
            break;
        }
    }

    if off < min_read_size || str_count < min_str_count {
        eprintln!("Failed to read request: incomplete request");
        return false;
    }
    if buffer[off - 1] != 0 {
        eprintln!("Failed to read request: not terminated");
        return false;
    }

    let data = &buffer[..off];
    let first_nul = data.iter().position(|&b| b == 0).unwrap_or(off);
    let name_part = &data[..first_nul];
    let arguments = &data[first_nul + 1..];

    let (name, options_str) = match name_part.iter().position(|&b| b == b' ') {
        Some(i) => {
            let opts = String::from_utf8_lossy(&name_part[i + 1..]).into_owned();
            (String::from_utf8_lossy(&name_part[..i]).into_owned(), Some(opts))
        }
        None => (String::from_utf8_lossy(name_part).into_owned(), None),
    };

    if let Some(opts) = options_str {
        parse_options(op, &opts);
    }
    op.set_name(&name);

    let mut cur = arguments;
    while cur.len() > 0 {
        match cur.iter().position(|&b| b == 0) {
            Some(i) => {
                let arg = String::from_utf8_lossy(&cur[..i]).into_owned();
                op.append_arg(&arg);
                cur = &cur[i + 1..];
            }
            None => break,
        }
        if cur.is_empty() {
            break;
        }
    }

    true
}

pub fn read_request(
    reader: &mut dyn RequestReader,
    error_writer: Option<&mut dyn ReplyWriter>,
    op: &mut AttachOperation,
) -> bool {
    let mut err = error_writer;
    let ver = read_uint(reader, true);
    if ver < 0 {
        return false;
    }
    let buffer_size: usize;
    let min_str_count;
    let min_read_size;
    match ver {
        1 => {
            buffer_size = (AttachOperation::NAME_LENGTH_MAX + 1)
                + AttachOperation::ARG_COUNT_MAX * (AttachOperation::ARG_LENGTH_MAX + 1);
            min_str_count = 1 + AttachOperation::ARG_COUNT_MAX;
            min_read_size = 1;
        }
        2 => {
            if AttachListener::get_supported_version() < 2 {
                eprintln!("Failed to read request: v2 is unsupported or disabled");
                if let Some(w) = err.as_deref_mut() {
                    let _ = w.write_reply_msg(ATTACH_ERROR_BADVERSION, b"v2 is unsupported or disabled");
                }
                return false;
            }
            let size = read_uint(reader, false);
            if size < 0 {
                eprintln!("Failed to read request: negative request size");
                return false;
            }
            buffer_size = size as usize;
            if buffer_size > 256 * 1024 {
                eprintln!("Failed to read request: too big");
                return false;
            }
            min_str_count = 0;
            min_read_size = buffer_size;
        }
        _ => {
            eprintln!("Failed to read request: unknown version {}", ver);
            if let Some(w) = err.as_deref_mut() {
                let _ = w.write_reply_msg(ATTACH_ERROR_BADVERSION, b"unknown version");
            }
            return false;
        }
    }

    let result = read_request_data(reader, op, buffer_size, min_str_count, min_read_size);
    if result && ver == 1 {
        if op.name().len() > AttachOperation::NAME_LENGTH_MAX {
            eprintln!("Failed to read request: operation name is too long");
            return false;
        }
        for i in 0..op.arg_count() {
            if op.arg(i).map(|a| a.len()).unwrap_or(0) > AttachOperation::ARG_LENGTH_MAX {
                eprintln!("Failed to read request: operation argument is too long");
                return false;
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Windows named pipe transport (PipeChannel / Win32AttachOperation)
// ---------------------------------------------------------------------------

type WinHandle = isize;
const INVALID_HANDLE_VALUE: WinHandle = -1;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        lp_file_name: *const u16,
        dw_desired_access: u32,
        dw_share_mode: u32,
        lp_security_attributes: *mut c_void,
        dw_creation_disposition: u32,
        dw_flags_and_attributes: u32,
        h_template_file: WinHandle,
    ) -> WinHandle;
    fn ReadFile(
        h_file: WinHandle,
        lp_buffer: *mut u8,
        n_number_of_bytes_to_read: u32,
        lp_number_of_bytes_read: *mut u32,
        lp_overlapped: *mut c_void,
    ) -> i32;
    fn WriteFile(
        h_file: WinHandle,
        lp_buffer: *const u8,
        n_number_of_bytes_to_write: u32,
        lp_number_of_bytes_written: *mut u32,
        lp_overlapped: *mut c_void,
    ) -> i32;
    fn FlushFileBuffers(h_file: WinHandle) -> i32;
    fn CloseHandle(h_object: WinHandle) -> i32;
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct PipeChannel {
    handle: WinHandle,
}

impl PipeChannel {
    pub fn new() -> Self {
        Self {
            handle: INVALID_HANDLE_VALUE,
        }
    }

    pub fn opened(&self) -> bool {
        self.handle != INVALID_HANDLE_VALUE
    }

    pub fn open(&mut self, pipe: &str, write_only: bool) -> bool {
        let access = GENERIC_WRITE | if write_only { 0 } else { GENERIC_READ };
        let wide = to_wide(pipe);
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                0,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                0,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            eprintln!("could not open pipe {}", pipe);
            return false;
        }
        self.handle = h;
        true
    }

    pub fn close(&mut self) {
        if self.opened() {
            unsafe {
                FlushFileBuffers(self.handle);
                CloseHandle(self.handle);
            }
            self.handle = INVALID_HANDLE_VALUE;
        }
    }
}

impl Drop for PipeChannel {
    fn drop(&mut self) {
        self.close();
    }
}

impl RequestReader for PipeChannel {
    fn read(&mut self, buf: &mut [u8]) -> isize {
        let mut nread: u32 = 0;
        let ok = unsafe {
            ReadFile(
                self.handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut nread,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            eprintln!("pipe read error");
            return -1;
        }
        nread as isize
    }
}

impl ReplyWriter for PipeChannel {
    fn write(&mut self, buf: &[u8]) -> isize {
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                self.handle,
                buf.as_ptr(),
                buf.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            eprintln!("pipe write error");
            return -1;
        }
        written as isize
    }
}

pub struct Win32AttachOperation {
    pub op: AttachOperation,
    pipe: Option<PipeChannel>,
}

impl Win32AttachOperation {
    pub const PIPE_NAME_MAX: usize = 256;

    pub fn new() -> Self {
        Self {
            op: AttachOperation::new(),
            pipe: None,
        }
    }

    fn open_pipe(&mut self, pipe_name: &str, write_only: bool) -> bool {
        let mut ch = PipeChannel::new();
        if !ch.open(pipe_name, write_only) {
            return false;
        }
        self.pipe = Some(ch);
        true
    }

    pub fn reply_writer(&mut self) -> Option<&mut dyn ReplyWriter> {
        self.pipe.as_mut().map(|p| p as &mut dyn ReplyWriter)
    }

    pub fn read_request(&mut self) -> bool {
        let mut op = AttachOperation::new();
        let result = {
            let reader = self.pipe.as_mut().unwrap() as &mut dyn RequestReader;
            read_request(reader, None, &mut op)
        };
        if result {
            self.op = op;
        }
        result
    }

    pub fn complete(&mut self, result: i32, payload: &[u8]) {
        if let Some(pipe) = self.pipe.as_mut() {
            let _ = pipe.write_reply_stream(result, payload);
        }
    }
}

// ---------------------------------------------------------------------------
// Win32AttachListener — operation queue (mutex + semaphore + request pool)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Win32AttachOperationRequest {
    ver: AttachApiVersion,
    name: String,
    args: [String; 3],
    pipe: String,
}

impl Win32AttachOperationRequest {
    fn new() -> Self {
        Self {
            ver: AttachApiVersion::V1,
            name: String::new(),
            args: [String::new(), String::new(), String::new()],
            pipe: "<nopipe>".to_string(),
        }
    }

    fn set(&mut self, ver: AttachApiVersion, pipe: &str, cmd: &str, args: [&str; 3]) {
        self.ver = ver;
        self.name = cmd.to_string();
        self.args = [args[0].to_string(), args[1].to_string(), args[2].to_string()];
        self.pipe = pipe.to_string();
    }
}

struct Win32State {
    avail: VecDeque<Win32AttachOperationRequest>,
    head: VecDeque<Win32AttachOperationRequest>,
    shutdown: bool,
}

const MAX_ENQUEUED_OPERATIONS: usize = 4;

struct Win32AttachListener {
    state: Mutex<Win32State>,
    condvar: Condvar,
}

impl Win32AttachListener {
    fn new() -> Self {
        let mut avail = VecDeque::new();
        for _ in 0..MAX_ENQUEUED_OPERATIONS {
            avail.push_back(Win32AttachOperationRequest::new());
        }
        Self {
            state: Mutex::new(Win32State {
                avail,
                head: VecDeque::new(),
                shutdown: false,
            }),
            condvar: Condvar::new(),
        }
    }

    fn init(&self) {
        // queue is preallocated in new(); version is set by pd_init caller
    }

    fn enqueue(
        &self,
        ver: AttachApiVersion,
        cmd: &str,
        arg0: &str,
        arg1: &str,
        arg2: &str,
        pipename: &str,
    ) -> i32 {
        let mut sleep_count = 0;
        while !AttachListener::is_initialized() {
            std::thread::sleep(std::time::Duration::from_secs(1));
            sleep_count += 1;
            if sleep_count > 10 {
                return ATTACH_ERROR_DISABLED;
            }
        }

        if cmd.len() > AttachOperation::NAME_LENGTH_MAX
            || arg0.len() > AttachOperation::ARG_LENGTH_MAX
            || arg1.len() > AttachOperation::ARG_LENGTH_MAX
            || arg2.len() > AttachOperation::ARG_LENGTH_MAX
            || pipename.len() > Win32AttachOperation::PIPE_NAME_MAX
        {
            return ATTACH_ERROR_ILLEGALARG;
        }
        if !pipename.starts_with("\\\\.\\pipe\\") {
            return ATTACH_ERROR_ILLEGALARG;
        }

        let mut st = self.state.lock().unwrap();
        if let Some(mut req) = st.avail.pop_front() {
            req.set(ver, pipename, cmd, [arg0, arg1, arg2]);
            st.head.push_back(req);
            self.condvar.notify_one();
            JNI_OK
        } else {
            ATTACH_ERROR_RESOURCE
        }
    }

    fn dequeue(&self) -> Option<Win32AttachOperation> {
        loop {
            let req = {
                let mut st = self.state.lock().unwrap();
                loop {
                    if st.shutdown {
                        return None;
                    }
                    if let Some(req) = st.head.pop_front() {
                        break req;
                    }
                    st = self.condvar.wait(st).unwrap();
                }
            };

            let mut op = Win32AttachOperation::new();
            let ok = match req.ver {
                AttachApiVersion::V1 => {
                    op.op.set_name(&req.name);
                    for i in 0..AttachOperation::ARG_COUNT_MAX {
                        op.op.append_arg(&req.args[i]);
                    }
                    op.open_pipe(&req.pipe, true)
                }
                AttachApiVersion::V2 => {
                    op.open_pipe(&req.pipe, false) && op.read_request()
                }
            };

            let mut st = self.state.lock().unwrap();
            st.avail.push_back(req);
            drop(st);

            if ok {
                return Some(op);
            }
        }
    }

    fn shutdown(&self) {
        let mut st = self.state.lock().unwrap();
        st.shutdown = true;
        self.condvar.notify_all();
    }
}

// ---------------------------------------------------------------------------
// AttachListener — AllStatic equivalent
// ---------------------------------------------------------------------------

static STATE: AtomicU8 = AtomicU8::new(AttachListenerState::NotInitialized as u8);
static SUPPORTED_VERSION: AtomicI32 = AtomicI32::new(1);
static DEFAULT_STREAMING: AtomicU8 = AtomicU8::new(1);
static WIN32: std::sync::OnceLock<Win32AttachListener> = std::sync::OnceLock::new();
static INITIALIZED_FLAG: AtomicU8 = AtomicU8::new(0);

fn win32() -> &'static Win32AttachListener {
    WIN32.get_or_init(|| Win32AttachListener::new())
}

pub struct AttachListener;

impl AttachListener {
    pub fn vm_start() {}

    pub fn state() -> AttachListenerState {
        match STATE.load(Ordering::SeqCst) {
            1 => AttachListenerState::Initializing,
            2 => AttachListenerState::Initialized,
            _ => AttachListenerState::NotInitialized,
        }
    }

    pub fn set_state(s: AttachListenerState) {
        STATE.store(s as u8, Ordering::SeqCst);
    }

    pub fn set_initialized() {
        INITIALIZED_FLAG.store(1, Ordering::SeqCst);
        Self::set_state(AttachListenerState::Initialized);
    }

    pub fn is_initialized() -> bool {
        INITIALIZED_FLAG.load(Ordering::SeqCst) != 0
    }

    pub fn get_supported_version() -> i32 {
        SUPPORTED_VERSION.load(Ordering::SeqCst)
    }

    pub fn set_supported_version(version: AttachApiVersion) {
        let compat = Management::global()
            .runtime_mxbean()
            .system_properties()
            .get("jdk.attach.compat")
            .map(|v| v == "true")
            .unwrap_or(false);
        if !compat {
            SUPPORTED_VERSION.store(version as i32, Ordering::SeqCst);
        }
    }

    pub fn get_default_streaming() -> bool {
        DEFAULT_STREAMING.load(Ordering::SeqCst) != 0
    }

    pub fn set_default_streaming(b: bool) {
        DEFAULT_STREAMING.store(b as u8, Ordering::SeqCst);
    }

    pub fn check_socket_file() -> bool {
        false
    }

    pub fn init_at_startup() -> bool {
        true
    }

    pub fn is_init_trigger() -> bool {
        false
    }

    pub fn abort() {}

    pub fn detachall() {}

    pub fn pd_init() -> i32 {
        win32().init();
        Self::set_supported_version(AttachApiVersion::V2);
        0
    }

    pub fn pd_detachall() {}

    pub fn pd_data_dump() {
        eprintln!("data dump requested (SIGBREAK transport not available)");
    }

    pub fn dequeue() -> Option<Win32AttachOperation> {
        win32().dequeue()
    }

    pub fn enqueue(
        ver: AttachApiVersion,
        cmd: &str,
        arg0: &str,
        arg1: &str,
        arg2: &str,
        pipename: &str,
    ) -> i32 {
        win32().enqueue(ver, cmd, arg0, arg1, arg2, pipename)
    }

    pub fn shutdown() {
        win32().shutdown();
    }

    pub fn init() {
        if Self::state() != AttachListenerState::NotInitialized {
            return;
        }
        Self::set_state(AttachListenerState::Initializing);
        std::thread::Builder::new()
            .name("Attach Listener".to_string())
            .spawn(thread_entry)
            .expect("Attach Listener thread creation failed");
    }
}

fn thread_entry() {
    crate::share::memory::heap::set_gc_exempt();
    let streaming = Management::global()
        .runtime_mxbean()
        .system_properties()
        .get("jdk.attach.vm.streaming")
        .map(|v| v != "false")
        .unwrap_or(true);
    AttachListener::set_default_streaming(streaming);

    if AttachListener::pd_init() != 0 {
        AttachListener::set_state(AttachListenerState::NotInitialized);
        return;
    }
    AttachListener::set_initialized();

    loop {
        let mut op = match AttachListener::dequeue() {
            Some(op) => op,
            None => {
                AttachListener::set_state(AttachListenerState::NotInitialized);
                return;
            }
        };

        let allow_streaming = op.op.streaming_output();
        let mut st = {
            let writer: Option<&mut dyn ReplyWriter> = op
                .pipe
                .as_mut()
                .map(|p| p as &mut dyn ReplyWriter);
            AttachStream::new(writer, allow_streaming)
        };

        if op.op.name() == AttachOperation::detachall_operation_name() {
            AttachListener::detachall();
            st.set_result(JNI_OK);
        } else {
            let func = find_operation(op.op.name());
            match func {
                Some(f) => {
                    let result = f(&mut op.op, &mut st);
                    st.set_result(result);
                }
                None => {
                    st.set_result(JNI_ERR);
                    let _ = writeln!(st, "Operation {} not recognized!", op.op.name());
                }
            }
        }
        st.complete();
        let result = st.get_result();
        let payload = st.buffered().to_vec();
        drop(st);
        op.complete(result, &payload);
    }
}

// ---------------------------------------------------------------------------
// Attach operation functions
// ---------------------------------------------------------------------------

type AttachOperationFunction = fn(&mut AttachOperation, &mut AttachStream) -> i32;

fn find_operation(name: &str) -> Option<AttachOperationFunction> {
    Some(match name {
        "agentProperties" => get_agent_properties,
        "datadump" => data_dump,
        "dumpheap" => dump_heap,
        "load" => load_agent,
        "properties" => get_system_properties,
        "threaddump" => thread_dump,
        "inspectheap" => heap_inspection,
        "setflag" => set_flag,
        "printflag" => print_flag,
        "jcmd" => jcmd,
        "getversion" => get_version,
        _ => return None,
    })
}

fn write_props(out: &mut AttachStream) {
    let props = Management::global().runtime_mxbean().system_properties();
    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    for k in keys {
        let _ = writeln!(out, "{}={}", k, props.get(k).map(|s| s.as_str()).unwrap_or(""));
    }
}

fn get_system_properties(_op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    write_props(out);
    JNI_OK
}

fn get_agent_properties(_op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    write_props(out);
    JNI_OK
}

fn data_dump(_op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    out.set_result(JNI_OK);
    AttachListener::pd_data_dump();
    JNI_OK
}

fn thread_dump(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let arg0 = op.arg(0).unwrap_or("");
    let print_concurrent_locks = arg0.contains('l');
    let print_extended_info = arg0.contains('e');
    out.set_result(JNI_OK);
    print_thread_dump(out, print_concurrent_locks, print_extended_info);
    JNI_OK
}

fn jcmd(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let allow_streaming = !op
        .arg(0)
        .map(|a| a.starts_with("ManagementAgent."))
        .unwrap_or(false);

    if allow_streaming {
        out.set_result(JNI_OK);
    }
    crate::share::services::diagnosticCommand::ensure_registered();
    let line = op.arg(0).unwrap_or("").to_string();
    let result = {
        let mut executor = Executor::new(DCmdSource::AttachApi, out);
        executor.parse_and_execute(&line, ' ')
    };
    match result {
        Ok(()) => JNI_OK,
        Err(e) => {
            let _ = writeln!(out, "{}", e);
            JNI_ERR
        }
    }
}

fn parse_uint_arg(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok()
}

fn dump_heap(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let path = op.arg(0).unwrap_or("");
    if path.is_empty() {
        let _ = writeln!(out, "No dump file specified");
        return JNI_OK;
    }
    let mut live_objects_only = true;
    if let Some(arg1) = op.arg(1) {
        if !arg1.is_empty() {
            if arg1 != "-all" && arg1 != "-live" {
                let _ = writeln!(out, "Invalid argument to dumpheap operation: {}", arg1);
                return JNI_ERR;
            }
            live_objects_only = arg1 == "-live";
        }
    }

    let mut level: i32 = 0;
    if let Some(num_str) = op.arg(2) {
        if !num_str.is_empty() {
            match num_str.parse::<u32>() {
                Ok(v) => level = v as i32,
                Err(_) => {
                    let _ = writeln!(out, "Invalid compress level: [{}]", num_str);
                    return JNI_ERR;
                }
            }
            if level < 1 || level > 9 {
                let _ = writeln!(out, "Compression level out of range (1-9): {}", level);
                return JNI_ERR;
            }
        }
    }

    out.set_result(JNI_OK);
    let dumper = HeapDumper::with_gc_request(live_objects_only);
    let parallel = (crate::share::runtime::os::Os::active_processor_count() as u32)
        .saturating_mul(3)
        / 8;
    match dumper.dump(path, level, false, parallel.max(1)) {
        Ok(_n) => JNI_OK,
        Err(e) => {
            let _ = writeln!(out, "{}", e);
            JNI_ERR
        }
    }
}

fn heap_inspection(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let arg0 = op.arg(0).unwrap_or("");
    let mut live_objects_only = true;
    if !arg0.is_empty() {
        if arg0 != "-all" && arg0 != "-live" {
            let _ = writeln!(out, "Invalid argument to inspectheap operation: {}", arg0);
            return JNI_ERR;
        }
        live_objects_only = arg0 == "-live";
    }

    let path = op.arg(1).unwrap_or("");
    let _parallel = op
        .arg(2)
        .and_then(parse_uint_arg)
        .unwrap_or_else(|| {
            (crate::share::runtime::os::Os::active_processor_count() as u32)
                .saturating_mul(3)
                / 8
        });

    out.set_result(JNI_OK);
    let _ = live_objects_only;
    let _ = path;
    let _ = writeln!(out, "Heap inspection is not available in the Rust port");
    JNI_ERR
}

fn set_flag(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let name = match op.arg(0) {
        Some(n) if !n.is_empty() => n,
        _ => {
            let _ = writeln!(out, "flag name is missing");
            return JNI_ERR;
        }
    };
    let value = op.arg(1);
    let (err, msg) = WriteableFlags::set_flag(
        Some(name),
        value,
        crate::share::services::writeableFlags::JvmFlagOrigin::AttachOnDemand,
    );
    match err {
        crate::share::services::writeableFlags::JvmFlagError::Success => JNI_OK,
        crate::share::services::writeableFlags::JvmFlagError::NonWritable => {
            let _ = writeln!(out, "flag '{}' cannot be changed", name);
            JNI_ERR
        }
        _ => {
            let _ = writeln!(out, "{}", msg);
            JNI_ERR
        }
    }
}

fn print_flag(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let name = match op.arg(0) {
        Some(n) if !n.is_empty() => n,
        _ => {
            let _ = writeln!(out, "flag name is missing");
            return JNI_ERR;
        }
    };
    match crate::share::services::writeableFlags::JvmFlag::find_flag(name) {
        Some(f) => {
            let _ = writeln!(out, "{}", f.print_as_flag());
            JNI_OK
        }
        None => {
            let _ = writeln!(out, "no such flag '{}'", name);
            JNI_ERR
        }
    }
}

fn get_version(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let _ = writeln!(out, "{}", AttachListener::get_supported_version());
    if op.arg(0) == Some("options") {
        let _ = write!(out, " streaming");
    }
    JNI_OK
}

fn load_agent(op: &mut AttachOperation, out: &mut AttachStream) -> i32 {
    let agent = op.arg(0).unwrap_or("");
    let abs = op.arg(1).unwrap_or("");
    let options = op.arg(2).unwrap_or("");
    let _ = abs;
    let _ = options;
    let _ = writeln!(
        out,
        "Loading agent '{}' is not supported (requires JVMTI/management agent)",
        agent
    );
    JNI_ERR
}

// ---------------------------------------------------------------------------
// Exports — JVM_EnqueueOperation / JVM_EnqueueOperation_v2
// ---------------------------------------------------------------------------

fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn JVM_EnqueueOperation(
    cmd: *const c_char,
    arg0: *const c_char,
    arg1: *const c_char,
    arg2: *const c_char,
    pipename: *const c_char,
) -> i32 {
    AttachListener::enqueue(
        AttachApiVersion::V1,
        &cstr_to_string(cmd),
        &cstr_to_string(arg0),
        &cstr_to_string(arg1),
        &cstr_to_string(arg2),
        &cstr_to_string(pipename),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn JVM_EnqueueOperation_v2(pipename: *const c_char) -> i32 {
    AttachListener::enqueue(
        AttachApiVersion::V2,
        "",
        "",
        "",
        "",
        &cstr_to_string(pipename),
    )
}
