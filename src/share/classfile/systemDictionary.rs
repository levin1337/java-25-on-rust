/*
 * Port of OpenJDK 25 HotSpot SystemDictionary (share/classfile/systemDictionary.hpp, systemDictionary.cpp)
 *
 * SystemDictionary tracks loaded classes, manages class loading resolution,
 * detects ClassCircularityErrors via placeholders, and checks subtyping/interface
 * implementation relationships.
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::oops::instanceKlass::InstanceKlass;
use std::collections::{HashMap, HashSet};
use std::io::{self, Error};
use std::sync::Arc;
use std::sync::RwLock;

// Placeholder entry used during class loading to detect ClassCircularityError
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaceholderKey {
    pub name: Vec<u8>,
    pub loader_id: usize,
}

#[derive(Default)]
pub struct PlaceholderTable {
    entries: RwLock<HashSet<PlaceholderKey>>,
}

impl PlaceholderTable {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashSet::new()),
        }
    }

    pub fn insert(&self, name: &[u8], loader_id: usize) -> bool {
        let key = PlaceholderKey {
            name: name.to_vec(),
            loader_id,
        };
        self.entries.write().unwrap().insert(key)
    }

    pub fn remove(&self, name: &[u8], loader_id: usize) -> bool {
        let key = PlaceholderKey {
            name: name.to_vec(),
            loader_id,
        };
        self.entries.write().unwrap().remove(&key)
    }

    pub fn contains(&self, name: &[u8], loader_id: usize) -> bool {
        let key = PlaceholderKey {
            name: name.to_vec(),
            loader_id,
        };
        self.entries.read().unwrap().contains(&key)
    }

    pub fn clear(&self) {
        self.entries.write().unwrap().clear();
    }
}

// Dictionary storing loaded classes per loader
#[derive(Default)]
pub struct Dictionary<'a> {
    entries: RwLock<HashMap<Vec<u8>, Arc<InstanceKlass<'a>>>>,
}

impl<'a> Dictionary<'a> {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub fn find_class(&self, name: &[u8]) -> Option<Arc<InstanceKlass<'a>>> {
        self.entries.read().unwrap().get(name).cloned()
    }

    pub fn add_class(&self, name: &[u8], klass: Arc<InstanceKlass<'a>>) {
        self.entries.write().unwrap().insert(name.to_vec(), klass);
    }

    pub fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().unwrap().is_empty()
    }

    pub fn clear(&self) {
        self.entries.write().unwrap().clear();
    }
}

pub struct SystemDictionary;

impl SystemDictionary {
    pub fn is_subclass_of<'a>(sub: &InstanceKlass<'a>, super_name: &[u8]) -> bool {
        if sub.class.name == super_name {
            return true;
        }
        let mut current = sub.super_klass();
        while let Some(k) = current {
            if k.class.name == super_name {
                return true;
            }
            current = k.super_klass();
        }
        false
    }

    pub fn implements_interface<'a>(klass: &InstanceKlass<'a>, interface_name: &[u8]) -> bool {
    let _rg = crate::share::recprobe::Guard::enter("implements_interface");
        for iface in &klass.interfaces {
            if iface.class.name == interface_name {
                return true;
            }
            if Self::implements_interface(iface, interface_name) {
                return true;
            }
        }
        let mut current = klass.super_klass();
        while let Some(k) = current {
            if Self::implements_interface(&k, interface_name) {
                return true;
            }
            current = k.super_klass();
        }
        false
    }

    pub fn find_instance_klass<'a>(
        loader: &ClassLoader<'a>,
        class_name: &[u8],
    ) -> Option<Arc<InstanceKlass<'a>>> {
        loader.classes.borrow().get(class_name).cloned()
    }

    pub fn resolve_or_null<'a>(
        loader: &ClassLoader<'a>,
        class_name: &[u8],
    ) -> Option<Arc<InstanceKlass<'a>>> {
        loader.load(class_name).ok()
    }

    pub fn resolve_or_fail<'a>(
        loader: &ClassLoader<'a>,
        class_name: &[u8],
        throw_error: bool,
    ) -> io::Result<Arc<InstanceKlass<'a>>> {
        match loader.load(class_name) {
            Ok(klass) => Ok(klass),
            Err(e) => {
                if throw_error {
                    Err(e)
                } else {
                    Err(Error::other(format!(
                        "ClassNotFoundException: {}",
                        String::from_utf8_lossy(class_name)
                    )))
                }
            }
        }
    }

    pub fn resolve_with_circularity_detection<'a>(
        loader: &ClassLoader<'a>,
        class_name: &[u8],
        next_name: &[u8],
        is_superclass: bool,
    ) -> io::Result<Arc<InstanceKlass<'a>>> {
        if class_name == next_name {
            return Err(Error::other("ClassCircularityError"));
        }
        let resolved = loader.load(next_name)?;
        if is_superclass && Self::is_subclass_of(&resolved, class_name) {
            return Err(Error::other("ClassCircularityError"));
        }
        Ok(resolved)
    }

    pub fn resolve_super_or_fail<'a>(
        loader: &ClassLoader<'a>,
        class_name: &[u8],
        super_name: &[u8],
        is_superclass: bool,
    ) -> io::Result<Arc<InstanceKlass<'a>>> {
        Self::resolve_with_circularity_detection(loader, class_name, super_name, is_superclass)
    }

    pub fn check_signature_loaders(signature: &[u8]) -> bool {
        let mut idx = 0;
        while idx < signature.len() {
            if signature[idx] == b'L' {
                if let Some(end) = signature[idx..].iter().position(|&b| b == b';') {
                    idx += end + 1;
                } else {
                    return false;
                }
            } else {
                idx += 1;
            }
        }
        true
    }

    pub fn is_well_known_class(name: &[u8]) -> bool {
        matches!(
            name,
            b"java/lang/Object"
                | b"java/lang/String"
                | b"java/lang/Class"
                | b"java/lang/Cloneable"
                | b"java/io/Serializable"
                | b"java/lang/Throwable"
                | b"java/lang/Thread"
                | b"java/lang/System"
        )
    }
}
