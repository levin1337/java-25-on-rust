/*
 * Port of OpenJDK 25 HotSpot ciKlassCache (share/ci/ciKlassCache.hpp/.cpp)
 *
 * The VM keeps one ciKlass* handle per loaded Klass*.  This port keys
 * the cache by internal class name and stores the resolved
 * `Arc<InstanceKlass>`; ci-level wrappers (`CiInstanceKlass`) are
 * built on demand since they are cheap clones of `Arc + CiEnv`.
 */

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::oops::instanceKlass::InstanceKlass;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// `ciKlassCache`: name -> resolved `InstanceKlass` cache shared by
/// all `CiEnv` clones over the same class loader.
pub struct CiKlassCache<'a> {
    loader: Arc<ClassLoader<'a>>,
    /// internal name (`java/lang/Object`) -> resolved klass.
    map: Mutex<HashMap<Vec<u8>, Arc<InstanceKlass<'a>>>>,
}

impl<'a> CiKlassCache<'a> {
    pub fn new(loader: Arc<ClassLoader<'a>>) -> Self {
        Self {
            loader,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// `ciKlassCache::get`: cached klass or `None` when absent —
    /// does NOT trigger a class load (HotSpot only caches already-
    /// loaded klasses).
    pub fn peek(&self, name: &[u8]) -> Option<Arc<InstanceKlass<'a>>> {
        self.map.lock().unwrap().get(name).map(Arc::clone)
    }

    /// `ciKlassCache::get` with load-through: resolves `name` through
    /// the class loader and caches the result.
    pub fn get_or_load(&self, name: &[u8]) -> std::io::Result<Arc<InstanceKlass<'a>>> {
        if let Some(k) = self.peek(name) {
            return Ok(k);
        }
        let klass = self.loader.load(name)?;
        self.map
            .lock()
            .unwrap()
            .insert(name.to_vec(), Arc::clone(&klass));
        Ok(klass)
    }

    /// Insert a klass resolved elsewhere (e.g. obtained from the
    /// loader's class table directly).
    pub fn insert(&self, name: &[u8], klass: Arc<InstanceKlass<'a>>) {
        self.map
            .lock()
            .unwrap()
            .insert(name.to_vec(), klass);
    }

    /// `ciKlassCache::is_loaded`: present in the cache.
    #[inline]
    pub fn is_cached(&self, name: &[u8]) -> bool {
        self.peek(name).is_some()
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    /// Snapshot of all cached klasses (replay/debug tooling).
    pub fn all(&self) -> Vec<Arc<InstanceKlass<'a>>> {
        self.map.lock().unwrap().values().map(Arc::clone).collect()
    }

    /// Drop all entries (e.g. after a class redefinition — the port
    /// has no redefinition, kept for API parity).
    pub fn clear(&self) {
        self.map.lock().unwrap().clear();
    }
}

impl<'a> std::fmt::Debug for CiKlassCache<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CiKlassCache({} entries)", self.len())
    }
}
