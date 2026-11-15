/*
 * Port of OpenJDK 25 HotSpot JVMCIObject (share/jvmci/jvmciObject.hpp)
 *
 * JVMCIObject is a lightweight wrapper around a JNI local handle
 * (`jobject`) plus a flag recording which heap the referent lives in
 * (HotSpot heap vs. JVMCI shared library heap).  In this port JNI
 * handles are indexes into a process-wide object store that backs both
 * heaps; the `is_hotspot` flag preserves the heap distinction used by
 * the C++ code for access checks.
 */

use crate::share::oops::oop::Oop;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// JNI handle value; 0 is the null handle.
pub type Jobject = u64;

/// A field value stored inside a `JvmciHeapValue::Object`.
#[derive(Clone, Debug, PartialEq)]
pub enum JvmciValue {
    Null,
    Object(JvmciObject),
    Int(i32),
    Long(i64),
    Bool(bool),
    Char(u16),
    Float(f32),
    Double(f64),
    Utf8(String),
    ByteArray(Vec<u8>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
    BoolArray(Vec<bool>),
    ObjectArray(Vec<JvmciObject>),
}

impl JvmciValue {
    pub fn as_object(&self) -> Option<JvmciObject> {
        match self {
            Self::Object(o) => Some(*o),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Bool(v) => Some(*v as i32),
            Self::Char(v) => Some(*v as i32),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Long(v) => Some(*v),
            Self::Int(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            Self::Int(v) => Some(*v != 0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Utf8(s) => Some(s),
            _ => None,
        }
    }
}

/// Contents of a JVMCI heap object reachable through a `JvmciObject`.
#[derive(Clone, Debug)]
pub enum JvmciHeapValue {
    Null,
    Utf8String(String),
    ByteArray(Vec<u8>),
    CharArray(Vec<u16>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
    BoolArray(Vec<bool>),
    ObjectArray(Vec<JvmciObject>),
    /// A HotSpot heap object (oop).
    Oop(Oop),
    /// A JVMCI Java object with named fields (the fields map plays the
    /// role of the object's instance memory for the subset of the JVMCI
    /// Java API modelled here).
    Object {
        class_name: String,
        fields: BTreeMap<String, JvmciValue>,
    },
    /// Raw pointer-sized payload (metadata handles, addresses).
    Raw(u64),
}

struct HeapEntry {
    is_hotspot: bool,
    value: JvmciHeapValue,
}

struct ObjectStore {
    map: HashMap<u64, HeapEntry>,
    free: Vec<u64>,
}

impl ObjectStore {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            free: Vec::new(),
        }
    }
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static OBJECT_STORE: OnceLock<Mutex<ObjectStore>> = OnceLock::new();

fn store() -> &'static Mutex<ObjectStore> {
    OBJECT_STORE.get_or_init(|| Mutex::new(ObjectStore::new()))
}

fn alloc_entry(is_hotspot: bool, value: JvmciHeapValue) -> u64 {
    let mut guard = store().lock().unwrap();
    let handle = if let Some(h) = guard.free.pop() {
        h
    } else {
        NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
    };
    guard.map.insert(handle, HeapEntry { is_hotspot, value });
    handle
}

fn free_entry(handle: u64) {
    let mut guard = store().lock().unwrap();
    if guard.map.remove(&handle).is_some() {
        guard.free.push(handle);
    }
}

fn get_entry(handle: u64) -> Option<HeapEntry> {
    let guard = store().lock().unwrap();
    guard.map.get(&handle).map(|e| HeapEntry {
        is_hotspot: e.is_hotspot,
        value: e.value.clone(),
    })
}

fn get_entry_mut<R>(handle: u64, f: impl FnOnce(&mut HeapEntry) -> R) -> Option<R> {
    let mut guard = store().lock().unwrap();
    guard.map.get_mut(&handle).map(f)
}

/// Allocate a new JVMCI heap object and return its handle wrapper.
pub fn alloc_object(is_hotspot: bool, value: JvmciHeapValue) -> JvmciObject {
    JvmciObject {
        jobject: alloc_entry(is_hotspot, value),
        is_hotspot,
    }
}

/// `JNIEnv::DeleteLocalRef` analogue for JVMCI-scoped handles.
pub fn free_object(obj: JvmciObject) {
    if obj.jobject != 0 {
        free_entry(obj.jobject);
    }
}

/// Resolve a `JvmciObject` to its heap contents.
pub fn resolve_object(obj: JvmciObject) -> Option<JvmciHeapValue> {
    if obj.jobject == 0 {
        return Some(JvmciHeapValue::Null);
    }
    get_entry(obj.jobject).map(|e| e.value)
}

/// Check whether `obj` belongs to the requested heap.
pub fn object_heap(obj: JvmciObject) -> Option<bool> {
    if obj.jobject == 0 {
        return None;
    }
    get_entry(obj.jobject).map(|e| e.is_hotspot)
}

/// Mutate the field map of a `JvmciHeapValue::Object`.
pub fn with_object_fields<R>(
    obj: JvmciObject,
    f: impl FnOnce(&mut BTreeMap<String, JvmciValue>, &str) -> R,
) -> Option<R> {
    get_entry_mut(obj.jobject, |e| match &mut e.value {
        JvmciHeapValue::Object {
            class_name,
            fields,
        } => f(fields, class_name),
        _ => unreachable!("with_object_fields on non-object"),
    })
}

/// Mutate a heap entry directly (used by the Env array mutators).
pub fn with_entry_mut<R>(handle: u64, f: impl FnOnce(&mut HeapEntryView) -> R) -> Option<R> {
    let mut guard = store().lock().unwrap();
    guard.map.get_mut(&handle).map(|e| {
        let mut view = HeapEntryView {
            is_hotspot: e.is_hotspot,
            value: &mut e.value,
        };
        f(&mut view)
    })
}

/// Mutable view of a heap entry.
pub struct HeapEntryView<'a> {
    pub is_hotspot: bool,
    pub value: &'a mut JvmciHeapValue,
}

/// Read one named field of a JVMCI heap object.
pub fn get_object_field(obj: JvmciObject, name: &str) -> Option<JvmciValue> {
    with_object_fields(obj, |fields, _| fields.get(name).cloned())
    .flatten()
}

/// Write one named field of a JVMCI heap object.
pub fn set_object_field(obj: JvmciObject, name: &str, value: JvmciValue) -> bool {
    with_object_fields(obj, |fields, _| {
        fields.insert(name.to_string(), value);
    })
    .is_some()
}

/// The class name recorded for a `JvmciHeapValue::Object`.
pub fn object_class_name(obj: JvmciObject) -> Option<String> {
    match resolve_object(obj)? {
        JvmciHeapValue::Object { class_name, .. } => Some(class_name),
        JvmciHeapValue::Utf8String(s) => Some(format!("java/lang/String({s})")),
        JvmciHeapValue::Oop(_) => Some("<hotspot oop>".to_string()),
        _ => None,
    }
}

/// Wrapper around a `jobject` handle plus the heap it belongs to.
/// Mirrors `JVMCIObject` in jvmciObject.hpp; `JvmciArray`,
/// `JvmciPrimitiveArray` and `JvmciObjectArray` are the typed views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JvmciObject {
    pub jobject: Jobject,
    pub is_hotspot: bool,
}

impl JvmciObject {
    pub const NULL: Self = Self {
        jobject: 0,
        is_hotspot: false,
    };

    pub fn create(obj: Jobject, is_hotspot: bool) -> Self {
        Self {
            jobject: obj,
            is_hotspot,
        }
    }

    #[inline]
    pub fn as_jobject(&self) -> Jobject {
        self.jobject
    }

    #[inline]
    pub fn as_jweak(&self) -> Jobject {
        self.jobject
    }

    #[inline]
    pub fn as_jstring(&self) -> Jobject {
        self.jobject
    }

    #[inline]
    pub fn is_hotspot(&self) -> bool {
        self.is_hotspot
    }

    #[inline]
    pub fn is_null(&self) -> bool {
        self.jobject == 0
    }

    #[inline]
    pub fn is_non_null(&self) -> bool {
        self.jobject != 0
    }

    /// Heap contents of the referent, if the handle is live.
    pub fn heap_value(&self) -> Option<JvmciHeapValue> {
        resolve_object(*self)
    }

    /// Read a named instance field (JVMCI Java object model).
    pub fn field(&self, name: &str) -> Option<JvmciValue> {
        get_object_field(*self, name)
    }

    /// Write a named instance field.
    pub fn set_field(&self, name: &str, value: JvmciValue) -> bool {
        set_object_field(*self, name, value)
    }
}

impl Default for JvmciObject {
    fn default() -> Self {
        Self::NULL
    }
}

/// `JVMCIArray` view: any JNI array handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JvmciArray(pub JvmciObject);

impl JvmciArray {
    pub fn as_jobject(&self) -> Jobject {
        self.0.jobject
    }

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

/// `JVMCIPrimitiveArray` view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JvmciPrimitiveArray(pub JvmciObject);

impl JvmciPrimitiveArray {
    pub fn as_jobject(&self) -> Jobject {
        self.0.jobject
    }

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

/// `JVMCIObjectArray` view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JvmciObjectArray(pub JvmciObject);

impl JvmciObjectArray {
    pub fn as_jobject(&self) -> Jobject {
        self.0.jobject
    }

    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl From<JvmciObject> for JvmciArray {
    fn from(o: JvmciObject) -> Self {
        Self(o)
    }
}

impl From<JvmciObject> for JvmciPrimitiveArray {
    fn from(o: JvmciObject) -> Self {
        Self(o)
    }
}

impl From<JvmciObject> for JvmciObjectArray {
    fn from(o: JvmciObject) -> Self {
        Self(o)
    }
}

/// Allocate a JVMCI Java object of `class_name` with `fields`.
pub fn new_jvmci_object(class_name: &str, fields: BTreeMap<String, JvmciValue>) -> JvmciObject {
    alloc_object(
        false,
        JvmciHeapValue::Object {
            class_name: class_name.to_string(),
            fields,
        },
    )
}

/// Allocate a UTF-8 string object (the JVMCI `create_string` backing).
pub fn new_jvmci_string(s: &str) -> JvmciObject {
    alloc_object(false, JvmciHeapValue::Utf8String(s.to_string()))
}

/// Allocate a primitive byte array object.
pub fn new_jvmci_byte_array(bytes: Vec<u8>) -> JvmciObject {
    alloc_object(false, JvmciHeapValue::ByteArray(bytes))
}
