/*
 * Port of OpenJDK 25 HotSpot ciEnv (share/ci/ciEnv.hpp)
 *
 * ciEnv provides a standalone metadata environment allowing background JIT
 * compiler threads to query classes and compile methods without acquiring VM runtime locks.
 */

use super::ciInstanceKlass::CIInstanceKlass;
use super::ciMethod::CIMethod;
use std::collections::HashMap;

pub struct CIEnv {
    klasses: HashMap<String, CIInstanceKlass>,
}

impl CIEnv {
    pub fn new() -> Self {
        Self {
            klasses: HashMap::new(),
        }
    }

    pub fn register_klass(&mut self, klass: CIInstanceKlass) {
        self.klasses.insert(klass.name().to_string(), klass);
    }

    pub fn get_klass(&self, name: &str) -> Option<&CIInstanceKlass> {
        self.klasses.get(name)
    }

    pub fn find_method(&self, holder: &str, name: &str, sig: &str) -> Option<&CIMethod> {
        self.get_klass(holder)?.find_method(name, sig)
    }

    pub fn find_field(&self, holder: &str, name: &str) -> Option<&super::ciField::CIField> {
        self.get_klass(holder)?.find_field(name)
    }

    #[inline]
    pub fn count_klasses(&self) -> usize {
        self.klasses.len()
    }
}

impl Default for CIEnv {
    fn default() -> Self {
        Self::new()
    }
}
