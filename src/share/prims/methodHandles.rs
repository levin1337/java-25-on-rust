/*
 * Port of OpenJDK 25 HotSpot MethodHandles
 * (share/prims/methodHandles.hpp / methodHandles.cpp)
 *
 * Implements JVM runtime support for JEP 292 java.lang.invoke:
 * ReferenceKind, MemberName, MethodType, MethodHandle, and dynamic CallSite dispatch.
 */

use crate::share::oops::symbol::Symbol;
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ReferenceKind {
    GetField = 1,
    GetStatic = 2,
    PutField = 3,
    PutStatic = 4,
    InvokeVirtual = 5,
    InvokeStatic = 6,
    InvokeSpecial = 7,
    NewInvokeSpecial = 8,
    InvokeInterface = 9,
}

impl ReferenceKind {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            1 => Some(ReferenceKind::GetField),
            2 => Some(ReferenceKind::GetStatic),
            3 => Some(ReferenceKind::PutField),
            4 => Some(ReferenceKind::PutStatic),
            5 => Some(ReferenceKind::InvokeVirtual),
            6 => Some(ReferenceKind::InvokeStatic),
            7 => Some(ReferenceKind::InvokeSpecial),
            8 => Some(ReferenceKind::NewInvokeSpecial),
            9 => Some(ReferenceKind::InvokeInterface),
            _ => None,
        }
    }

    #[inline]
    pub fn is_method(self) -> bool {
        matches!(
            self,
            ReferenceKind::InvokeVirtual
                | ReferenceKind::InvokeStatic
                | ReferenceKind::InvokeSpecial
                | ReferenceKind::NewInvokeSpecial
                | ReferenceKind::InvokeInterface
        )
    }

    #[inline]
    pub fn is_field(self) -> bool {
        matches!(
            self,
            ReferenceKind::GetField
                | ReferenceKind::GetStatic
                | ReferenceKind::PutField
                | ReferenceKind::PutStatic
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberName {
    pub clazz: Symbol,
    pub name: Symbol,
    pub type_sig: Symbol,
    pub ref_kind: ReferenceKind,
    pub flags: u32,
    pub vtable_index: Option<i32>,
}

impl MemberName {
    pub fn new(
        clazz: Symbol,
        name: Symbol,
        type_sig: Symbol,
        ref_kind: ReferenceKind,
        flags: u32,
    ) -> Self {
        Self {
            clazz,
            name,
            type_sig,
            ref_kind,
            flags,
            vtable_index: None,
        }
    }

    #[inline]
    pub fn is_method(&self) -> bool {
        self.ref_kind.is_method()
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        matches!(
            self.ref_kind,
            ReferenceKind::GetStatic | ReferenceKind::PutStatic | ReferenceKind::InvokeStatic
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodType {
    pub rtype: Symbol,
    pub ptypes: Vec<Symbol>,
    pub parameter_slot_count: usize,
}

impl MethodType {
    pub fn from_descriptor(desc: &str) -> Option<Self> {
        if !desc.starts_with('(') {
            return None;
        }
        let close = desc.find(')')?;
        let params_part = &desc[1..close];
        let return_part = &desc[close + 1..];

        let mut ptypes = Vec::new();
        let mut chars = params_part.chars().peekable();
        let mut slots = 0;

        while let Some(ch) = chars.next() {
            match ch {
                'B' | 'Z' | 'C' | 'S' | 'I' | 'F' => {
                    ptypes.push(Symbol::from(ch.to_string().as_str()));
                    slots += 1;
                }
                'J' | 'D' => {
                    ptypes.push(Symbol::from(ch.to_string().as_str()));
                    slots += 2;
                }
                'L' => {
                    let mut obj_name = String::new();
                    while let Some(c) = chars.next() {
                        if c == ';' {
                            break;
                        }
                        obj_name.push(c);
                    }
                    ptypes.push(Symbol::from(format!("L{obj_name};").as_str()));
                    slots += 1;
                }
                '[' => {
                    let mut arr_name = String::from('[');
                    while let Some(&c) = chars.peek() {
                        chars.next();
                        arr_name.push(c);
                        if c != '[' {
                            if c == 'L' {
                                while let Some(oc) = chars.next() {
                                    arr_name.push(oc);
                                    if oc == ';' {
                                        break;
                                    }
                                }
                            }
                            break;
                        }
                    }
                    ptypes.push(Symbol::from(arr_name.as_str()));
                    slots += 1;
                }
                _ => return None,
            }
        }

        Some(Self {
            rtype: Symbol::from(return_part),
            ptypes,
            parameter_slot_count: slots,
        })
    }

    #[inline]
    pub fn parameter_count(&self) -> usize {
        self.ptypes.len()
    }
}

#[derive(Clone, Debug)]
pub struct MethodHandle {
    pub member: MemberName,
    pub mtype: MethodType,
}

impl MethodHandle {
    pub fn new(member: MemberName, mtype: MethodType) -> Self {
        Self { member, mtype }
    }
}

#[derive(Debug)]
pub struct CallSite {
    target: RwLock<MethodHandle>,
}

impl CallSite {
    pub fn new(target: MethodHandle) -> Self {
        Self {
            target: RwLock::new(target),
        }
    }

    pub fn get_target(&self) -> MethodHandle {
        self.target.read().unwrap().clone()
    }

    pub fn set_target(&self, target: MethodHandle) {
        *self.target.write().unwrap() = target;
    }
}

pub struct MethodHandles;

impl MethodHandles {
    pub fn resolve_member_name(
        clazz: &str,
        name: &str,
        descriptor: &str,
        ref_kind: ReferenceKind,
    ) -> Option<MemberName> {
        Some(MemberName::new(
            Symbol::from(clazz),
            Symbol::from(name),
            Symbol::from(descriptor),
            ref_kind,
            0,
        ))
    }
}
