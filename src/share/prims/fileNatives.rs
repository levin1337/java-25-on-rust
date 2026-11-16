//! Native methods of `java.io.WinNTFileSystem` (and the shared
//! `java.io.FileSystem` contract) backed by the host filesystem.

use super::nioFileNatives::{self, io_error};
use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    oops::typeArrayOop::TypeArrayOop,
    prims::stringNatives,
};
use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

const BA_EXISTS: i32 = 0x01;
const BA_REGULAR: i32 = 0x02;
const BA_DIRECTORY: i32 = 0x04;
const BA_HIDDEN: i32 = 0x08;

const ACCESS_READ: i32 = 0x04;
const ACCESS_WRITE: i32 = 0x02;
const ACCESS_EXECUTE: i32 = 0x01;

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

/// Extracts the `path` field of a `java.io.File` object.
fn file_path<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<String> {
    let Value::Object(handle) = value else {
        return Err(invalid("Expected java.io.File"));
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid File object"))?;
    let slot = object
        .klass
        .instanceField(b"path", b"Ljava/lang/String;")
        .map_err(|_| invalid("File has no path field"))?;
    let path = object
        .fields
        .get(slot)
        .copied()
        .ok_or_else(|| invalid("Invalid File path slot"))?;
    drop(objects);
    let Some(text) = stringNatives::string(loader, path)? else {
        return Err(invalid("File path is not a string"));
    };
    Ok(text.into_owned())
}

fn java_path(path: &str) -> String {
    // Java on Windows accepts both separators; normalise to the host style.
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.replace('\\', "/")
    }
}

/// `WinNTFileSystem.canonicalize` semantics: `GetFullPathNameW` + `_wfullpath`
/// are purely lexical — they succeed for paths that do not exist. Only fall
/// back to the host canonicalizer (which resolves symlinks but requires the
/// file to exist) when it works; otherwise normalize lexically.
fn canonicalize_java(path: &Path) -> io::Result<String> {
    match std::fs::canonicalize(path) {
        Ok(canonical) => {
            let mut canonical = canonical.to_string_lossy().into_owned();
            // Strip the Windows \\?\ verbatim prefix like the JDK does.
            if let Some(stripped) = canonical.strip_prefix("\\\\?\\") {
                canonical = stripped.to_owned();
            }
            Ok(canonical)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(lexical_canonical(&path.to_string_lossy()))
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn lexical_canonical(path: &str) -> String {
    let mut text = path.replace('/', "\\");
    let bytes = text.as_bytes();
    let has_drive = bytes.len() >= 2 && bytes[1] == b':';
    let is_unc = text.starts_with("\\\\");
    if !has_drive && !is_unc {
        if let Ok(cwd) = std::env::current_dir() {
            text = format!("{}\\{}", cwd.to_string_lossy().replace('/', "\\"), text);
        }
    }
    // Split off the root prefix: "\\?\X:", "\\server\share", "X:\" or "X:".
    let (prefix, rest) = if let Some(stripped) = text.strip_prefix("\\\\?\\") {
        // Verbatim path: keep the prefix through the drive/root.
        let mut end = 0;
        let b = stripped.as_bytes();
        if b.len() >= 2 && b[1] == b':' {
            end = 2;
            if b.get(2) == Some(&b'\\') {
                end = 3;
            }
        }
        (text[..4 + end].to_owned(), stripped[end..].to_owned())
    } else if is_unc {
        // \\server\share\... — the share is the root.
        let parts: Vec<&str> = text.split('\\').collect();
        let take = parts.iter().take(4).map(|p| p.len() + 1).sum::<usize>();
        let prefix_len = take.min(text.len());
        (text[..prefix_len].to_owned(), text[prefix_len..].to_owned())
    } else {
        let drive_len = if has_drive { 2 } else { 0 };
        let mut end = drive_len;
        if text.as_bytes().get(end) == Some(&b'\\') {
            end += 1;
        }
        (text[..end].to_owned(), text[end..].to_owned())
    };
    let mut components: Vec<&str> = Vec::new();
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(part),
        }
    }
    let mut result = prefix;
    if !result.ends_with('\\') && !components.is_empty() {
        result.push('\\');
    }
    result.push_str(&components.join("\\"));
    result
}

#[cfg(not(windows))]
fn lexical_canonical(path: &str) -> String {
    let mut text = path.replace('\\', "/");
    if !text.starts_with('/') {
        if let Ok(cwd) = std::env::current_dir() {
            text = format!("{}/{}", cwd.to_string_lossy(), text);
        }
    }
    let mut components: Vec<&str> = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(part),
        }
    }
    format!("/{}", components.join("/"))
}

fn metadata(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(path).ok()
}

fn boolean_attributes(path: &Path) -> i32 {
    let Some(meta) = metadata(path) else {
        return 0;
    };
    let mut attrs = BA_EXISTS;
    if meta.is_dir() {
        attrs |= BA_DIRECTORY;
    } else if meta.is_file() {
        attrs |= BA_REGULAR;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x2 != 0 {
            attrs |= BA_HIDDEN;
        }
    }
    attrs
}

/// Returns the object index of the `java.io.FileDescriptor` referenced
/// by `value`. `value` may be a `FileDescriptor` itself or a stream
/// object (`FileInputStream`, `FileOutputStream`, `RandomAccessFile`)
/// carrying an `fd` field.
fn descriptor_object<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<Option<usize>> {
    let Value::Object(handle) = value else {
        return Ok(None);
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid object"))?;
    // A FileDescriptor carries a `handle` field directly.
    if object
        .klass
        .instanceField(b"handle", b"J")
        .is_ok()
    {
        return Ok(Some(handle));
    }
    match object
        .klass
        .instanceField(b"fd", b"Ljava/io/FileDescriptor;")
    {
        Ok(slot) => match object.fields.get(slot).copied().unwrap_or(Value::Null) {
            Value::Object(fd) => Ok(Some(fd)),
            _ => Ok(None),
        },
        Err(_) => Ok(None),
    }
}

/// Standard-stream handles produced by `FileDescriptor.getHandle`:
/// 1 = stdin, 2 = stdout, 3 = stderr. These never enter the
/// `nioFileNatives` handle table, so stream natives must route them
/// to the process' real standard handles.
fn is_std_handle(handle: u64) -> bool {
    (1..=3).contains(&handle)
}

fn std_write(handle: u64, buf: &[u8]) -> io::Result<()> {
    use std::io::Write;
    if std::env::var_os("JVM25_TRACE_STD").is_some() {
        eprintln!("[std_write] handle={handle} len={}", buf.len());
    }
    match handle {
        2 => {
            let mut out = io::stdout().lock();
            out.write_all(buf)?;
            out.flush()
        }
        3 => {
            let mut err = io::stderr().lock();
            err.write_all(buf)?;
            err.flush()
        }
        _ => Ok(()),
    }
}

fn std_read(handle: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::io::Read;
    match handle {
        1 => io::stdin().lock().read(buf),
        _ => Ok(0),
    }
}

/// Reads the opaque stream handle stored in a `FileDescriptor.handle`
/// field. Returns 0 for standard streams / unset descriptors.
fn stream_handle<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<u64> {
    let Some(fd) = descriptor_object(loader, value)? else {
        return Ok(0);
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(fd)
        .ok_or_else(|| invalid("Invalid FileDescriptor"))?;
    let slot = object
        .klass
        .instanceField(b"handle", b"J")
        .map_err(|_| invalid("FileDescriptor has no handle field"))?;
    match object.fields.get(slot).copied().unwrap_or(Value::Null) {
        Value::Long(handle) if handle > 0 => Ok(handle as u64),
        _ => Ok(0),
    }
}

/// The `fd` field of a stream object (`FileInputStream` etc.).
fn stream_fd_object<'a>(loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<usize> {
    let Value::Object(handle) = value else {
        return Err(invalid("Expected stream object"));
    };
    let objects = loader.borrow_objects();
    let object = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid stream object"))?;
    let slot = object
        .klass
        .instanceField(b"fd", b"Ljava/io/FileDescriptor;")
        .map_err(|_| invalid("Stream has no fd field"))?;
    match object.fields.get(slot).copied().unwrap_or(Value::Null) {
        Value::Object(fd) => Ok(fd),
        _ => Err(invalid("Stream fd is not a FileDescriptor")),
    }
}

fn set_fd_fields<'a>(loader: &ClassLoader<'a>, fd: usize, handle: i64) -> io::Result<()> {
    let mut objects = loader.borrow_objects_mut();
    let object = objects
        .get_mut(fd)
        .ok_or_else(|| invalid("Invalid FileDescriptor"))?;
    let hslot = object
        .klass
        .instanceField(b"handle", b"J")
        .map_err(|_| invalid("FileDescriptor has no handle field"))?;
    object.fields[hslot] = Value::Long(handle);
    if let Ok(fslot) = object.klass.instanceField(b"fd", b"I") {
        object.fields[fslot] = Value::Int(-1);
    }
    Ok(())
}

fn byte_array_handle<'a>(_loader: &ClassLoader<'a>, value: Value<'a>) -> io::Result<usize> {
    match value {
        Value::PrimitiveArray(handle) => Ok(handle),
        _ => Err(invalid("Expected byte array")),
    }
}

fn int_pair(arguments: &[Value<'_>], first: usize) -> io::Result<(i32, i32)> {
    match (arguments.get(first).copied(), arguments.get(first + 1).copied()) {
        (Some(Value::Int(a)), Some(Value::Int(b))) => Ok((a, b)),
        _ => Err(invalid("Invalid int arguments")),
    }
}

/// Instance native methods: `arguments[0]` is the FileSystem receiver.
pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let arg = |index: usize| arguments.get(index).copied().unwrap_or(Value::Null);
    let result = match (name, descriptor) {
        (b"canonicalize0", b"(Ljava/lang/String;)Ljava/lang/String;") => {
            let Some(text) = stringNatives::string(loader, arg(1))? else {
                return Err(invalid("Expected path string"));
            };
            let path = java_path(text.as_ref());
            match canonicalize_java(Path::new(&path)) {
                Ok(canonical) => {
                    Value::HostString(loader.storage.alloc_str(&canonical))
                }
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "IOException: {}",
                        error.to_string().replace("\\", "/")
                    )));
                }
            }
        }
        (b"getFinalPath0", b"(Ljava/lang/String;)Ljava/lang/String;") => {
            let Some(text) = stringNatives::string(loader, arg(1))? else {
                return Err(invalid("Expected path string"));
            };
            let path = java_path(text.as_ref());
            match canonicalize_java(Path::new(&path)) {
                Ok(canonical) => {
                    Value::HostString(loader.storage.alloc_str(&canonical))
                }
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "java.io.IOException: {}",
                        error
                    )));
                }
            }
        }
        (b"getBooleanAttributes0", b"(Ljava/io/File;)I") => {
            let path = file_path(loader, arg(1))?;
            Value::Int(boolean_attributes(Path::new(&java_path(&path))))
        }
        (b"checkAccess0", b"(Ljava/io/File;I)Z") => {
            let path = file_path(loader, arg(1))?;
            let access = match arg(2) {
                Value::Int(access) => access,
                _ => return Err(invalid("Invalid access mode")),
            };
            let path = java_path(&path);
            let exists = Path::new(&path).exists();
            #[cfg(windows)]
            let writable = {
                use std::os::windows::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .access_mode(0x0002)
                    .custom_flags(0x0200_0000)
                    .open(&path)
                    .is_ok()
            };
            #[cfg(not(windows))]
            let writable = std::fs::OpenOptions::new().write(true).open(&path).is_ok();
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata(Path::new(&path))
                    .map(|meta| meta.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            };
            #[cfg(not(unix))]
            let executable = {
                let lower = path.to_lowercase();
                [".exe", ".bat", ".cmd", ".com"]
                    .iter()
                    .any(|ext| lower.ends_with(ext))
            };
            let ok = exists
                && (access & ACCESS_READ != 0 || access & ACCESS_WRITE != 0 || access & ACCESS_EXECUTE != 0)
                && match access {
                    a if a & ACCESS_WRITE != 0 => writable,
                    a if a & ACCESS_EXECUTE != 0 => executable,
                    _ => true,
                };
            Value::Int(i32::from(ok))
        }
        (b"getLastModifiedTime0", b"(Ljava/io/File;)J") => {
            let path = file_path(loader, arg(1))?;
            let millis = metadata(Path::new(&java_path(&path)))
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            Value::Long(millis)
        }
        (b"getLength0", b"(Ljava/io/File;)J") => {
            let path = file_path(loader, arg(1))?;
            let length = metadata(Path::new(&java_path(&path)))
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len() as i64)
                .unwrap_or(0);
            Value::Long(length)
        }
        (b"setPermission0", b"(Ljava/io/File;IZZ)Z") => {
            let path = file_path(loader, arg(1))?;
            let access = match arg(2) {
                Value::Int(access) => access,
                _ => return Err(invalid("Invalid access mode")),
            };
            let enable = match arg(3) {
                Value::Int(flag) => flag != 0,
                _ => return Err(invalid("Invalid enable flag")),
            };
            let _owner_only = arg(4);
            let path = java_path(&path);
            let Ok(mut permissions) = std::fs::metadata(&path).map(|m| m.permissions()) else {
                return Ok(Some(Value::Int(0)));
            };
            let readonly = permissions.readonly();
            let target_readonly = if access & ACCESS_WRITE != 0 {
                !enable
            } else {
                readonly
            };
            permissions.set_readonly(target_readonly);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut mode = permissions.mode();
                let mask = match access {
                    a if a & ACCESS_READ != 0 => 0o444,
                    a if a & ACCESS_WRITE != 0 => 0o222,
                    a if a & ACCESS_EXECUTE != 0 => 0o111,
                    _ => 0,
                };
                if mask != 0 {
                    if enable {
                        mode |= mask;
                    } else {
                        mode &= !mask;
                    }
                    permissions = std::fs::Permissions::from_mode(mode);
                }
            }
            Value::Int(i32::from(
                std::fs::set_permissions(&path, permissions).is_ok(),
            ))
        }
        (b"createFileExclusively0", b"(Ljava/lang/String;)Z") => {
            let Some(text) = stringNatives::string(loader, arg(1))? else {
                return Err(invalid("Expected path string"));
            };
            let path = java_path(text.as_ref());
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => Value::Int(1),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(io::Error::other("java.io.IOException: File exists"));
                }
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "java.io.IOException: {}",
                        error
                    )));
                }
            }
        }
        (b"list0", b"(Ljava/io/File;)[Ljava/lang/String;") => {
            let path = file_path(loader, arg(1))?;
            let path = java_path(&path);
            let mut names = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&path) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        names.push(loader.storage.alloc_str(name));
                    }
                }
            }
            let mut array = crate::share::oops::objArrayOop::ObjArrayOop::withComponent(
                names.len() as i32,
                b"java/lang/String",
            )?;
            for (index, name) in names.into_iter().enumerate() {
                array.elements[index] = Value::HostString(name);
            }
            let mut arrays = loader.obj_arrays.borrow_mut();
            let handle = arrays.len();
            arrays.push(array);
            Value::ObjectArray(handle)
        }
        (b"createDirectory0", b"(Ljava/io/File;)Z") => {
            let path = file_path(loader, arg(1))?;
            let path = java_path(&path);
            Value::Int(i32::from(std::fs::create_dir(&path).is_ok()))
        }
        (b"setLastModifiedTime0", b"(Ljava/io/File;J)Z") => {
            let _path = file_path(loader, arg(1))?;
            // std does not expose mtime setting; report failure like a
            // read-only filesystem would.
            Value::Int(0)
        }
        (b"setReadOnly0", b"(Ljava/io/File;)Z") => {
            let path = file_path(loader, arg(1))?;
            let path = java_path(&path);
            let Ok(metadata) = std::fs::metadata(&path) else {
                return Ok(Some(Value::Int(0)));
            };
            let mut permissions = metadata.permissions();
            permissions.set_readonly(true);
            Value::Int(i32::from(std::fs::set_permissions(&path, permissions).is_ok()))
        }
        (b"delete0", b"(Ljava/io/File;)Z") => {
            let path = file_path(loader, arg(1))?;
            let path = java_path(&path);
            let result = if Path::new(&path).is_dir() {
                std::fs::remove_dir(&path)
            } else {
                std::fs::remove_file(&path)
            };
            Value::Int(i32::from(result.is_ok()))
        }
        (b"delete0", b"(Ljava/io/File;Z)Z") => {
            let path = file_path(loader, arg(1))?;
            let allow_readonly = match arg(2) {
                Value::Int(flag) => flag != 0,
                _ => false,
            };
            let path = java_path(&path);
            let fs_path = Path::new(&path);
            if allow_readonly
                && let Ok(metadata) = std::fs::metadata(fs_path)
            {
                let mut permissions = metadata.permissions();
                if permissions.readonly() {
                    permissions.set_readonly(false);
                    let _ = std::fs::set_permissions(fs_path, permissions);
                }
            }
            let result = if fs_path.is_dir() {
                std::fs::remove_dir(fs_path)
            } else {
                std::fs::remove_file(fs_path)
            };
            Value::Int(i32::from(result.is_ok()))
        }
        (b"rename0", b"(Ljava/io/File;Ljava/io/File;)Z") => {
            let from = file_path(loader, arg(1))?;
            let to = file_path(loader, arg(2))?;
            Value::Int(i32::from(
                std::fs::rename(java_path(&from), java_path(&to)).is_ok(),
            ))
        }
        (b"getSpace0", b"(Ljava/io/File;I)J") => {
            let _path = file_path(loader, arg(1))?;
            // std does not expose filesystem free space; report zero.
            Value::Long(0)
        }
        (b"getNameMax0", b"(Ljava/lang/String;)I") => {
            let _ = arg(1);
            Value::Int(255)
        }
        (b"sync0", b"()V") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle != 0 {
                let _ = nioFileNatives::with_file(handle, |file, _| {
                    file.sync_all().map_err(io_error)
                });
            }
            return Ok(Some(Value::Null));
        }
        (b"close0", b"()V") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle != 0 {
                nioFileNatives::remove_handle(handle);
            }
            return Ok(Some(Value::Null));
        }
        // java.io.FileInputStream / FileOutputStream / RandomAccessFile
        (b"open0", b"(Ljava/lang/String;)V") => {
            let Some(text) = stringNatives::string(loader, arg(1))? else {
                return Err(invalid("Expected path string"));
            };
            let path = java_path(text.as_ref());
            let handle = nioFileNatives::open_file(&path, true, false, false, false)
                .map_err(|error| {
                    nioFileNatives::throw_java(
                        loader,
                        b"java/io/FileNotFoundException",
                        &format!("{path} ({error})"),
                    )
                })?;
            let fd = stream_fd_object(loader, arg(0))?;
            set_fd_fields(loader, fd, handle as i64)?;
            return Ok(Some(Value::Null));
        }
        (b"open0", b"(Ljava/lang/String;Z)V") => {
            let Some(text) = stringNatives::string(loader, arg(1))? else {
                return Err(invalid("Expected path string"));
            };
            let append = match arg(2) {
                Value::Int(flag) => flag != 0,
                _ => return Err(invalid("Invalid append flag")),
            };
            let path = java_path(text.as_ref());
            let handle = nioFileNatives::open_file(&path, false, true, append, !append)
                .map_err(|error| {
                    nioFileNatives::throw_java(
                        loader,
                        b"java/io/FileNotFoundException",
                        &format!("{path} ({error})"),
                    )
                })?;
            let fd = stream_fd_object(loader, arg(0))?;
            set_fd_fields(loader, fd, handle as i64)?;
            return Ok(Some(Value::Null));
        }
        (b"open0", b"(Ljava/lang/String;I)V") => {
            let Some(text) = stringNatives::string(loader, arg(1))? else {
                return Err(invalid("Expected path string"));
            };
            let mode = match arg(2) {
                Value::Int(mode) => mode,
                _ => return Err(invalid("Invalid open mode")),
            };
            // RandomAccessFile.open0 mode bits (java.io.RandomAccessFile):
            // O_RDONLY=1, O_RDWR=2, plus O_SYNC/O_DSYNC/O_TEMPORARY flags.
            let access = mode & 3;
            let (read, write) = match access {
                1 => (true, false),
                2 => (true, true),
                _ => (true, false),
            };
            let append = false;
            let path = java_path(text.as_ref());
            if std::env::var_os("JVM25_TRACE_JAR").is_some() && path.contains(".jar") {
                static JAR_OPENS: std::sync::atomic::AtomicUsize =
                    std::sync::atomic::AtomicUsize::new(0);
                let n = JAR_OPENS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                eprintln!("[jar-open #{n}] {path}");
            }
            let handle = nioFileNatives::open_file(&path, read, write, append, false)
                .map_err(|error| {
                    nioFileNatives::throw_java(
                        loader,
                        b"java/io/FileNotFoundException",
                        &format!("{path} ({error})"),
                    )
                })?;
            let fd = stream_fd_object(loader, arg(0))?;
            set_fd_fields(loader, fd, handle as i64)?;
            return Ok(Some(Value::Null));
        }
        (b"isRegularFile0", b"(Ljava/io/FileDescriptor;)Z") => {
            let handle = stream_handle(loader, arg(1))?;
            let regular = if handle != 0 && !is_std_handle(handle) {
                nioFileNatives::with_file(handle, |file, _| {
                    file.metadata().map(|meta| meta.is_file()).map_err(io_error)
                })
                .unwrap_or(false)
            } else {
                false
            };
            Value::Int(i32::from(regular))
        }
        (b"read0", b"()I") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            if is_std_handle(handle) {
                let mut byte = [0u8; 1];
                let n = std_read(handle, &mut byte)?;
                return Ok(Some(Value::Int(if n == 0 { -1 } else { i32::from(byte[0]) })));
            }
            let mut byte = [0u8; 1];
            let count = nioFileNatives::with_file_seekable(handle, |file, pos, seekable| {
                if seekable {
                    file.seek(SeekFrom::Start(*pos))?;
                }
                let n = file.read(&mut byte)?;
                *pos += n as u64;
                Ok(n)
            })?;
            return Ok(Some(Value::Int(if count == 0 {
                -1
            } else {
                i32::from(byte[0])
            })));
        }
        (b"readBytes", b"([BII)I") | (b"readBytes0", b"([BII)I") => {
            let array_handle = byte_array_handle(loader, arg(1))?;
            let (offset, len) = int_pair(arguments, 2)?;
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(array_handle)
                .ok_or_else(|| invalid("Invalid array"))?;
            let TypeArrayOop::Byte(bytes) = array else {
                return Err(invalid("Expected byte array"));
            };
            if offset < 0 || len < 0 || offset as usize + len as usize > bytes.len() {
                return Err(io::Error::other(
                    "java.lang.IndexOutOfBoundsException",
                ));
            }
            let mut buf = vec![0u8; len as usize];
            if is_std_handle(handle) {
                let n = std_read(handle, &mut buf)?;
                buf.truncate(n);
                for (index, &byte) in buf.iter().enumerate() {
                    bytes[offset as usize + index] = byte as i8;
                }
                return Ok(Some(Value::Int(if n == 0 { -1 } else { n as i32 })));
            }
            if std::env::var_os("JVM25_TRACE_FREAD").is_some() {
                let path = nioFileNatives::entry_path(handle)
                    .unwrap_or_else(|| "?".to_string());
                eprintln!("[fread] {path} handle={handle} off={offset} len={len}");
            }
            let count = nioFileNatives::with_file_seekable(handle, |file, pos, seekable| {
                if seekable {
                    file.seek(SeekFrom::Start(*pos))?;
                }
                let n = file.read(&mut buf)?;
                buf.truncate(n);
                *pos += n as u64;
                Ok(n)
            })
            .map_err(|error| {
                let path = nioFileNatives::entry_path(handle)
                    .unwrap_or_else(|| "?".to_string());
                io::Error::other(format!(
                    "readBytes {path} handle={handle} off={offset} len={len}: {error}"
                ))
            })?;
            for (index, &byte) in buf.iter().enumerate() {
                bytes[offset as usize + index] = byte as i8;
            }
            return Ok(Some(Value::Int(if count == 0 { -1 } else { count as i32 })));
        }
        (b"length0", b"()J") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            return Ok(Some(Value::Long(nioFileNatives::file_len(handle)? as i64)));
        }
        (b"position0", b"()J") | (b"getFilePointer", b"()J") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            let pos = nioFileNatives::with_file(handle, |_, pos| Ok(*pos))?;
            return Ok(Some(Value::Long(pos as i64)));
        }
        (b"skip0", b"(J)J") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            let delta = match arg(1) {
                Value::Long(delta) => delta,
                _ => return Err(invalid("Invalid skip distance")),
            };
            if is_std_handle(handle) {
                return Ok(Some(Value::Long(0)));
            }
            let pos = nioFileNatives::with_file(handle, |file, pos| {
                let target = (*pos as i64 + delta).max(0) as u64;
                file.seek(SeekFrom::Start(target))?;
                let skipped = target - *pos;
                *pos = target;
                Ok(skipped as i64)
            })?;
            return Ok(Some(Value::Long(pos)));
        }
        (b"seek0", b"(J)V") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            let target = match arg(1) {
                Value::Long(target) => target,
                _ => return Err(invalid("Invalid seek position")),
            };
            nioFileNatives::with_file(handle, |file, pos| {
                file.seek(SeekFrom::Start(target as u64))?;
                *pos = target as u64;
                Ok(())
            })?;
            return Ok(Some(Value::Null));
        }
        (b"setLength0", b"(J)V") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            let length = match arg(1) {
                Value::Long(length) => length,
                _ => return Err(invalid("Invalid length")),
            };
            if length < 0 {
                return Err(io::Error::other("java.io.IOException: Negative length"));
            }
            nioFileNatives::with_file(handle, |file, _| {
                file.set_len(length as u64).map_err(io_error)
            })?;
            return Ok(Some(Value::Null));
        }
        (b"available0", b"()I") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            if is_std_handle(handle) {
                return Ok(Some(Value::Int(0)));
            }
            let available = nioFileNatives::with_file(handle, |file, pos| {
                let len = file.metadata().map_err(io_error)?.len();
                Ok(len.saturating_sub(*pos).min(i32::MAX as u64) as i32)
            })?;
            return Ok(Some(Value::Int(available)));
        }
        (b"write", b"(IZ)V") | (b"write0", b"(I)V") => {
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            let byte = match arg(1) {
                Value::Int(byte) => byte as u8,
                _ => return Err(invalid("Invalid byte")),
            };
            let append = name == b"write"
                && matches!(arguments.get(2).copied(), Some(Value::Int(flag)) if flag != 0);
            if is_std_handle(handle) {
                std_write(handle, &[byte])?;
                return Ok(Some(Value::Null));
            }
            nioFileNatives::with_file_seekable(handle, |file, pos, seekable| {
                if append {
                    let len = file.metadata().map_err(io_error)?.len();
                    file.seek(SeekFrom::Start(len))?;
                }
                file.write_all(&[byte]).map_err(io_error)?;
                if seekable {
                    *pos = file.stream_position().map_err(io_error)?;
                } else {
                    *pos += 1;
                }
                Ok(())
            })?;
            return Ok(Some(Value::Null));
        }
        (b"writeBytes", b"([BIIZ)V") | (b"writeBytes0", b"([BII)V") => {
            let array_handle = byte_array_handle(loader, arg(1))?;
            let has_append = descriptor.ends_with(b"IZ)V");
            let (offset, len) = int_pair(arguments, 2)?;
            let append = has_append
                && matches!(arguments.get(4).copied(), Some(Value::Int(flag)) if flag != 0);
            let handle = stream_handle(loader, arg(0))?;
            if handle == 0 {
                return Err(invalid("Stream is not open"));
            }
            let mut arrays = loader.arrays.borrow_mut();
            let array = arrays
                .get_mut(array_handle)
                .ok_or_else(|| invalid("Invalid array"))?;
            let TypeArrayOop::Byte(bytes) = array else {
                return Err(invalid("Expected byte array"));
            };
            if offset < 0 || len < 0 || offset as usize + len as usize > bytes.len() {
                return Err(io::Error::other(
                    "java.lang.IndexOutOfBoundsException",
                ));
            }
            let buf: Vec<u8> = bytes[offset as usize..offset as usize + len as usize]
                .iter()
                .map(|&b| b as u8)
                .collect();
            if is_std_handle(handle) {
                std_write(handle, &buf)?;
                return Ok(Some(Value::Null));
            }
            nioFileNatives::with_file_seekable(handle, |file, pos, seekable| {
                if append {
                    let end = file.metadata().map_err(io_error)?.len();
                    file.seek(SeekFrom::Start(end))?;
                }
                file.write_all(&buf).map_err(io_error)?;
                if seekable {
                    *pos = file.stream_position().map_err(io_error)?;
                } else {
                    *pos += buf.len() as u64;
                }
                Ok(())
            })?;
            return Ok(Some(Value::Null));
        }
        (b"getDriveDirectory", b"(I)Ljava/lang/String;") => {
            let drive = match arg(1) {
                Value::Int(drive) => drive,
                _ => return Err(invalid("Invalid drive index")),
            };
            if !(1..=26).contains(&drive) {
                return Ok(Some(Value::Null));
            }
            let letter = (b'A' + (drive - 1) as u8) as char;
            let root = format!("{letter}:\\");
            if Path::new(&root).exists() {
                Value::HostString(loader.storage.alloc_str(&root))
            } else {
                Value::Null
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// Static natives of the file system.
pub(crate) fn invoke_static<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let arg = |index: usize| arguments.get(index).copied().unwrap_or(Value::Null);
    let result = match (name, descriptor) {
        (b"initIDs", b"()V") => return Ok(Some(Value::Null)),
        (b"getAppend", b"(I)Z") => Value::Int(0),
        (b"getHandle", b"(I)J") => {
            match arg(0) {
                // Standard descriptors map to 1/2/3 (fd + 1) so that
                // `FileDescriptor.handle` is nonzero for stdin and stays
                // clear of the `nioFileNatives` handle table (0x1000+).
                Value::Int(fd) if (0..=2).contains(&fd) => Value::Long(i64::from(fd) + 1),
                Value::Int(_) => Value::Long(-1),
                _ => Value::Long(-1),
            }
        }
        (b"listRoots0", b"()I") => {
            let mut count = 0i32;
            for letter in b'A'..=b'Z' {
                let root = format!("{}:\\", letter as char);
                if Path::new(&root).exists() {
                    count += 1;
                }
            }
            Value::Int(count)
        }
        (b"canonicalize0", b"(Ljava/lang/String;)Ljava/lang/String;") => {
            // Some JDKs declare this static on the delegate; handle it here too.
            let Some(text) = stringNatives::string(loader, arg(0))? else {
                return Err(invalid("Expected path string"));
            };
            let path = java_path(text.as_ref());
            match canonicalize_java(Path::new(&path)) {
                Ok(canonical) => {
                    Value::HostString(loader.storage.alloc_str(&canonical))
                }
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "java.io.IOException: {}",
                        error
                    )));
                }
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}
