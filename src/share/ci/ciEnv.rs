/*
 * Port of OpenJDK 25 HotSpot ciEnv (share/ci/ciEnv.hpp/.cpp)
 *
 * ciEnv is the facade through which the compilers query class metadata.
 * It caches ciInstanceKlass handles per loaded klass (the analogue of
 * HotSpot's ciObjectFactory/ciKlassCache) and resolves methods and
 * fields, including canonical-holder lookup through superclasses.
 *
 * The environment is cheap to clone: all state is shared through Arc,
 * so compiler threads can each hold a CiEnv over the same ClassLoader.
 */

use super::ciField::CiField;
use super::ciInstanceKlass::CiInstanceKlass;
use super::ciMethod::CiMethod;
use super::ciType::CIType;
use crate::share::classfile::classLoader::ClassLoader;
use crate::share::oops::instanceKlass::InstanceKlass;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct CiEnv<'a> {
    loader: Arc<ClassLoader<'a>>,
    /// ciObjectFactory analogue: one ciInstanceKlass per loaded Klass*.
    klass_cache: Arc<Mutex<HashMap<Vec<u8>, CiInstanceKlass<'a>>>>,
}

impl<'a> CiEnv<'a> {
    /// `ciEnv` construction over an explicit class loader.
    pub fn new(loader: Arc<ClassLoader<'a>>) -> Self {
        Self {
            loader,
            klass_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// `ciEnv::current` analogue for compiler threads: uses the
    /// JVM-wide loader installed by `threading::install_loader`.
    /// Returns `None` when no loader has been installed (early VM
    /// bootstrap) — callers should treat that as "metadata unavailable".
    pub fn from_jvm() -> Option<CiEnv<'static>> {
        let loader = crate::share::runtime::threading::jvm_loader()?;
        Some(CiEnv::<'static>::new(loader))
    }

    /// The underlying VM class loader.
    #[inline]
    pub fn loader(&self) -> &Arc<ClassLoader<'a>> {
        &self.loader
    }

    /// Raw access to the InstanceKlass store for callers that need
    /// oops-level data not surfaced through the ci facade.
    #[inline]
    pub fn load_klass(&self, name: &[u8]) -> std::io::Result<Arc<InstanceKlass<'a>>> {
        self.loader.load(name)
    }

    /// `ciEnv::get_klass` / `lookup_klass`: resolve an internal class
    /// name (`java/lang/Object`, no `L...;` wrapper) to a ci klass.
    /// Loads the class through the loader when not yet cached.
    pub fn get_klass(&self, name: &[u8]) -> Option<CiInstanceKlass<'a>> {
        if let Some(k) = self.klass_cache.lock().unwrap().get(name) {
            return Some(k.clone());
        }
        let klass = self.loader.load(name).ok()?;
        let ci = CiInstanceKlass::new(self.clone(), Arc::clone(&klass));
        self.klass_cache
            .lock()
            .unwrap()
            .insert(name.to_vec(), ci.clone());
        Some(ci)
    }

    /// `ciEnv::get_klass_or_null`.
    #[inline]
    pub fn get_klass_or_null(&self, name: &[u8]) -> Option<CiInstanceKlass<'a>> {
        self.get_klass(name)
    }

    /// `ciEnv::is_loaded`: whether the class is already present in the
    /// loader's class table (no load attempted).
    pub fn is_loaded(&self, name: &[u8]) -> bool {
        self.loader.classes.borrow().contains_key(name)
    }

    /// Wrap an already-resolved `Arc<InstanceKlass>` without going
    /// through the name cache (used when the caller holds the klass,
    /// e.g. from `ClassLoader::classes`).
    pub fn get_klass_from_instance(&self, klass: Arc<InstanceKlass<'a>>) -> CiInstanceKlass<'a> {
        CiInstanceKlass::new(self.clone(), klass)
    }

    /// `ciEnv::lookup_method`: find a method by name+signature,
    /// searching the holder and its superclasses (HotSpot `get_method`
    /// walks the super chain; interface default methods are consulted
    /// afterwards for non-private lookups).
    pub fn lookup_method(
        &self,
        holder: &CiInstanceKlass<'a>,
        name: &[u8],
        signature: &[u8],
    ) -> Option<CiMethod<'a>> {
        let mut current = Some(holder.clone());
        while let Some(klass) = current {
            if let Some(m) = klass.find_method(name, signature) {
                return Some(m);
            }
            current = klass.super_klass();
        }
        // HotSpot resolves interface methods through the itable/maximally-
        // specific-superinterface walk; consult transitive interfaces as a
        // fallback for default methods.
        for iface in holder.transitive_interfaces() {
            if let Some(m) = iface.find_method(name, signature) {
                if !m.is_abstract() {
                    return Some(m);
                }
            }
        }
        None
    }

    /// `ciEnv::get_field`: resolve a field to its canonical holder.
    /// Walks superclasses (and interfaces for static fields) exactly
    /// like `InstanceKlass::find_field` in HotSpot.
    pub fn get_field(
        &self,
        holder: &CiInstanceKlass<'a>,
        name: &[u8],
        signature: &[u8],
        is_static: bool,
    ) -> Option<CiField<'a>> {
        let mut current = Some(holder.clone());
        while let Some(klass) = current {
            if let Some(f) = klass.find_field(name, signature, is_static) {
                return Some(f);
            }
            current = klass.super_klass();
        }
        for iface in holder.transitive_interfaces() {
            if let Some(f) = iface.find_field(name, signature, is_static) {
                return Some(f);
            }
        }
        None
    }

    /// `ciEnv::get_field_by_offset` analogue for the common case of a
    /// declared (non-inherited) field: searches `holder` and its super
    /// chain for a field occupying `offset`.
    pub fn get_field_by_offset(
        &self,
        holder: &CiInstanceKlass<'a>,
        offset: i32,
        is_static: bool,
    ) -> Option<CiField<'a>> {
        let mut current = Some(holder.clone());
        while let Some(klass) = current {
            if let Some(f) = klass.get_field_by_offset(offset, is_static) {
                return Some(f);
            }
            current = klass.super_klass();
        }
        None
    }

    /// Resolve a field/method descriptor to a `CIType` through this env.
    #[inline]
    pub fn type_from_descriptor(&self, desc: &[u8]) -> Option<CIType<'a>> {
        CIType::from_descriptor(self, desc)
    }

    /// Number of classes currently present in the loader's class table.
    pub fn loaded_class_count(&self) -> usize {
        self.loader.classes.borrow().len()
    }

    /// Snapshot of all loaded klasses as ci handles (ciKlassCache dump
    /// analogue, used by replay tooling).
    pub fn all_klasses(&self) -> Vec<CiInstanceKlass<'a>> {
        self.loader
            .classes
            .borrow()
            .values()
            .map(|k| CiInstanceKlass::new(self.clone(), Arc::clone(k)))
            .collect()
    }
}
