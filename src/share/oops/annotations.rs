/*
 * Port of OpenJDK 25 HotSpot Annotations (share/oops/annotations.hpp / annotations.cpp)
 *
 * Stores runtime visible and invisible annotations for classes, fields, and types.
 * Supports JVM specification §4.7.16 (RuntimeVisibleAnnotations attribute) parsing
 * and representation.
 */

#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationElementValue {
    Byte(i8),
    Char(char),
    Double(f64),
    Float(f32),
    Int(i32),
    Long(i64),
    Short(i16),
    Boolean(bool),
    String(String),
    Enum {
        type_name: String,
        const_name: String,
    },
    Class(String),
    Annotation(Box<Annotation>),
    Array(Vec<AnnotationElementValue>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub type_name: String,
    pub element_value_pairs: Vec<(String, AnnotationElementValue)>,
}

impl Annotation {
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            element_value_pairs: Vec::new(),
        }
    }

    pub fn add_pair(&mut self, name: impl Into<String>, value: AnnotationElementValue) {
        self.element_value_pairs.push((name.into(), value));
    }

    pub fn get_value(&self, name: &str) -> Option<&AnnotationElementValue> {
        self.element_value_pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }
}

/// HotSpot Metaspace Annotations structure
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Annotations {
    class_annotations: Vec<u8>,
    fields_annotations: Vec<Vec<u8>>,
    class_type_annotations: Vec<u8>,
    fields_type_annotations: Vec<Vec<u8>>,
    parsed_class_annotations: Vec<Annotation>,
}

impl Annotations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn class_annotations(&self) -> &[u8] {
        &self.class_annotations
    }

    pub fn set_class_annotations(&mut self, bytes: Vec<u8>) {
        self.class_annotations = bytes;
    }

    pub fn fields_annotations(&self) -> &[Vec<u8>] {
        &self.fields_annotations
    }

    pub fn set_fields_annotations(&mut self, fields: Vec<Vec<u8>>) {
        self.fields_annotations = fields;
    }

    pub fn class_type_annotations(&self) -> &[u8] {
        &self.class_type_annotations
    }

    pub fn set_class_type_annotations(&mut self, bytes: Vec<u8>) {
        self.class_type_annotations = bytes;
    }

    pub fn fields_type_annotations(&self) -> &[Vec<u8>] {
        &self.fields_type_annotations
    }

    pub fn set_fields_type_annotations(&mut self, fields: Vec<Vec<u8>>) {
        self.fields_type_annotations = fields;
    }

    pub fn parsed_class_annotations(&self) -> &[Annotation] {
        &self.parsed_class_annotations
    }

    pub fn add_parsed_annotation(&mut self, annotation: Annotation) {
        self.parsed_class_annotations.push(annotation);
    }

    pub fn is_annotation_present(&self, type_name: &str) -> bool {
        self.parsed_class_annotations
            .iter()
            .any(|ann| ann.type_name == type_name)
    }

    pub fn is_deprecated(&self) -> bool {
        self.is_annotation_present("Ljava/lang/Deprecated;")
    }

    pub fn get_annotation(&self, type_name: &str) -> Option<&Annotation> {
        self.parsed_class_annotations
            .iter()
            .find(|ann| ann.type_name == type_name)
    }
}
