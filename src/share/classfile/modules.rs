/*
 * Port of OpenJDK 25 HotSpot Modules & Module System
 * (share/classfile/modules.hpp / modules.cpp)
 *
 * Implements JEP 261 Module System runtime checks:
 * module entries, readability graphs (reads), export tables (qualified & unqualified),
 * and open packages for deep reflection, with java.base and unnamed module support.
 */

use crate::share::oops::symbol::Symbol;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

#[derive(Clone, Debug)]
pub struct ModuleEntry {
    name: Option<Symbol>,
    version: Option<Symbol>,
    location: Option<Symbol>,
    is_open: bool,
    reads: HashSet<Symbol>,
    // package name -> set of target module names. If empty, exported/open to ALL (unqualified)
    exports: HashMap<String, HashSet<Symbol>>,
    opens: HashMap<String, HashSet<Symbol>>,
}

impl ModuleEntry {
    pub fn new_unnamed() -> Self {
        Self {
            name: None,
            version: None,
            location: None,
            is_open: true,
            reads: HashSet::new(),
            exports: HashMap::new(),
            opens: HashMap::new(),
        }
    }

    pub fn new_named(
        name: Symbol,
        version: Option<Symbol>,
        location: Option<Symbol>,
        is_open: bool,
    ) -> Self {
        Self {
            name: Some(name),
            version,
            location,
            is_open,
            reads: HashSet::new(),
            exports: HashMap::new(),
            opens: HashMap::new(),
        }
    }

    #[inline]
    pub fn name(&self) -> Option<&Symbol> {
        self.name.as_ref()
    }

    #[inline]
    pub fn is_named(&self) -> bool {
        self.name.is_some()
    }

    #[inline]
    pub fn is_open_module(&self) -> bool {
        self.is_open
    }

    #[inline]
    pub fn version(&self) -> Option<&Symbol> {
        self.version.as_ref()
    }

    #[inline]
    pub fn location(&self) -> Option<&Symbol> {
        self.location.as_ref()
    }

    pub fn add_read(&mut self, target_module: Symbol) {
        self.reads.insert(target_module);
    }

    pub fn can_read(&self, target_module: &Symbol) -> bool {
        // Unnamed module can read all modules
        if !self.is_named() {
            return true;
        }
        // Every module can read itself
        if let Some(ref name) = self.name {
            if name == target_module {
                return true;
            }
        }
        // Every module can read java.base
        if target_module.as_str() == "java.base" {
            return true;
        }
        self.reads.contains(target_module)
    }

    pub fn add_export(&mut self, package: String, target: Option<Symbol>) {
        let entry = self.exports.entry(package).or_default();
        if let Some(t) = target {
            entry.insert(t);
        }
    }

    pub fn is_exported_to(&self, package: &str, target_module: Option<&Symbol>) -> bool {
        // Unnamed module exports all packages unqualifiedly
        if !self.is_named() {
            return true;
        }

        if let Some(targets) = self.exports.get(package) {
            // Unqualified export if target set is empty
            if targets.is_empty() {
                return true;
            }
            if let Some(t) = target_module {
                return targets.contains(t);
            }
        }
        false
    }

    pub fn add_open(&mut self, package: String, target: Option<Symbol>) {
        let entry = self.opens.entry(package).or_default();
        if let Some(t) = target {
            entry.insert(t);
        }
    }

    pub fn is_open_to(&self, package: &str, target_module: Option<&Symbol>) -> bool {
        // Unnamed module or open module is open to everyone
        if !self.is_named() || self.is_open {
            return true;
        }

        if let Some(targets) = self.opens.get(package) {
            if targets.is_empty() {
                return true;
            }
            if let Some(t) = target_module {
                return targets.contains(t);
            }
        }
        false
    }
}

pub struct ModuleTable {
    modules: HashMap<String, ModuleEntry>,
    unnamed_module: ModuleEntry,
}

impl ModuleTable {
    pub fn new() -> Self {
        let mut table = Self {
            modules: HashMap::new(),
            unnamed_module: ModuleEntry::new_unnamed(),
        };

        // Standard java.base module
        let mut java_base = ModuleEntry::new_named(
            Symbol::from("java.base"),
            Some(Symbol::from("25")),
            Some(Symbol::from("jrt:/java.base")),
            false,
        );
        // java.base exports essential packages
        java_base.add_export("java/lang".to_string(), None);
        java_base.add_export("java/io".to_string(), None);
        java_base.add_export("java/util".to_string(), None);
        java_base.add_export("java/util/concurrent".to_string(), None);
        table.modules.insert("java.base".to_string(), java_base);

        table
    }

    pub fn get(&self, name: &str) -> Option<&ModuleEntry> {
        self.modules.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut ModuleEntry> {
        self.modules.get_mut(name)
    }

    pub fn unnamed_module(&self) -> &ModuleEntry {
        &self.unnamed_module
    }

    pub fn unnamed_module_mut(&mut self) -> &mut ModuleEntry {
        &mut self.unnamed_module
    }

    pub fn define_module(
        &mut self,
        name: &str,
        version: Option<&str>,
        location: Option<&str>,
        is_open: bool,
    ) -> Result<&mut ModuleEntry, &'static str> {
        if self.modules.contains_key(name) {
            return Err("Module already defined");
        }

        let entry = ModuleEntry::new_named(
            Symbol::from(name),
            version.map(Symbol::from),
            location.map(Symbol::from),
            is_open,
        );
        self.modules.insert(name.to_string(), entry);
        Ok(self.modules.get_mut(name).unwrap())
    }
}

impl Default for ModuleTable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Modules {
    table: RwLock<ModuleTable>,
}

impl Modules {
    pub fn new() -> Self {
        Self {
            table: RwLock::new(ModuleTable::new()),
        }
    }

    pub fn define_module(
        &self,
        name: &str,
        version: Option<&str>,
        location: Option<&str>,
        is_open: bool,
    ) -> Result<(), &'static str> {
        let mut table = self.table.write().unwrap();
        table.define_module(name, version, location, is_open)?;
        Ok(())
    }

    pub fn add_module_reads(&self, from: &str, to: &str) -> Result<(), &'static str> {
        let mut table = self.table.write().unwrap();
        if let Some(entry) = table.get_mut(from) {
            entry.add_read(Symbol::from(to));
            Ok(())
        } else {
            Err("Source module not found")
        }
    }

    pub fn add_module_exports(
        &self,
        from: &str,
        package: &str,
        to: Option<&str>,
    ) -> Result<(), &'static str> {
        let mut table = self.table.write().unwrap();
        if let Some(entry) = table.get_mut(from) {
            entry.add_export(package.to_string(), to.map(Symbol::from));
            Ok(())
        } else {
            Err("Source module not found")
        }
    }

    pub fn add_module_opens(
        &self,
        from: &str,
        package: &str,
        to: Option<&str>,
    ) -> Result<(), &'static str> {
        let mut table = self.table.write().unwrap();
        if let Some(entry) = table.get_mut(from) {
            entry.add_open(package.to_string(), to.map(Symbol::from));
            Ok(())
        } else {
            Err("Source module not found")
        }
    }

    pub fn can_read(&self, from: &str, to: &str) -> bool {
        let table = self.table.read().unwrap();
        if let Some(from_entry) = table.get(from) {
            from_entry.can_read(&Symbol::from(to))
        } else {
            // Unnamed module can read everything
            true
        }
    }

    pub fn is_exported_to(&self, from: &str, package: &str, to: Option<&str>) -> bool {
        let table = self.table.read().unwrap();
        if let Some(entry) = table.get(from) {
            entry.is_exported_to(package, to.map(Symbol::from).as_ref())
        } else {
            true
        }
    }

    pub fn is_open_to(&self, from: &str, package: &str, to: Option<&str>) -> bool {
        let table = self.table.read().unwrap();
        if let Some(entry) = table.get(from) {
            entry.is_open_to(package, to.map(Symbol::from).as_ref())
        } else {
            true
        }
    }
}

impl Default for Modules {
    fn default() -> Self {
        Self::new()
    }
}
