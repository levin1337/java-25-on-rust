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
use crate::share::oops::objArrayOop::ObjArrayOop;
use crate::share::oops::typeArrayOop::TypeArrayOop;
use crate::share::runtime::synchronizer::Synchronizer;
use crate::{classfile::ClassFile, share::oops::instanceKlass::InstanceKlass};
use bumpalo::Bump;
use std::{
    cell::{Cell, Ref, RefCell},
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Error, Read},
    path::{Path, PathBuf},
    rc::Rc,
};
use zip::ZipArchive;

mod borrow_trace {
    use std::cell::RefCell;
    thread_local! {
        static SITES: RefCell<Vec<&'static std::panic::Location<'static>>> =
            const { RefCell::new(Vec::new()) };
    }
    pub fn note(loc: &'static std::panic::Location<'static>) {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FreeSlot {
    Object(usize),
    TypeArray(usize),
    ObjArray(usize),
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

pub struct ClassLoader<'a> {
    pub(crate) storage: &'a Bump,
    pub(crate) sources: RefCell<Vec<ClassSource>>,
    pub(crate) classes: RefCell<HashMap<&'a [u8], Rc<InstanceKlass<'a>>>>,
    loading: RefCell<HashSet<Vec<u8>>>,
    pub(crate) bootImage: Option<RefCell<super::jimage::JImageReader>>,
    pub(crate) systemProperties: RefCell<HashMap<String, &'a str>>,
    pub(crate) arrays: RefCell<Vec<TypeArrayOop>>,
    pub(crate) objects: RefCell<Vec<InstanceOop<'a>>>,
    pub(crate) obj_arrays: RefCell<Vec<ObjArrayOop<'a>>>,
    pub(crate) objects_live: RefCell<Vec<bool>>,
    pub(crate) arrays_live: RefCell<Vec<bool>>,
    pub(crate) obj_arrays_live: RefCell<Vec<bool>>,
    free_slots: RefCell<Vec<FreeSlot>>,
    allocations_since_gc: Cell<usize>,
    pub(crate) gc_extra_roots: RefCell<Vec<Value<'a>>>,
    pub(crate) synchronizer: RefCell<Synchronizer>,
    class_loader_object: RefCell<Option<usize>>,
    unnamed_module_object: RefCell<Option<usize>>,
    current_thread_object: RefCell<Option<usize>>,
    pub(crate) call_stack: RefCell<Vec<&'a [u8]>>,
    pub(crate) lambda_cache: RefCell<HashMap<(Vec<u8>, u16), Rc<InstanceKlass<'a>>>>,
    loader_type: ClassLoaderType,
}

/// Extracts the resource name (path inside the module) from a jimage
/// resource path of the form "/module/rest/of/path".
fn jimage_resource_name(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let (_, name) = rest.split_once('/')?;
    Some(name.to_owned())
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

    pub(crate) fn class_loader_object_handle(&self) -> &RefCell<Option<usize>> {
        &self.class_loader_object
    }

    /// Lazily creates the shared unnamed `java.lang.Module` instance used for
    /// `Class.getModule()` and `ClassLoader.getUnnamedModule()`.
    pub(crate) fn unnamedModuleObject(&self) -> io::Result<usize> {
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

    pub(crate) fn unnamed_module_object_handle(&self) -> &RefCell<Option<usize>> {
        &self.unnamed_module_object
    }

    pub(crate) fn current_thread_object_handle(&self) -> &RefCell<Option<usize>> {
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

    pub fn open(path: &Path, storage: &'a Bump) -> io::Result<Self> {
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
            sources: RefCell::new(vec![source]),
            classes: RefCell::new(HashMap::new()),
            loading: RefCell::new(HashSet::new()),
            bootImage: None,
            systemProperties: RefCell::new(Self::default_system_properties()),
            arrays: RefCell::new(Vec::new()),
            objects: RefCell::new(Vec::new()),
            obj_arrays: RefCell::new(Vec::new()),
            objects_live: RefCell::new(Vec::new()),
            arrays_live: RefCell::new(Vec::new()),
            obj_arrays_live: RefCell::new(Vec::new()),
            free_slots: RefCell::new(Vec::new()),
            allocations_since_gc: Cell::new(0),
            gc_extra_roots: RefCell::new(Vec::new()),
            synchronizer: RefCell::new(Synchronizer::new()),
            class_loader_object: RefCell::new(None),
            unnamed_module_object: RefCell::new(None),
            current_thread_object: RefCell::new(None),
            call_stack: RefCell::new(Vec::new()),
            lambda_cache: RefCell::new(HashMap::new()),
            loader_type: ClassLoaderType::AppLoader,
        })
    }

    pub fn open_classpath(classpath: &str, storage: &'a Bump) -> io::Result<Self> {
        let mut sources = Vec::new();
        for entry in std::env::split_paths(classpath) {
            let p = if entry.as_os_str().is_empty() {
                Path::new(".")
            } else {
                entry.as_path()
            };
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
            sources: RefCell::new(sources),
            classes: RefCell::new(HashMap::new()),
            loading: RefCell::new(HashSet::new()),
            bootImage: None,
            systemProperties: RefCell::new(Self::default_system_properties()),
            arrays: RefCell::new(Vec::new()),
            objects: RefCell::new(Vec::new()),
            obj_arrays: RefCell::new(Vec::new()),
            objects_live: RefCell::new(Vec::new()),
            arrays_live: RefCell::new(Vec::new()),
            obj_arrays_live: RefCell::new(Vec::new()),
            free_slots: RefCell::new(Vec::new()),
            allocations_since_gc: Cell::new(0),
            gc_extra_roots: RefCell::new(Vec::new()),
            synchronizer: RefCell::new(Synchronizer::new()),
            class_loader_object: RefCell::new(None),
            unnamed_module_object: RefCell::new(None),
            current_thread_object: RefCell::new(None),
            call_stack: RefCell::new(Vec::new()),
            lambda_cache: RefCell::new(HashMap::new()),
            loader_type: ClassLoaderType::AppLoader,
        })
    }

    pub(crate) fn note_allocation(&self) {
        self.allocations_since_gc
            .set(self.allocations_since_gc.get() + 1);
    }

    pub(crate) fn gc_due(&self) -> bool {
        self.allocations_since_gc.get() >= 2048
    }

    pub(crate) fn reset_allocations(&self) {
        self.allocations_since_gc.set(0);
    }

    #[track_caller]
    pub(crate) fn borrow_objects(&self) -> Ref<'_, Vec<InstanceOop<'a>>> {
        borrow_trace::note(std::panic::Location::caller());
        self.objects.borrow()
    }

    #[track_caller]
    pub(crate) fn borrow_objects_mut(&self) -> std::cell::RefMut<'_, Vec<InstanceOop<'a>>> {
        borrow_trace::note(std::panic::Location::caller());
        self.objects.borrow_mut()
    }

    pub(crate) fn alloc_object(&self, object: InstanceOop<'a>) -> usize {
        self.note_allocation();
        let mut objects = match self.objects.try_borrow_mut() {
            Ok(guard) => guard,
            Err(_) => {
                borrow_trace::dump();
                panic!("objects arena already borrowed");
            }
        };
        let mut free = self.free_slots.borrow_mut();
        if let Some(position) = free
            .iter()
            .rposition(|slot| matches!(slot, FreeSlot::Object(_)))
        {
            let FreeSlot::Object(index) = free.remove(position) else {
                unreachable!()
            };
            objects[index] = object;
            drop(objects);
            drop(free);
            let mut live = self.objects_live.borrow_mut();
            live.resize(self.borrow_objects().len(), true);
            live[index] = true;
            return index;
        }
        drop(free);
        let handle = objects.len();
        objects.push(object);
        drop(objects);
        let mut live = self.objects_live.borrow_mut();
        live.resize(handle + 1, true);
        live[handle] = true;
        handle
    }

    pub(crate) fn alloc_type_array(&self, array: TypeArrayOop) -> usize {
        self.note_allocation();
        let mut arrays = self.arrays.borrow_mut();
        let mut free = self.free_slots.borrow_mut();
        if let Some(position) = free
            .iter()
            .rposition(|slot| matches!(slot, FreeSlot::TypeArray(_)))
        {
            let FreeSlot::TypeArray(index) = free.remove(position) else {
                unreachable!()
            };
            arrays[index] = array;
            drop(arrays);
            drop(free);
            let mut live = self.arrays_live.borrow_mut();
            live.resize(self.arrays.borrow().len(), true);
            live[index] = true;
            return index;
        }
        drop(free);
        let handle = arrays.len();
        arrays.push(array);
        drop(arrays);
        let mut live = self.arrays_live.borrow_mut();
        live.resize(handle + 1, true);
        live[handle] = true;
        handle
    }

    pub(crate) fn alloc_obj_array(&self, array: ObjArrayOop<'a>) -> usize {
        self.note_allocation();
        let mut arrays = self.obj_arrays.borrow_mut();
        let mut free = self.free_slots.borrow_mut();
        if let Some(position) = free
            .iter()
            .rposition(|slot| matches!(slot, FreeSlot::ObjArray(_)))
        {
            let FreeSlot::ObjArray(index) = free.remove(position) else {
                unreachable!()
            };
            arrays[index] = array;
            drop(arrays);
            drop(free);
            let mut live = self.obj_arrays_live.borrow_mut();
            live.resize(self.obj_arrays.borrow().len(), true);
            live[index] = true;
            return index;
        }
        drop(free);
        let handle = arrays.len();
        arrays.push(array);
        drop(arrays);
        let mut live = self.obj_arrays_live.borrow_mut();
        live.resize(handle + 1, true);
        live[handle] = true;
        handle
    }

    pub(crate) fn free_slot(&self, slot: FreeSlot) {
        self.free_slots.borrow_mut().push(slot);
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
        self.bootImage = Some(RefCell::new(super::jimage::JImageReader::open(path)?));
        Ok(self)
    }

    /// Locates all resources (non-class entries) with the given path across
    /// the boot image and the class path, in loader delegation order.
    pub fn resource_locations(&self, name: &[u8]) -> io::Result<Vec<PathBuf>> {
        let name = super::classFileParser::decodeString(name)?;
        let mut locations = Vec::new();
        if let Some(image) = &self.bootImage {
            let image = image.borrow();
            let mut matches: Vec<String> = image
                .resource_paths()
                .filter(|path| jimage_resource_name(path).as_deref() == Some(name.as_str()))
                .map(str::to_owned)
                .collect();
            matches.sort_by(|a, b| {
                let a_base = a.starts_with("/java.base/");
                let b_base = b.starts_with("/java.base/");
                b_base.cmp(&a_base).then_with(|| a.cmp(b))
            });
            locations.extend(matches.into_iter().map(PathBuf::from));
        }
        let mut sources = self.sources.borrow_mut();
        for source in sources.iter_mut() {
            match source {
                ClassSource::Jar { archive: zip, path } => {
                    let entry_name = name.clone();
                    for index in 0..zip.len() {
                        if let Ok(entry) = zip.by_index(index)
                            && entry.name().ok().is_some_and(|entry| entry == entry_name)
                        {
                            locations.push(path.join(format!("!/{entry_name}")));
                        }
                    }
                }
                ClassSource::Directory(dir) => {
                    let path = dir.join(&name);
                    if path.is_file() {
                        locations.push(path);
                    }
                }
            }
        }
        Ok(locations)
    }

    pub fn open_stream(&self, name: &[u8]) -> io::Result<Vec<u8>> {
        let rel_path = super::classFileParser::decodeString(name)?;
        if let Some(image) = &self.bootImage
            && let Some(bytes) = image.borrow_mut().findClass(&rel_path)?
        {
            return Ok(bytes);
        }
        let mut bytes = Vec::new();

        let mut sources = self.sources.borrow_mut();
        for source in sources.iter_mut() {
            match source {
                ClassSource::Jar { archive: zip, .. } => {
                    if let Ok(mut entry) = zip.by_name(&format!("{rel_path}.class")) {
                        if entry.read_to_end(&mut bytes).is_ok() {
                            return Ok(bytes);
                        }
                    }
                }
                ClassSource::Directory(dir) => {
                    let file_path = dir.join(format!("{rel_path}.class"));
                    if file_path.exists() {
                        if let Ok(content) = fs::read(&file_path) {
                            return Ok(content);
                        }
                    }
                }
            }
        }

        Err(super::linkageError::LinkageError::class_not_found(name).io())
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
            let mut matches: Vec<String> = image
                .resource_paths()
                .filter(|path| jimage_resource_name(path).as_deref() == Some(rel_path.as_str()))
                .map(str::to_owned)
                .collect();
            matches.sort_by(|a, b| {
                let a_base = a.starts_with("/java.base/");
                let b_base = b.starts_with("/java.base/");
                b_base.cmp(&a_base).then_with(|| a.cmp(b))
            });
            if let Some(path) = matches.into_iter().next() {
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

    pub fn load_class(&self, name: &[u8]) -> io::Result<Rc<InstanceKlass<'a>>> {
        self.load(name)
    }

    pub fn load(&self, name: &[u8]) -> io::Result<Rc<InstanceKlass<'a>>> {
        if name.starts_with(b"[") || name.starts_with(b":") {
            return Err(Error::other(format!(
                "Array or primitive class cannot be loaded by name: {}",
                String::from_utf8_lossy(name)
            )));
        }
        if let Some(klass) = self.classes.borrow().get(name) {
            return Ok(Rc::clone(klass));
        }
        if !self.loading.borrow_mut().insert(name.to_vec()) {
            return Err(super::linkageError::LinkageError::new(
                super::linkageError::LinkageErrorKind::ClassCircularity,
                format!("{}", String::from_utf8_lossy(name)),
            )
            .io());
        }
        let result = self.loadClass(name);
        self.loading.borrow_mut().remove(name);
        result
    }

    fn loadClass(&self, name: &[u8]) -> io::Result<Rc<InstanceKlass<'a>>> {
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
            let klass = Rc::new(InstanceKlass::new(synthetic_class, None, Vec::new())?);
            self.classes
                .borrow_mut()
                .insert(klass.class.name, Rc::clone(&klass));
            return Ok(klass);
        }

        let bytes = self.open_stream(name)?;
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
        self.defineParsed(class)
    }

    /// Defines a class from raw classfile bytes (used for runtime-generated
    /// classes such as invokedynamic lambda proxies). The bytes are copied
    /// into the loader's bump storage so the parsed `ClassFile` borrows stay
    /// valid for `'a`.
    pub fn define_class_bytes(&self, name: &[u8], bytes: &[u8]) -> io::Result<Rc<InstanceKlass<'a>>> {
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
        self.defineParsed(class)
    }

    /// Links an already-parsed ClassFile: resolves the superclass and interfaces,
    /// builds the InstanceKlass and registers it. Used both for classes loaded
    /// from classpath/image sources and for synthetic classes generated at
    /// runtime (e.g. invokedynamic lambda classes).
    fn defineParsed(&self, class: ClassFile<'a>) -> io::Result<Rc<InstanceKlass<'a>>> {
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

        let klass = Rc::new(InstanceKlass::new(class, super_klass, interfaces)?);
        self.classes
            .borrow_mut()
            .insert(klass.class.name, Rc::clone(&klass));
        Ok(klass)
    }

    /// Defines a synthetic class from an already-parsed ClassFile structure.
    /// All byte slices inside `class` must be allocated in this loader's
    /// storage (or be 'static) so that they live for 'a.
    pub fn define_class(&self, class: ClassFile<'a>) -> io::Result<Rc<InstanceKlass<'a>>> {
        self.defineParsed(class)
    }

    /// Returns the cached synthetic lambda class for an invokedynamic call
    /// site, or `None`.
    pub(crate) fn lambda_class(
        &self,
        caller: &[u8],
        bsm_index: u16,
    ) -> Option<Rc<InstanceKlass<'a>>> {
        self.lambda_cache
            .borrow()
            .get(&(caller.to_vec(), bsm_index))
            .map(Rc::clone)
    }

    /// Caches a generated lambda class for an invokedynamic call site.
    pub(crate) fn cache_lambda_class(
        &self,
        caller: &[u8],
        bsm_index: u16,
        klass: Rc<InstanceKlass<'a>>,
    ) {
        self.lambda_cache
            .borrow_mut()
            .insert((caller.to_vec(), bsm_index), klass);
    }

    pub fn systemClassLoaderObject(&self) -> io::Result<usize> {
        if let Some(handle) = *self.class_loader_object.borrow() {
            return Ok(handle);
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
