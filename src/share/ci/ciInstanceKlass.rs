/*
 * Port of OpenJDK 25 HotSpot ciInstanceKlass (share/ci/ciInstanceKlass.hpp)
 *
 * Compiler interface snapshot of an InstanceKlass.
 */

use super::ciField::CIField;
use super::ciMethod::CIMethod;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CIInstanceKlass {
    name: String,
    super_name: Option<String>,
    fields: Vec<CIField>,
    methods: Vec<CIMethod>,
}

impl CIInstanceKlass {
    pub fn new(name: String, super_name: Option<String>) -> Self {
        Self {
            name,
            super_name,
            fields: Vec::new(),
            methods: Vec::new(),
        }
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn super_name(&self) -> Option<&str> {
        self.super_name.as_deref()
    }

    pub fn add_field(&mut self, field: CIField) {
        self.fields.push(field);
    }

    pub fn add_method(&mut self, method: CIMethod) {
        self.methods.push(method);
    }

    #[inline]
    pub fn fields(&self) -> &[CIField] {
        &self.fields
    }

    #[inline]
    pub fn methods(&self) -> &[CIMethod] {
        &self.methods
    }

    pub fn find_method(&self, name: &str, sig: &str) -> Option<&CIMethod> {
        self.methods
            .iter()
            .find(|m| m.name() == name && m.signature() == sig)
    }

    pub fn find_field(&self, name: &str) -> Option<&CIField> {
        self.fields.iter().find(|f| f.name() == name)
    }
}
