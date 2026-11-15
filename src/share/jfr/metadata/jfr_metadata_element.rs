/*
 * OpenJDK 25 HotSpot jfr/metadata/jfrMetadataElement.hpp + .cpp port
 *
 * JfrMetadataElement — an element in the metadata descriptor tree
 * (metadata/class/field/setting/annotation), mirroring the Java
 * MetadataDescriptor model.
 */

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum JfrMetadataName {
    Metadata,
    Class(String),
    Field(String),
    Setting(String),
    Annotation(String),
    Other(String),
}

impl JfrMetadataName {
    pub fn as_str(&self) -> &str {
        match self {
            JfrMetadataName::Metadata => "metadata",
            JfrMetadataName::Class(s)
            | JfrMetadataName::Field(s)
            | JfrMetadataName::Setting(s)
            | JfrMetadataName::Annotation(s)
            | JfrMetadataName::Other(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JfrMetadataElement {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
    pub children: Vec<JfrMetadataElement>,
}

impl JfrMetadataElement {
    pub fn new(name: &str) -> Self {
        JfrMetadataElement {
            name: name.to_string(),
            attributes: BTreeMap::new(),
            children: Vec::new(),
        }
    }

    pub fn set_attribute(&mut self, name: &str, value: &str) {
        self.attributes.insert(name.to_string(), value.to_string());
    }

    pub fn get_attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(|s| s.as_str())
    }

    pub fn add_child(&mut self, child: JfrMetadataElement) {
        self.children.push(child);
    }

    pub fn find_child(&self, name: &str) -> Option<&JfrMetadataElement> {
        self.children.iter().find(|c| c.name == name)
    }
}
