use jvm25_rust::classfile::ClassFile;
use jvm25_rust::share::{classfile::classLoader::ClassLoader, runtime::javaCalls};
use std::{env, error::Error, fs, io, path::Path, process::ExitCode};

fn inspect() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1).peekable();
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
    while arguments
        .peek()
        .is_some_and(|arg| arg.to_str().is_some_and(|arg| arg.starts_with("-D")))
    {
        let option = arguments.next().unwrap();
        let option = option
            .to_str()
            .ok_or_else(|| io::Error::other("Invalid system property"))?;
        let (key, value) = option[2..].split_once('=').unwrap_or((&option[2..], ""));
        if key.is_empty() {
            return Err(io::Error::other("Empty system property name").into());
        }
        properties.push((key.to_owned(), value.to_owned()));
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

    let storage = bumpalo::Bump::new();
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
        (
            ClassLoader::open_classpath(classpath, &storage)?,
            name.replace('.', "/"),
        )
    };
    if let Some(modules) = modules {
        loader = loader.withBootImage(Path::new(&modules))?;
    }
    for (key, value) in properties {
        loader.setSystemProperty(&key, &value)?;
    }
    let arguments: Vec<_> = arguments
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    javaCalls::mainClass(&loader, &name, &arguments, &mut io::stdout().lock())?;
    Ok(())
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
    // The bytecode interpreter recurses per Java call; give it a large stack
    // like a real JVM thread would have.
    let worker = std::thread::Builder::new()
        .stack_size(1024 * 1024 * 1024)
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
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
