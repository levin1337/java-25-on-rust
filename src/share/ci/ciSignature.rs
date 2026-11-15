/*
 * Port of OpenJDK 25 HotSpot ciSignature (share/ci/ciSignature.hpp/.cpp)
 *
 * ciSignature parses a method/field descriptor into its parameter and
 * return types with local-variable slot positions, and answers the
 * queries the C2 parser needs: `return_type`, `type_at_slot`,
 * `slot_at_type`, receiver offset and argument size.
 */

use super::ciEnv::CiEnv;
use super::ciType::{CIType, BasicType};

/// One parsed signature element: its ciType plus the local slot index
/// at which it starts (`-1` for the synthetic return slot entry).
#[derive(Debug, Clone)]
pub struct CiSignatureType<'a> {
    pub ty: CIType<'a>,
    /// First local slot occupied by this type; `None` marks the return
    /// type appended after the parameters.
    pub slot: Option<usize>,
}

/// `ciSignature`: a parsed `(params)return` descriptor.
#[derive(Debug, Clone)]
pub struct CiSignature<'a> {
    /// Raw descriptor bytes (`(II)Ljava/lang/String;`).
    pub signature: &'a [u8],
    /// Parameter types with their starting local slots.
    pub parameters: Vec<CiSignatureType<'a>>,
    /// Return type (slot `None`).
    pub return_type: CiSignatureType<'a>,
    /// Total argument slot count, receiver NOT included.
    pub size: usize,
    /// Whether the signature parses as a method descriptor.
    pub is_method: bool,
}

impl<'a> CiSignature<'a> {
    /// `ciSignature::as_symbol`-style construction: parse `signature`
    /// through `env`.  Returns `None` on malformed input or when a
    /// referenced class cannot be resolved.
    pub fn new(env: &CiEnv<'a>, signature: &'a [u8]) -> Option<Self> {
        Self::parse(env, signature, true)
    }

    /// `ciSignature` for a bare field descriptor (`I`, `[Ljava/lang/String;`).
    /// Treated as a single "parameter" with void return, matching
    /// HotSpot's field-signature use.
    pub fn from_field(env: &CiEnv<'a>, descriptor: &'a [u8]) -> Option<Self> {
        let (ty, used) = CIType::parse_field_type_at(env, descriptor)?;
        if used != descriptor.len() {
            return None;
        }
        let size = ty.size();
        Some(Self {
            signature: descriptor,
            parameters: vec![CiSignatureType {
                ty,
                slot: Some(0),
            }],
            return_type: CiSignatureType {
                ty: CIType::Void,
                slot: None,
            },
            size,
            is_method: false,
        })
    }

    fn parse(env: &CiEnv<'a>, signature: &'a [u8], is_method: bool) -> Option<Self> {
        if signature.first() != Some(&b'(') {
            return None;
        }
        let close = signature.iter().position(|&b| b == b')')?;
        let mut parameters = Vec::new();
        let mut slot = 0usize;
        let mut pos = 1usize;
        while pos < close {
            let (ty, len) = CIType::parse_field_type_at(env, &signature[pos..close])?;
            let size = ty.size();
            parameters.push(CiSignatureType {
                ty,
                slot: Some(slot),
            });
            slot += size;
            pos += len;
        }
        let (ret, ret_len) = CIType::parse_field_type_at(env, &signature[close + 1..])?;
        if ret_len != signature.len() - close - 1 {
            return None;
        }
        Some(Self {
            signature,
            size: slot,
            parameters,
            return_type: CiSignatureType {
                ty: ret,
                slot: None,
            },
            is_method,
        })
    }

    /// `ciSignature::return_type`.
    #[inline]
    pub fn return_type(&self) -> &CIType<'a> {
        &self.return_type.ty
    }

    /// `ciSignature::type_at_slot`: type occupying `slot` in the
    /// argument area (receiver excluded).  Two-slot types occupy their
    /// first slot only; the trailing slot reports the same type's
    /// `basic_type().size() == 2` partner — callers needing the slot
    /// owner should walk `parameters`.  Returns `None` outside the
    /// argument area.
    pub fn type_at_slot(&self, slot: usize) -> Option<CIType<'a>> {
        for p in &self.parameters {
            let start = p.slot?;
            let size = p.ty.size();
            if slot >= start && slot < start + size.max(1) {
                return Some(p.ty.clone());
            }
        }
        None
    }

    /// `ciSignature::slot_at_type` analogue: first slot of the
    /// `index`-th parameter (`None` when out of range).
    pub fn slot_at_index(&self, index: usize) -> Option<usize> {
        self.parameters.get(index).and_then(|p| p.slot)
    }

    /// `ciSignature::slot_count` including the receiver for instance
    /// methods (`ciMethod::arg_size` analogue).
    pub fn arg_size(&self, is_static: bool) -> usize {
        self.size + usize::from(!is_static)
    }

    /// Local slot index of parameter `index` including the receiver:
    /// `slot = index + (is_static ? 0 : 1)` adjusted for two-slot
    /// predecessors.  `None` when out of range.
    pub fn local_slot(&self, index: usize, is_static: bool) -> Option<usize> {
        let mut slot = usize::from(!is_static);
        for (i, p) in self.parameters.iter().enumerate() {
            if i == index {
                return p.slot.map(|s| slot + s);
            }
            slot += p.ty.size();
        }
        None
    }

    /// Parameter count.
    #[inline]
    pub fn parameter_count(&self) -> usize {
        self.parameters.len()
    }

    /// `ciSignature::has_return` — non-void return.
    #[inline]
    pub fn has_return(&self) -> bool {
        !self.return_type.ty.is_void()
    }

    /// `ciSignature::basic_type` of the return type.
    #[inline]
    pub fn return_basic_type(&self) -> BasicType {
        self.return_type.ty.basic_type()
    }
}
