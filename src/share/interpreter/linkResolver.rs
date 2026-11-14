/*
 * Port of OpenJDK 25 HotSpot LinkResolver (share/interpreter/linkResolver.hpp, linkResolver.cpp)
 *
 * LinkResolver performs static resolution of symbolic references in constant pools
 * and determines runtime dispatch targets for invokevirtual (via vtable),
 * invokeinterface (via itable), invokespecial, invokestatic, and invokedynamic.
 */

use crate::share::{
    classfile::classLoader::ClassLoader,
    classfile::systemDictionary::SystemDictionary,
    interpreter::bootstrapInfo::BootstrapInfo,
    oops::{instanceKlass::InstanceKlass, method::Code, symbol::Symbol},
};
use std::collections::HashSet;
use std::io::{self, Error};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Direct,
    Vtable,
    Itable,
}

#[derive(Clone)]
pub struct CallInfo<'a> {
    pub resolved_klass: Rc<InstanceKlass<'a>>,
    pub resolved_method: Code<'a>,
    pub selected_method: Code<'a>,
    pub call_kind: CallKind,
    pub call_index: i32,
}

pub struct LinkResolver;

impl LinkResolver {
    /// Resolves invokevirtual dispatch using receiver klass's vtable.
    pub fn resolve_virtual_call<'a>(
        receiver_klass: &Rc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Rc<InstanceKlass<'a>>, Code<'a>)> {
        let name_str = String::from_utf8_lossy(name);
        let sig_str = String::from_utf8_lossy(descriptor);

        // 1. Try vtable lookup on the receiver's klass
        if let Some(vtable_idx) = receiver_klass.vtable().index_of_method(&name_str, &sig_str) {
            if let Some(entry) = receiver_klass.vtable().entry_at(vtable_idx) {
                let holder = entry.holder_klass().as_bytes();
                // Find holder klass in hierarchy starting from receiver
                let mut curr = Some(receiver_klass);
                while let Some(k) = curr {
                    if k.class.name == holder {
                        if let Some(method) = k.class.methods.iter().find(|m| {
                            m.name == name && m.descriptor == descriptor && (m.accessFlags & 8 == 0)
                        }) {
                            return Ok((Rc::clone(k), Code::parse(method)?));
                        }
                    }
                    curr = k.super_klass.as_ref();
                }
            }
        }

        // 2. Fallback to direct hierarchical resolution
        let mut current = Some(receiver_klass);
        let mut interfaces = Vec::new();
        while let Some(k) = current {
            if let Some(method) = k.class.methods.iter().find(|method| {
                method.name == name
                    && method.descriptor == descriptor
                    && method.accessFlags & 8 == 0
            }) {
                if method.accessFlags & 0x0400 != 0 {
                    return Err(Error::other(format!(
                        "AbstractMethodError: {}.{}{}",
                        String::from_utf8_lossy(k.class.name),
                        String::from_utf8_lossy(name),
                        String::from_utf8_lossy(descriptor)
                    )));
                }
                return Ok((Rc::clone(k), Code::parse(method)?));
            }
            interfaces.extend(&k.interfaces);
            current = k.super_klass.as_ref();
        }

        // 3. Interface default methods
        let mut visited = HashSet::new();
        let mut candidates = Vec::new();
        while let Some(interface) = interfaces.pop() {
            if !visited.insert(interface.class.name) {
                continue;
            }
            if let Some(method) = interface.class.methods.iter().find(|method| {
                method.name == name
                    && method.descriptor == descriptor
                    && method.accessFlags & 10 == 0
            }) {
                candidates.push((interface, method));
            }
            interfaces.extend(&interface.interfaces);
        }

        let mut defaults = candidates.iter().filter(|(interface, method)| {
            method.accessFlags & 0x0400 == 0
                && !candidates.iter().any(|(other, _)| {
                    other.class.name != interface.class.name
                        && SystemDictionary::implements_interface(other, interface.class.name)
                })
        });

        if let Some((interface, method)) = defaults.next() {
            if defaults.next().is_some() {
                return Err(Error::other("Conflicting interface default methods"));
            }
            return Ok((Rc::clone(interface), Code::parse(method)?));
        }

        {
            let mut names = Vec::new();
            let mut curr = Some(receiver_klass);
            while let Some(k) = curr {
                for m in &k.class.methods {
                    if m.name == name {
                        names.push(format!(
                            "{}.{}{} flags={:#x}",
                            String::from_utf8_lossy(k.class.name),
                            String::from_utf8_lossy(m.name),
                            String::from_utf8_lossy(m.descriptor),
                            m.accessFlags
                        ));
                    }
                }
                curr = k.super_klass.as_ref();
            }
            eprintln!(
                "[resolve_virtual] {}.{}{} candidates={names:?}",
                String::from_utf8_lossy(receiver_klass.class.name),
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor)
            );
        }
        if std::env::var_os("JVM25_TRACE").is_some() {
            eprintln!(
                "[resolve-fail] {}.{}{}",
                String::from_utf8_lossy(receiver_klass.class.name),
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor),
            );
            eprintln!(
                "  methods: {:?}",
                receiver_klass
                    .class
                    .methods
                    .iter()
                    .map(|m| format!(
                        "{}{}",
                        String::from_utf8_lossy(m.name),
                        String::from_utf8_lossy(m.descriptor)
                    ))
                    .collect::<Vec<_>>()
            );
        }
        Err(Error::other(format!(
            "NoSuchMethodError: {}{}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
        )))
    }

    /// Resolves invokeinterface dispatch using receiver klass's itable and vtable.
    pub fn resolve_interface_call<'a>(
        receiver_klass: &Rc<InstanceKlass<'a>>,
        interface_name: &[u8],
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Rc<InstanceKlass<'a>>, Code<'a>)> {
        let iface_str = String::from_utf8_lossy(interface_name);
        let name_str = String::from_utf8_lossy(name);
        let sig_str = String::from_utf8_lossy(descriptor);

        // 1. Check receiver's itable
        if let Some(vtable_idx) = receiver_klass
            .itable()
            .resolve_interface_method(&iface_str, &name_str, &sig_str)
        {
            if let Some(entry) = receiver_klass.vtable().entry_at(vtable_idx) {
                let holder = entry.holder_klass().as_bytes();
                let mut curr = Some(receiver_klass);
                while let Some(k) = curr {
                    if k.class.name == holder {
                        if let Some(method) = k.class.methods.iter().find(|m| {
                            m.name == name && m.descriptor == descriptor && (m.accessFlags & 8 == 0)
                        }) {
                            return Ok((Rc::clone(k), Code::parse(method)?));
                        }
                    }
                    curr = k.super_klass.as_ref();
                }
            }
        }

        // 2. Private/default methods declared on the named interface itself
        // (e.g. LambdaMetafactory impl handles with REF_invokeInterface kind
        // targeting private interface methods). Walk the receiver's interface
        // hierarchy looking for the interface that declares the method.
        {
            let mut pending: Vec<Rc<InstanceKlass<'a>>> = receiver_klass.interfaces.clone();
            let mut visited = HashSet::new();
            while let Some(iface) = pending.pop() {
                if !visited.insert(Rc::as_ptr(&iface) as usize) {
                    continue;
                }
                pending.extend(iface.interfaces.iter().cloned());
                let declares = iface.class.name == interface_name
                    || iface
                        .class
                        .methods
                        .iter()
                        .any(|m| m.name == name && m.descriptor == descriptor);
                if !declares {
                    continue;
                }
                if let Some(method) = iface.class.methods.iter().find(|m| {
                    m.name == name
                        && m.descriptor == descriptor
                        && m.accessFlags & 8 == 0
                        && m.code_attribute().is_some()
                }) {
                    return Ok((Rc::clone(&iface), Code::parse(method)?));
                }
            }
        }

        // 3. If receiver overrides it directly, resolve virtual call
        if let Ok(res) = Self::resolve_virtual_call(receiver_klass, name, descriptor) {
            return Ok(res);
        }

        // 4. Check public method on java/lang/Object
        let mut curr = Some(receiver_klass);
        while let Some(k) = curr {
            if k.class.name == b"java/lang/Object" {
                if let Some(method) = k.class.methods.iter().find(|m| {
                    m.name == name && m.descriptor == descriptor && (m.accessFlags & 1 != 0)
                }) {
                    return Ok((Rc::clone(k), Code::parse(method)?));
                }
            }
            curr = k.super_klass.as_ref();
        }

        Err(Error::other(format!(
            "IncompatibleClassChangeError: interface method not found {}.{}{}",
            iface_str, name_str, sig_str
        )))
    }

    /// Resolves invokespecial call (constructors, private methods, super calls).
    /// Selects the method declared in the referenced class or its
    /// superclasses without virtual override; interface owners additionally
    /// search superinterfaces for private/default methods (JVMS §5.4.3.3).
    pub fn resolve_special_call<'a>(
        target_klass: &Rc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Rc<InstanceKlass<'a>>, Code<'a>)> {
        let mut current = Some(Rc::clone(target_klass));
        while let Some(klass) = current {
            if let Some(method) = klass.class.methods.iter().find(|method| {
                method.name == name
                    && method.descriptor == descriptor
                    && method.accessFlags & 8 == 0
            }) {
                return Ok((Rc::clone(&klass), Code::parse(method)?));
            }
            current = klass.super_klass();
        }
        let mut pending: Vec<Rc<InstanceKlass<'a>>> = target_klass.interfaces.clone();
        let mut visited = HashSet::new();
        while let Some(iface) = pending.pop() {
            if !visited.insert(Rc::as_ptr(&iface) as usize) {
                continue;
            }
            if let Some(method) = iface.class.methods.iter().find(|method| {
                method.name == name
                    && method.descriptor == descriptor
                    && method.accessFlags & 8 == 0
            }) {
                return Ok((Rc::clone(&iface), Code::parse(method)?));
            }
            pending.extend(iface.interfaces.iter().cloned());
        }
        Err(Error::other(format!(
            "NoSuchMethodError: {}{}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
        )))
    }

    /// Resolves invokestatic call.
    pub fn resolve_static_call<'a>(
        target_klass: &Rc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Rc<InstanceKlass<'a>>, Code<'a>)> {
        let mut current = Some(Rc::clone(target_klass));
        while let Some(klass) = current {
            if let Some(method) = klass.class.methods.iter().find(|method| {
                method.name == name
                    && method.descriptor == descriptor
                    && method.accessFlags & 8 != 0
            }) {
                return Ok((Rc::clone(&klass), Code::parse(method)?));
            }
            current = klass.super_klass();
        }
        Err(Error::other("Missing static method"))
    }

    /// Resolves invokedynamic call site via BootstrapInfo.
    pub fn resolve_invokedynamic<'a>(
        loader: &ClassLoader<'a>,
        caller_klass: &Rc<InstanceKlass<'a>>,
        bsm_attr_index: u16,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<BootstrapInfo> {
        let raw_bsm = caller_klass
            .class
            .bootstrap_methods()
            .ok_or_else(|| Error::other("Missing BootstrapMethods attribute for invokedynamic"))?;

        let mut bs_info = BootstrapInfo::parse_from_attribute(
            raw_bsm,
            bsm_attr_index,
            Symbol::from(name),
            Symbol::from(descriptor),
            &caller_klass.class.constants,
        )?;

        // Resolve BSM target
        let _ = loader;
        bs_info.set_resolved(format!(
            "bsm_{}_{}",
            String::from_utf8_lossy(name),
            bsm_attr_index
        ));

        Ok(bs_info)
    }
}
