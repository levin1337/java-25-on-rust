/*
 * Port of OpenJDK 25 HotSpot ciMethod (share/ci/ciMethod.hpp)
 *
 * Compiler interface representation of a method for JIT optimization.
 */

use super::ciType::CIType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CIMethod {
    holder_name: String,
    name: String,
    signature: String,
    return_type: CIType,
    max_stack: usize,
    max_locals: usize,
    code: Vec<u8>,
}

impl CIMethod {
    pub fn new(
        holder_name: String,
        name: String,
        signature: String,
        return_type: CIType,
        max_stack: usize,
        max_locals: usize,
        code: Vec<u8>,
    ) -> Self {
        Self {
            holder_name,
            name,
            signature,
            return_type,
            max_stack,
            max_locals,
            code,
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
    pub fn signature(&self) -> &str {
        &self.signature
    }

    #[inline]
    pub fn return_type(&self) -> &CIType {
        &self.return_type
    }

    #[inline]
    pub fn max_stack(&self) -> usize {
        self.max_stack
    }

    #[inline]
    pub fn max_locals(&self) -> usize {
        self.max_locals
    }

    #[inline]
    pub fn code_size(&self) -> usize {
        self.code.len()
    }

    #[inline]
    pub fn code(&self) -> &[u8] {
        &self.code
    }

    pub fn parameter_types(&self) -> Vec<CIType> {
        CIType::parse_signature(&self.signature)
            .map(|(params, _)| params)
            .unwrap_or_default()
    }
}
