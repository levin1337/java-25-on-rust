/*
 * Port of OpenJDK 25 HotSpot Metadata (share/oops/metadata.hpp, metadata.cpp)
 *
 * Base metadata trait and enumeration for HotSpot Metaspace metadata objects:
 * Klass, Method, ConstMethod, MethodCounters, ConstantPool, ConstantPoolCache.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataType {
    Klass,
    Method,
    ConstMethod,
    MethodData,
    ConstantPool,
    ConstantPoolCache,
    MethodCounters,
    RecordComponent,
}

pub trait Metadata {
    fn metadata_type(&self) -> MetadataType;
    fn size_in_words(&self) -> usize;
    fn internal_name(&self) -> &str;

    fn is_klass(&self) -> bool {
        self.metadata_type() == MetadataType::Klass
    }

    fn is_method(&self) -> bool {
        self.metadata_type() == MetadataType::Method
    }

    fn is_constant_pool(&self) -> bool {
        self.metadata_type() == MetadataType::ConstantPool
    }

    fn is_method_counters(&self) -> bool {
        self.metadata_type() == MetadataType::MethodCounters
    }
}
