/*
 * Port of OpenJDK 25 HotSpot BootstrapInfo (share/interpreter/bootstrapInfo.hpp, bootstrapInfo.cpp)
 *
 * BootstrapInfo holds resolution information for invokedynamic and condy
 * (constant dynamic) bootstrap specifiers extracted from the classfile BootstrapMethods attribute.
 */

use crate::classfile::Constant;
use crate::share::oops::symbol::Symbol;
use std::io::{self, Error};

#[derive(Debug, Clone, PartialEq)]
pub enum BootstrapArg {
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    String(String),
    Klass(String),
    MethodType(String),
    MethodHandle { kind: u8, index: u16 },
}

impl Eq for BootstrapArg {}

#[derive(Debug, Clone)]
pub struct BootstrapInfo {
    bss_index: usize,
    indy_index: i32,
    name: Symbol,
    signature: Symbol,
    bsm_ref: u16,
    arguments: Vec<BootstrapArg>,
    is_resolved: bool,
    resolved_target: Option<String>,
    resolved_method: Option<String>,
    resolved_appendix: Option<String>,
}

impl BootstrapInfo {
    pub fn new(
        bss_index: usize,
        indy_index: i32,
        name: Symbol,
        signature: Symbol,
        bsm_ref: u16,
        arguments: Vec<BootstrapArg>,
    ) -> Self {
        Self {
            bss_index,
            indy_index,
            name,
            signature,
            bsm_ref,
            arguments,
            is_resolved: false,
            resolved_target: None,
            resolved_method: None,
            resolved_appendix: None,
        }
    }

    #[inline]
    pub fn bss_index(&self) -> usize {
        self.bss_index
    }

    #[inline]
    pub fn indy_index(&self) -> i32 {
        self.indy_index
    }

    #[inline]
    pub fn is_method_call(&self) -> bool {
        self.indy_index != -1
    }

    #[inline]
    pub fn name(&self) -> &Symbol {
        &self.name
    }

    #[inline]
    pub fn signature(&self) -> &Symbol {
        &self.signature
    }

    #[inline]
    pub fn bsm_ref(&self) -> u16 {
        self.bsm_ref
    }

    #[inline]
    pub fn argc(&self) -> usize {
        self.arguments.len()
    }

    #[inline]
    pub fn arguments(&self) -> &[BootstrapArg] {
        &self.arguments
    }

    #[inline]
    pub fn is_resolved(&self) -> bool {
        self.is_resolved
    }

    #[inline]
    pub fn resolved_target(&self) -> Option<&str> {
        self.resolved_target.as_deref()
    }

    #[inline]
    pub fn resolved_method(&self) -> Option<&str> {
        self.resolved_method.as_deref()
    }

    #[inline]
    pub fn resolved_appendix(&self) -> Option<&str> {
        self.resolved_appendix.as_deref()
    }

    pub fn set_resolved(&mut self, target: String) {
        self.resolved_target = Some(target);
        self.is_resolved = true;
    }

    pub fn set_resolved_method(&mut self, method: String, appendix: Option<String>) {
        self.resolved_method = Some(method);
        self.resolved_appendix = appendix;
        self.is_resolved = true;
    }

    pub fn parse_from_attribute(
        attribute_bytes: &[u8],
        bsm_attr_index: u16,
        name: Symbol,
        signature: Symbol,
        constants: &[Constant],
    ) -> io::Result<Self> {
        if attribute_bytes.len() < 2 {
            return Err(Error::other("BootstrapMethods attribute truncated"));
        }
        let num_bsms = u16::from_be_bytes([attribute_bytes[0], attribute_bytes[1]]) as usize;
        let mut cursor = 2;
        let mut target_bsm_ref = 0;
        let mut target_args = Vec::new();
        let mut found = false;

        for i in 0..num_bsms {
            if cursor + 4 > attribute_bytes.len() {
                return Err(Error::other("BootstrapMethods entry truncated"));
            }
            let bsm_ref =
                u16::from_be_bytes([attribute_bytes[cursor], attribute_bytes[cursor + 1]]);
            let num_args =
                u16::from_be_bytes([attribute_bytes[cursor + 2], attribute_bytes[cursor + 3]])
                    as usize;
            cursor += 4;

            if cursor + num_args * 2 > attribute_bytes.len() {
                return Err(Error::other("BootstrapMethods arguments truncated"));
            }

            if i == bsm_attr_index as usize {
                target_bsm_ref = bsm_ref;
                for a in 0..num_args {
                    let arg_idx = u16::from_be_bytes([
                        attribute_bytes[cursor + a * 2],
                        attribute_bytes[cursor + a * 2 + 1],
                    ]);
                    // JVMS §4.7.23: bootstrap argument indices are
                    // CONSTANT_Integer, CONSTANT_Long, CONSTANT_Float,
                    // CONSTANT_Double, CONSTANT_String, CONSTANT_Class,
                    // CONSTANT_MethodType, or CONSTANT_MethodHandle entries.
                    // Anything else — including index 0, out-of-range
                    // indices, and indirect constants pointing at non-UTF8
                    // entries — is a class-file format error.
                    let c = constants.get(arg_idx as usize).ok_or_else(|| {
                        Error::other(format!(
                            "BootstrapMethods argument index {} out of bounds",
                            arg_idx
                        ))
                    })?;
                    let utf8_arg = |idx: u16| -> io::Result<String> {
                        match constants.get(idx as usize) {
                            Some(Constant::Utf8(bytes)) => {
                                Ok(String::from_utf8_lossy(bytes).to_string())
                            }
                            _ => Err(Error::other(format!(
                                "BootstrapMethods argument {} does not reference a CONSTANT_Utf8",
                                arg_idx
                            ))),
                        }
                    };
                    let parsed_arg = match c {
                        Constant::Integer(v) => BootstrapArg::Int(*v),
                        Constant::Long(v) => BootstrapArg::Long(*v),
                        Constant::Float(v) => BootstrapArg::Float(v.to_bits()),
                        Constant::Double(v) => BootstrapArg::Double(v.to_bits()),
                        Constant::String(s_idx) => BootstrapArg::String(utf8_arg(*s_idx)?),
                        Constant::Class(c_idx) => BootstrapArg::Klass(utf8_arg(*c_idx)?),
                        Constant::MethodType(m_idx) => {
                            BootstrapArg::MethodType(utf8_arg(*m_idx)?)
                        }
                        Constant::MethodHandle(kind, idx) => BootstrapArg::MethodHandle {
                            kind: *kind,
                            index: *idx,
                        },
                        other => {
                            return Err(Error::other(format!(
                                "BootstrapMethods argument {} has invalid constant tag",
                                match other {
                                    Constant::Unusable => "Unusable",
                                    Constant::FieldRef(..) => "FieldRef",
                                    Constant::MethodRef(..) => "MethodRef",
                                    Constant::InterfaceMethodRef(..) => "InterfaceMethodRef",
                                    Constant::Class(_) => "Class",
                                    Constant::NameAndType(..) => "NameAndType",
                                    Constant::Utf8(_) => "Utf8",
                                    Constant::Integer(_) => "Integer",
                                    Constant::Float(_) => "Float",
                                    Constant::Long(_) => "Long",
                                    Constant::Double(_) => "Double",
                                    Constant::String(_) => "String",
                                    Constant::MethodHandle(..) => "MethodHandle",
                                    Constant::MethodType(_) => "MethodType",
                                    Constant::Dynamic(..) => "Dynamic",
                                    Constant::InvokeDynamic(..) => "InvokeDynamic",
                                    Constant::Module(_) => "Module",
                                    Constant::Package(_) => "Package",
                                }
                            )))
                        }
                    };
                    target_args.push(parsed_arg);
                }
                found = true;
                break;
            }
            cursor += num_args * 2;
        }

        if !found {
            return Err(Error::other(format!(
                "Bootstrap method index {} out of bounds",
                bsm_attr_index
            )));
        }

        Ok(Self::new(
            bsm_attr_index as usize,
            0,
            name,
            signature,
            target_bsm_ref,
            target_args,
        ))
    }
}
