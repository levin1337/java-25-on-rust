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

use super::fieldInfo::FieldInfo;
use super::fieldLayout::FieldLayout;
use super::klassVtable::{KlassItable, KlassVtable};
use super::method::Code;
use super::objLayout::ObjLayout;
use crate::classfile::{ClassFile, Constant};
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

        let total_instance_fields = instance_slot;

        // Compute memory layout and field byte offsets
        let layout = ObjLayout::default();
        let super_instance_size = super_klass.as_ref().map_or(0, |s| s.instance_size);
        let layout_res = FieldLayout::compute_instance_layout(
            &layout,
            super_instance_size,
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
        // superinterface, direct or indirect, in the itable).
        let mut itable = KlassItable::new(class_name_str);
        let mut all_interfaces: Vec<Arc<InstanceKlass<'a>>> = Vec::new();
        let mut pending: Vec<Arc<InstanceKlass<'a>>> = interfaces.clone();
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
                    if let Some(v_idx) = vtable.index_of_method(&m_name, &m_sig) {
                        itable.add_entry(iface_name.clone(), m_name, m_sig, v_idx);
                    }
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
            "Missing instance field {}{}",
            String::from_utf8_lossy(name),
            String::from_utf8_lossy(descriptor)
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
        let (owner, member) = match (self.constant(index)?, field) {
            (Constant::FieldRef(owner, member), true)
            | (Constant::MethodRef(owner, member), false)
            | (Constant::InterfaceMethodRef(owner, member), false) => (*owner, *member),
            _ => return Err(Error::other("Invalid member reference")),
        };
        match (self.constant(owner)?, self.constant(member)?) {
            (Constant::Class(owner), Constant::NameAndType(name, descriptor)) => Ok([
                self.utf8(*owner)?,
                self.utf8(*name)?,
                self.utf8(*descriptor)?,
            ]),
            _ => Err(Error::other("Invalid member reference")),
        }
    }
}
