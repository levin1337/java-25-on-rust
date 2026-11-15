/*
 * Port of OpenJDK 25 HotSpot ciInstanceKlass (share/ci/ciInstanceKlass.hpp/.cpp)
 *
 * ciInstanceKlass is the compiler-interface view of an InstanceKlass:
 * type hierarchy queries, field and method enumeration, vtable/itable
 * access, initialization state and class-shape predicates.  It is a
 * thin facade over `Arc<InstanceKlass>` plus the owning `CiEnv`.
 */

use super::ciEnv::CiEnv;
use super::ciField::CiField;
use super::ciMethod::CiMethod;
use crate::share::classfile::classFileParser::{Constant, Member};
use crate::share::oops::instanceKlass::{InitState, InstanceKlass};
use crate::share::oops::method::Code;
use std::collections::HashSet;
use std::sync::Arc;

/// `ciInstanceKlass`: compiler-interface view of an `InstanceKlass`.
#[derive(Clone)]
pub struct CiInstanceKlass<'a> {
    pub(crate) env: CiEnv<'a>,
    klass: Arc<InstanceKlass<'a>>,
}

impl<'a> std::fmt::Debug for CiInstanceKlass<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ciInstanceKlass {}", self.name_string())
    }
}

impl<'a> PartialEq for CiInstanceKlass<'a> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.klass, &other.klass)
    }
}

impl<'a> Eq for CiInstanceKlass<'a> {}

impl<'a> CiInstanceKlass<'a> {
    pub fn new(env: CiEnv<'a>, klass: Arc<InstanceKlass<'a>>) -> Self {
        Self { env, klass }
    }

    /// `ciInstanceKlass::env` — the ciEnv that produced this handle.
    #[inline]
    pub fn env(&self) -> &CiEnv<'a> {
        &self.env
    }

    /// `ciInstanceKlass::get_instance_klass`: the underlying VM klass.
    #[inline]
    pub fn instance_klass(&self) -> Arc<InstanceKlass<'a>> {
        Arc::clone(&self.klass)
    }

    /// Borrowed access to the VM klass for ci-internal consumers.
    #[inline]
    pub(crate) fn klass(&self) -> &InstanceKlass<'a> {
        &self.klass
    }

    // ---- identity -----------------------------------------------------------

    /// `ciInstanceKlass::name` — internal name (`java/lang/Object`),
    /// or the array descriptor for array classes.
    #[inline]
    pub fn name(&self) -> &'a [u8] {
        self.klass.class.name
    }

    /// `ciInstanceKlass::name` rendered as a string.
    pub fn name_string(&self) -> String {
        String::from_utf8_lossy(self.name()).into_owned()
    }

    /// `ciInstanceKlass::java_name` — dotted Java name.
    pub fn java_name(&self) -> String {
        String::from_utf8_lossy(self.name())
            .replace('/', ".")
            .replace('[', "")
    }

    /// Whether this klass is `java/lang/Object`.
    #[inline]
    pub fn is_java_lang_object(&self) -> bool {
        self.name() == b"java/lang/Object"
    }

    /// `ciInstanceKlass::is_array_klass`-style query on the facade.
    #[inline]
    pub fn is_array(&self) -> bool {
        self.klass.is_array()
    }

    /// `ciInstanceKlass::array_dimension`.
    #[inline]
    pub fn array_dimension(&self) -> usize {
        self.klass.array_dimension()
    }

    /// Element descriptor of an array class (`[I` -> `I`).
    #[inline]
    pub fn array_element_descriptor(&self) -> Option<&'a [u8]> {
        self.klass.array_element_descriptor()
    }

    // ---- access flags --------------------------------------------------------

    #[inline]
    pub fn access_flags(&self) -> u16 {
        self.klass.class.accessFlags
    }

    #[inline]
    pub fn is_public(&self) -> bool {
        self.klass.class.accessFlags & 0x0001 != 0
    }

    #[inline]
    pub fn is_private(&self) -> bool {
        self.klass.class.accessFlags & 0x0002 != 0
    }

    #[inline]
    pub fn is_protected(&self) -> bool {
        self.klass.class.accessFlags & 0x0004 != 0
    }

    /// `ciInstanceKlass::is_interface`.
    #[inline]
    pub fn is_interface(&self) -> bool {
        self.klass.is_interface()
    }

    /// `ciInstanceKlass::is_abstract`.
    #[inline]
    pub fn is_abstract(&self) -> bool {
        self.klass.is_abstract()
    }

    /// `ciInstanceKlass::is_final`.
    #[inline]
    pub fn is_final(&self) -> bool {
        self.klass.is_final()
    }

    #[inline]
    pub fn is_static(&self) -> bool {
        false
    }

    #[inline]
    pub fn is_synthetic(&self) -> bool {
        self.klass.class.accessFlags & 0x1000 != 0
    }

    #[inline]
    pub fn is_annotation(&self) -> bool {
        self.is_interface() && self.klass.class.accessFlags & 0x2000 != 0
    }

    #[inline]
    pub fn is_enum(&self) -> bool {
        self.klass.class.accessFlags & 0x4000 != 0
    }

    #[inline]
    pub fn is_record(&self) -> bool {
        self.klass.class.record_components().is_some()
    }

    #[inline]
    pub fn is_sealed(&self) -> bool {
        !self.klass.class.permitted_subclasses().is_empty()
    }

    /// `ciInstanceKlass::is_leaf_class`: final classes (no subclassing
    /// possible) — HotSpot also excludes interfaces and hidden/anonymous
    /// classes which this model does not represent.
    #[inline]
    pub fn is_leaf_class(&self) -> bool {
        self.is_final() && !self.is_interface()
    }

    /// `ciInstanceKlass::is_box_klass` — primitive wrapper classes.
    pub fn is_box_klass(&self) -> bool {
        matches!(
            self.name(),
            b"java/lang/Boolean"
                | b"java/lang/Byte"
                | b"java/lang/Character"
                | b"java/lang/Short"
                | b"java/lang/Integer"
                | b"java/lang/Long"
                | b"java/lang/Float"
                | b"java/lang/Double"
                | b"java/lang/Void"
        )
    }

    /// `ciInstanceKlass::is_instantiable`: non-abstract, non-interface,
    /// non-array class that can be allocated with `new`.
    #[inline]
    pub fn is_instantiable(&self) -> bool {
        !self.is_abstract() && !self.is_interface() && !self.is_array()
    }

    // ---- initialization --------------------------------------------------------

    /// `ciInstanceKlass::is_initialized` — `clinit` completed or not
    /// required.
    pub fn is_initialized(&self) -> bool {
        matches!(
            self.klass.state.get(),
            InitState::FullyInitialized | InitState::Linked | InitState::Allocated
        )
    }

    /// Raw initialization state for callers needing finer detail.
    #[inline]
    pub fn init_state(&self) -> InitState {
        self.klass.state.get()
    }

    /// `ciInstanceKlass::has_finalizer`.
    #[inline]
    pub fn has_finalizer(&self) -> bool {
        self.klass.has_finalizer()
    }

    /// `ciInstanceKlass::initializer` — the `<clinit>` Code attribute.
    pub fn initializer(&self) -> Option<Code<'a>> {
        self.klass.initializer().ok().flatten()
    }

    // ---- hierarchy ---------------------------------------------------------------

    /// `ciInstanceKlass::super_klass`: direct superclass, `None` for
    /// `java/lang/Object` and interfaces.
    pub fn super_klass(&self) -> Option<CiInstanceKlass<'a>> {
        self.klass
            .super_klass
            .clone()
            .map(|k| CiInstanceKlass::new(self.env.clone(), k))
    }

    /// Direct superinterfaces.
    pub fn direct_interfaces(&self) -> Vec<CiInstanceKlass<'a>> {
        self.klass
            .interfaces
            .iter()
            .map(|k| CiInstanceKlass::new(self.env.clone(), Arc::clone(k)))
            .collect()
    }

    /// `ciInstanceKlass::transitive_interfaces`: all direct and indirect
    /// superinterfaces (depth-first, deduplicated by internal name).
    pub fn transitive_interfaces(&self) -> Vec<CiInstanceKlass<'a>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack: Vec<Arc<InstanceKlass<'a>>> = self.klass.interfaces.clone();
        while let Some(k) = stack.pop() {
            if !seen.insert(k.class.name.to_vec()) {
                continue;
            }
            stack.extend(k.interfaces.iter().cloned());
            out.push(CiInstanceKlass::new(self.env.clone(), k));
        }
        out
    }

    /// `ciInstanceKlass::is_subclass_of` by internal name:
    /// `self` equals `name` or inherits from a class named `name`.
    #[inline]
    pub fn is_subclass_of_name(&self, name: &[u8]) -> bool {
        self.klass.is_subclass_of(name)
    }

    /// `ciInstanceKlass::is_subclass_of` by handle.
    pub fn is_subclass_of(&self, other: &CiInstanceKlass<'a>) -> bool {
        self.name() == other.name() || self.klass.is_subclass_of(other.name())
    }

    /// `ciInstanceKlass::implements_interface` by internal name.
    #[inline]
    pub fn implements_interface_name(&self, name: &[u8]) -> bool {
        self.klass.implements_interface(name)
    }

    /// `ciInstanceKlass::implements_interface` by handle.
    pub fn implements_interface(&self, other: &CiInstanceKlass<'a>) -> bool {
        self.name() != other.name() && self.klass.implements_interface(other.name())
    }

    /// `ciInstanceKlass::is_subinterface_of`-style check: both are
    /// interfaces and `self` extends `other` transitively.
    pub fn is_subinterface_of(&self, other: &CiInstanceKlass<'a>) -> bool {
        self.is_interface() && other.is_interface() && self.implements_interface(other)
    }

    /// `ciInstanceKlass::is_assignable_from`-style helper: `other` is a
    /// subtype of `self` (subclass or implementing interface).
    pub fn is_assignable_from(&self, other: &CiInstanceKlass<'a>) -> bool {
        other.is_subclass_of(self) || other.implements_interface(self)
    }

    /// `ciInstanceKlass::super_depth`: number of superclasses above.
    #[inline]
    pub fn super_depth(&self) -> usize {
        self.klass.super_depth()
    }

    /// `ciInstanceKlass::has_subklass`: whether any *other* loaded class
    /// is a strict subclass of this one.  Scans the loader's class
    /// table (the port has no ClassHierarchyWalker cache).
    pub fn has_subklass(&self) -> bool {
        self.env
            .all_klasses()
            .iter()
            .any(|k| k.name() != self.name() && k.is_subclass_of(self))
    }

    /// `ciInstanceKlass::find_implementor` analogue: first loaded class
    /// implementing `self` as an interface (or the class itself).
    pub fn find_implementor(&self) -> Option<CiInstanceKlass<'a>> {
        if !self.is_interface() {
            return None;
        }
        self.env
            .all_klasses()
            .into_iter()
            .find(|k| k.name() != self.name() && k.implements_interface(self))
    }

    /// `ciInstanceKlass::has_nonstatic_concrete_methods`.
    #[inline]
    pub fn has_nonstatic_concrete_methods(&self) -> bool {
        self.klass.has_nonstatic_concrete_methods()
    }

    // ---- packages / loaders -------------------------------------------------------

    /// Package prefix of the internal name (`java/lang` for
    /// `java/lang/Object`), empty for default-package classes.
    pub fn package_name(&self) -> &'a [u8] {
        match self.name().iter().rposition(|&b| b == b'/') {
            Some(pos) => &self.name()[..pos],
            None => b"",
        }
    }

    /// `ciInstanceKlass::is_same_package`-style check: same package
    /// prefix and same class loader (all classes here share the
    /// CiEnv's loader).
    pub fn is_same_package(&self, other: &CiInstanceKlass<'a>) -> bool {
        self.package_name() == other.package_name()
    }

    // ---- size/layout ---------------------------------------------------------------

    /// `ciInstanceKlass::instance_size` in bytes.
    #[inline]
    pub fn instance_size(&self) -> usize {
        self.klass.instance_size()
    }

    /// `ciInstanceKlass::nonstatic_field_size` in bytes.
    #[inline]
    pub fn nonstatic_field_size(&self) -> usize {
        self.klass.nonstatic_field_size()
    }

    /// `ciInstanceKlass::static_oop_field_count`-style counts.
    pub fn nonstatic_oop_field_count(&self) -> usize {
        self.klass
            .instance_field_infos
            .iter()
            .filter(|f| f.is_reference())
            .count()
    }

    /// Byte offset of a declared field by name+descriptor
    /// (`instanceField`/`field_offset` underneath).
    pub fn field_offset(&self, name: &[u8], descriptor: &[u8]) -> Option<u32> {
        self.klass.field_offset(name, descriptor).ok()
    }

    /// `ciInstanceKlass::get_field_by_offset`: field occupying `offset`
    /// in this klass's own field area.  For instance fields `offset`
    /// is the byte offset; for static fields it is the slot index.
    pub fn get_field_by_offset(&self, offset: i32, is_static: bool) -> Option<CiField<'a>> {
        let infos = if is_static {
            &self.klass.static_field_infos
        } else {
            &self.klass.instance_field_infos
        };
        let info = infos.iter().find(|f| f.offset as i32 == offset)?;
        Some(CiField::new(self.clone(), info.clone()))
    }

    // ---- fields -------------------------------------------------------------------

    /// `ciInstanceKlass::fields`: all declared fields (static + instance)
    /// as ci handles.
    pub fn fields(&self) -> Vec<CiField<'a>> {
        self.klass
            .instance_field_infos
            .iter()
            .chain(self.klass.static_field_infos.iter())
            .map(|f| CiField::new(self.clone(), f.clone()))
            .collect()
    }

    /// `ciInstanceKlass::find_field`: field declared in *this* klass.
    /// `is_static` restricts the search to one field area, matching
    /// HotSpot's `find_field`.
    pub fn find_field(
        &self,
        name: &[u8],
        signature: &[u8],
        is_static: bool,
    ) -> Option<CiField<'a>> {
        let info = self.klass.field_info(name, signature)?;
        if info.is_static() != is_static {
            return None;
        }
        Some(CiField::new(self.clone(), info.clone()))
    }

    /// Field lookup walking the superclass chain (and transitive
    /// interfaces) — canonical-holder resolution matching
    /// `ciEnv::get_field` but reusable without an env handle.
    pub fn find_field_in_hierarchy(
        &self,
        name: &[u8],
        signature: &[u8],
        is_static: bool,
    ) -> Option<CiField<'a>> {
        self.env.get_field(self, name, signature, is_static)
    }

    // ---- methods -------------------------------------------------------------------

    /// `ciInstanceKlass::methods`: all declared methods as ci handles.
    pub fn methods(&self) -> Vec<CiMethod<'a>> {
        self.klass
            .methods()
            .map(|m| CiMethod::new(self.clone(), m.clone()))
            .collect()
    }

    /// Methods declared in this klass with the given name.
    pub fn methods_named(&self, name: &[u8]) -> Vec<CiMethod<'a>> {
        self.klass
            .methods()
            .filter(|m| m.name == name)
            .map(|m| CiMethod::new(self.clone(), m.clone()))
            .collect()
    }

    /// `ciInstanceKlass::find_method`: method declared in *this* klass
    /// with the given name+signature (HotSpot `find_method` is a
    /// declared-only lookup; use `CiEnv::lookup_method` for the
    /// hierarchy walk).
    pub fn find_method(&self, name: &[u8], signature: &[u8]) -> Option<CiMethod<'a>> {
        let member = self.klass.declared_method(name, signature)?;
        Some(CiMethod::new(self.clone(), member.clone()))
    }

    /// Method lookup walking the superclass chain plus interface
    /// default methods — thin wrapper over `CiEnv::lookup_method`.
    pub fn lookup_method(&self, name: &[u8], signature: &[u8]) -> Option<CiMethod<'a>> {
        self.env.lookup_method(self, name, signature)
    }

    // ---- vtable / itable -----------------------------------------------------------------

    /// `ciInstanceKlass::vtable_index` (`klassVtable::index_of_method`).
    /// Returns `Method::NONVIRTUAL_VTABLE_INDEX` (-1) for non-virtual
    /// methods, mirroring HotSpot's sentinel.
    pub fn vtable_index(&self, name: &[u8], signature: &[u8]) -> i32 {
        self.klass
            .vtable_index(name, signature)
            .map_or(-1, |i| i as i32)
    }

    /// `ciInstanceKlass::vtable_entry`-style query: raw vtable entry at
    /// `index`.  Returns `(holder klass name, method name,
    /// descriptor)` when the entry resolves.
    pub fn vtable_method(&self, index: usize) -> Option<(&str, &str, &str)> {
        let entry = self.klass.vtable_entry(index)?;
        Some((entry.holder_klass(), entry.method_name(), entry.signature()))
    }

    /// `ciInstanceKlass::itable_vtable_index`: receiver vtable slot for
    /// an `invokeinterface` target.
    pub fn itable_vtable_index(
        &self,
        interface_name: &[u8],
        name: &[u8],
        signature: &[u8],
    ) -> Option<i32> {
        self.klass
            .itable_vtable_index(interface_name, name, signature)
            .map(|i| i as i32)
    }

    /// `ciInstanceKlass::default_method_superinterfaces`.
    pub fn default_method_superinterfaces(&self) -> Vec<CiInstanceKlass<'a>> {
        self.klass
            .default_method_superinterfaces()
            .into_iter()
            .map(|k| CiInstanceKlass::new(self.env.clone(), k))
            .collect()
    }

    // ---- constant pool helpers --------------------------------------------------------------

    /// `ciInstanceKlass::className(index)`: internal name for a
    /// CONSTANT_Class index.
    pub fn class_name_of(&self, index: u16) -> Option<&'a [u8]> {
        self.klass.className(index).ok()
    }

    /// `ciInstanceKlass::constant(index)` for literal resolution.
    pub fn constant(&self, index: u16) -> Option<&Constant<'a>> {
        self.klass.constant(index).ok()
    }

    /// `ciInstanceKlass::utf8(index)`.
    pub fn utf8(&self, index: u16) -> Option<&'a [u8]> {
        self.klass.utf8(index).ok()
    }

    /// `ciInstanceKlass::member_ref(index)`: (owner, name, descriptor)
    /// of a Fieldref/Methodref constant.
    pub fn member_ref(&self, index: u16) -> Option<[&'a [u8]; 3]> {
        self.klass.memberRef(index, false).ok()
    }

    /// Whether a class-file member (method or field) exists in this
    /// klass's declared members — used by replay/debug tooling.
    pub fn has_declared_member(&self, name: &[u8], descriptor: &[u8], is_method: bool) -> bool {
        let members: &Vec<Member<'a>> = if is_method {
            &self.klass.class.methods
        } else {
            &self.klass.class.fields
        };
        members
            .iter()
            .any(|m| m.name == name && m.descriptor == descriptor)
    }
}
