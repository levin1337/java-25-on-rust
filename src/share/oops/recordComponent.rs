/*
 * Port of OpenJDK 25 HotSpot RecordComponent (share/oops/recordComponent.hpp / recordComponent.cpp)
 *
 * Stores metadata for components of a Java record class (JEP 395).
 */

use super::annotations::Annotations;

#[derive(Clone, Debug, PartialEq)]
pub struct RecordComponent {
    pub name: String,
    pub descriptor: String,
    pub generic_signature: Option<String>,
    pub annotations: Option<Annotations>,
}

impl RecordComponent {
    pub fn new(
        name: impl Into<String>,
        descriptor: impl Into<String>,
        generic_signature: Option<String>,
        annotations: Option<Annotations>,
    ) -> Self {
        Self {
            name: name.into(),
            descriptor: descriptor.into(),
            generic_signature,
            annotations,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }

    pub fn generic_signature(&self) -> Option<&str> {
        self.generic_signature.as_deref()
    }

    pub fn annotations(&self) -> Option<&Annotations> {
        self.annotations.as_ref()
    }
}
