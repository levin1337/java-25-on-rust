/*
 * Port of OpenJDK 25 HotSpot JavaAssertions
 * (share/classfile/javaAssertions.hpp / javaAssertions.cpp)
 *
 * Implements command-line Java assertion controls (-ea, -da, -esa, -dsa),
 * package assertion status hierarchy, and class assertion queries.
 */

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct JavaAssertions {
    user_default: bool,
    system_default: bool,
    package_assertions: HashMap<String, bool>,
    class_assertions: HashMap<String, bool>,
}

impl Default for JavaAssertions {
    fn default() -> Self {
        Self {
            user_default: false,
            system_default: false,
            package_assertions: HashMap::new(),
            class_assertions: HashMap::new(),
        }
    }
}

impl JavaAssertions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_user_default(&mut self, enabled: bool) {
        self.user_default = enabled;
    }

    pub fn set_system_default(&mut self, enabled: bool) {
        self.system_default = enabled;
    }

    pub fn add_package(&mut self, package_name: &str, enabled: bool) {
        let pkg = package_name.strip_suffix("...").unwrap_or(package_name);
        self.package_assertions
            .insert(pkg.replace('.', "/"), enabled);
    }

    pub fn add_class(&mut self, class_name: &str, enabled: bool) {
        self.class_assertions
            .insert(class_name.replace('.', "/"), enabled);
    }

    /// Process a standard JVM assertion argument (-ea, -da, -esa, -dsa, etc.)
    pub fn process_arg(&mut self, arg: &str) -> bool {
        if arg == "-ea" || arg == "-enableassertions" {
            self.set_user_default(true);
            true
        } else if arg == "-da" || arg == "-disableassertions" {
            self.set_user_default(false);
            true
        } else if arg == "-esa" || arg == "-enablesystemassertions" {
            self.set_system_default(true);
            true
        } else if arg == "-dsa" || arg == "-disablesystemassertions" {
            self.set_system_default(false);
            true
        } else if let Some(target) = arg
            .strip_prefix("-ea:")
            .or_else(|| arg.strip_prefix("-enableassertions:"))
        {
            if target.ends_with("...") {
                self.add_package(target, true);
            } else {
                self.add_class(target, true);
            }
            true
        } else if let Some(target) = arg
            .strip_prefix("-da:")
            .or_else(|| arg.strip_prefix("-disableassertions:"))
        {
            if target.ends_with("...") {
                self.add_package(target, false);
            } else {
                self.add_class(target, false);
            }
            true
        } else {
            false
        }
    }

    /// Checks if assertions are enabled for a given class name
    pub fn enabled(&self, class_name: &str) -> bool {
        let class_path = class_name.replace('.', "/");

        // 1. Direct class match
        if let Some(&status) = self.class_assertions.get(&class_path) {
            return status;
        }

        // 2. Longest prefix package match
        let mut best_match: Option<(&str, bool)> = None;
        for (pkg, &status) in &self.package_assertions {
            if class_path.starts_with(pkg) {
                if best_match.map_or(true, |(best, _)| pkg.len() > best.len()) {
                    best_match = Some((pkg.as_str(), status));
                }
            }
        }
        if let Some((_, status)) = best_match {
            return status;
        }

        // 3. System class check
        if class_path.starts_with("java/") || class_path.starts_with("jdk/") {
            return self.system_default;
        }

        // 4. Fall back to user default
        self.user_default
    }
}
