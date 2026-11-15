/*
 * JIT metadata collection for the compile broker.
 *
 * Scans a method's bytecode once and resolves every constant-pool
 * reference the JIT frontends need, producing plain data that maps
 * 1:1 onto `c1_GraphBuilder::MethodMetadata`:
 *
 * - `fields`: cp index -> field kind/byte offset/name for
 *   getfield/putfield/getstatic/putstatic lowering.
 * - `call_sites`: cp index -> resolved invoke target description
 *   (opcode, name, argument/return kinds, static/interface flags).
 * - `sites`: cp index -> owner class/name/descriptor triple for every
 *   heap-touching instruction; becomes the leaked `StubSite` pointer
 *   consumed by JIT runtime stubs.
 * - `exception_handlers`: the Code attribute exception table.
 *
 * All offsets come from `InstanceKlass`/`FieldLayout`, i.e. the same
 * HotSpot-compatible layout the interpreter uses.
 */

use super::classFileParser::Member;
use super::classLoader::ClassLoader;
use crate::share::interpreter::bytecodes;
use crate::share::oops::constMethod::split_method_descriptor;
use crate::share::oops::fieldInfo::{FieldKind, descriptor_kind};
use crate::share::oops::instanceKlass::InstanceKlass;
use crate::share::oops::method::Code;
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

/// Resolved field access site (getfield/putfield/getstatic/putstatic).
#[derive(Debug, Clone, PartialEq)]
pub struct JitFieldInfo {
    /// Machine kind of the field value.
    pub kind: FieldKind,
    /// Byte offset inside the object (instance fields) or inside the
    /// static field area (static fields); `None` when the declaring
    /// class could not be resolved.
    pub offset: Option<u32>,
    /// Field name (for diagnostics and stub fallback).
    pub name: String,
    pub is_static: bool,
    /// JVM access flags of the resolved field.
    pub access_flags: u16,
}

/// Resolved invoke site (invokevirtual/invokespecial/invokestatic/
/// invokeinterface).
#[derive(Debug, Clone, PartialEq)]
pub struct JitCallSite {
    pub opcode: u8,
    pub cp_index: u32,
    /// `owner.name+descriptor` in the `method_key` format.
    pub name: String,
    /// One entry per declared parameter (wide types count once).
    pub arg_kinds: Vec<FieldKind>,
    /// `None` for `void` returns.
    pub return_kind: Option<FieldKind>,
    pub is_static: bool,
    pub is_interface: bool,
}

/// Constant-pool site description handed to JIT runtime stubs.
/// `name`/`desc` are empty for pure class references (new/checkcast/
/// instanceof/anewarray/multianewarray).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JitSite {
    pub class: String,
    pub name: String,
    pub desc: String,
}

/// Exception-table entry of the compiled method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitExceptionHandler {
    pub start_pc: usize,
    pub end_pc: usize,
    pub handler_pc: usize,
    pub catch_type: u16,
}

/// Everything the JIT graph builder needs that is not plain bytecode.
#[derive(Debug, Clone, Default)]
pub struct JitMetadata {
    pub fields: BTreeMap<u32, JitFieldInfo>,
    pub call_sites: BTreeMap<u32, JitCallSite>,
    pub sites: BTreeMap<u32, JitSite>,
    pub exception_handlers: Vec<JitExceptionHandler>,
}

/// Padding bytes after `tableswitch`/`lookupswitch` opcodes so the
/// operand area starts on a 4-byte boundary relative to the method.
fn switch_padding(bci: usize) -> usize {
    (4 - ((bci + 1) % 4)) % 4
}

/// Byte length of the instruction at `bci`, including the variable
/// length `tableswitch`/`lookupswitch` forms and `wide` prefixes.
fn instruction_len(code: &[u8], bci: usize) -> usize {
    let Some(&op) = code.get(bci) else { return 1 };
    match op {
        0xaa => {
            let pad = switch_padding(bci);
            let base = bci + 1 + pad;
            if base + 12 > code.len() {
                return code.len() - bci;
            }
            let low = i32::from_be_bytes(code[base + 4..base + 8].try_into().unwrap());
            let high = i32::from_be_bytes(code[base + 8..base + 12].try_into().unwrap());
            let count = high.saturating_sub(low).saturating_add(1).max(0) as usize;
            (1 + pad + 12 + 4 * count).min(code.len() - bci)
        }
        0xab => {
            let pad = switch_padding(bci);
            let base = bci + 1 + pad;
            if base + 8 > code.len() {
                return code.len() - bci;
            }
            let npairs = i32::from_be_bytes(code[base + 4..base + 8].try_into().unwrap());
            let count = if (0..=1_000_000).contains(&npairs) {
                npairs as usize
            } else {
                0
            };
            (1 + pad + 8 + 8 * count).min(code.len() - bci)
        }
        0xc4 => match code.get(bci + 1).copied() {
            Some(0x15..=0x19 | 0x36..=0x3a | 0xa9) => 4,
            Some(0x84) => 6,
            _ => 2,
        },
        _ => {
            let len = bytecodes::length_for(op);
            if len > 0 {
                len as usize
            } else {
                1
            }
        }
    }
}

/// Two-byte constant-pool index operand at `bci + 1`.
fn cp_index_at(code: &[u8], bci: usize) -> Option<u32> {
    let hi = *code.get(bci + 1)?;
    let lo = *code.get(bci + 2)?;
    Some(u16::from_be_bytes([hi, lo]) as u32)
}

fn utf8_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn descriptor_kinds(descriptor: &[u8]) -> (Vec<FieldKind>, Option<FieldKind>) {
    let Ok((params, ret)) = split_method_descriptor(descriptor) else {
        return (Vec::new(), None);
    };
    let args = params
        .iter()
        .map(|p| descriptor_kind(p).unwrap_or(FieldKind::Int32))
        .collect();
    let ret_kind = ret.and_then(descriptor_kind);
    (args, ret_kind)
}

impl<'a> ClassLoader<'a> {
    /// Collects JIT metadata for `member` of `klass`: resolved field
    /// offsets, call-site descriptions, stub-site triples and the
    /// exception table.  Unresolvable constant-pool entries are skipped
    /// (JIT stubs treat missing sites as deoptimization points).
    pub fn jit_metadata(
        &self,
        klass: &Arc<InstanceKlass<'a>>,
        member: &Member<'a>,
    ) -> io::Result<JitMetadata> {
        let code = Code::parse(member)?;
        let mut metadata = JitMetadata {
            exception_handlers: code
                .exception_table
                .iter()
                .map(|e| JitExceptionHandler {
                    start_pc: e.start_pc,
                    end_pc: e.end_pc,
                    handler_pc: e.handler_pc,
                    catch_type: e.catch_type,
                })
                .collect(),
            ..JitMetadata::default()
        };

        let mut bci = 0usize;
        while bci < code.bytes.len() {
            let op = code.bytes[bci];
            let next = (bci + instruction_len(code.bytes, bci)).min(code.bytes.len());
            match op {
                // getstatic / putstatic / getfield / putfield
                0xb2..=0xb5 => {
                    let Some(idx) = cp_index_at(code.bytes, bci) else { break };
                    let cp_idx = idx as u16;
                    let is_static = matches!(op, 0xb2 | 0xb3);
                    if let Ok(reference) = klass.resolve_member_ref(cp_idx) {
                        let site = JitSite {
                            class: utf8_string(reference.owner),
                            name: utf8_string(reference.name),
                            desc: utf8_string(reference.descriptor),
                        };
                        metadata.sites.insert(idx, site.clone());
                        if let Ok(owner_klass) = self.load(reference.owner) {
                            if let Some((_, info)) =
                                owner_klass.lookup_field(reference.name, reference.descriptor)
                            {
                                metadata.fields.insert(
                                    idx,
                                    JitFieldInfo {
                                        kind: info
                                            .kind()
                                            .unwrap_or_else(|| {
                                                descriptor_kind(reference.descriptor)
                                                    .unwrap_or(FieldKind::Int32)
                                            }),
                                        offset: Some(info.offset),
                                        name: utf8_string(reference.name),
                                        is_static: info.is_static() || is_static,
                                        access_flags: info.access_flags,
                                    },
                                );
                            } else {
                                metadata.fields.insert(
                                    idx,
                                    JitFieldInfo {
                                        kind: descriptor_kind(reference.descriptor)
                                            .unwrap_or(FieldKind::Int32),
                                        offset: None,
                                        name: utf8_string(reference.name),
                                        is_static,
                                        access_flags: 0,
                                    },
                                );
                            }
                        } else {
                            metadata.fields.insert(
                                idx,
                                JitFieldInfo {
                                    kind: descriptor_kind(reference.descriptor)
                                        .unwrap_or(FieldKind::Int32),
                                    offset: None,
                                    name: utf8_string(reference.name),
                                    is_static,
                                    access_flags: 0,
                                },
                            );
                        }
                    }
                }
                // invokevirtual / invokespecial / invokestatic /
                // invokeinterface / invokedynamic
                0xb6..=0xba => {
                    let Some(idx) = cp_index_at(code.bytes, bci) else { break };
                    let cp_idx = idx as u16;
                    if let Ok(reference) = klass.resolve_member_ref(cp_idx) {
                        let (arg_kinds, return_kind) =
                            descriptor_kinds(reference.descriptor);
                        metadata.call_sites.insert(
                            idx,
                            JitCallSite {
                                opcode: op,
                                cp_index: idx,
                                name: format!(
                                    "{}.{}{}",
                                    utf8_string(reference.owner),
                                    utf8_string(reference.name),
                                    utf8_string(reference.descriptor)
                                ),
                                arg_kinds,
                                return_kind,
                                is_static: op == 0xb8,
                                is_interface: op == 0xb9
                                    || reference.kind
                                        == crate::share::oops::constantPool::MemberRefKind::InterfaceMethod,
                            },
                        );
                    }
                }
                // new / anewarray / checkcast / instanceof / multianewarray
                0xbb | 0xbd | 0xc0 | 0xc1 | 0xc5 => {
                    let Some(idx) = cp_index_at(code.bytes, bci) else { break };
                    let cp_idx = idx as u16;
                    if let Ok(class_name) = klass.className(cp_idx) {
                        metadata.sites.insert(
                            idx,
                            JitSite {
                                class: utf8_string(class_name),
                                name: String::new(),
                                desc: String::new(),
                            },
                        );
                    }
                }
                _ => {}
            }
            bci = next;
        }

        Ok(metadata)
    }
}
