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
        ("java.io.tmpdir".into(), format!("{java_home}\\temp")),
        ("sun.jnu.encoding".into(), "Cp1252".into()),
        ("native.encoding".into(), "Cp1252".into()),
        ("file.encoding".into(), "UTF-8".into()),
        ("user.timezone".into(), env("TZ")),
    ]
}

fn string(value: Value<'_>) -> io::Result<Cow<'_, str>> {
    match value {
        Value::HostString(value) => Ok(Cow::Borrowed(value)),
        Value::String(bytes) => decodeString(bytes).map(Cow::Owned),
        Value::Null => Err(Error::other("NullPointerException")),
        _ => Err(Error::other("Expected java.lang.String")),
    }
}
