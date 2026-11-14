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
    collections::HashMap,
    io::{self, Read, Seek, SeekFrom, Write},
    sync::{Mutex, OnceLock},
};

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
    windows_exception(loader, code, &error.to_string())
}

pub(crate) struct Entry {
    pub(crate) kind: EntryKind,
    pub(crate) pos: u64,
    pub(crate) path: String,
}

pub(crate) enum EntryKind {
    File(std::fs::File),
    ReadEnd(std::fs::File),
    WriteEnd(std::fs::File),
    Other,
}

struct Table {
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

pub(crate) fn with_entry<R>(handle: u64, f: impl FnOnce(&mut Entry) -> io::Result<R>) -> io::Result<R> {
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

/// Reads a NUL-terminated UTF-16LE path from off-heap memory.
fn read_wide_path(address: i64) -> io::Result<String> {
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
    Ok(String::from_utf16_lossy(&units))
}

fn java_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.replace('\\', "/")
    }
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
            let _ = share;
            let mut options = std::fs::OpenOptions::new();
            options.read(desired & 0x8000_0000 != 0 || desired & 0x1 != 0);
            options.write(desired & 0x4000_0000 != 0 || desired & 0x2000_0000 != 0 || desired & 0x400 != 0);
            options.append(desired & 0x4 != 0);
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
            free_handle(arg(0) as u64);
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
            let mut data = [0u8; 36];
            if let Ok(meta) = std::fs::metadata(java_path(&path)) {
                let file_attrs = if meta.is_dir() { 0x10u32 } else { 0x80 };
                data[0..4].copy_from_slice(&file_attrs.to_le_bytes());
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
            Value::Long(alloc_handle(EntryKind::Other, String::new()) as i64)
        }
        (b"ConvertSidToStringSid", b"(J)Ljava/lang/String;") => Value::Null,
        (b"GetSecurityDescriptorOwner", b"(J)J") => Value::Long(0),
        (b"GetSecurityDescriptorDacl", b"(J)J") => Value::Long(0),
        (b"GetAce", b"(JI)J") => Value::Long(0),
        (b"LookupPrivilegeValue0", b"(J)J") => Value::Long(0),
        (b"GetLengthSid", b"(J)I") => Value::Int(0),
        (b"GetQueuedCompletionStatus0", b"(JJJ)I") => Value::Int(0),
        (b"GetOverlappedResult", b"(JJ)I") => Value::Int(0),
        _ => return Ok(None),
    };
    Ok(Some(result))
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
            let _blocking = arg(3) != 0;
            let data = read_buffer(address, len)?;
            let written = with_entry(handle, |entry| {
                let pos = entry.pos;
                let file = file_of(entry)?;
                file.seek(SeekFrom::Start(pos))?;
                let n = file.write(&data)?;
                entry.pos = pos + n as u64;
                Ok(n)
            })?;
            Value::Int(written as i32)
        }
        (b"pwrite0", b"(Ljava/io/FileDescriptor;JIJZ)I") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let address = arg(1);
            let len = arg(2) as usize;
            let position = arg(3) as u64;
            let _blocking = arg(4) != 0;
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
            let _blocking = arg(3) != 0;
            let mut total = 0usize;
            with_entry(handle, |entry| {
                let pos = entry.pos;
                let file = file_of(entry)?;
                file.seek(SeekFrom::Start(pos))?;
                for index in 0..count {
                    let base = iov + (index * 16) as u64;
                    let ptr = OffHeapMemory::global().get_long(base)? as u64;
                    let len = OffHeapMemory::global().get_long(base + 8)? as usize;
                    let data = read_buffer(ptr as i64, len)?;
                    let n = file.write(&data)?;
                    total += n;
                    if n < len {
                        break;
                    }
                }
                entry.pos = pos + total as u64;
                Ok(())
            })?;
            Value::Long(total as i64)
        }
        (b"seek0", b"(Ljava/io/FileDescriptor;J)J") => {
            let handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            let position = arg(1) as u64;
            Value::Long(with_entry(handle, |entry| {
                let current = entry.pos;
                file_of(entry)?.seek(SeekFrom::Start(position))?;
                entry.pos = position;
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
            let _handle = file_descriptor_handle(loader, arguments.first().copied().unwrap_or(Value::Null))?;
            Value::Int(0)
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
        (b"maxDirectTransferSize0", b"()I") => Value::Int(0x7FFF_F000),
        (b"lock0", b"(Ljava/io/FileDescriptor;ZJJZ)I")
        | (b"release0", b"(Ljava/io/FileDescriptor;JJ)V") => {
            if name == b"lock0" {
                Value::Int(0)
            } else {
                return Ok(Some(Value::Null));
            }
        }
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
                    use std::time::{SystemTime, UNIX_EPOCH};
                    let seed = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
                        .unwrap_or(0x9E37_79B9);
                    let mut state = seed | 1;
                    for byte in bytes.iter_mut() {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        *byte = state as i8;
                    }
                    return Ok(Some(Value::Int(1)));
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

