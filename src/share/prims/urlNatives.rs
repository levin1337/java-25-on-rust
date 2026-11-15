//! URL / URLConnection / classpath-resource support.
//!
//! This module backs `java.util.ServiceLoader`-style classpath resource
//! discovery:
//!
//! * `ClassLoader.getResources` is re-implemented here so it emits real
//!   `java.net.URL` objects (`jar:file:/...!/entry`, `file:/...`,
//!   `jrt:/module/...`) built through the genuine JDK `URL.<init>`.
//! * `java.net.URL.openConnection` is intercepted for the `jar:` and `jrt:`
//!   protocols and answered with a small synthetic
//!   `jvm25.url.UrlConnection` whose native `open0`/`length0` read bytes
//!   straight from the classpath zip archives or the boot-image jimage.
//!   `file:` URLs keep the real JDK `FileURLConnection` (its
//!   `FileInputStream` natives live in `fileNatives`/`nioFileNatives`).

use super::stringNatives;
use crate::share::{
    classfile::classLoader::{ClassLoader, ClassSource},
    interpreter::bytecodeInterpreter::{self, JavaThrowable, Value},
    oops::{instanceKlass::InstanceKlass, instanceOop::InstanceOop, typeArrayOop::TypeArrayOop},
    runtime::javaCalls::{JavaCallArguments, JavaCalls},
};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    sync::Arc,
};

/// Internal name of the synthetic `java.net.URLConnection` subclass embedded
/// below (`src/share/prims/url_connection.class`, compiled from
/// `url_connection.java`).
const URL_CONNECTION_CLASS: &[u8] = b"jvm25/url/UrlConnection";

const URL_CONNECTION_CLASS_BYTES: &[u8] = &[
    202, 254, 186, 190, 0, 0, 0, 65, 0, 41, 10, 0, 2, 0, 3, 7,
    0, 4, 12, 0, 5, 0, 6, 1, 0, 22, 106, 97, 118, 97, 47, 110,
    101, 116, 47, 85, 82, 76, 67, 111, 110, 110, 101, 99, 116, 105, 111, 110,
    1, 0, 6, 60, 105, 110, 105, 116, 62, 1, 0, 17, 40, 76, 106, 97,
    118, 97, 47, 110, 101, 116, 47, 85, 82, 76, 59, 41, 86, 10, 0, 8,
    0, 9, 7, 0, 10, 12, 0, 11, 0, 12, 1, 0, 23, 106, 118, 109,
    50, 53, 47, 117, 114, 108, 47, 85, 114, 108, 67, 111, 110, 110, 101, 99,
    116, 105, 111, 110, 1, 0, 5, 111, 112, 101, 110, 48, 1, 0, 23, 40,
    41, 76, 106, 97, 118, 97, 47, 105, 111, 47, 73, 110, 112, 117, 116, 83,
    116, 114, 101, 97, 109, 59, 10, 0, 8, 0, 14, 12, 0, 15, 0, 16,
    1, 0, 7, 108, 101, 110, 103, 116, 104, 48, 1, 0, 3, 40, 41, 74,
    5, 0, 0, 0, 0, 127, 255, 255, 255, 1, 0, 4, 67, 111, 100, 101,
    1, 0, 15, 76, 105, 110, 101, 78, 117, 109, 98, 101, 114, 84, 97, 98,
    108, 101, 1, 0, 7, 99, 111, 110, 110, 101, 99, 116, 1, 0, 3, 40,
    41, 86, 1, 0, 14, 103, 101, 116, 73, 110, 112, 117, 116, 83, 116, 114,
    101, 97, 109, 1, 0, 10, 69, 120, 99, 101, 112, 116, 105, 111, 110, 115,
    7, 0, 26, 1, 0, 19, 106, 97, 118, 97, 47, 105, 111, 47, 73, 79,
    69, 120, 99, 101, 112, 116, 105, 111, 110, 1, 0, 16, 103, 101, 116, 67,
    111, 110, 116, 101, 110, 116, 76, 101, 110, 103, 116, 104, 1, 0, 3, 40,
    41, 73, 1, 0, 13, 83, 116, 97, 99, 107, 77, 97, 112, 84, 97, 98,
    108, 101, 1, 0, 20, 103, 101, 116, 67, 111, 110, 116, 101, 110, 116, 76,
    101, 110, 103, 116, 104, 76, 111, 110, 103, 1, 0, 14, 103, 101, 116, 67,
    111, 110, 116, 101, 110, 116, 84, 121, 112, 101, 1, 0, 20, 40, 41, 76,
    106, 97, 118, 97, 47, 108, 97, 110, 103, 47, 83, 116, 114, 105, 110, 103,
    59, 1, 0, 14, 103, 101, 116, 72, 101, 97, 100, 101, 114, 70, 105, 101,
    108, 100, 1, 0, 38, 40, 76, 106, 97, 118, 97, 47, 108, 97, 110, 103,
    47, 83, 116, 114, 105, 110, 103, 59, 41, 76, 106, 97, 118, 97, 47, 108,
    97, 110, 103, 47, 83, 116, 114, 105, 110, 103, 59, 1, 0, 15, 103, 101,
    116, 72, 101, 97, 100, 101, 114, 70, 105, 101, 108, 100, 115, 1, 0, 17,
    40, 41, 76, 106, 97, 118, 97, 47, 117, 116, 105, 108, 47, 77, 97, 112,
    59, 1, 0, 9, 83, 105, 103, 110, 97, 116, 117, 114, 101, 1, 0, 73,
    40, 41, 76, 106, 97, 118, 97, 47, 117, 116, 105, 108, 47, 77, 97, 112,
    60, 76, 106, 97, 118, 97, 47, 108, 97, 110, 103, 47, 83, 116, 114, 105,
    110, 103, 59, 76, 106, 97, 118, 97, 47, 117, 116, 105, 108, 47, 76, 105,
    115, 116, 60, 76, 106, 97, 118, 97, 47, 108, 97, 110, 103, 47, 83, 116,
    114, 105, 110, 103, 59, 62, 59, 62, 59, 1, 0, 10, 83, 111, 117, 114,
    99, 101, 70, 105, 108, 101, 1, 0, 18, 85, 114, 108, 67, 111, 110, 110,
    101, 99, 116, 105, 111, 110, 46, 106, 97, 118, 97, 0, 49, 0, 8, 0,
    2, 0, 0, 0, 0, 0, 10, 0, 1, 0, 5, 0, 6, 0, 1, 0,
    19, 0, 0, 0, 34, 0, 2, 0, 2, 0, 0, 0, 6, 42, 43, 183,
    0, 1, 177, 0, 0, 0, 1, 0, 20, 0, 0, 0, 10, 0, 2, 0,
    0, 0, 16, 0, 5, 0, 17, 0, 1, 0, 21, 0, 22, 0, 1, 0,
    19, 0, 0, 0, 25, 0, 0, 0, 1, 0, 0, 0, 1, 177, 0, 0,
    0, 1, 0, 20, 0, 0, 0, 6, 0, 1, 0, 0, 0, 21, 0, 1,
    0, 23, 0, 12, 0, 2, 0, 19, 0, 0, 0, 29, 0, 1, 0, 1,
    0, 0, 0, 5, 42, 182, 0, 7, 176, 0, 0, 0, 1, 0, 20, 0,
    0, 0, 6, 0, 1, 0, 0, 0, 25, 0, 24, 0, 0, 0, 4, 0,
    1, 0, 25, 1, 2, 0, 11, 0, 12, 0, 1, 0, 24, 0, 0, 0,
    4, 0, 1, 0, 25, 1, 2, 0, 15, 0, 16, 0, 0, 0, 1, 0,
    27, 0, 28, 0, 1, 0, 19, 0, 0, 0, 73, 0, 4, 0, 3, 0,
    0, 0, 24, 42, 182, 0, 13, 64, 31, 9, 148, 155, 0, 11, 31, 20,
    0, 17, 148, 158, 0, 5, 2, 172, 31, 136, 172, 0, 0, 0, 2, 0,
    20, 0, 0, 0, 18, 0, 4, 0, 0, 0, 34, 0, 5, 0, 35, 0,
    19, 0, 36, 0, 21, 0, 38, 0, 29, 0, 0, 0, 7, 0, 2, 252,
    0, 19, 4, 1, 0, 1, 0, 30, 0, 16, 0, 1, 0, 19, 0, 0,
    0, 29, 0, 2, 0, 1, 0, 0, 0, 5, 42, 182, 0, 13, 173, 0,
    0, 0, 1, 0, 20, 0, 0, 0, 6, 0, 1, 0, 0, 0, 43, 0,
    1, 0, 31, 0, 32, 0, 1, 0, 19, 0, 0, 0, 26, 0, 1, 0,
    1, 0, 0, 0, 2, 1, 176, 0, 0, 0, 1, 0, 20, 0, 0, 0,
    6, 0, 1, 0, 0, 0, 48, 0, 1, 0, 33, 0, 34, 0, 1, 0,
    19, 0, 0, 0, 26, 0, 1, 0, 2, 0, 0, 0, 2, 1, 176, 0,
    0, 0, 1, 0, 20, 0, 0, 0, 6, 0, 1, 0, 0, 0, 53, 0,
    1, 0, 35, 0, 36, 0, 2, 0, 19, 0, 0, 0, 26, 0, 1, 0,
    1, 0, 0, 0, 2, 1, 176, 0, 0, 0, 1, 0, 20, 0, 0, 0,
    6, 0, 1, 0, 0, 0, 58, 0, 37, 0, 0, 0, 2, 0, 38, 0,
    1, 0, 39, 0, 0, 0, 2, 0, 40,
];

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

fn sink() -> io::Sink {
    io::sink()
}

// ---------------------------------------------------------------------------
// Throwable helpers
// ---------------------------------------------------------------------------

/// Builds a real throwable object of `class_name` with `detailMessage` set,
/// without running its constructor (Throwable's native stack-trace fill-in is
/// a no-op in this VM anyway).
fn throwable<'a>(loader: &ClassLoader<'a>, class_name: &[u8], message: &str) -> io::Result<usize> {
    let klass = loader.load(class_name)?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut sink(), 0);
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

/// Returns an `io::Error` carrying a Java throwable so the interpreter raises
/// it as a real Java exception.
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

fn file_not_found<'a>(loader: &ClassLoader<'a>, message: &str) -> io::Error {
    throw_java(loader, b"java/io/FileNotFoundException", message)
}

// ---------------------------------------------------------------------------
// Object field helpers
// ---------------------------------------------------------------------------

/// Reads an instance field of an object value.
fn object_field<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<Value<'a>> {
    let Value::Object(handle) = object else {
        return Err(invalid("Expected object receiver"));
    };
    let slot = {
        let objects = loader.borrow_objects();
        let target = objects
            .get(handle)
            .ok_or_else(|| invalid("Invalid object reference"))?;
        target.klass.instanceField(name, descriptor)?
    };
    let objects = loader.borrow_objects();
    let target = objects
        .get(handle)
        .ok_or_else(|| invalid("Invalid object reference"))?;
    target
        .fields
        .get(slot)
        .copied()
        .ok_or_else(|| invalid("Invalid field slot"))
}

/// Reads a String-typed instance field as owned text.
fn string_field<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<Option<String>> {
    let value = object_field(loader, object, name, descriptor)?;
    let Some(text) = stringNatives::string(loader, value)? else {
        return Ok(None);
    };
    Ok(Some(text.into_owned()))
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// Extracts the `protocol` field of a `java.net.URL` object.
fn url_protocol<'a>(loader: &ClassLoader<'a>, url: Value<'a>) -> io::Result<Option<String>> {
    string_field(loader, url, b"protocol", b"Ljava/lang/String;")
}

/// Extracts `(protocol, file)` of a `java.net.URL` object.
fn url_parts<'a>(
    loader: &ClassLoader<'a>,
    url: Value<'a>,
) -> io::Result<(Option<String>, Option<String>)> {
    Ok((
        url_protocol(loader, url)?,
        string_field(loader, url, b"file", b"Ljava/lang/String;")?,
    ))
}

/// Lazily defines the synthetic `jvm25.url.UrlConnection` class.
fn connection_class<'a>(
    loader: &ClassLoader<'a>,
) -> io::Result<Arc<InstanceKlass<'a>>> {
    if let Ok(klass) = loader.load(URL_CONNECTION_CLASS) {
        return Ok(klass);
    }
    loader.define_class_bytes(URL_CONNECTION_CLASS, URL_CONNECTION_CLASS_BYTES)?;
    loader.load(URL_CONNECTION_CLASS)
}

/// Allocates a `jvm25.url.UrlConnection` for `url` with its `url` field set.
fn connection_object<'a>(
    loader: &ClassLoader<'a>,
    url: Value<'a>,
) -> io::Result<Value<'a>> {
    let klass = connection_class(loader)?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut sink(), 0);
    let mut connection = InstanceOop::new(klass.clone())?;
    if let Ok(slot) = klass.instanceField(b"url", b"Ljava/net/URL;") {
        connection.fields[slot] = url;
    }
    if let Ok(slot) = klass.instanceField(b"doInput", b"Z") {
        connection.fields[slot] = Value::Int(1);
    }
    Ok(Value::Object(loader.alloc_object(connection)))
}

// ---------------------------------------------------------------------------
// URL spec -> host resource resolution
// ---------------------------------------------------------------------------

/// Percent-decodes a URL path component (`%XX` -> byte, UTF-8 passthrough).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(hi), Some(lo)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push(hi << 4 | lo);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Converts the path portion of a `file:` URL into a host filesystem path.
/// `file:/C:/x` -> `C:\x` on Windows, `file:/tmp/x` -> `/tmp/x` on Unix.
fn file_url_to_path(spec: &str) -> String {
    let rest = spec.strip_prefix("file:").unwrap_or(spec);
    // "file:///abs" -> "/abs"; "file://host/path" collapses to "host/path"
    // (remote hosts are not supported; rare in classpath resources).
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let decoded = percent_decode(rest);
    #[cfg(windows)]
    {
        let mut path = decoded.replace('/', "\\");
        // "\C:\x" (from "/C:/x") -> "C:\x"; keep UNC "\\server\share" intact.
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'\\'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':'
            && bytes.get(3) == Some(&b'\\')
        {
            path.remove(0);
        }
        path
    }
    #[cfg(not(windows))]
    {
        decoded
    }
}

/// Converts a host path into a `file:` URL as produced by `URLClassPath`.
fn path_to_file_url(path: &Path) -> String {
    let mut text = path.to_string_lossy().into_owned();
    if let Ok(canonical) = std::fs::canonicalize(path) {
        text = canonical.to_string_lossy().into_owned();
        if let Some(stripped) = text.strip_prefix("\\\\?\\") {
            text = stripped.to_owned();
        }
    }
    let mut url = text.replace('\\', "/");
    if url.starts_with("//") {
        // UNC path -> file://server/share
        return format!("file:{}", encodeUrlPath(&url));
    }
    if !url.starts_with('/') {
        // "C:/..." -> "/C:/..."
        url.insert(0, '/');
    }
    format!("file:{}", encodeUrlPath(&url))
}

fn encodeUrlPath(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/:-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Splits a `jar:` URL file part ("file:/x.jar!/entry") into
/// (jar filesystem path, entry name).
fn parse_jar_spec<'b>(loader: &ClassLoader<'_>, file: &'b str) -> io::Result<(String, &'b str)> {
    let (jar_url, entry) = file
        .split_once("!/")
        .ok_or_else(|| io_exception(loader, "JAR URL does not contain !/ entry separator"))?;
    let path = file_url_to_path(jar_url);
    Ok((path, entry))
}

/// Reads the bytes of the resource addressed by a `jvm25.url.UrlConnection`.
fn read_url_bytes<'a>(
    loader: &ClassLoader<'a>,
    protocol: &str,
    file: &str,
) -> io::Result<Vec<u8>> {
    match protocol {
        "file" => {
            let path = file_url_to_path(file);
            match std::fs::read(Path::new(&path)) {
                Ok(bytes) => Ok(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Err(file_not_found(loader, &format!("{path} (No such file or directory)")))
                }
                Err(error) => Err(io_exception(loader, &error.to_string())),
            }
        }
        "jar" => {
            let (jar_path, entry) = parse_jar_spec(loader, file)?;
            let entry = percent_decode(entry);
            let archive = File::open(&jar_path).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    file_not_found(loader, &format!("{jar_path} (No such file or directory)"))
                } else {
                    io_exception(loader, &error.to_string())
                }
            })?;
            let mut zip = zip::ZipArchive::new(archive)
                .map_err(|error| io_exception(loader, &error.to_string()))?;
            let mut entry_file = zip.by_name(&entry).map_err(|_| {
                file_not_found(loader, &format!("JAR entry {entry} not found in {jar_path}"))
            })?;
            let mut bytes = Vec::new();
            entry_file
                .read_to_end(&mut bytes)
                .map_err(|error| io_exception(loader, &error.to_string()))?;
            Ok(bytes)
        }
        "jrt" => {
            let image = loader
                .bootImage
                .as_ref()
                .ok_or_else(|| io_exception(loader, "No boot image for jrt: URL"))?;
            let mut image = image.borrow_mut();
            if !image.has_resource(file) {
                return Err(file_not_found(loader, &format!("{file} (No such resource)")));
            }
            image
                .read_resource(file)
                .map_err(|error| io_exception(loader, &error.to_string()))
        }
        _ => Err(io_exception(
            loader,
            &format!("Protocol \"{protocol}\" not supported"),
        )),
    }
}

/// Returns the byte length of the resource, or -1 when unknown.
fn url_content_length<'a>(
    loader: &ClassLoader<'a>,
    protocol: &str,
    file: &str,
) -> i64 {
    match protocol {
        "file" => {
            let path = file_url_to_path(file);
            std::fs::metadata(Path::new(&path))
                .map(|meta| meta.len() as i64)
                .unwrap_or(-1)
        }
        "jar" => parse_jar_spec(loader, file)
            .ok()
            .and_then(|(jar_path, entry)| {
                let archive = File::open(jar_path).ok()?;
                let mut zip = zip::ZipArchive::new(archive).ok()?;
                let entry = percent_decode(entry);
                zip.by_name(&entry).ok().map(|e| e.size() as i64)
            })
            .unwrap_or(-1),
        "jrt" => loader
            .bootImage
            .as_ref()
            .and_then(|image| {
                let mut image = image.borrow_mut();
                image.read_resource(file).ok().map(|b| b.len() as i64)
            })
            .unwrap_or(-1),
        _ => -1,
    }
}

/// Wraps `bytes` in a real `java.io.ByteArrayInputStream`.
fn byte_array_input_stream<'a>(
    loader: &ClassLoader<'a>,
    bytes: &[u8],
) -> io::Result<Value<'a>> {
    let klass = loader.load(b"java/io/ByteArrayInputStream")?;
    let _ = bytecodeInterpreter::initialize(loader, &klass, &mut sink(), 0);
    let mut object = InstanceOop::new(klass.clone())?;
    let array = loader.alloc_type_array(TypeArrayOop::Byte(
        bytes.iter().map(|&b| b as i8).collect(),
    ));
    object.fields[klass.instanceField(b"buf", b"[B")?] = Value::PrimitiveArray(array);
    object.fields[klass.instanceField(b"pos", b"I")?] = Value::Int(0);
    object.fields[klass.instanceField(b"mark", b"I")?] = Value::Int(0);
    object.fields[klass.instanceField(b"count", b"I")?] =
        Value::Int(bytes.len().min(i32::MAX as usize) as i32);
    Ok(Value::Object(loader.alloc_object(object)))
}

// ---------------------------------------------------------------------------
// Resource enumeration (ClassLoader.getResources)
// ---------------------------------------------------------------------------

/// Builds the URL spec strings for every classpath/boot resource named `name`,
/// in loader delegation order (boot image first, then classpath sources).
fn resource_specs<'a>(loader: &ClassLoader<'a>, name: &str) -> io::Result<Vec<String>> {
    let mut specs = Vec::new();
    if let Some(image) = &loader.bootImage {
        let image = image.borrow();
        specs.extend(image.matchingResources(name).map(|path| format!("jrt:{path}")));
    }
    let mut sources = loader.sources.borrow_mut();
    for source in sources.iter_mut() {
        match source {
            ClassSource::Jar { archive: zip, path } => {
                if zip.by_name(name).is_ok() {
                    specs.push(format!("jar:{}!/{}", path_to_file_url(path), encodeUrlPath(name)));
                }
            }
            ClassSource::Directory(dir) => {
                let file = dir.join(name);
                if file.is_file() {
                    specs.push(path_to_file_url(&file));
                }
            }
        }
    }
    Ok(specs)
}

pub(crate) fn classResource<'a>(
    loader: &ClassLoader<'a>,
    class: &[u8],
    value: Value<'a>,
) -> io::Result<Value<'a>> {
    if matches!(value, Value::Null) {
        return Err(invalid("NullPointerException"));
    }
    let name = stringNatives::string(loader, value)?.ok_or_else(|| invalid("Expected resource name"))?;
    let name = if let Some(absolute) = name.strip_prefix('/') {
        absolute.to_owned()
    } else {
        let mut component = class;
        while let Some(rest) = component.strip_prefix(b"[") { component = rest; }
        if let Some(rest) = component.strip_prefix(b"L").and_then(|value| value.strip_suffix(b";")) {
            component = rest;
        }
        let prefix = component.iter().rposition(|byte| *byte == b'/')
            .map(|end| &component[..end + 1]).unwrap_or_default();
        format!("{}{}", crate::classfile::decodeString(prefix)?, name)
    };
    let Some(spec) = resource_specs(loader, &name)?.into_iter().next() else {
        return Ok(Value::Null);
    };
    let klass = loader.load(b"java/net/URL")?;
    bytecodeInterpreter::initialize(loader, &klass, &mut sink(), 0)?;
    Ok(Value::Object(make_url(loader, &klass, &spec)?))
}

/// Constructs a real `java.net.URL` for `spec` through `URL.<init>(String)`.
fn make_url<'a>(
    loader: &ClassLoader<'a>,
    url_klass: &Arc<InstanceKlass<'a>>,
    spec: &str,
) -> io::Result<usize> {
    let object = InstanceOop::new(url_klass.clone())?;
    let handle = loader.alloc_object(object);
    let materialized = stringNatives::materialize(
        loader,
        Value::HostString(loader.storage.alloc_str(spec)),
        &mut sink(),
        0,
    )?
    .unwrap_or(Value::HostString(loader.storage.alloc_str(spec)));
    let mut arguments = JavaCallArguments::new();
    arguments.push_oop(Value::Object(handle));
    arguments.push_oop(materialized);
    JavaCalls::call_special(
        loader,
        url_klass,
        b"<init>",
        b"(Ljava/lang/String;)V",
        &arguments,
        &mut sink(),
    )?;
    Ok(handle)
}

/// Replacement for the `java/lang/ClassLoader.getResources` hook: returns a
/// `java.util.Enumeration` of `java.net.URL` objects for every matching
/// resource. `arguments[0]` is the ClassLoader receiver, `arguments[1]` the
/// resource name.
pub(crate) fn get_resources<'a>(
    loader: &ClassLoader<'a>,
    arguments: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let name = arguments
        .get(1)
        .copied()
        .and_then(|value| stringNatives::string(loader, value).ok().flatten())
        .ok_or_else(|| invalid("Invalid resource name"))?
        .into_owned();
    let specs = resource_specs(loader, &name)?;
    if std::env::var_os("JVM25_TRACE_URL").is_some() {
        eprintln!("[getResources] name={name} specs={}", specs.len());
    }

    let url_klass = loader.load(b"java/net/URL")?;
    let _ = bytecodeInterpreter::initialize(loader, &url_klass, &mut sink(), 0);

    let mut urls = Vec::with_capacity(specs.len());
    for spec in &specs {
        urls.push(make_url(loader, &url_klass, spec)?);
    }

    let list_klass = loader.load(b"java/util/ArrayList")?;
    let _ = bytecodeInterpreter::initialize(loader, &list_klass, &mut sink(), 0);
    let list = InstanceOop::new(list_klass.clone())?;
    let list_handle = loader.alloc_object(list);
    let mut arguments = JavaCallArguments::new();
    arguments.push_oop(Value::Object(list_handle));
    JavaCalls::call_special(
        loader,
        &list_klass,
        b"<init>",
        b"()V",
        &arguments,
        &mut sink(),
    )?;
    for url in urls {
        let mut arguments = JavaCallArguments::new();
        arguments.push_oop(Value::Object(list_handle));
        arguments.push_oop(Value::Object(url));
        let _ = JavaCalls::call_virtual(
            loader,
            &list_klass,
            b"add",
            b"(Ljava/lang/Object;)Z",
            &arguments,
            &mut sink(),
        )?;
    }

    let collections = loader.load(b"java/util/Collections")?;
    let _ = bytecodeInterpreter::initialize(loader, &collections, &mut sink(), 0);
    let mut arguments = JavaCallArguments::new();
    arguments.push_oop(Value::Object(list_handle));
    let enumeration = JavaCalls::call_static(
        loader,
        &collections,
        b"enumeration",
        b"(Ljava/util/Collection;)Ljava/util/Enumeration;",
        &arguments,
        &mut sink(),
    )?;
    Ok(Some(enumeration.unwrap_or(Value::Null)))
}


// ---------------------------------------------------------------------------
// Dispatch entry points (wired into nativeLookup by the orchestrator)
// ---------------------------------------------------------------------------

/// Instance-method interception. Handles:
///  * `java/net/URL.openConnection()Ljava/net/URLConnection;` and the
///    `Proxy` overload for `jar:`/`jrt:` URLs (returns `None` for other
///    protocols so the real JDK handlers run, e.g. `file:`).
///  * `jvm25/url/UrlConnection.open0/length0` (native backing of the
///    synthetic connection).
///
/// `java.io.FileInputStream`/`FileDescriptor` natives are provided by
/// `fileNatives`/`nioFileNatives` (parallel work).
pub(crate) fn invoke_instance<'a>(
    loader: &ClassLoader<'a>,
    owner: &[u8],
    name: &[u8],
    descriptor: &[u8],
    arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    match (owner, name, descriptor) {
        (b"java/lang/ClassLoader", b"getResources", b"(Ljava/lang/String;)Ljava/util/Enumeration;") => {
            // Only intercept for the VM's own application/system loader,
            // which is backed by the flat class store (jimage + classpath
            // sources). Custom Java loaders (URLClassLoader & friends) must
            // run their real `getResources` so their `findResources`/
            // `URLClassPath` can supply jar resources — the symbolic owner
            // is `java/lang/ClassLoader` for every subclass call, so the
            // receiver must be checked explicitly.
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            let is_vm_loader = match receiver {
                Value::Object(handle) => {
                    let system = loader.systemClassLoaderObject().unwrap_or(usize::MAX);
                    let registered = *loader.class_loader_object_handle().borrow();
                    handle == system || Some(handle) == registered
                }
                _ => false,
            };
            if !is_vm_loader {
                return Ok(None);
            }
            get_resources(loader, arguments).map(|result| result.map(Some))
        }
        (b"java/net/URL", b"openConnection", b"()Ljava/net/URLConnection;")
        | (b"java/net/URL", b"openConnection", b"(Ljava/net/Proxy;)Ljava/net/URLConnection;") => {
            let url = arguments.first().copied().unwrap_or(Value::Null);
            if !matches!(url, Value::Object(_)) {
                return Ok(None);
            }
            match url_protocol(loader, url)? {
                Some(protocol) if protocol == "jar" || protocol == "jrt" => {
                    connection_object(loader, url).map(|value| Some(Some(value)))
                }
                _ => Ok(None),
            }
        }
        (b"jvm25/url/UrlConnection", b"open0", b"()Ljava/io/InputStream;") => {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            let url = object_field(loader, receiver, b"url", b"Ljava/net/URL;")?;
            let (protocol, file) = url_parts(loader, url)?;
            let (Some(protocol), Some(file)) = (protocol, file) else {
                return Err(io_exception(loader, "URLConnection has no URL"));
            };
            let bytes = read_url_bytes(loader, &protocol, &file)?;
            byte_array_input_stream(loader, &bytes).map(|value| Some(Some(value)))
        }
        (b"jvm25/url/UrlConnection", b"length0", b"()J") => {
            let receiver = arguments.first().copied().unwrap_or(Value::Null);
            let url = object_field(loader, receiver, b"url", b"Ljava/net/URL;")?;
            let (protocol, file) = url_parts(loader, url)?;
            let length = match (protocol, file) {
                (Some(protocol), Some(file)) => url_content_length(loader, &protocol, &file),
                _ => -1,
            };
            Ok(Some(Some(Value::Long(length))))
        }
        _ => Ok(None),
    }
}


