/*
 * Port of OpenJDK 25 HotSpot LinkResolver (share/interpreter/linkResolver.hpp, linkResolver.cpp)
 *
 * LinkResolver performs static resolution of symbolic references in constant pools
 * and determines runtime dispatch targets for invokevirtual (via vtable),
 * invokeinterface (via itable), invokespecial, invokestatic, and invokedynamic.
 *
 * Method resolution follows JVMS §5.4.3.3:
 *   - class C: search C then its superclasses; if nothing is found, search the
 *     superinterfaces of C and select a maximally-specific method;
 *   - interface C: search C and its superinterfaces, again maximally-specific;
 *   - access checks follow JVMS §5.4.4 (private/protected/package/public plus
 *     JPMS readability and package-export rules) where the caller class is known.
 */

use crate::share::{
    classfile::{classLoader::ClassLoader, systemDictionary::SystemDictionary, Constant},
    interpreter::bootstrapInfo::BootstrapInfo,
    oops::{instanceKlass::InstanceKlass, method::Code, symbol::Symbol},
};
use std::collections::HashSet;
use std::io::{self, Error};
use std::sync::Arc;

const ACC_PUBLIC: u16 = 0x0001;
const ACC_PRIVATE: u16 = 0x0002;
const ACC_PROTECTED: u16 = 0x0004;
const ACC_STATIC: u16 = 0x0008;
const ACC_INTERFACE: u16 = 0x0200;
const ACC_ABSTRACT: u16 = 0x0400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Direct,
    Vtable,
    Itable,
}

#[derive(Clone)]
pub struct CallInfo<'a> {
    pub resolved_klass: Arc<InstanceKlass<'a>>,
    pub resolved_method: Code<'a>,
    pub selected_method: Code<'a>,
    pub call_kind: CallKind,
    pub call_index: i32,
}

/// A resolved method: the class that declares it plus its index in
/// `InstanceKlass::class.methods`. Returning an index keeps the borrow tied to
/// the returned `Arc`, which callers hold for the duration of the call.
#[derive(Clone)]
struct Resolved<'a> {
    holder: Arc<InstanceKlass<'a>>,
    index: usize,
}

impl<'a> Resolved<'a> {
    fn member(&self) -> &crate::classfile::Member<'a> {
        &self.holder.class.methods[self.index]
    }

    fn flags(&self) -> u16 {
        self.member().accessFlags
    }

    fn is_abstract(&self) -> bool {
        self.flags() & ACC_ABSTRACT != 0
    }

    fn is_static(&self) -> bool {
        self.flags() & ACC_STATIC != 0
    }

    fn code(&self) -> io::Result<Code<'a>> {
        Code::parse(self.member())
    }
}

pub struct LinkResolver;

// ---------------------------------------------------------------------------
// Low-level helpers
// ---------------------------------------------------------------------------

fn is_interface<'a>(klass: &InstanceKlass<'a>) -> bool {
    klass.class.accessFlags & ACC_INTERFACE != 0
}

fn find_declared<'a>(
    klass: &InstanceKlass<'a>,
    name: &[u8],
    descriptor: &[u8],
) -> Option<usize> {
    klass
        .class
        .methods
        .iter()
        .position(|m| m.name == name && m.descriptor == descriptor)
}

/// `true` for the signature-polymorphic methods of
/// `java.lang.invoke.MethodHandle` (JVMS §2.9.3): they may be invoked with any
/// descriptor, so descriptor equality does not apply during resolution.
fn is_signature_polymorphic(name: &[u8]) -> bool {
    matches!(name, b"invoke" | b"invokeExact" | b"invokeBasic")
}

/// Collects `root` plus every transitively implemented/extended interface.
/// `root` itself is included only when it is an interface. Dedup is by class
/// name; interfaces form a DAG so a name identifies the interface uniquely.
fn interface_closure<'a>(root: &Arc<InstanceKlass<'a>>) -> Vec<Arc<InstanceKlass<'a>>> {
    let mut out: Vec<Arc<InstanceKlass<'a>>> = Vec::new();
    let mut seen: HashSet<&'a [u8]> = HashSet::new();
    let mut pending: Vec<Arc<InstanceKlass<'a>>> = Vec::new();
    if is_interface(root) {
        pending.push(Arc::clone(root));
    }
    pending.extend(root.interfaces.iter().cloned());
    while let Some(iface) = pending.pop() {
        if !seen.insert(iface.class.name) {
            continue;
        }
        pending.extend(iface.interfaces.iter().cloned());
        out.push(iface);
    }
    out
}

/// `true` when `sub` is `sup` or a (transitive) subinterface of `sup`.
fn is_subinterface<'a>(sub: &InstanceKlass<'a>, sup_name: &[u8]) -> bool {
    sub.class.name == sup_name
        || SystemDictionary::implements_interface(sub, sup_name)
}

/// Selects the maximally-specific method among interface candidates
/// (JVMS §5.4.3.3). A candidate is maximally-specific when no other candidate
/// is declared in a subinterface of its declaring interface. Ties between
/// unrelated interfaces resolve to a single non-abstract method when possible;
/// genuinely ambiguous non-abstract defaults raise
/// `IncompatibleClassChangeError`.
fn select_maximally_specific<'a>(
    candidates: Vec<Resolved<'a>>,
) -> io::Result<Option<Resolved<'a>>> {
    if candidates.is_empty() {
        return Ok(None);
    }
    // Deduplicate by (holder name, member identity is implied by resolution).
    let mut unique: Vec<Resolved<'a>> = Vec::new();
    for cand in candidates {
        if !unique.iter().any(|u| {
            Arc::ptr_eq(&u.holder, &cand.holder) || u.holder.class.name == cand.holder.class.name
        }) {
            unique.push(cand);
        }
    }
    // Drop candidates shadowed by a candidate in a subinterface.
    let mut maximal: Vec<Resolved<'a>> = Vec::new();
    'outer: for (i, cand) in unique.iter().enumerate() {
        for (j, other) in unique.iter().enumerate() {
            if i == j {
                continue;
            }
            // `other` is more specific when its interface extends cand's.
            if other.holder.class.name != cand.holder.class.name
                && is_subinterface(&other.holder, cand.holder.class.name)
            {
                continue 'outer;
            }
        }
        maximal.push(cand.clone());
    }
    maximal.sort_by(|a, b| a.holder.class.name.cmp(b.holder.class.name));
    match maximal.len() {
        0 => Ok(None),
        1 => Ok(maximal.pop()),
        _ => {
            let non_abstract: Vec<Resolved<'a>> =
                maximal.iter().filter(|m| !m.is_abstract()).cloned().collect();
            match non_abstract.len() {
                // Exactly one non-abstract maximally-specific method wins.
                1 => Ok(non_abstract.into_iter().next()),
                // Multiple unrelated concrete defaults: JVMS ICCE.
                n if n > 1 => Err(Error::other(
                    "IncompatibleClassChangeError: ambiguous interface defaults",
                )),
                // All maximally-specific methods are abstract: pick one
                // deterministically; selection reports AbstractMethodError.
                _ => Ok(maximal.into_iter().next()),
            }
        }
    }
}

/// JVMS §5.4.3.3 method resolution in class (or interface) `c`.
/// Returns the resolved method without call-kind-specific checks; callers
/// apply the ACC_STATIC / abstract rules for their opcode.
fn resolve_method_in<'a>(
    c: &Arc<InstanceKlass<'a>>,
    name: &[u8],
    descriptor: &[u8],
) -> io::Result<Option<Resolved<'a>>> {
    // Signature-polymorphic MethodHandle methods ignore the descriptor.
    if c.class.name == b"java/lang/invoke/MethodHandle" && is_signature_polymorphic(name) {
        if let Some(index) = c
            .class
            .methods
            .iter()
            .position(|m| m.name == name)
        {
            return Ok(Some(Resolved {
                holder: Arc::clone(c),
                index,
            }));
        }
    }

    if is_interface(c) {
        // Interface C: search C and its superinterfaces only. Private
        // methods of *super*interfaces are not inherited; C's own private
        // methods remain visible (invokespecial targets them directly).
        let mut candidates = Vec::new();
        for iface in interface_closure(c) {
            if let Some(index) = find_declared(&iface, name, descriptor) {
                let member = &iface.class.methods[index];
                if member.accessFlags & ACC_PRIVATE != 0
                    && iface.class.name != c.class.name
                {
                    continue;
                }
                candidates.push(Resolved {
                    holder: iface,
                    index,
                });
            }
        }
        return select_maximally_specific(candidates);
    }

    // Class C: search C then its superclasses; the first declaration wins.
    let mut current = Some(Arc::clone(c));
    while let Some(k) = current {
        if let Some(index) = find_declared(&k, name, descriptor) {
            return Ok(Some(Resolved {
                holder: k,
                index: index,
            }));
        }
        current = k.super_klass();
    }

    // Not found in the class hierarchy: search the superinterfaces of C and
    // select a maximally-specific method (may be abstract or a default).
    // Private and static interface methods are not inherited by classes.
    let mut candidates = Vec::new();
    for iface in interface_closure(c) {
        if let Some(index) = find_declared(&iface, name, descriptor) {
            let member = &iface.class.methods[index];
            if member.accessFlags & (ACC_PRIVATE | ACC_STATIC) != 0 {
                continue;
            }
            candidates.push(Resolved {
                holder: iface,
                index,
            });
        }
    }
    select_maximally_specific(candidates)
}

fn no_such_method<'a>(c: &InstanceKlass<'a>, name: &[u8], descriptor: &[u8]) -> Error {
    Error::other(format!(
        "NoSuchMethodError: {}.{}{}",
        String::from_utf8_lossy(c.class.name),
        String::from_utf8_lossy(name),
        String::from_utf8_lossy(descriptor)
    ))
}

fn icc(message: impl Into<String>) -> Error {
    Error::other(format!("IncompatibleClassChangeError: {}", message.into()))
}

// ---------------------------------------------------------------------------
// Access checks (JVMS §5.4.4 + JPMS)
// ---------------------------------------------------------------------------

fn package_name(class_name: &[u8]) -> &[u8] {
    match class_name.iter().rposition(|&b| b == b'/') {
        Some(pos) => &class_name[..pos],
        None => b"",
    }
}

/// Module providing `klass`, or `None` for the unnamed module.
fn module_name<'a>(loader: &ClassLoader<'a>, klass: &InstanceKlass<'a>) -> Option<String> {
    loader.class_module_name(klass.class.name)
}

/// Unqualified packages exported by `java.base` (the `java --describe-module
/// java.base` export list). Used to enforce JPMS encapsulation for callers in
/// the unnamed module; qualified exports target named modules only, so they
/// are irrelevant here.
const JAVA_BASE_EXPORTS: &[&str] = &[
    "java.lang",
    "java.lang.annotation",
    "java.lang.constant",
    "java.lang.invoke",
    "java.lang.module",
    "java.lang.ref",
    "java.lang.runtime",
    "java.io",
    "java.math",
    "java.net",
    "java.nio",
    "java.nio.channels",
    "java.nio.charset",
    "java.nio.file",
    "java.nio.file.attribute",
    "java.nio.file.spi",
    "java.nio.file.watch",
    "java.security",
    "java.security.cert",
    "java.security.interfaces",
    "java.security.spec",
    "java.text",
    "java.time",
    "java.time.chrono",
    "java.time.format",
    "java.time.temporal",
    "java.time.zone",
    "java.util",
    "java.util.concurrent",
    "java.util.concurrent.atomic",
    "java.util.concurrent.locks",
    "java.util.function",
    "java.util.jar",
    "java.util.logging",
    "java.util.prefs",
    "java.util.regex",
    "java.util.spi",
    "java.util.stream",
    "java.util.zip",
    "javax.crypto",
    "javax.crypto.interfaces",
    "javax.crypto.spec",
    "javax.net.ssl",
    "jdk.internal.jrtfs",
];

/// `true` when `package` is exported from the named module `module` to the
/// caller's module (`None` = unnamed module). Only unqualified exports reach
/// the unnamed module; named-to-named access is approximated as permitted
/// because this runtime does not yet maintain full `reads`/qualified-export
/// edges for every JDK module.
fn package_exported(module: &str, package: &str, caller_module: Option<&str>) -> bool {
    if caller_module == Some(module) {
        return true;
    }
    match caller_module {
        // Unnamed module: only unqualified exports apply.
        None => match module {
            "java.base" => JAVA_BASE_EXPORTS.contains(&package),
            // Other named modules' export tables are not tracked; assume
            // their packages are exported rather than breaking JDK internals.
            _ => true,
        },
        Some(_) => true,
    }
}

fn same_runtime_package<'a>(
    loader: &ClassLoader<'a>,
    a: &InstanceKlass<'a>,
    b: &InstanceKlass<'a>,
) -> bool {
    package_name(a.class.name) == package_name(b.class.name)
        && module_name(loader, a) == module_name(loader, b)
}

/// `true` when `sub` is `sup` or a subclass of `sup` (JVMS §5.4.3.1 used for
/// protected-member access).
fn is_subclass_of<'a>(sub: &InstanceKlass<'a>, sup_name: &[u8]) -> bool {
    SystemDictionary::is_subclass_of(sub, sup_name)
}

/// Nestmate check (JVMS §5.4.4): two classes are nestmates when they share a
/// nest host, or one is the other's nest host.
fn nestmates<'a>(a: &InstanceKlass<'a>, b: &InstanceKlass<'a>) -> bool {
    if a.class.name == b.class.name {
        return true;
    }
    let a_host = a.class.nest_host();
    let b_host = b.class.nest_host();
    if a_host == Some(b.class.name) || b_host == Some(a.class.name) {
        return true;
    }
    match (a_host, b_host) {
        (Some(ha), Some(hb)) => ha == hb,
        _ => false,
    }
}

fn illegal_access(message: impl Into<String>) -> Error {
    Error::other(format!("IllegalAccessError: {}", message.into()))
}

/// JVMS §5.4.4 access check for a method/field declared in `owner` accessed
/// from `caller`. `loader` supplies module information; callers that do not
/// know the accessing class should not call this.
pub fn check_member_access<'a>(
    loader: &ClassLoader<'a>,
    caller: &InstanceKlass<'a>,
    owner: &InstanceKlass<'a>,
    access_flags: u16,
) -> io::Result<()> {
    if caller.class.name == owner.class.name {
        return Ok(());
    }
    let describe = || {
        format!(
            "{} (from {})",
            String::from_utf8_lossy(owner.class.name),
            String::from_utf8_lossy(caller.class.name)
        )
    };

    // Private members: same class or nestmate only.
    if access_flags & ACC_PRIVATE != 0 {
        if nestmates(caller, owner) {
            return Ok(());
        }
        return Err(illegal_access(format!("private member of {}", describe())));
    }

    // JPMS: when the member's class lives in a named module different from
    // the caller's, the declaring package must be exported to the caller.
    let caller_module = module_name(loader, caller);
    let owner_module = module_name(loader, owner);
    if caller_module != owner_module {
        if let Some(ref module) = owner_module {
            let package = String::from_utf8_lossy(package_name(owner.class.name))
                .replace('/', ".");
            if !package_exported(module, &package, caller_module.as_deref()) {
                return Err(illegal_access(format!(
                    "class {} is not exported from module {}",
                    describe(),
                    module
                )));
            }
        }
    }

    if access_flags & (ACC_PUBLIC | ACC_PROTECTED) == 0 {
        // Package-private: same runtime package required.
        if !same_runtime_package(loader, caller, owner) {
            return Err(illegal_access(format!(
                "package-private member of {}",
                describe()
            )));
        }
        return Ok(());
    }

    if access_flags & ACC_PROTECTED != 0 && access_flags & ACC_PUBLIC == 0 {
        // Protected: same runtime package, or caller is a subclass of owner.
        if same_runtime_package(loader, caller, owner)
            || is_subclass_of(caller, owner.class.name)
        {
            return Ok(());
        }
        return Err(illegal_access(format!("protected member of {}", describe())));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Public resolution entry points (signatures kept stable for callers in
// bytecodeInterpreter / javaCalls / reflectionNatives)
// ---------------------------------------------------------------------------

impl LinkResolver {
    /// Resolves invokevirtual dispatch using receiver klass's vtable.
    ///
    /// This performs JVMS §5.4.3.3 resolution+selection directly on the
    /// receiver class: class-hierarchy methods win over interface defaults,
    /// private methods are never overridden, static resolutions are rejected
    /// with `IncompatibleClassChangeError`, and interface candidates go
    /// through maximally-specific selection.
    pub fn resolve_virtual_call<'a>(
        receiver_klass: &Arc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Arc<InstanceKlass<'a>>, Code<'a>)> {
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
                            return Ok((Arc::clone(k), Code::parse(method)?));
                        }
                    }
                    curr = k.super_klass.as_ref();
                }
            }
        }

        // 2. Class-hierarchy selection: first non-private, non-static
        // declaration walking from the receiver upwards. Private methods are
        // never overrides (JLS 8.4.8.1); static methods cannot be invoked
        // virtually — both fall through per JVMS selection rules.
        let mut current = Some(Arc::clone(receiver_klass));
        let mut interfaces = Vec::new();
        while let Some(k) = current {
            if let Some(index) = find_declared(&k, name, descriptor) {
                let member = &k.class.methods[index];
                let flags = member.accessFlags;
                // Static methods do not override instance methods and
                // private methods are never overridden (JLS 8.4.8.1): both
                // are invisible to virtual selection — keep searching.
                if flags & (ACC_PRIVATE | ACC_STATIC) == 0 {
                    if flags & ACC_ABSTRACT != 0 {
                        return Err(Error::other(format!(
                            "AbstractMethodError: {}.{}{}",
                            String::from_utf8_lossy(k.class.name),
                            name_str,
                            sig_str
                        )));
                    }
                    let code = Code::parse(member)?;
                    return Ok((k, code));
                }
            }
            interfaces.extend(k.interfaces.iter().cloned());
            current = k.super_klass();
        }

        // 3. Interface candidates (defaults and abstract methods) collected
        // from the receiver's whole type, maximally-specific selection.
        let mut candidates = Vec::new();
        let mut seen: HashSet<&'a [u8]> = HashSet::new();
        let mut pending = interfaces;
        while let Some(iface) = pending.pop() {
            if !seen.insert(iface.class.name) {
                continue;
            }
            pending.extend(iface.interfaces.iter().cloned());
            if let Some(index) = find_declared(&iface, name, descriptor) {
                let member = &iface.class.methods[index];
                // Private interface methods are not inherited.
                if member.accessFlags & ACC_PRIVATE == 0 {
                    candidates.push(Resolved {
                        holder: iface,
                        index,
                    });
                }
            }
        }
        if let Some(resolved) = select_maximally_specific(candidates)? {
            let member = resolved.member();
            if member.accessFlags & ACC_ABSTRACT != 0 {
                return Err(Error::other(format!(
                    "AbstractMethodError: {}.{}{}",
                    String::from_utf8_lossy(resolved.holder.class.name),
                    name_str,
                    sig_str
                )));
            }
            let code = Code::parse(member)?;
            return Ok((resolved.holder, code));
        }

        // 4. Public non-static methods of java.lang.Object are always
        // candidates for class method resolution (JVMS §5.4.3.3).
        let mut curr = Some(Arc::clone(receiver_klass));
        while let Some(k) = curr {
            if k.class.name == b"java/lang/Object" {
                if let Some(index) = find_declared(&k, name, descriptor) {
                    let member = &k.class.methods[index];
                    if member.accessFlags & (ACC_PUBLIC | ACC_STATIC) == ACC_PUBLIC {
                        let code = Code::parse(member)?;
                        return Ok((k, code));
                    }
                }
            }
            curr = k.super_klass();
        }

        if std::env::var_os("JVM25_TRACE").is_some() {
            eprintln!(
                "[resolve-fail] {}.{}{}",
                String::from_utf8_lossy(receiver_klass.class.name),
                name_str,
                sig_str,
            );
        }
        Err(no_such_method(receiver_klass, name, descriptor))
    }

    /// JVMS 5.4.3.3 + 5.4.5 dispatch for `invokevirtual`: first resolve the
    /// method in the *symbolic* class named by the constant pool. A private
    /// resolved method is selected directly — private methods are never
    /// overridden, so a subclass method with the same name/descriptor must
    /// not shadow it (e.g. `Handler.setEncoding` calling its private
    /// `setEncoding0` while the receiver is a `StreamHandler`).
    /// Non-private resolutions fall through to receiver-class selection.
    pub fn resolve_virtual_call_scoped<'a>(
        receiver_klass: &Arc<InstanceKlass<'a>>,
        owner_klass: &Arc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Arc<InstanceKlass<'a>>, Code<'a>)> {
        if let Some(resolved) = resolve_method_in(owner_klass, name, descriptor)? {
            let member = resolved.member();
            let flags = member.accessFlags;
            if flags & ACC_STATIC != 0 {
                return Err(icc(format!(
                    "static method {}.{}{} cannot be invoked virtually",
                    String::from_utf8_lossy(resolved.holder.class.name),
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor)
                )));
            }
            if flags & ACC_PRIVATE != 0 {
                // ACC_PRIVATE: select the resolved method itself.
                if flags & ACC_ABSTRACT != 0 {
                    return Err(Error::other(format!(
                        "AbstractMethodError: {}.{}{}",
                        String::from_utf8_lossy(resolved.holder.class.name),
                        String::from_utf8_lossy(name),
                        String::from_utf8_lossy(descriptor)
                    )));
                }
                let code = resolved.code()?;
                return Ok((resolved.holder, code));
            }
        }
        Self::resolve_virtual_call(receiver_klass, name, descriptor)
    }

    /// Resolves invokeinterface dispatch. JVMS §5.4.3.3 resolution happens in
    /// the named interface (and its superinterfaces, maximally-specific),
    /// then java.lang.Object public methods; selection uses the receiver's
    /// itable/vtable.
    pub fn resolve_interface_call<'a>(
        receiver_klass: &Arc<InstanceKlass<'a>>,
        interface_name: &[u8],
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Arc<InstanceKlass<'a>>, Code<'a>)> {
        let iface_str = String::from_utf8_lossy(interface_name);
        let name_str = String::from_utf8_lossy(name);
        let sig_str = String::from_utf8_lossy(descriptor);

        // 1. Check receiver's itable for the selected implementation.
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
                            return Ok((Arc::clone(k), Code::parse(method)?));
                        }
                    }
                    curr = k.super_klass.as_ref();
                }
            }
        }

        // 2. Resolve within the receiver's interface closure: the interface
        // matching `interface_name` (or declaring the method) provides the
        // resolved method; private/default methods on it are directly
        // selectable.
        {
            let mut pending: Vec<Arc<InstanceKlass<'a>>> = receiver_klass.interfaces.clone();
            let mut visited = HashSet::new();
            while let Some(iface) = pending.pop() {
                if !visited.insert(Arc::as_ptr(&iface) as usize) {
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
                    return Ok((Arc::clone(&iface), Code::parse(method)?));
                }
            }
        }

        // 3. If receiver overrides it directly, resolve virtual call.
        if let Ok(res) = Self::resolve_virtual_call(receiver_klass, name, descriptor) {
            return Ok(res);
        }

        // 4. Public non-static methods of java.lang.Object (JVMS §5.4.3.3:
        // interface method resolution falls back to Object's public methods).
        let mut curr = Some(Arc::clone(receiver_klass));
        while let Some(k) = curr {
            if k.class.name == b"java/lang/Object" {
                if let Some(index) = find_declared(&k, name, descriptor) {
                    let member = &k.class.methods[index];
                    if member.accessFlags & (ACC_PUBLIC | ACC_STATIC) == ACC_PUBLIC {
                        let code = Code::parse(member)?;
                        return Ok((k, code));
                    }
                }
            }
            curr = k.super_klass();
        }

        Err(Error::other(format!(
            "NoSuchMethodError: interface method {}.{}{}",
            iface_str, name_str, sig_str
        )))
    }

    /// Resolves invokespecial call (constructors, private methods, super calls).
    /// JVMS §5.4.3.3: for a class owner, only the class and its superclasses
    /// are searched; for an interface owner, the interface and its
    /// superinterfaces are searched maximally-specific. Static resolutions
    /// are rejected with `IncompatibleClassChangeError`.
    pub fn resolve_special_call<'a>(
        target_klass: &Arc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Arc<InstanceKlass<'a>>, Code<'a>)> {
        let resolved = resolve_method_in(target_klass, name, descriptor)?
            .ok_or_else(|| no_such_method(target_klass, name, descriptor))?;
        if resolved.is_static() {
            return Err(icc(format!(
                "static method {}.{}{} cannot be invoked with invokespecial",
                String::from_utf8_lossy(resolved.holder.class.name),
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor)
            )));
        }
        let member = resolved.member();
        if member.accessFlags & ACC_ABSTRACT != 0 {
            return Err(Error::other(format!(
                "AbstractMethodError: {}.{}{}",
                String::from_utf8_lossy(resolved.holder.class.name),
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor)
            )));
        }
        let code = resolved.code()?;
        Ok((resolved.holder, code))
    }

    /// Resolves invokestatic call. JVMS §5.4.3.3: search the class and its
    /// superclasses (superinterfaces too when C is an interface); a
    /// non-static resolution is an `IncompatibleClassChangeError`.
    pub fn resolve_static_call<'a>(
        target_klass: &Arc<InstanceKlass<'a>>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<(Arc<InstanceKlass<'a>>, Code<'a>)> {
        let resolved = resolve_method_in(target_klass, name, descriptor)?
            .ok_or_else(|| {
                Error::other(format!(
                    "NoSuchMethodError: static {}.{}{}",
                    String::from_utf8_lossy(target_klass.class.name),
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(descriptor)
                ))
            })?;
        if !resolved.is_static() {
            return Err(icc(format!(
                "non-static method {}.{}{} cannot be invoked statically",
                String::from_utf8_lossy(resolved.holder.class.name),
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(descriptor)
            )));
        }
        let code = resolved.code()?;
        Ok((resolved.holder, code))
    }

    /// Resolves invokedynamic call site via BootstrapInfo.
    ///
    /// Beyond parsing the BootstrapMethods attribute entry, this validates
    /// that the bootstrap method handle constant exists, refers to a valid
    /// method reference, resolves to a static method, and is accessible from
    /// the caller class under JVMS §5.4.4/JPMS rules. Linkage failures are
    /// reported as `BootstrapMethodError`.
    pub fn resolve_invokedynamic<'a>(
        loader: &ClassLoader<'a>,
        caller_klass: &InstanceKlass<'a>,
        bsm_attr_index: u16,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<BootstrapInfo> {
        let raw_bsm = caller_klass
            .class
            .bootstrap_methods()
            .ok_or_else(|| Error::other("BootstrapMethodError: Missing BootstrapMethods attribute for invokedynamic"))?;

        let mut bs_info = BootstrapInfo::parse_from_attribute(
            raw_bsm,
            bsm_attr_index,
            Symbol::from(name),
            Symbol::from(descriptor),
            &caller_klass.class.constants,
        )
        .map_err(|e| Error::other(format!("BootstrapMethodError: {}", e)))?;

        // The bootstrap_method_ref must be a CONSTANT_MethodHandle.
        let Constant::MethodHandle(kind, ref_index) = caller_klass.constant(bs_info.bsm_ref())
            .map_err(|e| Error::other(format!("BootstrapMethodError: {}", e)))?
        else {
            return Err(Error::other(
                "BootstrapMethodError: bootstrap method is not a MethodHandle constant",
            ));
        };
        if !matches!(*kind, 5 | 6 | 7 | 8) {
            // REF_getField/getStatic/etc. cannot serve as bootstrap methods.
            return Err(Error::other(format!(
                "BootstrapMethodError: unsupported bootstrap MethodHandle kind {kind}"
            )));
        }

        // Resolve the referenced method and enforce static + access rules.
        let [owner, bsm_name, bsm_desc] = caller_klass
            .memberRef(*ref_index, false)
            .map_err(|e| Error::other(format!("BootstrapMethodError: {}", e)))?;
        let bsm_owner = loader.load(owner).map_err(|e| {
            Error::other(format!(
                "BootstrapMethodError: cannot load bootstrap method class {}: {}",
                String::from_utf8_lossy(owner),
                e
            ))
        })?;
        let resolved = Self::resolve_static_call(&bsm_owner, bsm_name, bsm_desc).map_err(|e| {
            // A non-static or missing bootstrap method is a linkage error.
            let msg = e.to_string();
            if msg.contains("IncompatibleClassChangeError") || msg.contains("NoSuchMethodError") {
                Error::other(format!("BootstrapMethodError: {}", msg))
            } else {
                e
            }
        })?;
        let (bsm_holder, _bsm_code) = &resolved;
        let bsm_flags = find_declared(bsm_holder, bsm_name, bsm_desc)
            .map(|index| bsm_holder.class.methods[index].accessFlags)
            .unwrap_or(ACC_PUBLIC);
        check_member_access(loader, caller_klass, bsm_holder, bsm_flags).map_err(|e| {
            Error::other(format!(
                "BootstrapMethodError: bootstrap method not accessible: {}",
                e
            ))
        })?;

        // Record the resolved bootstrap method for diagnostics/caching.
        bs_info.set_resolved_method(
            format!(
                "{}.{}{}",
                String::from_utf8_lossy(bsm_holder.class.name),
                String::from_utf8_lossy(bsm_name),
                String::from_utf8_lossy(bsm_desc)
            ),
            None,
        );

        Ok(bs_info)
    }
}
