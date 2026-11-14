use crate::share::{
    classfile::{classFileParser::decodeString, classLoader::ClassLoader},
    interpreter::bytecodeInterpreter::Value,
};
use std::{
    borrow::Cow,
    io::{self, Error},
};

pub(crate) fn invoke<'a>(
    loader: &ClassLoader<'a>,
    name: &[u8],
    descriptor: &[u8],
    args: &[Value<'a>],
) -> io::Result<Option<Value<'a>>> {
    let previous = match (name, descriptor, args) {
        (
            b"setProperty",
            b"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
            [key, value],
        ) => {
            let key = propertyKey(*key)?.into_owned();
            let value = match value {
                Value::HostString(value) => *value,
                _ => loader.storage.alloc_str(&string(*value)?),
            };
            loader.systemProperties.borrow_mut().insert(key, value)
        }
        (b"getProperty", b"(Ljava/lang/String;)Ljava/lang/String;", [key])
        | (b"getProperty", b"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", [key, _]) => {
            loader
                .systemProperties
                .borrow()
                .get(propertyKey(*key)?.as_ref())
                .copied()
        }
        (b"clearProperty", b"(Ljava/lang/String;)Ljava/lang/String;", [key]) => loader
            .systemProperties
            .borrow_mut()
            .remove(propertyKey(*key)?.as_ref()),
        _ => return Ok(None),
    };
    let result = previous
        .map(Value::HostString)
        .unwrap_or_else(|| match (name, args) {
            (b"getProperty", [_, default]) => *default,
            _ => Value::Null,
        });
    Ok(Some(result))
}

fn propertyKey(value: Value<'_>) -> io::Result<Cow<'_, str>> {
    let key = string(value)?;
    if key.is_empty() {
        return Err(Error::other("IllegalArgumentException: empty property key"));
    }
    Ok(key)
}

/// Platform properties that HotSpot exposes through
/// `jdk.internal.util.SystemProps.Raw.platformProperties()`.
/// User-supplied `-D` entries are NOT included here; callers merge
/// them on top.
pub(crate) fn platform_properties(loader: &ClassLoader<'_>) -> Vec<(String, String)> {
    let env = |key: &str| std::env::var(key).unwrap_or_default();
    let arch = match env("PROCESSOR_ARCHITECTURE")
        .to_ascii_uppercase()
        .as_str()
    {
        "AMD64" => "amd64",
        "ARM64" => "aarch64",
        "X86" => "x86",
        _ => "unknown",
    };
    let home = env("USERPROFILE");
    let user = env("USERNAME");
    let dir = std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let java_home = loader
        .systemProperties
        .borrow()
        .get("java.home")
        .map(|value| (*value).to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| env("JAVA_HOME"));
    vec![
        ("java.vendor".into(), "Oracle Corporation".into()),
        ("java.vendor.url".into(), "https://java.oracle.com/".into()),
        ("java.version".into(), "25".into()),
        ("java.version.date".into(), "2025-09-16".into()),
        ("java.class.version".into(), "69.0".into()),
        ("java.runtime.version".into(), "25+36-3489".into()),
        ("java.specification.version".into(), "25".into()),
        (
            "java.specification.vendor".into(),
            "Oracle Corporation".into(),
        ),
        (
            "java.specification.name".into(),
            "Java Platform API Specification".into(),
        ),
        ("java.vm.version".into(), "25+36-3489".into()),
        ("java.vm.vendor".into(), "Oracle Corporation".into()),
        ("java.vm.name".into(), "OpenJDK 64-Bit Server VM".into()),
        ("java.vm.specification.version".into(), "25".into()),
        (
            "java.vm.specification.vendor".into(),
            "Oracle Corporation".into(),
        ),
        (
            "java.vm.specification.name".into(),
            "Java Virtual Machine Specification".into(),
        ),
        ("java.awt.headless".into(), "true".into()),
        ("os.name".into(), "Windows 11".into()),
        ("os.arch".into(), arch.into()),
        ("os.version".into(), "10.0".into()),
        ("file.separator".into(), "\\".into()),
        ("path.separator".into(), ";".into()),
        ("line.separator".into(), "\r\n".into()),
        ("user.name".into(), user),
        ("user.home".into(), home),
        ("user.dir".into(), dir),
        ("java.home".into(), java_home.clone()),
        (
            "java.io.tmpdir".into(),
            ["TEMP", "TMP"]
                .iter()
                .filter_map(|key| std::env::var(key).ok())
                .find(|value| !value.is_empty())
                .unwrap_or_else(|| format!("{java_home}\\temp")),
        ),
        ("sun.jnu.encoding".into(), "Cp1252".into()),
        ("native.encoding".into(), "Cp1252".into()),
        ("file.encoding".into(), "UTF-8".into()),
        ("user.timezone".into(), env("TZ")),
    ]
}

/// Positional `platformProperties()` array consumed by
/// `jdk.internal.util.SystemProps.Raw.propDefault(NDX)`. The order is fixed
/// (alphabetical by property concept) and must match the `@Native` NDX
/// constants in `SystemProps.java`; `FIXED_LENGTH` is 39. Elements are the
/// property *values* (or null when unset) — not key/value pairs.
pub(crate) fn platform_property_slots(loader: &ClassLoader<'_>) -> Vec<Option<String>> {
    let env = |key: &str| std::env::var(key).unwrap_or_default();
    let java_home = loader
        .systemProperties
        .borrow()
        .get("java.home")
        .map(|value| (*value).to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| env("JAVA_HOME"));
    let non_empty = |key: &str| {
        std::env::var(key).ok().filter(|value| !value.is_empty())
    };
    // JDK 25 moved `file.encoding` out of the positional array, added
    // `native.encoding` after `line.separator` and `stdin.encoding` before
    // `stdout.encoding`. Detect the boot JDK from the class-file version of
    // java.lang.Object (JDK 21 = 65, JDK 25 = 69).
    let jdk25 = loader
        .load(b"java/lang/Object")
        .map(|klass| klass.class.majorVersion >= 67)
        .unwrap_or(false);
    let tmpdir = non_empty("TEMP")
        .or_else(|| non_empty("TMP"))
        .unwrap_or_else(|| format!("{java_home}\\temp"));
    let user_dir = std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let display = [
        Some("US".into()),      // display_country
        Some("en".into()),      // display_language
        None,                   // display_script
        None,                   // display_variant
    ];
    let format = [
        Some("US".into()),      // format_country
        Some("en".into()),      // format_language
        None,                   // format_script
        None,                   // format_variant
    ];
    let proxies = [None, None, None, None, None, None, None, None]; // ftp, http, https
    let socks = [None, None, None];
    let encodings = [
        Some("Cp1252".into()), // stderr_encoding
        Some("Cp1252".into()), // stdout_encoding
    ];
    let tail = [
        Some("Windows".into()),        // sun_arch_abi
        Some("64".into()),             // sun_arch_data_model
        Some("little".into()),         // sun_cpu_endian
        None,                          // sun_cpu_isalist
        Some("UnicodeLittle".into()),  // sun_io_unicode_encoding
        Some("Cp1252".into()),         // sun_jnu_encoding
        Some("10.0".into()),           // sun_os_patch_level
        Some(user_dir),                // user_dir
        non_empty("USERPROFILE"),      // user_home
        non_empty("USERNAME"),         // user_name
    ];
    let mut slots: Vec<Option<String>> = Vec::with_capacity(40);
    slots.extend(display);
    if !jdk25 {
        slots.push(Some("UTF-8".into())); // 4 file_encoding (JDK <= 24)
    }
    slots.push(Some("\\".into())); // file_separator
    slots.extend(format);
    slots.extend(proxies);
    slots.push(Some(tmpdir)); // java_io_tmpdir
    slots.push(Some("\r\n".into())); // line_separator
    if jdk25 {
        slots.push(Some("Cp1252".into())); // 19 native_encoding
    }
    slots.push(Some(platform_properties_arch())); // os_arch
    slots.extend([
        Some("Windows 11".into()), // os_name
        Some("10.0".into()),       // os_version
        Some(";".into()),          // path_separator
    ]);
    slots.extend(socks); // socksNonProxyHosts, socksProxyHost, socksProxyPort
    if jdk25 {
        // socks proxies already pushed above; JDK25 inserts stdin_encoding
        // between stderr and stdout.
        slots.push(Some("Cp1252".into())); // stderr_encoding
        slots.push(Some("Cp1252".into())); // stdin_encoding
        slots.push(Some("Cp1252".into())); // stdout_encoding
    } else {
        slots.extend(encodings);
    }
    slots.extend(tail);
    slots
}

fn platform_properties_arch() -> String {
    match std::env::var("PROCESSOR_ARCHITECTURE")
        .unwrap_or_default()
        .to_ascii_uppercase()
        .as_str()
    {
        "AMD64" => "amd64",
        "ARM64" => "aarch64",
        "X86" => "x86",
        _ => "unknown",
    }
    .to_owned()
}

fn string(value: Value<'_>) -> io::Result<Cow<'_, str>> {
    match value {
        Value::HostString(value) => Ok(Cow::Borrowed(value)),
        Value::String(bytes) => decodeString(bytes).map(Cow::Owned),
        Value::Null => Err(Error::other("NullPointerException")),
        _ => Err(Error::other("Expected java.lang.String")),
    }
}
