use jvm25_rust::classfile::ClassFile;
use jvm25_rust::share::{classfile::classLoader::ClassLoader, runtime::javaCalls};
use std::{env, error::Error, fs, io, path::Path, process::ExitCode};

/// Windows vectored exception handler: on access violations prints the
/// faulting RIP, the accessed address, and — when the RIP lands inside a
/// JIT nmethod — the compiled method, then continues search so the
/// process still dies with the original exception.
#[cfg(windows)]
mod crash_handler {
    use std::ffi::c_void;

    const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        next: *mut c_void,
        address: *mut c_void,
        n_params: u32,
        _align: u32,
        params: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        record: *const ExceptionRecord,
        context: *mut c_void,
    }

    unsafe extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
        unsafe {
            if pointers.is_null() {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            let record_ptr = (*pointers).record;
            if record_ptr.is_null() {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            let record = &*record_ptr;
            if record.code != EXCEPTION_ACCESS_VIOLATION {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            let rip = record.address as usize;
            let access = record.params.get(1).copied().unwrap_or(0);
            let rw = record.params.first().copied().unwrap_or(0);
            let cache = jvm25_rust::share::code::codeCache::CodeCache::global();
            let where_ = if let Some(nm) = cache.find_nmethod_at_address(rip) {
                format!(
                    "nmethod {}.{}{} entry={:#x}",
                    nm.method_holder(),
                    nm.method_name(),
                    nm.method_signature(),
                    nm.entry_point()
                )
            } else if let Some(blob) = cache.find_blob_at_address(rip) {
                format!("codeblob {blob:?}")
            } else {
                "non-jit code".to_string()
            };
            eprintln!(
                "[crash] AV rip={rip:#x} {}={access:#x} -> {where_}",
                if rw == 0 { "read" } else { "write" }
            );
            let _ = std::fs::write(
                r"D:\jvm25-rust\crash.log",
                format!("rip={rip:#x} rw={rw} access={access:#x} {where_}\n"),
            );
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    pub fn install() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AddVectoredExceptionHandler(first: i32, handler: *const c_void) -> *mut c_void;
        }
        unsafe {
            AddVectoredExceptionHandler(1, handler as *const c_void);
        }
    }
}

fn inspect() -> Result<(), Box<dyn Error>> {
    // Panic-free thread identity for JNI callbacks arriving after TLS
    // destruction (native shutdown hooks).
    jvm25_rust::share::prims::jniReferences::init_shutdown_thread_id();
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
        eprintln!(
            "[env] TRACE_LABELED={:?} NO_JIT={:?} TRACE_NATIVE={:?}",
            std::env::var_os("JVM25_TRACE_LABELED").is_some(),
            std::env::var_os("JVM25_NO_JIT").is_some(),
            std::env::var_os("JVM25_TRACE_NATIVE").is_some()
        );
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
    jvm25_rust::share::runtime::threading::wait_non_daemon(loader.as_ref());
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
    #[cfg(windows)]
    crash_handler::install();
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
            // Never touch std::thread::current() here: it panics after a
            // thread's TLS was destroyed, which would panic inside this hook.
            let _ = writeln!(file, "[panic] pid={} at {location}: {message}", std::process::id());
            let _ = writeln!(file, "{}", std::backtrace::Backtrace::force_capture());
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
        .spawn(move || {
            jvm25_rust::share::runtime::threading::install_stack_bounds(
                stack_mb * 1024 * 1024,
            );
            inspect().map_err(|error| error.to_string())
        });
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
