use crate::share::interpreter::bytecodeInterpreter::Value;
use libloading::Library;
use std::{
    collections::HashMap,
    io::{self, Error},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

pub type NativeFunction = fn(args: &[Value<'_>]) -> Option<Value<'static>>;

/// A DLL loaded through System.load/loadLibrary. Keeps the OS handle alive
/// and records JNI lifecycle callbacks.
pub struct LoadedLibrary {
    pub path: PathBuf,
    library: Library,
    #[allow(dead_code)]
    on_load: Option<
        unsafe extern "system" fn(
            *mut crate::share::prims::jniInvocation::JavaVm,
            *mut std::ffi::c_void,
        ) -> i32,
    >,
    #[allow(dead_code)]
    on_unload: Option<
        unsafe extern "system" fn(
            *mut crate::share::prims::jniInvocation::JavaVm,
            *mut std::ffi::c_void,
        ),
    >,
}

impl LoadedLibrary {
    /// Returns the address of an exported symbol, if present.
    pub fn symbol(&self, name: &str) -> Option<*const std::ffi::c_void> {
        if name.as_bytes().contains(&0) {
            return None;
        }
        // libloading resolves symbols as typed values; fetch as a raw pointer.
        unsafe {
            self.library
                .get::<*const std::ffi::c_void>(name.as_bytes())
                .ok()
                .map(|sym| *sym)
        }
    }

    pub fn has_symbol(&self, name: &str) -> bool {
        self.symbol(name).is_some()
    }
}

pub struct NativeLibraries {
    loadedLibraries: Mutex<Vec<LoadedLibrary>>,
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
        {
            let libraries = self.loadedLibraries.lock().unwrap();
            if libraries.iter().any(|loaded| loaded.path == path) {
                return Ok(());
            }
        }
        let library = unsafe { Library::new(&loadPath) }.map_err(Error::other)?;

        // Resolve JNI lifecycle exports before registering the library.
        let on_load = unsafe {
            library
                .get::<unsafe extern "system" fn(
                    *mut crate::share::prims::jniInvocation::JavaVm,
                    *mut std::ffi::c_void,
                ) -> i32>(b"JNI_OnLoad")
                .ok()
                .map(|sym| *sym)
        };
        let on_unload = unsafe {
            library
                .get::<unsafe extern "system" fn(
                    *mut crate::share::prims::jniInvocation::JavaVm,
                    *mut std::ffi::c_void,
                )>(b"JNI_OnUnload")
                .ok()
                .map(|sym| *sym)
        };

        // JNI_OnLoad runs on the loading thread with a valid JavaVM*.
        if let Some(on_load) = on_load {
            let vm = crate::share::prims::jniInvocation::JavaVm::global();
            let _env = unsafe { &*vm }.ensure_attached();
            let result = unsafe { on_load(vm, std::ptr::null_mut()) };
            // JNI_VERSION_1_2 (0x00010002) is the minimum accepted by HotSpot.
            if result < 0x00010002 {
                return Err(Error::new(
                    io::ErrorKind::InvalidData,
                    format!("JNI_OnLoad returned unsupported version {result:#x}"),
                ));
            }
        }

        let loaded = LoadedLibrary {
            path,
            library,
            on_load,
            on_unload,
        };
        self.loadedLibraries.lock().unwrap().push(loaded);
        Ok(())
    }

    /// Unloads a library by canonical path, invoking JNI_OnUnload first.
    pub fn unload(&self, path: &Path) -> io::Result<()> {
        let canonical = path.canonicalize()?;
        let mut libraries = self.loadedLibraries.lock().unwrap();
        let position = libraries
            .iter()
            .position(|loaded| loaded.path == canonical)
            .ok_or_else(|| Error::new(io::ErrorKind::NotFound, "Library not loaded"))?;
        let loaded = libraries.swap_remove(position);
        if let Some(on_unload) = loaded.on_unload {
            let vm = crate::share::prims::jniInvocation::JavaVm::global();
            unsafe { on_unload(vm, std::ptr::null_mut()) };
        }
        drop(loaded);
        Ok(())
    }

    pub fn hasSymbol(&self, symbol: &str) -> io::Result<bool> {
        if symbol.as_bytes().contains(&0) {
            return Err(Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid symbol name",
            ));
        }
        let libraries = self.loadedLibraries.lock().unwrap();
        Ok(libraries.iter().any(|loaded| loaded.has_symbol(symbol)))
    }

    /// Resolves a JNI native method to a raw function pointer. Tries the
    /// short (pure) name first, then the long (signature-mangled) name,
    /// scanning libraries in load order — matching HotSpot's
    /// NativeLookup::lookup_entry.
    pub fn resolve_native_function(
        &self,
        class_name: &str,
        method_name: &str,
        signature: &str,
    ) -> Option<*const std::ffi::c_void> {
        let pure = super::nativeLookup::NativeLookup::pure_jni_name(class_name, method_name);
        let long =
            super::nativeLookup::NativeLookup::long_jni_name(class_name, method_name, signature);
        let libraries = self.loadedLibraries.lock().unwrap();
        for name in [&pure, &long] {
            for loaded in libraries.iter() {
                if let Some(symbol) = loaded.symbol(name) {
                    return Some(symbol);
                }
            }
        }
        None
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
