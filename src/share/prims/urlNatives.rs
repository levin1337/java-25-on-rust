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
    rc::Rc,
};

/// Internal name of the synthetic `java.net.URLConnection` subclass embedded
/// below (`src/share/prims/url_connection.class`, compiled from
/// `url_connection.java`).
const URL_CONNECTION_CLASS: &[u8] = b"jvm25/url/UrlConnection";

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

/// Writes an instance field of an object value.
fn set_object_field<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    name: &[u8],
    descriptor: &[u8],
    value: Value<'a>,
) -> io::Result<()> {
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
    let mut objects = loader.borrow_objects_mut();
    let target = objects
        .get_mut(handle)
        .ok_or_else(|| invalid("Invalid object reference"))?;
    if slot < target.fields.len() {
        target.fields[slot] = value;
    }
    Ok(())
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
) -> io::Result<Rc<InstanceKlass<'a>>> {
    if let Ok(klass) = loader.load(URL_CONNECTION_CLASS) {
        return Ok(klass);
    }
    loader.define_class_bytes(URL_CONNECTION_CLASS, include_bytes!("url_connection.class"))?;
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
        return format!("file:{url}");
    }
    if !url.starts_with('/') {
        // "C:/..." -> "/C:/..."
        url.insert(0, '/');
    }
    format!("file:{url}")
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

/// Extracts the resource name from a jimage path "/module/rest/of/path".
fn jimage_resource_name(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let (_, name) = rest.split_once('/')?;
    Some(name.to_owned())
}

/// Builds the URL spec strings for every classpath/boot resource named `name`,
/// in loader delegation order (boot image first, then classpath sources).
fn resource_specs<'a>(loader: &ClassLoader<'a>, name: &str) -> io::Result<Vec<String>> {
    let mut specs = Vec::new();
    if let Some(image) = &loader.bootImage {
        let image = image.borrow();
        let mut matches: Vec<String> = image
            .resource_paths()
            .filter(|path| jimage_resource_name(path).as_deref() == Some(name))
            .map(str::to_owned)
            .collect();
        matches.sort_by(|a, b| {
            let a_base = a.starts_with("/java.base/");
            let b_base = b.starts_with("/java.base/");
            b_base.cmp(&a_base).then_with(|| a.cmp(b))
        });
        specs.extend(matches.into_iter().map(|path| format!("jrt:{path}")));
    }
    let mut sources = loader.sources.borrow_mut();
    for source in sources.iter_mut() {
        match source {
            ClassSource::Jar { archive: zip, path } => {
                for index in 0..zip.len() {
                    if let Ok(entry) = zip.by_index(index)
                        && entry.name().ok().is_some_and(|entry| entry == name)
                    {
                        specs.push(format!("jar:{}!/{name}", path_to_file_url(path)));
                    }
                }
            }
            ClassSource::Directory(dir) => {
                if dir.join(name).is_file() {
                    specs.push(format!("{}/{name}", path_to_file_url(dir)));
                }
            }
        }
    }
    Ok(specs)
}

/// Constructs a real `java.net.URL` for `spec` through `URL.<init>(String)`.
fn make_url<'a>(
    loader: &ClassLoader<'a>,
    url_klass: &Rc<InstanceKlass<'a>>,
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

/// Static-method interception (currently unused; kept for symmetric wiring).
pub(crate) fn invoke_static<'a>(
    _loader: &ClassLoader<'a>,
    _owner: &[u8],
    _name: &[u8],
    _descriptor: &[u8],
    _arguments: &[Value<'a>],
) -> io::Result<Option<Option<Value<'a>>>> {
    Ok(None)
}
