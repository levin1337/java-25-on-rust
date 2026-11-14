/*
 * Port of OpenJDK 25 HotSpot ciField (share/ci/ciField.hpp)
 *
 * Compiler interface representation of a class field.
 */

use super::ciType::CIType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CIField {
    holder_name: String,
    name: String,
    field_type: CIType,
    offset: usize,
    is_static: bool,
    is_final: bool,
    is_volatile: bool,
}

impl CIField {
    pub fn new(
        holder_name: String,
        name: String,
        field_type: CIType,
        offset: usize,
        is_static: bool,
        is_final: bool,
        is_volatile: bool,
    ) -> Self {
        Self {
            holder_name,
            name,
            field_type,
            offset,
            is_static,
            is_final,
            is_volatile,
        }
    }

    #[inline]
    pub fn holder_name(&self) -> &str {
        &self.holder_name
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn field_type(&self) -> &CIType {
        &self.field_type
    }

    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        self.is_static
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        self.is_final
    }

    #[inline]
    pub fn is_volatile(&self) -> bool {
        self.is_volatile
    }
}
