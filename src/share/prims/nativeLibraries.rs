use crate::share::interpreter::bytecodeInterpreter::Value;
use libloading::Library;
use std::{
    collections::HashMap,
    io::{self, Error},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

pub type NativeFunction = fn(args: &[Value<'_>]) -> Option<Value<'static>>;

pub struct NativeLibraries {
    loadedLibraries: Mutex<Vec<(PathBuf, Library)>>,
    searchPaths: Mutex<Vec<PathBuf>>,
    customSymbols: Mutex<HashMap<String, NativeFunction>>,
}

static nativeLibraries: OnceLock<NativeLibraries> = OnceLock::new();

impl NativeLibraries {
    pub fn new() -> Self {
        Self {
            loadedLibraries: Mutex::new(Vec::new()),
            searchPaths: Mutex::new(vec![
                PathBuf::from("."),
                PathBuf::from("bin"),
                PathBuf::from("lib"),
            ]),
            customSymbols: Mutex::new(HashMap::new()),
        }
    }

    pub fn global() -> &'static Self {
        nativeLibraries.get_or_init(Self::new)
    }

    pub fn add_search_path<P: AsRef<Path>>(&self, path: P) {
        self.searchPaths
            .lock()
            .unwrap()
            .push(path.as_ref().to_path_buf());
    }

    pub fn load_library(&self, name: &str) -> io::Result<()> {
        self.loadLibraryWithPaths(name, None)
    }

    pub fn loadLibraryWithPaths(
        &self,
        name: &str,
        configuredPaths: Option<&str>,
    ) -> io::Result<()> {
        if name.is_empty() || name.contains(['/', '\\']) {
            return Err(Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid library name",
            ));
        }
        let filename = if cfg!(windows) {
            format!("{name}.dll")
        } else if cfg!(target_os = "macos") {
            format!("lib{name}.dylib")
        } else {
            format!("lib{name}.so")
        };
        let paths = configuredPaths
            .map(|value| std::env::split_paths(value).collect::<Vec<_>>())
            .unwrap_or_else(|| self.searchPaths.lock().unwrap().clone());
        for dir in paths {
            let path = dir.join(&filename);
            if path.is_file() {
                return self.loadPath(&path);
            }
        }
        Err(Error::new(
            io::ErrorKind::NotFound,
            format!("Native library not found: {name}"),
        ))
    }

    pub fn load(&self, path: &Path) -> io::Result<()> {
        if !path.is_absolute() {
            return Err(Error::new(
                io::ErrorKind::InvalidInput,
                "Native library path must be absolute",
            ));
        }
        self.loadPath(path)
    }

    fn loadPath(&self, path: &Path) -> io::Result<()> {
        let loadPath = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let path = loadPath.canonicalize()?;
        let mut libraries = self.loadedLibraries.lock().unwrap();
        if libraries.iter().any(|(loadedPath, _)| *loadedPath == path) {
            return Ok(());
        }
        let library = unsafe { Library::new(&loadPath) }.map_err(Error::other)?;
        if unsafe { library.get::<unsafe extern "system" fn()>(b"JNI_OnLoad") }.is_ok() {
            return Err(Error::new(
                io::ErrorKind::Unsupported,
                "JNI_OnLoad requires a compatible JavaVM interface",
            ));
        }
        libraries.push((path, library));
        Ok(())
    }

    pub fn hasSymbol(&self, symbol: &str) -> io::Result<bool> {
        if symbol.as_bytes().contains(&0) {
            return Err(Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid symbol name",
            ));
        }
        for (_, library) in self.loadedLibraries.lock().unwrap().iter() {
            if unsafe { library.get::<unsafe extern "system" fn()>(symbol.as_bytes()) }.is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn resolveJniSymbol(
        &self,
        className: &str,
        methodName: &str,
        signature: &str,
    ) -> io::Result<Option<String>> {
        let lookup = super::nativeLookup::NativeLookup::pure_jni_name(className, methodName);
        let overloaded =
            super::nativeLookup::NativeLookup::long_jni_name(className, methodName, signature);
        if self.hasSymbol(&lookup)? {
            return Ok(Some(lookup));
        }
        if self.hasSymbol(&overloaded)? {
            return Ok(Some(overloaded));
        }
        Ok(None)
    }

    pub fn register_symbol(&self, symbol_name: &str, function: NativeFunction) {
        self.customSymbols
            .lock()
            .unwrap()
            .insert(symbol_name.to_owned(), function);
    }

    pub fn dispatch(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
        arguments: &[Value<'_>],
    ) -> Option<Value<'static>> {
        let short = super::nativeLookup::NativeLookup::pure_jni_name(class_name, method_name);
        let long =
            super::nativeLookup::NativeLookup::long_jni_name(class_name, method_name, signature);
        let function = {
            let symbols = self.customSymbols.lock().unwrap();
            symbols.get(&short).or_else(|| symbols.get(&long)).copied()
        };
        function.and_then(|function| function(arguments))
    }
}

impl Default for NativeLibraries {
    fn default() -> Self {
        Self::new()
    }
}
