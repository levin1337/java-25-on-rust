/*
 * Port of OpenJDK 25 HotSpot InstanceKlass (share/oops/instanceKlass.hpp, instanceKlass.cpp)
 *
 * InstanceKlass represents the VM metadata for a loaded Java class:
 * - Constant pool and classfile references
 * - Superclass and implemented interface hierarchies
 * - Field layouts with computed memory offsets
 * - Virtual method dispatch table (vtable) and interface dispatch table (itable)
 * - Initialization state and static storage cells
 */

use super::constantPool::{MemberRef, MemberRefKind};
use super::fieldInfo::FieldInfo;
use super::fieldLayout::{FieldLayout, FieldLayoutResult};
use super::klassVtable::{KlassItable, KlassVtable, VtableEntry};
use super::method::Code;
use super::objLayout::ObjLayout;
use crate::share::classfile::{ClassFile, Constant, Member, SystemDictionary};
use crate::share::interpreter::bytecodeInterpreter::Value;
use crate::share::runtime::shareCell::Share;
use std::io::{self, Error};
use std::sync::{Arc, Condvar, Mutex};

pub struct InstanceKlass<'a> {
    pub class: ClassFile<'a>,
    pub super_klass: Option<Arc<InstanceKlass<'a>>>,
    pub interfaces: Vec<Arc<InstanceKlass<'a>>>,
    pub(crate) state: Share<InitState>,
    /// JVMS §12.4.2 initialization lock: `Some(tid)` while `tid` runs
    /// `<clinit>`; other threads wait on `init_cond` until the state
    /// leaves `BeingInitialized`.
    pub(crate) init_gate: Arc<(Mutex<Option<u64>>, Condvar)>,
    pub static_fields: Vec<Share<Value<'a>>>,
    pub static_field_infos: Vec<FieldInfo<'a>>,
    pub instance_field_infos: Vec<FieldInfo<'a>>,
    pub instance_field_offset: usize,
    pub total_instance_fields: usize,
    pub instance_size: usize,
    pub nonstatic_field_size: usize,
    /// Full instance field layout (header + inherited/own field blocks).
    /// Subclasses consume this to reproduce HotSpot's hole-filling field
    /// placement; JIT consumers read `FieldInfo::offset` values computed
    /// from it.
    pub instance_layout: FieldLayoutResult,
    pub static_field_size: usize,
    pub vtable: KlassVtable,
    pub itable: KlassItable,
}

impl<'a> std::fmt::Debug for InstanceKlass<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "InstanceKlass({})",
            String::from_utf8_lossy(self.class.name)
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InitState {
    Allocated,
    Loaded,
    Linked,
    BeingInitialized,
    FullyInitialized,
    InitializationError,
}

impl<'a> InstanceKlass<'a> {
    pub fn new(
        class: ClassFile<'a>,
        super_klass: Option<Arc<InstanceKlass<'a>>>,
        interfaces: Vec<Arc<InstanceKlass<'a>>>,
    ) -> io::Result<Self> {
        let instance_field_offset = super_klass.as_ref().map_or(0, |s| s.total_instance_fields);

        let mut static_field_infos = Vec::new();
        let mut instance_field_infos = Vec::new();
        let mut static_fields = Vec::new();

        let mut static_slot = 0;
        let mut instance_slot = instance_field_offset;

        for field in &class.fields {
            let is_static = field.accessFlags & 8 != 0;
            if is_static {
                let info =
                    FieldInfo::new(field.accessFlags, field.name, field.descriptor, static_slot);
                static_field_infos.push(info);
                static_slot += 1;

                let mut val = match field.descriptor {
                    b"I" | b"B" | b"C" | b"S" | b"Z" => Value::Int(0),
                    b"J" => Value::Long(0),
                    b"F" => Value::Float(0.0),
                    b"D" => Value::Double(0.0),
                    _ => Value::Null,
                };

                if let Some(attr) = field.attributes.iter().find(|a| a.name == b"ConstantValue") {
                    let index = u16::from_be_bytes(attr.bytes.try_into().map_err(Error::other)?);
                    if let Some(constant) = class.constants.get(usize::from(index)) {
                        val = match constant {
                            Constant::Integer(v) => Value::Int(*v),
                            Constant::Long(v) => Value::Long(*v),
                            Constant::Float(v) => Value::Float(*v),
                            Constant::Double(v) => Value::Double(*v),
                            Constant::String(s_idx) => {
                                if let Some(Constant::Utf8(bytes)) =
                                    class.constants.get(usize::from(*s_idx))
                                {
                                    Value::String(bytes)
                                } else {
                                    val
                                }
                            }
                            _ => val,
                        };
                    }
                }
                static_fields.push(Share::new(val));
            } else {
                let info = FieldInfo::new(
                    field.accessFlags,
                    field.name,
                    field.descriptor,
                    instance_slot,
                );
                instance_field_infos.push(info);
                instance_slot += 1;
            }
        }

        // JVM-injected fields (HotSpot `@Injected`): these members are added
        // by the VM at class-load time and are absent from the class file.
        // - java.lang.invoke.ResolvedMethodName: `vmtarget` (JVM_Method*),
        //   `vmholder` (declaring Class).
        // - java.lang.invoke.MemberName: `vmindex` (vtable index/offset).
        match class.name {
            b"java/lang/invoke/ResolvedMethodName" => {
                instance_field_infos.push(FieldInfo::new(0, b"vmtarget", b"J", instance_slot));
                instance_slot += 1;
                instance_field_infos.push(FieldInfo::new(
                    0,
                    b"vmholder",
                    b"Ljava/lang/Class;",
                    instance_slot,
                ));
                instance_slot += 1;
            }
            b"java/lang/invoke/MemberName" => {
                instance_field_infos.push(FieldInfo::new(0, b"vmindex", b"J", instance_slot));
                instance_slot += 1;
            }
            _ => {}
        }

        let total_instance_fields = instance_slot;

        // Compute memory layout and field byte offsets
        let layout = ObjLayout::default();
        let layout_res = FieldLayout::compute_instance_layout_inheriting(
            &layout,
            super_klass.as_ref().map(|s| &s.instance_layout),
            &mut instance_field_infos,
        );
        let static_size = FieldLayout::compute_static_layout(0, &mut static_field_infos);

        let class_name_str = String::from_utf8_lossy(class.name).to_string();

        // Build vtable inheriting from superclass
        let mut vtable = if let Some(super_k) = &super_klass {
            KlassVtable::from_super(class_name_str.clone(), &super_k.vtable)
        } else {
            KlassVtable::new(class_name_str.clone())
        };

        for method in &class.methods {
            let is_static = (method.accessFlags & 0x0008) != 0;
            let is_private = (method.accessFlags & 0x0002) != 0;
            let is_init = method.name == b"<init>" || method.name == b"<clinit>";
            if !is_static && !is_private && !is_init {
                let m_name = String::from_utf8_lossy(method.name).to_string();
                let m_sig = String::from_utf8_lossy(method.descriptor).to_string();
                vtable.add_or_override_method(
                    class_name_str.clone(),
                    m_name,
                    m_sig,
                    method.accessFlags,
                );
            }
        }

        // Build itable for interface implementations, covering the whole
        // transitive superinterface hierarchy (HotSpot registers every
        // superinterface, direct or indirect, in the itable).  Interfaces
        // inherited through the superclass chain are seeded as well:
        // HotSpot's itable of a subclass contains every interface the
        // superclass implements.
        let mut itable = KlassItable::new(class_name_str);
        let mut all_interfaces: Vec<Arc<InstanceKlass<'a>>> = Vec::new();
        let mut pending: Vec<Arc<InstanceKlass<'a>>> = interfaces.clone();
        let mut current_super = super_klass.as_ref();
        while let Some(k) = current_super {
            pending.extend(k.interfaces.iter().cloned());
            current_super = k.super_klass.as_ref();
        }
        let mut seen: std::collections::HashSet<&'a [u8]> = std::collections::HashSet::new();
        while let Some(iface) = pending.pop() {
            if !seen.insert(iface.class.name) {
                continue;
            }
            pending.extend(iface.interfaces.iter().cloned());
            all_interfaces.push(iface);
        }
        for iface in &all_interfaces {
            let iface_name = String::from_utf8_lossy(iface.class.name).to_string();
            for m in &iface.class.methods {
                let is_static = (m.accessFlags & 0x0008) != 0;
                let is_private = (m.accessFlags & 0x0002) != 0;
                if !is_static && !is_private {
                    let m_name = String::from_utf8_lossy(m.name).to_string();
                    let m_sig = String::from_utf8_lossy(m.descriptor).to_string();
                    // HotSpot registers every inherited interface method in
                    // the vtable: concrete defaults as regular entries and
                    // abstract methods as miranda entries, so that
                    // `invokeinterface` can resolve through the itable.
                    let v_idx = match vtable.index_of_method(&m_name, &m_sig) {
                        Some(idx) => idx,
                        None => {
                            if m.code_attribute().is_some() {
                                vtable.add_or_override_method(
                                    iface_name.clone(),
                                    m_name.clone(),
                                    m_sig.clone(),
                                    m.accessFlags,
                                )
                            } else {
                                vtable.add_miranda_method(
                                    iface_name.clone(),
                                    m_name.clone(),
                                    m_sig.clone(),
                                    m.accessFlags,
                                )
                            }
                        }
                    };
                    itable.add_entry(iface_name.clone(), m_name, m_sig, v_idx);
                }
            }
        }

        Ok(Self {
            class,
            super_klass,
            interfaces,
            state: Share::new(InitState::Loaded),
            init_gate: Arc::new((Mutex::new(None), Condvar::new())),
            static_fields,
            static_field_infos,
            instance_field_infos,
            instance_field_offset,
            total_instance_fields,
            instance_size: layout_res.instance_size,
            nonstatic_field_size: layout_res.nonstatic_field_size,
            instance_layout: layout_res,
            static_field_size: static_size,
            vtable,
            itable,
        })
    }

    #[inline]
    pub fn super_klass(&self) -> Option<Arc<InstanceKlass<'a>>> {
        self.super_klass.as_ref().map(Arc::clone)
    }

    #[inline]
    pub fn vtable(&self) -> &KlassVtable {
        &self.vtable
    }

    #[inline]
    pub fn itable(&self) -> &KlassItable {
        &self.itable
    }

    #[inline]
    pub fn instance_size(&self) -> usize {
        self.instance_size
    }

    #[inline]
    pub fn nonstatic_field_size(&self) -> usize {
        self.nonstatic_field_size
    }

    #[inline]
    pub fn static_field_size(&self) -> usize {
        self.static_field_size
    }

    /// Whether this type declares or inherits a non-static concrete
    /// (default) interface method, mirroring HotSpot's
    /// `has_nonstatic_concrete_methods`.
    pub fn has_nonstatic_concrete_methods(&self) -> bool {
        if self.is_interface() {
            let declares_concrete = self.class.methods.iter().any(|m| {
                (m.accessFlags & 0x0008) == 0
                    && (m.accessFlags & 0x0400) == 0
                    && m.code_attribute().is_some()
            });
            return declares_concrete
                || self
                    .interfaces
                    .iter()
                    .any(|iface| iface.has_nonstatic_concrete_methods());
        }
        self.interfaces
            .iter()
            .any(|iface| iface.has_nonstatic_concrete_methods())
            || self
                .super_klass
                .as_ref()
                .is_some_and(|super_k| super_k.has_nonstatic_concrete_methods())
    }

    /// Direct superinterfaces that must be initialized before this class,
    /// per JVMS §5.5: every direct superinterface declaring a non-static
    /// concrete method triggers recursive initialization.
    pub fn default_method_superinterfaces(&self) -> Vec<Arc<InstanceKlass<'a>>> {
        if self.is_interface() || !self.has_nonstatic_concrete_methods() {
            return Vec::new();
        }
        self.interfaces
            .iter()
            .filter(|iface| {
                iface.class.methods.iter().any(|m| {
                    (m.accessFlags & 0x0008) == 0
                        && (m.accessFlags & 0x0400) == 0
                        && m.code_attribute().is_some()
                })
            })
            .cloned()
            .collect()
    }

    #[inline]
    pub fn is_interface(&self) -> bool {
        (self.class.accessFlags & 0x0200) != 0
    }

    #[inline]
    pub fn is_abstract(&self) -> bool {
        (self.class.accessFlags & 0x0400) != 0
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        (self.class.accessFlags & 0x0010) != 0
    }

    pub fn staticField(&self, name: &[u8], descriptor: &[u8]) -> io::Result<&Share<Value<'a>>> {
        for (info, cell) in self.static_field_infos.iter().zip(&self.static_fields) {
            if info.name == name && info.descriptor == descriptor {
                return Ok(cell);
            }
        }
        if let Some(super_k) = &self.super_klass {
            return super_k.staticField(name, descriptor);
        }
        Err(Error::other(format!(
            "Missing static field {}{}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
        )))
    }

    pub fn staticInt(&self, name: &[u8], descriptor: &[u8]) -> io::Result<&Share<Value<'a>>> {
        self.staticField(name, descriptor)
    }

    pub(crate) fn staticFieldOwner(
        self: &Arc<Self>,
        name: &[u8],
        descriptor: &[u8],
    ) -> io::Result<Arc<Self>> {
        if self
            .static_field_infos
            .iter()
            .any(|field| field.name == name && field.descriptor == descriptor)
        {
            return Ok(Arc::clone(self));
        }
        for interface in &self.interfaces {
            if let Ok(owner) = interface.staticFieldOwner(name, descriptor) {
                return Ok(owner);
            }
        }
        if let Some(parent) = &self.super_klass {
            return parent.staticFieldOwner(name, descriptor);
        }
        Err(Error::other("Missing static field"))
    }

    pub fn initializer(&self) -> io::Result<Option<Code<'a>>> {
        self.class
            .methods
            .iter()
            .find(|method| method.name == b"<clinit>" && method.descriptor == b"()V")
            .map(Code::parse)
            .transpose()
    }

    pub fn staticCode(&self, name: &[u8], descriptor: &[u8], public: bool) -> io::Result<Code<'a>> {
        self.methodCode(name, descriptor, true, public)
    }

    pub fn methodCode(
        &self,
        name: &[u8],
        descriptor: &[u8],
        is_static: bool,
        public: bool,
    ) -> io::Result<Code<'a>> {
        let method = self.class.methods.iter().find(|method| {
            method.name == name
                && method.descriptor == descriptor
                && (method.accessFlags & 8 != 0) == is_static
                && (!public || method.accessFlags & 1 != 0)
        });

        if let Some(m) = method {
            return Code::parse(m);
        }

        if !is_static && let Some(super_k) = &self.super_klass {
            return super_k.methodCode(name, descriptor, is_static, public);
        }

        Err(Error::other(format!(
            "Missing method {}{}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
        )))
    }

    pub fn instanceField(&self, name: &[u8], descriptor: &[u8]) -> io::Result<usize> {
        for info in &self.instance_field_infos {
            if info.name == name && info.descriptor == descriptor {
                return Ok(info.slot);
            }
        }
        if let Some(super_k) = &self.super_klass {
            return super_k.instanceField(name, descriptor);
        }
        Err(Error::other(format!(
            "Missing instance field {}{} on {}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor),
            String::from_utf8_lossy(&self.class.name)
        )))
    }

    pub fn field_offset(&self, name: &[u8], descriptor: &[u8]) -> io::Result<u32> {
        for info in &self.instance_field_infos {
            if info.name == name && info.descriptor == descriptor {
                return Ok(info.offset);
            }
        }
        for info in &self.static_field_infos {
            if info.name == name && info.descriptor == descriptor {
                return Ok(info.offset);
            }
        }
        if let Some(super_k) = &self.super_klass {
            return super_k.field_offset(name, descriptor);
        }
        Err(Error::other(format!(
            "Missing field {}{}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
        )))
    }

    /// HotSpot `InstanceKlass::has_finalizer`: true when this class or a
    /// superclass (other than `java.lang.Object`) declares `finalize()V`.
    /// Such instances must be registered with `java.lang.ref.Finalizer`
    /// at allocation time so the FinalizerThread can run `finalize()`.
    pub fn has_finalizer(&self) -> bool {
        fn declares_finalize(klass: &InstanceKlass<'_>) -> bool {
            klass.class.methods.iter().any(|method| {
                method.name == b"finalize"
                    && method.descriptor == b"()V"
                    && (method.accessFlags & 0x0008) == 0 // not static
            })
        }
        // java.lang.Object.finalize and java.lang.Enum.finalize (final,
        // empty — enums may not declare finalize) do not make a class
        // finalizable.
        if self.class.name != b"java/lang/Object"
            && self.class.name != b"java/lang/Enum"
            && declares_finalize(self)
        {
            return true;
        }
        let mut current = self.super_klass.as_ref();
        while let Some(klass) = current {
            if klass.class.name == b"java/lang/Object" || klass.class.name == b"java/lang/Enum" {
                return false;
            }
            if declares_finalize(klass) {
                return true;
            }
            current = klass.super_klass.as_ref();
        }
        false
    }

    pub fn className(&self, index: u16) -> io::Result<&'a [u8]> {
        match self.constant(index)? {
            Constant::Class(name) => self.utf8(*name),
            _ => Err(Error::other("Expected class constant")),
        }
    }

    pub fn constant(&self, index: u16) -> io::Result<&Constant<'a>> {
        self.class
            .constants
            .get(usize::from(index))
            .ok_or_else(|| Error::other("Invalid constant index"))
    }

    pub fn utf8(&self, index: u16) -> io::Result<&'a [u8]> {
        match self.constant(index)? {
            Constant::Utf8(bytes) => Ok(bytes),
            _ => Err(Error::other("Expected UTF-8 constant")),
        }
    }

    pub fn memberRef(&self, index: u16, field: bool) -> io::Result<[&'a [u8]; 3]> {
        let kind = |c: &Constant<'a>| match c {
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
        };
        let (owner, member) = match (self.constant(index)?, field) {
            (Constant::FieldRef(owner, member), true)
            | (Constant::MethodRef(owner, member), false)
            | (Constant::InterfaceMethodRef(owner, member), false) => (*owner, *member),
            (other, _) => {
                return Err(Error::other(format!(
                    "Invalid member reference #{}: {}",
                    index,
                    kind(other)
                )));
            }
        };
        match (self.constant(owner)?, self.constant(member)?) {
            (Constant::Class(owner), Constant::NameAndType(name, descriptor)) => Ok([
                self.utf8(*owner)?,
                self.utf8(*name)?,
                self.utf8(*descriptor)?,
            ]),
            (o, m) => Err(Error::other(format!(
                "Invalid member reference #{}: {}/{}",
                index,
                kind(o),
                kind(m)
            ))),
        }
    }

    // ------------------------------------------------------------------
    // JIT metadata queries (additive; used by the compile broker to fill
    // MethodMetadata: field offsets, call sites, stub sites, vtable
    // indices).
    // ------------------------------------------------------------------

    /// `true` for VM-defined array classes whose name is a field
    /// descriptor starting with `[` (see `ClassLoader::load_array_class`).
    #[inline]
    pub fn is_array(&self) -> bool {
        self.class.name.first() == Some(&b'[')
    }

    /// Number of array dimensions for array classes (`[[I` -> 2).
    pub fn array_dimension(&self) -> usize {
        self.class.name.iter().take_while(|b| **b == b'[').count()
    }

    /// Element descriptor of an array class: the class name minus the
    /// leading `[` of the outermost dimension (`[I` -> `I`,
    /// `[[Ljava/lang/String;` -> `[Ljava/lang/String;`).
    pub fn array_element_descriptor(&self) -> Option<&'a [u8]> {
        if self.is_array() && self.class.name.len() > 1 {
            Some(&self.class.name[1..])
        } else {
            None
        }
    }

    /// HotSpot `InstanceKlass::is_subclass_of`: `true` when `name`
    /// names this class or one of its superclasses.
    pub fn is_subclass_of(&self, name: &[u8]) -> bool {
        SystemDictionary::is_subclass_of(self, name)
    }

    /// HotSpot `InstanceKlass::implements_interface`: `true` when
    /// `name` names a direct or transitive superinterface.
    pub fn implements_interface(&self, name: &[u8]) -> bool {
        SystemDictionary::implements_interface(self, name)
    }

    /// Number of superclasses above this class (`java.lang.Object` -> 0).
    pub fn super_depth(&self) -> usize {
        let mut depth = 0;
        let mut current = self.super_klass();
        while let Some(k) = current {
            depth += 1;
            current = k.super_klass();
        }
        depth
    }

    /// HotSpot `InstanceKlass::java_mirror` analogue.  This port has no
    /// dedicated `java.lang.Class` heap object per klass; the interpreter
    /// represents mirrors as `Value::ClassMirror` carrying the internal
    /// class name, materialized on demand by
    /// `prims::classNatives::materialize_class`.
    pub fn java_mirror(&self) -> Value<'a> {
        Value::ClassMirror(self.class.name)
    }

    /// Declared methods of this class (raw class-file members).
    pub fn methods(&self) -> impl Iterator<Item = &Member<'a>> {
        self.class.methods.iter()
    }

    /// Declared fields of this class (raw class-file members).
    pub fn fields(&self) -> impl Iterator<Item = &Member<'a>> {
        self.class.fields.iter()
    }

    /// Method declared in this class with the given name and descriptor.
    pub fn declared_method(&self, name: &[u8], descriptor: &[u8]) -> Option<&Member<'a>> {
        self.class
            .methods
            .iter()
            .find(|m| m.name == name && m.descriptor == descriptor)
    }

    /// Field declared in this class with the given name and descriptor.
    pub fn declared_field(&self, name: &[u8], descriptor: &[u8]) -> Option<&Member<'a>> {
        self.class
            .fields
            .iter()
            .find(|f| f.name == name && f.descriptor == descriptor)
    }

    /// `FieldInfo` (byte offset, access flags, slot) declared in this
    /// class, searching instance fields first then static fields.
    pub fn field_info(&self, name: &[u8], descriptor: &[u8]) -> Option<&FieldInfo<'a>> {
        self.instance_field_infos
            .iter()
            .chain(self.static_field_infos.iter())
            .find(|info| info.name == name && info.descriptor == descriptor)
    }

    /// Resolves a field reference to its declaring class and layout
    /// info, walking the superclass chain.  `FieldInfo::offset` is the
    /// byte offset inside the object for instance fields, or the offset
    /// inside the static field area for static fields (use
    /// `FieldInfo::slot`/`is_static` to distinguish).
    pub fn lookup_field(
        self: &Arc<Self>,
        name: &[u8],
        descriptor: &[u8],
    ) -> Option<(Arc<Self>, FieldInfo<'a>)> {
        if let Some(info) = self.field_info(name, descriptor) {
            return Some((Arc::clone(self), info.clone()));
        }
        self.super_klass
            .as_ref()
            .and_then(|super_k| super_k.lookup_field(name, descriptor))
    }

    /// Raw method lookup walking the superclass chain; returns the
    /// declaring class and the index into `InstanceKlass::class.methods`.
    /// Interface default methods are not covered — use the itable for
    /// `invokeinterface` targets.
    pub fn lookup_method(
        self: &Arc<Self>,
        name: &[u8],
        descriptor: &[u8],
    ) -> Option<(Arc<Self>, usize)> {
        if let Some(index) = self
            .class
            .methods
            .iter()
            .position(|m| m.name == name && m.descriptor == descriptor)
        {
            return Some((Arc::clone(self), index));
        }
        self.super_klass
            .as_ref()
            .and_then(|super_k| super_k.lookup_method(name, descriptor))
    }

    /// Virtual-dispatch lookup: first declaration in the superclass
    /// chain that is neither private nor static (JLS 8.4.8.1 hiding
    /// rules, same selection `LinkResolver::resolve_virtual_call`
    /// performs before consulting interfaces).
    pub fn lookup_virtual_method(
        self: &Arc<Self>,
        name: &[u8],
        descriptor: &[u8],
    ) -> Option<(Arc<Self>, usize)> {
        if let Some(index) = self.class.methods.iter().position(|m| {
            m.name == name
                && m.descriptor == descriptor
                && (m.accessFlags & 0x0002) == 0
                && (m.accessFlags & 0x0008) == 0
        }) {
            return Some((Arc::clone(self), index));
        }
        self.super_klass
            .as_ref()
            .and_then(|super_k| super_k.lookup_virtual_method(name, descriptor))
    }

    /// Vtable slot of a virtual method, mirroring
    /// `klassVtable::index_of_method` used by `LinkResolver`.
    pub fn vtable_index(&self, name: &[u8], descriptor: &[u8]) -> Option<usize> {
        let name = String::from_utf8_lossy(name);
        let sig = String::from_utf8_lossy(descriptor);
        self.vtable.index_of_method(&name, &sig)
    }

    /// Vtable entry at `index` (HotSpot `klassVtable::method_at`).
    pub fn vtable_entry(&self, index: usize) -> Option<&VtableEntry> {
        self.vtable.entry_at(index)
    }

    /// Resolves an `invokeinterface` target to the receiver's vtable
    /// slot through the itable, given the interface's internal name.
    pub fn itable_vtable_index(
        &self,
        interface_name: &[u8],
        name: &[u8],
        descriptor: &[u8],
    ) -> Option<usize> {
        let iface = std::str::from_utf8(interface_name).ok()?;
        let name = String::from_utf8_lossy(name);
        let sig = String::from_utf8_lossy(descriptor);
        self.itable.resolve_interface_method(iface, &name, &sig)
    }

    /// Structured view of a Fieldref/Methodref/InterfaceMethodref
    /// constant-pool index: kind plus owner/name/descriptor slices.
    /// This is what JIT stub sites and resolved cache entries consume.
    pub fn resolve_member_ref(&self, index: u16) -> io::Result<MemberRef<'a>> {
        let kind = match self.constant(index)? {
            Constant::FieldRef(..) => MemberRefKind::Field,
            Constant::MethodRef(..) => MemberRefKind::Method,
            Constant::InterfaceMethodRef(..) => MemberRefKind::InterfaceMethod,
            other => {
                return Err(Error::other(format!(
                    "Expected member reference constant #{index}, got {other:?}"
                )));
            }
        };
        let [owner, name, descriptor] = self.memberRef(index, kind == MemberRefKind::Field)?;
        Ok(MemberRef {
            kind,
            owner,
            name,
            descriptor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classfile::Member;

    fn member(access: u16, name: &'static [u8], desc: &'static [u8]) -> Member<'static> {
        Member {
            accessFlags: access,
            name,
            descriptor: desc,
            attributes: Vec::new(),
        }
    }

    fn class_file(
        name: &'static [u8],
        super_name: Option<&'static [u8]>,
        access: u16,
        fields: Vec<Member<'static>>,
        methods: Vec<Member<'static>>,
        constants: Vec<Constant<'static>>,
    ) -> ClassFile<'static> {
        ClassFile {
            majorVersion: 65,
            minorVersion: 0,
            accessFlags: access,
            name,
            superName: super_name,
            interfaces: Vec::new(),
            constants,
            fields,
            methods,
            attributes: Vec::new(),
        }
    }

    fn hierarchy() -> (Arc<InstanceKlass<'static>>, Arc<InstanceKlass<'static>>, Arc<InstanceKlass<'static>>) {
        let object = Arc::new(
            InstanceKlass::new(
                class_file(b"java/lang/Object", None, 0x0021, vec![], vec![], vec![]),
                None,
                vec![],
            )
            .unwrap(),
        );

        // Interface I { void m(); }
        let iface = Arc::new(
            InstanceKlass::new(
                class_file(
                    b"I",
                    Some(b"java/lang/Object".as_slice()),
                    0x0601,
                    vec![],
                    vec![member(0x0401, b"m", b"()V")],
                    vec![],
                ),
                Some(Arc::clone(&object)),
                vec![],
            )
            .unwrap(),
        );

        // class A extends Object implements I { int i; long l; static int s; void m(); }
        let a = Arc::new(
            InstanceKlass::new(
                class_file(
                    b"A",
                    Some(b"java/lang/Object".as_slice()),
                    0x0021,
                    vec![
                        member(0x0001, b"i", b"I"),
                        member(0x0001, b"l", b"J"),
                        member(0x0009, b"s", b"I"),
                    ],
                    vec![member(0x0001, b"m", b"()V"), member(0x000a, b"st", b"()V")],
                    vec![],
                ),
                Some(Arc::clone(&object)),
                vec![Arc::clone(&iface)],
            )
            .unwrap(),
        );

        // class B extends A { Object r; void m(); void n(); }
        let b = Arc::new(
            InstanceKlass::new(
                class_file(
                    b"B",
                    Some(b"A".as_slice()),
                    0x0021,
                    vec![member(0x0001, b"r", b"Ljava/lang/Object;")],
                    vec![member(0x0001, b"m", b"()V"), member(0x0001, b"n", b"()V")],
                    vec![
                        Constant::Unusable,
                        Constant::Utf8(b"java/lang/Object"),
                        Constant::Class(1),
                        Constant::Utf8(b"x"),
                        Constant::Utf8(b"I"),
                        Constant::NameAndType(3, 4),
                        Constant::FieldRef(2, 5),
                    ],
                ),
                Some(Arc::clone(&a)),
                vec![],
            )
            .unwrap(),
        );
        (object, a, b)
    }

    #[test]
    fn super_depth_and_subclass_queries() {
        let (object, a, b) = hierarchy();
        assert_eq!(object.super_depth(), 0);
        assert_eq!(a.super_depth(), 1);
        assert_eq!(b.super_depth(), 2);
        assert!(b.is_subclass_of(b"B"));
        assert!(b.is_subclass_of(b"A"));
        assert!(b.is_subclass_of(b"java/lang/Object"));
        assert!(!b.is_subclass_of(b"I"));
        assert!(!b.is_subclass_of(b"Unrelated"));
    }

    #[test]
    fn interface_queries_cover_superclass_chain() {
        let (_, a, b) = hierarchy();
        assert!(a.implements_interface(b"I"));
        assert!(b.implements_interface(b"I"));
        assert!(!a.implements_interface(b"java/io/Serializable"));
        assert!(!a.is_interface());
    }

    #[test]
    fn instance_field_offsets_match_layout() {
        let (_, a, b) = hierarchy();
        // A: i fills the klass gap at 12, l (8-byte) at 16.
        assert_eq!(a.instance_field_infos[0].offset, 12);
        assert_eq!(a.instance_field_infos[1].offset, 16);
        // B's first field starts at A's instance_size (24).
        assert_eq!(b.instance_field_infos[0].offset, 24);
        assert_eq!(a.instance_size, 24);
        assert_eq!(b.instance_size, 32);
    }

    #[test]
    fn lookup_field_walks_hierarchy() {
        let (_, a, b) = hierarchy();
        let b_arc = Arc::clone(&b);
        let (holder, info) = b_arc.lookup_field(b"i", b"I").unwrap();
        assert!(Arc::ptr_eq(&holder, &a));
        assert_eq!(info.offset, 12);
        assert!(!info.is_static());
        assert_eq!(info.kind(), Some(crate::share::oops::fieldInfo::FieldKind::Int32));

        let (holder, info) = b_arc.lookup_field(b"r", b"Ljava/lang/Object;").unwrap();
        assert!(Arc::ptr_eq(&holder, &b));
        assert_eq!(info.offset, 24);

        // Static field of A resolved through B.
        let (holder, info) = b_arc.lookup_field(b"s", b"I").unwrap();
        assert!(Arc::ptr_eq(&holder, &a));
        assert!(info.is_static());

        assert!(b_arc.lookup_field(b"missing", b"I").is_none());
    }

    #[test]
    fn vtable_inherits_and_overrides() {
        let (_, a, b) = hierarchy();
        // A: m()V -> slot 0 (st is static, excluded).
        assert_eq!(a.vtable_index(b"m", b"()V"), Some(0));
        // B overrides m in place, appends n.
        assert_eq!(b.vtable_index(b"m", b"()V"), Some(0));
        assert_eq!(b.vtable_index(b"n", b"()V"), Some(1));
        let entry = b.vtable_entry(0).unwrap();
        assert_eq!(entry.holder_klass(), "B");
        assert_eq!(entry.method_name(), "m");
        assert_eq!(entry.signature(), "()V");

        // Virtual lookup skips private/static declarations.
        let (holder, index) = Arc::clone(&b).lookup_virtual_method(b"m", b"()V").unwrap();
        assert!(Arc::ptr_eq(&holder, &b));
        assert_eq!(b.class.methods[index].name, b"m".as_slice());
        // Raw lookup finds inherited methods too.
        let (_, _) = Arc::clone(&b).lookup_method(b"m", b"()V").unwrap();
    }

    #[test]
    fn itable_resolves_interface_methods() {
        let (_, _, b) = hierarchy();
        // I.m()V is inherited by B through A; itable maps it to a vtable slot.
        let v_idx = b.itable_vtable_index(b"I", b"m", b"()V");
        // B declares its own m()V, so the itable resolves to slot 0.
        assert_eq!(v_idx, Some(0));
        assert!(b.itable_vtable_index(b"NoSuchIface", b"m", b"()V").is_none());
    }

    #[test]
    fn resolve_member_ref_returns_triple() {
        let (_, _, b) = hierarchy();
        let reference = b.resolve_member_ref(6).unwrap();
        assert_eq!(reference.kind, MemberRefKind::Field);
        assert_eq!(reference.owner, b"java/lang/Object".as_slice());
        assert_eq!(reference.name, b"x".as_slice());
        assert_eq!(reference.descriptor, b"I".as_slice());
        assert!(b.resolve_member_ref(1).is_err());
    }

    #[test]
    fn array_class_queries() {
        let array = InstanceKlass::new(
            class_file(b"[I", Some(b"java/lang/Object".as_slice()), 0x0411, vec![], vec![], vec![]),
            None,
            vec![],
        )
        .unwrap();
        assert!(array.is_array());
        assert_eq!(array.array_dimension(), 1);
        assert_eq!(array.array_element_descriptor(), Some(&b"I"[..]));

        let multi = InstanceKlass::new(
            class_file(
                b"[[Ljava/lang/String;",
                Some(b"java/lang/Object".as_slice()),
                0x0411,
                vec![],
                vec![],
                vec![],
            ),
            None,
            vec![],
        )
        .unwrap();
        assert_eq!(multi.array_dimension(), 2);
        assert_eq!(multi.array_element_descriptor(), Some(&b"[Ljava/lang/String;"[..]));

        let (_, _, b) = hierarchy();
        assert!(!b.is_array());
        assert_eq!(b.array_element_descriptor(), None);
    }

    #[test]
    fn access_flag_queries() {
        let (_, a, _) = hierarchy();
        assert!(!a.is_interface());
        assert!(!a.is_abstract());
        assert!(!a.is_final());
        let iface_klass = a.interfaces[0].clone();
        assert!(iface_klass.is_interface());
        assert!(iface_klass.is_abstract());
        assert!(!iface_klass.is_final());
    }
}
