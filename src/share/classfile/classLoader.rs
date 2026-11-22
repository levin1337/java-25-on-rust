/*
 * Port of OpenJDK 25 HotSpot ClassLoader (share/classfile/classLoader.hpp, classLoader.cpp)
 *
 * ClassLoader manages class path entries (directories, jar files, image files),
 * bytecode loading, class hierarchy verification, type assignability, and
 * synthetic bootstrap classes.
 */

use super::systemDictionary::SystemDictionary;
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::oops::instanceOop::InstanceOop;
use crate::share::oops::method::Code;
use crate::share::oops::objArrayOop::ObjArrayOop;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use crate::share::runtime::javaCalls::{JavaCallArguments, JavaCalls};
use crate::share::runtime::synchronizer::Synchronizer;
use crate::{classfile::ClassFile, share::oops::instanceKlass::InstanceKlass};
use crate::share::runtime::shareCell::{Share, ShareMut, ShareRef};
use crate::share::runtime::syncBump::SyncBump;
use parking_lot::ReentrantMutex;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{self, Error, Read},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use zip::ZipArchive;

thread_local! {
    /// Java call stack of the current OS thread (internal names of the
    /// classes owning the active interpreter frames). Each Java thread
    /// runs on a dedicated OS thread, so this must be thread-local: a
    /// shared stack would interleave frames of unrelated threads and
    /// corrupt caller detection (`MethodHandles.lookup`,
    /// `Reflection.getCallerClass`) and exception stack traces.
    static CALL_STACK: std::cell::RefCell<Vec<&'static [u8]>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Internal names currently being resolved through a Java `ClassLoader`
    /// delegate on this thread. While a name is in this set, re-entrant
    /// `load` calls for it must report "not found" (not circularity) so the
    /// Java loader's parent-first delegation can proceed to `findClass`.
    static DELEGATING: std::cell::RefCell<std::collections::HashSet<Vec<u8>>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Per-thread Java call stack. Zero-sized; all state lives in the
/// thread-local above so `ClassLoader` stays cheaply shareable.
#[derive(Clone, Copy, Default)]
pub(crate) struct CallStack;

impl CallStack {
    fn with<R>(&self, f: impl FnOnce(&mut Vec<&'static [u8]>) -> R) -> R {
        CALL_STACK.with(|stack| f(&mut stack.borrow_mut()))
    }

    /// Pushes a class internal name. The name is borrowed from
    /// loader-owned storage which outlives every Java thread (the VM
    /// allocator is leaked at startup and threads hold `Arc`s to it).
    pub(crate) fn push(&self, name: &[u8]) {
        // SAFETY: class names point into `SyncBump`/loader storage that is
        // never freed; every OS thread observing this stack keeps the
        // loader alive.
        let name: &'static [u8] = unsafe { std::mem::transmute(name) };
        self.with(|stack| stack.push(name));
    }

    pub(crate) fn pop(&self) {
        self.with(|stack| {
            stack.pop();
        });
    }

    /// Copies the stack top-first order is preserved (index 0 = outermost).
    pub(crate) fn snapshot(&self) -> Vec<&'static [u8]> {
        self.with(|stack| stack.clone())
    }

    pub(crate) fn len(&self) -> usize {
        self.with(|stack| stack.len())
    }

    pub(crate) fn get(&self, index: usize) -> Option<&'static [u8]> {
        self.with(|stack| stack.get(index).copied())
    }

    pub(crate) fn last(&self) -> Option<&'static [u8]> {
        self.with(|stack| stack.last().copied())
    }
}

mod borrow_trace {
    use std::cell::RefCell;
    use std::sync::OnceLock;
    thread_local! {
        static SITES: RefCell<Vec<&'static std::panic::Location<'static>>> =
            const { RefCell::new(Vec::new()) };
    }
    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("JVM25_BORROWTRACE").is_some())
    }
    pub fn note(loc: &'static std::panic::Location<'static>) {
        if !enabled() {
            return;
        }
        SITES.with(|sites| {
            let mut sites = sites.borrow_mut();
            if sites.len() >= 512 {
                sites.remove(0);
            }
            sites.push(loc);
        });
    }
    pub fn dump() {
        SITES.with(|sites| {
            for loc in sites.borrow().iter().rev().take(40) {
                eprintln!("[objects-borrow-site] {loc}");
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassLoaderType {
    Other = 0,
    BootLoader = 1,
    PlatformLoader = 2,
    AppLoader = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    pub name: String,
    pub is_exported: bool,
}

pub struct ClassPathDirEntry {
    pub path: PathBuf,
}

impl ClassPathDirEntry {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn name(&self) -> &Path {
        &self.path
    }

    pub fn read_class(&self, class_name: &str) -> io::Result<Vec<u8>> {
        let file_path = self.path.join(format!("{class_name}.class"));
        fs::read(file_path)
    }
}

pub struct ClassPathZipEntry {
    pub path: PathBuf,
}

impl ClassPathZipEntry {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn name(&self) -> &Path {
        &self.path
    }
}

pub struct ClassPathImageEntry {
    pub name: String,
}

pub struct ModuleClassPathList {
    pub module_name: String,
    pub entries: Vec<PathBuf>,
}

pub enum ClassSource {
    Jar {
        archive: ZipArchive<File>,
        path: PathBuf,
    },
    Directory(PathBuf),
}

/// Identity of an invoke* call site resolution. Pointers reference stable
/// 'a-lifetime slices inside parsed class files and canonical `Arc`
/// `InstanceKlass` allocations that live in `classes`/`lambda_cache`
/// forever, so the key is stable for the loader's lifetime.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CallSiteKey {
    kind: u8,
    klass: usize,
    owner_ptr: usize,
    owner_len: usize,
    name_ptr: usize,
    name_len: usize,
    desc_ptr: usize,
    desc_len: usize,
}

impl CallSiteKey {
    pub(crate) fn new(
        kind: u8,
        klass: &Arc<InstanceKlass<'_>>,
        owner: &[u8],
        name: &[u8],
        descriptor: &[u8],
    ) -> Self {
        Self {
            kind,
            klass: Arc::as_ptr(klass) as usize,
            owner_ptr: owner.as_ptr() as usize,
            owner_len: owner.len(),
            name_ptr: name.as_ptr() as usize,
            name_len: name.len(),
            desc_ptr: descriptor.as_ptr() as usize,
            desc_len: descriptor.len(),
        }
    }

    /// `true` for invokestatic sites (kind 2).
    pub(crate) fn is_static(&self) -> bool {
        self.kind == 2
    }
}

pub struct ClassLoader<'a> {
    pub(crate) storage: &'a SyncBump,
    pub(crate) sources: Share<Vec<ClassSource>>,
    pub(crate) classes: Share<HashMap<&'a [u8], Arc<InstanceKlass<'a>>>>,
    loading: Share<HashSet<Vec<u8>>>,
    /// Serializes the define/load slow path across Java OS threads.
    /// Reentrant so recursive loads (superclass/interfaces) on the same
    /// thread proceed; other threads wait, then hit the `classes` cache.
    load_lock: ReentrantMutex<()>,
    pub(crate) bootImage: Option<Share<super::jimage::JImageReader>>,
    pub(crate) systemProperties: Share<HashMap<String, &'a str>>,
    pub(crate) arrays: Share<Vec<TypeArrayOop>>,
    pub(crate) objects: Share<Vec<InstanceOop<'a>>>,
    pub(crate) obj_arrays: Share<Vec<ObjArrayOop<'a>>>,
    pub(crate) objects_live: Share<Vec<bool>>,
    pub(crate) arrays_live: Share<Vec<bool>>,
    pub(crate) obj_arrays_live: Share<Vec<bool>>,
    /// Per-arena free slot stacks (LIFO) reused by `alloc_*`. Kept as
    /// separate stacks so allocation is O(1) — a single mixed list made
    /// every allocation scan for the matching slot kind.
    free_objects: Share<Vec<usize>>,
    free_type_arrays: Share<Vec<usize>>,
    free_obj_arrays: Share<Vec<usize>>,
    allocations_since_gc: AtomicUsize,
    pub(crate) gc_extra_roots: Share<Vec<Value<'a>>>,
    /// Head of the VM pending-reference list (JVMS/JDK `Reference` discovery
    /// list). GC links cleared references through their `discovered` field;
    /// `Reference.getAndClearReferencePendingList` consumes the head.
    pub(crate) reference_pending: Share<Option<usize>>,
    pub(crate) synchronizer: Share<Synchronizer>,
    class_loader_object: Share<Option<usize>>,
    unnamed_module_object: Share<Option<usize>>,
    /// Java `ClassLoader` object handles that have defined classes through
    /// the `defineClass*` natives (URLClassLoader, custom loaders, hidden
    /// classes). When a symbolic dependency is absent from the flat class
    /// store, `load` delegates to these loaders so their `findClass` can
    /// supply it — mirroring HotSpot's per-loader class namespaces.
    delegate_loaders: Share<Vec<usize>>,
    module_objects: Share<HashMap<String, usize>>,
    /// Lazily created `java.lang.Package` objects keyed by package name.
    package_objects: Share<HashMap<String, usize>>,
    /// Defined packages (JVMS §5.3 / java.lang.Package table), mirroring
    /// HotSpot `ClassLoader::packages`.
    packages: Share<HashMap<String, PackageEntry>>,
    /// Whether this loader supports parallel class loading
    /// (`ClassLoader.registerAsParallelCapable` semantics). All loaders in
    /// this port synchronize definition through `load_lock` and use
    /// lock-free `Share` arenas, so they are parallel capable.
    parallel_capable: bool,
    current_thread_object: Share<Option<usize>>,
    pub(crate) call_stack: CallStack,
    pub(crate) lambda_cache: Share<HashMap<(Vec<u8>, u16), Arc<InstanceKlass<'a>>>>,
    /// Linked `java.lang.invoke.MethodHandle` targets for generic
    /// invokedynamic call sites serviced by real bootstrap methods
    /// (non-LambdaMetafactory/non-StringConcatFactory). Keyed by
    /// (caller class internal name, BootstrapMethods attribute index).
    /// Values are `Value::Object` handles kept alive via `gc_extra_roots`.
    pub(crate) indy_targets: Share<HashMap<(Vec<u8>, u16), Value<'a>>>,
    /// Resolved values for `condy` (ConstantDynamic) call sites, keyed by
    /// (caller class internal name, constant-pool index of the Dynamic
    /// constant). Values are pinned via `gc_extra_roots`.
    pub(crate) condy_values: Share<HashMap<(Vec<u8>, u32), Value<'a>>>,
    /// Cooperative Java thread registry (see runtime::threading).
    pub(crate) java_threads: Share<HashMap<usize, crate::share::runtime::threading::JavaThreadInfo>>,
    pub(crate) scopedValueCaches: Share<HashMap<usize, usize>>,
    /// FIFO of thread object handles ready to run.
    pub(crate) thread_ready: Share<VecDeque<usize>>,
    /// Thread object handle currently executing (`None` = main thread).
    pub(crate) current_java_thread: Share<Option<usize>>,
    /// Next monitor id assigned to a started Java thread.
    pub(crate) next_java_tid: Share<u64>,
    /// Nested scheduler depth guard.
    pub(crate) schedule_depth: Share<u32>,
    /// Consecutive suspend-and-reschedule activations; bounds scheduler
    /// churn when only suspended workers remain runnable.
    pub(crate) suspend_streak: Share<u32>,
    /// Materialized `java.lang.Class` objects keyed by internal class name.
    pub(crate) class_objects: Share<HashMap<Vec<u8>, usize>>,
    pub(crate) stringLiterals: Share<HashMap<&'a [u8], usize>>,
    /// Reverse lookup: materialized Class object handle -> internal name.
    pub(crate) class_object_names: Share<HashMap<usize, &'a [u8]>>,
    /// Classpath origin of each loaded class, used to synthesize the
    /// `protectionDomain`/`CodeSource` a real `defineClass` would store.
    pub(crate) class_origins: Share<HashMap<Vec<u8>, Option<ClassOrigin>>>,
    /// `ProtectionDomain` object handles supplied to `defineClass*` natives,
    /// keyed by internal class name. Mirrors the PD a real VM attaches to
    /// the `InstanceKlass` at definition time.
    pub(crate) class_protection_domains: Share<HashMap<Vec<u8>, usize>>,
    /// `java.lang.ClassLoader` object handle that defined each class via
    /// `defineClass*`, keyed by internal class name. Mirrors the defining
    /// loader a real VM records on the `InstanceKlass`; `Class.getClassLoader`
    /// consults it so `ServiceLoader`/resource lookups see the right loader.
    pub(crate) defining_loaders: Share<HashMap<Vec<u8>, usize>>,
    /// Nest-host overrides for hidden classes defined via `defineClass0`
    /// with `NESTMATE_CLASS`: internal name -> host internal name. Hidden
    /// class bytes carry no `NestHost` attribute; the VM assigns the host
    /// from the lookup class at definition time.
    pub(crate) nest_host_overrides: Share<HashMap<Vec<u8>, Vec<u8>>>,
    /// Resolved invoke* targets (klass + parsed Code) keyed by call site.
    /// Method resolution is deterministic per receiver class, so hits skip
    /// the vtable scan and `Code::parse` on every call.
    pub(crate) invoke_cache: Share<HashMap<CallSiteKey, (Arc<InstanceKlass<'a>>, Code<'a>, u16)>>,
    /// Memoized `native_declaring_owner` results: (owner,name,descriptor)
    /// slice pointers -> declaring class name or None. Class hierarchy is
    /// immutable after load, so results never change.
    pub(crate) native_declaring_cache:
        Share<HashMap<(usize, usize, usize, usize, usize, usize), Option<Vec<u8>>>>,
    loader_type: ClassLoaderType,
}

/// Where a class's bytecode was loaded from. Mirrors the distinction
/// `URLClassLoader` makes between JAR and directory code sources.
#[derive(Clone, Debug)]
pub enum ClassOrigin {
    Jar(PathBuf),
    Directory(PathBuf),
}

impl<'a> ClassLoader<'a> {
    pub fn setSystemProperty(&self, key: &str, value: &str) -> io::Result<()> {
        if key.is_empty() {
            return Err(Error::other("Empty system property name"));
        }
        self.systemProperties
            .borrow_mut()
            .insert(key.to_owned(), self.storage.alloc_str(value));
        Ok(())
    }
    pub fn loader_type(&self) -> ClassLoaderType {
        self.loader_type
    }

    pub fn classes_loaded_count(&self) -> usize {
        self.classes.borrow().len()
    }

    pub fn classes_inited_count(&self) -> usize {
        self.classes.borrow().len()
    }

    pub fn class_init_time_ms(&self) -> u64 {
        0
    }

    pub(crate) fn class_loader_object_handle(&self) -> &Share<Option<usize>> {
        &self.class_loader_object
    }

    /// Lazily creates the shared unnamed `java.lang.Module` instance used for
    /// `Class.getModule()` and `ClassLoader.getUnnamedModule()`.
    pub(crate) fn unnamedModuleObject(&self) -> io::Result<usize> {
        if let Some(handle) = *self.class_loader_object.borrow() {
            let objects = self.borrow_objects();
            if let Some(loader) = objects.get(handle)
                && let Ok(slot) = loader.klass.instanceField(b"unnamedModule", b"Ljava/lang/Module;")
                && let Some(Value::Object(module)) = loader.fields.get(slot)
            {
                return Ok(*module);
            }
        }
        if let Some(handle) = *self.unnamed_module_object.borrow() {
            return Ok(handle);
        }
        let klass = self.load(b"java/lang/Module")?;
        let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
            self,
            &klass,
            &mut io::sink(),
            0,
        );
        let object = InstanceOop::new(klass)?;
        let handle = self.alloc_object(object);
        *self.unnamed_module_object.borrow_mut() = Some(handle);
        Ok(handle)
    }

    pub(crate) fn unnamed_module_object_handle(&self) -> &Share<Option<usize>> {
        &self.unnamed_module_object
    }

    /// Registers a `java.lang.Module` object created by `Module.<init>` /
    /// `defineModule0` so `Class.getModule` can resolve it by module name.
    pub(crate) fn register_module_object(&self, name: &str, handle: usize) {
        self.module_objects
            .borrow_mut()
            .insert(name.to_owned(), handle);
    }

    pub(crate) fn module_object(&self, name: &str) -> Option<usize> {
        self.module_objects.borrow().get(name).copied()
    }

    /// GC root access: all materialized `java.lang.Module` objects.
    pub(crate) fn module_objects_handle(&self) -> &Share<HashMap<String, usize>> {
        &self.module_objects
    }

    pub(crate) fn package_object(&self, name: &str) -> Option<usize> {
        self.package_objects.borrow().get(name).copied()
    }

    pub(crate) fn register_package_object(&self, name: &str, handle: usize) {
        self.package_objects
            .borrow_mut()
            .insert(name.to_owned(), handle);
    }

    /// GC sweep access: dead arena slots awaiting reuse, per arena.
    pub(crate) fn free_objects_handle(&self) -> &Share<Vec<usize>> {
        &self.free_objects
    }
    pub(crate) fn free_type_arrays_handle(&self) -> &Share<Vec<usize>> {
        &self.free_type_arrays
    }
    pub(crate) fn free_obj_arrays_handle(&self) -> &Share<Vec<usize>> {
        &self.free_obj_arrays
    }

    /// Module name providing the given internal class name, when the class
    /// comes from a named JDK module in the boot image. Returns `None` for
    /// classpath classes (unnamed module) and unknown classes.
    pub(crate) fn class_module_name(&self, class: &[u8]) -> Option<String> {
        let name = super::classFileParser::decodeString(class).ok()?;
        let image = self.bootImage.as_ref()?;
        image.borrow().class_module(&name).map(str::to_owned)
    }

    pub(crate) fn current_thread_object_handle(&self) -> &Share<Option<usize>> {
        &self.current_thread_object
    }

    pub(crate) fn referenceAssignable(&self, value: Value<'a>, target: &[u8]) -> io::Result<bool> {
        Ok(match value {
            Value::Null => true,
            Value::Object(handle) => {
                let objects = self.borrow_objects();
                let object = objects
                    .get(handle)
                    .ok_or_else(|| Error::other("Invalid object reference"))?;
                SystemDictionary::is_subclass_of(&object.klass, target)
                    || SystemDictionary::implements_interface(&object.klass, target)
            }
            Value::ObjectArray(handle) => {
                let component = self
                    .obj_arrays
                    .borrow()
                    .get(handle)
                    .ok_or_else(|| Error::other("Invalid object array reference"))?
                    .component;
                self.arrayAssignable(component, target)?
            }
            Value::PrimitiveArray(handle) => {
                let arrays = self.arrays.borrow();
                let array = arrays
                    .get(handle)
                    .ok_or_else(|| Error::other("Invalid array reference"))?;
                array.descriptor() == target || Self::arraySupertype(target)
            }
            Value::Arguments(_) => self.arrayAssignable(b"java/lang/String", target)?,
            Value::String(_) | Value::HostString(_) => Self::stringAssignable(target),
            Value::ClassMirror(_) => matches!(
                target,
                b"java/lang/Class"
                    | b"java/lang/Object"
                    | b"java/io/Serializable"
                    | b"java/lang/reflect/Type"
                    | b"java/lang/reflect/AnnotatedElement"
                    | b"java/lang/reflect/GenericDeclaration"
                    | b"java/lang/invoke/TypeDescriptor"
                    | b"java/lang/invoke/TypeDescriptor$OfField"
                    | b"java/lang/constant/Constable"
            ),
            Value::Output => matches!(target, b"java/io/PrintStream" | b"java/lang/Object"),
            _ => false,
        })
    }

    pub(crate) fn classAssignable(&self, source: &[u8], target: &[u8]) -> io::Result<bool> {
        if source == target || Self::arraySupertype(target) {
            return Ok(true);
        }
        let component = source
            .strip_prefix(b"[")
            .ok_or_else(|| Error::other("Expected array class"))?;
        if component.len() == 1 {
            return Ok(false);
        }
        let component = component
            .strip_prefix(b"L")
            .and_then(|name| name.strip_suffix(b";"))
            .unwrap_or(component);
        self.arrayAssignable(component, target)
    }

    fn arrayAssignable(&self, component: &[u8], target: &[u8]) -> io::Result<bool> {
        if Self::arraySupertype(target) {
            return Ok(true);
        }
        let Some(target) = target.strip_prefix(b"[") else {
            return Ok(false);
        };
        if let Some(targetClass) = target
            .strip_prefix(b"L")
            .and_then(|name| name.strip_suffix(b";"))
        {
            if component.starts_with(b"[") {
                return Ok(Self::arraySupertype(targetClass));
            }
            if component == targetClass || targetClass == b"java/lang/Object" {
                return Ok(true);
            }
            if component == b"java/lang/String" {
                return Ok(Self::stringAssignable(targetClass));
            }
            let klass = self.load(component)?;
            return Ok(SystemDictionary::is_subclass_of(&klass, targetClass)
                || SystemDictionary::implements_interface(&klass, targetClass));
        }
        if target.starts_with(b"[")
            && let Some(nested) = component.strip_prefix(b"[")
        {
            if nested.len() == 1 {
                return Ok(component == target);
            }
            let nested = nested
                .strip_prefix(b"L")
                .and_then(|name| name.strip_suffix(b";"))
                .unwrap_or(nested);
            return self.arrayAssignable(nested, target);
        }
        Ok(false)
    }

    fn arraySupertype(target: &[u8]) -> bool {
        matches!(
            target,
            b"java/lang/Object" | b"java/lang/Cloneable" | b"java/io/Serializable"
        )
    }

    fn stringAssignable(target: &[u8]) -> bool {
        matches!(
            target,
            b"java/lang/String"
                | b"java/lang/Object"
                | b"java/io/Serializable"
                | b"java/lang/Comparable"
                | b"java/lang/CharSequence"
                | b"java/lang/constant/Constable"
                | b"java/lang/constant/ConstantDesc"
        )
    }

    /// Default system properties required for correct VM behavior.
    /// Core reflection is served by the native accessor path; method-handle
    /// accessors and bytecode inflation are disabled because this VM has no
    /// bytecode class generation facility.
    fn default_system_properties() -> HashMap<String, &'static str> {
        HashMap::from([
            ("jdk.reflect.useDirectMethodHandle".to_owned(), "false"),
            ("jdk.reflect.useNativeAccessorOnly".to_owned(), "true"),
            ("sun.reflect.inflationThreshold".to_owned(), "2147483647"),
        ])
    }

    pub fn open(path: &Path, storage: &'a SyncBump) -> io::Result<Self> {
        let source = if path.is_dir() {
            ClassSource::Directory(path.to_path_buf())
        } else {
            ClassSource::Jar {
                archive: ZipArchive::new(File::open(path)?)?,
                path: path.to_path_buf(),
            }
        };
        Ok(Self {
            storage,
            sources: Share::new(vec![source]),
            classes: Share::new(HashMap::new()),
            loading: Share::new(HashSet::new()),
            load_lock: ReentrantMutex::new(()),
            bootImage: None,
            systemProperties: Share::new(Self::default_system_properties()),
            arrays: Share::new(Vec::new()),
            objects: Share::new(Vec::new()),
            obj_arrays: Share::new(Vec::new()),
            objects_live: Share::new(Vec::new()),
            arrays_live: Share::new(Vec::new()),
            obj_arrays_live: Share::new(Vec::new()),
            free_objects: Share::new(Vec::new()),
            free_type_arrays: Share::new(Vec::new()),
            free_obj_arrays: Share::new(Vec::new()),
            allocations_since_gc: AtomicUsize::new(0),
            gc_extra_roots: Share::new(Vec::new()),
            reference_pending: Share::new(None),
            synchronizer: Share::new(Synchronizer::new()),
            class_loader_object: Share::new(None),
            unnamed_module_object: Share::new(None),
            delegate_loaders: Share::new(Vec::new()),
            module_objects: Share::new(HashMap::new()),
            package_objects: Share::new(HashMap::new()),
            packages: Share::new(HashMap::new()),
            parallel_capable: true,
            current_thread_object: Share::new(None),
            call_stack: CallStack,
            lambda_cache: Share::new(HashMap::new()),
            indy_targets: Share::new(HashMap::new()),
            condy_values: Share::new(HashMap::new()),
            java_threads: Share::new(HashMap::new()),
            scopedValueCaches: Share::new(HashMap::new()),
            thread_ready: Share::new(VecDeque::new()),
            current_java_thread: Share::new(None),
            next_java_tid: Share::new(2),
            schedule_depth: Share::new(0),
            suspend_streak: Share::new(0),
            class_objects: Share::new(HashMap::new()),
            stringLiterals: Share::new(HashMap::new()),
            class_object_names: Share::new(HashMap::new()),
            class_origins: Share::new(HashMap::new()),
            class_protection_domains: Share::new(HashMap::new()),
            nest_host_overrides: Share::new(HashMap::new()),
            defining_loaders: Share::new(HashMap::new()),
            invoke_cache: Share::new(HashMap::new()),
            native_declaring_cache: Share::new(HashMap::new()),
            loader_type: ClassLoaderType::AppLoader,
        })
    }

    pub fn open_classpath(classpath: &str, storage: &'a SyncBump) -> io::Result<Self> {
        let mut sources = Vec::new();
        for entry in std::env::split_paths(classpath) {
            let p = if entry.as_os_str().is_empty() {
                Path::new(".")
            } else {
                entry.as_path()
            };
            if p.file_name().is_some_and(|name| name == "*") {
                if let Some(dir) = p.parent()
                    && let Ok(entries) = fs::read_dir(dir)
                {
                    let mut jars: Vec<PathBuf> = entries
                        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                        .filter(|path| {
                            path.extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
                        })
                        .collect();
                    jars.sort();
                    for jar in jars {
                        if let Ok(file) = File::open(&jar)
                            && let Ok(zip) = ZipArchive::new(file)
                        {
                            sources.push(ClassSource::Jar {
                                archive: zip,
                                path: jar.clone(),
                            });
                        }
                    }
                }
                continue;
            }
            if p.is_dir() {
                sources.push(ClassSource::Directory(p.to_path_buf()));
            } else if p.is_file() {
                if let Ok(file) = File::open(p) {
                    if let Ok(zip) = ZipArchive::new(file) {
                        sources.push(ClassSource::Jar {
                            archive: zip,
                            path: p.to_path_buf(),
                        });
                    }
                }
            }
        }
        Ok(Self {
            storage,
            sources: Share::new(sources),
            classes: Share::new(HashMap::new()),
            loading: Share::new(HashSet::new()),
            load_lock: ReentrantMutex::new(()),
            bootImage: None,
            systemProperties: Share::new(Self::default_system_properties()),
            arrays: Share::new(Vec::new()),
            objects: Share::new(Vec::new()),
            obj_arrays: Share::new(Vec::new()),
            objects_live: Share::new(Vec::new()),
            arrays_live: Share::new(Vec::new()),
            obj_arrays_live: Share::new(Vec::new()),
            free_objects: Share::new(Vec::new()),
            free_type_arrays: Share::new(Vec::new()),
            free_obj_arrays: Share::new(Vec::new()),
            allocations_since_gc: AtomicUsize::new(0),
            gc_extra_roots: Share::new(Vec::new()),
            reference_pending: Share::new(None),
            synchronizer: Share::new(Synchronizer::new()),
            class_loader_object: Share::new(None),
            unnamed_module_object: Share::new(None),
            delegate_loaders: Share::new(Vec::new()),
            module_objects: Share::new(HashMap::new()),
            package_objects: Share::new(HashMap::new()),
            packages: Share::new(HashMap::new()),
            parallel_capable: true,
            current_thread_object: Share::new(None),
            call_stack: CallStack,
            lambda_cache: Share::new(HashMap::new()),
            indy_targets: Share::new(HashMap::new()),
            condy_values: Share::new(HashMap::new()),
            java_threads: Share::new(HashMap::new()),
            scopedValueCaches: Share::new(HashMap::new()),
            thread_ready: Share::new(VecDeque::new()),
            current_java_thread: Share::new(None),
            next_java_tid: Share::new(2),
            schedule_depth: Share::new(0),
            suspend_streak: Share::new(0),
            class_objects: Share::new(HashMap::new()),
            stringLiterals: Share::new(HashMap::new()),
            class_object_names: Share::new(HashMap::new()),
            class_origins: Share::new(HashMap::new()),
            class_protection_domains: Share::new(HashMap::new()),
            nest_host_overrides: Share::new(HashMap::new()),
            defining_loaders: Share::new(HashMap::new()),
            invoke_cache: Share::new(HashMap::new()),
            native_declaring_cache: Share::new(HashMap::new()),
            loader_type: ClassLoaderType::AppLoader,
        })
    }

    pub(crate) fn note_allocation(&self) {
        self.allocations_since_gc
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn gc_due(&self) -> bool {
        static THRESHOLD: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let threshold = *THRESHOLD.get_or_init(|| {
            std::env::var("JVM25_GC_EVERY")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(262_144)
        });
        self.allocations_since_gc.load(Ordering::Relaxed) >= threshold
    }

    pub(crate) fn reset_allocations(&self) {
        self.allocations_since_gc.store(0, Ordering::Relaxed);
    }

    #[track_caller]
    pub(crate) fn borrow_objects(&self) -> ShareRef<'_, Vec<InstanceOop<'a>>> {
        borrow_trace::note(std::panic::Location::caller());
        self.objects.borrow()
    }

    #[track_caller]
    pub(crate) fn borrow_objects_mut(&self) -> ShareMut<'_, Vec<InstanceOop<'a>>> {
        borrow_trace::note(std::panic::Location::caller());
        self.objects.borrow_mut()
    }

    fn allocateInArena<T>(
        &self,
        value: T,
        arena: &Share<Vec<T>>,
        live: &Share<Vec<bool>>,
        free: &Share<Vec<usize>>,
    ) -> usize {
        self.note_allocation();
        let reused = free.borrow_mut().pop();
        let mut arena = arena.borrow_mut();
        let mut live = live.borrow_mut();
        if let Some(index) = reused {
            arena[index] = value;
            live[index] = true;
            return index;
        }
        let handle = arena.len();
        arena.push(value);
        live.push(true);
        handle
    }

    pub(crate) fn alloc_object(&self, object: InstanceOop<'a>) -> usize {
        self.allocateInArena(object, &self.objects, &self.objects_live, &self.free_objects)
    }

    pub(crate) fn alloc_type_array(&self, array: TypeArrayOop) -> usize {
        self.allocateInArena(array, &self.arrays, &self.arrays_live, &self.free_type_arrays)
    }

    pub(crate) fn alloc_obj_array(&self, array: ObjArrayOop<'a>) -> usize {
        self.allocateInArena(array, &self.obj_arrays, &self.obj_arrays_live, &self.free_obj_arrays)
    }

    pub fn add_source(&self, source: ClassSource) {
        self.sources.borrow_mut().push(source);
    }

    pub fn has_entry(&self, name: &[u8]) -> bool {
        let rel_path = match super::classFileParser::decodeString(name) {
            Ok(p) => p,
            Err(_) => return false,
        };
        if let Some(image) = &self.bootImage
            && image.borrow().hasClass(&rel_path)
        {
            return true;
        }
        let mut sources = self.sources.borrow_mut();
        for source in sources.iter_mut() {
            match source {
                ClassSource::Jar { archive: zip, .. } => {
                    if zip.by_name(&format!("{rel_path}.class")).is_ok() {
                        return true;
                    }
                }
                ClassSource::Directory(dir) => {
                    let file_path = dir.join(format!("{rel_path}.class"));
                    if file_path.exists() {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn withBootImage(mut self, path: &Path) -> io::Result<Self> {
        self.bootImage = Some(Share::new(super::jimage::JImageReader::open(path)?));
        Ok(self)
    }

    pub fn open_stream(&self, name: &[u8]) -> io::Result<Vec<u8>> {
        self.open_stream_with_origin(name).map(|(bytes, _)| bytes)
    }

    /// Like `open_stream` but also reports the classpath source the class
    /// bytes came from (`None` for boot-image classes).
    pub fn open_stream_with_origin(
        &self,
        name: &[u8],
    ) -> io::Result<(Vec<u8>, Option<ClassOrigin>)> {
        let rel_path = super::classFileParser::decodeString(name)?;
        if let Some(image) = &self.bootImage
            && let Some(bytes) = image.borrow_mut().findClass(&rel_path)?
        {
            return Ok((bytes, None));
        }
        let mut bytes = Vec::new();

        let mut sources = self.sources.borrow_mut();
        for source in sources.iter_mut() {
            match source {
                ClassSource::Jar { archive: zip, path } => {
                    if let Ok(mut entry) = zip.by_name(&format!("{rel_path}.class")) {
                        if entry.read_to_end(&mut bytes).is_ok() {
                            return Ok((bytes, Some(ClassOrigin::Jar(path.clone()))));
                        }
                    }
                }
                ClassSource::Directory(dir) => {
                    let file_path = dir.join(format!("{rel_path}.class"));
                    if file_path.exists() {
                        if let Ok(content) = fs::read(&file_path) {
                            return Ok((content, Some(ClassOrigin::Directory(dir.clone()))));
                        }
                    }
                }
            }
        }

        Err(super::linkageError::LinkageError::class_not_found(name).io())
    }

    /// Classpath origin recorded when the class was loaded, if any.
    pub fn class_origin(&self, name: &[u8]) -> Option<ClassOrigin> {
        self.class_origins
            .borrow()
            .get(name)
            .cloned()
            .flatten()
    }

    /// Records the `ProtectionDomain` object a `defineClass*` native was
    /// given for `name`, keeping it alive as a GC root.
    pub(crate) fn record_class_protection_domain(&self, name: &[u8], handle: usize) {
        self.class_protection_domains
            .borrow_mut()
            .insert(name.to_vec(), handle);
        self.gc_extra_roots
            .borrow_mut()
            .push(Value::Object(handle));
    }

    /// `ProtectionDomain` object handle recorded for `name`, if any.
    pub fn class_protection_domain(&self, name: &[u8]) -> Option<usize> {
        self.class_protection_domains.borrow().get(name).copied()
    }

    /// Records the `ClassLoader` object that defined `name` via a
    /// `defineClass*` native, keeping it alive as a GC root.
    pub(crate) fn record_defining_loader(&self, name: &[u8], handle: usize) {
        if self
            .defining_loaders
            .borrow_mut()
            .insert(name.to_vec(), handle)
            .is_none()
        {
            self.gc_extra_roots
                .borrow_mut()
                .push(Value::Object(handle));
        }
    }

    /// `ClassLoader` object handle that defined `name`, if recorded.
    pub fn defining_loader(&self, name: &[u8]) -> Option<usize> {
        self.defining_loaders.borrow().get(name).copied()
    }

    /// Records the VM-assigned nest host for a hidden class.
    pub(crate) fn record_nest_host(&self, name: &[u8], host: &[u8]) {
        self.nest_host_overrides
            .borrow_mut()
            .insert(name.to_vec(), host.to_vec());
    }

    /// VM-assigned nest host for `name`, if the class was defined as a
    /// hidden nestmate without a `NestHost` classfile attribute.
    pub fn hidden_nest_host(&self, name: &[u8]) -> Option<Vec<u8>> {
        self.nest_host_overrides.borrow().get(name).cloned()
    }

    /// Hidden classes whose VM-assigned nest host is `host`.
    pub fn hidden_nest_members(&self, host: &[u8]) -> Vec<Vec<u8>> {
        self.nest_host_overrides
            .borrow()
            .iter()
            .filter(|(_, h)| h.as_slice() == host)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Reads a non-class resource (properties files, data files, ...).
    /// Returns `Ok(None)` when the resource does not exist.
    pub fn open_resource(&self, name: &[u8]) -> io::Result<Option<Vec<u8>>> {
        let rel_path = super::classFileParser::decodeString(name)?;
        if rel_path.is_empty() || rel_path.starts_with('/') || rel_path.contains("..") {
            return Ok(None);
        }
        if let Some(image) = &self.bootImage {
            let mut image = image.borrow_mut();
            let path = image.matchingResources(&rel_path).next();
            if let Some(path) = path {
                return image.read_resource(&path).map(Some);
            }
        }
        let mut sources = self.sources.borrow_mut();
        for source in sources.iter_mut() {
            match source {
                ClassSource::Jar { archive: zip, .. } => {
                    if let Ok(mut entry) = zip.by_name(&rel_path)
                        && let Ok(bytes) = {
                            let mut bytes = Vec::new();
                            entry.read_to_end(&mut bytes).map(|_| bytes)
                        }
                    {
                        return Ok(Some(bytes));
                    }
                }
                ClassSource::Directory(dir) => {
                    let file_path = dir.join(&rel_path);
                    if file_path.is_file()
                        && let Ok(content) = fs::read(&file_path)
                    {
                        return Ok(Some(content));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Returns true when the class is provided by the boot image (JDK
    /// modules), i.e. loaded by the bootstrap loader.
    pub fn is_boot_class(&self, name: &[u8]) -> io::Result<bool> {
        let name = super::classFileParser::decodeString(name)?;
        Ok(self
            .bootImage
            .as_ref()
            .is_some_and(|image| image.borrow().hasClass(&name)))
    }

    pub fn hasClass(&self, name: &[u8]) -> io::Result<bool> {
        let name = super::classFileParser::decodeString(name)?;
        if let Some(image) = &self.bootImage
            && image.borrow().hasClass(&name)
        {
            return Ok(true);
        }
        Ok(self
            .sources
            .borrow_mut()
            .iter_mut()
            .any(|source| match source {
                ClassSource::Jar { archive: zip, .. } => {
                    zip.by_name(&format!("{name}.class")).is_ok()
                }
                ClassSource::Directory(dir) => dir.join(format!("{name}.class")).exists(),
            }))
    }

    /// True when `name` is already defined in this loader's class map —
    /// i.e. a previous `load`/`define_class` registered it. Does not
    /// trigger a load, unlike `load`.
    pub fn is_defined(&self, name: &[u8]) -> bool {
        self.classes.borrow().contains_key(name)
    }

    pub fn load_class(&self, name: &[u8]) -> io::Result<Arc<InstanceKlass<'a>>> {
        self.load(name)
    }

    pub fn load(&self, name: &[u8]) -> io::Result<Arc<InstanceKlass<'a>>> {
        if name.contains(&b'=') {
            let stack: Vec<String> = self
                .call_stack
                .snapshot()
                .iter()
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect();
            let loading: Vec<String> = self
                .loading
                .borrow()
                .iter()
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect();
            eprintln!(
                "[load-suspect] name={:?} loading={loading:?} jstack={stack:?}\n{}",
                String::from_utf8_lossy(name),
                std::backtrace::Backtrace::force_capture()
            );
        }
        if name.starts_with(b"[") {
            return self.load_array_class(name);
        }
        if name.starts_with(b":") {
            return Err(Error::other(format!(
                "Array or primitive class cannot be loaded by name: {}",
                String::from_utf8_lossy(name)
            )));
        }
        if let Some(klass) = self.classes.borrow().get(name) {
            return Ok(Arc::clone(klass));
        }
        // A Java loader is currently resolving this name on our behalf
        // (delegate_load below). Report "not found" so the Java
        // parent-first delegation proceeds to `findClass` instead of
        // failing with a spurious circularity error.
        if DELEGATING.with(|d| d.borrow().contains(name)) {
            return Err(super::linkageError::LinkageError::class_not_found(name).io());
        }
        // Serialize class definition across Java OS threads; recursive
        // loads on this thread re-enter freely.
        let _load_guard = self.load_lock.lock();
        if let Some(klass) = self.classes.borrow().get(name) {
            return Ok(Arc::clone(klass));
        }
        if !self.loading.borrow_mut().insert(name.to_vec()) {
            return Err(super::linkageError::LinkageError::new(
                super::linkageError::LinkageErrorKind::ClassCircularity,
                format!("{}", String::from_utf8_lossy(name)),
            )
            .io());
        }
        let result = match self.loadClass(name) {
            Ok(klass) => Ok(klass),
            Err(error) => {
                let not_found = matches!(
                    super::linkageError::linkage_kind(&error),
                    Some(super::linkageError::LinkageErrorKind::ClassNotFound)
                        | Some(super::linkageError::LinkageErrorKind::NoClassDefFound)
                );
                if not_found
                    && let Some(klass) = self.delegate_load(name)
                {
                    Ok(klass)
                } else {
                    Err(error)
                }
            }
        };
        self.loading.borrow_mut().remove(name);
        result
    }

    /// Registers a Java `ClassLoader` object (by instance handle) that has
    /// defined classes through the `defineClass*` natives. Called from the
    /// native entry points; idempotent.
    pub(crate) fn register_delegate_loader(&self, handle: usize) {
        let mut loaders = self.delegate_loaders.borrow_mut();
        if !loaders.contains(&handle) {
            loaders.push(handle);
        }
    }

    /// Resolves a missing class by invoking `loadClass` on each registered
    /// Java loader. The loader's own parent-first delegation runs; when its
    /// `findClass` defines the class through our `defineClass*` natives it
    /// lands in `classes` and is picked up here. Returns `None` when no
    /// delegate can supply the class (the original load error then stands).
    fn delegate_load(&self, name: &[u8]) -> Option<Arc<InstanceKlass<'a>>> {
        let loaders = self.delegate_loaders.borrow().clone();
        if loaders.is_empty() {
            return None;
        }
        let dot_name = String::from_utf8_lossy(name).replace('/', ".");
        DELEGATING.with(|d| {
            d.borrow_mut().insert(name.to_vec());
        });
        let result = (|| {
            for handle in loaders {
                if self.delegate_load_class(handle, &dot_name)
                    && let Some(klass) = self.classes.borrow().get(name)
                {
                    return Some(Arc::clone(klass));
                }
            }
            None
        })();
        DELEGATING.with(|d| {
            d.borrow_mut().remove(name);
        });
        result
    }

    /// `loader.loadClass(dotted_name)` on a Java loader object. Returns true
    /// when the call produced a `java.lang.Class` (the class itself is then
    /// expected in `classes`).
    fn delegate_load_class(&self, handle: usize, class_name: &str) -> bool {
        let klass = {
            let objects = self.borrow_objects();
            let Some(object) = objects.get(handle) else {
                return false;
            };
            Arc::clone(&object.klass)
        };
        let name = self.storage.alloc_str(class_name);
        let mut call = JavaCallArguments::with_receiver(Value::Object(handle));
        call.push_oop(Value::HostString(name));
        matches!(
            JavaCalls::call_virtual(
                self,
                &klass,
                b"loadClass",
                b"(Ljava/lang/String;)Ljava/lang/Class;",
                &call,
                &mut io::sink(),
            ),
            Ok(Some(Value::ClassMirror(_)))
        )
    }

    /// Defines the VM-internal class for an array type descriptor
    /// (`[I`, `[Ljava/lang/String;`, `[[I`, ...). HotSpot represents these
    /// as `ArrayKlass` whose superclass is `java.lang.Object` and which
    /// implements `Cloneable`/`Serializable`; array classes declare no
    /// fields or methods of their own (`length`, `clone`, etc. are handled
    /// by dedicated VM paths).
    fn load_array_class(&self, name: &[u8]) -> io::Result<Arc<InstanceKlass<'a>>> {
        if let Some(klass) = self.classes.borrow().get(name) {
            return Ok(Arc::clone(klass));
        }
        let _load_guard = self.load_lock.lock();
        if let Some(klass) = self.classes.borrow().get(name) {
            return Ok(Arc::clone(klass));
        }
        let name = self.storage.alloc_slice_copy(name);
        let class = ClassFile {
            majorVersion: 65,
            minorVersion: 0,
            // ACC_PUBLIC | ACC_FINAL | ACC_ABSTRACT
            accessFlags: 0x0411,
            name,
            superName: Some(b"java/lang/Object"),
            interfaces: vec![b"java/lang/Cloneable", b"java/io/Serializable"],
            constants: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            attributes: Vec::new(),
        };
        self.defineParsed(class)
    }

    fn loadClass(&self, name: &[u8]) -> io::Result<Arc<InstanceKlass<'a>>> {
    let _rg = crate::share::recprobe::Guard::enter("loadClass");
        if name == b"java/lang/Object" && self.bootImage.is_none() {
            let synthetic_class = ClassFile {
                majorVersion: 65,
                minorVersion: 0,
                accessFlags: 0x0001,
                name: b"java/lang/Object",
                superName: None,
                interfaces: Vec::new(),
                constants: Vec::new(),
                fields: Vec::new(),
                methods: Vec::new(),
                attributes: Vec::new(),
            };
            let klass = Arc::new(InstanceKlass::new(synthetic_class, None, Vec::new())?);
            self.classes
                .borrow_mut()
                .insert(klass.class.name, Arc::clone(&klass));
            return Ok(klass);
        }

        let (bytes, origin) = self.open_stream_with_origin(name)?;
        // HotSpot verifies classes loaded outside the boot image; the
        // boot class path trusts the JDK's own classes.
        let verify_class = origin.is_some()
            && std::env::var_os("JVM25_NO_VERIFY").is_none();
        if let Some(origin) = origin {
            self.class_origins
                .borrow_mut()
                .insert(name.to_vec(), Some(origin));
        }
        let bytes = self.storage.alloc_slice_copy(&bytes);
        let class = ClassFile::parse(bytes, false).map_err(|error| {
            use super::classFileParser::ClassError as E;
            let kind = match &error {
                E::InvalidMagic
                | E::InvalidConstantPool
                | E::InvalidConstantTag(_)
                | E::InvalidConstantIndex(_)
                | E::TrailingBytes
                | E::ClassFormatError(_) => super::linkageError::LinkageErrorKind::ClassFormat,
                E::UnsupportedVersion(..) => {
                    super::linkageError::LinkageErrorKind::UnsupportedClassVersion
                }
                _ => super::linkageError::LinkageErrorKind::ClassFormat,
            };
            super::linkageError::LinkageError::new(kind, error.to_string()).io()
        })?;
        if class.name != name {
            return Err(super::linkageError::LinkageError::no_class_def_found(name).io());
        }
        if verify_class {
            super::verifier::Verifier::verify(&class, Some(self), true).map_err(|error| {
                super::linkageError::LinkageError::new(
                    super::linkageError::LinkageErrorKind::Verify,
                    error.to_string(),
                )
                .io()
            })?;
        }
        self.defineParsed(class)
    }

    /// Defines a class from raw classfile bytes (used for runtime-generated
    /// classes such as invokedynamic lambda proxies). The bytes are copied
    /// into the loader's bump storage so the parsed `ClassFile` borrows stay
    /// valid for `'a`.
    pub fn define_class_bytes(&self, name: &[u8], bytes: &[u8]) -> io::Result<Arc<InstanceKlass<'a>>> {
        let _load_guard = self.load_lock.lock();
        let bytes = self.storage.alloc_slice_copy(bytes);
        let class = ClassFile::parse(bytes, false).map_err(|error| {
            super::linkageError::LinkageError::new(
                super::linkageError::LinkageErrorKind::ClassFormat,
                error.to_string(),
            )
            .io()
        })?;
        if class.name != name {
            return Err(super::linkageError::LinkageError::no_class_def_found(name).io());
        }
        if std::env::var_os("JVM25_DUMP_PROXY").is_some() && name.starts_with(b"jdk/proxy") {
            let file = format!(
                "D:/jvm25-rust/proxy_{}.class",
                String::from_utf8_lossy(name).replace('/', "_")
            );
            let _ = std::fs::write(&file, &*bytes);
        }
        self.defineParsed(class)
    }

    /// Links an already-parsed ClassFile: resolves the superclass and interfaces,
    /// builds the InstanceKlass and registers it. Used both for classes loaded
    /// from classpath/image sources and for synthetic classes generated at
    /// runtime (e.g. invokedynamic lambda classes).
    fn defineParsed(&self, class: ClassFile<'a>) -> io::Result<Arc<InstanceKlass<'a>>> {
        let class_name = class.name;
        let no_class_def = |dep: &[u8], error: io::Error| {
            if super::linkageError::linkage_kind(&error).is_some() {
                return error;
            }
            super::linkageError::LinkageError::new(
                super::linkageError::LinkageErrorKind::NoClassDefFound,
                format!(
                    "{}/{} (wrong name: {})",
                    String::from_utf8_lossy(class_name),
                    String::from_utf8_lossy(dep),
                    String::from_utf8_lossy(dep)
                ),
            )
            .io()
        };

        let super_klass = if let Some(super_name) = class.superName {
            Some(
                self.load(super_name)
                    .map_err(|e| no_class_def(super_name, e))?,
            )
        } else {
            None
        };

        let mut interfaces = Vec::with_capacity(class.interfaces.len());
        for iface_name in &class.interfaces {
            interfaces.push(
                self.load(iface_name)
                    .map_err(|e| no_class_def(iface_name, e))?,
            );
        }

        let is_interface = (class.accessFlags & 0x0200) != 0;
        if !is_interface
            && let Some(super_klass) = &super_klass
            && super_klass.is_interface()
        {
            return Err(
                super::linkageError::LinkageError::incompatible_class_change(format!(
                    "class {} has interface {} as super class",
                    String::from_utf8_lossy(class_name),
                    String::from_utf8_lossy(super_klass.class.name)
                ))
                .io(),
            );
        }
        for iface in &interfaces {
            if !iface.is_interface() {
                return Err(
                    super::linkageError::LinkageError::incompatible_class_change(format!(
                        "class {} implements non-interface {}",
                        String::from_utf8_lossy(class_name),
                        String::from_utf8_lossy(iface.class.name)
                    ))
                    .io(),
                );
            }
        }

        let klass = Arc::new(InstanceKlass::new(class, super_klass, interfaces)?);
        self.classes
            .borrow_mut()
            .insert(klass.class.name, Arc::clone(&klass));
        Ok(klass)
    }

    /// Defines a synthetic class from an already-parsed ClassFile structure.
    /// All byte slices inside `class` must be allocated in this loader's
    /// storage (or be 'static) so that they live for 'a.
    pub fn define_class(&self, class: ClassFile<'a>) -> io::Result<Arc<InstanceKlass<'a>>> {
        let _load_guard = self.load_lock.lock();
        self.defineParsed(class)
    }

    /// HotSpot `ClassLoader::find_class` equivalent: searches only this
    /// loader's own class path entries and the boot image, without the
    /// Java-loader delegate fallback performed by `load`.
    pub fn find_class(&self, name: &[u8]) -> io::Result<Arc<InstanceKlass<'a>>> {
        let _load_guard = self.load_lock.lock();
        if let Some(klass) = self.classes.borrow().get(name) {
            return Ok(Arc::clone(klass));
        }
        self.loadClass(name)
    }

    /// Loads and links the named class (JVMS §5.3.5 resolution + §5.4.3.1
    /// preparation is performed lazily by the interpreter; this performs
    /// loading and marks the class `Linked`).
    pub fn resolve(&self, name: &[u8]) -> io::Result<Arc<InstanceKlass<'a>>> {
        let klass = self.load(name)?;
        use crate::share::oops::instanceKlass::InitState;
        if *klass.state.borrow() == InitState::Loaded {
            klass.state.set(InitState::Linked);
        }
        Ok(klass)
    }

    /// Whether this loader supports parallel class loading
    /// (`ClassLoader.registerAsParallelCapable`).
    pub fn parallel_capable(&self) -> bool {
        self.parallel_capable
    }

    /// Builder-style override for `parallel_capable`.
    pub fn with_parallel_capable(mut self, capable: bool) -> Self {
        self.parallel_capable = capable;
        self
    }

    /// JVMS §5.3 package definition: records `name` in this loader's
    /// package table. Defining the same package twice is idempotent.
    pub fn define_package(&self, name: &[u8]) -> io::Result<PackageEntry> {
        let name = super::classFileParser::decodeString(name)?;
        let valid = !name.is_empty()
            && !name.starts_with('.')
            && !name.ends_with('.')
            && !name.contains("..")
            && name
                .split('.')
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$'));
        if !valid {
            return Err(Error::other(format!(
                "Illegal package name: {name}"
            )));
        }
        let mut packages = self.packages.borrow_mut();
        Ok(packages
            .entry(name.clone())
            .or_insert_with(|| PackageEntry {
                name,
                is_exported: true,
            })
            .clone())
    }

    /// Looks up a defined package by dot-separated name.
    pub fn get_package(&self, name: &[u8]) -> io::Result<Option<PackageEntry>> {
        let name = super::classFileParser::decodeString(name)?;
        Ok(self.packages.borrow().get(&name).cloned())
    }

    /// All packages defined by this loader.
    pub fn get_packages(&self) -> Vec<PackageEntry> {
        self.packages.borrow().values().cloned().collect()
    }

    /// `java.lang.ClassLoader.getResource` equivalent: returns the
    /// resource path inside this loader's class path / boot image when the
    /// named resource exists.
    pub fn get_resource(&self, name: &[u8]) -> io::Result<Option<String>> {
        let rel_path = super::classFileParser::decodeString(name)?;
        if rel_path.is_empty() || rel_path.starts_with('/') || rel_path.contains("..") {
            return Ok(None);
        }
        if let Some(image) = &self.bootImage {
            let mut image = image.borrow_mut();
            if let Some(path) = image.matchingResources(&rel_path).next() {
                return Ok(Some(path));
            }
        }
        let mut sources = self.sources.borrow_mut();
        for source in sources.iter_mut() {
            match source {
                ClassSource::Jar { archive: zip, .. } => {
                    if zip.by_name(&rel_path).is_ok() {
                        return Ok(Some(rel_path));
                    }
                }
                ClassSource::Directory(dir) => {
                    if dir.join(&rel_path).is_file() {
                        return Ok(Some(rel_path));
                    }
                }
            }
        }
        Ok(None)
    }

    /// `java.lang.ClassLoader.getResourceAsStream` equivalent: reads the
    /// named resource's bytes, `None` when absent.
    pub fn get_resource_as_stream(&self, name: &[u8]) -> io::Result<Option<Vec<u8>>> {
        self.open_resource(name)
    }

    /// Returns the cached synthetic lambda class for an invokedynamic call
    /// site, or `None`.
    pub(crate) fn lambda_class(
        &self,
        caller: &[u8],
        bsm_index: u16,
    ) -> Option<Arc<InstanceKlass<'a>>> {
        self.lambda_cache
            .borrow()
            .get(&(caller.to_vec(), bsm_index))
            .map(Arc::clone)
    }

    /// Caches a generated lambda class for an invokedynamic call site.
    pub(crate) fn cache_lambda_class(
        &self,
        caller: &[u8],
        bsm_index: u16,
        klass: Arc<InstanceKlass<'a>>,
    ) {
        self.lambda_cache
            .borrow_mut()
            .insert((caller.to_vec(), bsm_index), klass);
    }

    /// Cached linked target for a generic invokedynamic call site.
    pub(crate) fn indy_target(&self, caller: &[u8], bsm_index: u16) -> Option<Value<'a>> {
        self.indy_targets
            .borrow()
            .get(&(caller.to_vec(), bsm_index))
            .copied()
    }

    /// Caches the linked target MethodHandle for a generic invokedynamic
    /// call site and pins it as a GC root.
    pub(crate) fn cache_indy_target(&self, caller: &[u8], bsm_index: u16, target: Value<'a>) {
        self.indy_targets
            .borrow_mut()
            .insert((caller.to_vec(), bsm_index), target);
        self.gc_extra_roots.borrow_mut().push(target);
    }

    /// Cached resolved value for a `condy` constant.
    pub(crate) fn condy_value(&self, caller: &[u8], cp_index: u32) -> Option<Value<'a>> {
        self.condy_values
            .borrow()
            .get(&(caller.to_vec(), cp_index))
            .copied()
    }

    /// Caches a resolved `condy` value and pins it as a GC root.
    pub(crate) fn cache_condy_value(&self, caller: &[u8], cp_index: u32, value: Value<'a>) {
        self.condy_values
            .borrow_mut()
            .insert((caller.to_vec(), cp_index), value);
        self.gc_extra_roots.borrow_mut().push(value);
    }

    pub fn systemClassLoaderObject(&self) -> io::Result<usize> {
        if let Some(handle) = *self.class_loader_object.borrow() {
            return Ok(handle);
        }
        // Prefer the real application class loader created by
        // `jdk.internal.loader.ClassLoaders.<clinit>`: it has a live
        // `URLClassPath`, `nameToModule` map and parent chain, all of which
        // `BuiltinClassLoader.findResource`/`findMiscResource` require.
        // A bare `InstanceOop` leaves those fields null and resource lookup
        // dies with NPE.
        if self.bootImage.is_some()
            && let Ok(klass) = self.load(b"jdk/internal/loader/ClassLoaders")
        {
            let _ = crate::share::interpreter::bytecodeInterpreter::initialize(
                self,
                &klass,
                &mut io::sink(),
                0,
            );
            if let Ok(cell) = klass.staticField(b"APP_LOADER", b"Ljdk/internal/loader/ClassLoaders$AppClassLoader;") {
                if let Value::Object(handle) = cell.get() {
                    *self.class_loader_object.borrow_mut() = Some(handle);
                    return Ok(handle);
                }
            }
        }
        let klass = if self.bootImage.is_some() {
            match self.load(b"jdk/internal/loader/ClassLoaders$AppClassLoader") {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("[dbg] AppClassLoader load failed: {e}");
                    self.load(b"java/lang/ClassLoader")?
                }
            }
        } else {
            self.load(b"java/lang/ClassLoader")?
        };
        let object = InstanceOop::new(klass.clone())?;
        let handle = self.alloc_object(object);
        if let Ok(module) = self.unnamedModuleObject()
            && let Ok(slot) = klass.instanceField(b"unnamedModule", b"Ljava/lang/Module;")
        {
            let mut objects = self.borrow_objects_mut();
            if let Some(object) = objects.get_mut(handle)
                && slot < object.fields.len()
            {
                object.fields[slot] = Value::Object(module);
            }
        }
        *self.class_loader_object.borrow_mut() = Some(handle);
        Ok(handle)
    }

    pub fn currentThreadObject(&self) -> io::Result<usize> {
        if let Some(handle) = *self.current_thread_object.borrow() {
            return Ok(handle);
        }
        let klass = self.load(b"java/lang/Thread")?;
        let mut object = InstanceOop::new(klass)?;
        // Thread.threadState() and friends require the private FieldHolder
        // (and its ThreadGroup) to exist; real threads get them from
        // Thread.<init>.
        if let Ok(holder_klass) = self.load(b"java/lang/Thread$FieldHolder") {
            let mut holder_object = InstanceOop::new(holder_klass)?;
            if let Ok(group_klass) = self.load(b"java/lang/ThreadGroup") {
                let mut group_object = InstanceOop::new(group_klass)?;
                if let Ok(index) =
                    group_object.klass.instanceField(b"name", b"Ljava/lang/String;")
                {
                    group_object.fields[index] = Value::HostString(
                        self.storage.alloc_str("system"),
                    );
                }
                if let Ok(index) = group_object
                    .klass
                    .instanceField(b"maxPriority", b"I")
                {
                    group_object.fields[index] = Value::Int(10);
                }
                let group_handle = self.alloc_object(group_object);
                if let Ok(index) = holder_object
                    .klass
                    .instanceField(b"group", b"Ljava/lang/ThreadGroup;")
                {
                    holder_object.fields[index] = Value::Object(group_handle);
                }
            }
            if let Ok(index) = holder_object
                .klass
                .instanceField(b"priority", b"I")
            {
                holder_object.fields[index] = Value::Int(5);
            }
            if let Ok(index) = holder_object
                .klass
                .instanceField(b"threadStatus", b"I")
            {
                holder_object.fields[index] = Value::Int(5);
            }
            let holder_handle = self.alloc_object(holder_object);
            if let Ok(index) = object
                .klass
                .instanceField(b"holder", b"Ljava/lang/Thread$FieldHolder;")
            {
                object.fields[index] = Value::Object(holder_handle);
            }
        }
        if let Ok(index) = object.klass.instanceField(b"name", b"Ljava/lang/String;") {
            object.fields[index] = Value::HostString(self.storage.alloc_str("main"));
        }
        // `Thread.<init>` never ran for this synthesized object, so lock
        // fields that real threads get from the constructor stay null.
        // `Thread.blockedOn` does `synchronized(interruptLock)` and NPEs on
        // null — give it a real monitor object like `<init>` would.
        if let Ok(index) = object
            .klass
            .instanceField(b"interruptLock", b"Ljava/lang/Object;")
            && let Ok(lock_klass) = self.load(b"java/lang/Object")
        {
            if let Ok(lock) = InstanceOop::new(lock_klass) {
                let lock_handle = self.alloc_object(lock);
                object.fields[index] = Value::Object(lock_handle);
            }
        }
        let handle = self.alloc_object(object);
        *self.current_thread_object.borrow_mut() = Some(handle);
        if std::env::var_os("JVM25_TRACE").is_some() {
            let objects = self.borrow_objects();
            if let Some(thread) = objects.get(handle) {
                eprintln!("[thread] main thread fields={:?}", thread.fields);
                if let Ok(slot) = thread.klass.instanceField(b"holder", b"Ljava/lang/Thread$FieldHolder;")
                    && let Some(Value::Object(h)) = thread.fields.get(slot)
                    && let Some(holder) = objects.get(*h)
                {
                    eprintln!("[thread] holder fields={:?}", holder.fields);
                }
            }
        }
        Ok(handle)
    }

    pub fn jarMain(&self) -> io::Result<String> {
        let mut manifest = Vec::new();
        let mut sources = self.sources.borrow_mut();
        for source in sources.iter_mut() {
            if let ClassSource::Jar { archive: zip, .. } = source {
                if let Ok(mut entry) = zip.by_name("META-INF/MANIFEST.MF") {
                    if entry.read_to_end(&mut manifest).is_ok() {
                        break;
                    }
                }
            }
        }
        drop(sources);

        if manifest.is_empty() {
            return Err(Error::other("Missing META-INF/MANIFEST.MF in JAR"));
        }

        let mut lines = manifest
            .split(|byte| *byte == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
        while let Some(line) = lines.next().filter(|line| !line.is_empty()) {
            let mut value = line.to_vec();
            while let Some(continuation) = lines
                .clone()
                .next()
                .and_then(|line| line.strip_prefix(b" "))
            {
                value.extend_from_slice(continuation);
                lines.next();
            }
            if let Some(separator) = value.iter().position(|byte| *byte == b':')
                && value[..separator].eq_ignore_ascii_case(b"Main-Class")
            {
                let class = std::str::from_utf8(&value[separator + 1..])
                    .map_err(Error::other)?
                    .trim()
                    .replace('.', "/");
                if class.is_empty() {
                    break;
                }
                return Ok(class);
            }
        }
        Err(Error::other("Missing Main-Class in JAR manifest"))
    }
}

#[allow(dead_code)]
fn _assert_loader_is_thread_safe() {
    fn check<T: Send + Sync>() {}
    check::<ClassLoader<'static>>();
}
