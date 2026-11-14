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
use crate::{classfile::ClassFile, share::oops::instanceKlass::InstanceKlass};
use bumpalo::Bump;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Error, Read},
    path::{Path, PathBuf},
    rc::Rc,
};
use zip::ZipArchive;

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
    Jar { archive: ZipArchive<File>, path: PathBuf },
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
    class_loader_object: RefCell<Option<usize>>,
    current_thread_object: RefCell<Option<usize>>,
    loader_type: ClassLoaderType,
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

    pub(crate) fn referenceAssignable(&self, value: Value<'a>, target: &[u8]) -> io::Result<bool> {
        Ok(match value {
            Value::Null => true,
            Value::Object(handle) => {
                let objects = self.objects.borrow();
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
            systemProperties: RefCell::new(HashMap::new()),
            arrays: RefCell::new(Vec::new()),
            objects: RefCell::new(Vec::new()),
            obj_arrays: RefCell::new(Vec::new()),
            class_loader_object: RefCell::new(None),
            current_thread_object: RefCell::new(None),
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
            systemProperties: RefCell::new(HashMap::new()),
            arrays: RefCell::new(Vec::new()),
            objects: RefCell::new(Vec::new()),
            obj_arrays: RefCell::new(Vec::new()),
            class_loader_object: RefCell::new(None),
            current_thread_object: RefCell::new(None),
            loader_type: ClassLoaderType::AppLoader,
        })
    }

    pub fn add_source(&self, source: ClassSource) {
        self.sources.borrow_mut().push(source);
    }

    pub fn has_entry(&self, name: &[u8]) -> bool {
        let rel_path = match super::classFileParser::decodeString(name) {
            Ok(p) => p,
            Err(_) => return false,
        };
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

    pub fn resource_locations(&self, name: &[u8]) -> io::Result<Vec<PathBuf>> {
        let name = super::classFileParser::decodeString(name)?;
        let mut locations = Vec::new();
        if let Some(image) = &self.bootImage {
            let image = image.borrow();
            for path in image.paths_with_suffix(&name) {
                locations.push(PathBuf::from(path));
            }
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

        Err(Error::other(format!(
            "ClassNotFoundException: {}",
            String::from_utf8_lossy(name)
        )))
    }

    pub fn hasClass(&self, name: &[u8]) -> io::Result<bool> {
        let name = super::classFileParser::decodeString(name)?;
        if let Some(image) = &self.bootImage {
            return Ok(image.borrow().hasClass(&name));
        }
        Ok(self.sources.borrow_mut().iter_mut().any(|source| match source {
            ClassSource::Jar { archive: zip, .. } => zip.by_name(&format!("{name}.class")).is_ok(),
            ClassSource::Directory(dir) => dir.join(format!("{name}.class")).exists(),
        }))
    }

    pub fn load_class(&self, name: &[u8]) -> io::Result<Rc<InstanceKlass<'a>>> {
        self.load(name)
    }

    pub fn load(&self, name: &[u8]) -> io::Result<Rc<InstanceKlass<'a>>> {
        if let Some(klass) = self.classes.borrow().get(name) {
            return Ok(Rc::clone(klass));
        }
        if !self.loading.borrow_mut().insert(name.to_vec()) {
            return Err(Error::other("ClassCircularityError"));
        }
        let result = self.loadClass(name);
        self.loading.borrow_mut().remove(name);
        result
    }

    fn loadClass(&self, name: &[u8]) -> io::Result<Rc<InstanceKlass<'a>>> {
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
        let class = ClassFile::parse(bytes, false).map_err(Error::other)?;
        if class.name != name {
            return Err(Error::other("Class name mismatch"));
        }

        let super_klass = if let Some(super_name) = class.superName {
            Some(self.load(super_name)?)
        } else {
            None
        };

        let mut interfaces = Vec::with_capacity(class.interfaces.len());
        for iface_name in &class.interfaces {
            interfaces.push(self.load(iface_name)?);
        }

        let klass = Rc::new(InstanceKlass::new(class, super_klass, interfaces)?);
        self.classes
            .borrow_mut()
            .insert(klass.class.name, Rc::clone(&klass));
        Ok(klass)
    }

    pub fn systemClassLoaderObject(&self) -> io::Result<usize> {
        if let Some(handle) = *self.class_loader_object.borrow() {
            return Ok(handle);
        }
        let klass = self.load(b"java/lang/ClassLoader")?;
        let object = InstanceOop::new(klass)?;
        let mut objects = self.objects.borrow_mut();
        let handle = objects.len();
        objects.push(object);
        *self.class_loader_object.borrow_mut() = Some(handle);
        Ok(handle)
    }

    pub fn currentThreadObject(&self) -> io::Result<usize> {
        if let Some(handle) = *self.current_thread_object.borrow() {
            return Ok(handle);
        }
        let klass = self.load(b"java/lang/Thread")?;
        let object = InstanceOop::new(klass)?;
        let mut objects = self.objects.borrow_mut();
        let handle = objects.len();
        objects.push(object);
        *self.current_thread_object.borrow_mut() = Some(handle);
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
