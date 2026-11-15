//! Native methods behind `sun.nio.fs.WindowsNativeDispatcher`,
//! `sun.nio.ch.FileDispatcherImpl` and `sun.nio.ch.IOUtil`.
//!
//! File handles are opaque u64 values allocated from a global table; the
//! `java.io.FileDescriptor.handle` field carries them through Java code.
//! Buffer addresses passed to `FileDispatcherImpl` point into
//! `OffHeapMemory` (direct buffers allocated via `Unsafe`).

use super::offHeapMemory::OffHeapMemory;
use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Seek, SeekFrom, Write},
    sync::{Condvar, Mutex, OnceLock},
};
#[cfg(windows)]
use std::os::windows::fs::FileExt as _;

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

pub(crate) fn io_error(error: io::Error) -> io::Error {
    io::Error::other(format!("java.io.IOException: {}", error))
}

use crate::share::interpreter::bytecodeInterpreter::JavaThrowable;
use crate::share::oops::instanceOop::InstanceOop;

/// Builds a real throwable object without running its constructor.
fn throwable<'a>(
    loader: &ClassLoader<'a>,
    class_name: &[u8],
    message: &str,
) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
        loader,
        &klass,
        &mut std::io::sink(),
        0,
    );
    let object = InstanceOop::new(klass.clone())?;
    let handle = loader.alloc_object(object);
    if let Ok(slot) = klass.instanceField(b"detailMessage", b"Ljava/lang/String;") {
        let mut objects = loader.borrow_objects_mut();
        if let Some(object) = objects.get_mut(handle) {
            object.fields[slot] = Value::HostString(loader.storage.alloc_str(message));
        }
    }
    Ok(handle)
}

fn throw_java<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: &str) -> io::Error {
    match throwable(loader, class_name, message) {
        Ok(handle) => io::Error::other(JavaThrowable(handle)),
        Err(_) => io::Error::other(format!(
            "{}: {}",
            String::from_utf8_lossy(class_name).replace('/', "."),
            message
        )),
    }
}

fn io_exception<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/io/IOException", message)
}

/// `sun.nio.fs.WindowsException` carrying a Win32 error code in `lastError`.
fn windows_exception<'a>(
    loader: &ClassLoader<'a>,
    code: i32,
    message: &str,
) -> io::Error {
    let class_name = b"sun/nio/fs/WindowsException";
    if code == 2 || code == 3 {
        eprintln!("[WindowsException] code={code} msg={message}");
        eprintln!("{}", std::backtrace::Backtrace::force_capture());
    }
    match throwable(loader, class_name, message) {
        Ok(handle) => {
            if let Ok(klass) = loader.load(class_name)
                && let Ok(slot) = klass.instanceField(b"lastError", b"I")
            {
                let mut objects = loader.borrow_objects_mut();
                if let Some(object) = objects.get_mut(handle)
                    && slot < object.fields.len()
                {
                    object.fields[slot] = Value::Int(code);
                }
            }
            io::Error::other(JavaThrowable(handle))
        }
        Err(_) => throw_java(loader, class_name, message),
    }
}

/// Maps an `io::Error` to a `WindowsException` with the closest Win32 code.
fn win32_error<'a>(loader: &ClassLoader<'a>, error: &io::Error) -> io::Error {
    use io::ErrorKind::*;
    let code = match error.kind() {
        NotFound => 2,          // ERROR_FILE_NOT_FOUND
        PermissionDenied => 5,  // ERROR_ACCESS_DENIED
        AlreadyExists => 80,    // ERROR_ALREADY_EXISTS
        _ => 1,                 // ERROR_INVALID_FUNCTION
    };
    eprintln!("[win32_error] code={code} error={error}");
    windows_exception(loader, code, &error.to_string())
}

pub(crate) struct Entry {
    pub(crate) kind: EntryKind,
    pub(crate) pos: u64,
    pub(crate) path: String,
}

#[allow(dead_code)]
pub(crate) enum EntryKind {
    File(std::fs::File),
    ReadEnd(std::fs::File),
    WriteEnd(std::fs::File),
    Other,
}

pub(crate) struct Table {
    entries: HashMap<u64, Entry>,
    next: u64,
}

pub(crate) fn table() -> &'static Mutex<Table> {
    static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(Table {
            entries: HashMap::new(),
            next: 0x1000,
        })
    })
}

pub(crate) fn alloc_handle(kind: EntryKind, path: String) -> u64 {
    let mut table = table().lock().unwrap();
    let handle = table.next;
    table.next += 0x10;
    table.entries.insert(handle, Entry { kind, pos: 0, path });
    handle
}

pub(crate) fn free_handle(handle: u64) -> Option<Entry> {
    table().lock().unwrap().entries.remove(&handle)
}

/// Path recorded for a registered handle (diagnostics).
pub(crate) fn entry_path(handle: u64) -> Option<String> {
    table().lock().unwrap().entries.get(&handle).map(|e| e.path.clone())
}

pub(crate) fn with_entry<R>(handle: u64, f: impl FnOnce(&mut Entry) -> io::Result<R>) -> io::Result<R> {
    // Keep memory-mapped regions coherent: pending mapped writes are
    // flushed before any other I/O through this descriptor.
    flush_maps_for_fd(handle);
    let mut table = table().lock().unwrap();
    let entry = table
        .entries
        .get_mut(&handle)
        .ok_or_else(|| invalid("Invalid file handle"))?;
    f(entry)
}

pub(crate) fn file_of(entry: &mut Entry) -> io::Result<&mut std::fs::File> {
    match &mut entry.kind {
        EntryKind::File(file) => Ok(file),
        _ => Err(invalid("Handle is not a file")),
    }
}

// ---------------------------------------------------------------------------
// File mappings (`FileDispatcherImpl.map0`/`unmap0`).
//
// Real mmap is not available through std; instead the mapped region is copied
// into `OffHeapMemory` and tracked here. Writes through the MappedByteBuffer
// land in that copy; writable mappings are flushed back to the file on
// `unmap0` and before any other I/O on the same descriptor (keeps
// FileChannel.read-after-map-write coherent).
// ---------------------------------------------------------------------------

struct FileMap {
    fd: u64,
    path: String,
    file_pos: u64,
    len: u64,
    writable: bool,
}

fn file_maps() -> &'static Mutex<HashMap<u64, FileMap>> {
    static MAPS: OnceLock<Mutex<HashMap<u64, FileMap>>> = OnceLock::new();
    MAPS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Writes back every writable mapping of `fd`, if any.
pub(crate) fn flush_maps_for_fd(fd: u64) {
    let pending: Vec<(u64, FileMap)> = {
        let maps = file_maps().lock().unwrap();
        maps.iter()
            .filter(|(_, m)| m.fd == fd && m.writable)
            .map(|(&addr, m)| {
                (
                    addr,
                    FileMap {
                        fd: m.fd,
                        path: m.path.clone(),
                        file_pos: m.file_pos,
                        len: m.len,
                        writable: m.writable,
                    },
                )
            })
            .collect()
    };
    for (addr, m) in pending {
        flush_map(addr, &m);
    }
}

fn flush_map(addr: u64, m: &FileMap) {
    let Ok(data) = OffHeapMemory::global().read_bytes(addr, m.len as usize) else {
        return;
    };
    let _ = write_file_at(&m.path, m.file_pos, &data);
}

fn write_file_at(path: &str, pos: u64, data: &[u8]) -> io::Result<()> {
    use std::io::Seek;
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(pos))?;
    file.write_all(data)
}

// ---------------------------------------------------------------------------
// Directory iteration (FindFirstFile*/FindNextFile*/FindClose)
// ---------------------------------------------------------------------------

struct DirEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<std::time::SystemTime>,
    attrs: u32,
}

struct DirState {
    entries: Vec<DirEntry>,
    index: usize,
}

fn dir_table() -> &'static Mutex<HashMap<u64, DirState>> {
    static TABLE: OnceLock<Mutex<HashMap<u64, DirState>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn filetime(time: std::time::SystemTime) -> i64 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => {
            (duration.as_secs() * 10_000_000 + duration.subsec_nanos() as u64 / 100) as i64
                + 11_644_473_600_000_000
        }
        Err(_) => 0,
    }
}

/// Simple `*`/`?` wildcard match on a single path component.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ni;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Splits a Windows search path into (directory, file-name pattern).
/// A trailing `\*` or bare `*` is treated as "all entries".
fn split_search_path(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches(['\\', '/']);
    if let Some(index) = trimmed.rfind(['\\', '/']) {
        let (dir, pat) = trimmed.split_at(index);
        let pat = &pat[1..];
        let mut dir = dir.to_string();
        // `C:\foo` splits into dir `C:` which is not a listable path;
        // the drive root must keep its trailing separator (`C:\`).
        if dir.len() == 2 && dir.ends_with(':') {
            dir.push('\\');
        }
        if pat.is_empty() {
            return (dir, "*".to_string());
        }
        (dir, pat.to_string())
    } else if trimmed.len() == 2 && trimmed.ends_with(':') {
        (format!("{}\\", trimmed), "*".to_string())
    } else {
        (String::new(), "*".to_string())
    }
}

/// Reads a directory and returns snapshot entries matching `pattern`.
fn scan_directory(dir: &str, pattern: &str) -> io::Result<Vec<DirEntry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." {
            continue;
        }
        if !wildcard_match(pattern, &name) {
            continue;
        }
        let meta = item.metadata()?;
        let is_dir = meta.is_dir();
        let attrs = if is_dir { 0x10u32 } else { 0x80 | 0x20 };
        entries.push(DirEntry {
            name,
            is_dir,
            size: meta.len(),
            modified: meta.modified().ok(),
            attrs,
        });
    }
    Ok(entries)
}

/// Opens a directory search and returns a handle plus the first match.
fn dir_open(dir_path: &str, pattern: &str) -> io::Result<(u64, Option<DirEntry>)> {
    let mut entries = scan_directory(dir_path, pattern)?;
    let first = if entries.is_empty() { None } else { Some(entries.remove(0)) };
    let mut table = dir_table().lock().unwrap();
    let mut next = 0x8000_0000u64;
    while table.contains_key(&next) {
        next += 0x10;
    }
    table.insert(next, DirState { entries, index: 0 });
    Ok((next, first))
}

/// Returns the next matching entry for an open search handle.
fn dir_next(handle: u64) -> io::Result<Option<DirEntry>> {
    let mut table = dir_table().lock().unwrap();
    let state = table
        .get_mut(&handle)
        .ok_or_else(|| invalid("Invalid directory handle"))?;
    if state.index >= state.entries.len() {
        return Ok(None);
    }
    let entry = state.entries[state.index].clone_name_only();
    state.index += 1;
    Ok(Some(entry))
}

impl DirEntry {
    fn clone_name_only(&self) -> DirEntry {
        DirEntry {
            name: self.name.clone(),
            is_dir: self.is_dir,
            size: self.size,
            modified: self.modified,
            attrs: self.attrs,
        }
    }
}

/// Serializes a `WIN32_FIND_DATAW` (592 bytes) into off-heap memory.
fn write_find_data(address: i64, entry: &DirEntry) -> io::Result<()> {
    let mut data = vec![0u8; 592];
    data[0..4].copy_from_slice(&entry.attrs.to_le_bytes());
    if let Some(modified) = entry.modified {
        let ft = filetime(modified);
        data[20..28].copy_from_slice(&ft.to_le_bytes()); // ftLastWriteTime
    }
    data[28..32].copy_from_slice(&((entry.size >> 32) as u32).to_le_bytes());
    data[32..36].copy_from_slice(&(entry.size as u32).to_le_bytes());
    let name: Vec<u16> = entry.name.encode_utf16().collect();
    for (index, unit) in name.iter().take(259).enumerate() {
        let offset = 44 + index * 2;
        data[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
    }
    write_buffer(address, &data)
}

/// Sets `handle`/`name`/`attributes` on a `FirstFile`/`FirstStream` object.
fn set_first_file_fields<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    handle: u64,
    name: &str,
    attributes: u32,
) -> io::Result<()> {
    let Value::Object(object_handle) = value else {
        return Err(invalid("Expected FirstFile object"));
    };
    let updates = {
        let objects = loader.borrow_objects();
        let object = objects
            .get(object_handle)
            .ok_or_else(|| invalid("Invalid FirstFile object"))?;
        let klass = &object.klass;
        vec![
            (klass.instanceField(b"handle", b"J")?, Value::Long(handle as i64)),
            (
                klass.instanceField(b"name", b"Ljava/lang/String;")?,
                Value::HostString(loader.storage.alloc_str(name)),
            ),
            (klass.instanceField(b"attributes", b"I")?, Value::Int(attributes as i32)),
        ]
    };
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(object_handle)
        .ok_or_else(|| invalid("Invalid FirstFile object"))?;
    for (slot, value) in updates {
        if slot < object.fields.len() {
            object.fields[slot] = value;
        }
    }
    Ok(())
}

/// Reads a NUL-terminated UTF-16LE path from off-heap memory.
fn read_wide_units(address: i64) -> io::Result<Vec<u16>> {
    let base = u64::try_from(address).map_err(|_| invalid("Invalid path address"))?;
    let memory = OffHeapMemory::global();
    let mut units = Vec::new();
    let mut offset = 0u64;
    loop {
        let lo = memory.get_byte(base + offset)? as u8;
        let hi = memory.get_byte(base + offset + 1)? as u8;
        let unit = u16::from_le_bytes([lo, hi]);
        offset += 2;
        if unit == 0 {
            break;
        }
        units.push(unit);
        if units.len() > 32768 {
            return Err(invalid("Path too long"));
        }
    }
    Ok(units)
}

fn read_wide_path(address: i64) -> io::Result<String> {
    let units = read_wide_units(address)?;
    Ok(String::from_utf16_lossy(&units))
}

fn java_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.replace('\\', "/")
    }
}

/// Fills `bytes` with operating-system entropy (BCryptGenRandom on Windows,
/// /dev/urandom elsewhere).
#[cfg(windows)]
fn fill_random(bytes: &mut [u8]) -> bool {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut core::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    if bytes.is_empty() {
        return true;
    }
    let mut offset = 0usize;
    while offset < bytes.len() {
        let chunk = (bytes.len() - offset).min(u32::MAX as usize) as u32;
        let status = unsafe {
            BCryptGenRandom(
                core::ptr::null_mut(),
                bytes[offset..].as_mut_ptr(),
                chunk,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return false;
        }
        offset += chunk as usize;
    }
    true
}

#[cfg(not(windows))]
fn fill_random(bytes: &mut [u8]) -> bool {
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .is_ok()
}

// ---------------------------------------------------------------------------
// I/O completion ports (CreateIoCompletionPort / GetQueuedCompletionStatus0 /
// PostQueuedCompletionStatus)
// ---------------------------------------------------------------------------

struct CompletionEntry {
    bytes: u32,
    key: i64,
}

struct IocpState {
    queue: VecDeque<CompletionEntry>,
    closed: bool,
}

fn iocp_sync() -> &'static (Mutex<HashMap<u64, IocpState>>, Condvar) {
    static IOCP: OnceLock<(Mutex<HashMap<u64, IocpState>>, Condvar)> = OnceLock::new();
    IOCP.get_or_init(|| (Mutex::new(HashMap::new()), Condvar::new()))
}

fn iocp_alloc() -> u64 {
    static NEXT: Mutex<u64> = Mutex::new(0x4000_0000);
    let mut next = NEXT.lock().unwrap();
    let handle = *next;
    *next += 0x10;
    handle
}

/// Writes `error`/`bytesTransferred`/`completionKey` onto a
/// `WindowsNativeDispatcher$CompletionStatus` object.
fn set_completion_fields<'a>(
    loader: &ClassLoader<'a>,
    value: Value<'a>,
    error: i32,
    bytes: i32,
    key: i64,
) -> io::Result<()> {
    let Value::Object(object_handle) = value else {
        return Err(invalid("Expected CompletionStatus object"));
    };
    let updates = {
        let objects = loader.borrow_objects();
        let object = objects
            .get(object_handle)
            .ok_or_else(|| invalid("Invalid CompletionStatus object"))?;
        let klass = &object.klass;
        vec![
            (klass.instanceField(b"error", b"I")?, Value::Int(error)),
            (
                klass.instanceField(b"bytesTransferred", b"I")?,
                Value::Int(bytes),
            ),
            (
                klass.instanceField(b"completionKey", b"J")?,
                Value::Long(key),
            ),
        ]
    };
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(object_handle)
        .ok_or_else(|| invalid("Invalid CompletionStatus object"))?;
    for (slot, value) in updates {
        if slot < object.fields.len() {
            object.fields[slot] = value;
        }
    }
    Ok(())
}

/// Reads `count` bytes from off-heap memory into a Vec.
fn read_buffer(address: i64, count: usize) -> io::Result<Vec<u8>> {
    let base = u64::try_from(address).map_err(|_| invalid("Invalid buffer address"))?;
    let memory = OffHeapMemory::global();
    let mut data = vec![0u8; count];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = memory.get_byte(base + index as u64)? as u8;
    }
    Ok(data)
}

/// Writes bytes into off-heap memory.
fn copy_dir_all(source: &str, dest: &str) -> io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = std::path::Path::new(dest).join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from.to_string_lossy(), &to.to_string_lossy())?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn write_buffer(address: i64, data: &[u8]) -> io::Result<()> {
    let base = u64::try_from(address).map_err(|_| invalid("Invalid buffer address"))?;
    let memory = OffHeapMemory::global();
    for (index, &byte) in data.iter().enumerate() {
        memory.put_byte(base + index as u64, byte as i8)?;
    }
    Ok(())
}

pub(crate) fn file_descriptor_handle<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<u64> {
    let Value::Object(handle) = value else {
        return Err(invalid("Expected FileDescriptor"));
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid FileDescriptor"))?;
    let slot = object
        .klass
        .instanceField(b"handle", b"J")
        .map_err(|_| invalid("FileDescriptor has no handle field"))?;
    match object.fields.get(slot).copied().unwrap_or(Value::Null) {
        Value::Long(handle) => Ok(handle as u64),
        Value::Int(handle) => Ok(handle as u32 as u64),
        _ => Err(invalid("FileDescriptor handle is not set")),
    }
}

/// `sun.nio.fs.WindowsNativeDispatcher` static natives.
pub(crate) fn dispatcher<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let arg = |index: usize| -> i64 {
        match arguments.get(index).copied().unwrap_or(Value::Null) {
            Value::Long(value) => value,
            Value::Int(value) => i64::from(value),
            _ => 0,
        }
    };
    let result = match (name, descriptor) {
        (b"initIDs", b"()V") => return Ok(Some(Value::Null)),
        (b"CreateFile0", b"(JIIJII)J") => {
            let path = read_wide_path(arg(0))?;
            let desired = arg(1) as u32;
            let share = arg(2) as u32;
            let _security = arg(3);
            let disposition = arg(4) as u32;
            let _flags = arg(5) as u32;
            if std::env::var_os("JVM25_TRACE").is_some() {
                eprintln!("[CreateFile0] path={:?} desired={:#x} disp={}", path, desired, disposition);
            }
            let _ = share;
            if path.contains("asm-9.10.1") {
                eprintln!("[CreateFile0] path={path:?} desired={desired:#x} disp={disposition}");
            }
            let mut options = std::fs::OpenOptions::new();
            // FILE_* / GENERIC_* desired-access bits.
            let wants_read = desired
                & (0x8000_0000 | 0x1000_0000 | 0x1 | 0x8 | 0x20 | 0x80 | 0x0010_0000)
                != 0;
            let wants_write = desired
                & (0x4000_0000 | 0x1000_0000 | 0x2 | 0x10 | 0x100 | 0x0001_0000)
                != 0;
            options.read(wants_read);
            options.write(wants_write);
            options.append(desired & 0x4 != 0);
            // Attribute-only opens (FILE_READ_ATTRIBUTES etc.) still need a
            // usable handle for metadata queries.
            if !wants_read && !wants_write && desired & 0x4 == 0 {
                options.read(true);
            }
            match disposition {
                1 => {
                    options.create_new(true);
                }
                2 => {
                    options.create(true).truncate(true);
                }
                3 => {}
                4 => {
                    options.create(true);
                }
                5 => {
                    options.truncate(true);
                }
                _ => {
                    options.create(true);
                }
            }
            match options.open(java_path(&path)) {
                Ok(file) => Value::Long(alloc_handle(EntryKind::File(file), path) as i64),
                Err(error) => return Err(win32_error(loader, &error)),
            }
        }
        (b"CloseHandle", b"(J)V") => {
            let handle = arg(0) as u64;
            free_handle(handle);
            let (lock, cvar) = iocp_sync();
            let mut ports = lock.lock().unwrap();
            if let Some(state) = ports.get_mut(&handle) {
                state.closed = true;
                state.queue.clear();
                cvar.notify_all();
            }
            return Ok(Some(Value::Null));
        }
        (b"GetFileAttributes0", b"(J)I") => {
            let path = read_wide_path(arg(0))?;
            let attrs = std::fs::metadata(java_path(&path))
                .map(|meta| {
                    let mut value = 0x80i32; // FILE_ATTRIBUTE_NORMAL
                    if meta.is_dir() {
                        value = 0x10;
                    }
                    value
                })
                .unwrap_or(0xFFFFFFFFu32 as i32); // INVALID_FILE_ATTRIBUTES
            Value::Int(attrs)
        }
        (b"GetFileAttributesEx0", b"(JJ)V") => {
            let path = read_wide_path(arg(0))?;
            let out = arg(1);
            if path.contains("asm-9.10.1") {
                eprintln!("[GetFileAttributesEx0] path={path:?}");
            }
            let mut data = [0u8; 36];
            let meta = match std::fs::metadata(java_path(&path)) {
                Ok(meta) => meta,
                Err(error) => {
                    eprintln!("[GetFileAttributesEx0] MISSING {path:?}: {error}");
                    return Err(win32_error(loader, &error));
                }
            };
            {
                let file_attrs = if meta.is_dir() { 0x10u32 } else { 0x80 };
                data[0..4].copy_from_slice(&file_attrs.to_le_bytes());
                let len = meta.len();
                data[28..32].copy_from_slice(&((len >> 32) as u32).to_le_bytes());
                data[32..36].copy_from_slice(&(len as u32).to_le_bytes());
                if let Ok(modified) = meta.modified()
                    && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
                {
                    let ticks = duration.as_secs() * 10_000_000 + duration.subsec_nanos() as u64 / 100;
                    let filetime = ticks + 116_444_736_000_000_000;
                    data[20..28].copy_from_slice(&filetime.to_le_bytes());
                }
            }
            write_buffer(out, &data)?;
            return Ok(Some(Value::Null));
        }
        (b"DeleteFile0", b"(J)V") => {
            let path = read_wide_path(arg(0))?;
            std::fs::remove_file(java_path(&path)).map_err(|e| win32_error(loader, &e))?;
            return Ok(Some(Value::Null));
        }
        (b"CreateDirectory0", b"(JJ)V") => {
            let path = read_wide_path(arg(0))?;
            let _security = arg(1);
            std::fs::create_dir(java_path(&path)).map_err(|e| win32_error(loader, &e))?;
            return Ok(Some(Value::Null));
        }
        (b"RemoveDirectory0", b"(J)V") => {
            let path = read_wide_path(arg(0))?;
            std::fs::remove_dir(java_path(&path)).map_err(|e| win32_error(loader, &e))?;
            return Ok(Some(Value::Null));
        }
        (b"CopyFileEx0", b"(JJJI)V") => {
            let source = read_wide_path(arg(0))?;
            let dest = read_wide_path(arg(1))?;
            let flags = arg(2) as u32;
            let _cancel = arg(3);
            const COPY_FILE_FAIL_IF_EXISTS: u32 = 0x1;
            let dest_path = java_path(&dest);
            if flags & COPY_FILE_FAIL_IF_EXISTS != 0
                && std::path::Path::new(&dest_path).exists()
            {
                return Err(windows_exception(
                    loader,
                    80,
                    "Cannot create a file when that file already exists",
                ));
            }
            std::fs::copy(java_path(&source), &dest_path)
                .map_err(|e| win32_error(loader, &e))?;
            return Ok(Some(Value::Null));
        }
        (b"MoveFileEx0", b"(JJI)V") => {
            let source = read_wide_path(arg(0))?;
            let dest = read_wide_path(arg(1))?;
            let flags = arg(2) as u32;
            let src = java_path(&source);
            let dst = java_path(&dest);
            // MOVEFILE_REPLACE_EXISTING | MOVEFILE_COPY_ALLOWED | MOVEFILE_WRITE_THROUGH
            let _ = flags;
            if std::path::Path::new(&dst).exists() {
                let _ = std::fs::remove_file(&dst);
            }
            if std::fs::rename(&src, &dst).is_err() {
                // Cross-volume fallback: copy then remove.
                let fallback = if std::fs::metadata(&src).map(|m| m.is_dir()).unwrap_or(false) {
                    copy_dir_all(&src, &dst).and_then(|_| std::fs::remove_dir_all(&src))
                } else {
                    std::fs::copy(&src, &dst).and_then(|_| std::fs::remove_file(&src))
                };
                fallback.map_err(|e| win32_error(loader, &e))?;
            }
            return Ok(Some(Value::Null));
        }
        (b"GetFileSizeEx", b"(J)J") => {
            let size = with_entry(arg(0) as u64, |entry| {
                file_of(entry)?.metadata().map(|meta| meta.len() as i64)
            })?;
            Value::Long(size)
        }
        (b"GetFileInformationByHandle0", b"(JJ)V") => {
            let out = arg(1);
            let mut data = [0u8; 52];
            with_entry(arg(0) as u64, |entry| {
                let meta = file_of(entry)?.metadata()?;
                let attrs = if meta.is_dir() { 0x10u32 } else { 0x80 };
                data[0..4].copy_from_slice(&attrs.to_le_bytes());
                data[4..8].copy_from_slice(&(1u32).to_le_bytes());
                data[20..24].copy_from_slice(&(meta.len() as u32).to_le_bytes());
                data[28..32].copy_from_slice(&((meta.len() >> 32) as u32).to_le_bytes());
                if let Ok(modified) = meta.modified()
                    && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
                {
                    let ticks = duration.as_secs() * 10_000_000 + duration.subsec_nanos() as u64 / 100;
                    let filetime = ticks + 116_444_736_000_000_000;
                    data[32..40].copy_from_slice(&filetime.to_le_bytes());
                }
                Ok(())
            })?;
            write_buffer(out, &data)?;
            return Ok(Some(Value::Null));
        }
        (b"SetEndOfFile", b"(J)V") => {
            with_entry(arg(0) as u64, |entry| {
                let pos = entry.pos;
                file_of(entry)?.set_len(pos).map_err(|e| io_exception(loader, &e.to_string()))
            })?;
            return Ok(Some(Value::Null));
        }
        (b"GetFullPathName0", b"(J)Ljava/lang/String;") => {
            let path = read_wide_path(arg(0))?;
            match std::fs::canonicalize(java_path(&path)) {
                Ok(canonical) => {
                    let mut text = canonical.to_string_lossy().into_owned();
                    if let Some(stripped) = text.strip_prefix("\\\\?\\") {
                        text = stripped.to_owned();
                    }
                    Value::HostString(loader.storage.alloc_str(&text))
                }
                Err(_) => Value::HostString(loader.storage.alloc_str(&java_path(&path))),
            }
        }
        (b"GetFinalPathNameByHandle", b"(J)Ljava/lang/String;") => {
            let path = with_entry(arg(0) as u64, |entry| Ok(entry.path.clone()))?;
            match std::fs::canonicalize(java_path(&path)) {
                Ok(canonical) => {
                    let mut text = canonical.to_string_lossy().into_owned();
                    if let Some(stripped) = text.strip_prefix("\\\\?\\") {
                        text = stripped.to_owned();
                    }
                    Value::HostString(loader.storage.alloc_str(&text))
                }
                Err(_) => Value::HostString(loader.storage.alloc_str(&path)),
            }
        }
        (b"GetLogicalDrives", b"()I") => {
            let mut mask = 0i32;
            for letter in b'A'..=b'Z' {
                if std::path::Path::new(&format!("{}:\\", letter as char)).exists() {
                    mask |= 1 << (letter - b'A');
                }
            }
            Value::Int(mask)
        }
        (b"GetDriveType0", b"(J)I") => {
            let _path = read_wide_path(arg(0))?;
            Value::Int(3) // DRIVE_FIXED
        }
        (b"FormatMessage", b"(I)Ljava/lang/String;") => {
            Value::HostString(loader.storage.alloc_str("I/O error"))
        }
        (b"LocalFree", b"(J)V")
        | (b"DeviceIoControlSetSparse", b"(J)V")
        | (b"CancelIo", b"(J)V") => return Ok(Some(Value::Null)),
        (b"CreateEvent", b"(ZZ)J") => Value::Long(alloc_handle(EntryKind::Other, String::new()) as i64),
        (b"GetCurrentProcess", b"()J") => Value::Long(0xFFFF_FFFFu64 as i64),
        (b"GetCurrentThread", b"()J") => Value::Long(0xFFFF_FFFF_FFFE_FFFEu64 as i64),
        (b"OpenProcessToken", b"(JI)J")
        | (b"OpenThreadToken", b"(JIZ)J")
        | (b"DuplicateTokenEx", b"(JII)J") => Value::Long(alloc_handle(EntryKind::Other, String::new()) as i64),
        (b"CreateIoCompletionPort", b"(JJJ)J") => {
            let file = arg(0) as u64;
            let existing = arg(1) as u64;
            let _key = arg(2);
            if existing != 0 {
                let (lock, _) = iocp_sync();
                let ports = lock.lock().unwrap();
                if !ports.contains_key(&existing) {
                    return Err(windows_exception(loader, 6, "The handle is invalid"));
                }
                if file != 0 && !table().lock().unwrap().entries.contains_key(&file) {
                    return Err(windows_exception(loader, 6, "The handle is invalid"));
                }
                Value::Long(existing as i64)
            } else {
                if file != 0 && !table().lock().unwrap().entries.contains_key(&file) {
                    return Err(windows_exception(loader, 6, "The handle is invalid"));
                }
                let handle = iocp_alloc();
                let (lock, _) = iocp_sync();
                lock.lock()
                    .unwrap()
                    .insert(handle, IocpState { queue: VecDeque::new(), closed: false });
                Value::Long(handle as i64)
            }
        }
        (b"ConvertSidToStringSid", b"(J)Ljava/lang/String;") => Value::Null,
        (b"GetSecurityDescriptorOwner", b"(J)J") => Value::Long(0),
        (b"GetSecurityDescriptorDacl", b"(J)J") => Value::Long(0),
        (b"GetAce", b"(JI)J") => Value::Long(0),
        (b"LookupPrivilegeValue0", b"(J)J") => Value::Long(0),
        (b"GetLengthSid", b"(J)I") => Value::Int(0),
        (b"FindFirstFile0", b"(JLsun/nio/fs/WindowsNativeDispatcher$FirstFile;)V") => {
            let search = read_wide_path(arg(0))?;
            let stack = loader.call_stack.snapshot();
            let depth = stack.len();
            let tail: Vec<String> = stack.iter().rev().take(24).map(|n| String::from_utf8_lossy(n).into_owned()).collect();
            eprintln!("[FindFirstFile0] search={search:?} depth={depth} stack(top->bottom)={tail:?}");
            let (dir, pattern) = split_search_path(&search);
            let dir = if dir.is_empty() { ".".to_string() } else { dir };
            let (handle, first) = match dir_open(&dir, &pattern) {
                Ok(opened) => opened,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(windows_exception(loader, 2, "The system cannot find the file specified"));
                }
                Err(error) => return Err(io_exception(loader, &error.to_string())),
            };
            match first {
                Some(entry) => {
                    set_first_file_fields(
                        loader,
                        arguments.get(1).copied().unwrap_or(Value::Null),
                        handle,
                        &entry.name,
                        entry.attrs,
                    )?;
                }
                None => {
                    dir_table().lock().unwrap().remove(&handle);
                    return Err(windows_exception(loader, 2, "The system cannot find the file specified"));
                }
            }
            return Ok(Some(Value::Null));
        }
        (b"FindFirstFile1", b"(JJ)J") => {
            let search = read_wide_path(arg(0))?;
            let stack = loader.call_stack.snapshot();
            let depth = stack.len();
            let tail: Vec<String> = stack.iter().rev().take(24).map(|n| String::from_utf8_lossy(n).into_owned()).collect();
            eprintln!("[FindFirstFile1] search={search:?} depth={depth} stack(top->bottom)={tail:?}");
            let (dir, pattern) = split_search_path(&search);
            let dir = if dir.is_empty() { ".".to_string() } else { dir };
            let (handle, first) = match dir_open(&dir, &pattern) {
                Ok(opened) => opened,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(windows_exception(loader, 2, "The system cannot find the path specified"));
                }
                Err(error) => return Err(io_exception(loader, &error.to_string())),
            };
            match first {
                Some(entry) => {
                    write_find_data(arg(1), &entry)?;
                    Value::Long(handle as i64)
                }
                None => {
                    dir_table().lock().unwrap().remove(&handle);
                    return Err(windows_exception(loader, 2, "The system cannot find the file specified"));
                }
            }
        }
        (b"FindNextFile0", b"(JJ)Ljava/lang/String;") => {
            let handle = arg(0) as u64;
            match dir_next(handle)? {
                Some(entry) => {
                    write_find_data(arg(1), &entry)?;
                    Value::HostString(loader.storage.alloc_str(&entry.name))
                }
                None => Value::Null,
            }
        }
        (b"FindClose", b"(J)V") => {
            dir_table().lock().unwrap().remove(&(arg(0) as u64));
            return Ok(Some(Value::Null));
        }
        (b"GetQueuedCompletionStatus0", b"(JLsun/nio/fs/WindowsNativeDispatcher$CompletionStatus;)V") => {
            let port = arg(0) as u64;
            let status_value = arguments.get(1).copied().unwrap_or(Value::Null);
            let (lock, cvar) = iocp_sync();
            let mut ports = lock.lock().unwrap();
            let (bytes, key) = loop {
                match ports.get_mut(&port) {
                    None => {
                        return Err(windows_exception(loader, 6, "The handle is invalid"));
                    }
                    Some(state) => {
                        if let Some(entry) = state.queue.pop_front() {
                            break (entry.bytes as i32, entry.key);
                        }
                        if state.closed {
                            return Err(windows_exception(loader, 6, "The handle is invalid"));
                        }
                    }
                }
                ports = cvar
                    .wait(ports)
                    .map_err(|_| invalid("IOCP wait failed"))?;
            };
            drop(ports);
            set_completion_fields(loader, status_value, 0, bytes, key)?;
            return Ok(Some(Value::Null));
        }
        (b"PostQueuedCompletionStatus", b"(JJ)V") => {
            let port = arg(0) as u64;
            let key = arg(1);
            let (lock, cvar) = iocp_sync();
            let mut ports = lock.lock().unwrap();
            let state = ports
                .get_mut(&port)
                .ok_or_else(|| windows_exception(loader, 6, "The handle is invalid"))?;
            if state.closed {
                return Err(windows_exception(loader, 6, "The handle is invalid"));
            }
            state.queue.push_back(CompletionEntry { bytes: 0, key });
            cvar.notify_one();
            return Ok(Some(Value::Null));
        }
        (b"GetOverlappedResult", b"(JJ)I") => Value::Int(0),
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// `sun.nio.ch.FileDispatcher` lock status codes.
const LOCK_NO_LOCK: i32 = -1;
const LOCK_LOCKED: i32 = 0;

/// Byte-range file lock; mirrors `LockFileEx`/`UnlockFileEx` on Windows.
/// Returns `LOCK_LOCKED` on success, `LOCK_NO_LOCK` when a non-blocking
/// request cannot be satisfied, and an `io::Error` for real failures.
#[cfg(windows)]
fn lock_range(file: &std::fs::File, pos: i64, size: i64, block: bool, shared: bool) -> io::Result<i32> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut core::ffi::c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFileEx(
            file: *mut core::ffi::c_void,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn GetOverlappedResult(
            file: *mut core::ffi::c_void,
            overlapped: *mut Overlapped,
            transferred: *mut u32,
            wait: i32,
        ) -> i32;
    }
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x1;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x2;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    const ERROR_IO_PENDING: i32 = 997;

    let handle = file.as_raw_handle() as *mut core::ffi::c_void;
    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: pos as u32,
        offset_high: (pos >> 32) as u32,
        event: core::ptr::null_mut(),
    };
    let mut flags = 0u32;
    if !block {
        flags |= LOCKFILE_FAIL_IMMEDIATELY;
    }
    if !shared {
        flags |= LOCKFILE_EXCLUSIVE_LOCK;
    }
    let ok = unsafe {
        LockFileEx(
            handle,
            flags,
            0,
            size as u32,
            (size >> 32) as u32,
            &mut overlapped,
        )
    };
    if ok != 0 {
        return Ok(LOCK_LOCKED);
    }
    let mut error = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if error == ERROR_IO_PENDING {
        let mut transferred = 0u32;
        let waited = unsafe { GetOverlappedResult(handle, &mut overlapped, &mut transferred, 1) };
        if waited != 0 {
            return Ok(LOCK_LOCKED);
        }
        error = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    }
    if error != ERROR_LOCK_VIOLATION {
        return Err(io::Error::last_os_error());
    }
    if !block {
        return Ok(LOCK_NO_LOCK);
    }
    Err(io::Error::other("Lock failed"))
}

#[cfg(windows)]
fn unlock_range(file: &std::fs::File, pos: i64, size: i64) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut core::ffi::c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn UnlockFileEx(
            file: *mut core::ffi::c_void,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn GetOverlappedResult(
            file: *mut core::ffi::c_void,
            overlapped: *mut Overlapped,
            transferred: *mut u32,
            wait: i32,
        ) -> i32;
    }
    const ERROR_NOT_LOCKED: i32 = 158;
    const ERROR_IO_PENDING: i32 = 997;

    let handle = file.as_raw_handle() as *mut core::ffi::c_void;
    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: pos as u32,
        offset_high: (pos >> 32) as u32,
        event: core::ptr::null_mut(),
    };
    let ok = unsafe {
        UnlockFileEx(handle, 0, size as u32, (size >> 32) as u32, &mut overlapped)
    };
    if ok != 0 {
        return Ok(());
    }
    let mut error = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if error == ERROR_IO_PENDING {
        let mut transferred = 0u32;
        let waited = unsafe { GetOverlappedResult(handle, &mut overlapped, &mut transferred, 1) };
        if waited != 0 {
            return Ok(());
        }
        error = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    }
    if error != ERROR_NOT_LOCKED {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn lock_range(file: &std::fs::File, _pos: i64, _size: i64, block: bool, shared: bool) -> io::Result<i32> {
    use std::fs::TryLockError;
    let acquired = if shared {
        if block {
            file.lock_shared().map(|_| true)
        } else {
            file.try_lock_shared().map(|ok| ok)
        }
    } else if block {
        file.lock().map(|_| true)
    } else {
        file.try_lock().map(|ok| ok)
    };
    match acquired {
        Ok(true) => Ok(LOCK_LOCKED),
        Ok(false) => Ok(LOCK_NO_LOCK),
        Err(TryLockError::WouldBlock) => Ok(LOCK_NO_LOCK),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(not(windows))]
fn unlock_range(file: &std::fs::File, _pos: i64, _size: i64) -> io::Result<()> {
    file.unlock()
}

/// `sun.nio.ch.FileDispatcherImpl` static natives.
pub(crate) fn dispatcher_impl<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let arg = |index: usize| -> i64 {
        match arguments.get(index).copied().unwrap_or(Value::Null) {
            Value::Long(value) => value,
            Value::Int(value) => i64::from(value),
            _ => 0,
        }
    };
    let result = match (name, descriptor) {
        (b"read0", b"(Ljava/io/FileDescriptor;JI)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let address = arg(1);
            let len = arg(2) as usize;
            let read = with_entry(handle, |entry| {
                let pos = entry.pos;
                let file = file_of(entry)?;
                file.seek(SeekFrom::Start(pos))?;
                let mut buf = vec![0u8; len];
                let n = file.read(&mut buf)?;
                buf.truncate(n);
                entry.pos = pos + n as u64;
                Ok(buf)
            })?;
            write_buffer(address, &read)?;
            Value::Int(read.len() as i32)
        }
        (b"pread0", b"(Ljava/io/FileDescriptor;JIJ)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let address = arg(1);
            let len = arg(2) as usize;
            let position = arg(3) as u64;
            let data = with_entry(handle, |entry| {
                let file = file_of(entry)?;
                file.seek(SeekFrom::Start(position))?;
                let mut buf = vec![0u8; len];
                let n = file.read(&mut buf)?;
                buf.truncate(n);
                Ok(buf)
            })?;
            write_buffer(address, &data)?;
            Value::Int(data.len() as i32)
        }
        (b"write0", b"(Ljava/io/FileDescriptor;JIZ)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let address = arg(1);
            let len = arg(2) as usize;
            let append = arg(3) != 0;
            let data = read_buffer(address, len)?;
            let written = with_entry(handle, |entry| {
                let start = if append {
                    file_of(entry)?.seek(SeekFrom::End(0))?
                } else {
                    entry.pos
                };
                let file = file_of(entry)?;
                file.seek(SeekFrom::Start(start))?;
                let n = file.write(&data)?;
                entry.pos = start + n as u64;
                Ok(n)
            })?;
            Value::Int(written as i32)
        }
        (b"pwrite0", b"(Ljava/io/FileDescriptor;JIJ)I")
        | (b"pwrite0", b"(Ljava/io/FileDescriptor;JIJZ)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let address = arg(1);
            let len = arg(2) as usize;
            let position = arg(3) as u64;
            let data = read_buffer(address, len)?;
            let written = with_entry(handle, |entry| {
                let file = file_of(entry)?;
                file.seek(SeekFrom::Start(position))?;
                let n = file.write(&data)?;
                Ok(n)
            })?;
            Value::Int(written as i32)
        }
        (b"readv0", b"(Ljava/io/FileDescriptor;JI)J") => {
            let handle = file_descriptor_handle(
                loader,
                arguments.first().copied().unwrap_or(Value::Null),
            )?;
            let iov = arg(1) as u64;
            let count = arg(2) as usize;
            let mut buffers = Vec::with_capacity(count);
            for index in 0..count {
                let base = iov + (index * 16) as u64;
                let ptr = OffHeapMemory::global().get_long(base)? as u64;
                let len = OffHeapMemory::global().get_long(base + 8)? as usize;
                buffers.push((ptr, len));
            }
            let mut total = 0usize;
            with_entry(handle, |entry| {
                let pos = entry.pos;
                let file = file_of(entry)?;
                file.seek(SeekFrom::Start(pos))?;
                for (ptr, len) in &buffers {
                    let mut buf = vec![0u8; *len];
                    let n = file.read(&mut buf)?;
                    write_buffer(*ptr as i64, &buf[..n])?;
                    total += n;
                    if n < *len {
                        break;
                    }
                }
                entry.pos = pos + total as u64;
                Ok(())
            })?;
            Value::Long(total as i64)
        }
        (b"writev0", b"(Ljava/io/FileDescriptor;JIZ)J") => {
            let handle = file_descriptor_handle(
                loader,
                arguments.first().copied().unwrap_or(Value::Null),
            )?;
            let iov = arg(1) as u64;
            let count = arg(2) as usize;
            let append = arg(3) != 0;
            let mut total = 0usize;
            with_entry(handle, |entry| {
                let mut pos = entry.pos;
                if append {
                    let file = file_of(entry)?;
                    pos = file.seek(SeekFrom::End(0))?;
                }
                for index in 0..count {
                    let base = iov + (index * 16) as u64;
                    let ptr = OffHeapMemory::global().get_long(base)? as u64;
                    let len = OffHeapMemory::global().get_long(base + 8)? as usize;
                    let data = read_buffer(ptr as i64, len)?;
                    let n = {
                        let file = file_of(entry)?;
                        file.seek(SeekFrom::Start(pos))?;
                        file.write(&data)?
                    };
                    pos += n as u64;
                    total += n;
                    if n < len {
                        break;
                    }
                }
                entry.pos = pos;
                Ok(())
            })?;
            Value::Long(total as i64)
        }
        (b"seek0", b"(Ljava/io/FileDescriptor;J)J") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let offset = arg(1);
            Value::Long(with_entry(handle, |entry| {
                let current = entry.pos;
                if offset < 0 {
                    return Ok(current as i64);
                }
                file_of(entry)?.seek(SeekFrom::Start(offset as u64))?;
                entry.pos = offset as u64;
                Ok(current as i64)
            })?)
        }
        (b"size0", b"(Ljava/io/FileDescriptor;)J") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            Value::Long(with_entry(handle, |entry| {
                file_of(entry)?.metadata().map(|meta| meta.len() as i64)
            })?)
        }
        (b"truncate0", b"(Ljava/io/FileDescriptor;J)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let length = arg(1) as u64;
            with_entry(handle, |entry| {
                file_of(entry)?.set_len(length).map_err(|e| io_exception(loader, &e.to_string()))
            })?;
            Value::Int(0)
        }
        (b"force0", b"(Ljava/io/FileDescriptor;Z)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            with_entry(handle, |entry| {
                let file = file_of(entry)?;
                file.sync_all().or_else(|error| {
                    if error.kind() == io::ErrorKind::PermissionDenied {
                        Ok(())
                    } else {
                        Err(io_exception(loader, &error.to_string()))
                    }
                })
            })?;
            Value::Int(0)
        }
        (b"available0", b"(Ljava/io/FileDescriptor;)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let available = with_entry(handle, |entry| {
                let file = file_of(entry)?;
                let meta = file.metadata()?;
                if !meta.is_file() {
                    return Ok(0i64);
                }
                Ok((meta.len() as i64).saturating_sub(entry.pos as i64).clamp(0, i32::MAX as i64))
            })?;
            Value::Int(available as i32)
        }
        (b"isOther0", b"(Ljava/io/FileDescriptor;)Z") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let other = with_entry(handle, |entry| {
                let file = file_of(entry)?;
                let meta = file.metadata()?;
                let kind = meta.file_type();
                Ok(!(kind.is_file() || kind.is_symlink()))
            })?;
            Value::Int(i32::from(other))
        }
        (b"close0", b"(Ljava/io/FileDescriptor;)V") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            free_handle(handle);
            return Ok(Some(Value::Null));
        }
        (b"duplicateHandle", b"(J)J") => {
            let source = arg(0) as u64;
            let mut table = table().lock().unwrap();
            let (kind, pos, path) = {
                let entry = table
                    .entries
                    .get(&source)
                    .ok_or_else(|| invalid("Invalid file handle"))?;
                let kind = match &entry.kind {
                    EntryKind::File(file) => EntryKind::File(file.try_clone().map_err(|e| io_exception(loader, &e.to_string()))?),
                    EntryKind::Other => EntryKind::Other,
                    _ => return Err(invalid("Cannot duplicate handle")),
                };
                (kind, entry.pos, entry.path.clone())
            };
            let handle = table.next;
            table.next += 0x10;
            table.entries.insert(handle, Entry { kind, pos, path });
            Value::Long(handle as i64)
        }
        (b"allocationGranularity0", b"()J") => Value::Long(65_536),
        (b"maxDirectTransferSize0", b"()I") => Value::Int(i32::MAX - 1),
        (b"lock0", b"(Ljava/io/FileDescriptor;ZJJZ)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let block = arg(1) != 0;
            let pos = arg(2);
            let size = arg(3);
            let shared = arg(4) != 0;
            let result = with_entry(handle, |entry| {
                lock_range(file_of(entry)?, pos, size, block, shared)
            })?;
            Value::Int(result)
        }
        (b"release0", b"(Ljava/io/FileDescriptor;JJ)V") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let pos = arg(1);
            let size = arg(2);
            with_entry(handle, |entry| {
                unlock_range(file_of(entry)?, pos, size)
                    .map_err(|e| io_exception(loader, &e.to_string()))
            })?;
            return Ok(Some(Value::Null));
        }
        (b"map0", b"(Ljava/io/FileDescriptor;IJJZ)J") => {
            flush_maps_for_fd(file_descriptor_handle(
                loader,
                arguments.first().copied().unwrap_or(Value::Null),
            )?);
            let fd = file_descriptor_handle(
                loader,
                arguments.first().copied().unwrap_or(Value::Null),
            )?;
            let prot = arg(1);
            let position = arg(2).max(0) as u64;
            let length = arg(3).max(0) as u64;
            let is_sync = arg(4) != 0;
            if is_sync {
                return Err(throw_java(loader, b"java/lang/InternalError", "should never call map on platform where MAP_SYNC is unimplemented"));
            }
            const MAP_RO: i64 = 0;
            const MAP_RW: i64 = 1;
            const MAP_PV: i64 = 2;
            if !(MAP_RO..=MAP_PV).contains(&prot) {
                return Err(invalid("Invalid map protection"));
            }
            if length == 0 || length > i32::MAX as u64 {
                return Err(invalid("Invalid map size"));
            }
            // Read the region into off-heap memory (zero-padded past EOF).
            let data = with_entry(fd, |entry| {
                let file = file_of(entry)?;
                #[cfg(windows)]
                {
                    let mut buf = vec![0u8; length as usize];
                    let mut done = 0usize;
                    while done < buf.len() {
                        let n = file.seek_read(&mut buf[done..], position + done as u64)?;
                        if n == 0 {
                            break;
                        }
                        done += n;
                    }
                    Ok(buf)
                }
                #[cfg(not(windows))]
                {
                    file.seek(SeekFrom::Start(position))?;
                    let mut buf = vec![0u8; length as usize];
                    let mut done = 0usize;
                    while done < buf.len() {
                        let n = file.read(&mut buf[done..])?;
                        if n == 0 {
                            break;
                        }
                        done += n;
                    }
                    Ok(buf)
                }
            })?;
            let addr = OffHeapMemory::global().allocate_memory(length as usize);
            OffHeapMemory::global().write_bytes(addr, &data)?;
            file_maps().lock().unwrap().insert(
                addr,
                FileMap {
                    fd,
                    path: entry_path(fd).unwrap_or_default(),
                    file_pos: position,
                    len: length,
                    writable: prot == MAP_RW,
                },
            );
            Value::Long(addr as i64)
        }
        (b"unmap0", b"(JJ)I") => {
            let addr = arg(0).max(0) as u64;
            let len = arg(1).max(0) as u64;
            if let Some(m) = file_maps().lock().unwrap().remove(&addr) {
                if m.writable {
                    flush_map(addr, &FileMap {
                        fd: m.fd,
                        path: m.path.clone(),
                        file_pos: m.file_pos,
                        len: m.len.min(len.max(m.len)),
                        writable: true,
                    });
                }
                OffHeapMemory::global().free_memory(addr);
            }
            Value::Int(0)
        }
        (b"transferTo0", b"(Ljava/io/FileDescriptor;JJLjava/io/FileDescriptor;JZ)J") => {
            let src = file_descriptor_handle(
                loader,
                arguments.first().copied().unwrap_or(Value::Null),
            )?;
            let position = arg(1).max(0) as u64;
            let dst = file_descriptor_handle(
                loader,
                arguments.get(3).copied().unwrap_or(Value::Null),
            )?;
            let count = arg(2).max(0) as u64;
            let append = matches!(arguments.get(4), Some(Value::Int(i)) if *i != 0);
            flush_maps_for_fd(src);
            flush_maps_for_fd(dst);
            let mut pos = position;
            let mut remaining = count;
            let mut total = 0u64;
            while remaining > 0 {
                let chunk = remaining.min(1 << 20) as usize;
                let buf = with_entry(src, |entry| {
                    let file = file_of(entry)?;
                    #[cfg(windows)]
                    {
                        let mut buf = vec![0u8; chunk];
                        let n = file.seek_read(&mut buf, pos)?;
                        buf.truncate(n);
                        Ok(buf)
                    }
                    #[cfg(not(windows))]
                    {
                        file.seek(SeekFrom::Start(pos))?;
                        let mut buf = vec![0u8; chunk];
                        let n = file.read(&mut buf)?;
                        buf.truncate(n);
                        Ok(buf)
                    }
                })?;
                if buf.is_empty() {
                    break;
                }
                let n = buf.len() as u64;
                with_entry(dst, |entry| {
                    let pos = entry.pos;
                    let file = file_of(entry)?;
                    if append {
                        file.seek(SeekFrom::End(0))?;
                    }
                    #[cfg(windows)]
                    {
                        file.seek_write(&buf, pos)?;
                    }
                    #[cfg(not(windows))]
                    {
                        file.seek(SeekFrom::Start(pos))?;
                        file.write_all(&buf)?;
                    }
                    entry.pos += n;
                    Ok(())
                })?;
                pos += n;
                remaining -= n;
                total += n;
            }
            Value::Long(total as i64)
        }
        (b"setDirect0", b"(Ljava/io/FileDescriptor;Ljava/nio/CharBuffer;)I") => Value::Int(0),
        _ => return Ok(None),
    };
    Ok(Some(result))
}

fn arg_value<'a>(arguments: &[Value<'a>], index: usize) -> i64 {
    match arguments.get(index).copied().unwrap_or(Value::Null) {
        Value::Long(value) => value,
        Value::Int(value) => i64::from(value),
        _ => 0,
    }
}

/// `sun.nio.ch.IOUtil` static natives.
pub(crate) fn io_util<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let result = match (name, descriptor) {
        (b"randomBytes", b"([B)Z") => {
            if let Some(Value::PrimitiveArray(handle)) = arguments.first().copied() {
                let mut arrays = loader.arrays.borrow_mut();
                let array = arrays
                    .get_mut(handle)
                    .ok_or_else(|| invalid("Invalid array"))?;
                if let crate::share::oops::typeArrayOop::TypeArrayOop::Byte(bytes) = array {
                    let mut random = vec![0u8; bytes.len()];
                    let filled = fill_random(&mut random);
                    for (slot, &byte) in bytes.iter_mut().zip(random.iter()) {
                        *slot = byte as i8;
                    }
                    return Ok(Some(Value::Int(i32::from(filled))));
                }
            }
            Value::Int(0)
        }
        (b"fdVal", b"(Ljava/io/FileDescriptor;)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            Value::Int(handle as u32 as i32)
        }
        (b"setfdVal", b"(Ljava/io/FileDescriptor;I)V") => {
            let _handle = arg_value(arguments, 0);
            let _fd = arg_value(arguments, 1);
            return Ok(Some(Value::Null));
        }
        (b"configureBlocking", b"(Ljava/io/FileDescriptor;Z)V") => return Ok(Some(Value::Null)),
        (b"write1", b"(II)I") => Value::Int(0),
        (b"drain", b"(I)Z") => Value::Int(0),
        (b"drain1", b"(I)I") => Value::Int(0),
        (b"fdLimit", b"()I") => Value::Int(65_536),
        (b"iovMax", b"()I") => Value::Int(1024),
        (b"writevMax", b"()J") => Value::Long(i64::MAX),
        (b"makePipe", b"(Z)J") => {
            Value::Long(alloc_handle(EntryKind::Other, String::new()) as i64)
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// Opens `path` for the classic `java.io` streams
/// (`FileInputStream`, `FileOutputStream`, `RandomAccessFile`) and
/// registers it in the shared handle table. The returned opaque handle
/// is stored in `FileDescriptor.handle`.
pub(crate) fn open_file(
    path: &str,
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
) -> io::Result<u64> {
    let mut options = std::fs::OpenOptions::new();
    options
        .read(read)
        .write(write)
        .create(write)
        .append(append)
        .truncate(truncate && !append);
    let file = options.open(java_path(path)).map_err(io_error)?;
    Ok(alloc_handle(EntryKind::File(file), path.to_string()))
}

/// Runs `f` with the registered file and its tracked position. The
/// position is updated by the caller through the `&mut u64`.
pub(crate) fn with_file<R>(
    handle: u64,
    f: impl FnOnce(&mut std::fs::File, &mut u64) -> io::Result<R>,
) -> io::Result<R> {
    with_entry(handle, |entry| {
        let file = match &mut entry.kind {
            EntryKind::File(file) => file,
            _ => return Err(invalid("Handle is not a file")),
        };
        f(file, &mut entry.pos)
    })
}

/// Length of the registered file in bytes.
pub(crate) fn file_len(handle: u64) -> io::Result<u64> {
    with_file(handle, |file, _| {
        file.metadata().map(|meta| meta.len()).map_err(io_error)
    })
}

/// Removes a handle from the table. Returns false when unknown.
pub(crate) fn remove_handle(handle: u64) -> bool {
    free_handle(handle).is_some()
}


/// `java.nio.MappedMemoryUtils` statics (msync/madvise equivalents).
pub(crate) fn mapped_memory_utils<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    match (name, descriptor) {
        (b"force0", b"(Ljava/io/FileDescriptor;JJ)V") => {
            let fd = file_descriptor_handle(
                loader,
                arguments.first().copied().unwrap_or(Value::Null),
            )?;
            flush_maps_for_fd(fd);
            Ok(Some(Value::Null))
        }
        (b"load0", b"(JJ)V") | (b"unload0", b"(JJ)V") => Ok(Some(Value::Null)),
        (b"isLoaded0", b"(JJJ)Z") => Ok(Some(Value::Int(1))),
        _ => Ok(None),
    }
}
