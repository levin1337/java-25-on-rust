//! Native methods of `java.io.WinNTFileSystem` (and the shared
//! `java.io.FileSystem` contract) backed by the host filesystem.

use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    prims::stringNatives,
};
use std::{io, path::Path};

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
            match std::fs::canonicalize(&path) {
                Ok(canonical) => {
                    let mut canonical = canonical.to_string_lossy().into_owned();
                    // Strip the Windows \\?\ verbatim prefix like the JDK does.
                    if let Some(stripped) = canonical.strip_prefix("\\\\?\\") {
                        canonical = stripped.to_owned();
                    }
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
            match std::fs::canonicalize(&path) {
                Ok(canonical) => {
                    let mut canonical = canonical.to_string_lossy().into_owned();
                    if let Some(stripped) = canonical.strip_prefix("\\\\?\\") {
                        canonical = stripped.to_owned();
                    }
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
            let writable = std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .is_ok();
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
        (b"sync0", b"()V") => return Ok(Some(Value::Null)),
        (b"close0", b"()V") => return Ok(Some(Value::Null)),
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
                Value::Int(fd) => Value::Long(i64::from(fd)),
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
            match std::fs::canonicalize(&path) {
                Ok(canonical) => {
                    let mut canonical = canonical.to_string_lossy().into_owned();
                    if let Some(stripped) = canonical.strip_prefix("\\\\?\\") {
                        canonical = stripped.to_owned();
                    }
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
