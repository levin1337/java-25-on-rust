use jvm25_rust::classfile::ClassFile;
use jvm25_rust::share::{classfile::classLoader::ClassLoader, runtime::javaCalls};
use std::{env, error::Error, fs, io, path::Path, process::ExitCode};

fn inspect() -> Result<(), Box<dyn Error>> {
    let mut raw: Vec<std::ffi::OsString> = env::args_os().skip(1).collect();
    // Java launcher @argument files: `@file` expands to the whitespace-
    // separated tokens inside (double quotes group, `\"`/`\\` escape).
    // `@@` yields a literal `@` argument.
    let mut expanded: Vec<std::ffi::OsString> = Vec::with_capacity(raw.len());
    for arg in raw.drain(..) {
        let text = arg.to_string_lossy();
        if text.starts_with("@@") {
            expanded.push(std::ffi::OsString::from(&text[1..]));
        } else if let Some(path) = text.strip_prefix('@')
            && !path.is_empty()
        {
            let contents = fs::read_to_string(path)
                .map_err(|e| io::Error::other(format!("Cannot read argument file {path}: {e}")))?;
            for token in parse_argfile(&contents) {
                expanded.push(std::ffi::OsString::from(token));
            }
        } else {
            expanded.push(arg);
        }
    }
    let mut arguments = expanded.into_iter().peekable();
    let modules = if arguments.next_if_eq("--modules").is_some() {
        Some(
            arguments
                .next()
                .ok_or_else(|| io::Error::other("Missing modules image path"))?,
        )
    } else {
        None
    };
    let mut properties = Vec::new();
    loop {
        let Some(peek) = arguments.peek() else { break };
        let Some(text) = peek.to_str() else { break };
        if text.starts_with("-D") {
            let option = arguments.next().unwrap();
            let option = option
                .to_str()
                .ok_or_else(|| io::Error::other("Invalid system property"))?;
            let (key, value) = option[2..].split_once('=').unwrap_or((&option[2..], ""));
            if key.is_empty() {
                return Err(io::Error::other("Empty system property name").into());
            }
            properties.push((key.to_owned(), value.to_owned()));
        } else if (text.starts_with("-X") || text.starts_with("--")) && text != "--class-path" {
            let option = arguments.next().unwrap();
            if option.to_str().is_some_and(|arg| {
                arg == "--add-exports" || arg == "--add-opens" || arg == "--patch-module"
            }) {
                let _ = arguments.next();
            }
        } else {
            break;
        }
    }
    let target = arguments.next().ok_or_else(|| io::Error::other(
        "Usage: jvm25-rust [--modules <image>] [-cp <path>] <main-class> [args...] | -jar <file> [args...] | <class-file>",
    ))?;
    if modules.is_none()
        && arguments.len() == 0
        && Path::new(&target)
            .extension()
            .is_some_and(|extension| extension == "class")
    {
        return inspectClass(Path::new(&target));
    }

    let storage: &'static jvm25_rust::share::runtime::syncBump::SyncBump = Box::leak(Box::new(
        jvm25_rust::share::runtime::syncBump::SyncBump::new(),
    ));
    let (mut loader, name) = if target == "-jar" {
        let path = arguments
            .next()
            .ok_or_else(|| io::Error::other("Missing JAR path"))?;
        let loader = ClassLoader::open(Path::new(&path), &storage)?;
        let name = loader.jarMain()?;
        (loader, name)
    } else {
        let (classpath, name) =
            if target == "-cp" || target == "-classpath" || target == "--class-path" {
                let classpath = arguments
                    .next()
                    .ok_or_else(|| io::Error::other("Missing classpath"))?;
                let name = arguments
                    .next()
                    .ok_or_else(|| io::Error::other("Missing main class"))?;
                (classpath, name)
            } else {
                (
                    env::var_os("CLASSPATH").unwrap_or_else(|| ".".into()),
                    target,
                )
            };
        let classpath = classpath
            .to_str()
            .ok_or_else(|| io::Error::other("Classpath must be Unicode"))?;
        let name = name
            .to_str()
            .filter(|name| !name.is_empty() && !name.starts_with('-'))
            .ok_or_else(|| io::Error::other("Invalid main class"))?;
        eprintln!("[args] name={name:?} classpath={classpath:?}");
        let classpath = expand_classpath_wildcards(classpath);
        let mut loader = ClassLoader::open_classpath(&classpath, &storage)?;
        loader.setSystemProperty("java.class.path", &classpath)?;
        (loader, name.replace('.', "/"))
    };
    if let Some(modules) = modules {
        // Derive java.home from the modules image location
        // (<java.home>/lib/modules) so JDK code that resolves files
        // relative to the runtime (conf/security/java.security, lib,
        // ...) sees a consistent layout.
        if let Some(java_home) = Path::new(&modules).parent().and_then(|lib| lib.parent()) {
            loader.setSystemProperty("java.home", &java_home.to_string_lossy())?;
        }
        loader = loader.withBootImage(Path::new(&modules))?;
    }
    for (key, value) in properties {
        loader.setSystemProperty(&key, &value)?;
    }
    let arguments: Vec<_> = arguments
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let loader = std::sync::Arc::new(loader);
    jvm25_rust::share::runtime::threading::install_loader(loader.clone());
    let arguments: &'static [String] = Box::leak(arguments.into_boxed_slice());
    let mut out = jvm25_rust::share::runtime::jvmOut::JvmOut::stdout();
    jvm25_rust::share::runtime::jvmOut::install(&out);
    javaCalls::mainClass(loader.as_ref(), &name, arguments, &mut out)?;
    Ok(())
}

/// Expands `dir/*` classpath elements into the sorted list of JAR files they
/// contain, matching `java` launcher semantics. The expanded form is what the
/// JDK's own `URLClassPath` sees via `java.class.path`, so entries must not
/// retain the literal `*` (Win32 canonicalization rejects it).
fn expand_classpath_wildcards(classpath: &str) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let mut expanded = Vec::new();
    for entry in classpath.split(separator) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let path = Path::new(entry);
        if path.file_name().is_some_and(|name| name == "*") {
            if let Some(dir) = path.parent()
                && let Ok(entries) = fs::read_dir(dir)
            {
                let mut jars: Vec<_> = entries
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
                    })
                    .collect();
                jars.sort();
                expanded.extend(jars.iter().map(|jar| jar.to_string_lossy().into_owned()));
            }
        } else {
            expanded.push(entry.to_owned());
        }
    }
    expanded.join(&separator.to_string())
}

/// Splits an `@argument` file into tokens. Whitespace separates tokens;
/// `"..."` groups whitespace, `\"` and `\\` are escapes, `#` at the start
/// of a line comments out the rest of the line.
fn parse_argfile(contents: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = contents.chars().peekable();
    let mut quoted = false;
    let mut token_started = false;
    let mut line_start = true;
    while let Some(ch) = chars.next() {
        if line_start && ch == '#' {
            while let Some(c) = chars.next() {
                if c == '\n' {
                    break;
                }
            }
            line_start = true;
            continue;
        }
        line_start = ch == '\n';
        match ch {
            '"' => {
                quoted = !quoted;
                token_started = true;
            }
            '\\' => match chars.peek().copied() {
                Some('"') | Some('\\') => {
                    token.push(chars.next().unwrap());
                    token_started = true;
                }
                _ => {
                    token.push('\\');
                    token_started = true;
                }
            },
            c if c.is_whitespace() && !quoted => {
                if token_started || !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                    token_started = false;
                }
            }
            c => {
                token.push(c);
                token_started = true;
            }
        }
    }
    if token_started || !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn inspectClass(path: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let class = ClassFile::parse(&bytes, false)?;
    println!(
        "{} ({}.{}) | {} fields | {} methods",
        String::from_utf8_lossy(class.name),
        class.majorVersion,
        class.minorVersion,
        class.fields.len(),
        class.methods.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic".to_string());
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(r"D:\jvm25-rust\panic.log")
        {
            use std::io::Write;
            let _ = writeln!(
                file,
                "[panic] tid={:?} at {location}: {message}",
                std::thread::current().id()
            );
        }
        eprintln!("thread panicked at {location}: {message}");
    }));
    // The bytecode interpreter recurses per Java call; give it a large stack
    // like a real JVM thread would have. JVM25_STACK_MB overrides the size
    // for memory-constrained machines.
    let stack_mb: usize = std::env::var("JVM25_STACK_MB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1024);
    let worker = std::thread::Builder::new()
        .stack_size(stack_mb * 1024 * 1024)
        .spawn(|| inspect().map_err(|error| error.to_string()));
    let result = match worker {
        Ok(handle) => handle.join().unwrap_or_else(|_| {
            eprintln!("Worker thread panicked");
            Err("Worker thread panicked".to_owned())
        }),
        Err(_) => inspect().map_err(|error| error.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[exit-err] len={} {:?}", error.len(), error);
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
